//! The files beside the terminals, as an editor's file tree shows them. It
//! shows the shell's folder, on this computer or on the server an SSH shell
//! runs on, follows the shell when it changes folder, and opens a clicked file
//! in the editor.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, UniformListScrollHandle, Window, div, prelude::FluentBuilder as _, px,
    uniform_list,
};

use crate::terminal::disk::{Disk, Entry};
use crate::ui::common::Lucide;

const ROW: f32 = 24.;
/// How far each level of folders is pushed in.
const INDENT: f32 = 12.;
/// A folder with more names than this shows only the first ones.
const MAX_ENTRIES: usize = 5_000;
/// Names hidden from the tree, as most editors hide them.
const HIDDEN: &[&str] = &[".git", ".hg", ".svn", ".DS_Store", "Thumbs.db"];

pub enum ExplorerEvent {
    Open(PathBuf),
    Close,
}

enum Folder {
    Reading,
    Read(Vec<Entry>),
    Failed,
}

/// One line of the tree as shown.
struct Line {
    path: PathBuf,
    name: SharedString,
    depth: usize,
    dir: bool,
    open: bool,
}

pub struct Explorer {
    disk: Disk,
    root: PathBuf,
    /// Folders shown open.
    open: HashSet<PathBuf>,
    folders: HashMap<PathBuf, Folder>,
    selected: Option<PathBuf>,
    scroll: UniformListScrollHandle,
}

impl EventEmitter<ExplorerEvent> for Explorer {}

impl Explorer {
    pub fn new(disk: Disk, root: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut explorer = Self {
            disk,
            root: root.clone(),
            open: HashSet::new(),
            folders: HashMap::new(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
        };
        explorer.read(root, cx);
        explorer
    }

    /// Shows `root` instead, when the shell has gone to another folder.
    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if root == self.root {
            return;
        }
        if !self.folders.contains_key(&root) {
            self.read(root.clone(), cx);
        }
        self.root = root;
        cx.notify();
    }

    fn read(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        self.folders.insert(dir.clone(), Folder::Reading);
        let disk = self.disk.clone();
        cx.spawn(async move |explorer, cx| {
            let read = disk
                .run({
                    let dir = dir.clone();
                    move |disk| disk.list(&dir, MAX_ENTRIES, |name| !HIDDEN.contains(&name))
                })
                .await;
            let _ = explorer.update(cx, |explorer, cx| {
                let folder = match read {
                    Some(mut entries) => {
                        sort(&mut entries);
                        Folder::Read(entries)
                    }
                    None => Folder::Failed,
                };
                explorer.folders.insert(dir, folder);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn toggle(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if !self.open.remove(&dir) {
            if !self.folders.contains_key(&dir) {
                self.read(dir.clone(), cx);
            }
            self.open.insert(dir);
        }
        cx.notify();
    }

    /// Reads every shown folder again.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.folders.clear();
        let shown: Vec<PathBuf> =
            std::iter::once(self.root.clone()).chain(self.open.iter().filter(|dir| dir.starts_with(&self.root)).cloned()).collect();
        for dir in shown {
            self.read(dir, cx);
        }
    }

    fn collapse(&mut self, cx: &mut Context<Self>) {
        self.open.clear();
        cx.notify();
    }

    /// The tree as shown: each open folder's names under it.
    fn lines(&self) -> Vec<Line> {
        let mut lines = Vec::new();
        self.add_lines(&self.root, 0, &mut lines);
        lines
    }

    fn add_lines(&self, dir: &Path, depth: usize, lines: &mut Vec<Line>) {
        let Some(Folder::Read(entries)) = self.folders.get(dir) else { return };
        for entry in entries {
            let path = dir.join(&entry.name);
            let open = entry.dir && self.open.contains(&path);
            lines.push(Line {
                path: path.clone(),
                name: entry.name.clone().into(),
                depth,
                dir: entry.dir,
                open,
            });
            if open {
                self.add_lines(&path, depth + 1, lines);
            }
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let name = self.root.file_name().map_or_else(|| self.root.to_string_lossy(), |name| name.to_string_lossy()).into_owned();
        h_flex()
            .flex_none()
            .h(px(40.))
            .pl_3()
            .pr_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(Icon::new(Lucide::FolderTree).size(px(16.)))
            .child(div().flex_1().min_w_0().truncate().font_semibold().child(name))
            .child(
                Button::new("explorer-refresh")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::RefreshCw))
                    .tooltip("Refresh")
                    .on_click(cx.listener(|explorer, _, _, cx| explorer.refresh(cx))),
            )
            .child(
                Button::new("explorer-collapse")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::ChevronsDownUp))
                    .tooltip("Collapse folders")
                    .on_click(cx.listener(|explorer, _, _, cx| explorer.collapse(cx))),
            )
            .child(
                Button::new("explorer-close")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::X))
                    .tooltip("Close")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ExplorerEvent::Close))),
            )
    }

    fn render_line(&self, line: &Line, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let selected = self.selected.as_deref() == Some(line.path.as_path());
        let icon = match (line.dir, line.open) {
            (true, true) => Lucide::FolderOpen,
            (true, false) => Lucide::Folder,
            (false, _) => Lucide::File,
        };
        let chevron = if line.open { Lucide::ChevronDown } else { Lucide::ChevronRight };
        let (path, dir) = (line.path.clone(), line.dir);
        h_flex()
            .id(SharedString::from(line.path.to_string_lossy().into_owned()))
            .h(px(ROW))
            .w_full()
            .pl(px(8. + INDENT * line.depth as f32))
            .pr_2()
            .gap_1()
            .items_center()
            .text_sm()
            .cursor_pointer()
            .when(selected, |row| row.bg(theme.accent).text_color(theme.accent_foreground))
            .when(!selected, |row| row.hover(|row| row.bg(theme.muted)))
            .child(
                div()
                    .flex_none()
                    .w(px(14.))
                    .text_color(theme.muted_foreground)
                    .when(dir, |slot| slot.child(Icon::new(chevron).size(px(14.)))),
            )
            .child(
                Icon::new(icon)
                    .size(px(14.))
                    .text_color(if dir { theme.primary } else { theme.muted_foreground }),
            )
            .child(div().flex_1().min_w_0().truncate().child(line.name.clone()))
            .on_click(cx.listener(move |explorer, _, _, cx| {
                explorer.selected = Some(path.clone());
                if dir {
                    explorer.toggle(path.clone(), cx);
                } else {
                    cx.emit(ExplorerEvent::Open(path.clone()));
                    cx.notify();
                }
            }))
            .into_any_element()
    }
}

/// Folders first, then files, each by name whatever the case.
fn sort(entries: &mut [Entry]) {
    entries.sort_by_cached_key(|entry| (!entry.dir, entry.name.to_lowercase()));
}

impl Render for Explorer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let note = |text: &'static str| div().p_3().text_sm().text_color(theme.muted_foreground).child(text).into_any_element();
        let body = match self.folders.get(&self.root) {
            None | Some(Folder::Reading) => note("Reading the folder…"),
            Some(Folder::Failed) => note("This folder can't be read."),
            Some(Folder::Read(entries)) if entries.is_empty() => note("This folder is empty."),
            Some(Folder::Read(_)) => {
                let lines = Rc::new(self.lines());
                uniform_list(
                    "explorer-lines",
                    lines.len(),
                    cx.processor(move |explorer, range: std::ops::Range<usize>, _, cx| {
                        range.map(|ix| explorer.render_line(&lines[ix], cx)).collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .py_1()
                .track_scroll(&self.scroll)
                .into_any_element()
            }
        };
        v_flex()
            .size_full()
            .bg(theme.background)
            .border_r_1()
            .border_color(theme.border)
            .child(self.render_header(cx))
            .child(v_flex().flex_1().min_h_0().child(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, dir: bool) -> Entry {
        Entry { name: name.to_owned(), dir, runnable: false }
    }

    #[test]
    fn open_folders_show_their_names_under_them() {
        let root = PathBuf::from("/p");
        let mut explorer = Explorer {
            disk: Disk::Local,
            root: root.clone(),
            open: HashSet::from([root.join("src")]),
            folders: HashMap::new(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
        };
        explorer.folders.insert(root.clone(), Folder::Read(vec![entry("docs", true), entry("src", true), entry("a.txt", false)]));
        explorer.folders.insert(root.join("src"), Folder::Read(vec![entry("main.rs", false)]));
        let lines: Vec<_> = explorer.lines().iter().map(|line| (line.name.to_string(), line.depth, line.open)).collect();
        assert_eq!(
            lines,
            [
                ("docs".to_owned(), 0, false),
                ("src".to_owned(), 0, true),
                ("main.rs".to_owned(), 1, false),
                ("a.txt".to_owned(), 0, false),
            ]
        );
    }

    #[test]
    fn folders_come_first_then_names_whatever_the_case() {
        let mut entries = vec![entry("b.txt", false), entry("src", true), entry("A.md", false), entry(".github", true)];
        sort(&mut entries);
        let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, [".github", "src", "A.md", "b.txt"]);
    }
}
