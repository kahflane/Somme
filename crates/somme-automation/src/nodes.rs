//! What each step does. The engine asks `Tools` for what lives in the app:
//! the saved servers, notifications. Commands on this computer run here.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt as _;

use somme_ai::llm::{ChatReply, ChatRequest};
use somme_patch::{Applied, Failed};

use crate::agent::{self, Job};
use crate::computer;
use crate::fence::{self, Fence};
use crate::net::{Certificate, HttpReply};
use crate::schema::{ALLOWED, DENIED, Direction, FAILED, HostPick, NO, NodeKind, OK, Test, UNSURE, YES};
use crate::values::{Shell, ShellCommand, as_text, render, render_shell, with_values};

/// Output kept from each stream of a command.
pub const OUTPUT_LIMIT: usize = 256 * 1024;

/// A saved server, as steps see it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Output {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }
}

/// Said when a step needs AI and the app gives none.
pub const NO_AI: &str = "AI is off. Turn it on in Settings → AI, with your own LLM or Jev key.";

/// Said when a server's files are wanted and the app gives none.
const SERVER_FILES: &str = "Files on servers need Somme.";
const NO_SCREEN: &str = "This computer's screen can only be used from the Somme window.";
/// The largest file an Agent step reads.
pub const READ_LIMIT: u64 = 2 * 1024 * 1024;

/// What a vault value shows as in logs and to AI.
pub const HIDDEN: &str = "[hidden]";

/// A yes/no answer and who gave it.
#[derive(Clone, Debug, PartialEq)]
pub struct Decided {
    /// The chance of yes, from 0 to 1.
    pub yes: f64,
    /// Jev or the LLM.
    pub by: String,
}

/// Text an LLM wrote.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Written {
    pub text: String,
    /// Tokens used, when the server says.
    pub tokens: Option<u64>,
}

/// A change an agent wants to make, for the user to OK.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Approval {
    /// Like "Run this command?".
    pub title: String,
    /// Where: a server's name, or "this computer".
    pub place: String,
    /// The command, patch or request.
    pub detail: String,
    /// What the agent said it is for.
    pub why: String,
    /// What it deletes is gone for good, not kept in the Somme bin.
    pub for_good: bool,
}

/// The user's answer to an approval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approved {
    Once,
    /// This and every change after it, until the task is done.
    Rest,
    No,
}

/// What the app gives the engine.
#[async_trait]
pub trait Tools: Send + Sync {
    async fn hosts(&self) -> Result<Vec<HostRef>, String>;

    async fn run_on_host(&self, host: &HostRef, command: &str, timeout: Duration) -> Result<Output, String>;

    /// `vars` are set for the command, as its `{{…}}` values.
    async fn run_local(&self, command: &str, vars: &[(String, String)], dir: Option<&str>, timeout: Duration) -> Result<Output, String> {
        run_local(command, vars, dir, timeout).await
    }

    /// Runs `fenced`, the step's command `shown` inside its fence. What
    /// checks commands reads `shown`, not the fence around it.
    async fn run_fenced_on_host(&self, host: &HostRef, shown: &str, fenced: &str, timeout: Duration) -> Result<Output, String> {
        let _ = shown;
        self.run_on_host(host, fenced, timeout).await
    }

    /// Like `run_fenced_on_host`, on this computer.
    async fn run_fenced_local(&self, shown: &str, fenced: &str, vars: &[(String, String)], dir: Option<&str>, timeout: Duration) -> Result<Output, String> {
        let _ = shown;
        self.run_local(fenced, vars, dir, timeout).await
    }

    async fn notify(&self, title: &str, body: &str);

    /// Asks Jev, else the LLM, `question` about `about`. Moves on to the
    /// next one while the chance of yes is between `no_to` and `yes_from`.
    async fn decide(&self, _question: &str, _about: &str, _yes_from: f64, _no_to: f64) -> Result<Decided, String> {
        Err(NO_AI.into())
    }

    async fn write(&self, _prompt: &str) -> Result<Written, String> {
        Err(NO_AI.into())
    }

    /// Lets a command go ahead once the user allows it; `why` says why it
    /// needs their OK. Refused when no one can be asked.
    async fn allow_command(&self, _command: &str, _host: Option<&HostRef>, why: &str) -> Result<(), String> {
        Err(format!("{why} Only you can let it run. Open Somme and run it from there."))
    }

    /// Asks the user to allow or deny. `Ok(true)` when they allow.
    async fn ask(&self, _title: &str, _message: &str, _timeout: Duration) -> Result<bool, String> {
        Err("Ask me needs someone to answer. Open Somme and run it from there.".into())
    }

    /// Asks the user to OK a change an agent wants to make. Here, the plain
    /// yes or no of `ask`.
    async fn approve(&self, approval: &Approval, timeout: Duration) -> Result<Approved, String> {
        let message = if approval.place.is_empty() { approval.detail.clone() } else { format!("On {}:\n{}", approval.place, approval.detail) };
        self.ask(&approval.title, &message, timeout).await.map(|yes| if yes { Approved::Once } else { Approved::No })
    }

    /// Asks the user a question for an agent. With `choices`, the user picks
    /// one or writes their own; here, only yes or no.
    async fn question(&self, question: &str, details: &str, _choices: &[String], timeout: Duration) -> Result<String, String> {
        self.ask(question, details, timeout).await.map(|yes| if yes { "yes" } else { "no" }.to_owned())
    }

    /// What earlier runs of this automation's agent noted for later ones.
    fn notes(&self) -> Vec<String> {
        Vec::new()
    }

    /// Tools the app adds to an agent's, like reading a skill.
    fn more_tools(&self) -> Vec<somme_ai::llm::Tool> {
        Vec::new()
    }

    /// Answers a call to one of `more_tools`.
    async fn more(&self, call: &somme_ai::llm::ToolCall) -> String {
        format!("Not done: there is no tool called {}.", call.name)
    }

    /// Copies one file over SFTP. Gives the bytes copied.
    async fn copy(&self, _host: &HostRef, _direction: Direction, _local: &str, _remote: &str, _timeout: Duration) -> Result<u64, String> {
        Err("Copying files needs Somme.".into())
    }

    /// The vault values named, opened. A name with no value is left out.
    async fn vault(&self, _names: &[String]) -> Result<BTreeMap<String, String>, String> {
        Err("Vault values need Somme.".into())
    }

    async fn http(&self, method: &str, url: &str, headers: &str, body: &str, timeout: Duration) -> Result<HttpReply, String> {
        crate::net::http(method, url, headers, body, timeout).await
    }

    async fn tls(&self, host: &str, port: u16, timeout: Duration) -> Result<Certificate, String> {
        crate::net::tls(host, port, timeout).await
    }

    async fn dns(&self, name: &str, record: &str, timeout: Duration) -> Result<Vec<String>, String> {
        crate::net::dns(name, record, timeout).await
    }

    /// One turn of an Agent step's talk with the LLM. The app checks the
    /// switches, counts the call and cleans secrets out first.
    async fn agent_turn(&self, _request: &ChatRequest) -> Result<ChatReply, String> {
        Err(NO_AI.into())
    }

    /// The guard's check for what is not a command, like `write file`.
    /// Ok when it may go ahead.
    async fn check(&self, _kind: &str, _detail: &str, _host: Option<&HostRef>) -> Result<(), String> {
        Ok(())
    }

    /// Reads a text file on a server, or on this computer when `host` is None.
    async fn read_file(&self, host: Option<&HostRef>, path: &str) -> Result<String, String> {
        match host {
            None => read_local(path),
            Some(_) => Err(SERVER_FILES.into()),
        }
    }

    /// Applies a patch on a server, or on this computer when `host` is None.
    async fn patch(&self, host: Option<&HostRef>, patch: &str) -> Result<Applied, Failed> {
        match host {
            None => somme_patch::apply(patch, &ComputerFiles).await,
            Some(_) => Err(Failed { message: SERVER_FILES.into(), done: Applied::default() }),
        }
    }

    /// Takes control of this computer's screen, mouse and keys, in `apps`
    /// only, and shows the user how to take it back.
    async fn computer_start(&self, _apps: &[String]) -> Result<(), String> {
        Err(NO_SCREEN.into())
    }

    /// Does one thing on the screen and gives what it looks like after.
    /// Fails with [`computer::STOPPED`] once the user took back control.
    async fn computer(&self, _action: &computer::Action) -> Result<computer::Seen, String> {
        Err(NO_SCREEN.into())
    }

    /// Gives control of the screen back.
    async fn computer_end(&self) {}

    /// True once the user took back control of the screen.
    fn computer_stopped(&self) -> bool {
        false
    }
}

/// A path on this computer: `~` and relative paths start at the home folder.
pub fn local_path(path: &str) -> std::path::PathBuf {
    let path = expand_home(path.trim());
    match std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        Some(home) if path.is_relative() => std::path::Path::new(&home).join(path),
        _ => path,
    }
}

/// Reads a text file on this computer, up to [`READ_LIMIT`].
pub fn read_local(path: &str) -> Result<String, String> {
    let file = local_path(path);
    let size = std::fs::metadata(&file).map_err(|err| format!("Could not read {}: {err}", file.display()))?.len();
    if size > READ_LIMIT {
        return Err(format!("{} is {size} bytes, more than Somme reads ({READ_LIMIT}).", file.display()));
    }
    let bytes = std::fs::read(&file).map_err(|err| format!("Could not read {}: {err}", file.display()))?;
    String::from_utf8(bytes).map_err(|_| format!("{} is not a text file.", file.display()))
}

/// This computer's files, for patches.
pub struct ComputerFiles;

#[async_trait]
impl somme_patch::Files for ComputerFiles {
    async fn read(&self, path: &str) -> std::io::Result<String> {
        std::fs::read_to_string(local_path(path))
    }

    async fn write(&self, path: &str, contents: &str) -> std::io::Result<()> {
        let path = local_path(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)
    }

    async fn remove(&self, path: &str) -> std::io::Result<()> {
        let path = local_path(path);
        if std::fs::metadata(&path)?.is_dir() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "path is a directory"));
        }
        std::fs::remove_file(path)
    }
}

/// How a step ended: the way out it took, and what later steps can use.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Finished {
    pub port: String,
    pub result: Value,
    /// Why it failed, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Finished {
    pub fn ok(result: Value) -> Self {
        Self { port: OK.into(), result, error: None }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        let error = error.into();
        Self { port: FAILED.into(), result: json!({ "error": error }), error: Some(error) }
    }

    pub fn is_failure(&self) -> bool {
        self.port == FAILED
    }
}

/// Runs one try of a step, with its `{{…}}` values filled from `context`.
/// `ai_text`: its command holds text an AI wrote in this run, so the user
/// must allow it first. `note` gets lines about what a long step is doing.
/// `fence`: it runs inside one, as a step the AI wrote does.
pub async fn run(
    kind: &NodeKind,
    context: &Value,
    tools: &dyn Tools,
    timeout: Duration,
    ai_text: bool,
    note: &(dyn Fn(String) + Send + Sync),
    fence: Option<&Fence>,
) -> Finished {
    match kind {
        NodeKind::Command { hosts, command, one_by_one, sudo } => {
            let shell = render_shell(command, context, Shell::Posix);
            let mut command = with_values(&shell.script, &shell.vars);
            if *sudo {
                command = format!("sudo -n sh -c {}", quote(&command));
            }
            let fenced = match fence {
                Some(fence) => match fence::can_fence(&command) {
                    Ok(()) => Some(fence::command(&command, &Fence { sandbox: false, ..fence.clone() })),
                    Err(why) => return Finished::failed(unfenced(&why)),
                },
                None => None,
            };
            run_command(hosts, &command, fenced.as_deref(), *one_by_one, tools, timeout, needs_ok(ai_text, &shell)).await
        }
        NodeKind::Local { command, dir } => {
            let shell = render_shell(command, context, if cfg!(windows) { Shell::Cmd } else { Shell::Posix });
            let dir = dir.as_deref().map(|dir| render(dir, context)).filter(|dir| !dir.trim().is_empty());
            let shown = with_values(&shell.script, &shell.vars);
            if let Some(why) = needs_ok(ai_text, &shell)
                && let Err(err) = tools.allow_command(&shown, None, why).await
            {
                return Finished::failed(err);
            }
            let ran = match fence {
                // Windows runs steps with cmd, which the fence doesn't know.
                Some(fence) if !cfg!(windows) => {
                    if let Err(why) = fence::can_fence(&shell.script) {
                        return Finished::failed(unfenced(&why));
                    }
                    let fenced = fence::command(&shell.script, &Fence { sandbox: fence::can_sandbox(), ..fence.clone() });
                    tools.run_fenced_local(&shown, &fenced, &shell.vars, dir.as_deref(), timeout).await
                }
                _ => tools.run_local(&shell.script, &shell.vars, dir.as_deref(), timeout).await,
            };
            match ran {
                Ok(output) => from_output(output, None),
                Err(err) => Finished::failed(err),
            }
        }
        NodeKind::If { value, test, against } => {
            let value = render(value, context);
            let against = render(against, context);
            let yes = check(*test, &value, &against);
            Finished { port: if yes { YES } else { NO }.into(), result: json!({ "value": value, "yes": yes }), error: None }
        }
        NodeKind::Wait { seconds } => {
            tokio::time::sleep(Duration::from_secs(*seconds)).await;
            Finished::ok(json!({ "waited": seconds }))
        }
        NodeKind::Notify { title, body } => {
            let (title, body) = (render(title, context), render(body, context));
            tools.notify(&title, &body).await;
            Finished::ok(json!({ "title": title, "body": body }))
        }
        NodeKind::Decide { yes_no, about, yes_from, no_to } => {
            // AI never sees vault values.
            let context = &without_vault(context);
            let (question, about) = (render(yes_no, context), render(about, context));
            match tools.decide(&question, &about, *yes_from, *no_to).await {
                Ok(decided) => {
                    let port = if decided.yes >= *yes_from {
                        YES
                    } else if decided.yes <= *no_to {
                        NO
                    } else {
                        UNSURE
                    };
                    let output = format!("{port} ({:.0}% yes, by {})", decided.yes * 100.0, decided.by);
                    Finished {
                        port: port.into(),
                        result: json!({ "answer": port, "yes": decided.yes, "by": decided.by, "output": output }),
                        error: None,
                    }
                }
                Err(err) => Finished::failed(err),
            }
        }
        NodeKind::Write { prompt } => match tools.write(&render(prompt, &without_vault(context))).await {
            Ok(written) => Finished::ok(json!({ "text": written.text, "output": written.text, "tokens": written.tokens })),
            Err(err) => Finished::failed(err),
        },
        NodeKind::Copy { host, direction, local, remote } => {
            let (local, remote) = (render(local, context), render(remote, context));
            let host = match tools.hosts().await {
                Ok(hosts) => pick(&hosts, host).into_iter().next(),
                Err(err) => return Finished::failed(err),
            };
            let Some(host) = host else { return Finished::failed("No saved server matches this step's server.") };
            let shown = match direction {
                Direction::Upload => format!("Copy {local} to {}:{remote}", host.name),
                Direction::Download => format!("Copy {}:{remote} to {local}", host.name),
            };
            if let Err(stop) = allow_ai_text(tools, ai_text, &shown, Some(&host)).await {
                return stop;
            }
            match tools.copy(&host, *direction, &local, &remote, timeout).await {
                Ok(bytes) => {
                    let output = match direction {
                        Direction::Upload => format!("Copied {local} to {}:{remote} ({bytes} bytes).", host.name),
                        Direction::Download => format!("Copied {}:{remote} to {local} ({bytes} bytes).", host.name),
                    };
                    Finished::ok(json!({ "bytes": bytes, "host": host.name, "local": local, "remote": remote, "output": output }))
                }
                Err(err) => Finished { result: json!({ "error": err, "host": host.name }), ..Finished::failed(err) },
            }
        }
        NodeKind::Ask { title, message } => {
            let (title, message) = (render(title, context), render(message, context));
            match tools.ask(&title, &message, timeout).await {
                Ok(allowed) => {
                    let port = if allowed { ALLOWED } else { DENIED };
                    let output = if allowed { "Allowed." } else { "Denied." };
                    Finished { port: port.into(), result: json!({ "answer": port, "allowed": allowed, "output": output }), error: None }
                }
                Err(err) => Finished::failed(err),
            }
        }
        NodeKind::Http { method, url, headers, body } => {
            let (url, headers, body) = (render(url, context), render(headers, context), render(body, context));
            if let Err(stop) = allow_ai_text(tools, ai_text, &format!("{method} {}\n{headers}\n\n{body}", url.trim()), None).await {
                return stop;
            }
            match tools.http(method, url.trim(), &headers, &body, timeout).await {
                Ok(reply) => from_reply(reply),
                Err(err) => Finished::failed(err),
            }
        }
        NodeKind::Tls { host, port } => {
            let host = render(host, context);
            let host = host.trim();
            if let Err(stop) = allow_ai_text(tools, ai_text, &format!("Check the certificate of {host}:{port}"), None).await {
                return stop;
            }
            match tools.tls(host, *port, timeout).await {
                Ok(certificate) => {
                    let now = crate::engine::now_ms() / 1000;
                    let mut result = certificate.to_json(now);
                    let mut output = format!("{host}: {} days left, until {}.", certificate.days_left(now), as_text(&result["expires"]));
                    if let Some(why) = &certificate.untrusted {
                        output.push_str(&format!(" Not trusted: {why}"));
                    }
                    result["output"] = json!(output);
                    Finished::ok(result)
                }
                Err(err) => Finished::failed(err),
            }
        }
        NodeKind::Dns { name, record } => {
            let name = render(name, context);
            if let Err(stop) = allow_ai_text(tools, ai_text, &format!("Look up {record} {}", name.trim()), None).await {
                return stop;
            }
            match tools.dns(name.trim(), &record.trim().to_uppercase(), timeout).await {
                Ok(records) => Finished::ok(json!({ "records": records, "output": records.join("\n") })),
                Err(err) => Finished::failed(err),
            }
        }
        NodeKind::Agent { goal, hosts, local, ask_first, max_steps, max_tokens, computer, apps } => {
            // AI never sees vault values.
            let goal = render(goal, &without_vault(context));
            // Asked here only when it won't ask before each change.
            if let Err(stop) = allow_ai_text(tools, ai_text && !ask_first, &goal, None).await {
                return stop;
            }
            let hosts = if hosts.is_empty() {
                Vec::new()
            } else {
                match tools.hosts().await {
                    Ok(all) => pick(&all, hosts),
                    Err(err) => return Finished::failed(err),
                }
            };
            if hosts.is_empty() && !local && !computer {
                return Finished::failed("No saved server matches this step's servers.");
            }
            let apps: Vec<String> = if *computer { apps.iter().map(|app| app.trim().to_owned()).filter(|app| !app.is_empty()).collect() } else { Vec::new() };
            let job = Job {
                goal: &goal,
                hosts,
                local: *local,
                ask_first: *ask_first,
                max_steps: *max_steps,
                max_tokens: *max_tokens,
                computer: *computer,
                apps,
                timeout,
                effort: Some(somme_ai::llm::Effort::Low),
                // Inside the automation's scope it works on its own; past it, it asks.
                fence: fence.cloned(),
                ask_outside: fence.is_some(),
                ..Job::default()
            };
            // The run view gets a line for each thing it does, and why a call
            // was not done.
            let lines = |event: agent::Event| match event {
                agent::Event::Said(text) => note(text),
                agent::Event::Doing { line, .. } => note(line),
                agent::Event::Done { result, .. } => {
                    if let Some(first) = result.lines().next().filter(|line| line.starts_with("Not done")) {
                        note(first.to_owned());
                    }
                }
                agent::Event::Kept { said, .. } => note(said),
            };
            agent::run(job, tools, &lines).await
        }
    }
}

/// `context` with every vault value as `[hidden]`.
fn without_vault(context: &Value) -> Value {
    let mut context = context.clone();
    if let Some(vault) = context.get_mut("vault").and_then(Value::as_object_mut) {
        vault.values_mut().for_each(|value| *value = json!(HIDDEN));
    }
    context
}

/// An answer from 400 up fails the step; its body is still kept.
fn from_reply(reply: HttpReply) -> Finished {
    let headers: serde_json::Map<String, Value> =
        reply.headers.iter().map(|(name, value)| (name.to_lowercase(), json!(value))).collect();
    let mut result = json!({
        "status": reply.status,
        "headers": headers,
        "body": reply.body,
        "output": format!("{}\n{}", reply.status, reply.body),
    });
    if let Ok(parsed) = serde_json::from_str::<Value>(reply.body.trim())
        && (parsed.is_object() || parsed.is_array())
    {
        result["json"] = parsed;
    }
    if reply.status < 400 {
        return Finished::ok(result);
    }
    let error = format!("The server answered {}.", reply.status);
    result["error"] = json!(error);
    Finished { port: FAILED.into(), result, error: Some(error) }
}

/// `text` in single quotes, for a shell.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn check(test: Test, value: &str, against: &str) -> bool {
    let (value_trimmed, against_trimmed) = (value.trim(), against.trim());
    let number = |text: &str| text.trim().trim_end_matches('%').parse::<f64>().ok();
    match test {
        Test::Equals => value_trimmed == against_trimmed,
        Test::NotEquals => value_trimmed != against_trimmed,
        Test::Contains => value.contains(against),
        Test::NotContains => !value.contains(against),
        Test::Matches => regex::Regex::new(against).is_ok_and(|re| re.is_match(value)),
        Test::Above => matches!((number(value), number(against)), (Some(a), Some(b)) if a > b),
        Test::Below => matches!((number(value), number(against)), (Some(a), Some(b)) if a < b),
        Test::Empty => value_trimmed.is_empty(),
        Test::NotEmpty => !value_trimmed.is_empty(),
    }
}

/// The servers a pick names, in saved order.
pub fn pick(hosts: &[HostRef], picked: &HostPick) -> Vec<HostRef> {
    let tag = picked.tag.as_deref().map(str::trim).filter(|tag| !tag.is_empty()).map(str::to_lowercase);
    hosts
        .iter()
        .filter(|host| {
            picked.ids.contains(&host.id)
                || picked.names.iter().any(|name| name.trim().eq_ignore_ascii_case(host.name.trim()))
                || tag.as_ref().is_some_and(|tag| host.tags.iter().any(|t| t.to_lowercase() == *tag))
        })
        .cloned()
        .collect()
}

const AI_WROTE: &str = "An AI wrote part of this step during this run.";

/// Holds a step that acts on text an AI wrote in this run until the user
/// allows it; `shown` says what it would do.
async fn allow_ai_text(tools: &dyn Tools, ai_text: bool, shown: &str, host: Option<&HostRef>) -> Result<(), Finished> {
    if !ai_text {
        return Ok(());
    }
    tools.allow_command(shown, host, AI_WROTE).await.map_err(Finished::failed)
}

/// Why a command needs the user's OK before it runs, if it does.
fn needs_ok(ai_text: bool, shell: &ShellCommand) -> Option<&'static str> {
    if ai_text {
        Some(AI_WROTE)
    } else if shell.read_as_code {
        Some("A value in this command has characters a shell acts on, and the command hands it to another shell or program (like ssh, sh -c or eval) that could run it.")
    } else {
        None
    }
}

/// Why a step the AI wrote didn't run: it couldn't be kept in its scope.
fn unfenced(why: &str) -> String {
    format!("Not run: Somme can't keep this step inside what it may change, because {why}.")
}

/// Runs `command` on the servers `picked`; `fenced` is it inside its fence.
async fn run_command(
    picked: &HostPick,
    command: &str,
    fenced: Option<&str>,
    one_by_one: bool,
    tools: &dyn Tools,
    timeout: Duration,
    why: Option<&str>,
) -> Finished {
    let run_on = |host: &HostRef| {
        let host = host.clone();
        async move {
            match fenced {
                Some(fenced) => tools.run_fenced_on_host(&host, command, fenced, timeout).await,
                None => tools.run_on_host(&host, command, timeout).await,
            }
        }
    };
    let hosts = match tools.hosts().await {
        Ok(hosts) => pick(&hosts, picked),
        Err(err) => return Finished::failed(err),
    };
    if hosts.is_empty() {
        return Finished::failed("No saved server matches this step's servers.");
    }
    // Asked once for all of them.
    if let Some(why) = why
        && let Err(err) = tools.allow_command(command, hosts.first(), why).await
    {
        return Finished::failed(err);
    }
    if let [host] = hosts.as_slice() {
        return match run_on(host).await {
            Ok(output) => from_output(output, Some(host)),
            Err(err) => Finished { result: json!({ "error": err, "host": host.name }), ..Finished::failed(err) },
        };
    }
    let mut results = Vec::new();
    if one_by_one {
        for host in &hosts {
            let result = run_on(host).await;
            let failed = !result.as_ref().is_ok_and(Output::ok);
            results.push((host, result));
            if failed {
                break;
            }
        }
    } else {
        let runs = hosts.iter().map(run_on);
        results = hosts.iter().zip(futures::future::join_all(runs).await).collect();
    }
    let mut output = String::new();
    let mut each = Vec::new();
    let mut failed = Vec::new();
    for (host, result) in &results {
        let (entry, text, ok) = match result {
            Ok(out) => (
                json!({ "host": host.name, "exit_code": out.exit_code, "stdout": out.stdout, "stderr": out.stderr, "timed_out": out.timed_out }),
                joined(out),
                out.ok(),
            ),
            Err(err) => (json!({ "host": host.name, "error": err }), err.clone(), false),
        };
        output.push_str(&format!("── {} ──\n{}\n", host.name, text.trim_end()));
        if !ok {
            failed.push(host.name.clone());
        }
        each.push(entry);
    }
    let skipped = hosts.len() - results.len();
    let result = json!({ "output": output, "hosts": each, "failed": failed, "skipped": skipped });
    if failed.is_empty() {
        Finished::ok(result)
    } else {
        let error = format!("Failed on {}.", failed.join(", "));
        let mut result = result;
        result["error"] = json!(error);
        Finished { port: FAILED.into(), result, error: Some(error) }
    }
}

/// Standard output, then standard error, as a terminal shows them.
fn joined(output: &Output) -> String {
    match (output.stdout.is_empty(), output.stderr.is_empty()) {
        (_, true) => output.stdout.clone(),
        (true, false) => output.stderr.clone(),
        (false, false) => format!("{}\n{}", output.stdout.trim_end(), output.stderr),
    }
}

fn from_output(output: Output, host: Option<&HostRef>) -> Finished {
    let text = joined(&output);
    let mut result = json!({
        "output": text,
        "stdout": output.stdout,
        "stderr": output.stderr,
        "exit_code": output.exit_code,
        "timed_out": output.timed_out,
    });
    if let Some(host) = host {
        result["host"] = json!(host.name);
    }
    // Output that is JSON can be read into, like `{{nodes.api.json.status}}`.
    if let Ok(parsed) = serde_json::from_str::<Value>(output.stdout.trim())
        && (parsed.is_object() || parsed.is_array())
    {
        result["json"] = parsed;
    }
    if output.ok() {
        return Finished::ok(result);
    }
    let error = if output.timed_out {
        "Ran out of time.".to_owned()
    } else {
        match output.exit_code {
            Some(code) => format!("Ended with exit code {code}."),
            None => "Stopped before it ended.".to_owned(),
        }
    };
    result["error"] = json!(error);
    Finished { port: FAILED.into(), result, error: Some(error) }
}

/// Runs a command on this computer with the user's shell, with `vars` set,
/// cut at `timeout`.
pub async fn run_local(command: &str, vars: &[(String, String)], dir: Option<&str>, timeout: Duration) -> Result<Output, String> {
    #[cfg(windows)]
    let mut child = {
        let mut c = tokio::process::Command::new("cmd");
        // `!name!` reads a variable after the line is read, so its value
        // never becomes part of the command.
        if !vars.is_empty() {
            c.arg("/V:ON");
        }
        c.arg("/C").arg(command);
        c
    };
    #[cfg(not(windows))]
    let mut child = {
        let shell = std::env::var("SHELL").ok().filter(|shell| !shell.is_empty()).unwrap_or_else(|| "/bin/sh".into());
        let mut c = tokio::process::Command::new(shell);
        c.arg("-c").arg(command);
        c
    };
    child.envs(vars.iter().map(|(name, value)| (name, value)));
    if let Some(dir) = dir {
        child.current_dir(expand_home(dir));
    }
    let mut child = child
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("Could not start the command: {err}"))?;
    let (mut stdout, mut stderr) = (child.stdout.take(), child.stderr.take());
    let read = async {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let read_out = async {
            if let Some(pipe) = stdout.as_mut() {
                let _ = pipe.take(OUTPUT_LIMIT as u64).read_to_end(&mut out).await;
            }
        };
        let read_err = async {
            if let Some(pipe) = stderr.as_mut() {
                let _ = pipe.take(OUTPUT_LIMIT as u64).read_to_end(&mut err).await;
            }
        };
        tokio::join!(read_out, read_err);
        let status = child.wait().await;
        (out, err, status)
    };
    match tokio::time::timeout(timeout, read).await {
        Ok((out, err, status)) => {
            let status = status.map_err(|err| format!("The command failed: {err}"))?;
            Ok(Output {
                exit_code: status.code(),
                stdout: String::from_utf8_lossy(&out).into_owned(),
                stderr: String::from_utf8_lossy(&err).into_owned(),
                timed_out: false,
            })
        }
        // Dropping the child kills it.
        Err(_) => Ok(Output { exit_code: None, stdout: String::new(), stderr: String::new(), timed_out: true }),
    }
}

/// `~` as the home folder.
pub fn expand_home(dir: &str) -> std::path::PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    match (dir.strip_prefix("~"), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') => {
            std::path::Path::new(&home).join(rest.trim_start_matches(['/', '\\']))
        }
        _ => dir.into(),
    }
}

/// Text for a result, for logs and the canvas.
pub fn summary(finished: &Finished) -> String {
    if let Some(error) = &finished.error {
        return error.clone();
    }
    finished.result.get("output").map(as_text).unwrap_or_else(|| finished.port.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_compare_as_asked() {
        assert!(check(Test::Equals, " 0\n", "0"));
        assert!(check(Test::Above, "93%", "90"));
        assert!(!check(Test::Below, "abc", "3"));
        assert!(check(Test::Matches, "/dev/sda1  95%", r"9[0-9]%"));
        assert!(check(Test::Empty, " \n", ""));
        assert!(check(Test::NotContains, "all good", "error"));
    }

    #[test]
    fn picks_by_id_name_or_tag() {
        let hosts = vec![
            HostRef { id: "1".into(), name: "web-1".into(), address: String::new(), tags: vec!["Web".into()] },
            HostRef { id: "2".into(), name: "db".into(), address: String::new(), tags: vec![] },
            HostRef { id: "3".into(), name: "web-2".into(), address: String::new(), tags: vec!["web".into()] },
        ];
        let by_tag = HostPick { tag: Some("web".into()), ..HostPick::default() };
        assert_eq!(pick(&hosts, &by_tag).iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["1", "3"]);
        let by_name = HostPick { names: vec!["DB".into()], ..HostPick::default() };
        assert_eq!(pick(&hosts, &by_name)[0].id, "2");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_commands_run_and_stop_on_time() {
        let out = run_local("echo hi; echo oops >&2; exit 3", &[], None, Duration::from_secs(10)).await.unwrap();
        assert_eq!((out.stdout.as_str(), out.stderr.as_str(), out.exit_code), ("hi\n", "oops\n", Some(3)));
        let slow = run_local("sleep 5", &[], None, Duration::from_millis(200)).await.unwrap();
        assert!(slow.timed_out);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn values_in_commands_never_run() {
        struct Here;
        #[async_trait]
        impl Tools for Here {
            async fn hosts(&self) -> Result<Vec<HostRef>, String> {
                Ok(Vec::new())
            }
            async fn run_on_host(&self, _: &HostRef, _: &str, _: Duration) -> Result<Output, String> {
                Err("no servers".into())
            }
            async fn notify(&self, _: &str, _: &str) {}
        }
        let dir = tempfile::tempdir().unwrap();
        let bad = r#"x'"; touch pwned; $(touch pwned2) `touch pwned3` '"#;
        let context = json!({ "input": { "v": bad } });
        let step = NodeKind::Local {
            command: "printf '%s|' {{input.v}} \"in {{input.v}}\" 'in {{input.v}}'\ncat <<EOF\n{{input.v}}\nEOF".into(),
            dir: Some(dir.path().display().to_string()),
        };
        let done = run(&step, &context, &Here, Duration::from_secs(10), false, &|_| {}, None).await;
        assert_eq!(done.result["stdout"], json!(format!("{bad}|in {bad}|in {bad}|{bad}\n")), "{done:?}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "a value ran as code");
        // A `~` at the start of a value is still the home folder.
        let home = json!({ "input": { "d": "~/somme-tilde-check" } });
        let step = NodeKind::Local { command: "printf %s {{input.d}}".into(), dir: None };
        let done = run(&step, &home, &Here, Duration::from_secs(10), false, &|_| {}, None).await;
        assert_eq!(done.result["stdout"], json!(format!("{}/somme-tilde-check", std::env::var("HOME").unwrap())), "{done:?}");
        // A second shell would read it as code again, so it waits for an OK
        // that no one here can give.
        let step = NodeKind::Local { command: "sh -c 'echo {{input.v}}'".into(), dir: Some(dir.path().display().to_string()) };
        let done = run(&step, &context, &Here, Duration::from_secs(10), false, &|_| {}, None).await;
        assert!(done.is_failure() && done.error.as_deref().is_some_and(|error| error.contains("another shell")), "{done:?}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "a value ran as code");
    }
}
