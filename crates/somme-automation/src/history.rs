//! Past runs, kept on this computer only: one file of events per run, in a
//! folder only this user can read. Each run is put back together from its
//! events for the run list and the canvas.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::{Event, RunStatus};
use crate::nodes::summary;

/// Runs kept for each automation. Older ones are removed.
pub const KEPT_RUNS: usize = 50;

/// Who started a run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum StartedBy {
    You,
    Schedule,
    Agent(String),
}

impl StartedBy {
    pub fn words(&self) -> String {
        match self {
            Self::You => "You".into(),
            Self::Schedule => "Schedule".into(),
            Self::Agent(name) => name.clone(),
        }
    }
}

/// The first line of a run's file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunHead {
    pub id: String,
    pub automation_id: String,
    pub automation_name: String,
    pub by: StartedBy,
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Running,
    Done,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    pub status: StepStatus,
    pub started: Option<i64>,
    pub ended: Option<i64>,
    pub attempt: u32,
    /// The way out it took, like `yes`.
    pub port: Option<String>,
    /// One text to show: its output, or why it failed.
    pub summary: String,
    pub result: Value,
    /// What it said it was doing, like an Agent's tool calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// One run, from its events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub head: RunHead,
    pub started: i64,
    pub ended: Option<i64>,
    pub status: RunStatus,
    pub error: Option<String>,
    pub steps: BTreeMap<String, StepRecord>,
}

impl RunRecord {
    pub fn new(head: RunHead) -> Self {
        Self { head, started: 0, ended: None, status: RunStatus::Running, error: None, steps: BTreeMap::new() }
    }

    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::Started { at } => self.started = *at,
            Event::NodeStarted { node, at, attempt } => {
                self.steps.insert(
                    node.clone(),
                    StepRecord {
                        status: StepStatus::Running,
                        started: Some(*at),
                        ended: None,
                        attempt: *attempt,
                        port: None,
                        summary: String::new(),
                        result: Value::Null,
                        notes: Vec::new(),
                    },
                );
            }
            Event::NodeFinished { node, at, finished } => {
                let step = self.steps.entry(node.clone()).or_insert_with(|| StepRecord {
                    status: StepStatus::Running,
                    started: Some(*at),
                    ended: None,
                    attempt: 1,
                    port: None,
                    summary: String::new(),
                    result: Value::Null,
                    notes: Vec::new(),
                });
                step.status = if finished.is_failure() { StepStatus::Failed } else { StepStatus::Done };
                step.ended = Some(*at);
                step.port = Some(finished.port.clone());
                step.summary = summary(finished);
                step.result = finished.result.clone();
            }
            Event::NodeNote { node, text, .. } => {
                if let Some(step) = self.steps.get_mut(node) {
                    step.notes.push(text.clone());
                }
            }
            Event::NodeSkipped { node } => {
                self.steps.insert(
                    node.clone(),
                    StepRecord {
                        status: StepStatus::Skipped,
                        started: None,
                        ended: None,
                        attempt: 0,
                        port: None,
                        summary: String::new(),
                        result: Value::Null,
                        notes: Vec::new(),
                    },
                );
            }
            Event::Finished { at, status, error } => {
                self.ended = Some(*at);
                self.status = *status;
                self.error = error.clone();
            }
        }
    }
}

/// Where runs are kept: `<dir>/<automation id>/<run id>.jsonl`.
#[derive(Clone, Debug)]
pub struct History {
    dir: PathBuf,
}

impl History {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn folder(&self, automation_id: &str) -> PathBuf {
        // Ids are made by Somme, but a synced one could be anything.
        let safe: String = automation_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        self.dir.join(if safe.is_empty() { "unknown".to_owned() } else { safe })
    }

    /// Starts a run's file. Old runs past `KEPT_RUNS` are removed.
    pub fn start(&self, head: &RunHead) -> std::io::Result<RunWriter> {
        let folder = self.folder(&head.automation_id);
        make_private_dir(&folder)?;
        let path = folder.join(format!("{}.jsonl", head.id));
        let mut file = open_private(&path)?;
        let mut line = serde_json::to_vec(head).map_err(std::io::Error::other)?;
        line.push(b'\n');
        file.write_all(&line)?;
        self.prune(&folder);
        Ok(RunWriter { file: Mutex::new(Some(file)) })
    }

    fn prune(&self, folder: &Path) {
        let mut runs = run_files(folder);
        if runs.len() <= KEPT_RUNS {
            return;
        }
        runs.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        for (_, path) in runs.into_iter().skip(KEPT_RUNS) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// An automation's runs, newest first.
    pub fn runs(&self, automation_id: &str, limit: usize) -> Vec<RunRecord> {
        let mut files = run_files(&self.folder(automation_id));
        files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        let mut runs: Vec<RunRecord> = files.into_iter().take(limit).filter_map(|(_, path)| read_run(&path)).collect();
        runs.sort_by_key(|run| std::cmp::Reverse(run.started));
        runs
    }

    /// The newest run of each automation.
    pub fn latest(&self) -> BTreeMap<String, RunRecord> {
        let Ok(folders) = std::fs::read_dir(&self.dir) else { return BTreeMap::new() };
        folders
            .flatten()
            .filter_map(|entry| {
                let id = entry.file_name().to_string_lossy().into_owned();
                let run = self.runs(&id, 1).into_iter().next()?;
                Some((run.head.automation_id.clone(), run))
            })
            .collect()
    }

    /// One run, by its id.
    pub fn find(&self, run_id: &str) -> Option<RunRecord> {
        let name = format!("{run_id}.jsonl");
        if run_id.is_empty() || !run_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return None;
        }
        let folders = std::fs::read_dir(&self.dir).ok()?;
        folders.flatten().map(|folder| folder.path().join(&name)).find(|path| path.is_file()).and_then(|path| read_run(&path))
    }

    /// Removes an automation's runs, when it is deleted.
    pub fn forget(&self, automation_id: &str) {
        let _ = std::fs::remove_dir_all(self.folder(automation_id));
    }
}

fn run_files(folder: &Path) -> Vec<(std::time::SystemTime, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect()
}

fn read_run(path: &Path) -> Option<RunRecord> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let head: RunHead = serde_json::from_str(lines.next()?).ok()?;
    let mut run = RunRecord::new(head);
    for line in lines {
        if let Ok(event) = serde_json::from_str::<Event>(line) {
            run.apply(&event);
        }
    }
    if run.ended.is_none() {
        // The file was not finished: Somme quit or crashed during the run.
        run.status = RunStatus::Cancelled;
        run.error = Some("Somme stopped before the run ended.".into());
    }
    Some(run)
}

/// Writes one run's events as they happen.
pub struct RunWriter {
    file: Mutex<Option<File>>,
}

impl RunWriter {
    pub fn write(&self, event: &Event) {
        let mut file = self.file.lock();
        let Some(file) = file.as_mut() else { return };
        let Ok(mut line) = serde_json::to_vec(event) else { return };
        line.push(b'\n');
        if let Err(err) = file.write_all(&line) {
            tracing::warn!(%err, "could not write the run log");
        }
    }
}

#[cfg(unix)]
fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

#[cfg(not(unix))]
fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(unix)]
fn open_private(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    OpenOptions::new().create(true).append(true).mode(0o600).open(path)
}

#[cfg(not(unix))]
fn open_private(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::nodes::Finished;

    fn head(id: &str) -> RunHead {
        RunHead { id: id.into(), automation_id: "a1".into(), automation_name: "Backup".into(), by: StartedBy::You, inputs: BTreeMap::new() }
    }

    #[test]
    fn runs_are_kept_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::new(dir.path());
        let writer = history.start(&head("r1")).unwrap();
        writer.write(&Event::Started { at: 10 });
        writer.write(&Event::NodeStarted { node: "dump".into(), at: 11, attempt: 1 });
        let finished = Finished { port: "ok".into(), result: json!({ "output": "done\n" }), error: None };
        writer.write(&Event::NodeFinished { node: "dump".into(), at: 12, finished });
        writer.write(&Event::NodeSkipped { node: "alarm".into() });
        writer.write(&Event::Finished { at: 13, status: RunStatus::Ok, error: None });
        drop(writer);
        let unfinished = history.start(&head("r2")).unwrap();
        unfinished.write(&Event::Started { at: 20 });
        drop(unfinished);

        let runs = history.runs("a1", 10);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].head.id, "r2");
        assert_eq!(runs[0].status, RunStatus::Cancelled);
        let done = &runs[1];
        assert_eq!(done.status, RunStatus::Ok);
        assert_eq!(done.steps["dump"].summary, "done\n");
        assert_eq!(done.steps["alarm"].status, StepStatus::Skipped);
        assert_eq!(history.latest()["a1"].head.id, "r2");
        history.forget("a1");
        assert!(history.runs("a1", 10).is_empty());
    }
}
