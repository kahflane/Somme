//! Shows a repo's changes: the files on the left, every diff in one long list on the right.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::highlighter::{LanguageRegistry, SyntaxHighlighter};
use gpui_kit::component::input::Rope;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, FocusHandle, Focusable, FontWeight, HighlightStyle, Hsla,
    InteractiveElement as _, StyledText,
    IntoElement, ListHorizontalSizingBehavior, ParentElement as _, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement as _, Styled as _, UniformListScrollHandle, Window, div, prelude::FluentBuilder as _,
    px, uniform_list,
};

use super::diff::{self, Changes, FileDiff, FileStatus, LineKind};
use crate::editor::language;
use crate::settings::Settings;
use crate::ui::common::Lucide;

/// Files with more changed lines than this start folded.
const FOLD_LINES: usize = 1000;
/// Files with more diff lines than this show without code colours.
const MAX_COLOR_LINES: usize = 20_000;

enum State {
    Loading,
    Failed(String),
    Ready(Changes),
}

/// One line of the diff list.
#[derive(Clone, Copy, Debug)]
enum Row {
    /// Space between two files.
    Gap,
    File(usize),
    Hunk(usize, usize),
    Line(usize, usize, usize),
    /// Says why a file shows no lines.
    Note(usize),
}

pub struct ChangesView {
    /// The folder it was opened from.
    dir: PathBuf,
    /// The commit whose changes show, instead of the uncommitted ones.
    commit: Option<String>,
    state: State,
    /// Reading again while the old changes still show.
    refreshing: bool,
    rows: Vec<Row>,
    /// Files the user folded or unfolded, by path. Others use [`folds_at_first`].
    folds: HashMap<String, bool>,
    selected: Option<usize>,
    /// The row with the longest text, which sets how wide the list scrolls.
    widest: usize,
    /// Code colours by file, made the first time a file's lines show. None when
    /// the language has no colours.
    colors: HashMap<usize, Option<FileColors>>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
}

impl ChangesView {
    pub fn new(dir: PathBuf, cx: &mut Context<Self>) -> Self {
        Self::of(dir, None, cx)
    }

    /// Shows what commit `sha` changed.
    pub fn commit(dir: PathBuf, sha: String, cx: &mut Context<Self>) -> Self {
        Self::of(dir, Some(sha), cx)
    }

    fn of(dir: PathBuf, commit: Option<String>, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            dir,
            commit,
            state: State::Loading,
            refreshing: false,
            rows: Vec::new(),
            folds: HashMap::new(),
            selected: None,
            widest: 0,
            colors: HashMap::new(),
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
        };
        view.reload(cx);
        view
    }

    /// The commit this tab shows, if it shows one.
    pub fn shown_commit(&self) -> Option<&str> {
        self.commit.as_deref()
    }

    /// True when `dir` is in the repo this tab shows.
    pub fn shows(&self, dir: &Path) -> bool {
        match &self.state {
            State::Ready(changes) => dir.starts_with(&changes.root),
            _ => dir == self.dir,
        }
    }

    pub fn title(&self) -> SharedString {
        let root = match &self.state {
            State::Ready(changes) => &changes.root,
            _ => &self.dir,
        };
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match &self.commit {
            Some(sha) => format!("{} · {name}", &sha[..7.min(sha.len())]).into(),
            None => format!("Changes · {name}").into(),
        }
    }

    /// Reads the changes again.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        let dir = self.dir.clone();
        let commit = self.commit.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    match commit {
                        Some(sha) => diff::load_commit(&dir, &sha),
                        None => diff::load(&dir),
                    }
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.refreshing = false;
                view.colors.clear();
                view.state = match result {
                    Ok(changes) => State::Ready(changes),
                    Err(reason) => State::Failed(reason),
                };
                view.rebuild_rows();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn files(&self) -> &[FileDiff] {
        match &self.state {
            State::Ready(changes) => &changes.files,
            _ => &[],
        }
    }

    fn is_folded(&self, file: &FileDiff) -> bool {
        self.folds.get(&file.path).copied().unwrap_or_else(|| folds_at_first(file))
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        let (mut widest, mut widest_len) = (0, 0);
        for (f, file) in self.files().iter().enumerate() {
            if f > 0 {
                rows.push(Row::Gap);
            }
            rows.push(Row::File(f));
            if self.is_folded(file) {
                continue;
            }
            if file.binary || file.hunks.is_empty() {
                rows.push(Row::Note(f));
                continue;
            }
            for (h, hunk) in file.hunks.iter().enumerate() {
                rows.push(Row::Hunk(f, h));
                for (l, line) in hunk.lines.iter().enumerate() {
                    let len = display_len(&line.text);
                    if len > widest_len {
                        (widest, widest_len) = (rows.len(), len);
                    }
                    rows.push(Row::Line(f, h, l));
                }
            }
        }
        self.rows = rows;
        self.widest = widest;
    }

    fn toggle_fold(&mut self, f: usize, cx: &mut Context<Self>) {
        let Some(file) = self.files().get(f) else { return };
        let folded = !self.is_folded(file);
        let path = file.path.clone();
        self.folds.insert(path, folded);
        self.rebuild_rows();
        cx.notify();
    }

    /// Unfolds a file and scrolls its diff to the top.
    fn show_file(&mut self, f: usize, cx: &mut Context<Self>) {
        let Some(file) = self.files().get(f) else { return };
        if self.is_folded(file) {
            let path = file.path.clone();
            self.folds.insert(path, false);
            self.rebuild_rows();
        }
        self.selected = Some(f);
        if let Some(row) = self.rows.iter().position(|row| matches!(row, Row::File(i) if *i == f)) {
            self.scroll.scroll_to_item_strict(row, ScrollStrategy::Top);
        }
        cx.notify();
    }

    /// Parses the files that have lines in `range`, if not done yet.
    fn color_rows(&mut self, range: Range<usize>) {
        let State::Ready(changes) = &self.state else { return };
        for row in self.rows.get(range).unwrap_or_default() {
            if let Row::Line(f, ..) = *row {
                self.colors.entry(f).or_insert_with(|| FileColors::new(&changes.files[f]));
            }
        }
    }

    /// Colours for one line of shown text, `len` bytes long.
    fn line_colors(&self, f: usize, h: usize, l: usize, len: usize, cx: &App) -> Vec<(Range<usize>, HighlightStyle)> {
        let Some(Some(colors)) = self.colors.get(&f) else { return Vec::new() };
        let Some(Some((side, start))) = colors.lines.get(h).and_then(|lines| lines.get(l)).copied() else {
            return Vec::new();
        };
        let highlighter = match side {
            Side::Old => &colors.old,
            Side::New => &colors.new,
        };
        let theme = cx.theme().highlight_theme.clone();
        highlighter
            .styles(&(start..start + len), &*theme)
            .into_iter()
            .filter_map(|(range, style)| {
                let range = range.start.saturating_sub(start)..range.end.saturating_sub(start).min(len);
                (range.start < range.end).then_some((range, style))
            })
            .collect()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let files = self.files();
        let (added, removed) = files.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
        let branch = match &self.state {
            State::Ready(changes) if !changes.branch.is_empty() => Some(changes.branch.clone()),
            _ => None,
        };
        h_flex()
            .h(px(40.))
            .flex_none()
            .px_3()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(Icon::new(Lucide::FileDiff).size(px(16.)))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(self.title()))
            .when_some(branch, |bar, branch| {
                bar.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .px_1p5()
                        .rounded_md()
                        .bg(theme.foreground.opacity(0.07))
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(Lucide::GitBranch).size(px(12.)))
                        .child(branch),
                )
            })
            .when(!files.is_empty(), |bar| {
                bar.child(
                    h_flex()
                        .gap_1p5()
                        .text_sm()
                        .child(div().text_color(theme.muted_foreground).child(plural(files.len(), "file")))
                        .child(div().text_color(theme.success).child(format!("+{added}")))
                        .child(div().text_color(theme.danger).child(format!("-{removed}"))),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("refresh-changes")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::RefreshCw))
                    .label(if self.refreshing { "Reading…" } else { "Refresh" })
                    .disabled(self.refreshing)
                    .on_click(cx.listener(|view, _, _, cx| view.reload(cx))),
            )
    }

    fn render_file_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let items = self.files().iter().enumerate().map(|(f, file)| {
            let (dir, name) = split_path(&file.path);
            let (letter, color) = status_mark(file.status, cx);
            h_flex()
                .id(("changed-file", f))
                .gap_2()
                .items_center()
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .when(self.selected == Some(f), |row| row.bg(theme.list_active))
                .hover(|row| row.bg(theme.list_hover))
                .child(div().flex_none().w(px(12.)).text_xs().font_weight(FontWeight::BOLD).text_color(color).child(letter))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().text_sm().child(name.to_owned()))
                        .when(!dir.is_empty(), |col| {
                            col.child(div().truncate().text_xs().text_color(theme.muted_foreground).child(dir.to_owned()))
                        }),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap_1()
                        .text_xs()
                        .when(file.added > 0, |el| el.child(div().text_color(theme.success).child(format!("+{}", file.added))))
                        .when(file.removed > 0, |el| {
                            el.child(div().text_color(theme.danger).child(format!("-{}", file.removed)))
                        }),
                )
                .on_click(cx.listener(move |view, _, _, cx| view.show_file(f, cx)))
        });
        v_flex()
            .id("changed-files")
            .w(px(280.))
            .flex_none()
            .h_full()
            .p_1()
            .gap_0p5()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(theme.border)
            .children(items)
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let settings = Settings::get(cx);
        let font_size = settings.font_size.clamp(6., 72.) * 0.9;
        let height = px((font_size * 1.6).round());
        let number_width = px((font_size * 3.6).round());
        let files = self.files();
        let row = h_flex().h(height).w_full().items_center().whitespace_nowrap();
        match self.rows.get(ix).copied() {
            None | Some(Row::Gap) => row.into_any_element(),
            Some(Row::File(f)) => {
                let file = &files[f];
                let folded = self.is_folded(file);
                let (dir, name) = split_path(&file.path);
                let (_, color) = status_mark(file.status, cx);
                let renamed = file.old_path.as_ref().map(|old| format!("{old} → "));
                row.id(("file-row", f))
                    .gap_2()
                    .px_2()
                    .cursor_pointer()
                    .bg(theme.foreground.opacity(0.06))
                    .hover(|row| row.bg(theme.foreground.opacity(0.1)))
                    .border_t_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Icon::new(if folded { Lucide::ChevronRight } else { Lucide::ChevronDown })
                            .size(px(14.))
                            .text_color(theme.muted_foreground),
                    )
                    .child(Icon::new(status_icon(file.status)).size(px(14.)).text_color(color))
                    .child(
                        h_flex()
                            .text_sm()
                            .when_some(renamed, |el, renamed| el.child(div().text_color(theme.muted_foreground).child(renamed)))
                            .child(div().text_color(theme.muted_foreground).child(dir.to_owned()))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(name.to_owned())),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .text_xs()
                            .child(div().text_color(theme.success).child(format!("+{}", file.added)))
                            .child(div().text_color(theme.danger).child(format!("-{}", file.removed))),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| view.toggle_fold(f, cx)))
                    .into_any_element()
            }
            Some(Row::Hunk(f, h)) => row
                .pl(number_width * 2.)
                .bg(theme.info.opacity(0.08))
                .font_family(settings.mono_font())
                .text_size(px(font_size))
                .text_color(theme.muted_foreground)
                .child(div().px_2().child(files[f].hunks[h].header.clone()))
                .into_any_element(),
            Some(Row::Note(f)) => {
                let file = &files[f];
                let note = match (file.binary, file.status) {
                    (true, _) => "Binary or large file not shown",
                    (false, FileStatus::Renamed) => "Renamed without changes",
                    (false, FileStatus::Untracked | FileStatus::Added) => "Empty file",
                    _ => "Only the file mode changed",
                };
                row.pl(number_width * 2.)
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(div().px_2().child(note))
                    .into_any_element()
            }
            Some(Row::Line(f, h, l)) => {
                let line = &files[f].hunks[h].lines[l];
                let (sign, tint): (&str, Option<Hsla>) = match line.kind {
                    LineKind::Added => ("+", Some(theme.success)),
                    LineKind::Removed => ("-", Some(theme.danger)),
                    LineKind::Context => (" ", None),
                    LineKind::Note => ("", None),
                };
                let number = |n: Option<u32>| {
                    div()
                        .flex_none()
                        .w(number_width)
                        .h_full()
                        .pr_2()
                        .flex()
                        .items_center()
                        .justify_end()
                        .text_color(theme.muted_foreground.opacity(0.7))
                        .when_some(tint, |el, tint| el.bg(tint.opacity(0.1)))
                        .child(n.map(|n| n.to_string()).unwrap_or_default())
                };
                let text_color = match line.kind {
                    LineKind::Note => theme.muted_foreground,
                    _ => theme.foreground,
                };
                let text = expand_tabs(&line.text);
                let colors = self.line_colors(f, h, l, text.len(), cx);
                row.font_family(settings.mono_font())
                    .text_size(px(font_size))
                    .when_some(tint, |el, tint| el.bg(tint.opacity(0.14)))
                    .child(number(line.old))
                    .child(number(line.new))
                    .child(
                        div()
                            .flex_none()
                            .w(px(font_size * 1.4))
                            .pl_1()
                            .text_color(tint.unwrap_or(theme.muted_foreground))
                            .child(sign),
                    )
                    .child(div().pr_4().text_color(text_color).child(StyledText::new(text).with_highlights(colors)))
                    .into_any_element()
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Old,
    New,
}

/// Code colours for one file. Each side of the diff is parsed as one text: the
/// old side has the removed lines, the new side the added ones, and both keep the
/// unchanged lines around them so the code still makes sense.
struct FileColors {
    old: SyntaxHighlighter,
    new: SyntaxHighlighter,
    /// By hunk, then line: the side it shows from and where it starts there.
    lines: Vec<Vec<Option<(Side, usize)>>>,
}

impl FileColors {
    fn new(file: &FileDiff) -> Option<Self> {
        let grammar = language::detect(Path::new(&file.path))?.grammar;
        LanguageRegistry::singleton().language(grammar)?;
        if file.hunks.iter().map(|hunk| hunk.lines.len()).sum::<usize>() > MAX_COLOR_LINES {
            return None;
        }
        let (mut old, mut new) = (String::new(), String::new());
        let push = |text: &mut String, line: &str| {
            let start = text.len();
            // Tabs as shown, so offsets match the drawn text.
            text.push_str(&expand_tabs(line));
            text.push('\n');
            start
        };
        let lines = file
            .hunks
            .iter()
            .map(|hunk| {
                hunk.lines
                    .iter()
                    .map(|line| match line.kind {
                        LineKind::Removed => Some((Side::Old, push(&mut old, &line.text))),
                        LineKind::Added => Some((Side::New, push(&mut new, &line.text))),
                        LineKind::Context => {
                            push(&mut old, &line.text);
                            Some((Side::New, push(&mut new, &line.text)))
                        }
                        LineKind::Note => None,
                    })
                    .collect()
            })
            .collect();
        let parse = |text: &str| {
            let mut highlighter = SyntaxHighlighter::new(grammar);
            highlighter.update(None, &Rope::from(text), None);
            highlighter
        };
        Some(Self {
            old: parse(&old),
            new: parse(&new),
            lines,
        })
    }
}

/// Deleted files and very big diffs start folded.
fn folds_at_first(file: &FileDiff) -> bool {
    file.status == FileStatus::Deleted || file.added + file.removed > FOLD_LINES
}

fn status_mark(status: FileStatus, cx: &App) -> (&'static str, Hsla) {
    let theme = cx.theme();
    match status {
        FileStatus::Modified => ("M", theme.warning),
        FileStatus::Added => ("A", theme.success),
        FileStatus::Untracked => ("U", theme.success),
        FileStatus::Deleted => ("D", theme.danger),
        FileStatus::Renamed => ("R", theme.info),
    }
}

fn status_icon(status: FileStatus) -> Lucide {
    match status {
        FileStatus::Modified => Lucide::FilePen,
        FileStatus::Added | FileStatus::Untracked => Lucide::FilePlus,
        FileStatus::Deleted => Lucide::FileMinus,
        FileStatus::Renamed => Lucide::ArrowRightLeft,
    }
}

/// `src/ui/` and `main.rs` from `src/ui/main.rs`.
fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => path.split_at(i + 1),
        None => ("", path),
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

/// Tabs as four spaces, so code lines up.
fn expand_tabs(text: &str) -> String {
    text.replace('\t', "    ")
}

fn display_len(text: &str) -> usize {
    text.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

impl Focusable for ChangesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ChangesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let message = match &self.state {
            State::Loading => Some("Reading changes…".to_owned()),
            State::Failed(reason) => Some(reason.clone()),
            State::Ready(changes) if changes.files.is_empty() => Some(if self.commit.is_some() {
                "This commit changed no files.".to_owned()
            } else {
                "No changes. Everything matches the last commit.".to_owned()
            }),
            State::Ready(_) => None,
        };
        let body = match message {
            Some(text) => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(text)
                .into_any_element(),
            None => h_flex()
                .flex_1()
                .min_h_0()
                .child(self.render_file_list(cx))
                .child(
                    uniform_list(
                        "diff-rows",
                        self.rows.len(),
                        cx.processor(|view, range: Range<usize>, _, cx| {
                            view.color_rows(range.clone());
                            range.map(|ix| view.render_row(ix, cx)).collect::<Vec<_>>()
                        }),
                    )
                    .flex_1()
                    .h_full()
                    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                    .with_width_from_item(Some(self.widest))
                    .track_scroll(&self.scroll),
                )
                .into_any_element(),
        };
        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .child(self.render_header(cx))
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_paths_and_counts() {
        assert_eq!(split_path("src/ui/main.rs"), ("src/ui/", "main.rs"));
        assert_eq!(split_path("README.md"), ("", "README.md"));
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(3, "file"), "3 files");
        assert_eq!(display_len("\tab"), 6);
    }
}
