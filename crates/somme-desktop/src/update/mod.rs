//! Looks for a new Somme on GitHub now and then, and installs it when the
//! user asks. Nothing is downloaded before that, and nothing is installed
//! that doesn't check out (see `verify`).

pub mod install;
pub mod release;
pub mod verify;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{App, AsyncApp, Global};

use crate::runtime;
use crate::settings::Settings;
use install::{Fetch, Step, Swap, Way};
use release::{Release, Version};

/// The first check waits a little, so starting stays quick.
const FIRST_CHECK: Duration = Duration::from_secs(30);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// After a check that failed, say while offline.
const RETRY: Duration = Duration::from_secs(30 * 60);
/// While checks are off, how often the setting is looked at again.
const IDLE: Duration = Duration::from_secs(60);
const TIMEOUT: Duration = Duration::from_secs(30);
const AGENT: &str = concat!("somme/", env!("CARGO_PKG_VERSION"));

/// A newer release, and how this copy of Somme can get it.
#[derive(Clone, Debug)]
pub struct Found {
    pub release: Release,
    pub way: Way,
}

/// Where installing is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Idle,
    Downloading { done: u64, total: u64 },
    Checking,
    /// The new app is ready and Somme is quitting.
    Restarting,
    /// What went wrong, in plain words.
    Failed(String),
}

impl From<Step> for Stage {
    fn from(step: Step) -> Self {
        match step {
            Step::Downloading { done, total } => Stage::Downloading { done, total },
            Step::Checking => Stage::Checking,
        }
    }
}

pub struct Updates {
    pub found: Option<Found>,
    pub stage: Stage,
    data_dir: PathBuf,
    /// For asking GitHub. None when it couldn't be made.
    client: Option<reqwest::Client>,
}

impl Global for Updates {}

impl Updates {
    /// The update to show: newer than this copy, not put off with Later,
    /// and checks are on.
    pub fn shown(cx: &App) -> Option<&Found> {
        let settings = Settings::get(cx);
        if !settings.check_updates {
            return None;
        }
        let found = cx.try_global::<Self>()?.found.as_ref()?;
        let later = settings.update_later.as_deref().and_then(Version::parse);
        later.is_none_or(|later| found.release.version > later).then_some(found)
    }

    pub fn stage(cx: &App) -> Stage {
        cx.try_global::<Self>().map_or(Stage::Idle, |updates| updates.stage.clone())
    }

    pub fn busy(&self) -> bool {
        matches!(self.stage, Stage::Downloading { .. } | Stage::Checking | Stage::Restarting)
    }
}

/// Starts checking: once soon after start, then every few hours.
pub fn init(data_dir: PathBuf, cx: &mut App) {
    let client = reqwest::Client::builder()
        .user_agent(AGENT)
        .timeout(TIMEOUT)
        .build()
        .inspect_err(|err| tracing::warn!(%err, "can't check for updates"))
        .ok();
    cx.set_global(Updates {
        found: None,
        stage: Stage::Idle,
        data_dir,
        client,
    });
    cx.spawn(async move |cx| {
        cx.background_executor().timer(first_check()).await;
        loop {
            let wait = if cx.update(|cx| Settings::get(cx).check_updates) {
                if check(cx).await { EVERY } else { RETRY }
            } else {
                IDLE
            };
            cx.background_executor().timer(wait).await;
        }
    })
    .detach();
}

/// UI check builds pointed at a fake GitHub check right away.
fn first_check() -> Duration {
    #[cfg(feature = "snapshot")]
    if crate::github::api::fake_api().is_some() {
        return Duration::from_secs(1);
    }
    FIRST_CHECK
}

/// Asks GitHub once. Returns whether it answered. A failed check only logs:
/// the next one may work.
async fn check(cx: &mut AsyncApp) -> bool {
    let Some(client) = cx.update(|cx| cx.global::<Updates>().client.clone()) else { return false };
    let answer = runtime::io(async move {
        let release = release::latest(&client).await?;
        Ok::<_, anyhow::Error>(release.filter(|release| release.version > Version::running()).map(|release| {
            // Looks at the disk, so it is worked out here.
            let way = install::way(&release);
            Found { release, way }
        }))
    })
    .await;
    match answer {
        Ok(found) => {
            cx.update(|cx| {
                let updates = cx.global_mut::<Updates>();
                // An install in progress keeps the release it started with.
                if updates.busy() {
                    return;
                }
                let same = updates.found.as_ref().map(|found| &found.release) == found.as_ref().map(|found| &found.release);
                if !same {
                    updates.stage = Stage::Idle;
                }
                updates.found = found;
            });
            true
        }
        Err(err) => {
            tracing::debug!("update check failed: {err:#}");
            false
        }
    }
}

fn set_stage(stage: Stage, cx: &mut App) {
    cx.global_mut::<Updates>().stage = stage;
}

/// Hides the update until a newer one comes out.
pub fn later(cx: &mut App) {
    let Some(found) = Updates::shown(cx) else { return };
    let version = found.release.version.to_string();
    Settings::update(cx, |settings| settings.update_later = Some(version));
}

/// Opens the release on github.com.
pub fn open_page(cx: &mut App) {
    if let Some(found) = Updates::shown(cx) {
        let page = found.release.page();
        cx.open_url(&page);
    }
}

/// Downloads and checks the update, starts the script that swaps the apps,
/// then quits so it can. Where Somme can't install, opens the release page.
pub fn install(cx: &mut App) {
    let Some(updates) = cx.try_global::<Updates>() else { return };
    if updates.busy() {
        return;
    }
    let Some(Found {
        release,
        way: Way::Install(plan),
    }) = Updates::shown(cx).cloned()
    else {
        return open_page(cx);
    };
    let dir = updates.data_dir.join(install::FOLDER);
    set_stage(Stage::Downloading { done: 0, total: plan.zip.size }, cx);
    let (steps, mut step_rx) = mpsc::unbounded::<Step>();
    cx.spawn(async move |cx| {
        let version = release.version;
        let (work_plan, work_dir) = (plan.clone(), dir.clone());
        let prepared = runtime::io(async move {
            let fetch = Fetch::new().context(install::NO_DOWNLOAD)?;
            let work = fresh_folder(&work_dir).context("Couldn't make a folder for the update.")?;
            let app = install::prepare(&work_plan, version, &work, &fetch, move |step| {
                let _ = steps.unbounded_send(step);
            })
            .await;
            match app {
                Ok(app) => Ok((app, work)),
                Err(err) => {
                    let _ = std::fs::remove_dir_all(&work);
                    Err(err)
                }
            }
        });
        let progress = {
            let cx = cx.clone();
            async move {
                while let Some(step) = step_rx.next().await {
                    cx.update(|cx| set_stage(step.into(), cx));
                }
            }
        };
        let (prepared, ()) = futures::join!(prepared, progress);
        let started = prepared.and_then(|(app, work)| {
            let swap = Swap::new(&plan.bundle, app, work, dir.join("update.log"));
            install::relaunch(&swap, &dir.join("relaunch.sh"))
        });
        cx.update(|cx| match started {
            Ok(()) => {
                set_stage(Stage::Restarting, cx);
                cx.quit();
            }
            Err(err) => {
                tracing::warn!("update failed: {err:#}");
                set_stage(Stage::Failed(err.to_string()), cx);
            }
        });
    })
    .detach();
}

/// A new, empty folder for one install, with what earlier ones left removed.
fn fresh_folder(dir: &Path) -> std::io::Result<PathBuf> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let work = dir.join(uuid::Uuid::new_v4().simple().to_string());
    std::fs::create_dir_all(&work)?;
    Ok(work)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_folder_clears_old_installs() {
        let dir = tempfile::tempdir().unwrap();
        let first = fresh_folder(dir.path()).unwrap();
        std::fs::write(first.join("download.zip"), "x").unwrap();
        std::fs::write(dir.path().join("update.log"), "kept").unwrap();
        let second = fresh_folder(dir.path()).unwrap();
        assert!(!first.exists());
        assert!(second.is_dir());
        assert!(dir.path().join("update.log").exists());
    }

    #[test]
    fn steps_become_stages() {
        assert_eq!(Stage::from(Step::Downloading { done: 1, total: 2 }), Stage::Downloading { done: 1, total: 2 });
        assert_eq!(Stage::from(Step::Checking), Stage::Checking);
    }
}
