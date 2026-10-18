//! Takes agents' connections (see `transport`) and links the tools to the
//! saved hosts.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::channel::mpsc::UnboundedSender;
use serde_json::Value;
use somme_application::AppError;
use somme_application::ports::GatewayError;
use somme_domain::Id;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::task::JoinSet;

use super::AgentEvent;
use super::log::{HostRef, Record, ScriptLog, kept};
use super::protocol::{self, Backend, HostInfo, ScriptRun, Session};
use super::transport::{Listener, Mark, Stream};
use crate::runtime;
use crate::services::Services;

/// Room for a 1 MB script, written out as JSON.
const MAX_MESSAGE: usize = 4 * 1024 * 1024;
/// Output an agent gets back from each stream.
const OUTPUT_LIMIT: usize = 256 * 1024;

const LOCKED: &str = "Somme is locked. Ask the user to unlock it with their master password, then try again.";

/// What the Somme app sends the background service to take over from it.
pub const YIELD: &str = r#"{"somme":"yield"}"#;

/// The running server. Dropping it stops it: agents are cut off, and their
/// running scripts are stopped and logged as such.
pub struct Server {
    task: tokio::task::JoinHandle<()>,
    mark: Mark,
}

impl Drop for Server {
    fn drop(&mut self) {
        // Removed now, before another server can bind the same name.
        self.mark.remove();
        self.task.abort();
    }
}

#[derive(Clone)]
struct Shared {
    services: Arc<Services>,
    log: Arc<ScriptLog>,
    events: UnboundedSender<AgentEvent>,
    connections: Arc<AtomicUsize>,
    /// Told when the app asks for the agents back. Only the service has one.
    yields: Option<UnboundedSender<()>>,
}

/// Starts taking agents for the agents folder `dir`.
pub fn start(
    dir: &Path,
    services: Arc<Services>,
    log: Arc<ScriptLog>,
    events: UnboundedSender<AgentEvent>,
    yields: Option<UnboundedSender<()>>,
) -> std::io::Result<Server> {
    let handle = runtime::handle();
    let (listener, mark) = {
        let _entered = handle.enter();
        Listener::bind(dir)?
    };
    let shared = Shared {
        services,
        log,
        events,
        connections: Arc::default(),
        yields,
    };
    let task = handle.spawn(accept(listener, shared));
    Ok(Server { task, mark })
}

async fn accept(listener: Listener, shared: Shared) {
    // Dropping the set, when this task is stopped, ends every connection.
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(Some(stream)) => {
                    connections.spawn(connection(stream, shared.clone()));
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(%err, "agent connection failed");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            },
            Some(_) = connections.join_next() => {}
        }
    }
}

async fn connection(stream: Stream, shared: Shared) {
    let count = shared.connections.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = shared.events.unbounded_send(AgentEvent::Connections(count));
    let (read, mut write) = tokio::io::split(stream);
    let (answers, mut outbox) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        while let Some(answer) = outbox.recv().await {
            let Ok(mut line) = serde_json::to_vec(&answer) else { continue };
            line.push(b'\n');
            if write.write_all(&line).await.is_err() {
                break;
            }
        }
    });
    let session = Arc::new(Session::default());
    let backend: Arc<dyn Backend> = Arc::new(shared.clone());
    // Each request runs on its own, so a long script does not hold up the rest.
    let mut requests = JoinSet::new();
    let mut reader = BufReader::new(read);
    let mut line = Vec::new();
    loop {
        line.clear();
        let limit = (MAX_MESSAGE + 1) as u64;
        match (&mut reader).take(limit).read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) if line.len() > MAX_MESSAGE => {
                let _ = answers.send(protocol::error(Value::Null, -32600, "Message too large"));
                break;
            }
            Ok(_) => {}
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if line.trim_ascii() == YIELD.as_bytes() {
            if let Some(yields) = &shared.yields {
                let _ = yields.unbounded_send(());
            }
            continue;
        }
        let message = match serde_json::from_slice(&line) {
            Ok(message) => message,
            Err(_) => {
                let _ = answers.send(protocol::error(Value::Null, -32700, "Parse error"));
                continue;
            }
        };
        let (session, backend, answers) = (session.clone(), backend.clone(), answers.clone());
        requests.spawn(async move {
            if let Some(answer) = session.handle(backend.as_ref(), message).await {
                let _ = answers.send(answer);
            }
        });
    }
    // Scripts keep going when the agent leaves, so they end in the log.
    drop(answers);
    while requests.join_next().await.is_some() {}
    let _ = writer.await;
    let count = shared.connections.fetch_sub(1, Ordering::SeqCst) - 1;
    let _ = shared.events.unbounded_send(AgentEvent::Connections(count));
}

impl Shared {
    /// Unlocks with a remembered master password, the first time.
    async fn unlocked(&self) -> bool {
        let _ = self.services.vault.status().await;
        self.services.vault.is_unlocked()
    }

    fn record(&self, record: Record) {
        if let Err(err) = self.log.write(&record) {
            tracing::error!(%err, "could not write the script log");
        }
        let _ = self.events.unbounded_send(AgentEvent::Record(record));
    }
}

/// Logs a run's end when it is dropped before it ends, as when agents are
/// turned off or the app takes over from the background service.
struct Unfinished<'a> {
    shared: &'a Shared,
    id: Option<String>,
}

impl Drop for Unfinished<'_> {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else { return };
        self.shared.record(Record::End {
            id,
            at: now_ms(),
            exit_status: None,
            exit_signal: None,
            timed_out: false,
            error: Some("Somme stopped it before it ended: agents were turned off, or Somme quit or took over.".into()),
            stdout: String::new(),
            stderr: String::new(),
        });
    }
}

#[async_trait]
impl Backend for Shared {
    async fn hosts(&self) -> Result<Vec<HostInfo>, String> {
        if !self.unlocked().await {
            return Err(LOCKED.to_owned());
        }
        let failed = |err: AppError| format!("Somme could not read the saved servers: {err}");
        let hosts = self.services.hosts.list().await.map_err(failed)?;
        let groups = self.services.groups.list().await.map_err(failed)?;
        let identities = self.services.identities.list().await.map_err(failed)?;
        Ok(hosts
            .into_iter()
            .map(|host| HostInfo {
                id: host.id.to_string(),
                user: host
                    .identity_id
                    .and_then(|id| identities.iter().find(|i| i.id == id))
                    .map(|i| i.username.clone())
                    .or(host.username),
                group: host.group_id.and_then(|id| groups.iter().find(|g| g.id == id)).map(|g| g.name.clone()),
                name: host.label,
                address: host.address,
                port: host.port,
                tags: host.tags,
            })
            .collect())
    }

    async fn run(&self, agent: &str, host: &HostInfo, script: &str, timeout: Duration) -> Result<ScriptRun, String> {
        if !self.unlocked().await {
            return Err(LOCKED.to_owned());
        }
        let host_id: Id = host.id.parse().map_err(|_| "That server id is not valid.".to_owned())?;
        let id = uuid::Uuid::new_v4().to_string();
        let start = Record::Start {
            id: id.clone(),
            at: now_ms(),
            agent: agent.to_owned(),
            host: HostRef {
                id: host.id.clone(),
                name: host.name.clone(),
                address: match &host.user {
                    Some(user) => format!("{user}@{}:{}", host.address, host.port),
                    None => format!("{}:{}", host.address, host.port),
                },
            },
            script: script.to_owned(),
            timeout_seconds: timeout.as_secs(),
        };
        // No log, no run.
        self.log
            .write(&start)
            .map_err(|err| format!("Somme could not write its script log ({err}), so the script did not run."))?;
        let _ = self.events.unbounded_send(AgentEvent::Record(start));
        let mut unfinished = Unfinished {
            shared: self,
            id: Some(id.clone()),
        };

        let began = Instant::now();
        let result = self.services.scripts.run(host_id, script, timeout, OUTPUT_LIMIT).await;
        let seconds = began.elapsed().as_secs_f64();
        unfinished.id = None;
        let (end, answer) = match result {
            Ok(out) => {
                let run = ScriptRun {
                    exit_status: out.exit_status,
                    exit_signal: out.exit_signal,
                    timed_out: out.timed_out,
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                    seconds,
                };
                let end = Record::End {
                    id,
                    at: now_ms(),
                    exit_status: run.exit_status,
                    exit_signal: run.exit_signal.clone(),
                    timed_out: run.timed_out,
                    error: None,
                    stdout: kept(&run.stdout),
                    stderr: kept(&run.stderr),
                };
                (end, Ok(run))
            }
            Err(err) => {
                let message = explain(&err, &host.name);
                let end = Record::End {
                    id,
                    at: now_ms(),
                    exit_status: None,
                    exit_signal: None,
                    timed_out: false,
                    error: Some(message.clone()),
                    stdout: String::new(),
                    stderr: String::new(),
                };
                (end, Err(message))
            }
        };
        self.record(end);
        answer
    }
}

/// Why a script could not start, in words an agent can pass on.
fn explain(err: &AppError, host: &str) -> String {
    match err {
        AppError::VaultLocked => LOCKED.to_owned(),
        AppError::Gateway(GatewayError::HostKeyRejected) => format!(
            "Somme does not trust {host}'s host key yet, or the key changed. Ask the user to connect to {host} once in Somme and check the key."
        ),
        AppError::Gateway(GatewayError::Cancelled) => format!(
            "{host} asked for a password, passphrase or code that is not saved in Somme. Ask the user to save it in the host, or use a key."
        ),
        AppError::Gateway(GatewayError::AuthFailed(who)) => {
            format!("{host} refused the login for {who}. The saved password or key does not work.")
        }
        other => format!("Could not run the script on {host}: {other}"),
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
