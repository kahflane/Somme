//! Who last changed each line of a file in a git repo, and when, as
//! `git blame` tells it. Lines changed since the last commit belong to no one yet.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::changes::diff::is_sha;
use crate::terminal::blocks::git_input_raw;

/// Files longer than this are not blamed: git takes too long.
pub const MAX_LINES: usize = 50_000;

#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    /// None for lines not committed yet.
    pub sha: Option<String>,
    pub author: String,
    /// Seconds since 1970.
    pub time: i64,
    pub summary: String,
}

/// The change behind each line, by line number from 0.
#[derive(Debug, Default)]
pub struct Blame {
    lines: Vec<Option<Arc<Change>>>,
}

impl Blame {
    pub fn line(&self, line: usize) -> Option<&Arc<Change>> {
        self.lines.get(line)?.as_ref()
    }
}

/// Blames the file at `path` as it reads now, `text`, which may not be saved.
/// Slow: run it off the UI thread. None when the file is not in a repo or not
/// committed.
pub fn read(path: &Path, text: &str) -> Option<Blame> {
    let (dir, name) = (path.parent()?, path.file_name()?.to_str()?);
    if text.lines().count() > MAX_LINES {
        return None;
    }
    let out = git_input_raw(dir, &["blame", "--porcelain", "--no-textconv", "--contents", "-", "--", name], text)?;
    Some(parse(&out))
}

/// Reads `git blame --porcelain`: for each line a header naming its commit,
/// the commit's details the first time it comes up, and the line itself.
fn parse(out: &str) -> Blame {
    let mut changes: HashMap<&str, Arc<Change>> = HashMap::new();
    let mut fields: HashMap<&str, HashMap<&str, &str>> = HashMap::new();
    // The commit and line number of each line, in the order git gives them.
    let mut order: Vec<(&str, usize)> = Vec::new();
    let mut current: Option<&str> = None;
    for line in out.lines() {
        if line.starts_with('\t') {
            current = None;
            continue;
        }
        let mut words = line.split(' ');
        match (current, words.next()) {
            (None, Some(sha)) if sha.len() >= 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()) => {
                let line = words.nth(1).and_then(|line| line.parse::<usize>().ok()).unwrap_or_default();
                order.push((sha, line.saturating_sub(1)));
                current = Some(sha);
            }
            (Some(sha), Some(key)) => {
                let value = line.get(key.len() + 1..).unwrap_or_default();
                fields.entry(sha).or_default().insert(key, value);
            }
            _ => {}
        }
    }
    let mut lines = vec![None; order.iter().map(|(_, line)| line + 1).max().unwrap_or_default()];
    for (sha, line) in order {
        let change = changes.entry(sha).or_insert_with(|| {
            let fields = fields.get(sha);
            let field = |key: &str| fields.and_then(|fields| fields.get(key)).copied().unwrap_or_default();
            // Lines not committed yet come as all zeros.
            let committed = is_sha(sha) && sha.bytes().any(|b| b != b'0');
            Arc::new(Change {
                sha: committed.then(|| sha.to_owned()),
                author: field("author").to_owned(),
                time: field("author-time").parse().unwrap_or_default(),
                summary: field("summary").to_owned(),
            })
        });
        lines[line] = Some(change.clone());
    }
    Blame { lines }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_git_blame_porcelain() {
        let a = "a".repeat(40);
        let zero = "0".repeat(40);
        let out = format!(
            "{a} 1 1 2\nauthor Ann Lee\nauthor-mail <ann@example.test>\nauthor-time 1700000000\nsummary Fix the cron time zone\nfilename cron.txt\n\tfirst\n\
             {a} 2 2\n\tsecond\n\
             {zero} 3 3 1\nauthor Not Committed Yet\nauthor-time 1800000000\nsummary Version of cron.txt from cron.txt\nfilename cron.txt\n\tthird\n"
        );
        let blame = parse(&out);
        let first = blame.line(0).unwrap();
        assert_eq!(first.sha.as_deref(), Some(a.as_str()));
        assert_eq!((first.author.as_str(), first.time), ("Ann Lee", 1_700_000_000));
        assert_eq!(first.summary, "Fix the cron time zone");
        assert!(Arc::ptr_eq(first, blame.line(1).unwrap()));
        assert_eq!(blame.line(2).unwrap().sha, None);
        assert!(blame.line(3).is_none());
    }
}
