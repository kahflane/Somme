use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::terminal::session::SessionTarget;

const LOG_DIR: &str = "session-logs";
const INDEX: &str = "index.jsonl";
/// Marks a terminal resize inside a log. Terminals ignore this unknown OSC code,
/// so a log still reads fine with `cat`.
const SIZE_MARK: &[u8] = b"\x1b]6973;size=";

/// One recorded terminal session, as listed on the Logs page.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub file: String,
    pub label: String,
    pub host_id: Option<String>,
    pub started_at: i64,
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

pub fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(LOG_DIR)
}

/// Logs can hold anything a session printed, so only the owner may read them.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn private_file(options: &mut OpenOptions) -> &mut OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options
}

fn slug(label: &str) -> String {
    let slug: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() { "session".to_owned() } else { slug.chars().take(40).collect() }
}

/// Writes everything a session prints to a file, so it can be read later.
pub struct SessionLog {
    out: BufWriter<File>,
}

impl SessionLog {
    pub fn create(data_dir: &Path, target: &SessionTarget) -> Option<Self> {
        let dir = log_dir(data_dir);
        create_private_dir(&dir).ok()?;
        let (label, host_id) = match target {
            SessionTarget::Local => ("Local Terminal".to_owned(), None),
            SessionTarget::Ssh { host_id, label } => (label.clone(), Some(host_id.to_string())),
            SessionTarget::Replay { .. } => return None,
        };
        let started_at = now_secs();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        let file = format!("{started_at}-{nanos:09}-{}.log", slug(&label));
        let out = private_file(OpenOptions::new().write(true).create_new(true))
            .open(dir.join(&file))
            .ok()?;
        let entry = LogEntry {
            file,
            label,
            host_id,
            started_at,
        };
        let mut index = private_file(OpenOptions::new().create(true).append(true))
            .open(dir.join(INDEX))
            .ok()?;
        let line = serde_json::to_string(&entry).ok()?;
        writeln!(index, "{line}").ok()?;
        Some(Self {
            out: BufWriter::new(out),
        })
    }

    pub fn write(&mut self, bytes: &[u8]) {
        let _ = self.out.write_all(bytes);
    }

    /// Records the new terminal size, so a replay lays text out the same way.
    pub fn resize(&mut self, columns: usize, lines: usize) {
        let _ = self.out.write_all(SIZE_MARK);
        let _ = write!(self.out, "{columns}x{lines}\x07");
    }

    pub fn flush(&mut self) {
        let _ = self.out.flush();
    }

    pub fn note(&mut self, text: &str) {
        let _ = write!(self.out, "\r\n\x1b[2m[{text}]\x1b[0m\r\n");
        let _ = self.out.flush();
    }
}

impl Drop for SessionLog {
    fn drop(&mut self) {
        let _ = self.out.flush();
    }
}

/// Lists recorded sessions, newest first. Entries whose file is gone are skipped.
pub fn list(data_dir: &Path) -> Vec<LogEntry> {
    let dir = log_dir(data_dir);
    let Ok(index) = File::open(dir.join(INDEX)) else {
        return Vec::new();
    };
    let mut entries: Vec<LogEntry> = BufReader::new(index)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<LogEntry>(&line).ok())
        .filter(|entry| is_plain_file_name(&entry.file) && dir.join(&entry.file).exists())
        .collect();
    entries.sort_by(|a, b| b.started_at.cmp(&a.started_at).then_with(|| b.file.cmp(&a.file)));
    entries
}

fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != ".."
}

pub fn read(data_dir: &Path, file: &str) -> std::io::Result<Vec<u8>> {
    if !is_plain_file_name(file) {
        return Err(std::io::Error::other("bad log file name"));
    }
    std::fs::read(log_dir(data_dir).join(file))
}

pub enum Chunk<'a> {
    Output(&'a [u8]),
    Size { columns: usize, lines: usize },
}

/// Splits a log into printed output and the size changes between it.
pub fn chunks(bytes: &[u8]) -> Vec<Chunk<'_>> {
    let mut out = Vec::new();
    let mut rest = bytes;
    while let Some(start) = rest.windows(SIZE_MARK.len()).position(|w| w == SIZE_MARK) {
        let body = &rest[start + SIZE_MARK.len()..];
        let Some(end) = body.iter().position(|&b| b == 0x07) else { break };
        let size = std::str::from_utf8(&body[..end]).ok().and_then(|text| {
            let (columns, lines) = text.split_once('x')?;
            Some((columns.parse::<usize>().ok()?, lines.parse::<usize>().ok()?))
        });
        if start > 0 {
            out.push(Chunk::Output(&rest[..start]));
        }
        if let Some((columns, lines)) = size.filter(|(c, l)| *c > 0 && *l > 0) {
            out.push(Chunk::Size { columns, lines });
        }
        rest = &body[end + 1..];
    }
    if !rest.is_empty() {
        out.push(Chunk::Output(rest));
    }
    out
}

/// Size of a log file in bytes, or 0 when it cannot be read.
pub fn size(data_dir: &Path, entry: &LogEntry) -> u64 {
    if !is_plain_file_name(&entry.file) {
        return 0;
    }
    std::fs::metadata(log_dir(data_dir).join(&entry.file)).map_or(0, |m| m.len())
}

pub fn delete(data_dir: &Path, entry: &LogEntry) -> std::io::Result<()> {
    if !is_plain_file_name(&entry.file) {
        return Err(std::io::Error::other("bad log file name"));
    }
    let dir = log_dir(data_dir);
    match std::fs::remove_file(dir.join(&entry.file)) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let kept: Vec<String> = list(data_dir)
        .into_iter()
        .filter_map(|e| serde_json::to_string(&e).ok())
        .collect();
    let mut index = private_file(OpenOptions::new().write(true).create(true).truncate(true)).open(dir.join(INDEX))?;
    for line in kept {
        writeln!(index, "{line}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_lists_sessions() {
        let dir = tempfile::tempdir().expect("temp dir");
        {
            let mut log = SessionLog::create(dir.path(), &SessionTarget::Local).expect("log");
            log.write(b"hello\r\n");
            log.note("Session closed");
        }
        let entries = list(dir.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "Local Terminal");
        let body = read(dir.path(), &entries[0].file).expect("read");
        assert!(String::from_utf8_lossy(&body).contains("hello"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |p: &Path| std::fs::metadata(p).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode(&log_dir(dir.path())), 0o700);
            assert_eq!(mode(&log_dir(dir.path()).join(&entries[0].file)), 0o600);
            assert_eq!(mode(&log_dir(dir.path()).join(INDEX)), 0o600);
        }
        delete(dir.path(), &entries[0]).expect("delete");
        assert!(list(dir.path()).is_empty());
    }

    #[test]
    fn splits_size_marks() {
        let bytes = b"ab\x1b]6973;size=120x40\x07cd\x1b]6973;size=bad\x07ef";
        let parts: Vec<String> = chunks(bytes)
            .into_iter()
            .map(|c| match c {
                Chunk::Output(b) => String::from_utf8_lossy(b).into_owned(),
                Chunk::Size { columns, lines } => format!("[{columns}x{lines}]"),
            })
            .collect();
        assert_eq!(parts, ["ab", "[120x40]", "cd", "ef"]);
    }

    #[test]
    fn rejects_paths_in_index() {
        let entry = LogEntry {
            file: "../secret".into(),
            label: "x".into(),
            host_id: None,
            started_at: 0,
        };
        assert!(read(Path::new("/tmp"), &entry.file).is_err());
    }
}
