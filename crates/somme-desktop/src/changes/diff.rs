//! Uncommitted changes in a git repo: what `git diff HEAD` prints, plus new files git
//! does not track yet. Or the changes one commit made.

use std::path::{Path, PathBuf};

use crate::terminal::blocks::{git_diff_raw, git_raw};

/// New files bigger than this show without their text.
const MAX_NEW_FILE_BYTES: u64 = 512 * 1024;
/// Only this many new files are listed.
const MAX_NEW_FILES: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    /// New and not added to git yet.
    Untracked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    /// Like `\ No newline at end of file`.
    Note,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// Line number before the change.
    pub old: Option<u32>,
    /// Line number after the change.
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// The `@@ -1,4 +1,5 @@ fn main()` line.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    /// From the repo's top folder.
    pub path: String,
    /// The name before a rename.
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub added: usize,
    pub removed: usize,
    /// Not text, or too big to show.
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    fn new(path: String) -> Self {
        Self {
            path,
            old_path: None,
            status: FileStatus::Modified,
            added: 0,
            removed: 0,
            binary: false,
            hunks: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changes {
    /// The repo's top folder.
    pub root: PathBuf,
    pub branch: String,
    pub files: Vec<FileDiff>,
}

/// Reads the changes of the repo that holds `dir`. Slow: run it off the UI thread.
pub fn load(dir: &Path) -> Result<Changes, String> {
    let root = git_raw(dir, &["rev-parse", "--show-toplevel"])
        .map(|out| PathBuf::from(out.trim_end_matches('\n')))
        .ok_or("This folder is not in a git repo.")?;
    let branch = git_raw(&root, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    // Fails before the first commit; then every file shows as new.
    let patch = git_diff_raw(
        &root,
        &[
            "-c",
            "core.quotePath=false",
            "diff",
            "HEAD",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--find-renames",
            "--ignore-submodules=all",
        ],
    );
    let mut files = patch.as_deref().map(parse).unwrap_or_default();
    let untracked = git_raw(&root, &["ls-files", "--others", "--exclude-standard", "-z"]).unwrap_or_default();
    let new_files = untracked.split('\0').filter(|path| !path.is_empty()).take(MAX_NEW_FILES);
    files.extend(new_files.map(|path| untracked_file(&root, path)));
    Ok(Changes {
        root,
        branch: branch.trim().to_owned(),
        files,
    })
}

/// Whether `sha` is a whole commit id, so it is safe to hand to git.
pub fn is_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Reads what commit `sha` changed, against its first parent, in the repo that
/// holds `dir`. Slow: run it off the UI thread.
pub fn load_commit(dir: &Path, sha: &str) -> Result<Changes, String> {
    if !is_sha(sha) {
        return Err("That is not a commit.".to_owned());
    }
    let root = git_raw(dir, &["rev-parse", "--show-toplevel"])
        .map(|out| PathBuf::from(out.trim_end_matches('\n')))
        .ok_or("This folder is not in a git repo.")?;
    let flags = ["--no-ext-diff", "--no-textconv", "--no-color", "--find-renames", "--ignore-submodules=all"];
    let parent = git_raw(&root, &["rev-parse", "--verify", "--quiet", &format!("{sha}^1")]);
    let patch = match parent.as_deref().map(str::trim).filter(|parent| is_sha(parent)) {
        Some(parent) => git_diff_raw(&root, &[&["-c", "core.quotePath=false", "diff", parent, sha][..], &flags].concat()),
        // The first commit: everything in it is new.
        None => git_diff_raw(&root, &[&["-c", "core.quotePath=false", "diff-tree", "-p", "--root"][..], &flags, &[sha]].concat()),
    }
    .ok_or("Git could not show this commit.")?;
    Ok(Changes {
        root,
        branch: String::new(),
        files: parse(&patch),
    })
}

/// A new file shown as all added lines.
fn untracked_file(root: &Path, path: &str) -> FileDiff {
    let mut file = FileDiff::new(path.to_owned());
    file.status = FileStatus::Untracked;
    let full = root.join(path);
    let Ok(meta) = std::fs::symlink_metadata(&full) else { return file };
    // A link shows where it points, as git does, and is never followed.
    let text = if meta.file_type().is_symlink() {
        std::fs::read_link(&full).map(|target| target.to_string_lossy().into_owned()).ok()
    } else if meta.len() <= MAX_NEW_FILE_BYTES {
        std::fs::read(&full)
            .ok()
            .filter(|bytes| !bytes.iter().take(8192).any(|byte| *byte == 0))
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    } else {
        None
    };
    let Some(text) = text else {
        file.binary = true;
        return file;
    };
    let lines: Vec<DiffLine> = text
        .lines()
        .zip(1..)
        .map(|(line, n)| DiffLine {
            kind: LineKind::Added,
            old: None,
            new: Some(n),
            text: line.to_owned(),
        })
        .collect();
    file.added = lines.len();
    if !lines.is_empty() {
        file.hunks.push(Hunk {
            header: format!("@@ -0,0 +1,{} @@", lines.len()),
            lines,
        });
    }
    file
}

/// Reads the output of `git diff`.
pub fn parse(patch: &str) -> Vec<FileDiff> {
    let mut files = Vec::new();
    let mut current: Option<FileDiff> = None;
    let (mut old, mut new) = (0u32, 0u32);
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            files.extend(current.take());
            current = Some(FileDiff::new(header_path(rest)));
            continue;
        }
        let Some(file) = current.as_mut() else { continue };
        if let Some(range) = line.strip_prefix("@@ ") {
            (old, new) = hunk_start(range);
            file.hunks.push(Hunk {
                header: line.to_owned(),
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = file.hunks.last_mut() else {
            // Lines before the first hunk say what kind of change it is.
            read_file_header(file, line);
            continue;
        };
        let (kind, text) = match line.split_at_checked(1) {
            Some(("+", text)) => (LineKind::Added, text),
            Some(("-", text)) => (LineKind::Removed, text),
            Some(("\\", _)) => (LineKind::Note, line),
            Some((_, text)) => (LineKind::Context, text),
            // An empty context line that lost its space.
            None => (LineKind::Context, ""),
        };
        let (old_no, new_no) = match kind {
            LineKind::Context => (Some(old), Some(new)),
            LineKind::Added => (None, Some(new)),
            LineKind::Removed => (Some(old), None),
            LineKind::Note => (None, None),
        };
        match kind {
            LineKind::Context => (old, new) = (old + 1, new + 1),
            LineKind::Added => {
                new += 1;
                file.added += 1;
            }
            LineKind::Removed => {
                old += 1;
                file.removed += 1;
            }
            LineKind::Note => {}
        }
        hunk.lines.push(DiffLine {
            kind,
            old: old_no,
            new: new_no,
            text: text.to_owned(),
        });
    }
    files.extend(current);
    files
}

fn read_file_header(file: &mut FileDiff, line: &str) {
    if line.starts_with("new file mode") {
        file.status = FileStatus::Added;
    } else if line.starts_with("deleted file mode") {
        file.status = FileStatus::Deleted;
    } else if let Some(from) = line.strip_prefix("rename from ") {
        file.status = FileStatus::Renamed;
        file.old_path = Some(unquote(from));
    } else if let Some(to) = line.strip_prefix("rename to ") {
        file.path = unquote(to);
    } else if let Some(name) = line.strip_prefix("+++ ") {
        if let Some(path) = side_path(name, "b/") {
            file.path = path;
        }
    } else if let Some(name) = line.strip_prefix("--- ") {
        if file.status == FileStatus::Deleted
            && let Some(path) = side_path(name, "a/")
        {
            file.path = path;
        }
    } else if line.starts_with("Binary files ") {
        file.binary = true;
    }
}

/// The path in a `--- a/x` or `+++ b/x` line, None for `/dev/null`.
fn side_path(name: &str, prefix: &str) -> Option<String> {
    // Git adds a tab after names that have a space.
    let name = unquote(name.trim_end_matches('\t'));
    name.strip_prefix(prefix).map(str::to_owned)
}

/// The path in `diff --git a/x b/x`. Only a guess when the two names differ; the
/// lines after it name the file for sure.
fn header_path(rest: &str) -> String {
    if rest.starts_with('"') {
        let names: Vec<&str> = rest.split("\" \"").collect();
        let last = names.last().copied().unwrap_or(rest);
        let unquoted = unquote(&format!("\"{}", last.trim_start_matches('"')));
        return unquoted.strip_prefix("b/").unwrap_or(&unquoted).to_owned();
    }
    // `a/NAME b/NAME`: both halves are the same length when the name did not change.
    if rest.len() >= 5 && (rest.len() - 5).is_multiple_of(2) {
        let len = (rest.len() - 5) / 2;
        if let (Some(a), Some(b)) = (rest.get(2..2 + len), rest.get(5 + len..))
            && a == b
            && rest.starts_with("a/")
        {
            return a.to_owned();
        }
    }
    rest.rsplit_once(" b/").map_or(rest, |(_, name)| name).to_owned()
}

/// Reads `-12,7 +12,8 @@ …` into the first old and new line numbers.
fn hunk_start(range: &str) -> (u32, u32) {
    let mut parts = range.split_whitespace();
    let mut start = |sign: char| {
        parts
            .next()
            .and_then(|part| part.strip_prefix(sign))
            .and_then(|part| part.split(',').next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(1)
    };
    let old = start('-');
    (old, start('+'))
}

/// Undoes git's C-style quoting: `"caf\303\251"` → `café`. Plain names come back as is.
fn unquote(name: &str) -> String {
    let Some(inner) = name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) else {
        return name.to_owned();
    };
    let mut bytes = Vec::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('n') => bytes.push(b'\n'),
            Some('t') => bytes.push(b'\t'),
            Some('r') => bytes.push(b'\r'),
            Some('a') => bytes.push(7),
            Some('b') => bytes.push(8),
            Some('f') => bytes.push(12),
            Some('v') => bytes.push(11),
            Some(d @ '0'..='7') => {
                let mut value = d.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    if let Some(next) = chars.peek().and_then(|c| c.to_digit(8)) {
                        value = value * 8 + next;
                        chars.next();
                    }
                }
                bytes.push(value as u8);
            }
            Some(other) => {
                let mut buf = [0; 4];
                bytes.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "\
diff --git a/src/main.rs b/src/main.rs
index 1111111..2222222 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@ fn main() {
 fn main() {
-    println!(\"hi\");
+    println!(\"hello\");
+    println!(\"again\");
 }
diff --git a/old name.txt b/new name.txt
similarity index 90%
rename from old name.txt
rename to new name.txt
--- a/old name.txt\t
+++ b/new name.txt\t
@@ -5,2 +5,2 @@
-a
+b
\\ No newline at end of file
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
--- not a header
diff --git a/logo.png b/logo.png
new file mode 100644
Binary files /dev/null and b/logo.png differ
";

    #[test]
    fn reads_files_hunks_and_line_numbers() {
        let files = parse(PATCH);
        assert_eq!(files.len(), 4);

        let main = &files[0];
        assert_eq!((main.path.as_str(), main.status, main.added, main.removed), ("src/main.rs", FileStatus::Modified, 2, 1));
        let lines = &main.hunks[0].lines;
        assert_eq!((lines[0].old, lines[0].new), (Some(1), Some(1)));
        assert_eq!((lines[1].kind, lines[1].old, lines[1].new), (LineKind::Removed, Some(2), None));
        assert_eq!((lines[3].kind, lines[3].new), (LineKind::Added, Some(3)));
        assert_eq!((lines[4].old, lines[4].new), (Some(3), Some(4)));

        let renamed = &files[1];
        assert_eq!(renamed.path, "new name.txt");
        assert_eq!(renamed.old_path.as_deref(), Some("old name.txt"));
        assert_eq!(renamed.status, FileStatus::Renamed);
        assert_eq!(renamed.hunks[0].lines.last().map(|l| l.kind), Some(LineKind::Note));

        let gone = &files[2];
        assert_eq!((gone.path.as_str(), gone.status, gone.removed), ("gone.txt", FileStatus::Deleted, 1));
        assert_eq!(gone.hunks[0].lines[0].text, "-- not a header");

        let logo = &files[3];
        assert_eq!((logo.path.as_str(), logo.status, logo.binary), ("logo.png", FileStatus::Added, true));
    }

    #[test]
    fn reads_quoted_names() {
        assert_eq!(unquote("\"tab\\there\""), "tab\there");
        assert_eq!(unquote("\"caf\\303\\251\""), "café");
        assert_eq!(unquote("plain"), "plain");
        assert_eq!(header_path("a/x y.txt b/x y.txt"), "x y.txt");
        assert_eq!(header_path("\"a/q\\\"uote\" \"b/q\\\"uote\""), "q\"uote");
    }
}
