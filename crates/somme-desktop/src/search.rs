//! Finds files and folders by name, or files by the text in them, for the
//! command palette. Folders on this computer are walked the way ripgrep walks
//! them, in parallel and skipping hidden and git-ignored files. The whole Mac
//! is searched with Spotlight, which has it indexed already. On a server,
//! `find` and `grep` run there.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use ignore::{WalkBuilder, WalkState};
use regex::bytes::{Regex, RegexBuilder};

use crate::terminal::disk::Disk;

/// Hits shown at most. Nobody reads further; a better query beats scrolling.
pub const MAX_HITS: usize = 200;
/// Bigger files are skipped when looking for text.
const MAX_TEXT_FILE: u64 = 16 * 1024 * 1024;
/// Lines shown from one file.
const PER_FILE: usize = 5;
/// Characters kept of a line with a hit.
const LINE_CHARS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Find {
    /// Files and folders whose names hold the query.
    Names,
    /// Lines in files that hold the query.
    Text,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    pub folder: bool,
    /// For text: the line's number, from 1, and the line.
    pub line: Option<(u32, String)>,
}

/// Where to look.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    Folder(PathBuf),
    /// Everything on this computer.
    Computer,
}

/// A query with no capitals ignores case, as in ripgrep and Vim.
pub fn ignore_case(query: &str) -> bool {
    !query.chars().any(char::is_uppercase)
}

/// Looks for `query` and hands each hit to `found` as it turns up, until
/// [`MAX_HITS`], the end, or `stop`. Slow: run it in [`Disk::run`].
pub fn search(
    disk: &Disk,
    place: &Place,
    find: Find,
    query: &str,
    stop: &AtomicBool,
    found: &(dyn Fn(Hit) + Sync),
) -> Result<(), String> {
    match (disk, place) {
        (Disk::Local, Place::Folder(dir)) => {
            walk(&[dir.clone()], find, query, stop, found);
            Ok(())
        }
        (Disk::Local, Place::Computer) => computer(find, query, stop, found),
        (Disk::Remote { .. }, Place::Folder(dir)) => {
            let hits = disk
                .server_search(dir, query, find == Find::Text, ignore_case(query), MAX_HITS, stop)
                .map_err(|err| err.to_string())?;
            for hit in hits {
                found(Hit {
                    path: PathBuf::from(hit.path),
                    folder: hit.is_dir,
                    line: hit.line,
                });
            }
            Ok(())
        }
        (Disk::Remote { .. }, Place::Computer) => Err("A server is searched one folder at a time.".into()),
    }
}

fn text_finder(query: &str) -> Option<Regex> {
    RegexBuilder::new(&regex::escape(query)).case_insensitive(ignore_case(query)).build().ok()
}

/// Walks `roots` with several threads.
pub fn walk(roots: &[PathBuf], find: Find, query: &str, stop: &AtomicBool, found: &(dyn Fn(Hit) + Sync)) {
    let Some((first, rest)) = roots.split_first() else { return };
    let lower = ignore_case(query).then(|| query.to_lowercase());
    let finder = match find {
        Find::Names => None,
        Find::Text => match text_finder(query) {
            Some(finder) => Some(finder),
            None => return,
        },
    };
    let count = AtomicUsize::new(0);
    let mut walker = WalkBuilder::new(first);
    for root in rest {
        walker.add(root);
    }
    // The kernel's own folders are endless or never hold anything to find.
    walker.filter_entry(|entry| !matches!(entry.path().to_str(), Some("/proc" | "/sys" | "/dev")));
    walker.build_parallel().run(|| {
        let (count, lower, finder) = (&count, &lower, &finder);
        Box::new(move |entry| {
            if stop.load(Ordering::Relaxed) || count.load(Ordering::Relaxed) >= MAX_HITS {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else { return WalkState::Continue };
            if entry.depth() == 0 {
                return WalkState::Continue;
            }
            let folder = entry.file_type().is_some_and(|kind| kind.is_dir());
            match finder {
                None => {
                    let name = entry.file_name().to_string_lossy();
                    let matched = match lower {
                        Some(lower) => name.to_lowercase().contains(lower.as_str()),
                        None => name.contains(query),
                    };
                    if matched && count.fetch_add(1, Ordering::Relaxed) < MAX_HITS {
                        found(Hit {
                            path: entry.into_path(),
                            folder,
                            line: None,
                        });
                    }
                }
                Some(_) if folder || entry.metadata().map_or(true, |meta| meta.len() > MAX_TEXT_FILE) => {}
                Some(finder) => {
                    for line in lines_with(entry.path(), finder) {
                        if count.fetch_add(1, Ordering::Relaxed) >= MAX_HITS {
                            break;
                        }
                        found(Hit {
                            path: entry.path().to_path_buf(),
                            folder: false,
                            line: Some(line),
                        });
                    }
                }
            }
            WalkState::Continue
        })
    });
}

/// The first lines of a text file that `finder` matches, with their numbers.
fn lines_with(path: &Path, finder: &Regex) -> Vec<(u32, String)> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    if memchr::memchr(0, &bytes[..bytes.len().min(8192)]).is_some() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let (mut number, mut counted, mut last_line) = (1, 0, None);
    for found in finder.find_iter(&bytes) {
        let start = found.start();
        number += memchr::memchr_iter(b'\n', &bytes[counted..start]).count() as u32;
        counted = start;
        if last_line == Some(number) {
            continue;
        }
        last_line = Some(number);
        let begin = memchr::memrchr(b'\n', &bytes[..start]).map_or(0, |at| at + 1);
        let end = memchr::memchr(b'\n', &bytes[start..]).map_or(bytes.len(), |at| start + at);
        let text: String = String::from_utf8_lossy(&bytes[begin..end]).trim().chars().take(LINE_CHARS).collect();
        lines.push((number, text));
        if lines.len() == PER_FILE {
            break;
        }
    }
    lines
}

#[cfg(target_os = "macos")]
fn computer(find: Find, query: &str, stop: &AtomicBool, found: &(dyn Fn(Hit) + Sync)) -> Result<(), String> {
    use std::io::BufRead as _;
    use std::process::{Command, Stdio};

    let mut spotlight = Command::new("/usr/bin/mdfind");
    match find {
        Find::Names => spotlight.arg("-name").arg(query),
        Find::Text => spotlight.arg(query),
    };
    let mut child = spotlight
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("Spotlight did not start: {err}"))?;
    let finder = text_finder(query);
    let mut count = 0;
    let out = std::io::BufReader::new(child.stdout.take().expect("piped"));
    for line in out.lines() {
        let Ok(line) = line else { break };
        if stop.load(Ordering::Relaxed) || count >= MAX_HITS {
            break;
        }
        let path = PathBuf::from(line);
        let folder = path.is_dir();
        // Spotlight also finds words in PDFs and documents, which have no lines to show.
        let lines = match (&finder, find, folder) {
            (Some(finder), Find::Text, false) => lines_with(&path, finder),
            _ => Vec::new(),
        };
        if lines.is_empty() {
            count += 1;
            found(Hit { path, folder, line: None });
            continue;
        }
        for line in lines.into_iter().take(MAX_HITS - count) {
            count += 1;
            found(Hit {
                path: path.clone(),
                folder: false,
                line: Some(line),
            });
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

/// Without an index, every drive is walked.
#[cfg(not(target_os = "macos"))]
fn computer(find: Find, query: &str, stop: &AtomicBool, found: &(dyn Fn(Hit) + Sync)) -> Result<(), String> {
    #[cfg(windows)]
    let roots: Vec<PathBuf> =
        (b'A'..=b'Z').map(|drive| PathBuf::from(format!("{}:\\", drive as char))).filter(|root| root.exists()).collect();
    #[cfg(not(windows))]
    let roots = vec![PathBuf::from("/")];
    walk(&roots, find, query, stop, found);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/Order Query")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        std::fs::write(root.join("src/order.rs"), "fn main() {}\nlet order = 1; // Order\n\norder again\n").unwrap();
        std::fs::write(root.join("src/notes.txt"), "nothing\n").unwrap();
        std::fs::write(root.join(".hidden/order"), "order\n").unwrap();
        std::fs::write(root.join("src/bin.dat"), b"order\0\x01").unwrap();
        dir
    }

    fn run(disk: &Disk, place: &Place, find: Find, query: &str) -> Vec<Hit> {
        let hits = Mutex::new(Vec::new());
        search(disk, place, find, query, &AtomicBool::new(false), &|hit| hits.lock().unwrap().push(hit)).unwrap();
        let mut hits = hits.into_inner().unwrap();
        hits.sort_by(|a, b| (&a.path, &a.line).cmp(&(&b.path, &b.line)));
        hits
    }

    #[test]
    fn finds_names_and_text_on_this_computer() {
        let dir = tree();
        let place = Place::Folder(dir.path().to_path_buf());
        let names = run(&Disk::Local, &place, Find::Names, "order");
        let names: Vec<_> = names.iter().map(|hit| (hit.path.strip_prefix(dir.path()).unwrap().to_owned(), hit.folder)).collect();
        assert_eq!(names, [(PathBuf::from("src/Order Query"), true), (PathBuf::from("src/order.rs"), false)]);
        assert_eq!(run(&Disk::Local, &place, Find::Names, "Order").len(), 1, "capitals match case");

        let text = run(&Disk::Local, &place, Find::Text, "order");
        let lines: Vec<_> = text.iter().map(|hit| hit.line.clone().unwrap()).collect();
        assert_eq!(lines, [(2, "let order = 1; // Order".to_owned()), (4, "order again".to_owned())]);
        assert!(text.iter().all(|hit| hit.path.ends_with("src/order.rs")));
    }

    #[test]
    fn finds_files_on_a_server() {
        use crate::terminal::disk::fake;

        let dir = tree();
        let disk = fake::disk(dir.path());
        let names = run(&disk, &Place::Folder("/src".into()), Find::Names, "order");
        let names: Vec<_> = names.iter().map(|hit| (hit.path.clone(), hit.folder)).collect();
        assert_eq!(names, [(PathBuf::from("/src/Order Query"), true), (PathBuf::from("/src/order.rs"), false)]);
        let text = run(&disk, &Place::Folder("/".into()), Find::Text, "again");
        assert_eq!(text.len(), 1);
        assert_eq!(text[0].path, Path::new("/src/order.rs"));
        assert_eq!(text[0].line, Some((4, "order again".into())));
    }

    #[test]
    fn stops_when_asked() {
        let dir = tree();
        let hits = Mutex::new(Vec::new());
        let place = Place::Folder(dir.path().to_path_buf());
        search(&Disk::Local, &place, Find::Names, "o", &AtomicBool::new(true), &|hit| hits.lock().unwrap().push(hit))
            .unwrap();
        assert!(hits.lock().unwrap().is_empty());
    }
}
