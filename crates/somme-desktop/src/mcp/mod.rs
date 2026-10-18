//! Somme's MCP server, so agents like Claude Code can run scripts on the
//! user's saved servers. Agents start `Somme --mcp` (see `bridge`), which
//! talks to the running app, or to the background service while the app is
//! closed (see `service`). It is off until the user turns it on, and every
//! script is written to the script log before it runs.

mod bridge;
pub mod install;
mod log;
mod protocol;
mod server;
mod service;
pub mod transport;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use futures::StreamExt as _;
use futures::channel::mpsc::{self, UnboundedSender};
use gpui_kit::{App, AppContext as _, Context, Entity, Global};

pub use log::Run;
use log::{Record, ScriptLog};

use crate::services::Services;
use crate::settings::Settings;

/// Runs kept in the app; the file keeps them all.
const SHOWN: usize = 200;

pub enum AgentEvent {
    Connections(usize),
    Record(Record),
}

/// What agents are doing, for the settings page.
pub struct Agents {
    /// Newest first.
    pub runs: Vec<Run>,
    /// Runs started since Somme opened that have not ended.
    pub live: HashSet<String>,
    pub connections: usize,
    /// Why agents can not connect.
    pub problem: Option<String>,
    /// Why the background service could not be set up.
    pub service_problem: Option<String>,
    /// Whether the background service is set up, once known.
    service: Option<bool>,
    installing: bool,
    dir: PathBuf,
    server: Option<server::Server>,
    log: Arc<ScriptLog>,
    services: Arc<Services>,
    events: UnboundedSender<AgentEvent>,
}

struct GlobalAgents(Entity<Agents>);

impl Global for GlobalAgents {}

/// Holds the socket, the log and the lock. Only this user may open it.
fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("agents")
}

/// For `Somme --mcp`. Gives the exit code.
pub fn run_bridge() -> i32 {
    match crate::services::data_dir() {
        Ok(data_dir) => bridge::run(&dir(&data_dir)),
        Err(err) => {
            eprintln!("somme: {err}");
            1
        }
    }
}

/// For `Somme --service`.
pub fn run_service() -> i32 {
    service::run()
}

/// For `Somme --service-worker`.
pub fn run_worker() -> i32 {
    service::run_worker()
}

pub use service::{SERVICE_ARG, WORKER_ARG};

/// Held while the app runs, so the background service leaves the agents to it.
static APP_LOCK: OnceLock<std::fs::File> = OnceLock::new();

/// Takes the agents from the background service, if it has them, so only one
/// Somme opens the database. Call before the database opens.
pub fn claim() {
    let Ok(data_dir) = crate::services::data_dir() else { return };
    if let Some(lock) = service::claim(&dir(&data_dir)) {
        let _ = APP_LOCK.set(lock);
    }
}

pub fn init(services: Arc<Services>, cx: &mut App) {
    let dir = dir(&services.data_dir);
    let problem = make_private_dir(&dir).err().map(|err| format!("Could not make {}: {err}", dir.display()));
    let log = Arc::new(ScriptLog::new(&dir));
    let (events, mut incoming) = mpsc::unbounded();
    let agents = cx.new(|_| Agents {
        runs: log.recent(SHOWN),
        live: HashSet::new(),
        connections: 0,
        problem,
        service_problem: None,
        service: None,
        installing: false,
        dir,
        server: None,
        log,
        services,
        events,
    });
    cx.set_global(GlobalAgents(agents.clone()));
    let weak = agents.downgrade();
    cx.spawn(async move |cx| {
        while let Some(event) = incoming.next().await {
            if weak.update(cx, |agents, cx| agents.apply(event, cx)).is_err() {
                break;
            }
        }
    })
    .detach();
    cx.observe_global::<Settings>(|cx| Agents::global(cx).update(cx, |agents, cx| agents.sync(cx))).detach();
    agents.update(cx, |agents, cx| agents.sync(cx));
    cx.on_app_quit(move |cx| {
        // Removes the socket, so `Somme --mcp` says plainly that Somme is closed.
        drop(agents.update(cx, |agents, _| agents.server.take()));
        async {}
    })
    .detach();
}

impl Agents {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalAgents>().0.clone()
    }

    pub fn is_on(&self) -> bool {
        self.server.is_some()
    }

    pub fn log_path(&self) -> &Path {
        self.log.path()
    }

    /// Whether agents keep working while Somme is closed.
    pub fn service_on(&self) -> bool {
        self.service == Some(true)
    }

    pub fn service_busy(&self) -> bool {
        self.installing
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        self.sync_server(cx);
        self.sync_service(cx);
    }

    /// Sets the background service up, or removes it, to match the setting.
    /// When that fails, the setting goes back.
    fn sync_service(&mut self, cx: &mut Context<Self>) {
        let wanted = Settings::get(cx).agent_service;
        if self.installing || self.service == Some(wanted) {
            return;
        }
        self.installing = true;
        cx.notify();
        let dir = self.dir.clone();
        cx.spawn(async move |agents, cx| {
            let result = cx.background_spawn(async move { install::apply(&dir, wanted) }).await;
            let _ = agents.update(cx, |agents, cx| {
                agents.installing = false;
                match result {
                    Ok(()) => {
                        agents.service = Some(wanted);
                        agents.service_problem = None;
                    }
                    Err(err) => {
                        tracing::warn!("{err:#}");
                        agents.service = Some(!wanted);
                        agents.service_problem = Some(format!("Could not {} the background service: {err:#}", if wanted { "set up" } else { "remove" }));
                        Settings::update(cx, |s| s.agent_service = !wanted);
                    }
                }
                cx.notify();
                // The setting may have changed meanwhile.
                agents.sync_service(cx);
            });
        })
        .detach();
    }

    /// Starts or stops taking agents, to match the setting.
    fn sync_server(&mut self, cx: &mut Context<Self>) {
        let wanted = Settings::get(cx).agent_access;
        if wanted == self.server.is_some() {
            return;
        }
        if wanted {
            match server::start(&self.dir, self.services.clone(), self.log.clone(), self.events.clone(), None) {
                Ok(server) => {
                    self.server = Some(server);
                    self.problem = None;
                }
                Err(err) => self.problem = Some(format!("Agents can not connect: {err}.")),
            }
        } else {
            self.server = None;
            self.connections = 0;
        }
        cx.notify();
    }

    fn apply(&mut self, event: AgentEvent, cx: &mut Context<Self>) {
        match event {
            AgentEvent::Connections(count) => self.connections = if self.is_on() { count } else { 0 },
            AgentEvent::Record(record) => {
                let id = record.id().to_owned();
                match record {
                    Record::Start { .. } => {
                        self.live.insert(id);
                        if let Some(run) = Run::from_start(record) {
                            self.runs.insert(0, run);
                            self.runs.truncate(SHOWN);
                        }
                    }
                    Record::End { .. } => {
                        self.live.remove(&id);
                        if let Some(run) = self.runs.iter_mut().find(|run| run.id == id) {
                            run.finish(record);
                        }
                    }
                }
            }
        }
        cx.notify();
    }
}

#[cfg(unix)]
pub fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
pub fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}
