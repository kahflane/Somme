//! Opening files and folders in Somme from outside it: "Open With" in Finder,
//! Explorer or a file manager, dropping on the app icon, or `Somme <path>` in
//! a shell. A folder opens in a new terminal there, a file in the editor.
//!
//! Only one Somme opens the database, so a copy started while Somme runs
//! hands its paths to the running one over a local link and quits.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use gpui_kit::{AnyWindowHandle, App, Entity};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};

use crate::mcp::install::{self, Step};
use crate::mcp::make_private_dir;
use crate::mcp::transport::{self, Listener, Mark, Stream};
use crate::runtime;
use crate::ui::workspace::Workspace;

/// Holds the link to the running Somme.
const DIR: &str = "open";
/// What was last set up for "Open With", so it is done again only when it changes.
const MARKER: &str = "open-with.txt";
const MAX_MESSAGE: u64 = 1024 * 1024;
/// The name file managers know Somme by. Wayland matches windows to it.
const APP_ID: &str = "app.somme.desktop";

/// Kinds of text files Somme offers to open, by extension. The macOS app
/// claims the same ones (see `macos-app.sh`).
pub const EXTENSIONS: &[&str] = &[
    "txt", "text", "log", "md", "markdown", "mdx", "json", "jsonc", "jsonl", "ndjson", "yaml", "yml", "toml", "ini",
    "cfg", "conf", "env", "properties", "csv", "tsv", "xml", "plist", "html", "htm", "css", "scss", "less", "js",
    "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "vue", "svelte", "py", "pyi", "rb", "php", "pl", "lua", "go",
    "rs", "c", "h", "cpp", "cc", "cxx", "hpp", "hh", "cs", "java", "kt", "kts", "swift", "scala", "dart", "zig",
    "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd", "sql", "graphql", "gql", "proto", "tf", "hcl", "nix",
    "dockerfile", "gitignore", "editorconfig", "diff", "patch", "lock",
];

/// The same for Linux file managers, by MIME type. Most text types count as
/// `text/plain` too.
const MIME_TYPES: &[&str] = &[
    "inode/directory",
    "text/plain",
    "text/markdown",
    "text/x-markdown",
    "application/json",
    "application/x-ndjson",
    "application/jsonl",
    "application/x-yaml",
    "application/yaml",
    "application/toml",
    "application/xml",
    "application/javascript",
    "application/x-shellscript",
    "application/sql",
    "application/x-php",
    "application/x-ruby",
    "application/x-perl",
];

/// Paths given on the command line, made whole. Options are left out.
pub fn from_args() -> Vec<PathBuf> {
    let here = std::env::current_dir().ok();
    std::env::args_os()
        .skip(1)
        .filter(|arg| !arg.to_string_lossy().starts_with('-'))
        .map(PathBuf::from)
        .map(|path| match &here {
            Some(here) if path.is_relative() => here.join(path),
            _ => path,
        })
        .collect()
}

/// Paths from the file links macOS sends when Somme is asked to open files.
pub fn from_urls(urls: Vec<String>) -> Vec<PathBuf> {
    urls.iter()
        .filter_map(|url| url.strip_prefix("file://"))
        .map(|rest| rest.strip_prefix("localhost").unwrap_or(rest))
        .map(|path| PathBuf::from(percent_encoding::percent_decode_str(path).decode_utf8_lossy().into_owned()))
        .collect()
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Ask {
    open: Vec<PathBuf>,
}

/// Gives `paths` to the Somme that runs for `data_dir`, which also comes to
/// the front. False when none runs, or it doesn't answer.
pub fn hand_over(data_dir: &Path, paths: &[PathBuf]) -> bool {
    let dir = data_dir.join(DIR);
    let Ok(mut line) = serde_json::to_string(&Ask { open: paths.to_vec() }) else { return false };
    line.push('\n');
    runtime::block_on(async move {
        tokio::time::timeout(Duration::from_secs(3), async move {
            let stream = transport::connect(&dir).await.ok()?;
            let (read, mut write) = tokio::io::split(stream);
            write.write_all(line.as_bytes()).await.ok()?;
            write.flush().await.ok()?;
            let mut answer = String::new();
            BufReader::new(read).take(16).read_line(&mut answer).await.ok()?;
            (answer.trim_end() == "ok").then_some(())
        })
        .await
        .ok()
        .flatten()
        .is_some()
    })
}

/// Takes paths from copies of Somme started later. Dropping it stops that.
pub struct Listening {
    task: tokio::task::JoinHandle<()>,
    mark: Mark,
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.mark.remove();
        self.task.abort();
    }
}

pub fn listen(data_dir: &Path, asks: UnboundedSender<Vec<PathBuf>>) -> std::io::Result<Listening> {
    let dir = data_dir.join(DIR);
    make_private_dir(&dir)?;
    let handle = runtime::handle();
    let (listener, mark) = {
        let _entered = handle.enter();
        Listener::bind(&dir)?
    };
    let task = handle.spawn(async move {
        loop {
            match listener.accept().await {
                Ok(Some(incoming)) => {
                    let asks = asks.clone();
                    tokio::spawn(async move {
                        if let Some(stream) = incoming.admit().await {
                            answer(stream, asks).await;
                        }
                    });
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(%err, "a copy of Somme could not reach this one");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    });
    Ok(Listening { task, mark })
}

async fn answer(stream: Stream, asks: UnboundedSender<Vec<PathBuf>>) {
    let (read, mut write) = tokio::io::split(stream);
    let mut line = Vec::new();
    let read = tokio::time::timeout(
        Duration::from_secs(5),
        BufReader::new(read).take(MAX_MESSAGE).read_until(b'\n', &mut line),
    )
    .await;
    if !matches!(read, Ok(Ok(_))) {
        return;
    }
    let Ok(ask) = serde_json::from_slice::<Ask>(&line) else { return };
    // The copy that sent them made them whole.
    let paths = ask.open.into_iter().filter(|path| path.is_absolute()).collect();
    if asks.unbounded_send(paths).is_ok() {
        let _ = write.write_all(b"ok\n").await;
        let _ = write.flush().await;
    }
}

/// Opens what Somme is asked to open, and brings it to the front.
pub fn pump(mut asks: UnboundedReceiver<Vec<PathBuf>>, window: AnyWindowHandle, workspace: Entity<Workspace>, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Some(paths) = asks.next().await {
            let _ = window.update(cx, |_, window, cx| {
                window.activate_window();
                workspace.update(cx, |workspace, cx| workspace.open_paths(paths, window, cx));
            });
            cx.update(|cx| cx.activate(true));
        }
    })
    .detach();
}

/// Where and what to set up.
struct Target {
    exe: PathBuf,
    home: PathBuf,
}

/// The Linux steps: a desktop entry that file managers list under "Open
/// With", and its icon.
fn linux(target: &Target) -> Vec<Step> {
    let apps = target.home.join(".local/share/applications");
    let icon = target.home.join(".local/share/icons/hicolor/scalable/apps").join(format!("{APP_ID}.svg"));
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Somme\nGenericName=Terminal and SSH client\nComment=Terminals, servers and files\n\
         Exec={} %F\nIcon={APP_ID}\nTerminal=false\nStartupWMClass={APP_ID}\nCategories=Development;System;TerminalEmulator;\n\
         MimeType={};\n",
        desktop_word(&target.exe.to_string_lossy()),
        MIME_TYPES.join(";"),
    );
    let apps_arg = apps.to_string_lossy().into_owned();
    vec![
        Step::Write(apps.join(format!("{APP_ID}.desktop")), entry),
        Step::Write(icon, include_str!("../assets/icon.svg").to_owned()),
        // Some file managers only see new entries once this runs.
        install::run("update-desktop-database", &[&apps_arg], true),
    ]
}

/// `text` as one word on a desktop entry's Exec line.
fn desktop_word(text: &str) -> String {
    let escaped: String = text
        .chars()
        .flat_map(|ch| match ch {
            '"' | '`' | '$' | '\\' => vec!['\\', ch],
            '%' => vec!['%', '%'],
            _ => vec![ch],
        })
        .collect();
    // Backslashes are read once more for the whole line.
    format!("\"{}\"", escaped.replace('\\', r"\\"))
}

/// The Windows steps: Somme in "Open with" for text files, and "Open with
/// Somme" on folders. All of it is for this user only.
fn windows(target: &Target, out: &Path) -> Vec<Step> {
    let exe = target.exe.to_string_lossy().into_owned();
    let name = exe.rsplit(['\\', '/']).next().filter(|name| !name.is_empty()).unwrap_or("Somme.exe").to_owned();
    let classes = r"HKEY_CURRENT_USER\Software\Classes";
    let value = |text: &str| format!("\"{}\"", text.replace('\\', r"\\").replace('"', "\\\""));
    let icon = value(&format!("\"{exe}\",0"));
    let mut reg = String::from("Windows Registry Editor Version 5.00\r\n\r\n");
    let mut key = |path: String, values: &[(&str, String)]| {
        reg.push_str(&format!("[{classes}\\{path}]\r\n"));
        for (name, data) in values {
            let name = if name.is_empty() { "@".to_owned() } else { value(name) };
            reg.push_str(&format!("{name}={data}\r\n"));
        }
        reg.push_str("\r\n");
    };
    let app = format!(r"Applications\{name}");
    key(app.clone(), &[("FriendlyAppName", value("Somme"))]);
    key(format!(r"{app}\DefaultIcon"), &[("", icon.clone())]);
    key(format!(r"{app}\shell\open\command"), &[("", value(&format!("\"{exe}\" \"%1\"")))]);
    let types: Vec<(String, String)> = EXTENSIONS.iter().map(|ext| (format!(".{ext}"), value(""))).collect();
    let types: Vec<(&str, String)> = types.iter().map(|(ext, data)| (ext.as_str(), data.clone())).collect();
    key(format!(r"{app}\SupportedTypes"), &types);
    for ext in EXTENSIONS {
        key(format!(r".{ext}\OpenWithList\{name}"), &[]);
    }
    for place in [r"Directory\shell\Somme", r"Directory\Background\shell\Somme"] {
        key(place.to_owned(), &[("", value("Open with Somme")), ("Icon", icon.clone())]);
        key(format!(r"{place}\command"), &[("", value(&format!("\"{exe}\" \"%V\"")))]);
    }
    let file = out.join("open-with.reg");
    let file_arg = file.to_string_lossy().into_owned();
    vec![Step::WriteUtf16(file, reg), install::run("reg", &["import", &file_arg], false)]
}

fn steps(target: &Target, data_dir: &Path) -> Vec<Step> {
    if cfg!(target_os = "macos") {
        // The app bundle says what it opens (see `macos-app.sh`).
        Vec::new()
    } else if cfg!(windows) {
        windows(target, data_dir)
    } else {
        linux(target)
    }
}

/// Lists Somme in the system's "Open With" for text files and folders. Runs
/// the system's tools, so call it off the UI thread. Test data folders get
/// the files written into them instead.
pub fn register(data_dir: &Path) -> anyhow::Result<()> {
    // An AppImage runs from a folder that is gone once it quits.
    let exe = match std::env::var_os("APPIMAGE") {
        Some(image) => PathBuf::from(image),
        None => std::env::current_exe()?,
    };
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot find the home folder"))?;
    let steps = steps(&Target { exe, home }, data_dir);
    let marker = data_dir.join(MARKER);
    let wanted = install::describe(&steps);
    if steps.is_empty() || std::fs::read_to_string(&marker).ok().as_deref() == Some(wanted.as_str()) {
        return Ok(());
    }
    if std::env::var_os("SOMME_DATA_DIR").is_some() {
        install::dry_run(&data_dir.join("open-with-files"), &steps)?;
    } else {
        for step in &steps {
            install::perform(step)?;
        }
    }
    std::fs::write(marker, wanted)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(steps: &[Step]) -> Vec<&str> {
        steps
            .iter()
            .filter_map(|step| match step {
                Step::Write(_, text) | Step::WriteUtf16(_, text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn file_links_become_paths() {
        let urls = vec![
            "file:///Users/me/My%20Notes/read%20me.md".to_owned(),
            "file://localhost/tmp/project/".to_owned(),
            "https://example.test/x".to_owned(),
        ];
        assert_eq!(
            from_urls(urls),
            vec![PathBuf::from("/Users/me/My Notes/read me.md"), PathBuf::from("/tmp/project/")]
        );
    }

    #[test]
    fn linux_lists_somme_for_folders_and_text() {
        let target = Target {
            exe: PathBuf::from("/opt/Somme 1.0/somme"),
            home: PathBuf::from("/home/me"),
        };
        let steps = linux(&target);
        let entry = written(&steps)[0];
        assert!(entry.contains("Exec=\"/opt/Somme 1.0/somme\" %F\n"), "{entry}");
        assert!(entry.contains("MimeType=inode/directory;text/plain;"), "{entry}");
        assert_eq!(
            steps[0],
            Step::Write("/home/me/.local/share/applications/app.somme.desktop.desktop".into(), entry.to_owned())
        );
        assert_eq!(desktop_word(r#"/a "b"/100%/c\d"#), r#""/a \\"b\\"/100%%/c\\\\d""#);
    }

    #[test]
    fn windows_adds_open_with_for_this_user() {
        let target = Target {
            exe: PathBuf::from(r"C:\Users\Me\AppData\Local\Somme\Somme.exe"),
            home: PathBuf::from(r"C:\Users\Me"),
        };
        let steps = windows(&target, Path::new("/data"));
        let reg = written(&steps)[0];
        assert!(reg.starts_with("Windows Registry Editor Version 5.00\r\n"));
        assert!(reg.contains(
            "[HKEY_CURRENT_USER\\Software\\Classes\\Applications\\Somme.exe\\shell\\open\\command]\r\n@=\"\\\"C:\\\\Users\\\\Me\\\\AppData\\\\Local\\\\Somme\\\\Somme.exe\\\" \\\"%1\\\"\"\r\n"
        ), "{reg}");
        assert!(reg.contains("\".md\"=\"\"\r\n"));
        assert!(reg.contains("[HKEY_CURRENT_USER\\Software\\Classes\\.json\\OpenWithList\\Somme.exe]"));
        assert!(reg.contains("[HKEY_CURRENT_USER\\Software\\Classes\\Directory\\shell\\Somme\\command]\r\n@=\"\\\"C:\\\\Users\\\\Me\\\\AppData\\\\Local\\\\Somme\\\\Somme.exe\\\" \\\"%V\\\"\""));
        assert!(!reg.contains("HKEY_LOCAL_MACHINE"));
        assert!(matches!(&steps[1], Step::Run { program, args, .. } if program == "reg" && args[0] == "import"));
    }

    #[test]
    fn the_mac_app_claims_the_same_files() {
        let script = include_str!("../macos-app.sh");
        for ext in EXTENSIONS {
            assert!(script.contains(&format!("<string>{ext}</string>")), "macos-app.sh does not claim .{ext}");
        }
        assert!(script.contains("<string>public.folder</string>"));
    }

    #[test]
    fn a_running_somme_takes_the_paths() {
        runtime::init().unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert!(!hand_over(dir.path(), &[PathBuf::from("/tmp/a.md")]));
        let (asks, mut taken) = futures::channel::mpsc::unbounded();
        let listening = listen(dir.path(), asks).unwrap();
        let paths = vec![PathBuf::from("/tmp/a.md"), PathBuf::from("/tmp/folder")];
        assert!(hand_over(dir.path(), &paths));
        assert_eq!(runtime::block_on(taken.next()), Some(paths));
        drop(listening);
        assert!(!hand_over(dir.path(), &[]));
    }
}
