//! Files open beside a terminal: a tab per file, the code, and a status line.
//! Files can be on this computer or on the server an SSH shell runs on.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::highlighter::LanguageRegistry;
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Position, Rope};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, KeyBinding, MouseButton, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, actions, div, prelude::FluentBuilder as _, px,
};

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
/// Bigger files do not open.
const MAX_BYTES: u64 = 20 * 1024 * 1024;

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
}

/// Whether a language server helps with a file.
enum Help {
    Server { client: Rc<Client>, uri: String },
    /// A server is installed, but the project folder is not trusted yet.
    Untrusted { root: PathBuf, found: lsp::Found },
    /// Why there is none, empty when the language has no server to suggest.
    None(String),
}

struct OpenFile {
    path: PathBuf,
    disk: Disk,
    editor: Entity<EditorState>,
    language: Option<Language>,
    /// The highlighter knows the language.
    colored: bool,
    /// Changed since it was opened or saved.
    dirty: bool,
    /// Read and saved as root, with sudo.
    as_root: bool,
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
}

pub struct EditorPane {
    files: Vec<OpenFile>,
    active: usize,
    focus_handle: FocusHandle,
}

impl EventEmitter<EditorPaneEvent> for EditorPane {}

impl EditorPane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            files: Vec::new(),
            active: 0,
            focus_handle: cx.focus_handle(),
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
        text: String,
        as_root: bool,
        shell_path: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.show_open(&path, &disk, window, cx) {
            return;
        }
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
                if let InputEvent::Change = event
                    && let Some(file) = pane.files.iter_mut().find(|file| file.editor == *editor)
                {
                    file.dirty = true;
                    if let Help::Server { client, uri } = &file.help {
                        client.sync(uri, editor.read(cx).text());
                    }
                    cx.notify();
                }
            }),
        ];
        self.files.push(OpenFile {
            path,
            disk,
            editor,
            language,
            colored,
            dirty: false,
            as_root,
            help,
            _subscriptions: subscriptions,
        });
        self.activate(self.files.len() - 1, window, cx);
    }

    /// Opens `path` with `selection` picked, for Go to Definition.
    fn open_at(&mut self, path: PathBuf, selection: Option<lsp_types::Range>, window: &mut Window, cx: &mut Context<Self>) {
        match read_text(&Disk::Local, &path) {
            Ok(text) => self.add(path.clone(), Disk::Local, text, false, None, window, cx),
            Err(err) => return cant_open(&path, &err, window, cx),
        }
        let file = self.files.iter().find(|file| file.path == path && file.disk.is_local());
        let (Some(range), Some(file)) = (selection, file) else {
            return;
        };
        file.editor.update(cx, |editor, cx| {
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
            let (Help::Untrusted { root, found }, Some(language)) = (&file.help, file.language) else { continue };
            if !lsp::is_trusted(root, cx) {
                continue;
            }
            let (path, editor, root, found) = (file.path.clone(), file.editor.clone(), root.clone(), found.clone());
            self.files[ix].help = Self::attach_server(&path, &language, &editor, &found, &root, cx);
        }
        cx.notify();
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.files.get(ix) else { return };
        self.active = ix;
        file.editor.update(cx, |editor, cx| editor.focus(window, cx));
        cx.notify();
    }

    fn save(&mut self, _: &SaveFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.files.get(self.active) else { return };
        let text = file.editor.read(cx).text().clone();
        let (path, disk, editor, name) = (file.path.clone(), file.disk.clone(), file.editor.clone(), file.name());
        if disk.is_local() {
            let result = disk.write(&path, text.to_string().as_bytes());
            self.saved(&editor, &text, result, false, window, cx);
            return;
        }
        let as_root = file.as_root;
        cx.spawn_in(window, async move |pane, cx| {
            let bytes = text.to_string();
            if !as_root {
                let result = disk.clone().run({
                    let (path, bytes) = (path.clone(), bytes.clone());
                    move |disk| disk.write(&path, bytes.as_bytes())
                });
                match result.await {
                    Err(DiskError::Denied) => {
                        let title = format!("Save {name} with sudo?");
                        let detail = "Your account can't change this file. Somme can save it as root with sudo. \
                                      It keeps its owner and permissions.";
                        if !sudo::ask(&title, detail, "Save with sudo", cx).await {
                            return;
                        }
                    }
                    result => {
                        let _ = pane.update_in(cx, |pane, window, cx| pane.saved(&editor, &text, result, false, window, cx));
                        return;
                    }
                }
            }
            let result = sudo::as_root(&disk, move |disk, password| disk.write_as_root(&path, bytes.as_bytes(), password), cx).await;
            let _ = pane.update_in(cx, |pane, window, cx| pane.saved(&editor, &text, result, true, window, cx));
        })
        .detach();
    }

    /// Marks a file saved as `text`, or says why it wasn't. `as_root` says it
    /// was saved with sudo, so later saves go the same way.
    fn saved(
        &mut self,
        editor: &Entity<EditorState>,
        text: &Rope,
        result: Result<(), DiskError>,
        as_root: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = self.files.iter_mut().find(|file| file.editor == *editor) else { return };
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
        file.dirty = *file.editor.read(cx).text() != *text;
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
        let (editor, pane) = (file.editor.clone(), cx.entity().downgrade());
        native::confirm(
            &format!("Close {} without saving?", file.name()),
            "Your changes will be lost.",
            "Close Without Saving",
            move |window, cx| {
                let _ = pane.update(cx, |pane, cx| {
                    if let Some(ix) = pane.files.iter().position(|file| file.editor == editor) {
                        pane.close(ix, window, cx);
                    }
                });
            },
            window,
            cx,
        );
    }

    fn close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.files.remove(ix);
        if let Help::Server { client, uri } = &file.help {
            client.close(uri);
        }
        if self.files.is_empty() {
            cx.emit(EditorPaneEvent::Empty);
            return;
        }
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
        let Position { line, character } = file.editor.read(cx).cursor_position();
        let language = match (file.language, file.colored) {
            (Some(language), true) => language.label.to_owned(),
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
        let problems = file.editor.read(cx).diagnostics().map_or(0, |set| set.len());
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

/// Reads a file as text. Err says why it can't be edited. Slow for big files
/// and server files: run it in [`Disk::run`].
pub fn read_text(disk: &Disk, path: &Path) -> Result<String, DiskError> {
    text(disk.read(path, MAX_BYTES)?)
}

/// [`read_text`] as root, with sudo.
pub fn read_text_as_root(disk: &Disk, path: &Path, password: Option<&str>) -> Result<String, DiskError> {
    text(disk.read_as_root(path, MAX_BYTES, password)?)
}

fn text(bytes: Vec<u8>) -> Result<String, DiskError> {
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return Err(DiskError::Other("It is not a text file.".into()));
    }
    String::from_utf8(bytes).map_err(|_| DiskError::Other("It is not UTF-8 text.".into()))
}

impl Focusable for EditorPane {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.files.get(self.active) {
            Some(file) => file.editor.focus_handle(cx),
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
            .border_l_1()
            .border_color(theme.border)
            .child(self.render_tabs(cx))
            .when_some(file, |pane, file| {
                pane.child(
                    div().flex_1().min_h_0().child(
                        Editor::new(&file.editor)
                            .appearance(false)
                            .bordered(false)
                            .size_full()
                            .font_family(settings.font_family.clone())
                            .text_size(px(settings.font_size.clamp(6., 72.))),
                    ),
                )
                .child(self.render_status(file, cx))
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
        assert_eq!(results[0].as_deref(), Ok("hello"));
        assert!(results[1].is_err());
        assert!(results[2].is_err());
    }
}
