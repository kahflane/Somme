//! Starts scheduled automations on time. Runs in the app while it is open,
//! and in the background worker while it is closed, never both: whoever
//! holds the agents lock runs it. When each automation was last checked is
//! kept in a file, so runs missed while the computer slept are found.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use somme_automation::schedule;
use somme_automation::{Automation, StartedBy};

use super::runner::Runner;

/// How often it looks. A run starts at most this late.
const TICK: Duration = Duration::from_secs(20);

fn checked_path(data_dir: &Path) -> PathBuf {
    data_dir.join("automations").join("schedule.json")
}

/// Looks for due runs until the task is dropped.
pub async fn run(runner: Arc<Runner>) {
    let path = checked_path(&runner.services().data_dir);
    loop {
        tick(&runner, &path).await;
        tokio::time::sleep(TICK).await;
    }
}

async fn tick(runner: &Arc<Runner>, path: &Path) {
    // A locked vault can't open them; it looks again next time.
    let Ok(views) = runner.services().automations.list().await else { return };
    let automations: Vec<Automation> = views
        .iter()
        .filter_map(|view| {
            let mut automation = Automation::from_json(&view.document).ok()?;
            automation.id = view.id.to_string();
            Some(automation)
        })
        .collect();
    let scheduled: Vec<(String, somme_automation::schema::Start)> =
        automations.iter().map(|automation| (automation.id.clone(), automation.start.clone())).collect();
    let checked = read(path);
    let (due, next) = schedule::plan(&scheduled, &checked, &chrono::Local::now());
    if next != checked {
        write(path, &next);
    }
    for id in due {
        let Some(automation) = automations.iter().find(|automation| automation.id == id) else { continue };
        if runner.is_automation_running(&id) {
            tracing::info!(name = %automation.name, "skipped a scheduled run: the last one is still going");
            continue;
        }
        if let Err(err) = runner.start(automation.clone(), BTreeMap::new(), StartedBy::Schedule) {
            tracing::warn!(name = %automation.name, %err, "could not start a scheduled run");
        }
    }
}

fn read(path: &Path) -> BTreeMap<String, i64> {
    std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn write(path: &Path, checked: &BTreeMap<String, i64>) {
    let Ok(text) = serde_json::to_string(checked) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Written whole, then moved, so a crash never leaves half a file.
    let partial = path.with_extension("json.partial");
    if std::fs::write(&partial, text).and_then(|()| std::fs::rename(&partial, path)).is_err() {
        tracing::warn!(path = %path.display(), "could not save when schedules were checked");
    }
}

/// When it runs next, in words, like `Mon 20 Oct, 09:00`. None when it has no schedule.
pub fn next_run(automation: &Automation) -> Option<Result<String, String>> {
    let text = automation.start.schedule.as_deref().filter(|text| !text.trim().is_empty())?;
    Some(schedule::Cron::parse(text).and_then(|cron| {
        cron.next_after(&chrono::Local::now())
            .map(|at| format!("{}, next {}", schedule::words(text), at.format("%a %-d %b, %H:%M")))
            .ok_or_else(|| "It never runs.".to_owned())
    }))
}
