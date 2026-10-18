//! Reads the user's shell history, so Up in the command editor reaches commands
//! from other terminals too. For an SSH shell, that is the history on the server.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::terminal::disk::Disk;

/// Most commands kept from the file.
const MAX_COMMANDS: usize = 5000;
/// Bigger history files on a server are skipped.
const MAX_SERVER_FILE: u64 = 8 * 1024 * 1024;

/// zsh marks some bytes with this one and flips a bit in the next byte.
const META: u8 = 0x83;

fn history_file() -> Option<PathBuf> {
    std::env::var_os("HISTFILE")
        .map(PathBuf::from)
        .or_else(|| Some(dirs::home_dir()?.join(".zsh_history")))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub command: String,
    /// When it ran, in seconds since 1970. zsh only writes this with extended history on.
    pub time: Option<i64>,
}

/// The user's zsh history, oldest first, each command once. Slow: run it off the UI thread.
pub fn load_zsh_history() -> Vec<Entry> {
    let Some(bytes) = history_file().and_then(|path| std::fs::read(path).ok()) else {
        return Vec::new();
    };
    parse(&bytes)
}

/// The zsh or bash history in the user's home folder on a server. Slow: run it
/// in [`Disk::run`].
pub fn load_server_history(disk: &Disk) -> Vec<Entry> {
    // Server paths without a leading slash start in the home folder.
    if let Ok(bytes) = disk.read(Path::new(".zsh_history"), MAX_SERVER_FILE) {
        return parse(&bytes);
    }
    disk.read(Path::new(".bash_history"), MAX_SERVER_FILE)
        .map(|bytes| parse_bash(&bytes))
        .unwrap_or_default()
}

fn unmetafy(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut iter = bytes.iter();
    while let Some(&byte) = iter.next() {
        if byte == META {
            if let Some(&next) = iter.next() {
                out.push(next ^ 32);
            }
        } else {
            out.push(byte);
        }
    }
    out
}

fn parse(bytes: &[u8]) -> Vec<Entry> {
    let text = String::from_utf8_lossy(&unmetafy(bytes)).into_owned();
    let mut commands = Vec::new();
    let mut current: Option<Entry> = None;
    for line in text.lines() {
        let part = match current.as_mut() {
            Some(entry) => {
                entry.command.push('\n');
                line
            }
            None => {
                // `: <start>:<elapsed>;<command>` when extended history is on.
                let stamped = line.strip_prefix(": ").and_then(|rest| rest.split_once(';')).and_then(|(stamp, command)| {
                    let mut numbers = stamp.split(':').map(|n| n.trim().parse::<i64>());
                    let start = numbers.next()?.ok()?;
                    numbers.all(|n| n.is_ok()).then_some((start, command))
                });
                let (time, command) = match stamped {
                    Some((start, command)) => (Some(start), command),
                    None => (None, line),
                };
                current = Some(Entry {
                    command: String::new(),
                    time,
                });
                command
            }
        };
        if let Some(entry) = current.as_mut() {
            entry.command.push_str(part);
        }
        if !part.ends_with('\\') {
            commands.extend(current.take());
        }
    }
    commands.extend(current);
    // A line ending in a backslash went on to the next line.
    for entry in &mut commands {
        entry.command = entry.command.replace("\\\n", "\n");
    }
    newest_once(commands)
}

/// bash writes a command per line, after a `#<time>` line when it keeps times.
fn parse_bash(bytes: &[u8]) -> Vec<Entry> {
    let text = String::from_utf8_lossy(bytes);
    let mut time = None;
    let mut commands = Vec::new();
    for line in text.lines() {
        if let Some(stamp) = line.strip_prefix('#')
            && let Ok(stamp) = stamp.parse::<i64>()
        {
            time = Some(stamp);
            continue;
        }
        commands.push(Entry {
            command: line.to_owned(),
            time: time.take(),
        });
    }
    newest_once(commands)
}

/// Keeps the newest copy of each command, oldest first.
fn newest_once(commands: Vec<Entry>) -> Vec<Entry> {
    let mut seen = HashSet::new();
    let mut out: Vec<Entry> = commands
        .into_iter()
        .rev()
        .filter(|entry| !entry.command.trim().is_empty() && seen.insert(entry.command.clone()))
        .take(MAX_COMMANDS)
        .collect();
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(entries: Vec<Entry>) -> Vec<String> {
        entries.into_iter().map(|entry| entry.command).collect()
    }

    #[test]
    fn reads_plain_and_extended_lines() {
        let file = b": 1700000000:0;ls -la\n: 1700000001:3;git status\nmake\nls -la\n";
        let entries = parse(file);
        assert_eq!(commands(entries.clone()), vec!["git status", "make", "ls -la"]);
        assert_eq!(entries[0].time, Some(1700000001));
        // The newest `ls -la` had no time.
        assert_eq!(entries[2].time, None);
    }

    #[test]
    fn joins_commands_split_over_lines() {
        let file = b": 1700000000:0;for f in *; do\\\necho $f\\\ndone\n: 1700000001:0;pwd\n";
        let entries = parse(file);
        assert_eq!(commands(entries.clone()), vec!["for f in *; do\necho $f\ndone", "pwd"]);
        assert_eq!(entries[0].time, Some(1700000000));
    }

    #[test]
    fn reads_bash_history_with_and_without_times() {
        let file = b"ls\n#1700000000\ngit status\n#1700000002\nls\n\nmake\n";
        let entries = parse_bash(file);
        assert_eq!(commands(entries.clone()), vec!["git status", "ls", "make"]);
        assert_eq!(entries[0].time, Some(1700000000));
        assert_eq!(entries[1].time, Some(1700000002));
        assert_eq!(entries[2].time, None);
    }

    #[test]
    fn undoes_meta_bytes() {
        // "ă" is 0xC4 0x83, and zsh writes the 0x83 as META, 0x83 ^ 32.
        let file = [b"echo \xc4".as_slice(), &[META, 0x83 ^ 32], b"\n"].concat();
        assert_eq!(commands(parse(&file)), vec!["echo ă"]);
    }
}
