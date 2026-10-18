//! A small language server client. It starts the server, keeps open files in step
//! with it, and asks it for completions, hover text and definitions. The problems it
//! finds go back to the editor showing the file.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};

use anyhow::{Result, anyhow};
use futures::StreamExt as _;
use futures::channel::{mpsc as async_mpsc, oneshot};
use gpui_kit::component::highlighter::Diagnostic;
use gpui_kit::component::input::{
    CompletionProvider, DefinitionProvider, EditorState, HoverProvider, Rope, RopeExt as _,
};
use gpui_kit::{App, AppContext as _, Global, Task, WeakEntity, Window};
use lsp_types::{
    CompletionContext, CompletionResponse, GotoDefinitionResponse, Hover, LocationLink, PublishDiagnosticsParams,
};
use serde_json::{Value, json};

use crate::editor::language::Language;
use crate::settings::Settings;

/// Where a server is in its life.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Starting,
    Ready,
    /// The server quit, with the last thing it printed.
    Stopped(String),
}

/// Running servers, one per program and project folder.
#[derive(Default)]
struct Servers(HashMap<(PathBuf, PathBuf), Rc<Client>>);

impl Global for Servers {}

/// A language server installed on this computer, not started yet.
#[derive(Clone, Debug)]
pub struct Found {
    program: PathBuf,
    args: &'static [&'static str],
    search: String,
}

impl Found {
    /// The program's file name, like `rust-analyzer`.
    pub fn name(&self) -> String {
        self.program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

/// The first installed server for `language`. Err says why there is none, like a
/// server that is not installed, or is empty when the language has no server.
pub fn find_server(language: &Language, shell_path: Option<&str>) -> Result<Found, String> {
    let Some(first) = language.servers.first() else {
        return Err(String::new());
    };
    let search = search_path(shell_path);
    let found = language
        .servers
        .iter()
        .find_map(|command| find_program(command[0], &search).map(|program| (program, &command[1..])));
    let Some((program, args)) = found else {
        return Err(format!("Install {} for code help", first[0]));
    };
    Ok(Found { program, args, search })
}

/// Servers read the project and can run code from it, like build scripts, so they
/// only start in folders the user trusts.
pub fn is_trusted(root: &Path, cx: &App) -> bool {
    Settings::get(cx).trusted_folders.iter().any(|folder| root.starts_with(folder))
}

/// The server for the project at `root`, started if needed. The caller checks
/// [`is_trusted`] first.
pub fn server_for(found: &Found, root: &Path, cx: &mut App) -> Result<Rc<Client>, String> {
    let key = (found.program.clone(), root.to_path_buf());
    if let Some(client) = cx.default_global::<Servers>().0.get(&key)
        && !matches!(client.status(), Status::Stopped(_))
    {
        return Ok(client.clone());
    }
    let client = Client::start(&found.program, found.args, root, &found.search, cx).map_err(|err| err.to_string())?;
    cx.default_global::<Servers>().0.insert(key, client.clone());
    Ok(client)
}

/// What the reader thread passes to the app.
enum Event {
    Diagnostics(PublishDiagnosticsParams),
    Stopped,
}

struct Doc {
    version: i32,
    /// The text the server has.
    synced: Rope,
    editor: WeakEntity<EditorState>,
}

/// State the reader thread also touches.
struct Shared {
    status: Status,
    /// Characters after which the server offers completions, like `.` or `::`.
    triggers: Vec<String>,
    /// The last line the server printed on stderr, to explain why it stopped.
    last_error: String,
}

pub struct Client {
    /// The program's file name, shown in the status bar.
    pub name: String,
    outgoing: mpsc::Sender<Vec<u8>>,
    /// Messages held back until the server answers `initialize`.
    queue: RefCell<Option<Vec<Vec<u8>>>>,
    next_id: Cell<i64>,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value>>>>>,
    shared: Arc<Mutex<Shared>>,
    docs: RefCell<HashMap<String, Doc>>,
    child: RefCell<Child>,
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.get_mut().kill();
    }
}

impl Client {
    fn start(program: &Path, args: &[&str], root: &Path, search: &str, cx: &mut App) -> Result<Rc<Self>> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(root)
            // Servers run tools of their own, like cargo or node.
            .env("PATH", search)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| anyhow!("Could not start {}: {err}", program.display()))?;
        let (stdin, stdout, stderr) = (
            child.stdin.take().expect("piped stdin"),
            child.stdout.take().expect("piped stdout"),
            child.stderr.take().expect("piped stderr"),
        );
        let name = program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let shared = Arc::new(Mutex::new(Shared {
            status: Status::Starting,
            triggers: Vec::new(),
            last_error: String::new(),
        }));
        let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value>>>>> = Arc::default();
        let (outgoing, to_write) = mpsc::channel::<Vec<u8>>();
        let (events, mut incoming) = async_mpsc::unbounded::<Event>();

        std::thread::spawn(move || write_loop(stdin, to_write));
        let errors = shared.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    errors.lock().expect("lsp state").last_error = line.trim().to_owned();
                }
            }
        });
        {
            let (pending, replies) = (pending.clone(), outgoing.clone());
            std::thread::spawn(move || read_loop(stdout, pending, replies, events));
        }

        let client = Rc::new(Self {
            name,
            outgoing,
            queue: RefCell::new(Some(Vec::new())),
            next_id: Cell::new(0),
            pending,
            shared,
            docs: RefCell::default(),
            child: RefCell::new(child),
        });

        let root_uri = file_uri(root);
        let folder = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let init = client.send_request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "rootPath": root,
                "workspaceFolders": [{ "uri": root_uri, "name": folder }],
                "clientInfo": { "name": crate::ui::common::APP_NAME },
                "capabilities": {
                    // The editor counts columns in characters.
                    "general": { "positionEncodings": ["utf-32", "utf-16"] },
                    "textDocument": {
                        "synchronization": { "didSave": true },
                        "completion": {
                            "contextSupport": true,
                            "completionItem": { "snippetSupport": false, "documentationFormat": ["markdown", "plaintext"] }
                        },
                        "hover": { "contentFormat": ["markdown", "plaintext"] },
                        "definition": { "linkSupport": true },
                        "formatting": {},
                        "publishDiagnostics": {}
                    },
                    "workspace": { "workspaceFolders": true, "configuration": true }
                }
            }),
            true,
        );

        let this = client.clone();
        cx.spawn(async move |cx| {
            let answer = init.await.unwrap_or_else(|_| Err(anyhow!("no answer")));
            cx.update(|cx| this.initialized(answer, cx));
            while let Some(event) = incoming.next().await {
                cx.update(|cx| this.handle(event, cx));
            }
        })
        .detach();
        Ok(client)
    }

    pub fn status(&self) -> Status {
        self.shared.lock().expect("lsp state").status.clone()
    }

    fn initialized(&self, answer: Result<Value>, cx: &mut App) {
        match answer {
            Ok(result) => {
                let triggers = result
                    .pointer("/capabilities/completionProvider/triggerCharacters")
                    .and_then(|chars| serde_json::from_value::<Vec<String>>(chars.clone()).ok())
                    .unwrap_or_default();
                {
                    let mut shared = self.shared.lock().expect("lsp state");
                    shared.triggers = triggers;
                    if shared.status == Status::Starting {
                        shared.status = Status::Ready;
                    }
                }
                let _ = self.outgoing.send(frame(&json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} })));
                for message in self.queue.borrow_mut().take().unwrap_or_default() {
                    let _ = self.outgoing.send(message);
                }
            }
            Err(err) => self.stop(&err.to_string()),
        }
        cx.refresh_windows();
    }

    fn handle(&self, event: Event, cx: &mut App) {
        match event {
            Event::Diagnostics(params) => {
                let docs = self.docs.borrow();
                let Some(doc) = docs.get(params.uri.as_str()) else { return };
                let _ = doc.editor.update(cx, |editor, cx| {
                    let text = editor.text().clone();
                    if let Some(set) = editor.diagnostics_mut() {
                        set.reset(&text);
                        set.extend(params.diagnostics.into_iter().map(Diagnostic::from));
                    }
                    cx.notify();
                });
            }
            Event::Stopped => {
                let reason = self.shared.lock().expect("lsp state").last_error.clone();
                self.stop(&reason);
                cx.refresh_windows();
            }
        }
    }

    fn stop(&self, reason: &str) {
        let reason = if reason.is_empty() { "it quit".to_owned() } else { reason.to_owned() };
        self.shared.lock().expect("lsp state").status = Status::Stopped(reason);
        for (_, waiting) in self.pending.lock().expect("lsp state").drain() {
            let _ = waiting.send(Err(anyhow!("the language server stopped")));
        }
    }

    fn send(&self, message: Vec<u8>, now: bool) {
        match self.queue.borrow_mut().as_mut() {
            Some(queue) if !now => queue.push(message),
            _ => {
                let _ = self.outgoing.send(message);
            }
        }
    }

    fn notify(&self, method: &str, params: Value) {
        self.send(frame(&json!({ "jsonrpc": "2.0", "method": method, "params": params })), false);
    }

    fn send_request(&self, method: &str, params: Value, now: bool) -> oneshot::Receiver<Result<Value>> {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let (tx, rx) = oneshot::channel();
        if matches!(self.status(), Status::Stopped(_)) {
            let _ = tx.send(Err(anyhow!("the language server stopped")));
            return rx;
        }
        self.pending.lock().expect("lsp state").insert(id, tx);
        self.send(frame(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })), now);
        rx
    }

    fn request(&self, method: &str, params: Value) -> impl Future<Output = Result<Value>> + use<> {
        let answer = self.send_request(method, params, false);
        async move { answer.await.unwrap_or_else(|_| Err(anyhow!("the language server stopped"))) }
    }

    /// Tells the server about a file the editor opened.
    pub fn open(&self, uri: &str, language_id: &str, text: &Rope, editor: WeakEntity<EditorState>) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": language_id, "version": 1, "text": text.to_string() } }),
        );
        let doc = Doc {
            version: 1,
            synced: text.clone(),
            editor,
        };
        self.docs.borrow_mut().insert(uri.to_owned(), doc);
    }

    /// Sends the file's text when it changed since the server last saw it.
    pub fn sync(&self, uri: &str, text: &Rope) {
        let mut docs = self.docs.borrow_mut();
        let Some(doc) = docs.get_mut(uri) else { return };
        if doc.synced == text.slice(..) {
            return;
        }
        doc.version += 1;
        doc.synced = text.clone();
        let version = doc.version;
        drop(docs);
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": text.to_string() }]
            }),
        );
    }

    pub fn saved(&self, uri: &str, text: &Rope) {
        self.sync(uri, text);
        self.notify("textDocument/didSave", json!({ "textDocument": { "uri": uri } }));
    }

    /// Asks the server how to tidy the whole file.
    pub fn format(&self, uri: &str, text: &Rope) -> impl Future<Output = Result<Vec<lsp_types::TextEdit>>> + use<> {
        self.sync(uri, text);
        let answer = self.request(
            "textDocument/formatting",
            json!({
                "textDocument": { "uri": uri },
                "options": { "tabSize": 4, "insertSpaces": true, "trimTrailingWhitespace": true }
            }),
        );
        async move { Ok(serde_json::from_value::<Option<Vec<lsp_types::TextEdit>>>(answer.await?)?.unwrap_or_default()) }
    }

    pub fn close(&self, uri: &str) {
        if self.docs.borrow_mut().remove(uri).is_some() {
            self.notify("textDocument/didClose", json!({ "textDocument": { "uri": uri } }));
        }
    }
}

/// Answers the editor's questions about one file.
pub struct DocProvider {
    pub client: Rc<Client>,
    pub uri: String,
}

impl DocProvider {
    fn position(&self, text: &Rope, offset: usize) -> Value {
        self.client.sync(&self.uri, text);
        json!({ "textDocument": { "uri": self.uri }, "position": text.offset_to_position(offset) })
    }
}

impl CompletionProvider for DocProvider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let mut params = self.position(text, offset);
        params["context"] = serde_json::to_value(trigger).unwrap_or(Value::Null);
        let answer = self.client.request("textDocument/completion", params);
        cx.background_spawn(async move {
            let found = serde_json::from_value::<Option<CompletionResponse>>(answer.await?)?;
            Ok(found.unwrap_or(CompletionResponse::Array(Vec::new())))
        })
    }

    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
        if new_text.chars().last().is_some_and(|ch| ch.is_alphanumeric() || ch == '_') {
            return true;
        }
        let shared = self.client.shared.lock().expect("lsp state");
        shared.triggers.iter().any(|trigger| new_text.ends_with(trigger.as_str()))
    }
}

impl HoverProvider for DocProvider {
    fn hover(&self, text: &Rope, offset: usize, _: &mut Window, cx: &mut App) -> Task<Result<Option<Hover>>> {
        let answer = self.client.request("textDocument/hover", self.position(text, offset));
        cx.background_spawn(async move { Ok(serde_json::from_value::<Option<Hover>>(answer.await?)?) })
    }
}

impl DefinitionProvider for DocProvider {
    fn definitions(&self, text: &Rope, offset: usize, _: &mut Window, cx: &mut App) -> Task<Result<Vec<LocationLink>>> {
        let answer = self.client.request("textDocument/definition", self.position(text, offset));
        cx.background_spawn(async move {
            let link = |location: lsp_types::Location| LocationLink {
                origin_selection_range: None,
                target_uri: location.uri,
                target_range: location.range,
                target_selection_range: location.range,
            };
            Ok(match serde_json::from_value::<Option<GotoDefinitionResponse>>(answer.await?)? {
                None => Vec::new(),
                Some(GotoDefinitionResponse::Scalar(location)) => vec![link(location)],
                Some(GotoDefinitionResponse::Array(locations)) => locations.into_iter().map(link).collect(),
                Some(GotoDefinitionResponse::Link(links)) => links,
            })
        })
    }
}

fn frame(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

fn write_loop(mut stdin: std::process::ChildStdin, messages: mpsc::Receiver<Vec<u8>>) {
    for message in messages {
        if stdin.write_all(&message).and_then(|()| stdin.flush()).is_err() {
            return;
        }
    }
}

/// Reads the server's messages: answers go to whoever asked, its questions get
/// a plain answer, and diagnostics go to the app.
fn read_loop(
    stdout: std::process::ChildStdout,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value>>>>>,
    replies: mpsc::Sender<Vec<u8>>,
    events: async_mpsc::UnboundedSender<Event>,
) {
    let mut reader = BufReader::new(stdout);
    while let Some(message) = read_message(&mut reader) {
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        match (method, id) {
            (None, Some(id)) => {
                let Some(waiting) = id.as_i64().and_then(|id| pending.lock().expect("lsp state").remove(&id)) else {
                    continue;
                };
                let answer = match message.get("error") {
                    Some(error) => Err(anyhow!(
                        "{}",
                        error.get("message").and_then(Value::as_str).unwrap_or("request failed")
                    )),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = waiting.send(answer);
            }
            (Some(method), Some(id)) => {
                let reply = server_request_reply(method, message.get("params"));
                let mut answer = json!({ "jsonrpc": "2.0", "id": id });
                match reply {
                    Some(result) => answer["result"] = result,
                    None => answer["error"] = json!({ "code": -32601, "message": "not supported" }),
                }
                let _ = replies.send(frame(&answer));
            }
            (Some("textDocument/publishDiagnostics"), None) => {
                if let Some(params) = message
                    .get("params")
                    .and_then(|params| serde_json::from_value::<PublishDiagnosticsParams>(params.clone()).ok())
                {
                    let _ = events.unbounded_send(Event::Diagnostics(params));
                }
            }
            _ => {}
        }
    }
    // Nobody will answer these now, `initialize` included.
    for (_, waiting) in pending.lock().expect("lsp state").drain() {
        let _ = waiting.send(Err(anyhow!("the language server stopped")));
    }
    let _ = events.unbounded_send(Event::Stopped);
}

/// Plain answers to what servers ask of the editor. None for questions it cannot answer.
fn server_request_reply(method: &str, params: Option<&Value>) -> Option<Value> {
    match method {
        // No settings: one empty answer per item asked about.
        "workspace/configuration" => {
            let items = params.and_then(|p| p.get("items")).and_then(Value::as_array).map_or(0, Vec::len);
            Some(Value::Array(vec![Value::Null; items]))
        }
        "client/registerCapability" | "client/unregisterCapability" | "window/workDoneProgress/create" => {
            Some(Value::Null)
        }
        "window/showMessageRequest" => Some(Value::Null),
        "workspace/applyEdit" => Some(json!({ "applied": false })),
        _ => None,
    }
}

fn read_message(reader: &mut impl std::io::BufRead) -> Option<Value> {
    loop {
        let mut length = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).ok()? == 0 {
                return None;
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse::<usize>().ok();
            }
        }
        let Some(length) = length else { continue };
        let mut body = vec![0; length];
        reader.read_exact(&mut body).ok()?;
        if let Ok(message) = serde_json::from_slice(&body) {
            return Some(message);
        }
    }
}

/// Folders to look for servers in: the shell's PATH, then the app's, then common
/// install places a GUI app does not get on its PATH.
fn search_path(shell_path: Option<&str>) -> String {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = shell_path {
        dirs.extend(std::env::split_paths(path));
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(home) = dirs::home_dir() {
        dirs.extend([".cargo/bin", "go/bin", ".local/bin", ".bun/bin"].map(|dir| home.join(dir)));
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|dir| seen.insert(dir.clone()));
    std::env::join_paths(dirs).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

fn find_program(name: &str, search: &str) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["", ".exe", ".cmd", ".bat"] } else { &[""] };
    std::env::split_paths(search).find_map(|dir| {
        names
            .iter()
            .map(|ext| dir.join(format!("{name}{ext}")))
            .find(|path| path.is_file())
    })
}

/// A `file://` link for a path.
pub fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !text.starts_with('/') {
        out.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(byte as char),
            // A Windows drive letter's colon stays as it is.
            b':' if cfg!(windows) => out.push(':'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The path in a `file://` link.
pub fn uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = rest.get(i + 1..i + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let text = String::from_utf8_lossy(&out).into_owned();
    // `/C:/x` on Windows is `C:/x`.
    let text = if cfg!(windows) { text.trim_start_matches('/').to_owned() } else { text };
    Some(PathBuf::from(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_links_round_trip() {
        let path = Path::new("/Users/me/My Project/naïve#1.rs");
        let uri = file_uri(path);
        assert_eq!(uri, "file:///Users/me/My%20Project/na%C3%AFve%231.rs");
        assert_eq!(uri_path(&uri).as_deref(), Some(path));
        assert_eq!(uri_path("https://x"), None);
    }

    #[test]
    fn reads_framed_messages() {
        let body = r#"{"jsonrpc":"2.0","id":3,"result":null}"#;
        let input = format!("Content-Length: {}\r\nContent-Type: x\r\n\r\n{body}", body.len());
        let mut reader = std::io::Cursor::new(input.into_bytes());
        let message = read_message(&mut reader).unwrap();
        assert_eq!(message["id"], 3);
        assert!(read_message(&mut reader).is_none());
    }

    #[test]
    fn answers_settings_questions_with_one_null_per_item() {
        let params = json!({ "items": [{}, {}] });
        assert_eq!(server_request_reply("workspace/configuration", Some(&params)), Some(json!([null, null])));
        assert_eq!(server_request_reply("something/else", None), None);
    }
}
