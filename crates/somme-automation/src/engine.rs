//! Runs a flow. A step starts once every line coming into it is settled and
//! at least one of them was taken; steps on a branch not taken are skipped.
//! Steps that are ready together run at the same time.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt as _;
use futures::stream::FuturesUnordered;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::nodes::{self, Finished, HIDDEN, Tools};
use crate::schema::{Automation, FAILED, problems, runs_ai_text, vault_names};

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Ok,
    Failed,
    Cancelled,
}

/// What happens during a run, for the log and the canvas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Started { at: i64 },
    NodeStarted { node: String, at: i64, attempt: u32 },
    NodeFinished { node: String, at: i64, finished: Finished },
    /// What a long step, like an Agent, is doing.
    NodeNote { node: String, at: i64, text: String },
    /// On a branch that was not taken.
    NodeSkipped { node: String },
    Finished { at: i64, status: RunStatus, error: Option<String> },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Line {
    Waiting,
    Taken,
    NotTaken,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Waiting,
    Running,
    Done,
    Skipped,
}

/// Runs `automation` to its end, or until `cancel`. `inputs` fill
/// `{{input.…}}`. Every event goes to `on_event` as it happens.
pub async fn run(
    automation: &Automation,
    inputs: BTreeMap<String, String>,
    tools: Arc<dyn Tools>,
    on_event: &(dyn Fn(Event) + Send + Sync),
    cancel: CancellationToken,
) -> RunStatus {
    // Vault values never reach the log: each event is cleaned first.
    let secrets = Hider::default();
    let on_event = &|event: Event| on_event(secrets.event(event));
    on_event(Event::Started { at: now_ms() });
    let finish = |status: RunStatus, error: Option<String>| {
        on_event(Event::Finished { at: now_ms(), status, error });
        status
    };
    if let Some(problem) = problems(automation).into_iter().next() {
        return finish(RunStatus::Failed, Some(problem));
    }
    let names: Vec<String> = vault_names(automation).into_iter().collect();
    let mut vault = serde_json::Map::new();
    if !names.is_empty() {
        let opened = match tools.vault(&names).await {
            Ok(opened) => opened,
            Err(err) => return finish(RunStatus::Failed, Some(err)),
        };
        if let Some(missing) = names.iter().find(|name| !opened.contains_key(*name)) {
            return finish(RunStatus::Failed, Some(format!("No vault value named \u{201c}{missing}\u{201d}. Add it under Automations \u{2192} Vault.")));
        }
        secrets.add(opened.values().cloned());
        vault.extend(opened.into_iter().map(|(name, value)| (name, json!(value))));
    }
    let mut input: serde_json::Map<String, Value> =
        automation.inputs.iter().map(|i| (i.name.clone(), json!(i.default))).collect();
    input.extend(inputs.into_iter().map(|(name, value)| (name, json!(value))));
    let mut context = json!({
        "input": input,
        "nodes": {},
        "automation": { "name": automation.name },
        "run": { "started": now_ms() },
        "vault": vault,
    });

    let nodes = &automation.nodes;
    let edges = &automation.edges;
    let incoming: Vec<Vec<usize>> =
        nodes.iter().map(|node| edges.iter().enumerate().filter(|(_, e)| e.to == node.id).map(|(ix, _)| ix).collect()).collect();
    let mut lines = vec![Line::Waiting; edges.len()];
    let mut steps = vec![Step::Waiting; nodes.len()];
    let mut running = FuturesUnordered::new();
    let mut failure: Option<String> = None;

    loop {
        // Settle what can be settled: start ready steps, skip dead ones.
        if failure.is_none() {
            let mut changed = true;
            while changed {
                changed = false;
                for ix in 0..nodes.len() {
                    if steps[ix] != Step::Waiting {
                        continue;
                    }
                    let coming = &incoming[ix];
                    if coming.iter().any(|&e| lines[e] == Line::Waiting) {
                        continue;
                    }
                    if coming.is_empty() || coming.iter().any(|&e| lines[e] == Line::Taken) {
                        steps[ix] = Step::Running;
                        let node = nodes[ix].clone();
                        let ai_text = runs_ai_text(automation, &node);
                        let (tools, context, cancel) = (tools.clone(), context.clone(), cancel.clone());
                        running.push(async move {
                            let finished = run_node(&node, &context, tools.as_ref(), on_event, &cancel, ai_text).await;
                            (ix, finished)
                        });
                    } else {
                        steps[ix] = Step::Skipped;
                        on_event(Event::NodeSkipped { node: nodes[ix].id.clone() });
                        for (e, edge) in edges.iter().enumerate() {
                            if edge.from == nodes[ix].id {
                                lines[e] = Line::NotTaken;
                            }
                        }
                        changed = true;
                    }
                }
            }
        }
        let next = tokio::select! {
            next = running.next() => next,
            () = cancel.cancelled() => return finish(RunStatus::Cancelled, None),
        };
        let Some((ix, finished)) = next else { break };
        steps[ix] = Step::Done;
        let node = &nodes[ix];
        // Later steps, and AI among them, see what the log sees.
        let finished = secrets.finished(finished);
        context["nodes"][&node.id] = finished.result.clone();
        let mut handled = false;
        for (e, edge) in edges.iter().enumerate() {
            if edge.from != node.id {
                continue;
            }
            let taken = edge.port_of(&node.kind) == finished.port;
            lines[e] = if taken { Line::Taken } else { Line::NotTaken };
            handled |= taken;
        }
        if finished.port == FAILED && !handled && failure.is_none() {
            // Nothing handles it, so the run stops. Steps already running end first.
            failure = Some(format!("{}: {}", node.title(), finished.error.clone().unwrap_or_else(|| "failed".into())));
        }
        on_event(Event::NodeFinished { node: node.id.clone(), at: now_ms(), finished });
    }
    match failure {
        Some(error) => finish(RunStatus::Failed, Some(error)),
        None => finish(RunStatus::Ok, None),
    }
}

/// Hides vault values in what a run shows.
#[derive(Default)]
struct Hider {
    values: std::sync::RwLock<Vec<String>>,
}

impl Hider {
    fn add(&self, values: impl IntoIterator<Item = String>) {
        let mut held = self.values.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Also as a command's sh quotes write them.
        held.extend(values.into_iter().filter(|value| !value.is_empty()).flat_map(|value| somme_ai::clean::with_quoted(&value)));
        // Longest first, so a value that holds another goes whole.
        held.sort_by_key(|value| std::cmp::Reverse(value.len()));
    }

    fn text(&self, text: &str) -> String {
        let held = self.values.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        held.iter().fold(text.to_owned(), |text, value| if text.contains(value.as_str()) { text.replace(value.as_str(), HIDDEN) } else { text })
    }

    fn value(&self, value: Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.text(&text)),
            Value::Array(items) => Value::Array(items.into_iter().map(|item| self.value(item)).collect()),
            Value::Object(fields) => Value::Object(fields.into_iter().map(|(name, field)| (name, self.value(field))).collect()),
            other => other,
        }
    }

    fn finished(&self, finished: Finished) -> Finished {
        if self.values.read().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty() {
            return finished;
        }
        Finished { port: finished.port, result: self.value(finished.result), error: finished.error.map(|error| self.text(&error)) }
    }

    fn event(&self, event: Event) -> Event {
        match event {
            Event::NodeFinished { node, at, finished } => Event::NodeFinished { node, at, finished: self.finished(finished) },
            Event::NodeNote { node, at, text } => Event::NodeNote { node, at, text: self.text(&text) },
            Event::Finished { at, status, error } => Event::Finished { at, status, error: error.map(|error| self.text(&error)) },
            other => other,
        }
    }
}

/// One step, with its tries and its time limit.
async fn run_node(
    node: &crate::schema::Node,
    context: &Value,
    tools: &dyn Tools,
    on_event: &(dyn Fn(Event) + Send + Sync),
    cancel: &CancellationToken,
    ai_text: bool,
) -> Finished {
    let limit = Duration::from_secs(node.timeout_seconds.unwrap_or_else(|| node.kind.default_timeout()).max(1));
    let mut attempt = 0;
    loop {
        attempt += 1;
        on_event(Event::NodeStarted { node: node.id.clone(), at: now_ms(), attempt });
        let note = |text: String| on_event(Event::NodeNote { node: node.id.clone(), at: now_ms(), text });
        // The step gets the limit itself, and a little more before it is cut off.
        let tried = tokio::time::timeout(limit + Duration::from_secs(5), nodes::run(&node.kind, context, tools, limit, ai_text, &note)).await;
        let finished = tried.unwrap_or_else(|_| Finished::failed(format!("Ran out of time ({} s).", limit.as_secs())));
        if !finished.is_failure() || attempt > node.retries || cancel.is_cancelled() {
            return finished;
        }
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(node.retry_wait_seconds)) => {}
            () = cancel.cancelled() => return finished,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::nodes::{Decided, HostRef, Output, Written};
    use crate::schema::{Edge, HostPick, Node, NodeKind, Test};

    /// Servers that answer from a list: a command containing `fail` exits 1.
    #[derive(Default)]
    struct Fake {
        ran: Mutex<Vec<String>>,
        notes: Mutex<Vec<String>>,
        /// Fails this many times before it works.
        flaky: Mutex<u32>,
        /// The chance of yes Decide steps get.
        yes: f64,
        /// Lets commands with AI text run.
        allow: bool,
        /// What Ask me steps get.
        answer: bool,
        /// What the LLM says, turn by turn, to Agent steps.
        turns: Mutex<Vec<somme_ai::llm::ChatReply>>,
        /// What Agent steps sent the LLM.
        sent: Mutex<Vec<somme_ai::llm::ChatRequest>>,
        /// What Agent steps did on the screen.
        screen: Mutex<Vec<String>>,
        /// The user takes back control at a screen action that says this.
        stop_at: Option<&'static str>,
        stopped: std::sync::atomic::AtomicBool,
        /// What the guard was asked about.
        checked: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Tools for Fake {
        async fn hosts(&self) -> Result<Vec<HostRef>, String> {
            Ok(vec![
                HostRef { id: "1".into(), name: "web-1".into(), address: String::new(), tags: vec!["web".into()] },
                HostRef { id: "2".into(), name: "web-2".into(), address: String::new(), tags: vec!["web".into()] },
            ])
        }

        async fn run_on_host(&self, host: &HostRef, command: &str, _: Duration) -> Result<Output, String> {
            self.ran.lock().unwrap().push(format!("{}: {command}", host.name));
            if command.contains("sleep") {
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
            let mut flaky = self.flaky.lock().unwrap();
            let fails = command.contains("fail") || (command.contains("flaky") && *flaky > 0);
            if command.contains("flaky") && *flaky > 0 {
                *flaky -= 1;
            }
            Ok(Output { exit_code: Some(i32::from(fails)), stdout: format!("{} said {command}\n", host.name), ..Output::default() })
        }

        async fn notify(&self, title: &str, body: &str) {
            self.notes.lock().unwrap().push(format!("{title}: {body}"));
        }

        async fn decide(&self, _: &str, _: &str, _: f64, _: f64) -> Result<Decided, String> {
            Ok(Decided { yes: self.yes, by: "Jev".into() })
        }

        async fn write(&self, prompt: &str) -> Result<Written, String> {
            Ok(Written { text: format!("wrote {prompt}"), tokens: Some(7) })
        }

        async fn allow_command(&self, _: &str, _: Option<&HostRef>, _: &str) -> Result<(), String> {
            if self.allow { Ok(()) } else { Err("not allowed".into()) }
        }

        async fn ask(&self, title: &str, message: &str, _: Duration) -> Result<bool, String> {
            self.notes.lock().unwrap().push(format!("asked {title}: {message}"));
            Ok(self.answer)
        }

        async fn vault(&self, names: &[String]) -> Result<BTreeMap<String, String>, String> {
            Ok(names.iter().filter(|name| *name == "TOKEN").map(|name| (name.clone(), "s3cret-token".to_owned())).collect())
        }

        async fn agent_turn(&self, request: &somme_ai::llm::ChatRequest) -> Result<somme_ai::llm::ChatReply, String> {
            self.sent.lock().unwrap().push(request.clone());
            let mut turns = self.turns.lock().unwrap();
            if turns.is_empty() { Err("no more turns".into()) } else { Ok(turns.remove(0)) }
        }

        async fn check(&self, kind: &str, detail: &str, _: Option<&HostRef>) -> Result<(), String> {
            self.checked.lock().unwrap().push(format!("{kind}: {detail}"));
            if kind == "click" && detail.contains("Pay") { Err("The guard stopped it.".into()) } else { Ok(()) }
        }

        async fn computer_start(&self, apps: &[String]) -> Result<(), String> {
            self.screen.lock().unwrap().push(format!("start {}", apps.join(", ")));
            Ok(())
        }

        async fn computer(&self, action: &crate::computer::Action) -> Result<crate::computer::Seen, String> {
            let done = action.describe();
            self.screen.lock().unwrap().push(done.clone());
            if self.stop_at.is_some_and(|stop| done.contains(stop)) {
                self.stopped.store(true, std::sync::atomic::Ordering::SeqCst);
                return Err(crate::computer::STOPPED.into());
            }
            Ok(crate::computer::Seen { png: vec![1, 2, 3], width: 1280, height: 800, app: "Safari".into() })
        }

        async fn computer_end(&self) {
            self.screen.lock().unwrap().push("end".into());
        }

        fn computer_stopped(&self) -> bool {
            self.stopped.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    fn turn(text: &str, calls: &[(&str, serde_json::Value)]) -> somme_ai::llm::ChatReply {
        somme_ai::llm::ChatReply {
            text: text.into(),
            tool_calls: calls
                .iter()
                .enumerate()
                .map(|(ix, (name, args))| somme_ai::llm::ToolCall { id: format!("call{ix}"), name: (*name).into(), arguments: args.to_string() })
                .collect(),
            usage: somme_ai::llm::Usage { input_tokens: 100, output_tokens: 10 },
        }
    }

    fn agent(goal: &str, ask_first: bool, max_steps: u32) -> Node {
        Node::new(
            "fix",
            NodeKind::Agent {
                goal: goal.into(),
                hosts: HostPick { names: vec!["web-1".into()], ..HostPick::default() },
                local: true,
                ask_first,
                max_steps,
                max_tokens: 100_000,
                computer: false,
                apps: Vec::new(),
            },
        )
    }

    /// The tool results the LLM got in its last turn.
    fn results(fake: &Fake) -> Vec<String> {
        let sent = fake.sent.lock().unwrap();
        sent.last().unwrap().messages.iter().filter(|message| message.tool_call_id.is_some()).map(|message| message.plain_text()).collect()
    }

    fn command(id: &str, text: &str) -> Node {
        Node::new(id, NodeKind::Command { hosts: HostPick { names: vec!["web-1".into()], ..HostPick::default() }, command: text.into(), one_by_one: false, sudo: false })
    }

    fn notify(id: &str, body: &str) -> Node {
        Node::new(id, NodeKind::Notify { title: id.into(), body: body.into() })
    }

    async fn go(automation: &Automation, fake: Arc<Fake>) -> (RunStatus, Vec<Event>) {
        let events = Mutex::new(Vec::new());
        let status = run(automation, BTreeMap::from([("who".to_owned(), "ops".to_owned())]), fake, &|event| events.lock().unwrap().push(event), CancellationToken::new()).await;
        (status, events.into_inner().unwrap())
    }

    fn flow(nodes: Vec<Node>, edges: Vec<Edge>) -> Automation {
        Automation { nodes, edges, ..Automation::new("test") }
    }

    #[tokio::test]
    async fn steps_run_in_order_and_pass_values_on() {
        let fake = Arc::new(Fake::default());
        let automation = flow(
            vec![command("check", "uptime for {{input.who}}"), notify("tell", "{{nodes.check.output}}")],
            vec![Edge::new("check", "tell")],
        );
        let (status, events) = go(&automation, fake.clone()).await;
        assert_eq!(status, RunStatus::Ok);
        assert_eq!(*fake.ran.lock().unwrap(), ["web-1: SOMME_VALUE_1='ops'\nuptime for \"$SOMME_VALUE_1\""]);
        assert_eq!(*fake.notes.lock().unwrap(), ["tell: web-1 said SOMME_VALUE_1='ops'\nuptime for \"$SOMME_VALUE_1\"\n"]);
        assert!(matches!(events.last(), Some(Event::Finished { status: RunStatus::Ok, .. })));
    }

    #[tokio::test]
    async fn the_branch_not_taken_is_skipped_and_joins_still_run() {
        let fake = Arc::new(Fake::default());
        let check = Node::new("big", NodeKind::If { value: "{{nodes.check.exit_code}}".into(), test: Test::Equals, against: "0".into() });
        let automation = flow(
            vec![command("check", "df"), check, notify("good", "fine"), notify("bad", "full"), notify("after", "done")],
            vec![
                Edge::new("check", "big"),
                Edge::on("big", "yes", "good"),
                Edge::on("big", "no", "bad"),
                Edge::new("good", "after"),
                Edge::new("bad", "after"),
            ],
        );
        let (status, events) = go(&automation, fake.clone()).await;
        assert_eq!(status, RunStatus::Ok);
        assert_eq!(*fake.notes.lock().unwrap(), ["good: fine", "after: done"]);
        assert!(events.contains(&Event::NodeSkipped { node: "bad".into() }));
    }

    #[tokio::test]
    async fn an_unhandled_failure_stops_the_run_and_a_handled_one_does_not() {
        let fake = Arc::new(Fake::default());
        let mut automation = flow(vec![command("deploy", "fail now"), notify("next", "x")], vec![Edge::new("deploy", "next")]);
        let (status, events) = go(&automation, fake.clone()).await;
        assert_eq!(status, RunStatus::Failed);
        assert!(fake.notes.lock().unwrap().is_empty());
        assert!(matches!(events.last(), Some(Event::Finished { error: Some(error), .. }) if error.contains("exit code 1")));
        automation.nodes.push(notify("alarm", "{{nodes.deploy.error}}"));
        automation.edges.push(Edge::on("deploy", "failed", "alarm"));
        let (status, _) = go(&automation, fake.clone()).await;
        assert_eq!(status, RunStatus::Ok);
        assert_eq!(*fake.notes.lock().unwrap(), ["alarm: Ended with exit code 1."]);
    }

    #[test]
    fn vault_values_in_sh_quotes_are_hidden_too() {
        let hider = Hider::default();
        hider.add(["it's-s3cret".to_owned(), String::new()]);
        assert_eq!(hider.text("SOMME_VALUE_1='it'\\''s-s3cret'\nrun"), "SOMME_VALUE_1='[hidden]'\nrun");
        assert_eq!(hider.text("plain text"), "plain text");
    }

    #[tokio::test]
    async fn vault_values_are_used_but_never_shown() {
        let fake = Arc::new(Fake::default());
        let mut automation = flow(
            vec![command("push", "deploy --token {{vault.TOKEN}}"), notify("tell", "{{nodes.push.output}}")],
            vec![Edge::new("push", "tell")],
        );
        let (status, events) = go(&automation, fake.clone()).await;
        assert_eq!(status, RunStatus::Ok);
        assert_eq!(*fake.ran.lock().unwrap(), ["web-1: SOMME_VALUE_1='s3cret-token'\ndeploy --token \"$SOMME_VALUE_1\""]);
        assert_eq!(*fake.notes.lock().unwrap(), ["tell: web-1 said SOMME_VALUE_1='[hidden]'\ndeploy --token \"$SOMME_VALUE_1\"\n"]);
        assert!(!format!("{events:?}").contains("s3cret"));
        automation.nodes[0] = command("push", "deploy --token {{vault.NOPE}}");
        let (status, events) = go(&automation, fake.clone()).await;
        assert_eq!(status, RunStatus::Failed);
        assert!(matches!(events.last(), Some(Event::Finished { error: Some(error), .. }) if error.contains("NOPE")));
    }

    #[tokio::test]
    async fn ask_me_branches_on_the_answer() {
        let ask = Node::new("ok", NodeKind::Ask { title: "Restart {{input.who}}?".into(), message: String::new() });
        let automation = flow(
            vec![ask, notify("yes", "go"), notify("no", "stop")],
            vec![Edge::on("ok", "allowed", "yes"), Edge::on("ok", "denied", "no")],
        );
        let fake = Arc::new(Fake { answer: true, ..Fake::default() });
        assert_eq!(go(&automation, fake.clone()).await.0, RunStatus::Ok);
        assert_eq!(*fake.notes.lock().unwrap(), ["asked Restart ops?: ", "yes: go"]);
        let fake = Arc::new(Fake::default());
        go(&automation, fake.clone()).await;
        assert_eq!(fake.notes.lock().unwrap().last().unwrap(), "no: stop");
    }

    #[tokio::test]
    async fn http_steps_read_the_answer() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut asked = [0u8; 1024];
            let _ = socket.read(&mut asked).await;
            let body = r#"{"healthy":true}"#;
            let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            socket.write_all(reply.as_bytes()).await.unwrap();
        });
        let get = Node::new("get", NodeKind::Http { method: "GET".into(), url: format!("http://127.0.0.1:{port}/health"), headers: String::new(), body: String::new() });
        let check = Node::new("up", NodeKind::If { value: "{{nodes.get.json.healthy}}".into(), test: Test::Equals, against: "true".into() });
        let automation = flow(vec![get, check, notify("fine", "{{nodes.get.status}}")], vec![Edge::new("get", "up"), Edge::on("up", "yes", "fine")]);
        let fake = Arc::new(Fake::default());
        let (status, events) = go(&automation, fake.clone()).await;
        assert_eq!(status, RunStatus::Ok, "{events:?}");
        assert_eq!(*fake.notes.lock().unwrap(), ["fine: 200"]);
    }

    #[tokio::test]
    async fn an_agent_runs_commands_reads_and_patches_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("site.conf");
        std::fs::write(&file, "workers 1\nport 80\n").unwrap();
        let path = file.display().to_string();
        let patch = format!("*** Begin Patch\n*** Update File: {path}\n@@\n-workers 1\n+workers 4\n port 80\n*** End Patch");
        let fake = Arc::new(Fake {
            turns: Mutex::new(vec![
                turn("Looking first.", &[("exec_command", json!({ "cmd": "uptime", "host": "web-1" })), ("read_file", json!({ "path": path, "host": "local" }))]),
                turn("", &[("apply_patch", json!({ "input": patch, "host": "local" }))]),
                turn("It had 1 worker. Now it has 4.", &[]),
            ]),
            ..Fake::default()
        });
        let automation = flow(vec![agent("Make {{input.who}} faster", false, 10), notify("tell", "{{nodes.fix.answer}}")], vec![Edge::new("fix", "tell")]);
        let (status, events) = go(&automation, fake.clone()).await;
        assert_eq!(status, RunStatus::Ok, "{events:?}");
        assert_eq!(*fake.ran.lock().unwrap(), ["web-1: uptime"]);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "workers 4\nport 80\n");
        assert_eq!(*fake.notes.lock().unwrap(), ["tell: It had 1 worker. Now it has 4."]);
        let sent = fake.sent.lock().unwrap().clone();
        assert_eq!(sent[0].messages[1].plain_text(), "Make ops faster");
        assert!(sent[0].messages[0].plain_text().contains("- web-1"));
        let second: Vec<String> = sent[1].messages.iter().filter(|m| m.tool_call_id.is_some()).map(|m| m.plain_text()).collect();
        assert_eq!(second, ["Exit code: 0\nweb-1 said uptime\n", "     1\tworkers 1\n     2\tport 80\n"]);
        assert_eq!(results(&fake).last().unwrap(), &format!("Success. Updated the following files:\nM {path}"));
        let notes: Vec<&str> = events.iter().filter_map(|event| match event { Event::NodeNote { text, .. } => Some(text.as_str()), _ => None }).collect();
        assert_eq!(notes, ["Looking first.", "Runs on web-1: uptime", &format!("Reads on this computer: {path}"), &format!("Changes on this computer: {path}")]);
        let Some(Event::NodeFinished { finished, .. }) = events.iter().find(|event| matches!(event, Event::NodeFinished { node, .. } if node == "fix")) else { panic!() };
        assert_eq!(finished.result["steps"], 3);
        assert_eq!(finished.result["tokens"], 330);
        assert_eq!(finished.result["changed"], json!([path]));
    }

    #[tokio::test]
    async fn an_agent_asks_first_and_stops_at_its_limits() {
        let fake = Arc::new(Fake {
            turns: Mutex::new(vec![turn(
                "",
                &[("exec_command", json!({ "cmd": "systemctl restart nginx", "host": "web-1" })), ("exec_command", json!({ "cmd": "uptime" }))],
            )]),
            ..Fake::default()
        });
        let (status, events) = go(&flow(vec![agent("Restart nginx", true, 2)], vec![]), fake.clone()).await;
        assert_eq!(status, RunStatus::Failed);
        // The user said no, so nothing ran; with two places, it must say which.
        assert!(fake.ran.lock().unwrap().is_empty());
        assert_eq!(*fake.notes.lock().unwrap(), ["asked Let the agent run this?: On web-1:\nsystemctl restart nginx"]);
        let notes: Vec<&str> = events.iter().filter_map(|event| match event { Event::NodeNote { text, .. } => Some(text.as_str()), _ => None }).collect();
        assert_eq!(notes.len(), 4, "{notes:?}");
        assert!(notes[1].starts_with("Not done: the user said no."), "{notes:?}");
        assert_eq!(notes[2..], ["Runs: uptime", "Not done: say which host: web-1, local."]);
        assert!(matches!(events.last(), Some(Event::Finished { error: Some(error), .. }) if error.contains("Used all its 2 tool calls")), "{events:?}");
    }

    #[tokio::test]
    async fn an_agent_that_asks_first_asks_before_a_get_too() {
        let fake = Arc::new(Fake {
            turns: Mutex::new(vec![turn("", &[("http_request", json!({ "url": "https://example.test/?key=s3cret" }))])]),
            ..Fake::default()
        });
        go(&flow(vec![agent("Check the site", true, 1)], vec![]), fake.clone()).await;
        assert_eq!(*fake.notes.lock().unwrap(), ["asked Let the agent send this?: GET https://example.test/?key=s3cret"]);
    }

    fn screen_agent(apps: &[&str]) -> Node {
        let mut node = agent("Reply to Sam", false, 10);
        if let NodeKind::Agent { hosts, local, computer, apps: allowed, .. } = &mut node.kind {
            *hosts = HostPick::default();
            *local = false;
            *computer = true;
            *allowed = apps.iter().map(|app| (*app).to_owned()).collect();
        }
        node
    }

    #[tokio::test]
    async fn an_agent_uses_the_screen_in_its_apps_only() {
        let fake = Arc::new(Fake {
            turns: Mutex::new(vec![
                turn("", &[("computer", json!({ "action": "screenshot" }))]),
                turn(
                    "",
                    &[
                        ("computer", json!({ "action": "open_app", "name": "Terminal" })),
                        ("computer", json!({ "action": "click", "x": 40, "y": 60, "target": "Pay now" })),
                        ("computer", json!({ "action": "type", "text": "Thanks!" })),
                        ("exec_command", json!({ "cmd": "ls" })),
                    ],
                ),
                turn("", &[("computer", json!({ "action": "screenshot" }))]),
                turn("", &[("computer", json!({ "action": "screenshot" }))]),
                turn("", &[("computer", json!({ "action": "screenshot" }))]),
                turn("Replied.", &[]),
            ]),
            ..Fake::default()
        });
        assert!(crate::schema::problems(&flow(vec![screen_agent(&[])], vec![])).iter().any(|problem| problem.contains("names no apps")));
        let (status, events) = go(&flow(vec![screen_agent(&["Safari"])], vec![]), fake.clone()).await;
        assert_eq!(status, RunStatus::Ok, "{events:?}");
        // Terminal is not allowed and the guard stopped the Pay click; the
        // rest reached the screen, and control went back at the end.
        assert_eq!(
            *fake.screen.lock().unwrap(),
            ["start Safari", "Looks at the screen", "Types \u{201c}Thanks!\u{201d}", "Looks at the screen", "Looks at the screen", "Looks at the screen", "end"]
        );
        let sent = fake.sent.lock().unwrap();
        let second = &sent[1];
        assert!(second.tools.iter().any(|tool| tool.name == "computer"));
        // With nowhere to run commands, there are no command tools.
        assert!(!second.tools.iter().any(|tool| tool.name == "exec_command"));
        let last = second.messages.last().unwrap();
        assert!(last.parts.iter().any(|part| matches!(part, somme_ai::llm::Part::Image { .. })), "{last:?}");
        let results: Vec<String> = sent[2].messages.iter().filter(|message| message.tool_call_id.is_some()).map(|message| message.plain_text()).collect();
        assert!(results[1].contains("Terminal is not one of this step's apps"), "{results:?}");
        assert_eq!(results[2], "Not done: The guard stopped it.");
        assert!(fake.checked.lock().unwrap().contains(&"click: Pay now (in Safari)".to_owned()));
        assert!(results[4].contains("no tool called exec_command"), "{results:?}");
        // Only the last few screenshots go to the LLM.
        let shots = |request: &somme_ai::llm::ChatRequest| {
            request.messages.iter().flat_map(|message| &message.parts).filter(|part| matches!(part, somme_ai::llm::Part::Image { .. })).count()
        };
        assert_eq!(shots(&sent[5]), 3);
    }

    #[tokio::test]
    async fn the_user_takes_back_the_screen() {
        let fake = Arc::new(Fake {
            turns: Mutex::new(vec![turn("", &[("computer", json!({ "action": "key", "keys": "cmd+n" })), ("computer", json!({ "action": "screenshot" }))]), turn("More.", &[])]),
            stop_at: Some("cmd+n"),
            ..Fake::default()
        });
        let (status, events) = go(&flow(vec![screen_agent(&["Safari"])], vec![]), fake.clone()).await;
        assert_eq!(status, RunStatus::Failed);
        assert_eq!(*fake.screen.lock().unwrap(), ["start Safari", "Presses cmd+n", "end"]);
        assert_eq!(fake.sent.lock().unwrap().len(), 1);
        assert!(matches!(events.last(), Some(Event::Finished { error: Some(error), .. }) if error.contains(crate::computer::STOPPED)), "{events:?}");
    }

    #[tokio::test]
    async fn a_failing_step_tries_again() {
        let fake = Arc::new(Fake { flaky: Mutex::new(2), ..Fake::default() });
        let mut step = command("pull", "flaky pull");
        step.retries = 2;
        let (status, _) = go(&flow(vec![step], vec![]), fake.clone()).await;
        assert_eq!(status, RunStatus::Ok);
        assert_eq!(fake.ran.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn many_servers_report_each_one() {
        let fake = Arc::new(Fake::default());
        let mut step = command("all", "hostname");
        step.kind = NodeKind::Command { hosts: HostPick { tag: Some("web".into()), ..HostPick::default() }, command: "hostname".into(), one_by_one: false, sudo: false };
        let (status, events) = go(&flow(vec![step], vec![]), fake.clone()).await;
        assert_eq!(status, RunStatus::Ok);
        let result = events.iter().find_map(|e| match e {
            Event::NodeFinished { finished, .. } => Some(finished.result.clone()),
            _ => None,
        });
        assert_eq!(result.unwrap()["hosts"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn decide_branches_and_ai_text_needs_an_ok() {
        let decide = Node::new("bad", NodeKind::Decide { yes_no: "Bad?".into(), about: "x".into(), yes_from: 0.8, no_to: 0.2 });
        let write = Node::new("say", NodeKind::Write { prompt: "fix".into() });
        let automation = flow(
            vec![decide, write, notify("fine", "ok"), notify("ask", "?"), command("fix", "run {{nodes.say.text}}")],
            vec![
                Edge::on("bad", "yes", "say"),
                Edge::on("bad", "no", "fine"),
                Edge::on("bad", "unsure", "ask"),
                Edge::new("say", "fix"),
            ],
        );
        let fake = Arc::new(Fake { yes: 0.5, ..Fake::default() });
        go(&automation, fake.clone()).await;
        assert_eq!(*fake.notes.lock().unwrap(), ["ask: ?"]);

        let fake = Arc::new(Fake { yes: 0.95, ..Fake::default() });
        let (status, _) = go(&automation, fake.clone()).await;
        assert_eq!(status, RunStatus::Failed);
        assert!(fake.ran.lock().unwrap().is_empty(), "AI text ran without an OK");

        let fake = Arc::new(Fake { yes: 0.95, allow: true, ..Fake::default() });
        let (status, _) = go(&automation, fake.clone()).await;
        assert_eq!(status, RunStatus::Ok);
        assert_eq!(*fake.ran.lock().unwrap(), ["web-1: SOMME_VALUE_1='wrote fix'\nrun \"$SOMME_VALUE_1\""]);
    }

    #[tokio::test]
    async fn a_run_can_be_cancelled() {
        let automation = flow(vec![command("slow", "sleep")], vec![]);
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            stop.cancel();
        });
        let status = run(&automation, BTreeMap::new(), Arc::new(Fake::default()), &|_| {}, cancel).await;
        assert_eq!(status, RunStatus::Cancelled);
    }

    #[tokio::test]
    async fn a_broken_flow_does_not_start() {
        let automation = flow(vec![notify("a", "x")], vec![Edge::new("a", "missing")]);
        let (status, events) = go(&automation, Arc::new(Fake::default())).await;
        assert_eq!(status, RunStatus::Failed);
        assert!(!events.iter().any(|e| matches!(e, Event::NodeStarted { .. })));
    }
}
