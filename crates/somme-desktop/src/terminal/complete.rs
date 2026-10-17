//! Tab completion for the command editor: command names from the shell's PATH,
//! and files and folders.

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// Most choices kept, so a huge folder stays quick.
const MAX_CHOICES: usize = 500;

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

/// Completes the word before `cursor` (a byte offset into `line`).
pub fn complete(line: &str, cursor: usize, cwd: &Path, path_var: Option<&str>) -> Option<Completion> {
    let cursor = cursor.min(line.len());
    let start = word_start(line, cursor);
    let word = &line[start..cursor];
    let before = line[..start].trim_end();
    let command_spot = before.is_empty() || before.ends_with(['|', ';', '&', '(']);
    let choices = if command_spot && !word.contains('/') && !word.is_empty() {
        commands(word, path_var)
    } else {
        paths(word, cwd)
    };
    (!choices.is_empty()).then_some(Completion {
        range: start..cursor,
        choices,
    })
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

fn commands(prefix: &str, path_var: Option<&str>) -> Vec<Choice> {
    let mut names: BTreeSet<String> = BUILTINS
        .iter()
        .filter(|name| name.starts_with(prefix))
        .map(|name| (*name).to_owned())
        .collect();
    let path_var = path_var.map(str::to_owned).or_else(|| std::env::var("PATH").ok()).unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else { continue };
            if name.starts_with(prefix) && !names.contains(&name) && is_executable(&entry.path()) {
                names.insert(name);
            }
        }
    }
    names
        .into_iter()
        .take(MAX_CHOICES)
        .map(|name| Choice {
            insert: escape(&name),
            label: name,
            dir: false,
        })
        .collect()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn paths(word: &str, cwd: &Path) -> Vec<Choice> {
    // Keep what the user typed for the folder part, and only replace the name.
    let (typed_dir, typed_name) = match word.rfind('/') {
        Some(slash) => word.split_at(slash + 1),
        None => ("", word),
    };
    let dir_text = unescape(typed_dir);
    let name = unescape(typed_name);
    let base: PathBuf = if let Some(rest) = dir_text.strip_prefix("~/") {
        dirs::home_dir().unwrap_or_default().join(rest)
    } else if dir_text == "~" || (dir_text.is_empty() && name == "~") {
        return vec![Choice {
            label: "~/".into(),
            insert: "~/".into(),
            dir: true,
        }];
    } else if dir_text.starts_with('/') {
        PathBuf::from(&dir_text)
    } else {
        cwd.join(&dir_text)
    };
    let Ok(entries) = std::fs::read_dir(&base) else { return Vec::new() };
    let mut found: Vec<(String, bool)> = entries
        .flatten()
        .filter_map(|entry| {
            let file = entry.file_name().into_string().ok()?;
            if !file.starts_with(&name) || (file.starts_with('.') && !name.starts_with('.')) {
                return None;
            }
            // Follows links, so a link to a folder counts as a folder.
            let dir = entry.path().is_dir();
            Some((file, dir))
        })
        .collect();
    found.sort();
    found.truncate(MAX_CHOICES);
    found
        .into_iter()
        .map(|(file, dir)| {
            let slash = if dir { "/" } else { "" };
            Choice {
                label: format!("{file}{slash}"),
                insert: format!("{typed_dir}{}{slash}", escape(&file)),
                dir,
            }
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
        let done = complete("cd s", 4, dir.path(), None).unwrap();
        assert_eq!(done.range, 3..4);
        assert_eq!(done.common().as_deref(), Some("src/"));
    }

    #[test]
    fn escapes_spaces_and_keeps_the_typed_folder() {
        let dir = folder();
        let done = complete("ls my", 5, dir.path(), None).unwrap();
        assert_eq!(done.common().as_deref(), Some("my\\ docs/"));
        let done = complete("cat src/m", 9, dir.path(), None).unwrap();
        assert_eq!(done.choices.len(), 2);
        assert_eq!(done.common().as_deref(), Some("src/m"));
    }

    #[test]
    fn hides_dot_files_unless_asked() {
        let dir = folder();
        let all = complete("ls ", 3, dir.path(), None).unwrap();
        assert!(all.choices.iter().all(|c| !c.label.starts_with('.')));
        let dots = complete("ls .h", 5, dir.path(), None).unwrap();
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
        let done = complete("echo hi | sommet", 16, dir.path(), Some(&path)).unwrap();
        assert_eq!(done.common().as_deref(), Some("sommetool "));
    }

    #[test]
    fn word_start_skips_escaped_spaces() {
        assert_eq!(word_start("ls my\\ do", 9), 3);
        assert_eq!(word_start("a=b", 3), 2);
    }
}
