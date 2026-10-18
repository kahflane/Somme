//! Files open beside a terminal or in their own tab: a tab per file, the code,
//! and a status line. Files can be on this computer or on the server an SSH
//! shell runs on.

use std::io::{BufReader, BufWriter, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::highlighter::LanguageRegistry;
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Position, Rope};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, KeyBinding, MouseButton, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, actions, div, prelude::FluentBuilder as _, px,
};

use crate::editor::big::{BigText, BigTextEvent};
use crate::editor::language::{self, Language};
use crate::editor::lsp::{self, Client, DocProvider, Status};
use crate::settings::Settings;
use crate::editor::sudo;
use crate::terminal::disk::{Disk, DiskError};
use crate::ui::common::Lucide;
use crate::ui::native;

actions!(editor_pane, [SaveFile, CloseFile]);

const CONTEXT: &str = "EditorPane";
/// Bigger files open without a language server, which gets the whole text on each change.
const LSP_MAX_BYTES: usize = 2 * 1024 * 1024;
/// Files bigger than this, or with more lines than `BIG_LINES`, open in the
/// plain editor made for big files. The code editor slows down on them.
const BIG_BYTES: u64 = 8 * 1024 * 1024;
const BIG_LINES: usize = 100_000;
/// Any size at all, for reads that take a limit.
const ANY_SIZE: u64 = 1 << 40;

pub fn init(cx: &mut App) {
    #[cfg(target_os = "macos")]
    let m = "cmd";
    #[cfg(not(target_os = "macos"))]
    let m = "ctrl";
    cx.bind_keys([
        KeyBinding::new(&format!("{m}-s"), SaveFile, Some(CONTEXT)),
        KeyBinding::new(&format!("{m}-w"), CloseFile, Some(CONTEXT)),
    ]);
}

pub enum EditorPaneEvent {
    /// The last file was closed.
    Empty,
    /// A file was opened, closed, or saved under a new name.
    FilesChanged,
}

/// Whether a language server helps with a file.
enum Help {
    Server { client: Rc<Client>, uri: String },
    /// A server is installed, but the project folder is not trusted yet.
    Untrusted { root: PathBuf, found: lsp::Found },
    /// Why there is none, empty when the language has no server to suggest.
    None(String),
}

/// A file's text, as [`read_text`] reads it.
pub enum FileText {
    /// Small enough for the code editor.
    Code(String),
    /// For the plain editor made for big files.
    Big(Rope),
}

impl FileText {
    fn sort(text: String) -> Self {
        let lines = memchr::memchr_iter(b'\n', text.as_bytes()).count();
        if text.len() as u64 > BIG_BYTES || lines > BIG_LINES {
            Self::Big(Rope::from(text))
        } else {
            Self::Code(text)
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Code(text) => text.is_empty(),
            Self::Big(text) => text.len() == 0,
        }
    }
}

/// What shows a file's text.
enum Body {
    /// The code editor, with colors and code help.
    Code(Entity<EditorState>),
    /// The plain editor that stays fast on big files.
    Big(Entity<BigText>),
}

impl Body {
    fn id(&self) -> EntityId {
        match self {
            Self::Code(editor) => editor.entity_id(),
            Self::Big(editor) => editor.entity_id(),
        }
    }

    fn text(&self, cx: &App) -> Rope {
        match self {
            Self::Code(editor) => editor.read(cx).text().clone(),
            Self::Big(editor) => editor.read(cx).text().clone(),
        }
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            Self::Code(editor) => editor.focus_handle(cx),
            Self::Big(editor) => editor.focus_handle(cx),
        }
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        match self {
            Self::Code(editor) => editor.update(cx, |editor, cx| editor.focus(window, cx)),
            Self::Big(editor) => window.focus(&editor.focus_handle(cx), cx),
        }
    }

    /// The cursor's line and column, counting from zero.
    fn cursor(&self, cx: &App) -> (usize, usize) {
        match self {
            Self::Code(editor) => {
                let Position { line, character } = editor.read(cx).cursor_position();
                (line as usize, character as usize)
            }
            Self::Big(editor) => editor.read(cx).cursor_position(),
        }
    }
}

struct OpenFile {
    path: PathBuf,
    disk: Disk,
    body: Body,
    language: Option<Language>,
    /// The highlighter knows the language.
    colored: bool,
    /// Changed since it was opened or saved.
    dirty: bool,
    /// Counts changes, to tell whether typing went on during a save.
    edits: u64,
    /// Read and saved as root, with sudo.
    as_root: bool,
    /// Not saved anywhere yet. Its text is kept in the scratch folder.
    scratch: bool,
    /// Writes a scratch file's text once typing stops.
    autosave: Option<Task<()>>,
    help: Help,
    _subscriptions: [Subscription; 2],
}

impl OpenFile {
    fn name(&self) -> SharedString {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into()
    }

    /// Writes a scratch file's text to the scratch folder.
    fn keep_scratch(&self, cx: &App) {
        if let Err(err) = write_text(&Disk::Local, &self.path, &self.body.text(cx)) {
            tracing::warn!(%err, "could not keep the scratch file");
        }
    }

    /// Stops the language server watching this file. A scratch file is deleted.
    fn forget(self) {
        if let Help::Server { client, uri } = &self.help {
            client.close(uri);
        }
        if self.scratch {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub struct EditorPane {
    files: Vec<OpenFile>,
    active: usize,
    /// Fills a tab of its own, instead of sitting beside terminals.
    own_tab: bool,
    focus_handle: FocusHandle,
    _quit: Option<Subscription>,
}

impl EventEmitter<EditorPaneEvent> for EditorPane {}

impl EditorPane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            files: Vec::new(),
            active: 0,
            own_tab: false,
            focus_handle: cx.focus_handle(),
            _quit: None,
        }
    }

    /// A pane for a tab of its own, which can hold scratch files.
    pub fn in_tab(cx: &mut Context<Self>) -> Self {
        // Typing just before quitting would miss the autosave.
        let quit = cx.on_app_quit(|pane, cx| {
            pane.keep_scratch_now(cx);
            async {}
        });
        Self {
            own_tab: true,
            _quit: Some(quit),
            ..Self::new(cx)
        }
    }

    /// Shows `path` when it is open already.
    pub fn show_open(&mut self, path: &Path, disk: &Disk, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.files.iter().position(|file| file.path == path && file.disk.same(disk)) else {
            return false;
        };
        self.activate(ix, window, cx);
        true
    }

    /// Adds a file read with [`read_text`], or shows it when it is open
    /// already. `shell_path` is where to look for language servers. `as_root`
    /// saves it with sudo, as it was read.
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &mut self,
        path: PathBuf,
        disk: Disk,
        text: FileText,
        as_root: bool,
        shell_path: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.show_open(&path, &disk, window, cx) {
            return;
        }
        let (body, language, colored, help, subscriptions) = match text {
            FileText::Code(text) => {
                let language = language::detect(&path);
                let colored = language.is_some_and(|l| LanguageRegistry::singleton().language(l.grammar).is_some());
                let size = text.len();
                let editor = cx.new(|cx| {
                    let editor = EditorState::new(window, cx).default_value(text);
                    match language.filter(|_| colored) {
                        Some(language) => editor.language(language.grammar),
                        None => editor,
                    }
                });
                let help = match language {
                    // Language servers run here, and read files from this computer.
                    Some(_) if !disk.is_local() => Help::None("Code help runs for files on this computer".into()),
                    Some(_) if size > LSP_MAX_BYTES => Help::None("File too big for code help".into()),
                    Some(language) => Self::help_for(&path, &language, &editor, shell_path, cx),
                    None => Help::None(String::new()),
                };
                let subscriptions = [
                    cx.observe(&editor, |_, _, cx| cx.notify()),
                    cx.subscribe_in(&editor, window, |pane, editor, event, _, cx| {
                        if let InputEvent::Change = event {
                            pane.changed(editor.entity_id(), cx);
                        }
                    }),
                ];
                (Body::Code(editor), language, colored, help, subscriptions)
            }
            FileText::Big(text) => {
                let editor = cx.new(|cx| BigText::new(text, cx));
                let subscriptions = [
                    cx.observe(&editor, |_, _, cx| cx.notify()),
                    cx.subscribe(&editor, |pane, editor, _: &BigTextEvent, cx| pane.changed(editor.entity_id(), cx)),
                ];
                let help = Help::None("Big file: plain text only".into());
                (Body::Big(editor), None, false, help, subscriptions)
            }
        };
        self.files.push(OpenFile {
            path,
            disk,
            body,
            language,
            colored,
            dirty: false,
            edits: 0,
            as_root,
            scratch: false,
            autosave: None,
            help,
            _subscriptions: subscriptions,
        });
        self.activate(self.files.len() - 1, window, cx);
        cx.emit(EditorPaneEvent::FilesChanged);
    }

    /// Notes a change to a file's text.
    fn changed(&mut self, id: EntityId, cx: &mut Context<Self>) {
        let Some(file) = self.files.iter_mut().find(|file| file.body.id() == id) else { return };
        file.edits += 1;
        if file.scratch {
            // Empty scratch files close without asking.
            file.dirty = file.body.text(cx).len() > 0;
            file.autosave = Some(Self::keep_scratch_soon(id, cx));
        } else {
            file.dirty = true;
        }
        if let (Help::Server { client, uri }, Body::Code(editor)) = (&file.help, &file.body) {
            client.sync(uri, editor.read(cx).text());
        }
        cx.notify();
    }

    /// Adds a scratch file from the scratch folder, with its kept text.
    pub fn add_scratch(&mut self, path: PathBuf, text: FileText, window: &mut Window, cx: &mut Context<Self>) {
        let dirty = !text.is_empty();
        self.add(path.clone(), Disk::Local, text, false, None, window, cx);
        if let Some(file) = self.files.iter_mut().find(|file| file.path == path) {
            file.scratch = true;
            file.dirty = dirty;
        }
    }

    /// Writes a scratch file's text half a second after the last change.
    fn keep_scratch_soon(id: EntityId, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |pane, cx| {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            let _ = pane.update(cx, |pane, cx| {
                if let Some(file) = pane.files.iter().find(|file| file.body.id() == id) {
                    file.keep_scratch(cx);
                }
            });
        })
    }

    /// Writes every scratch file's text now.
    fn keep_scratch_now(&mut self, cx: &mut Context<Self>) {
        for file in self.files.iter_mut().filter(|file| file.scratch) {
            file.autosave = None;
            file.keep_scratch(cx);
        }
    }

    /// The files to open again next time, with whether each is a scratch file.
    pub fn files(&self) -> impl Iterator<Item = (&Path, bool)> {
        self.files.iter().filter(|file| file.disk.is_local()).map(|file| (file.path.as_path(), file.scratch))
    }

    pub fn holds(&self, path: &Path) -> bool {
        self.files.iter().any(|file| file.path == path && file.disk.is_local())
    }

    /// The name of the file in front.
    pub fn title(&self) -> SharedString {
        self.files.get(self.active).map(OpenFile::name).unwrap_or_default()
    }

    /// Lets go of every file, for a tab closed without saving. Scratch text is thrown away.
    pub fn discard(&mut self) {
        for file in self.files.drain(..) {
            file.forget();
        }
    }

    /// Opens `path` with `selection` picked, for Go to Definition.
    fn open_at(&mut self, path: PathBuf, selection: Option<lsp_types::Range>, window: &mut Window, cx: &mut Context<Self>) {
        match read_text(&Disk::Local, &path) {
            Ok(text) => self.add(path.clone(), Disk::Local, text, false, None, window, cx),
            Err(err) => return cant_open(&path, &err, window, cx),
        }
        let file = self.files.iter().find(|file| file.path == path && file.disk.is_local());
        let (Some(range), Some(Body::Code(editor))) = (selection, file.map(|file| &file.body)) else {
            return;
        };
        editor.update(cx, |editor, cx| {
            editor.set_cursor_position(range.start, window, cx);
        });
    }

    /// Starts the language server for a file, unless its folder is not trusted yet.
    fn help_for(
        path: &Path,
        language: &Language,
        editor: &Entity<EditorState>,
        shell_path: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Help {
        let found = match lsp::find_server(language, shell_path) {
            Ok(found) => found,
            Err(reason) => return Help::None(reason),
        };
        let root = language::project_root(path, language.roots);
        if !lsp::is_trusted(&root, cx) {
            return Help::Untrusted { root, found };
        }
        Self::attach_server(path, language, editor, &found, &root, cx)
    }

    fn attach_server(
        path: &Path,
        language: &Language,
        editor: &Entity<EditorState>,
        found: &lsp::Found,
        root: &Path,
        cx: &mut Context<Self>,
    ) -> Help {
        let client = match lsp::server_for(found, root, cx) {
            Ok(client) => client,
            Err(reason) => return Help::None(reason),
        };
        let uri = lsp::file_uri(path);
        client.open(&uri, language.lsp_id, editor.read(cx).text(), editor.downgrade());
        let provider = Rc::new(DocProvider {
            client: client.clone(),
            uri: uri.clone(),
        });
        let pane = cx.entity().downgrade();
        let own_uri = uri.clone();
        editor.update(cx, |editor, _| {
            let lsp = editor.lsp_mut();
            lsp.completion_provider = Some(provider.clone());
            lsp.hover_provider = Some(provider.clone());
            lsp.definition_provider = Some(provider);
            // A definition in another file opens that file here.
            lsp.show_document = Some(Rc::new(move |params, window, cx| {
                if params.uri.as_str() == own_uri {
                    return false;
                }
                let Some(target) = lsp::uri_path(params.uri.as_str()) else { return false };
                let selection = params.selection;
                pane.update(cx, |pane, cx| pane.open_at(target, selection, window, cx)).is_ok()
            }));
        });
        Help::Server { client, uri }
    }

    /// Asks before letting language servers run in `root`, then starts them.
    fn trust_folder(&mut self, root: PathBuf, server: String, window: &mut Window, cx: &mut Context<Self>) {
        // Another tab may have trusted it already.
        if lsp::is_trusted(&root, cx) {
            self.start_trusted(cx);
            return;
        }
        let pane = cx.entity().downgrade();
        let detail = format!(
            "{server} will read {} and can run code from it, like build scripts. \
             Only turn this on for folders you trust.",
            root.display()
        );
        native::confirm(
            "Turn on code help for this folder?",
            &detail,
            "Trust Folder",
            move |_, cx| {
                let folder = root.to_string_lossy().into_owned();
                Settings::update(cx, |settings| {
                    if !settings.trusted_folders.contains(&folder) {
                        settings.trusted_folders.push(folder);
                    }
                });
                let _ = pane.update(cx, |pane, cx| pane.start_trusted(cx));
            },
            window,
            cx,
        );
    }

    /// Starts servers for open files whose folders are trusted now.
    fn start_trusted(&mut self, cx: &mut Context<Self>) {
        for ix in 0..self.files.len() {
            let file = &self.files[ix];
            let (Help::Untrusted { root, found }, Some(language), Body::Code(editor)) = (&file.help, file.language, &file.body)
            else {
                continue;
            };
            if !lsp::is_trusted(root, cx) {
                continue;
            }
            let (path, editor, root, found) = (file.path.clone(), editor.clone(), root.clone(), found.clone());
            self.files[ix].help = Self::attach_server(&path, &language, &editor, &found, &root, cx);
        }
        cx.notify();
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.files.get(ix) else { return };
        self.active = ix;
        file.body.focus(window, cx);
        cx.notify();
    }

    fn save(&mut self, _: &SaveFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.files.get(self.active) else { return };
        if file.scratch {
            self.save_scratch(window, cx);
            return;
        }
        let text = file.body.text(cx);
        let (path, disk, id, name) = (file.path.clone(), file.disk.clone(), file.body.id(), file.name());
        let (edits, as_root) = (file.edits, file.as_root);
        // Off the UI thread, as big files take a while.
        cx.spawn_in(window, async move |pane, cx| {
            if !as_root {
                let result = disk.clone().run({
                    let (path, text) = (path.clone(), text.clone());
                    move |disk| write_text(disk, &path, &text)
                });
                match result.await {
                    Err(DiskError::Denied) if disk.can_sudo() => {
                        let title = format!("Save {name} with sudo?");
                        let detail = "Your account can't change this file. Somme can save it as root with sudo. \
                                      It keeps its owner and permissions.";
                        if !sudo::ask(&title, detail, "Save with sudo", cx).await {
                            return;
                        }
                    }
                    result => {
                        let _ = pane.update_in(cx, |pane, window, cx| pane.saved(id, edits, &text, result, false, window, cx));
                        return;
                    }
                }
            }
            let bytes = text.to_string();
            let result = sudo::as_root(&disk, move |disk, password| disk.write_as_root(&path, bytes.as_bytes(), password), cx).await;
            let _ = pane.update_in(cx, |pane, window, cx| pane.saved(id, edits, &text, result, true, window, cx));
        })
        .detach();
    }

    /// Asks where to save the scratch file in front, then saves it there.
    fn save_scratch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.files.get(self.active) else { return };
        let (id, name) = (file.body.id(), file.name());
        let dir = dirs::document_dir().or_else(dirs::home_dir).unwrap_or_default();
        let picked = cx.prompt_for_new_path(&dir, Some(name.as_ref()));
        cx.spawn_in(window, async move |pane, cx| {
            let Ok(Ok(Some(path))) = picked.await else { return };
            let _ = pane.update_in(cx, |pane, window, cx| pane.move_scratch(id, path, window, cx));
        })
        .detach();
    }

    /// Saves a scratch file at `path`, where it lives from now on.
    fn move_scratch(&mut self, id: EntityId, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.files.iter().position(|file| file.body.id() == id) else { return };
        if let Err(err) = write_text(&Disk::Local, &path, &self.files[ix].body.text(cx)) {
            native::error(&format!("Can't save {}", self.files[ix].name()), &err.to_string(), window, cx);
            return;
        }
        // The new name may say what language it is. Big files stay plain.
        let (language, colored, help) = match &self.files[ix].body {
            Body::Code(editor) => {
                let editor = editor.clone();
                let language = language::detect(&path);
                let colored = language.is_some_and(|l| LanguageRegistry::singleton().language(l.grammar).is_some());
                if let Some(language) = language.filter(|_| colored) {
                    editor.update(cx, |editor, cx| editor.set_highlighter(language.grammar, cx));
                }
                let help = match language {
                    Some(language) => Self::help_for(&path, &language, &editor, None, cx),
                    None => Help::None(String::new()),
                };
                (language, colored, help)
            }
            Body::Big(_) => (None, false, Help::None("Big file: plain text only".into())),
        };
        let file = &mut self.files[ix];
        let scratch = std::mem::replace(&mut file.path, path);
        let _ = std::fs::remove_file(scratch);
        file.scratch = false;
        file.autosave = None;
        file.dirty = false;
        file.language = language;
        file.colored = colored;
        file.help = help;
        cx.emit(EditorPaneEvent::FilesChanged);
        cx.notify();
    }

    /// Marks a file saved as `text`, or says why it wasn't. `as_root` says it
    /// was saved with sudo, so later saves go the same way.
    #[allow(clippy::too_many_arguments)]
    fn saved(
        &mut self,
        id: EntityId,
        edits: u64,
        text: &Rope,
        result: Result<(), DiskError>,
        as_root: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = self.files.iter_mut().find(|file| file.body.id() == id) else { return };
        match result {
            Ok(()) => {}
            Err(DiskError::Cancelled) => return,
            Err(err) => {
                let name = file.name();
                native::error(&format!("Can't save {name}"), &err.to_string(), window, cx);
                return;
            }
        }
        file.as_root |= as_root;
        // Typing during a slow save leaves changes to save.
        file.dirty = file.edits != edits;
        if let Help::Server { client, uri } = &file.help {
            client.saved(uri, text);
        }
        cx.notify();
    }

    fn on_close_file(&mut self, _: &CloseFile, window: &mut Window, cx: &mut Context<Self>) {
        self.request_close(self.active, window, cx);
    }

    /// Closes a file, asking first when it has unsaved changes.
    fn request_close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.files.get(ix) else { return };
        if !file.dirty {
            self.close(ix, window, cx);
            return;
        }
        let (id, pane) = (file.body.id(), cx.entity().downgrade());
        native::confirm(
            &format!("Close {} without saving?", file.name()),
            "Your changes will be lost.",
            "Close Without Saving",
            move |window, cx| {
                let _ = pane.update(cx, |pane, cx| {
                    if let Some(ix) = pane.files.iter().position(|file| file.body.id() == id) {
                        pane.close(ix, window, cx);
                    }
                });
            },
            window,
            cx,
        );
    }

    fn close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.files.remove(ix).forget();
        if self.files.is_empty() {
            cx.emit(EditorPaneEvent::Empty);
            return;
        }
        cx.emit(EditorPaneEvent::FilesChanged);
        if self.active >= ix && self.active > 0 {
            self.active -= 1;
        }
        self.activate(self.active, window, cx);
    }

    /// Whether any open file has unsaved changes.
    pub fn has_changes(&self) -> bool {
        self.files.iter().any(|file| file.dirty)
    }

    fn render_tabs(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let tabs = self.files.iter().enumerate().map(|(ix, file)| {
            let active = ix == self.active;
            h_flex()
                .id(("file-tab", ix))
                .group("file-tab")
                .h_full()
                .max_w(px(200.))
                .flex_none()
                .pl_3()
                .pr_1()
                .gap_1()
                .items_center()
                .border_r_1()
                .border_color(theme.border)
                .text_sm()
                .cursor_pointer()
                .when(active, |tab| tab.bg(theme.background).text_color(theme.foreground))
                .when(!active, |tab| tab.text_color(theme.muted_foreground).hover(|tab| tab.bg(theme.muted)))
                .child(div().min_w_0().truncate().child(file.name()))
                .child(
                    div()
                        .relative()
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        // A dot marks unsaved changes; the close button shows on hover.
                        .when(file.dirty, |slot| {
                            slot.child(
                                div()
                                    .group_hover("file-tab", |dot| dot.invisible())
                                    .size(px(7.))
                                    .rounded_full()
                                    .bg(theme.foreground.opacity(0.7)),
                            )
                        })
                        .child(
                            div()
                                .when(file.dirty, |close| close.absolute().invisible().group_hover("file-tab", |c| c.visible()))
                                .child(
                                    Button::new(("close-file", ix))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .tab_stop(false)
                                        .on_click(cx.listener(move |pane, _, window, cx| {
                                            cx.stop_propagation();
                                            pane.request_close(ix, window, cx);
                                        })),
                                ),
                        ),
                )
                .on_click(cx.listener(move |pane, _, window, cx| pane.activate(ix, window, cx)))
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |pane, _, window, cx| pane.request_close(ix, window, cx)),
                )
        });
        h_flex()
            .id("file-tabs")
            .h(px(32.))
            .flex_none()
            .w_full()
            .overflow_x_scroll()
            .bg(theme.foreground.opacity(0.03))
            .border_b_1()
            .border_color(theme.border)
            .children(tabs)
    }

    fn render_status(&self, file: &OpenFile, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (line, character) = file.body.cursor(cx);
        let plain = file.scratch || matches!(file.body, Body::Big(_)) || file.path.extension().is_some_and(|ext| ext == "txt");
        let language = match (file.language, file.colored) {
            (Some(language), true) => language.label.to_owned(),
            _ if plain => "Plain text".to_owned(),
            _ => "Language support is unavailable for this file type".to_owned(),
        };
        let (server, server_color) = match &file.help {
            Help::Server { client, .. } => match client.status() {
                Status::Starting => (format!("{} starting…", client.name), theme.muted_foreground),
                Status::Ready => (client.name.clone(), theme.muted_foreground),
                Status::Stopped(reason) => (format!("{} stopped: {reason}", client.name), theme.warning),
            },
            Help::None(reason) => (reason.clone(), theme.muted_foreground),
            Help::Untrusted { .. } => (String::new(), theme.muted_foreground),
        };
        let trust = match &file.help {
            Help::Untrusted { root, found } => Some((root.clone(), found.name())),
            _ => None,
        };
        let problems = match &file.body {
            Body::Code(editor) => editor.read(cx).diagnostics().map_or(0, |set| set.len()),
            Body::Big(_) => 0,
        };
        h_flex()
            .h(px(26.))
            .flex_none()
            .px_3()
            .gap_4()
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(div().flex_none().child(format!("Ln {}, Col {}", line + 1, character + 1)))
            .when(problems > 0, |bar| {
                bar.child(
                    h_flex()
                        .flex_none()
                        .gap_1()
                        .items_center()
                        .text_color(theme.warning)
                        .child(Icon::new(Lucide::TriangleAlert).size(px(12.)))
                        .child(format!("{problems}")),
                )
            })
            .child(div().flex_1())
            .when(file.as_root, |bar| {
                bar.child(div().flex_none().text_color(theme.warning).child("Saving as root"))
            })
            .when(!server.is_empty(), |bar| {
                bar.child(div().min_w_0().truncate().text_color(server_color).child(server))
            })
            .when_some(trust, |bar, (root, server)| {
                bar.child(
                    Button::new("trust-folder")
                        .ghost()
                        .xsmall()
                        .label("Turn on code help")
                        .tab_stop(false)
                        .on_click(cx.listener(move |pane, _, window, cx| {
                            pane.trust_folder(root.clone(), server.clone(), window, cx);
                        })),
                )
            })
            .child(div().flex_none().child(language))
    }
}

/// Says a file did not open, and why.
pub fn cant_open(path: &Path, err: &DiskError, window: &mut Window, cx: &mut App) {
    if *err == DiskError::Cancelled {
        return;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    native::error(&format!("Can't open {name}"), &err.to_string(), window, cx);
}

/// Reads a file as text, of any size. Err says why it can't be edited. Slow
/// for big files and server files: run it in [`Disk::run`].
pub fn read_text(disk: &Disk, path: &Path) -> Result<FileText, DiskError> {
    if disk.is_local() && std::fs::metadata(path)?.len() > BIG_BYTES {
        return read_big(path).map(FileText::Big);
    }
    text(disk.read(path, ANY_SIZE)?).map(FileText::sort)
}

/// [`read_text`] as root, with sudo.
pub fn read_text_as_root(disk: &Disk, path: &Path, password: Option<&str>) -> Result<FileText, DiskError> {
    text(disk.read_as_root(path, ANY_SIZE, password)?).map(FileText::sort)
}

/// Reads a big file on this computer straight into a rope, without a second copy.
fn read_big(path: &Path) -> Result<Rope, DiskError> {
    let mut file = std::fs::File::open(path)?;
    let mut head = vec![0; 8192];
    let read = file.read(&mut head)?;
    if head[..read].contains(&0) {
        return Err(not_text());
    }
    file.seek(SeekFrom::Start(0))?;
    Rope::from_reader(BufReader::with_capacity(1 << 20, file)).map_err(|err| match err.kind() {
        std::io::ErrorKind::InvalidData => not_utf8(),
        _ => err.into(),
    })
}

fn text(bytes: Vec<u8>) -> Result<String, DiskError> {
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return Err(not_text());
    }
    String::from_utf8(bytes).map_err(|_| not_utf8())
}

fn not_text() -> DiskError {
    DiskError::Other("It is not a text file.".into())
}

fn not_utf8() -> DiskError {
    DiskError::Other("It is not UTF-8 text.".into())
}

/// Writes `text` to `path`. On this computer it goes a piece at a time, so a
/// big file needs no second copy.
fn write_text(disk: &Disk, path: &Path, text: &Rope) -> Result<(), DiskError> {
    if !disk.is_local() {
        return disk.write(path, text.to_string().as_bytes());
    }
    let mut out = BufWriter::new(std::fs::File::create(path)?);
    text.write_to(&mut out)?;
    out.flush()?;
    Ok(())
}

impl Focusable for EditorPane {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.files.get(self.active) {
            Some(file) => file.body.focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

impl Render for EditorPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = Settings::get(cx).clone();
        let theme = cx.theme();
        let file = self.files.get(self.active);
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::on_close_file))
            .size_full()
            .bg(theme.background)
            .when(!self.own_tab, |pane| pane.border_l_1().border_color(theme.border))
            // In a tab of its own, the workspace tab already names a single file.
            .when(!self.own_tab || self.files.len() > 1, |pane| pane.child(self.render_tabs(cx)))
            .when_some(file, |pane, file| {
                let body = match &file.body {
                    Body::Code(editor) => Editor::new(editor)
                        .appearance(false)
                        .bordered(false)
                        .size_full()
                        .font_family(settings.font_family.clone())
                        .text_size(px(settings.font_size.clamp(6., 72.)))
                        .into_any_element(),
                    Body::Big(editor) => editor.clone().into_any_element(),
                };
                pane.child(div().flex_1().min_h_0().child(body)).child(self.render_status(file, cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_binary_and_non_utf8_files() {
        let dir = std::env::temp_dir().join(format!("somme-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        std::fs::write(dir.join("b.bin"), [1, 0, 2]).unwrap();
        std::fs::write(dir.join("c.txt"), [0xff, 0xfe, 0x41]).unwrap();
        let read = |name: &str| read_text(&Disk::Local, &dir.join(name));
        let results = [read("a.txt"), read("b.bin"), read("c.txt")];
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(&results[0], Ok(FileText::Code(text)) if text == "hello"));
        assert!(results[1].is_err());
        assert!(results[2].is_err());
    }

    #[test]
    fn files_on_a_server_open_and_save_at_any_size() {
        use crate::terminal::disk::fake;

        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("srv")).unwrap();
        let line = "2026-10-18 10:00:00 INFO order query done\n";
        let big = line.repeat(BIG_BYTES as usize / line.len() + 10);
        std::fs::write(root.path().join("srv/app.log"), &big).unwrap();
        std::fs::write(root.path().join("srv/a.txt"), "hi").unwrap();
        let disk = fake::disk(root.path());
        let Ok(FileText::Big(text)) = read_text(&disk, Path::new("/srv/app.log")) else {
            panic!("a big file on a server opens in the plain editor");
        };
        assert_eq!(text.len(), big.len());
        write_text(&disk, Path::new("/srv/copy.log"), &text).unwrap();
        assert_eq!(std::fs::read(root.path().join("srv/copy.log")).unwrap(), big.as_bytes());
        assert!(matches!(read_text(&disk, Path::new("/srv/a.txt")), Ok(FileText::Code(text)) if text == "hi"));
    }

    #[test]
    fn big_and_long_files_go_to_the_plain_editor() {
        let dir = std::env::temp_dir().join(format!("somme-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let line = "2026-10-18 10:00:00 INFO order query done\n";
        let big = line.repeat(BIG_BYTES as usize / line.len() + 10);
        std::fs::write(dir.join("big.log"), &big).unwrap();
        let read = read_text(&Disk::Local, &dir.join("big.log"));
        let saved = match &read {
            Ok(FileText::Big(text)) => write_text(&Disk::Local, &dir.join("copy.log"), text).map(|()| std::fs::read(dir.join("copy.log")).unwrap()),
            _ => Err(DiskError::Cancelled),
        };
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(&read, Ok(FileText::Big(text)) if text.len() == big.len()));
        assert_eq!(saved.unwrap(), big.as_bytes());
        assert!(matches!(FileText::sort("a\n".repeat(BIG_LINES + 1)), FileText::Big(_)));
        assert!(matches!(FileText::sort("a\n".repeat(10)), FileText::Code(_)));
    }
}
