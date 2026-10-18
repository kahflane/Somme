//! `Somme --service`: keeps agents working while the app is closed. It starts
//! at login (see `install`) and only watches. Whenever agents are on and the
//! app is closed, it starts `Somme --service-worker`, which takes the agents.
//!
//! Only one Somme may open the database, so whoever holds `app.lock` in the
//! agents folder opens it. The app holds it for as long as it runs. When the
//! app opens, it asks the worker to give the agents back, and the worker
//! quits, which also closes the database for sure.

use std::fs::File;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fs4::fs_std::FileExt as _;
use futures::StreamExt as _;
use futures::channel::mpsc;

use super::log::ScriptLog;
use super::{dir, make_private_dir, server, transport};
use crate::runtime;
use crate::services::{Services, data_dir};
use crate::settings::Settings;

pub const SERVICE_ARG: &str = "--service";
pub const WORKER_ARG: &str = "--service-worker";

const LOCK: &str = "app.lock";
/// The worker quit because the app or another worker has the agents.
const BUSY: i32 = 2;

/// Takes `app.lock`, if no one else has it.
pub fn try_lock(dir: &Path) -> io::Result<Option<File>> {
    let file = File::options().create(true).truncate(false).write(true).open(dir.join(LOCK))?;
    Ok(file.try_lock_exclusive()?.then_some(file))
}

fn is_free(dir: &Path) -> bool {
    match try_lock(dir) {
        Ok(Some(file)) => {
            let _ = fs4::fs_std::FileExt::unlock(&file);
            true
        }
        _ => false,
    }
}

/// For the app, before it opens the database: takes `app.lock`, asking the
/// worker to give it up if it has it. Gives up after a while, as when another
/// copy of the app has it.
pub fn claim(dir: &Path) -> Option<File> {
    make_private_dir(dir).ok()?;
    if let Ok(Some(file)) = try_lock(dir) {
        return Some(file);
    }
    let asked = runtime::block_on(async {
        tokio::time::timeout(Duration::from_secs(2), async {
            use tokio::io::AsyncWriteExt as _;
            let mut stream = transport::connect(dir).await.ok()?;
            stream.write_all(format!("{}\n", server::YIELD).as_bytes()).await.ok()?;
            stream.flush().await.ok()
        })
        .await
        .ok()
        .flatten()
        .is_some()
    });
    // A worker that is starting may not take agents yet.
    let wait = if asked { Duration::from_secs(10) } else { Duration::from_secs(3) };
    let began = Instant::now();
    while began.elapsed() < wait {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(Some(file)) = try_lock(dir) {
            tracing::info!("took the agents back from the background service");
            return Some(file);
        }
    }
    tracing::warn!("another Somme keeps the agents");
    None
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false).with_writer(io::stderr).init();
}

/// The data folder and its agents folder.
fn paths() -> anyhow::Result<(PathBuf, PathBuf)> {
    let data_dir = data_dir()?;
    let dir = dir(&data_dir);
    make_private_dir(&dir)?;
    Ok((data_dir, dir))
}

/// `Somme --service`. Gives the exit code: 0 when the user turned it off.
pub fn run() -> i32 {
    init_tracing();
    let (data_dir, dir) = match paths() {
        Ok(paths) => paths,
        Err(err) => {
            tracing::error!(%err, "cannot start");
            return 1;
        }
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            tracing::error!(%err, "cannot find the Somme program");
            return 1;
        }
    };
    tracing::info!("watching for when agents need Somme");
    loop {
        let settings = Settings::load(&data_dir);
        if !settings.agent_service {
            tracing::info!("turned off");
            return 0;
        }
        if settings.agent_access && is_free(&dir) {
            let began = Instant::now();
            let mut command = Command::new(&exe);
            command.arg(WORKER_ARG).stdin(Stdio::piped()).stdout(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt as _;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                command.creation_flags(CREATE_NO_WINDOW);
            }
            match command.spawn() {
                Ok(mut worker) => {
                    // The worker quits when its input closes, as when this ends.
                    // Waiting closes it, so it is held here.
                    let _input = worker.stdin.take();
                    match worker.wait() {
                        Ok(status) if status.success() || status.code() == Some(BUSY) => {}
                        Ok(status) => {
                            tracing::warn!(%status, "the worker stopped");
                            // Keeps a worker that cannot start from starting all the time.
                            if began.elapsed() < Duration::from_secs(30) {
                                std::thread::sleep(Duration::from_secs(10));
                            }
                        }
                        Err(err) => tracing::warn!(%err, "lost the worker"),
                    }
                }
                Err(err) => {
                    tracing::error!(%err, "cannot start the worker");
                    std::thread::sleep(Duration::from_secs(10));
                }
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// `Somme --service-worker`. Takes the agents until the app wants them, the
/// user turns them off, or the service goes away.
pub fn run_worker() -> i32 {
    init_tracing();
    let result = (|| {
        let (data_dir, dir) = paths()?;
        let Some(lock) = try_lock(&dir)? else { return Ok(BUSY) };
        runtime::init()?;
        runtime::block_on(serve(data_dir, dir))?;
        // Kept to here, past the database closing.
        drop(lock);
        Ok::<_, anyhow::Error>(0)
    })();
    result.unwrap_or_else(|err| {
        tracing::error!("{err:#}");
        1
    })
}

async fn serve(data_dir: PathBuf, dir: PathBuf) -> anyhow::Result<()> {
    let (stop, mut stopped) = mpsc::unbounded::<()>();
    std::thread::spawn(move || {
        let _ = io::stdin().read_to_end(&mut Vec::new());
        let _ = stop.unbounded_send(());
    });
    // No one shows these here.
    let (backend_events, _) = mpsc::unbounded();
    let (agent_events, _) = mpsc::unbounded();
    let services = Arc::new(Services::build(backend_events).await?);
    let _ = services.vault.status().await;
    #[cfg(feature = "snapshot")]
    if let Ok(password) = std::env::var("SOMME_TEST_MASTER_PASSWORD") {
        services.vault.unlock(&password, false).await?;
    }
    if !services.vault.is_unlocked() {
        tracing::warn!("the vault is locked, so agents will be told to unlock it in Somme");
    }
    let log = Arc::new(ScriptLog::new(&dir));
    let (yields, mut yielded) = mpsc::unbounded();
    let server = server::start(&dir, services.clone(), log, agent_events, Some(yields))?;
    tracing::info!("taking agents");
    loop {
        tokio::select! {
            _ = yielded.next() => {
                tracing::info!("the app took the agents back");
                break;
            }
            _ = stopped.next() => break,
            () = tokio::time::sleep(Duration::from_secs(1)) => {
                let settings = Settings::load(&data_dir);
                if !settings.agent_access || !settings.agent_service {
                    tracing::info!("agents were turned off");
                    break;
                }
            }
        }
    }
    drop(server);
    // Stopped runs log their end as their tasks drop.
    let began = Instant::now();
    while Arc::strong_count(&services) > 1 && began.elapsed() < Duration::from_secs(3) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    services.shutdown().await;
    Ok(())
}
