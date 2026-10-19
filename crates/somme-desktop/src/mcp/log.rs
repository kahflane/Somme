//! The script log: every script an agent runs, written before it starts and
//! again when it ends, one JSON line each, in a file only this user can read.

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use somme_ai::guard::{CheckedBy, Decision, Verdict};

const FILE: &str = "script-log.jsonl";
/// Output kept in the log for each run. The agent gets more.
const KEPT_OUTPUT: usize = 32 * 1024;
/// The viewer reads at most this much of the file's end.
const READ_BACK: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRef {
    pub id: String,
    pub name: String,
    /// `user@address:port`.
    pub address: String,
}

/// What the guard said about a script, kept with its run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuardNote {
    pub decision: Decision,
    /// The rule that decided, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// How sure Jev or the LLM was that the rule applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    pub by: CheckedBy,
    pub reason: String,
    /// What the user said when asked: true when they allowed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<bool>,
}

impl GuardNote {
    /// None when there was nothing to check.
    pub fn from(verdict: &Verdict, answer: Option<bool>) -> Option<Self> {
        (verdict.by != CheckedBy::Nothing).then(|| Self {
            decision: verdict.decision,
            rule: verdict.rule.clone(),
            percent: verdict.probability.map(|p| (p.clamp(0.0, 1.0) * 100.0).round() as u8),
            by: verdict.by,
            reason: verdict.reason.clone(),
            answer,
        })
    }

    /// Whether the script ran past the guard.
    pub fn allowed(&self) -> bool {
        match self.decision {
            Decision::Run => true,
            Decision::Ask => self.answer == Some(true),
            Decision::Block => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum Record {
    #[serde(rename_all = "camelCase")]
    Start {
        id: String,
        /// Unix time in milliseconds.
        at: i64,
        /// The agent's own name and version, from its MCP hello.
        agent: String,
        host: HostRef,
        script: String,
        timeout_seconds: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        guard: Option<GuardNote>,
    },
    #[serde(rename_all = "camelCase")]
    End {
        id: String,
        at: i64,
        exit_status: Option<u32>,
        exit_signal: Option<String>,
        timed_out: bool,
        /// Why it could not run, like a refused login.
        error: Option<String>,
        stdout: String,
        stderr: String,
    },
}

impl Record {
    pub fn id(&self) -> &str {
        match self {
            Record::Start { id, .. } | Record::End { id, .. } => id,
        }
    }
}

/// One script run, put together from its two records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub id: String,
    pub started: i64,
    pub agent: String,
    pub host: HostRef,
    pub script: String,
    pub timeout_seconds: u64,
    pub guard: Option<GuardNote>,
    pub end: Option<RunEnd>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunEnd {
    pub at: i64,
    pub exit_status: Option<u32>,
    pub exit_signal: Option<String>,
    pub timed_out: bool,
    pub error: Option<String>,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    pub fn finish(&mut self, record: Record) {
        if let Record::End { at, exit_status, exit_signal, timed_out, error, stdout, stderr, .. } = record {
            self.end = Some(RunEnd { at, exit_status, exit_signal, timed_out, error, stdout, stderr });
        }
    }

    pub fn from_start(record: Record) -> Option<Self> {
        match record {
            Record::Start { id, at, agent, host, script, timeout_seconds, guard } => Some(Self {
                id,
                started: at,
                agent,
                host,
                script,
                timeout_seconds,
                guard,
                end: None,
            }),
            Record::End { .. } => None,
        }
    }
}

/// Text cut to what the log keeps, on a character boundary.
pub fn kept(text: &str) -> String {
    if text.len() <= KEPT_OUTPUT {
        return text.to_owned();
    }
    let mut end = KEPT_OUTPUT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[… cut in the log …]", &text[..end])
}

pub struct ScriptLog {
    path: PathBuf,
    file: Mutex<Option<File>>,
}

impl ScriptLog {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(FILE),
            file: Mutex::new(None),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Adds a record. A script whose start cannot be written must not run.
    pub fn write(&self, record: &Record) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(record).map_err(std::io::Error::other)?;
        line.push(b'\n');
        let mut file = self.file.lock();
        if file.is_none() {
            *file = Some(open_private(&self.path)?);
        }
        let result = file.as_mut().map_or(Ok(()), |file| file.write_all(&line).and_then(|()| file.sync_data()));
        if result.is_err() {
            // Opened again next time, in case the file was moved away.
            *file = None;
        }
        result
    }

    /// The latest runs, newest first.
    pub fn recent(&self, limit: usize) -> Vec<Run> {
        let Ok(mut file) = File::open(&self.path) else { return Vec::new() };
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        let from = size.saturating_sub(READ_BACK);
        let mut raw = Vec::new();
        if file.seek(SeekFrom::Start(from)).and_then(|_| file.read_to_end(&mut raw)).is_err() {
            return Vec::new();
        }
        // Reading from the middle starts inside a line, so that line is skipped.
        let raw = if from > 0 {
            raw.iter().position(|b| *b == b'\n').map_or(&raw[..0], |nl| &raw[nl + 1..])
        } else {
            &raw[..]
        };
        runs_from(raw, limit)
    }
}

fn runs_from(raw: &[u8], limit: usize) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for line in raw.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        let Ok(record) = serde_json::from_slice::<Record>(line) else { continue };
        match record {
            Record::Start { .. } => runs.extend(Run::from_start(record)),
            Record::End { .. } => {
                if let Some(run) = runs.iter_mut().rev().find(|run| run.id == record.id()) {
                    run.finish(record);
                }
            }
        }
    }
    runs.reverse();
    runs.truncate(limit);
    runs
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
    use super::*;

    fn start(id: &str, script: &str) -> Record {
        Record::Start {
            id: id.into(),
            at: 1,
            agent: "claude-code 2.1".into(),
            host: HostRef {
                id: "h1".into(),
                name: "web".into(),
                address: "deploy@web.example.test:22".into(),
            },
            script: script.into(),
            timeout_seconds: 60,
            guard: None,
        }
    }

    fn end(id: &str, status: u32) -> Record {
        Record::End {
            id: id.into(),
            at: 2,
            exit_status: Some(status),
            exit_signal: None,
            timed_out: false,
            error: None,
            stdout: "ok\n".into(),
            stderr: String::new(),
        }
    }

    #[test]
    fn runs_come_back_newest_first_with_their_ends() {
        let dir = tempfile::tempdir().unwrap();
        let log = ScriptLog::new(dir.path());
        log.write(&start("a", "uptime")).unwrap();
        log.write(&start("b", "df -h")).unwrap();
        log.write(&end("a", 0)).unwrap();
        let runs = log.recent(10);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].script, "df -h");
        assert!(runs[0].end.is_none());
        assert_eq!(runs[1].end.as_ref().and_then(|e| e.exit_status), Some(0));
        assert_eq!(log.recent(1).len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn only_this_user_can_read_the_log() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let log = ScriptLog::new(dir.path());
        log.write(&start("a", "id")).unwrap();
        let mode = std::fs::metadata(log.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn the_guards_word_is_kept_with_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let log = ScriptLog::new(dir.path());
        let verdict = Verdict {
            decision: Decision::Ask,
            rule: Some("Ask before restarts".into()),
            probability: Some(0.874),
            by: CheckedBy::Jev,
            reason: "Your rule says to ask first.".into(),
        };
        let mut record = start("a", "systemctl restart nginx");
        if let Record::Start { guard, .. } = &mut record {
            *guard = GuardNote::from(&verdict, Some(false));
        }
        log.write(&record).unwrap();
        let note = log.recent(1)[0].guard.clone().unwrap();
        assert_eq!(note.percent, Some(87));
        assert!(!note.allowed());
        // Nothing checked, nothing noted.
        let nothing = Verdict { decision: Decision::Run, rule: None, probability: None, by: CheckedBy::Nothing, reason: String::new() };
        assert!(GuardNote::from(&nothing, None).is_none());
    }

    #[test]
    fn runs_from_before_the_guard_still_read() {
        let line = br#"{"event":"start","id":"a","at":1,"agent":"x","host":{"id":"h","name":"web","address":"w:22"},"script":"ls","timeoutSeconds":60}"#;
        let runs = runs_from(line, 10);
        assert_eq!(runs.len(), 1);
        assert!(runs[0].guard.is_none());
    }

    #[test]
    fn long_output_is_cut_on_a_character() {
        let text = "é".repeat(KEPT_OUTPUT);
        let cut = kept(&text);
        assert!(cut.len() < text.len());
        assert!(cut.ends_with("[… cut in the log …]"));
    }
}
