//! File and folder names in command output. Hovering one underlines it; a
//! click goes into the folder, or opens the file beside the terminal.

use std::ops::Range;
use std::path::{Path, PathBuf};

use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::index::{Column, Line, Point as GridPoint};
use alacritty_terminal::term::cell::Flags;
use gpui_kit::{Context, Pixels, Point, Window};

use super::{TerminalView, TerminalViewEvent, picker};
use crate::terminal::blocks::Block;
use crate::terminal::complete;
use crate::terminal::disk::Disk;

/// Words on each side of the hovered one that may join it, for names with spaces.
const JOIN_WORDS: usize = 2;
/// Longer text is not looked up.
const MAX_NAME: usize = 1024;
/// Commands whose output lists the folders named in their arguments.
const LISTERS: &[&str] = &["ls", "ll", "la", "l", "eza", "exa", "lsd", "tree"];

/// A name in a block's output that is a real file or folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PathLink {
    pub block: usize,
    pub line: Line,
    /// Columns it covers.
    pub columns: Range<usize>,
    path: PathBuf,
    dir: bool,
}

impl PathLink {
    fn contains(&self, block: usize, point: GridPoint) -> bool {
        self.block == block && self.line == point.line && self.columns.contains(&point.column.0)
    }
}

/// The word last looked up, so moving within it does not look again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Probe {
    block: usize,
    line: Line,
    start: usize,
}

/// Text that may name a file, and the columns it covers.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Candidate {
    text: String,
    columns: Range<usize>,
}

/// A row's characters: column, character, and how many columns it takes.
type Row = Vec<(usize, char, usize)>;

impl TerminalView {
    pub(crate) fn hovered_link(&self) -> Option<&PathLink> {
        self.link.as_ref()
    }

    pub(super) fn clear_link(&mut self, cx: &mut Context<Self>) {
        self.link_probe = None;
        if self.link.take().is_some() {
            cx.notify();
        }
    }

    /// Looks for a file or folder name under the mouse. The disk is checked off
    /// the UI thread, as a path can sit on a slow network drive or a server.
    pub(super) fn hover_link(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((block, point)) = self.output_cell(position) else { return self.clear_link(cx) };
        if self.link.as_ref().is_some_and(|link| link.contains(block, point)) {
            return;
        }
        let session = self.session.read(cx);
        let found = session.blocks.as_ref().and_then(|blocks| {
            let entry = blocks.list.get(block)?;
            let (start, candidates) = candidates(&row(entry, point.line)?, point.column.0)?;
            let bases = bases(entry.cwd.as_deref()?, &entry.command, blocks.home.as_deref());
            Some((start, candidates, bases))
        });
        let disk = session.disk();
        let Some((start, candidates, bases)) = found else { return self.clear_link(cx) };
        let probe = Probe {
            block,
            line: point.line,
            start,
        };
        if self.link_probe == Some(probe) {
            return;
        }
        self.clear_link(cx);
        self.link_probe = Some(probe);
        cx.spawn(async move |this, cx| {
            let Some(disk) = disk.await else { return };
            let found = disk.run(move |disk| find(&candidates, &bases, disk)).await;
            let _ = this.update(cx, |view, cx| {
                if view.link_probe == Some(probe) {
                    view.link = found.map(|(columns, path, dir)| PathLink {
                        block,
                        line: point.line,
                        columns,
                        path,
                        dir,
                    });
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The link under the mouse, if any.
    pub(super) fn link_at(&self, position: Point<Pixels>) -> Option<PathLink> {
        let (block, point) = self.output_cell(position)?;
        self.link.clone().filter(|link| link.contains(block, point))
    }

    /// Goes into a folder with `cd`, or opens a file beside the terminal.
    pub(super) fn open_link(&mut self, link: PathLink, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_link(cx);
        if !link.dir {
            cx.emit(TerminalViewEvent::OpenFile(link.path));
            return;
        }
        let cwd = self.session.read(cx).blocks.as_ref().and_then(|blocks| blocks.cwd.clone());
        // Short and readable when the folder is under the current one.
        let relative = cwd
            .as_deref()
            .and_then(|cwd| link.path.strip_prefix(cwd).ok())
            .filter(|relative| !relative.as_os_str().is_empty());
        let target = relative.unwrap_or(&link.path).to_string_lossy();
        self.run_from_ui(format!("cd {}", picker::cd_target(&target)), window, cx);
    }

    /// The block and output cell under the mouse, when it is over a block's output.
    fn output_cell(&self, position: Point<Pixels>) -> Option<(usize, GridPoint)> {
        let layout = self.blocks_layout.as_ref()?;
        let block = layout.block_at(position.y)?;
        let geom = layout.blocks[block];
        let (x, y) = (position.x - geom.output_origin.x, position.y - geom.output_origin.y);
        if x < Pixels::ZERO || y < Pixels::ZERO {
            return None;
        }
        let row = (y / layout.line_height).floor() as usize;
        let column = (x / layout.cell_width).floor() as usize;
        (row < geom.rows && column < layout.columns)
            .then(|| (block, GridPoint::new(geom.first_line + row as i32, Column(column))))
    }
}

/// The characters on one output row.
fn row(block: &Block, line: Line) -> Option<Row> {
    let grid = block.term.grid();
    if line < grid.topmost_line() || line > grid.bottommost_line() {
        return None;
    }
    let cells = &grid[line];
    let spacer = Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER;
    Some(
        (0..grid.columns())
            .map(|column| (column, &cells[Column(column)]))
            .filter(|(_, cell)| !cell.flags.intersects(spacer))
            .map(|(column, cell)| (column, cell.c, if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 }))
            .collect(),
    )
}

/// Text around `column` that may name a file, longest first: the word under the
/// mouse, and that word joined to its neighbours for names with spaces. Also
/// gives the column where the hovered word starts.
fn candidates(row: &Row, column: usize) -> Option<(usize, Vec<Candidate>)> {
    let at = row.iter().rposition(|(start, _, _)| *start <= column)?;
    if row[at].1.is_whitespace() {
        return None;
    }
    let mut words: Vec<Range<usize>> = Vec::new();
    for (i, (_, ch, _)) in row.iter().enumerate() {
        match words.last_mut() {
            _ if ch.is_whitespace() => {}
            Some(word) if word.end == i => word.end = i + 1,
            _ => words.push(i..i + 1),
        }
    }
    let hovered = words.iter().position(|word| word.contains(&at))?;
    // Joins only across single spaces: columns of output are padded wider.
    let joins = |from: usize, to: usize| (from..to).all(|i| words[i + 1].start == words[i].end + 1);
    let mut found = Vec::new();
    for left in hovered.saturating_sub(JOIN_WORDS)..=hovered {
        for right in hovered..=(hovered + JOIN_WORDS).min(words.len() - 1) {
            if !joins(left, right) {
                continue;
            }
            let span = words[left].start..words[right].end;
            let chars: Vec<char> = row[span.clone()].iter().map(|(_, ch, _)| *ch).collect();
            let name = trim_name(&chars);
            if name.is_empty() || name.len() > MAX_NAME {
                continue;
            }
            let (first, last) = (row[span.start + name.start], row[span.start + name.end - 1]);
            let text: String = chars[name].iter().collect();
            if complete::can_escape(&text) && !found.iter().any(|c: &Candidate| c.text == text) {
                found.push(Candidate {
                    text,
                    columns: first.0..last.0 + last.2,
                });
            }
        }
    }
    found.sort_by_key(|candidate| std::cmp::Reverse(candidate.text.chars().count()));
    Some((row[words[hovered].start].0, found))
}

/// The part of a word that may be a path: without quotes or brackets around it,
/// sentence marks after it, or a `:line:column` ending.
fn trim_name(chars: &[char]) -> Range<usize> {
    let (mut start, mut end) = (0, chars.len());
    while start < end && "'\"`([{<".contains(chars[start]) {
        start += 1;
    }
    loop {
        let before = end;
        while end > start && "'\"`)]}>,;:".contains(chars[end - 1]) {
            end -= 1;
        }
        let digits = chars[start..end].iter().rev().take_while(|ch| ch.is_ascii_digit()).count();
        if digits > 0 && end - digits > start && chars[end - digits - 1] == ':' {
            end -= digits + 1;
        }
        // A full stop ends a sentence, but `.` and `..` are folders.
        if end > start && chars[end - 1] == '.' && chars[start..end].iter().any(|ch| *ch != '.') {
            end -= 1;
        }
        if end == before {
            return start..end;
        }
    }
}

/// Folders a relative name is looked up in: where the command ran, and for
/// `ls` and the like, the folders it listed.
fn bases(cwd: &Path, command: &str, home: Option<&Path>) -> Vec<PathBuf> {
    let mut bases = vec![cwd.to_path_buf()];
    let mut words = command.split_whitespace();
    let program = words.next().unwrap_or_default();
    if LISTERS.contains(&program.rsplit('/').next().unwrap_or_default()) {
        for word in words.filter(|word| !word.starts_with('-')) {
            let word = word.trim_matches(['\'', '"']);
            if let Some(path) = resolve(word, cwd, home) {
                bases.push(path);
            }
        }
    }
    bases
}

fn resolve(text: &str, base: &Path, home: Option<&Path>) -> Option<PathBuf> {
    if text == "~" {
        return home.map(Path::to_path_buf);
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return home.map(|home| home.join(rest));
    }
    Some(base.join(text))
}

/// The first candidate that is on disk, with its columns and whether it is a folder.
fn find(candidates: &[Candidate], bases: &[PathBuf], disk: &Disk) -> Option<(Range<usize>, PathBuf, bool)> {
    let home = disk.home();
    let mut seen = Vec::new();
    for candidate in candidates {
        for base in bases {
            let Some(path) = resolve(&candidate.text, base, home.as_deref()) else { continue };
            if seen.contains(&path) {
                continue;
            }
            if let Some(dir) = disk.is_dir(&path) {
                return Some((candidate.columns.clone(), path, dir));
            }
            seen.push(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str) -> Row {
        text.chars().enumerate().map(|(i, ch)| (i, ch, 1)).collect()
    }

    fn texts(text: &str, column: usize) -> Vec<String> {
        candidates(&row(text), column)
            .map(|(_, found)| found.into_iter().map(|c| c.text).collect())
            .unwrap_or_default()
    }

    #[test]
    fn trims_quotes_marks_and_line_numbers() {
        let trimmed = |text: &str| {
            let chars: Vec<char> = text.chars().collect();
            chars[trim_name(&chars)].iter().collect::<String>()
        };
        assert_eq!(trimmed("'My Files'"), "My Files");
        assert_eq!(trimmed("src/main.rs:12:5"), "src/main.rs");
        assert_eq!(trimmed("(README.md)."), "README.md");
        assert_eq!(trimmed(".."), "..");
        assert_eq!(trimmed("lib.rs:"), "lib.rs");
    }

    #[test]
    fn joins_words_only_across_single_spaces() {
        // Hovering "My": up to two words each side.
        let found = texts("Oct 17 12:20 My Files", 13);
        assert_eq!(found[0], "17 12:20 My Files");
        assert!(found.contains(&"My Files".to_owned()));
        assert!(found.contains(&"My".to_owned()));
        assert!(!found.iter().any(|text| text.contains("Oct")));
        let found = texts("staff    846 fly.toml", 14);
        assert!(found.contains(&"fly.toml".to_owned()));
        assert!(!found.iter().any(|text| text.contains("staff")));
        assert!(texts("a  b", 1).is_empty());
    }

    #[test]
    fn columns_follow_wide_characters() {
        let row: Row = vec![(0, '日', 2), (2, '.', 1), (3, 't', 1), (4, ' ', 1), (5, 'x', 1)];
        let (start, found) = candidates(&row, 1).unwrap();
        assert_eq!(start, 0);
        let name = found.iter().find(|c| c.text == "日.t").unwrap();
        assert_eq!(name.columns, 0..4);
    }

    #[test]
    fn finds_names_in_the_listed_folder() {
        let dir = std::env::temp_dir().join(format!("somme-links-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub/inner")).unwrap();
        std::fs::write(dir.join("sub/notes.txt"), "").unwrap();
        let bases = bases(&dir, "ls -l sub", None);
        let (_, found) = candidates(&row("-rw-r--r-- 1 me staff 0 notes.txt"), 26).unwrap();
        let hit = find(&found, &bases, &Disk::Local);
        let missing = find(&candidates(&row("nothing-here"), 2).unwrap().1, &bases, &Disk::Local);
        let folder = find(&candidates(&row("inner"), 2).unwrap().1, &bases, &Disk::Local);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(hit, Some((24..33, dir.join("sub/notes.txt"), false)));
        assert_eq!(missing, None);
        assert!(folder.is_some_and(|(_, _, dir)| dir));
    }
}
