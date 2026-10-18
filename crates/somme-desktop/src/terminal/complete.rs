//! Tab completion for the command editor: command names from the shell's PATH,
//! and files and folders, wherever the shell runs. Also the folders shown while
//! typing after `cd`.

use std::collections::{BTreeSet, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::terminal::disk::{Disk, Entry};

/// Most choices kept, so a huge folder stays quick.
const MAX_CHOICES: usize = 500;
/// Most folders from past `cd` commands shown, and most looked up, as each
/// look on a server takes a round trip.
const MAX_RECENT: usize = 5;
const MAX_RECENT_CHECKS: usize = 20;
/// How long a folder's names are kept while a `cd` is typed.
const LISTING_TTL: Duration = Duration::from_secs(3);

/// Commands that take a folder.
const FOLDER_COMMANDS: &[&str] = &["cd", "pushd"];

/// Shell words that are not files on PATH.
const BUILTINS: &[&str] = &[
    "alias", "bg", "bindkey", "builtin", "cd", "command", "echo", "eval", "exec", "exit", "export", "fc", "fg",
    "functions", "history", "jobs", "kill", "popd", "print", "printf", "pushd", "pwd", "read", "rehash", "set",
    "setopt", "source", "time", "type", "typeset", "ulimit", "unalias", "unset", "unsetopt", "wait", "where",
    "which",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// What the list shows.
    pub label: String,
    /// What goes into the line, already escaped.
    pub insert: String,
    pub dir: bool,
    /// A folder from a past `cd`, not from the folder listed.
    pub recent: bool,
}

/// The folder last listed for [`suggest_folders`], so typing a name in it
/// does not read it again on each key. That matters most on a server.
#[derive(Default)]
pub struct Listings(Mutex<Option<(PathBuf, Instant, Vec<Entry>)>>);

impl Listings {
    /// Forgets the folder, as a command may have changed it.
    pub fn clear(&self) {
        *self.0.lock() = None;
    }

    fn list(&self, dir: &Path, disk: &Disk) -> Option<Vec<Entry>> {
        if let Some((kept, at, entries)) = &*self.0.lock()
            && kept == dir
            && at.elapsed() < LISTING_TTL
        {
            return Some(entries.clone());
        }
        let entries = disk.list(dir, usize::MAX, can_escape)?;
        *self.0.lock() = Some((dir.to_path_buf(), Instant::now(), entries.clone()));
        Some(entries)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// The part of the line the choice replaces.
    pub range: Range<usize>,
    pub choices: Vec<Choice>,
}

impl Completion {
    /// The text to put in place of the word: the only choice, or what all choices start with.
    pub fn common(&self) -> Option<String> {
        if let [only] = self.choices.as_slice() {
            return Some(finished(only));
        }
        let first = &self.choices.first()?.insert;
        let mut len = first.len();
        for choice in &self.choices[1..] {
            len = len.min(
                first
                    .char_indices()
                    .zip(choice.insert.chars())
                    .take_while(|((_, a), b)| a == b)
                    .map(|((i, a), _)| i + a.len_utf8())
                    .last()
                    .unwrap_or(0),
            );
        }
        Some(first[..len].to_owned())
    }
}

/// The choice as it goes in when picked: folders keep going, other words end with a space.
pub fn finished(choice: &Choice) -> String {
    if choice.dir {
        choice.insert.clone()
    } else {
        format!("{} ", choice.insert)
    }
}

/// Completes the word before `cursor` (a byte offset into `line`). Reads
/// `disk`, so it may block.
pub fn complete(line: &str, cursor: usize, cwd: &Path, path_var: Option<&str>, disk: &Disk) -> Option<Completion> {
    let cursor = cursor.min(line.len());
    let start = word_start(line, cursor);
    let word = &line[start..cursor];
    let before = line[..start].trim_end();
    let command_spot = before.is_empty() || before.ends_with(['|', ';', '&', '(']);
    let choices = if command_spot && !word.contains('/') && !word.is_empty() {
        commands(word, path_var, disk)
    } else {
        paths(word, cwd, disk, wants_folder(line, start), None)
    };
    (!choices.is_empty()).then_some(Completion {
        range: start..cursor,
        choices,
    })
}

/// Whether the cursor sits at the end of a `cd` or `pushd` line, on its folder.
pub fn typing_folder(line: &str, cursor: usize) -> bool {
    cursor == line.len() && !line.contains('\n') && {
        let start = word_start(line, cursor);
        wants_folder(line, start) && !line[start..].starts_with('-')
    }
}

/// Where the folder word being typed starts, while a `cd` is typed.
pub fn folder_word_start(line: &str, cursor: usize) -> Option<usize> {
    typing_folder(line, cursor).then(|| word_start(line, cursor))
}

/// Folders for the `cd` being typed: ones in the folder the word points at
/// first, then ones from past `cd` commands. `recent` holds those commands'
/// folders as typed, newest first. Reads `disk`, so it may block.
pub fn suggest_folders(
    line: &str,
    cursor: usize,
    cwd: &Path,
    recent: &[String],
    listings: &Listings,
    disk: &Disk,
) -> Option<Completion> {
    if !typing_folder(line, cursor) {
        return None;
    }
    let start = word_start(line, cursor);
    let word = &line[start..cursor];
    let mut choices = paths(word, cwd, disk, true, Some(listings));
    let (typed_dir, _) = split_word(word);
    let shown: Vec<PathBuf> = match base_dir(&unescape(typed_dir), cwd, disk) {
        Some(base) => choices.iter().map(|choice| base.join(choice.label.trim_end_matches('/'))).collect(),
        None => Vec::new(),
    };
    choices.extend(recent_folders(word, recent, cwd, &shown, disk));
    (!choices.is_empty()).then_some(Completion {
        range: start..cursor,
        choices,
    })
}

/// The folders past `cd` commands went to, as typed, in the commands' order.
/// Commands newest first give folders newest first.
pub fn cd_targets<'a>(commands: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut seen = HashSet::new();
    commands
        .filter_map(|command| {
            let (name, target) = command.trim().split_once(' ')?;
            let target = target.trim();
            let simple = !target.is_empty() && !target.contains(['\n', ';', '&', '|', '$', '`', '(', ')', '<', '>']);
            (FOLDER_COMMANDS.contains(&name) && simple).then(|| target.to_owned())
        })
        .filter(|target| seen.insert(target.clone()))
        .take(200)
        .collect()
}

/// Whether the word starting at `start` is the folder for `cd` or `pushd`.
fn wants_folder(line: &str, start: usize) -> bool {
    let before = line[..start].trim_end();
    let command = before.rsplit(['|', ';', '&', '(']).next().unwrap_or_default().trim();
    FOLDER_COMMANDS.contains(&command)
}

/// Where the word under the cursor starts. A backslash keeps the next character in the word.
fn word_start(line: &str, cursor: usize) -> usize {
    let mut start = 0;
    let mut escaped = false;
    for (i, ch) in line[..cursor].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            ' ' | '\t' | '\n' | '|' | ';' | '&' | '<' | '>' | '(' | ')' | '=' => start = i + ch.len_utf8(),
            _ => {}
        }
    }
    start
}

fn unescape(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut chars = word.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => out.extend(chars.next()),
            '\'' | '"' => {}
            _ => out.push(ch),
        }
    }
    out
}

/// Backslashes can't make a newline or other control character safe in a
/// shell word: it would end the command and start another. Names holding one
/// are left out rather than escaped.
pub fn can_escape(text: &str) -> bool {
    !text.contains(char::is_control)
}

/// Only for text that passes [`can_escape`].
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if " \t'\"\\$&;|<>()*?![]{}#`".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn commands(prefix: &str, path_var: Option<&str>, disk: &Disk) -> Vec<Choice> {
    let mut names: BTreeSet<String> = BUILTINS
        .iter()
        .filter(|name| name.starts_with(prefix))
        .map(|name| (*name).to_owned())
        .collect();
    // This app's own PATH only says something about this computer.
    let path_var = path_var
        .map(str::to_owned)
        .or_else(|| disk.is_local().then(|| std::env::var("PATH").ok()).flatten())
        .unwrap_or_default();
    // PATH on a server is a list of server folders, so split it the server's way.
    for dir in path_var.split(if disk.is_local() { PATH_SEPARATOR } else { ':' }).filter(|dir| !dir.is_empty()) {
        let keep = |name: &str| name.starts_with(prefix) && can_escape(name) && !names.contains(name);
        let Some(entries) = disk.list(Path::new(dir), usize::MAX, keep) else { continue };
        names.extend(entries.into_iter().filter(|entry| entry.runnable).map(|entry| entry.name));
    }
    names
        .into_iter()
        .take(MAX_CHOICES)
        .map(|name| Choice {
            insert: escape(&name),
            label: name,
            dir: false,
            recent: false,
        })
        .collect()
}

#[cfg(unix)]
const PATH_SEPARATOR: char = ':';
#[cfg(not(unix))]
const PATH_SEPARATOR: char = ';';

/// Splits a word into its folder part, up to the last slash, and the name.
fn split_word(word: &str) -> (&str, &str) {
    match word.rfind('/') {
        Some(slash) => word.split_at(slash + 1),
        None => ("", word),
    }
}

/// The folder a typed folder part points at. Paths follow the shell's rules,
/// with `/`, even when this computer uses `\`.
fn base_dir(dir_text: &str, cwd: &Path, disk: &Disk) -> Option<PathBuf> {
    if let Some(rest) = dir_text.strip_prefix("~/") {
        Some(disk.home()?.join(rest))
    } else if dir_text.starts_with('/') {
        Some(PathBuf::from(dir_text))
    } else {
        Some(cwd.join(dir_text))
    }
}

/// Names in the folder the word points at that start with its last part.
/// `folders` keeps only folders. With `listings`, for suggestions while
/// typing, names that start the same in another case count too, or failing
/// that, ones with the part anywhere in them.
fn paths(word: &str, cwd: &Path, disk: &Disk, folders: bool, listings: Option<&Listings>) -> Vec<Choice> {
    let loose = listings.is_some();
    // Keep what the user typed for the folder part, and only replace the name.
    let (typed_dir, typed_name) = split_word(word);
    let dir_text = unescape(typed_dir);
    let name = unescape(typed_name);
    if dir_text == "~" || (dir_text.is_empty() && name == "~") {
        return vec![Choice {
            label: "~/".into(),
            insert: "~/".into(),
            dir: true,
            recent: false,
        }];
    }
    let Some(base) = base_dir(&dir_text, cwd, disk) else { return Vec::new() };
    let lower = name.to_lowercase();
    // 0: starts with the name, 1: the same in another case, 2: has it anywhere.
    let rank = |file: &str| -> Option<u8> {
        if file.starts_with(&name) {
            Some(0)
        } else if !loose {
            None
        } else if file.to_lowercase().starts_with(&lower) {
            Some(1)
        } else if !name.is_empty() && file.to_lowercase().contains(&lower) {
            Some(2)
        } else {
            None
        }
    };
    let keep = |file: &str| rank(file).is_some() && (!file.starts_with('.') || name.starts_with('.')) && can_escape(file);
    let entries = match listings {
        Some(listings) => listings.list(&base, disk),
        None => disk.list(&base, usize::MAX, keep),
    };
    let Some(entries) = entries else { return Vec::new() };
    let mut found: Vec<(u8, String, bool)> = entries
        .into_iter()
        .filter(|entry| (entry.dir || !folders) && keep(&entry.name))
        .filter_map(|entry| Some((rank(&entry.name)?, entry.name, entry.dir)))
        .collect();
    // Names that only have the part somewhere count when nothing starts with it.
    if found.iter().any(|(rank, ..)| *rank < 2) {
        found.retain(|(rank, ..)| *rank < 2);
    }
    if loose {
        found.sort_by_cached_key(|(rank, file, _)| (*rank, file.to_lowercase()));
    } else {
        found.sort();
    }
    found.truncate(MAX_CHOICES);
    let mut choices: Vec<Choice> = found
        .into_iter()
        .map(|(_, file, dir)| {
            let slash = if dir { "/" } else { "" };
            Choice {
                label: format!("{file}{slash}"),
                insert: format!("{typed_dir}{}{slash}", escape(&file)),
                dir,
                recent: false,
            }
        })
        .collect();
    // `..` is never listed, but `cd ..` is common.
    if folders && name.starts_with('.') && "..".starts_with(name.as_str()) {
        choices.insert(0, Choice {
            label: "../".into(),
            insert: format!("{typed_dir}../"),
            dir: true,
            recent: false,
        });
    }
    choices
}

/// Folders from past `cd` commands that fit the word: ones under the folder
/// typed, or whose name starts with it. Only full paths count, as others
/// depended on where the shell was. Skips `skip` and folders that are gone.
fn recent_folders(word: &str, recent: &[String], cwd: &Path, skip: &[PathBuf], disk: &Disk) -> Vec<Choice> {
    let typed = unescape(word);
    let full = typed.starts_with(['/', '~']);
    if typed.contains('/') && !full {
        return Vec::new();
    }
    let lower = typed.to_lowercase();
    let home = disk.home();
    let mut seen = HashSet::new();
    recent
        .iter()
        .filter_map(|target| {
            let target = unescape(target);
            let target = target.trim_end_matches('/');
            let (path, insert) = if let Some(rest) = target.strip_prefix("~/") {
                (home.as_ref()?.join(rest), format!("~/{}/", escape(rest)))
            } else if target.starts_with('/') && target.len() > 1 {
                (PathBuf::from(target), format!("{}/", escape(target)))
            } else {
                return None;
            };
            let fits = if full {
                target.starts_with(typed.as_str()) && target.len() > typed.trim_end_matches('/').len()
            } else {
                let name = target.rsplit('/').next().unwrap_or_default();
                name.to_lowercase().starts_with(&lower)
            };
            let new = path != cwd && !skip.contains(&path) && can_escape(target) && seen.insert(path.clone());
            (fits && new).then_some((path, insert))
        })
        .take(MAX_RECENT_CHECKS)
        .filter(|(path, _)| disk.is_dir(path) == Some(true))
        .take(MAX_RECENT)
        .map(|(_, insert)| Choice {
            label: insert.clone(),
            insert,
            dir: true,
            recent: true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::create_dir(dir.path().join("my docs")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "").unwrap();
        std::fs::write(dir.path().join("src/mod.rs"), "").unwrap();
        std::fs::write(dir.path().join(".hidden"), "").unwrap();
        dir
    }

    #[test]
    fn completes_a_folder_name() {
        let dir = folder();
        let done = complete("cd s", 4, dir.path(), None, &Disk::Local).unwrap();
        assert_eq!(done.range, 3..4);
        assert_eq!(done.common().as_deref(), Some("src/"));
    }

    #[test]
    fn names_with_control_characters_cannot_be_escaped() {
        assert!(can_escape("My Files"));
        assert!(!can_escape("x\nevil"));
        assert!(!can_escape("x\revil"));
    }

    #[test]
    fn escapes_spaces_and_keeps_the_typed_folder() {
        let dir = folder();
        let done = complete("ls my", 5, dir.path(), None, &Disk::Local).unwrap();
        assert_eq!(done.common().as_deref(), Some("my\\ docs/"));
        let done = complete("cat src/m", 9, dir.path(), None, &Disk::Local).unwrap();
        assert_eq!(done.choices.len(), 2);
        assert_eq!(done.common().as_deref(), Some("src/m"));
    }

    #[test]
    fn hides_dot_files_unless_asked() {
        let dir = folder();
        let all = complete("ls ", 3, dir.path(), None, &Disk::Local).unwrap();
        assert!(all.choices.iter().all(|c| !c.label.starts_with('.')));
        let dots = complete("ls .h", 5, dir.path(), None, &Disk::Local).unwrap();
        assert_eq!(dots.common().as_deref(), Some(".hidden "));
    }

    #[test]
    fn completes_commands_from_path() {
        let dir = folder();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let tool = bin.join("sommetool");
        std::fs::write(&tool, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = bin.to_string_lossy().into_owned();
        let done = complete("echo hi | sommet", 16, dir.path(), Some(&path), &Disk::Local).unwrap();
        assert_eq!(done.common().as_deref(), Some("sommetool "));
    }

    #[test]
    fn cd_lists_folders_only() {
        let dir = folder();
        std::fs::write(dir.path().join("setup.txt"), "").unwrap();
        let done = complete("cd s", 4, dir.path(), None, &Disk::Local).unwrap();
        assert_eq!(done.choices.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["src/"]);
        let ls = complete("ls s", 4, dir.path(), None, &Disk::Local).unwrap();
        assert_eq!(ls.choices.len(), 2);
    }

    #[test]
    fn suggests_local_folders_then_recent_ones() {
        let dir = folder();
        std::fs::create_dir(dir.path().join("Projects")).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::create_dir(elsewhere.path().join("pictures")).unwrap();
        let far = elsewhere.path().join("pictures").to_string_lossy().replace('\\', "/");
        let gone = format!("{far}-gone");
        let commands = [format!("cd {far}"), format!("cd {gone}"), "cd src".into(), "ls".into()];
        let recent = cd_targets(commands.iter().map(String::as_str));
        assert_eq!(recent, [far.clone(), gone, "src".into()]);
        let listings = Listings::default();
        let near = |line: &str| suggest_folders(line, line.len(), dir.path(), &recent, &listings, &Disk::Local).unwrap();
        let labels = |done: Completion| done.choices.into_iter().map(|c| c.label).collect::<Vec<_>>();
        // Folders here first, in any case, then the recent one that is still there.
        assert_eq!(labels(near("cd ")), ["my docs/", "Projects/", "src/", &format!("{far}/")]);
        assert_eq!(labels(near("cd p")), ["Projects/", &format!("{far}/")]);
        // Nothing starts with "doc", so names that have it count.
        assert_eq!(labels(near("cd doc")), ["my docs/"]);
        assert_eq!(labels(near("cd ..")), ["../"]);
        assert_eq!(labels(near("cd ./s")), ["src/"]);
        assert!(suggest_folders("ls ", 3, dir.path(), &recent, &listings, &Disk::Local).is_none());
        assert!(suggest_folders("cd src ", 7, dir.path(), &recent, &listings, &Disk::Local).is_none());
        assert!(typing_folder("git pull && cd sr", 17));
        assert!(!typing_folder("cd -", 4));
    }

    #[test]
    fn suggests_folders_on_a_server() {
        use crate::terminal::disk::fake;

        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home/me");
        std::fs::create_dir_all(home.join("proj/deep")).unwrap();
        std::fs::create_dir_all(home.join("Pics")).unwrap();
        std::fs::create_dir_all(root.path().join("var/log")).unwrap();
        std::fs::write(home.join("notes.txt"), "").unwrap();
        let mut here = vec!["Pics/", "proj/"];
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.path().join("var/log"), home.join("logs")).unwrap();
            here.insert(0, "logs/");
        }
        let disk = fake::disk(root.path());
        let listings = Listings::default();
        let recent = cd_targets(["cd /var/log", "cd ~/gone", "cd proj"].into_iter());
        let cwd = Path::new(fake::HOME);
        let labels = |line: &str| {
            let found = suggest_folders(line, line.len(), cwd, &recent, &listings, &disk);
            found.map(|done| done.choices.into_iter().map(|c| c.label).collect::<Vec<_>>()).unwrap_or_default()
        };
        // Folders here, links to folders too, then the recent one still there.
        assert_eq!(labels("cd "), [here.as_slice(), &["/var/log/"]].concat());
        assert_eq!(labels("cd ~/p"), ["proj/", "Pics/"]);
        assert_eq!(labels("cd proj/"), ["deep/"]);
        assert_eq!(labels("cd /v"), ["var/", "/var/log/"]);
        assert_eq!(labels("cd ~"), ["~/"]);
        assert_eq!(labels("cd ../"), ["me/"]);
        // Tab completion goes to the server too.
        let done = complete("cd ~/pr", 7, cwd, None, &disk).unwrap();
        assert_eq!(done.common().as_deref(), Some("~/proj/"));
    }

    #[test]
    fn word_start_skips_escaped_spaces() {
        assert_eq!(word_start("ls my\\ do", 9), 3);
        assert_eq!(word_start("a=b", 3), 2);
    }
}
