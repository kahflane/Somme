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

use crate::fence::Fence;
use crate::nodes::{self, Finished, HIDDEN, Tools};
use crate::schema::{Automation, FAILED, NodeKind, problems, runs_ai_text, vault_names};

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
                        let (tools, context, cancel, scope) = (tools.clone(), context.clone(), cancel.clone(), automation.scope.clone());
                        running.push(async move {
                            let finished = run_node(&node, &context, tools.as_ref(), on_event, &cancel, ai_text, &scope).await;
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

/// A report-only step's preview, run in its command's place. It must only
/// look; otherwise nothing runs, and the step says what it would have run.
fn report_kind(node: &crate::schema::Node) -> Result<crate::schema::NodeKind, String> {
    let command = node.kind.command().unwrap_or_default().trim();
    let preview = node.preview.trim();
    if preview.is_empty() {
        return Err(format!("Report only: it changed nothing. It would run:\n{command}"));
    }
    if let Err(why) = crate::readonly::check(preview) {
        return Err(format!("Report only: it changed nothing. Its preview was not run ({why}). It would run:\n{command}"));
    }
    Ok(with_command(&node.kind, preview))
}

/// `kind` running `text` in place of its command, where it would run, and
/// without sudo.
fn with_command(kind: &NodeKind, text: &str) -> NodeKind {
    let mut kind = kind.clone();
    match &mut kind {
        NodeKind::Command { command, sudo, .. } => {
            *command = text.to_owned();
            *sudo = false;
        }
        NodeKind::Local { command, .. } => *command = text.to_owned(),
        _ => {}
    }
    kind
}

/// How long a step's check may take.
const CHECK_LIMIT: Duration = Duration::from_secs(60);

/// Runs `node`'s check after it did its work. A failed check fails the
/// step: it ran, but didn't do its job.
async fn check_after(
    node: &crate::schema::Node,
    finished: Finished,
    context: &Value,
    tools: &dyn Tools,
    ai_text: bool,
    note: &(dyn Fn(String) + Send + Sync),
) -> Finished {
    let check = node.check.trim();
    if check.is_empty() || finished.is_failure() || node.kind.command().is_none() {
        return finished;
    }
    // A check the AI wrote must only look; it is never fenced.
    if node.by_ai
        && let Err(why) = crate::readonly::check(check)
    {
        note(format!("Its check was not run: it doesn't only look ({why})."));
        return finished;
    }
    let checked = nodes::run(&with_command(&node.kind, check), context, tools, CHECK_LIMIT, ai_text, note, None).await;
    let mut finished = finished;
    finished.result["check"] = serde_json::json!({ "command": check, "passed": !checked.is_failure(), "output": checked.result["output"] });
    if checked.is_failure() {
        let why = checked.error.unwrap_or_else(|| "it failed".into());
        let error = format!("It ran, but its check `{check}` failed: {why}");
        finished.result["error"] = serde_json::json!(error);
        finished.port = FAILED.into();
        finished.error = Some(error);
    } else {
        note(format!("Its check passed: {check}"));
    }
    finished
}

/// One step, with its tries and its time limit.
async fn run_node(
    node: &crate::schema::Node,
    context: &Value,
    tools: &dyn Tools,
    on_event: &(dyn Fn(Event) + Send + Sync),
    cancel: &CancellationToken,
    ai_text: bool,
    scope: &crate::schema::Scope,
) -> Finished {
    let limit = Duration::from_secs(node.timeout_seconds.unwrap_or_else(|| node.kind.default_timeout()).max(1));
    // A step that only reports runs its preview in place of its command, or
    // nothing at all.
    let reporting = node.mode == crate::schema::Mode::Report && node.kind.command().is_some();
    let kind = if reporting {
        match report_kind(node) {
            Ok(kind) => kind,
            Err(said) => {
                on_event(Event::NodeStarted { node: node.id.clone(), at: now_ms(), attempt: 1 });
                return Finished::ok(serde_json::json!({ "output": said, "report": true }));
            }
        }
    } else {
        node.kind.clone()
    };
    // A step the AI wrote stays in the scope the user approved.
    let fence = if reporting { None } else { Fence::for_step(node, scope) };
    let sandboxed = fence.is_some() && matches!(node.kind, NodeKind::Local { .. }) && crate::fence::can_sandbox();
    let mut attempt = 0;
    loop {
        attempt += 1;
        on_event(Event::NodeStarted { node: node.id.clone(), at: now_ms(), attempt });
        let note = |text: String| on_event(Event::NodeNote { node: node.id.clone(), at: now_ms(), text });
        // The step gets the limit itself, and a little more before it is cut off.
        let tried = tokio::time::timeout(limit + Duration::from_secs(5), nodes::run(&kind, context, tools, limit, ai_text, &note, fence.as_ref())).await;
        let mut finished = tried.unwrap_or_else(|_| Finished::failed(format!("Ran out of time ({} s).", limit.as_secs())));
        // An agent says itself what the fence stopped; its answer isn't read for it.
        if fence.is_some()
            && node.kind.command().is_some()
            && finished.is_failure()
            && let Some(why) = crate::fence::stopped(finished.result["output"].as_str().unwrap_or_default(), sandboxed)
        {
            finished.result["error"] = serde_json::json!(why);
            finished.error = Some(why);
        }
        if !reporting {
            finished = check_after(node, finished, context, tools, ai_text, &note).await;
        }
        if reporting {
            let shown = finished.result["output"].as_str().unwrap_or_default().to_owned();
            finished.result["output"] = serde_json::json!(format!("Report only: it changed nothing. Its preview shows what it would touch:\n{shown}"));
            finished.result["report"] = serde_json::json!(true);
        }
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
        /// What the user answers when an agent wants to change something,
        /// in turn; when none are left, `ask` answers.
        approvals: Mutex<Vec<crate::nodes::Approved>>,
        /// No one is there to OK anything.
        nobody: bool,
    }

    /// The bin the tests' fences use, never the real one.
    fn test_bin() -> String {
        std::env::temp_dir().join(format!("somme-engine-test-bin-{}", std::process::id())).display().to_string()
    }

    #[async_trait]
    impl Tools for Fake {
        fn bin_root(&self) -> Option<String> {
            Some(test_bin())
        }

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

        async fn run_fenced_on_host(&self, host: &HostRef, shown: &str, fenced: &str, timeout: Duration) -> Result<Output, String> {
            // A look runs as it is, with git kept from running programs.
            if fenced.contains("GIT_CONFIG_PARAMETERS") {
                assert!(fenced.starts_with("/bin/sh -c ") && fenced.contains(shown.split_whitespace().next().unwrap_or_default()), "{fenced}");
                return self.run_on_host(host, shown, timeout).await;
            }
            assert!(fenced.starts_with("/bin/sh -c ") && fenced.contains("somme-fence-paths") && fenced.contains(shown), "{fenced}");
            // The fence stops what is outside its scope: here, anything in Documents.
            if shown.contains("Documents") && !fenced.contains("/\\*\\*") && !fenced.contains("'/*'") {
                self.ran.lock().unwrap().push(format!("{}: stopped {shown}", host.name));
                return Ok(Output { exit_code: Some(126), stderr: format!("Somme stopped `{shown}`: /home/u/Documents/x is outside what this automation may change (its scope).\n"), ..Output::default() });
            }
            self.run_on_host(host, &format!("fenced {shown}"), timeout).await
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

        async fn approve(&self, approval: &crate::nodes::Approval, timeout: Duration) -> Result<crate::nodes::Approved, String> {
            let next = {
                let mut approvals = self.approvals.lock().unwrap();
                (!approvals.is_empty()).then(|| approvals.remove(0))
            };
            if self.nobody {
                return Err("Somme was closed.".into());
            }
            match next {
                Some(answer) => {
                    self.notes.lock().unwrap().push(format!("approve {}: {}", approval.detail, approval.why));
                    Ok(answer)
                }
                None => {
                    let message = if approval.place.is_empty() { approval.detail.clone() } else { format!("On {}:\n{}", approval.place, approval.detail) };
                    self.ask(&approval.title, &message, timeout).await.map(|yes| if yes { crate::nodes::Approved::Once } else { crate::nodes::Approved::No })
                }
            }
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
            ..somme_ai::llm::ChatReply::default()
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
        // On its own, it changes only what its scope covers without asking.
        let mut automation = flow(vec![agent("Make {{input.who}} faster", false, 10), notify("tell", "{{nodes.fix.answer}}")], vec![Edge::new("fix", "tell")]);
        automation.scope = crate::schema::Scope { paths: vec![format!("{}/**", dir.path().display())], ..Default::default() };
        automation.scope_ok = automation.scope.key();
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
        // The file was copied to the bin before it changed.
        assert_eq!(notes, [
            "Looking first.",
            "Runs on web-1: uptime",
            &format!("Reads on this computer: {path}"),
            &format!("Changes on this computer: {path}"),
            "What it deleted or changed is kept in the Somme bin for 7 days.",
        ]);
        let Some(Event::NodeFinished { finished, .. }) = events.iter().find(|event| matches!(event, Event::NodeFinished { node, .. } if node == "fix")) else { panic!() };
        assert_eq!(finished.result["steps"], 3);
        assert_eq!(finished.result["tokens"], 330);
        assert_eq!(finished.result["changed"], json!([path]));
    }

    #[tokio::test]
    async fn an_agent_looks_at_once_asks_before_changes_and_keeps_its_talk() {
        use crate::agent::{self, Job, Session};
        use crate::nodes::Approved;
        let fake = Arc::new(Fake {
            turns: Mutex::new(vec![
                turn("Looking.", &[("exec_command", json!({ "cmd": "df -h /" })), ("exec_command", json!({ "cmd": "du -sh /var/log" }))]),
                turn("Cleaning the old logs.", &[("exec_command", json!({ "cmd": "rm -rf /var/log/old" })), ("exec_command", json!({ "cmd": "rm -rf /var/log/older" }))]),
                turn("Freed 2 GB.", &[]),
                turn("", &[("exec_command", json!({ "cmd": "rm -rf /var/log/today" }))]),
                turn("I left today's logs.", &[]),
            ]),
            approvals: Mutex::new(vec![Approved::Rest, Approved::No]),
            ..Fake::default()
        });
        let hosts = fake.hosts().await.unwrap()[..1].to_vec();
        let job = Job { hosts, ask_first: true, max_steps: 20, max_tokens: 100_000, timeout: Duration::from_secs(60), fence: Some(crate::fence::Fence::keep("chat", true)), ..Job::default() };
        let events = Mutex::new(Vec::new());
        let note = |event: agent::Event| events.lock().unwrap().push(event);
        let mut session = Session { messages: vec![somme_ai::llm::Message::system("rules"), somme_ai::llm::Message::user("free some space")], allow_all: false };
        let done = agent::resume(&job, &mut session, fake.as_ref(), &note).await;
        assert_eq!(done.result["answer"], "Freed 2 GB.");
        let mut ran = fake.ran.lock().unwrap().clone();
        ran[..2].sort();
        // Looks run as they are; changes run in the fence, and "the rest" lets the second one through.
        assert_eq!(ran, ["web-1: df -h /", "web-1: du -sh /var/log", "web-1: fenced rm -rf /var/log/old", "web-1: fenced rm -rf /var/log/older"]);
        assert_eq!(*fake.notes.lock().unwrap(), ["approve rm -rf /var/log/old: Cleaning the old logs."]);
        let events = events.lock().unwrap().clone();
        assert!(matches!(&events[..3], [agent::Event::Said(_), agent::Event::Doing { .. }, agent::Event::Doing { .. }]), "{events:?}");
        // The next message carries on the same talk, and asks again.
        session.messages.push(somme_ai::llm::Message::user("and today's too"));
        let done = agent::resume(&job, &mut session, fake.as_ref(), &note).await;
        assert_eq!(done.result["answer"], "I left today's logs.");
        assert_eq!(fake.ran.lock().unwrap().len(), 4);
        assert_eq!(fake.notes.lock().unwrap().len(), 2);
        let sent = fake.sent.lock().unwrap().clone();
        assert_eq!((sent.last().unwrap().messages.len(), session.messages.len()), (12, 13));
        assert!(sent.iter().all(|request| request.cache && request.messages[0].plain_text() == "rules"));
    }

    #[tokio::test]
    async fn an_agent_out_of_tool_calls_still_says_what_it_did() {
        let fake = Arc::new(Fake {
            turns: Mutex::new(vec![
                turn("", &[("exec_command", json!({ "cmd": "df -h", "host": "web-1" })), ("exec_command", json!({ "cmd": "uptime", "host": "web-1" }))]),
                turn("The disk is 91% full; I found nothing safe to remove yet.", &[]),
            ]),
            ..Fake::default()
        });
        let (status, events) = go(&flow(vec![agent("Free space", true, 2)], vec![]), fake.clone()).await;
        assert_eq!(status, RunStatus::Failed);
        let Some(Event::NodeFinished { finished, .. }) = events.iter().find(|event| matches!(event, Event::NodeFinished { node, .. } if node == "fix")) else { panic!() };
        assert_eq!(finished.result["answer"], "The disk is 91% full; I found nothing safe to remove yet.");
        assert!(finished.error.as_deref().unwrap().contains("Used all its 2 tool calls"));
        // Its last turn had no tools to call.
        assert!(fake.sent.lock().unwrap().last().unwrap().tools.is_empty());
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
    async fn a_report_only_step_runs_its_preview_and_changes_nothing() {
        let fake = Arc::new(Fake::default());
        let mut clean = command("clean", "rm -rf /srv/cache/*");
        clean.mode = crate::schema::Mode::Report;
        clean.preview = "du -sh /srv/cache/*".into();
        let mut risky = command("risky", "rm -rf /srv/old");
        risky.mode = crate::schema::Mode::Report;
        risky.preview = "rm -rf /srv/old".into();
        let (status, events) = go(&flow(vec![clean, risky], vec![Edge::new("clean", "risky")]), fake.clone()).await;
        assert_eq!(status, RunStatus::Ok);
        // Only the preview ran; a preview that changes things doesn't run.
        assert_eq!(*fake.ran.lock().unwrap(), ["web-1: du -sh /srv/cache/*"]);
        assert!(format!("{events:?}").contains("Its preview was not run"), "{events:?}");
    }

    fn scoped(nodes: Vec<Node>, scope: crate::schema::Scope) -> Automation {
        let mut automation = flow(nodes, vec![]);
        automation.scope = scope;
        automation.scope_ok = automation.scope.key();
        automation
    }

    #[tokio::test]
    async fn an_agent_works_inside_its_scope_and_keeps_what_it_wanted_past_it() {
        let fake = Arc::new(Fake {
            turns: Mutex::new(vec![
                turn("", &[
                    ("exec_command", json!({ "cmd": "rm -rf ~/Projects/a/node_modules", "host": "web-1" })),
                    ("exec_command", json!({ "cmd": "rm -rf ~/Documents/x", "host": "web-1" })),
                ]),
                turn("Removed one; the other needs your OK.", &[]),
            ]),
            nobody: true,
            ..Fake::default()
        });
        let automation = scoped(vec![agent("Free space", false, 10)], crate::schema::Scope { paths: vec!["~/Projects/**/node_modules".into()], delete: true, ..Default::default() });
        let (status, events) = go(&automation, fake.clone()).await;
        assert_eq!(status, RunStatus::Ok, "{events:?}");
        // Nothing asked inside the scope; the change past it was kept for later.
        assert_eq!(*fake.ran.lock().unwrap(), ["web-1: fenced rm -rf ~/Projects/a/node_modules", "web-1: stopped rm -rf ~/Documents/x"]);
        let Some(Event::NodeFinished { finished, .. }) = events.iter().find(|event| matches!(event, Event::NodeFinished { node, .. } if node == "fix")) else { panic!() };
        assert_eq!(finished.result["wanted"][0]["widen"], json!({ "kind": "path", "value": "/home/u/Documents/x" }));
        assert_eq!(finished.result["wanted"][0]["command"], "rm -rf ~/Documents/x");
    }

    #[tokio::test]
    async fn a_step_the_ai_wrote_runs_fenced_and_its_check_runs_after() {
        let fake = Arc::new(Fake::default());
        let mut clean = command("clean", "rm -rf /srv/cache/old");
        clean.by_ai = true;
        clean.check = "test ! -e /srv/cache/old".into();
        let scope = crate::schema::Scope { paths: vec!["/srv/cache".into()], delete: true, ..Default::default() };
        let (status, events) = go(&scoped(vec![clean], scope), fake.clone()).await;
        assert_eq!(status, RunStatus::Ok, "{events:?}");
        assert_eq!(*fake.ran.lock().unwrap(), ["web-1: fenced rm -rf /srv/cache/old", "web-1: test ! -e /srv/cache/old"]);
        assert!(format!("{events:?}").contains("Its check passed"));
    }

    #[tokio::test]
    async fn a_failed_check_fails_the_step() {
        let fake = Arc::new(Fake::default());
        let mut restart = command("restart", "systemctl restart app");
        restart.check = "curl -fsS localhost/fail".into();
        let (status, events) = go(&flow(vec![restart], vec![]), fake.clone()).await;
        assert_eq!(status, RunStatus::Failed);
        assert!(format!("{events:?}").contains("It ran, but its check `curl -fsS localhost/fail` failed"), "{events:?}");
        // A check the AI wrote that changes things is never run.
        let fake = Arc::new(Fake::default());
        let mut restart = command("restart", "uptime");
        restart.by_ai = true;
        restart.check = "rm -rf /tmp/x".into();
        let (status, events) = go(&flow(vec![restart], vec![]), fake.clone()).await;
        assert_eq!(status, RunStatus::Ok);
        assert_eq!(*fake.ran.lock().unwrap(), ["web-1: uptime"]);
        assert!(format!("{events:?}").contains("Its check was not run"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_step_the_ai_wrote_on_this_computer_stays_in_its_scope() {
        // Outside the system's temporary folders, which any step may write.
        let exe = std::env::current_exe().unwrap();
        let dir = tempfile::Builder::new().prefix("engine-fence").tempdir_in(exe.ancestors().nth(2).unwrap()).unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("cache/old")).unwrap();
        std::fs::create_dir_all(root.join("keep")).unwrap();
        std::fs::write(root.join("keep/file"), "x").unwrap();
        let scope = crate::schema::Scope { paths: vec![root.join("cache").display().to_string()], delete: true, ..Default::default() };
        let local = |id: &str, text: String| {
            let mut node = Node::new(id, NodeKind::Local { command: text, dir: None });
            node.by_ai = true;
            // So nothing goes to the bin in the home folder.
            node.effects.rebuildable = true;
            node
        };
        let fake = Arc::new(Fake::default());
        let (status, events) = go(&scoped(vec![local("clean", format!("rm -rf {}/cache/old", root.display()))], scope.clone()), fake.clone()).await;
        assert_eq!(status, RunStatus::Ok, "{events:?}");
        assert!(!root.join("cache/old").exists());
        let (status, events) = go(&scoped(vec![local("wide", format!("rm -rf {}/keep/file", root.display()))], scope), fake).await;
        assert_eq!(status, RunStatus::Failed);
        assert!(format!("{events:?}").contains("is outside what this automation may change"), "{events:?}");
        assert!(root.join("keep/file").exists());
    }

    #[tokio::test]
    async fn an_agent_cut_off_mid_thought_is_told_to_go_on() {
        let mut cut = turn("Wait, or what if we", &[]);
        cut.cut_off = true;
        let fake = Arc::new(Fake { turns: Mutex::new(vec![cut, turn("All clear.", &[])]), ..Fake::default() });
        let (status, events) = go(&flow(vec![agent("Check the disk", false, 3)], vec![]), fake.clone()).await;
        assert_eq!(status, RunStatus::Ok, "{events:?}");
        let sent = fake.sent.lock().unwrap();
        let last = sent[1].messages.last().unwrap();
        assert_eq!(last.plain_text(), somme_ai::llm::GO_ON);
        assert!(format!("{events:?}").contains("All clear."));
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
