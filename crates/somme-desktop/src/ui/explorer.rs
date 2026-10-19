//! The files beside the terminals, as an editor's file tree shows them. It
//! shows the shell's folder, on this computer or on the server an SSH shell
//! runs on, follows the shell when it changes folder, and opens a clicked file
//! in the editor. It keeps itself up to date: on this computer it watches the
//! open folders; on a server it reads them again every few seconds and after
//! each command. A right-click menu renames, deletes, makes files and more.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FocusHandle, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task,
    UniformListScrollHandle, WeakEntity, Window, div, prelude::FluentBuilder as _, px, uniform_list,
};
use notify::Watcher as _;

use crate::settings::Settings;
use crate::terminal::disk::{Disk, Entry};
use crate::ui::attachments;
use crate::ui::common::{Lucide, copy_text, notify_error, notify_ok};
use crate::ui::native;

const ROW: f32 = 24.;
/// How far each level of folders is pushed in.
const INDENT: f32 = 12.;
/// A folder with more names than this shows only the first ones.
const MAX_ENTRIES: usize = 5_000;
/// Names hidden from the tree, as most editors hide them.
const HIDDEN: &[&str] = &[".git", ".hg", ".svn", ".DS_Store", "Thumbs.db"];
/// Changes on this computer come in bursts; the tree waits for one to end.
const SETTLE: Duration = Duration::from_millis(200);
/// How often a server's open folders are read again.
const POLL: Duration = Duration::from_secs(3);

pub enum ExplorerEvent {
    /// Open the file in the editor beside the terminals.
    Open(PathBuf),
    /// Open the file in a tab of its own.
    OpenInTab(PathBuf),
    /// Go to this folder in the current terminal.
    Cd(PathBuf),
    /// Open a new terminal tab in this folder.
    NewTerminal(PathBuf),
    Close,
}

enum Folder {
    Reading,
    Read(Vec<Entry>),
    Failed,
}

/// A name being typed in the tree.
enum Editing {
    Rename { path: PathBuf, input: Entity<InputState> },
    New { dir: PathBuf, folder: bool, input: Entity<InputState> },
}

impl Editing {
    fn input(&self) -> &Entity<InputState> {
        match self {
            Editing::Rename { input, .. } | Editing::New { input, .. } => input,
        }
    }
}

/// One line of the tree as shown.
#[derive(Clone)]
struct Line {
    path: PathBuf,
    name: SharedString,
    depth: usize,
    dir: bool,
    open: bool,
    /// The box for a new file's name, not a file.
    new_name: bool,
}

pub struct Explorer {
    disk: Disk,
    root: PathBuf,
    /// Folders shown open.
    open: HashSet<PathBuf>,
    folders: HashMap<PathBuf, Folder>,
    selected: Option<PathBuf>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    editing: Option<Editing>,
    /// Watches the shown folders on this computer.
    watcher: Option<notify::RecommendedWatcher>,
    watched: HashSet<PathBuf>,
    /// Folders being read again now, so polls do not pile up.
    reloading: HashSet<PathBuf>,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<ExplorerEvent> for Explorer {}

impl Explorer {
    pub fn new(disk: Disk, root: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut tasks = Vec::new();
        let mut watcher = None;
        if disk.is_local() {
            let (changes, mut changed) = mpsc::unbounded::<PathBuf>();
            watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event {
                    for path in event.paths {
                        let _ = changes.unbounded_send(path);
                    }
                }
            })
            .map_err(|err| tracing::warn!(%err, "the file tree can't watch for changes"))
            .ok();
            tasks.push(cx.spawn(async move |explorer, cx| {
                while let Some(first) = changed.next().await {
                    cx.background_executor().timer(SETTLE).await;
                    let mut paths = HashSet::from([first]);
                    while let Ok(path) = changed.try_recv() {
                        paths.insert(path);
                    }
                    if explorer.update(cx, |explorer, cx| explorer.changed(paths, cx)).is_err() {
                        break;
                    }
                }
            }));
        } else {
            tasks.push(cx.spawn(async move |explorer, cx| {
                loop {
                    cx.background_executor().timer(POLL).await;
                    if explorer.update(cx, |explorer, cx| explorer.reload_shown(cx)).is_err() {
                        break;
                    }
                }
            }));
        }
        let mut explorer = Self {
            disk,
            root: root.clone(),
            open: HashSet::new(),
            folders: HashMap::new(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            editing: None,
            watcher,
            watched: HashSet::new(),
            reloading: HashSet::new(),
            _tasks: tasks,
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
        self.watch_shown();
        cx.notify();
    }

    /// Where the files are: this computer or a server.
    pub fn disk(&self) -> &Disk {
        &self.disk
    }

    /// A command ended in the terminal. On a server that is the time to look
    /// again; on this computer the watcher has seen it already.
    pub fn command_ended(&mut self, cx: &mut Context<Self>) {
        if !self.disk.is_local() {
            self.reload_shown(cx);
        }
    }

    fn read(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        self.folders.insert(dir.clone(), Folder::Reading);
        self.read_again(dir, cx);
    }

    /// Reads a folder, keeping what it showed until the new list comes.
    fn read_again(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if !self.reloading.insert(dir.clone()) {
            return;
        }
        let disk = self.disk.clone();
        cx.spawn(async move |explorer, cx| {
            let read = disk
                .run({
                    let dir = dir.clone();
                    move |disk| disk.list(&dir, MAX_ENTRIES, |name| !HIDDEN.contains(&name))
                })
                .await;
            let _ = explorer.update(cx, |explorer, cx| {
                explorer.reloading.remove(&dir);
                let folder = match read {
                    Some(mut entries) => {
                        sort(&mut entries);
                        // Folders gone from it are no longer open.
                        explorer.open.retain(|open| {
                            open.parent() != Some(dir.as_path())
                                || entries.iter().any(|entry| entry.dir && dir.join(&entry.name) == *open)
                        });
                        Folder::Read(entries)
                    }
                    // A server that did not answer this once keeps what it showed.
                    None if matches!(explorer.folders.get(&dir), Some(Folder::Read(_))) => return,
                    None => Folder::Failed,
                };
                let same = matches!((explorer.folders.get(&dir), &folder), (Some(Folder::Read(a)), Folder::Read(b)) if a == b);
                explorer.folders.insert(dir, folder);
                explorer.watch_shown();
                if !same {
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// The folders shown: the top one and every open one under it.
    fn shown(&self) -> Vec<PathBuf> {
        std::iter::once(self.root.clone())
            .chain(self.open.iter().filter(|dir| dir.starts_with(&self.root) && **dir != self.root).cloned())
            .collect()
    }

    fn reload_shown(&mut self, cx: &mut Context<Self>) {
        for dir in self.shown() {
            self.read_again(dir, cx);
        }
    }

    /// Watches exactly the shown folders.
    fn watch_shown(&mut self) {
        if self.watcher.is_none() {
            return;
        }
        let wanted: HashSet<PathBuf> =
            self.shown().into_iter().filter(|dir| matches!(self.folders.get(dir), Some(Folder::Read(_)))).collect();
        let Some(watcher) = self.watcher.as_mut() else { return };
        for gone in self.watched.difference(&wanted) {
            let _ = watcher.unwatch(gone);
        }
        for new in wanted.difference(&self.watched) {
            if let Err(err) = watcher.watch(new, notify::RecursiveMode::NonRecursive) {
                tracing::debug!(%err, dir = %new.display(), "could not watch a folder");
            }
        }
        self.watched = wanted;
    }

    /// Reads again the shown folders that changed.
    fn changed(&mut self, paths: HashSet<PathBuf>, cx: &mut Context<Self>) {
        let shown: HashSet<PathBuf> = self.shown().into_iter().collect();
        let mut dirs = HashSet::new();
        for path in paths {
            if let Some(parent) = path.parent().filter(|parent| shown.contains(*parent)) {
                dirs.insert(parent.to_path_buf());
            }
            if shown.contains(&path) {
                dirs.insert(path);
            }
        }
        for dir in dirs {
            self.read_again(dir, cx);
        }
    }

    fn toggle(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if !self.open.remove(&dir) {
            if !self.folders.contains_key(&dir) {
                self.read(dir.clone(), cx);
            }
            self.open.insert(dir);
        }
        self.watch_shown();
        cx.notify();
    }

    /// Reads every shown folder again.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.folders.clear();
        for dir in self.shown() {
            self.read(dir, cx);
        }
    }

    fn collapse(&mut self, cx: &mut Context<Self>) {
        self.open.clear();
        self.watch_shown();
        cx.notify();
    }

    fn lines(&self) -> Vec<Line> {
        let new = match &self.editing {
            Some(Editing::New { dir, folder, .. }) => Some((dir.as_path(), *folder)),
            _ => None,
        };
        tree_lines(&self.root, &self.open, &self.folders, new)
    }

    fn is_dir(&self, path: &Path) -> bool {
        if self.folders.contains_key(path) {
            return true;
        }
        let Some(Folder::Read(entries)) = path.parent().and_then(|parent| self.folders.get(parent)) else { return false };
        let name = path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default();
        entries.iter().any(|entry| entry.dir && entry.name == name)
    }

    // ---- what the menu does ----

    fn name_box(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(text.to_owned()));
        cx.subscribe_in(&input, window, |explorer, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => explorer.finish_editing(window, cx),
            InputEvent::Blur => explorer.stop_editing(cx),
            _ => {}
        })
        .detach();
        input.update(cx, |input, cx| input.focus(window, cx));
        input
    }

    fn start_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let input = self.name_box(&name, window, cx);
        self.selected = Some(path.clone());
        self.editing = Some(Editing::Rename { path, input });
        cx.notify();
    }

    fn start_new(&mut self, dir: PathBuf, folder: bool, window: &mut Window, cx: &mut Context<Self>) {
        if dir != self.root && !self.open.contains(&dir) {
            self.toggle(dir.clone(), cx);
        }
        let input = self.name_box("", window, cx);
        self.editing = Some(Editing::New { dir, folder, input });
        cx.notify();
    }

    fn stop_editing(&mut self, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            cx.notify();
        }
    }

    fn finish_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = self.editing.take() else { return };
        let name = editing.input().read(cx).value().trim().to_owned();
        window.focus(&self.focus, cx);
        cx.notify();
        if name.is_empty() {
            return;
        }
        if name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
            notify_error(window, cx, "Can't use that name", "A name can't hold / or \\, or be . or ..");
            return;
        }
        let disk = self.disk.clone();
        let (dir, target, done) = match &editing {
            Editing::Rename { path, .. } => {
                let Some(parent) = path.parent() else { return };
                if path.file_name().is_some_and(|old| old.to_string_lossy() == name) {
                    return;
                }
                (parent.to_path_buf(), parent.join(&name), Done::Renamed(path.clone()))
            }
            Editing::New { dir, folder, .. } => (dir.clone(), dir.join(&name), Done::Made(*folder)),
        };
        cx.spawn_in(window, async move |explorer, cx| {
            let result = disk
                .run({
                    let (target, done) = (target.clone(), done.clone());
                    move |disk| match done {
                        Done::Renamed(from) => disk.rename(&from, &target),
                        Done::Made(folder) => disk.create(&target, folder),
                    }
                })
                .await;
            let _ = explorer.update_in(cx, |explorer, window, cx| match result {
                Ok(()) => {
                    if let Done::Renamed(from) = &done {
                        // Open folders inside it stay open under the new name.
                        let moved: Vec<PathBuf> = explorer.open.iter().filter(|dir| dir.starts_with(from)).cloned().collect();
                        for old in moved {
                            explorer.open.remove(&old);
                            if let Ok(rest) = old.strip_prefix(from) {
                                explorer.open.insert(target.join(rest));
                            }
                        }
                    }
                    explorer.selected = Some(target.clone());
                    explorer.read_again(dir, cx);
                    if matches!(done, Done::Made(false)) {
                        cx.emit(ExplorerEvent::Open(target));
                    }
                }
                Err(err) => {
                    let title = if matches!(done, Done::Renamed(_)) { "Can't rename it" } else { "Can't make it" };
                    notify_error(window, cx, title, err);
                }
            });
        })
        .detach();
    }

    fn delete(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned());
        let dir = self.is_dir(&path);
        let (title, detail, ok) = if self.disk.is_local() {
            (format!("Move \u{201c}{name}\u{201d} to the Trash?"), "You can bring it back from the Trash.".to_owned(), "Move to Trash")
        } else {
            let what = if dir { "this folder and everything in it" } else { "this file" };
            (format!("Delete \u{201c}{name}\u{201d}?"), format!("This deletes {what} from the server. It can't be undone."), "Delete")
        };
        let explorer = cx.entity().downgrade();
        native::confirm(&title, &detail, ok, move |window, cx| {
            let _ = explorer.update(cx, |explorer, cx| explorer.really_delete(path, dir, window, cx));
        }, window, cx);
    }

    fn really_delete(&mut self, path: PathBuf, dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        let disk = self.disk.clone();
        cx.spawn_in(window, async move |explorer, cx| {
            let result = disk.run({
                let path = path.clone();
                move |disk| disk.remove(&path, dir)
            })
            .await;
            let _ = explorer.update_in(cx, |explorer, window, cx| match result {
                Ok(()) => {
                    explorer.open.retain(|open| !open.starts_with(&path));
                    if explorer.selected.as_ref().is_some_and(|selected| selected.starts_with(&path)) {
                        explorer.selected = None;
                    }
                    if let Some(parent) = path.parent() {
                        explorer.read_again(parent.to_path_buf(), cx);
                    }
                }
                Err(err) => notify_error(window, cx, "Can't delete it", err),
            });
        })
        .detach();
    }

    /// Shows it in Finder, or on a server, saves a copy in Downloads.
    fn reveal(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.disk.is_local() {
            cx.reveal_path(&path);
            return;
        }
        let Some(downloads) = dirs::download_dir().or_else(dirs::home_dir) else { return };
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "download".into());
        let local = free_path(&downloads, &name);
        let disk = self.disk.clone();
        cx.spawn_in(window, async move |_, cx| {
            let result = disk.run({
                let local = local.clone();
                move |disk| disk.download(&path, &local)
            })
            .await;
            let _ = cx.update(|window, cx| match result {
                Ok(()) => {
                    notify_ok(cx, format!("Saved to {}", local.display()));
                    cx.reveal_path(&local);
                }
                Err(err) => notify_error(window, cx, "Can't download it", err),
            });
        })
        .detach();
    }

    fn menu(explorer: WeakEntity<Self>, line: Line) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        move |menu, _, cx| {
            let Some(this) = explorer.upgrade() else { return menu };
            let (local, root) = {
                let explorer = this.read(cx);
                (explorer.disk.is_local(), explorer.root.clone())
            };
            let ai_on = Settings::get(cx).ai.on;
            let path = line.path.clone();
            let relative = path.strip_prefix(&root).map_or_else(|_| path.display().to_string(), |rest| rest.display().to_string());
            let act = |label: &str, icon: Lucide, run: MenuAction| {
                let explorer = explorer.clone();
                PopupMenuItem::new(label.to_owned()).icon(Icon::new(icon)).on_click(move |_, window, cx| {
                    let _ = explorer.update(cx, |explorer, cx| run(explorer, window, cx));
                })
            };
            let p = path.clone();
            let menu = if line.dir {
                let (a, b, c, d) = (p.clone(), p.clone(), p.clone(), p.clone());
                menu.item(act("New File", Lucide::FilePlus, Box::new(move |e, w, cx| e.start_new(a.clone(), false, w, cx))))
                    .item(act("New Folder", Lucide::FolderPlus, Box::new(move |e, w, cx| e.start_new(b.clone(), true, w, cx))))
                    .separator()
                    .item(act("cd to Folder", Lucide::SquareTerminal, Box::new(move |_, _, cx| cx.emit(ExplorerEvent::Cd(c.clone())))))
                    .item(act("Open in New Tab", Lucide::Plus, Box::new(move |_, _, cx| cx.emit(ExplorerEvent::NewTerminal(d.clone())))))
            } else {
                let (a, b) = (p.clone(), p.clone());
                menu.item(act("Open in Pane", Lucide::PanelRight, Box::new(move |_, _, cx| cx.emit(ExplorerEvent::Open(a.clone())))))
                    .item(act("Open in New Tab", Lucide::Plus, Box::new(move |_, _, cx| cx.emit(ExplorerEvent::OpenInTab(b.clone())))))
            };
            let (reveal, rename, delete, attach) = (p.clone(), p.clone(), p.clone(), p.clone());
            let reveal_label = if local {
                if cfg!(target_os = "macos") { "Reveal in Finder" } else if cfg!(windows) { "Show in Explorer" } else { "Show in Files" }
            } else {
                "Download"
            };
            let reveal_icon = if local { Lucide::FolderSearch } else { Lucide::Download };
            // The shell's own folder is not renamed or deleted from here.
            let top = path == root;
            menu.item(act(reveal_label, reveal_icon, Box::new(move |e, w, cx| e.reveal(reveal.clone(), w, cx))))
                .when(!top, |menu| {
                    menu.separator()
                        .item(act("Rename", Lucide::Pencil, Box::new(move |e, w, cx| e.start_rename(rename.clone(), w, cx))))
                        .item(act("Delete", Lucide::Trash, Box::new(move |e, w, cx| e.delete(delete.clone(), w, cx))))
                })
                .when(ai_on, |menu| {
                    menu.separator().item(act(
                        "Attach as Context",
                        Lucide::Sparkles,
                        Box::new(move |e, _, cx| attachments::attach(attach.clone(), e.disk.clone(), cx)),
                    ))
                })
                .separator()
                .item(PopupMenuItem::new("Copy Path").icon(Icon::new(Lucide::Copy)).on_click({
                    let path = path.display().to_string();
                    move |_, _, cx| copy_text(path.clone(), cx)
                }))
                .item(PopupMenuItem::new("Copy Relative Path").icon(Icon::new(Lucide::Copy)).on_click(move |_, _, cx| copy_text(relative.clone(), cx)))
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            if event.keystroke.key == "escape" {
                self.stop_editing(cx);
                window.focus(&self.focus, cx);
                cx.stop_propagation();
            }
            return;
        }
        let Some(path) = self.selected.clone() else { return };
        let keys = &event.keystroke;
        let command = keys.modifiers.platform || (cfg!(not(target_os = "macos")) && keys.modifiers.control);
        match keys.key.as_str() {
            "f2" => self.start_rename(path, window, cx),
            "delete" => self.delete(path, window, cx),
            "backspace" if keys.modifiers.platform => self.delete(path, window, cx),
            "c" if command => {
                copy_text(path.display().to_string(), cx);
                notify_ok(cx, "Copied the path");
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    // ---- drawing ----

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
        let name_box = match &self.editing {
            Some(Editing::New { input, .. }) if line.new_name => Some(input.clone()),
            Some(Editing::Rename { path, input, .. }) if *path == line.path => Some(input.clone()),
            _ => None,
        };
        let row = h_flex()
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
                    .when(dir && !line.new_name, |slot| slot.child(Icon::new(chevron).size(px(14.)))),
            )
            .child(
                Icon::new(icon)
                    .size(px(14.))
                    .text_color(if dir { theme.primary } else { theme.muted_foreground }),
            );
        let row = match name_box {
            Some(input) => row.child(div().flex_1().min_w_0().child(Input::new(&input).xsmall())),
            None => row.child(div().flex_1().min_w_0().truncate().child(line.name.clone())),
        };
        if line.new_name {
            return row.into_any_element();
        }
        row.on_click(cx.listener(move |explorer, _, window, cx| {
            explorer.selected = Some(path.clone());
            window.focus(&explorer.focus, cx);
            if dir {
                explorer.toggle(path.clone(), cx);
            } else {
                cx.emit(ExplorerEvent::Open(path.clone()));
                cx.notify();
            }
        }))
        .context_menu(Self::menu(cx.entity().downgrade(), line.clone()))
        .into_any_element()
    }
}

/// The tree as shown: each open folder's names under it. `new` is the folder
/// a name is being typed in, and whether it is for a folder; its box comes
/// first in that folder.
fn tree_lines(root: &Path, open: &HashSet<PathBuf>, folders: &HashMap<PathBuf, Folder>, new: Option<(&Path, bool)>) -> Vec<Line> {
    fn add(dir: &Path, depth: usize, tree: &Tree, lines: &mut Vec<Line>) {
        if let Some((new_in, folder)) = tree.new
            && new_in == dir
        {
            let path = dir.join("\u{0}new");
            lines.push(Line { path, name: SharedString::default(), depth, dir: folder, open: false, new_name: true });
        }
        let Some(Folder::Read(entries)) = tree.folders.get(dir) else { return };
        for entry in entries {
            let path = dir.join(&entry.name);
            let open = entry.dir && tree.open.contains(&path);
            lines.push(Line { path: path.clone(), name: entry.name.clone().into(), depth, dir: entry.dir, open, new_name: false });
            if open {
                add(&path, depth + 1, tree, lines);
            }
        }
    }
    struct Tree<'a> {
        open: &'a HashSet<PathBuf>,
        folders: &'a HashMap<PathBuf, Folder>,
        new: Option<(&'a Path, bool)>,
    }
    let mut lines = Vec::new();
    add(root, 0, &Tree { open, folders, new }, &mut lines);
    lines
}

/// What a menu item does to the tree.
type MenuAction = Box<dyn Fn(&mut Explorer, &mut Window, &mut Context<Explorer>)>;

#[derive(Clone)]
enum Done {
    Renamed(PathBuf),
    /// True for a folder.
    Made(bool),
}

/// `dir/name`, or `dir/name (2)` and on when that is taken.
fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_owned(), format!(".{ext}")),
        _ => (name.to_owned(), String::new()),
    };
    (2..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|path| !path.exists()).unwrap_or(first)
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
            Some(Folder::Read(entries)) if entries.is_empty() && self.editing.is_none() => note("This folder is empty."),
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
        let root = self.root.clone();
        v_flex()
            .size_full()
            .bg(theme.background)
            .border_r_1()
            .border_color(theme.border)
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|explorer, event: &KeyDownEvent, window, cx| {
                if explorer.editing.is_some() && event.keystroke.key == "escape" {
                    explorer.on_key(event, window, cx);
                }
            }))
            .on_key_down(cx.listener(Self::on_key))
            .child(self.render_header(cx))
            .child(
                v_flex()
                    .id("explorer-body")
                    .flex_1()
                    .min_h_0()
                    .child(body)
                    .context_menu(Self::menu(
                        cx.entity().downgrade(),
                        Line { path: root, name: SharedString::default(), depth: 0, dir: true, open: true, new_name: false },
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, dir: bool) -> Entry {
        Entry { name: name.to_owned(), dir, runnable: false }
    }

    fn tree() -> (PathBuf, HashSet<PathBuf>, HashMap<PathBuf, Folder>) {
        let root = PathBuf::from("/p");
        let open = HashSet::from([root.join("src")]);
        let mut folders = HashMap::new();
        folders.insert(root.clone(), Folder::Read(vec![entry("docs", true), entry("src", true), entry("a.txt", false)]));
        folders.insert(root.join("src"), Folder::Read(vec![entry("main.rs", false)]));
        (root, open, folders)
    }

    #[test]
    fn open_folders_show_their_names_under_them() {
        let (root, open, folders) = tree();
        let lines: Vec<_> =
            tree_lines(&root, &open, &folders, None).iter().map(|line| (line.name.to_string(), line.depth, line.open)).collect();
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
    fn a_new_name_is_typed_at_the_top_of_its_folder() {
        let (root, open, folders) = tree();
        let src = root.join("src");
        let lines = tree_lines(&root, &open, &folders, Some((&src, false)));
        let names: Vec<_> = lines.iter().map(|line| (line.new_name, line.depth)).collect();
        assert_eq!(names, [(false, 0), (false, 0), (true, 1), (false, 1), (false, 0)]);
    }

    #[test]
    fn folders_come_first_then_names_whatever_the_case() {
        let mut entries = vec![entry("b.txt", false), entry("src", true), entry("A.md", false), entry(".github", true)];
        sort(&mut entries);
        let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, [".github", "src", "A.md", "b.txt"]);
    }

    #[test]
    fn downloads_never_replace_a_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(free_path(dir.path(), "a.txt"), dir.path().join("a.txt"));
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        assert_eq!(free_path(dir.path(), "a.txt"), dir.path().join("a (2).txt"));
    }
}
