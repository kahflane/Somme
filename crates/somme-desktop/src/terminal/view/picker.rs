//! The lists that open from the chips above the command editor. The folder chip
//! lists the folder: pick a folder to go there, or a file to open it. The branch
//! chip lists git branches: pick one to switch to it, fetch, or make a new one.
//! It also stages, commits, pushes and renames the current branch.

use std::path::{Path, PathBuf};

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};

use super::{TerminalView, TerminalViewEvent};
use crate::settings::Settings;
use crate::terminal::blocks::{self, Branch, BranchState};
use crate::terminal::colors::{self, to_hsla};
use crate::terminal::complete;
use crate::terminal::disk::Disk;
use crate::ui::common::Lucide;

/// Rows shown at once.
const ROWS: usize = 10;
/// A folder with more than this many entries shows only the first ones.
const MAX_ENTRIES: usize = 5000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PickerKind {
    Folders,
    Branches,
}

pub(super) struct Picker {
    pub(super) kind: PickerKind,
    /// The folder listed, or the folder git runs in.
    dir: PathBuf,
    search: Entity<InputState>,
    /// Folders first, then files; or branches.
    entries: Vec<PickEntry>,
    /// The checked-out branch and its changes. Branches are in `entries`.
    git: BranchState,
    loaded: bool,
    selected: usize,
    /// Set while the box takes a commit message or a new branch name.
    asking: Option<Ask>,
    _subscription: Subscription,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    Commit,
    Rename,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PickEntry {
    Folder(String),
    File(String),
    Branch(Branch),
}

impl PickEntry {
    fn name(&self) -> &str {
        match self {
            Self::Folder(name) | Self::File(name) => name,
            Self::Branch(branch) => &branch.name,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Row<'a> {
    Parent,
    /// Asks for a message, then commits.
    Commit,
    Stage,
    Push,
    /// Asks for a name, then renames the current branch.
    Rename,
    Fetch,
    /// Commits with, or renames to, what was typed.
    Confirm(String),
    Entry(&'a PickEntry),
    /// Make a branch with the typed name.
    Create(String),
}

impl Picker {
    /// What the list shows for the typed search: an action row, then matching names.
    fn rows(&self, cx: &App) -> Vec<Row<'_>> {
        let typed = self.search.read(cx).value().trim().to_owned();
        if let Some(ask) = self.asking {
            let ok = match ask {
                Ask::Commit => !typed.is_empty() && complete::can_escape(&typed),
                Ask::Rename => valid_branch_name(&typed) && self.git.current.as_deref() != Some(typed.as_str()),
            };
            return if ok { vec![Row::Confirm(typed)] } else { Vec::new() };
        }
        let query = typed.to_lowercase();
        let first: Vec<Row> = match self.kind {
            PickerKind::Folders if query.is_empty() && self.dir.parent().is_some() => vec![Row::Parent],
            PickerKind::Branches if query.is_empty() => {
                let git = &self.git;
                let named = git.current.is_some();
                let push = named && (git.same_name_upstream().is_some() || git.push_remote().is_some());
                [
                    (git.staged + git.unstaged > 0, Row::Commit),
                    (git.unstaged > 0, Row::Stage),
                    (push, Row::Push),
                    (named, Row::Rename),
                    (true, Row::Fetch),
                ]
                .into_iter()
                .filter_map(|(show, row)| show.then_some(row))
                .collect()
            }
            _ => Vec::new(),
        };
        let create = self.kind == PickerKind::Branches
            && self.loaded
            && !typed.is_empty()
            && valid_branch_name(&typed)
            && !self.entries.iter().any(|entry| entry.name() == typed);
        first
            .into_iter()
            .chain(
                self.entries
                    .iter()
                    .filter(|entry| entry.name().to_lowercase().contains(&query))
                    .map(Row::Entry),
            )
            .chain(create.then_some(Row::Create(typed)))
            .collect()
    }
}

impl TerminalView {
    /// Opens the list for `kind`, or closes it when it is open already.
    pub(super) fn toggle_picker(&mut self, kind: PickerKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.as_ref().is_some_and(|picker| picker.kind == kind) {
            self.close_picker(window, cx);
            return;
        }
        let session = self.session.read(cx);
        let Some(dir) = session.blocks.as_ref().and_then(|b| b.cwd.clone()) else { return };
        let disk = session.disk();
        let placeholder = match kind {
            PickerKind::Folders => "Search directories…",
            PickerKind::Branches => "Search or create a branch…",
        };
        let (search, subscription) = self.picker_input(placeholder, None, window, cx);
        let read_dir = dir.clone();
        cx.spawn(async move |this, cx| {
            let Some(disk) = disk.await else { return };
            let (entries, git) = disk
                .run(move |disk| match kind {
                    PickerKind::Folders => (read_folder(&read_dir, disk), BranchState::default()),
                    PickerKind::Branches => {
                        let mut git = blocks::git_branch_state(&read_dir);
                        let branches = std::mem::take(&mut git.branches);
                        (branches.into_iter().map(PickEntry::Branch).collect(), git)
                    }
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if let Some(picker) = view.picker.as_mut().filter(|picker| picker.kind == kind) {
                    picker.entries = entries;
                    picker.git = git;
                    picker.loaded = true;
                    cx.notify();
                }
            });
        })
        .detach();
        self.picker = Some(Picker {
            kind,
            dir,
            search,
            entries: Vec::new(),
            git: BranchState::default(),
            loaded: false,
            selected: 0,
            asking: None,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// A focused text box for the picker. Enter acts on the selected row.
    fn picker_input(
        &mut self,
        placeholder: &'static str,
        value: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputState>, Subscription) {
        let input = cx.new(|cx| {
            let input = InputState::new(window, cx).placeholder(placeholder);
            match value {
                Some(value) => input.default_value(value),
                None => input,
            }
        });
        let subscription = cx.subscribe_in(&input, window, |view, _, event, window, cx| match event {
            InputEvent::Change => {
                if let Some(picker) = view.picker.as_mut() {
                    picker.selected = 0;
                }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => {
                let selected = view.picker.as_ref().map(|p| p.selected).unwrap_or(0);
                view.pick(selected, window, cx);
            }
            _ => {}
        });
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        (input, subscription)
    }

    /// Turns the search box into a box for a commit message or a branch name.
    fn ask(&mut self, ask: Ask, window: &mut Window, cx: &mut Context<Self>) {
        let (placeholder, value) = match ask {
            Ask::Commit => ("Commit message", None),
            Ask::Rename => ("New branch name", self.picker.as_ref().and_then(|p| p.git.current.clone())),
        };
        let (input, subscription) = self.picker_input(placeholder, value, window, cx);
        if let Some(picker) = self.picker.as_mut() {
            picker.search = input;
            picker._subscription = subscription;
            picker.asking = Some(ask);
            picker.selected = 0;
        }
        cx.notify();
    }

    /// Closes the picker and puts the keys back in the command editor.
    pub(super) fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.picker.take().is_none() {
            return false;
        }
        self.focus_prompt(window, cx);
        cx.notify();
        true
    }

    pub(super) fn move_picker(&mut self, up: bool, cx: &mut Context<Self>) -> bool {
        let Some(picker) = self.picker.as_ref() else { return false };
        let count = picker.rows(cx).len();
        let selected = picker.selected;
        let Some(picker) = self.picker.as_mut() else { return false };
        if count > 0 {
            picker.selected = if up { selected.saturating_sub(1) } else { (selected + 1).min(count - 1) };
        }
        cx.notify();
        true
    }

    /// Acts on the row at `index`: goes to a folder, opens a file, or switches branch.
    fn pick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.as_ref() else { return };
        let git = &picker.git;
        let action = match picker.rows(cx).get(index) {
            Some(Row::Parent) => Pick::Run("cd ..".to_owned()),
            Some(Row::Commit) => Pick::Ask(Ask::Commit),
            Some(Row::Rename) => Pick::Ask(Ask::Rename),
            Some(Row::Stage) => Pick::Run("git add -A".to_owned()),
            Some(Row::Push) => match (git.same_name_upstream(), &git.current, git.push_remote()) {
                (Some(_), _, _) => Pick::Run("git push".to_owned()),
                // Pushes a new branch and makes it follow the remote copy.
                (None, Some(branch), Some(remote)) => Pick::Run(format!(
                    "git push -u {} {}",
                    complete::escape(remote),
                    complete::escape(branch)
                )),
                _ => Pick::Nothing,
            },
            Some(Row::Confirm(text)) => match picker.asking {
                // With nothing staged, commits every change, as the row said.
                Some(Ask::Commit) if git.staged == 0 => {
                    Pick::Run(format!("git add -A && git commit -m {}", quote(text)))
                }
                Some(Ask::Commit) => Pick::Run(format!("git commit -m {}", quote(text))),
                Some(Ask::Rename) => Pick::Run(format!("git branch -m {}", complete::escape(text))),
                None => Pick::Nothing,
            },
            Some(Row::Fetch) => Pick::Run("git fetch --all --prune".to_owned()),
            Some(Row::Create(name)) => Pick::Run(format!("git switch -c {}", complete::escape(name))),
            Some(Row::Entry(PickEntry::Folder(name))) => Pick::Run(format!("cd {}", cd_target(name))),
            Some(Row::Entry(PickEntry::File(name))) => Pick::Open(picker.dir.join(name)),
            Some(Row::Entry(PickEntry::Branch(branch))) if branch.current => Pick::Nothing,
            Some(Row::Entry(PickEntry::Branch(branch))) if branch.remote => {
                // Makes a local branch that follows the remote one.
                Pick::Run(format!("git switch --track {}", complete::escape(&branch.name)))
            }
            Some(Row::Entry(PickEntry::Branch(branch))) => {
                Pick::Run(format!("git switch {}", complete::escape(&branch.name)))
            }
            None => return,
        };
        if let Pick::Ask(ask) = action {
            self.ask(ask, window, cx);
            return;
        }
        self.close_picker(window, cx);
        match action {
            Pick::Run(command) => self.run_from_ui(command, window, cx),
            Pick::Open(path) => cx.emit(TerminalViewEvent::OpenFile(path)),
            Pick::Ask(_) | Pick::Nothing => {}
        }
    }

    /// Runs a command picked with the mouse or a menu, as if it was typed.
    /// Only at the prompt, so it never goes to a program that is running.
    pub(super) fn run_from_ui(&mut self, command: String, window: &mut Window, cx: &mut Context<Self>) {
        // A control character, like a newline, would run a second command.
        if !self.session.read(cx).at_prompt() || !complete::can_escape(&command) {
            return;
        }
        self.session.update(cx, |session, cx| session.run_command(&command, cx));
        self.blocks_scroll = px(0.);
        self.awaiting_block = true;
        window.focus(&self.focus_handle, cx);
        cx.emit(TerminalViewEvent::Ran(command));
    }

    pub(super) fn render_picker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.picker.as_ref()?;
        let settings = Settings::get(cx);
        let theme = colors::active_theme(cx);
        let foreground = to_hsla(theme.foreground);
        let muted = foreground.opacity(0.55);
        let folder_color = to_hsla(theme.ansi[4]);
        let current_color = to_hsla(theme.ansi[2]);
        let git = &picker.git;
        let branch = git.current.as_deref().unwrap_or_default();
        let rows = picker.rows(cx);
        let start = picker.selected.saturating_sub(ROWS - 1).min(rows.len().saturating_sub(ROWS));
        let list = rows.iter().enumerate().skip(start).take(ROWS).map(|(i, row)| {
            let (icon, label, color) = match row {
                Row::Parent => (Lucide::CornerLeftUp, ".. (Parent Directory)".to_owned(), muted),
                Row::Commit if git.staged > 0 => {
                    (Lucide::GitCommitHorizontal, format!("Commit {}…", files(git.staged, "staged ")), foreground)
                }
                Row::Commit => {
                    (Lucide::GitCommitHorizontal, format!("Commit all changes ({})…", files(git.unstaged, "")), foreground)
                }
                Row::Stage => (Lucide::Plus, format!("Stage all changes ({})", files(git.unstaged, "")), foreground),
                Row::Push => match (git.same_name_upstream(), git.push_remote()) {
                    (Some(upstream), _) => (Lucide::CloudUpload, format!("Push “{branch}” to {upstream}"), foreground),
                    (None, remote) => (
                        Lucide::CloudUpload,
                        format!("Publish “{branch}” to {}", remote.unwrap_or_default()),
                        foreground,
                    ),
                },
                Row::Rename => (Lucide::Pencil, format!("Rename “{branch}”…"), foreground),
                Row::Confirm(text) if picker.asking == Some(Ask::Rename) => {
                    (Lucide::Pencil, format!("Rename “{branch}” to “{text}”"), foreground)
                }
                Row::Confirm(_) if git.staged > 0 => (
                    Lucide::GitCommitHorizontal,
                    format!("Commit {} with this message", files(git.staged, "staged ")),
                    foreground,
                ),
                Row::Confirm(_) => (
                    Lucide::GitCommitHorizontal,
                    format!("Stage and commit {} with this message", files(git.unstaged, "")),
                    foreground,
                ),
                Row::Fetch => (Lucide::CloudDownload, "Fetch branches from remotes".to_owned(), muted),
                Row::Create(name) => (Lucide::GitBranchPlus, format!("Create branch “{name}”"), foreground),
                Row::Entry(PickEntry::Folder(name)) => (Lucide::Folder, name.clone(), folder_color),
                Row::Entry(PickEntry::File(name)) => (Lucide::File, name.clone(), foreground),
                Row::Entry(PickEntry::Branch(branch)) if branch.current => {
                    (Lucide::Check, branch.name.clone(), current_color)
                }
                Row::Entry(PickEntry::Branch(branch)) if branch.remote => (Lucide::Cloud, branch.name.clone(), muted),
                Row::Entry(PickEntry::Branch(branch)) => (Lucide::GitBranch, branch.name.clone(), foreground),
            };
            h_flex()
                .id(("pick", i))
                .gap_2()
                .items_center()
                .px_1p5()
                .py_0p5()
                .rounded_sm()
                .cursor_pointer()
                .when(i == picker.selected, |row| row.bg(foreground.opacity(0.12)))
                .hover(|row| row.bg(foreground.opacity(0.07)))
                .child(Icon::new(icon).size(px(14.)).text_color(color))
                .child(div().flex_1().min_w_0().truncate().text_color(color).child(label))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, window, cx| {
                        cx.stop_propagation();
                        view.pick(i, window, cx);
                    }),
                )
        });
        let empty = match (picker.asking, picker.loaded, rows.is_empty(), picker.kind) {
            (Some(Ask::Commit), _, true, _) => Some("Type a message for the commit"),
            (Some(Ask::Rename), _, true, _) => Some("Type a new name, with no spaces"),
            (Some(_), ..) => None,
            (None, false, _, PickerKind::Folders) => Some("Reading folder…"),
            (None, false, _, PickerKind::Branches) => Some("Reading branches…"),
            (None, true, true, _) => Some("No matches"),
            _ => None,
        };
        let (search_icon, hint) = match (picker.asking, picker.kind) {
            (Some(Ask::Commit), _) => (Lucide::GitCommitHorizontal, "Enter to commit · Esc to close"),
            (Some(Ask::Rename), _) => (Lucide::Pencil, "Enter to rename · Esc to close"),
            (None, PickerKind::Folders) => (Lucide::Search, "↑ ↓ to move · Enter to open · Esc to close"),
            (None, PickerKind::Branches) => (Lucide::Search, "↑ ↓ to move · Enter to pick · Esc to close"),
        };
        Some(
            v_flex()
                .gap_1()
                .p_1()
                .rounded_md()
                .border_1()
                .border_color(foreground.opacity(0.12))
                .bg(foreground.opacity(0.04))
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .pl_1p5()
                        .border_b_1()
                        .border_color(foreground.opacity(0.08))
                        .child(Icon::new(search_icon).size(px(14.)).text_color(muted))
                        .child(div().flex_1().child(Input::new(&picker.search).small().appearance(false))),
                )
                .child(
                    v_flex()
                        .gap_0p5()
                        .font_family(settings.font_family.clone())
                        .text_size(px(settings.font_size.clamp(6., 72.) * 0.9))
                        .children(list)
                        .when_some(empty, |list, text| {
                            list.child(div().px_1p5().py_0p5().text_color(muted).child(text))
                        }),
                )
                .child(div().px_1p5().text_xs().text_color(muted).child(hint))
                .into_any_element(),
        )
    }
}

enum Pick {
    Run(String),
    Open(PathBuf),
    Ask(Ask),
    Nothing,
}

/// "3 staged files", "1 file".
fn files(count: usize, kind: &str) -> String {
    format!("{count} {kind}file{}", if count == 1 { "" } else { "s" })
}

/// Wraps text in single quotes for the shell, so nothing in it runs.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Rejects names git would refuse, and ones that would read as an option.
fn valid_branch_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.contains(|ch: char| ch.is_whitespace() || ch.is_control() || "~^:?*[\\".contains(ch))
        && !name.contains("..")
        && !name.contains("@{")
        && !name.ends_with(['.', '/'])
        && !name.ends_with(".lock")
}

/// A folder name ready for `cd`: escaped, and kept from reading as an option or a home folder.
pub(super) fn cd_target(name: &str) -> String {
    let escaped = complete::escape(name);
    if name.starts_with(['-', '~']) { format!("./{escaped}") } else { escaped }
}

/// Folders first, then files. Hidden ones come after the rest in each group.
fn read_folder(dir: &Path, disk: &Disk) -> Vec<PickEntry> {
    let Some(read) = disk.list(dir, MAX_ENTRIES, complete::can_escape) else { return Vec::new() };
    let mut entries: Vec<(bool, String)> = read.into_iter().map(|entry| (entry.dir, entry.name)).collect();
    entries.sort_by_cached_key(|(dir, name)| (!dir, name.starts_with('.'), name.to_lowercase()));
    entries
        .into_iter()
        .map(|(dir, name)| if dir { PickEntry::Folder(name) } else { PickEntry::File(name) })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cd_target_keeps_odd_names_safe() {
        assert_eq!(cd_target("src"), "src");
        assert_eq!(cd_target("My Files"), "My\\ Files");
        assert_eq!(cd_target("-rf"), "./-rf");
        assert_eq!(cd_target("~backup"), "./~backup");
    }

    #[test]
    fn quote_keeps_the_message_as_one_word() {
        assert_eq!(quote("fix it"), "'fix it'");
        assert_eq!(quote("don't $(rm) `x`"), "'don'\\''t $(rm) `x`'");
    }

    #[test]
    fn checks_branch_names() {
        assert!(valid_branch_name("feature/login"));
        assert!(!valid_branch_name("has space"));
        assert!(!valid_branch_name("-f"));
        assert!(!valid_branch_name("a..b"));
        assert!(!valid_branch_name("main.lock"));
    }

    #[test]
    fn folders_come_first_and_hidden_last() {
        let dir = std::env::temp_dir().join(format!("somme-picker-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("b-dir")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden-dir")).unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();
        std::fs::write(dir.join(".env"), "").unwrap();
        std::fs::create_dir_all(dir.join("x\nevil")).unwrap();
        let names: Vec<_> = read_folder(&dir, &Disk::Local).iter().map(|e| e.name().to_owned()).collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(names, ["b-dir", ".hidden-dir", "a.txt", ".env"]);
    }
}
