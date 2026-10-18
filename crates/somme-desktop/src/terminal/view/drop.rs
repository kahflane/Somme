//! Files and folders dragged onto a terminal. On this computer, a text file
//! opens in the editor and a folder becomes the shell's folder; anything else
//! is typed in as a path, as other terminals do. On a server, all of it is
//! copied into the shell's folder there.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use alacritty_terminal::term::TermMode;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, ExternalPaths, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
    Window, div, px,
};

use super::{TerminalView, TerminalViewEvent};
use crate::editor::system_open;
use crate::terminal::blocks::short_path;
use crate::terminal::complete;
use crate::terminal::session::Status;

/// How long a note about a finished upload stays.
const NOTE_TIME: Duration = Duration::from_secs(6);

/// An upload, shown over the terminal.
pub(super) struct Note {
    id: usize,
    text: String,
    state: NoteState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NoteState {
    Busy,
    Done,
    Failed,
}

enum Dropped {
    Folder(PathBuf),
    Text(PathBuf),
    Other(PathBuf),
}

/// Looks at what a path is. Reads the start of files, so keep it off the UI thread.
fn sort(path: PathBuf) -> Dropped {
    match std::fs::metadata(&path) {
        Ok(meta) if meta.is_dir() => Dropped::Folder(path),
        Ok(meta) if meta.is_file() && !system_open::opens_outside(&path) && looks_like_text(&path) => Dropped::Text(path),
        _ => Dropped::Other(path),
    }
}

/// Whether a file starts the way text does: no zero bytes, and UTF-8 but for
/// a character cut off at the end.
fn looks_like_text(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else { return false };
    let mut head = Vec::new();
    if file.take(8192).read_to_end(&mut head).is_err() || head.contains(&0) {
        return false;
    }
    match std::str::from_utf8(&head) {
        Ok(_) => true,
        Err(err) => err.error_len().is_none(),
    }
}

/// `path` as one word for the shell on this computer, or `None` when no
/// quoting keeps the shell from acting on its name.
fn shell_word(path: &Path) -> Option<String> {
    word(path.to_str()?, cfg!(windows))
}

fn word(text: &str, windows: bool) -> Option<String> {
    // Typed into a terminal these are keys, not text: Ctrl-U wipes the line
    // typed so far and Enter runs what follows.
    if !complete::can_escape(text) {
        return None;
    }
    if windows {
        // Names can't hold quotes, but cmd still expands %…% inside them, and
        // PowerShell, often run from cmd, expands $… and `….
        (!text.contains(['%', '$', '`'])).then(|| format!("\"{text}\""))
    } else {
        Some(complete::escape(text))
    }
}

fn cd_command(word: &str) -> String {
    // cmd only changes the drive too with /d.
    if cfg!(windows) { format!("cd /d {word}") } else { format!("cd {word}") }
}

fn items(count: usize) -> String {
    if count == 1 { "1 item".to_owned() } else { format!("{count} items") }
}

fn files(count: u64) -> String {
    if count == 1 { "1 file".to_owned() } else { format!("{count} files") }
}

impl TerminalView {
    /// A live shell takes drops; a recording or a closed one doesn't.
    fn takes_drops(&self, cx: &App) -> bool {
        let session = self.session.read(cx);
        !session.is_replay() && session.status == Status::Connected
    }

    fn dropped(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        let paths = paths.paths().to_vec();
        if paths.is_empty() || !self.takes_drops(cx) {
            return;
        }
        window.focus(&self.focus_handle, cx);
        if !self.session.read(cx).is_local() {
            self.upload(paths, cx);
            return;
        }
        let sorted = cx.background_spawn(async move { paths.into_iter().map(sort).collect::<Vec<_>>() });
        cx.spawn_in(window, async move |view, cx| {
            let sorted = sorted.await;
            let _ = view.update(cx, |view, cx| view.drop_here(sorted, cx));
        })
        .detach();
    }

    /// Text files open in the editor. One folder becomes the shell's folder,
    /// when the shell waits for a command; the rest is typed in as paths.
    fn drop_here(&mut self, sorted: Vec<Dropped>, cx: &mut Context<Self>) {
        let mut rest = Vec::new();
        let mut skipped = 0;
        for item in sorted {
            let (folder, path) = match item {
                Dropped::Text(path) => {
                    cx.emit(TerminalViewEvent::OpenFile(path));
                    continue;
                }
                Dropped::Folder(path) => (true, path),
                Dropped::Other(path) => (false, path),
            };
            match shell_word(&path) {
                Some(word) => rest.push((folder, word)),
                None => skipped += 1,
            }
        }
        if skipped > 0 {
            let id = self.add_note(String::new(), cx);
            let text = format!("Didn't type {}. The name has characters the shell would act on.", items(skipped));
            self.end_note(id, text, NoteState::Failed, cx);
        }
        if let [(true, folder)] = rest.as_slice()
            && skipped == 0
            && self.shell_waits(cx)
        {
            self.run_text(&cd_command(folder), cx);
            return;
        }
        if !rest.is_empty() {
            let words: Vec<&str> = rest.iter().map(|(_, word)| word.as_str()).collect();
            self.paste_text(&format!("{} ", words.join(" ")), cx);
        }
    }

    /// Whether the shell is at its prompt, not running a program. Without
    /// Somme's hooks that is a guess: no full-screen program is open.
    fn shell_waits(&self, cx: &App) -> bool {
        let session = self.session.read(cx);
        match &session.blocks {
            Some(blocks) if blocks.ready => session.at_prompt(),
            _ => !session.mode().contains(TermMode::ALT_SCREEN),
        }
    }

    /// Copies files and folders from this computer into the shell's folder on
    /// the server, or its home folder when the shell doesn't say where it is.
    fn upload(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        let cwd = session.blocks.as_ref().and_then(|blocks| blocks.cwd.clone());
        let disk = session.disk();
        let id = self.add_note(format!("Uploading {}…", items(paths.len())), cx);
        cx.spawn(async move |view, cx| {
            let result = match disk.await {
                None => Err("This server doesn't take files.".to_owned()),
                Some(disk) => {
                    disk.run(move |disk| {
                        let home = disk.find_home();
                        let dir = cwd
                            .or_else(|| home.clone())
                            .ok_or_else(|| "Somme can't tell which folder the shell is in.".to_owned())?;
                        let mut count = 0;
                        for path in &paths {
                            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                            count += disk.upload(path, &dir).map_err(|err| format!("{name}: {err}"))?;
                        }
                        Ok((count, short_path(&dir, home.as_deref())))
                    })
                    .await
                }
            };
            let _ = view.update(cx, |view, cx| match result {
                Ok((count, dir)) => view.end_note(id, format!("Uploaded {} to {dir}", files(count)), NoteState::Done, cx),
                Err(err) => view.end_note(id, format!("Upload failed. {err}"), NoteState::Failed, cx),
            });
        })
        .detach();
    }

    fn add_note(&mut self, text: String, cx: &mut Context<Self>) -> usize {
        self.next_note += 1;
        let id = self.next_note;
        self.notes.push(Note {
            id,
            text,
            state: NoteState::Busy,
        });
        cx.notify();
        id
    }

    fn end_note(&mut self, id: usize, text: String, state: NoteState, cx: &mut Context<Self>) {
        let Some(note) = self.notes.iter_mut().find(|note| note.id == id) else { return };
        note.text = text;
        note.state = state;
        cx.notify();
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(NOTE_TIME).await;
            let _ = view.update(cx, |view, cx| {
                view.notes.retain(|note| note.id != id);
                cx.notify();
            });
        })
        .detach();
    }

    /// Lights up while files are dragged over the terminal, and takes them.
    pub(super) fn render_drop_target(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        self.takes_drops(cx).then(|| {
            div()
                .id("terminal-drop")
                .absolute()
                .inset_0()
                .drag_over::<ExternalPaths>(|style, _, _, cx| {
                    style.bg(cx.theme().drop_target).border_2().border_color(cx.theme().ring)
                })
                .on_drop(cx.listener(|view, paths: &ExternalPaths, window, cx| view.dropped(paths, window, cx)))
        })
    }

    pub(super) fn render_notes(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if self.notes.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let notes = self.notes.iter().map(|note| {
            h_flex()
                .gap_2()
                .px_3()
                .py_1p5()
                .max_w(px(420.))
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .shadow_md()
                .text_sm()
                .child(match note.state {
                    NoteState::Busy => Spinner::new().small().into_any_element(),
                    NoteState::Done => Icon::new(IconName::CircleCheck).small().text_color(theme.success).into_any_element(),
                    NoteState::Failed => Icon::new(IconName::TriangleAlert).small().text_color(theme.danger).into_any_element(),
                })
                .child(div().min_w_0().child(note.text.clone()))
        });
        Some(v_flex().absolute().bottom_2().right_3().gap_1().items_end().children(notes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_things_are_told_apart() {
        let dir = tempfile::tempdir().unwrap();
        let (text, binary, image) = (dir.path().join("notes"), dir.path().join("app"), dir.path().join("logo.png"));
        std::fs::write(&text, "héllo\n").unwrap();
        std::fs::write(&binary, [0x7f, b'E', b'L', b'F', 0, 1]).unwrap();
        std::fs::write(&image, "not really a png").unwrap();
        assert!(matches!(sort(dir.path().to_owned()), Dropped::Folder(_)));
        assert!(matches!(sort(text), Dropped::Text(_)));
        assert!(matches!(sort(binary), Dropped::Other(_)));
        assert!(matches!(sort(image), Dropped::Other(_)));
        assert!(matches!(sort(dir.path().join("gone")), Dropped::Other(_)));
    }

    #[test]
    fn a_character_cut_off_at_the_end_is_still_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.txt");
        let mut text = "a".repeat(8191).into_bytes();
        text.extend("é".as_bytes());
        std::fs::write(&path, &text).unwrap();
        assert!(looks_like_text(&path));
        std::fs::write(&path, [b'a', 0xff, b'b']).unwrap();
        assert!(!looks_like_text(&path));
    }

    #[test]
    fn paths_are_one_word_each() {
        assert_eq!(word("/Users/me/My Files/a&b $(x).txt", false).unwrap(), r"/Users/me/My\ Files/a\&b\ \$\(x\).txt");
        assert_eq!(word(r"C:\My Files\a&b.txt", true).unwrap(), r#""C:\My Files\a&b.txt""#);
        // Ctrl-U and Enter would wipe the line and run the rest.
        assert_eq!(word("/tmp/x\u{15}rm -rf ~\r", false), None);
        assert_eq!(word("/tmp/line\nbreak", false), None);
        for name in [r"C:\$(calc)", r"C:\%PATH%", r"C:\`calc`"] {
            assert_eq!(word(name, true), None, "{name}");
        }
    }
}
