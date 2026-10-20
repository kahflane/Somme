//! The step tools: what the assistant uses to put fixed steps on the canvas,
//! and to build an automation with `make_automatic`, and what each does to
//! the draft. Nothing here runs a step; `dry_run` only describes them. No
//! GPUI, so the app and tests use it alike.

use serde_json::{Value, json};
use somme_ai::llm::{Tool, ToolCall};
use somme_automation::schema::{self, Automation, Edge, Input, Mode, Node, NodeKind, Plan, Scope, free_id, problems};
use somme_automation::{dry, layout, readonly, templates};

/// Text kept from each snippet for the LLM.
const SNIPPET_TEXT: usize = 400;

/// What the LLM may know about this Somme, read before a turn.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// Saved servers: names and tags only.
    pub hosts: Vec<(String, Vec<String>)>,
    /// Saved snippets: name and command, with secrets hidden.
    pub snippets: Vec<(String, String)>,
    /// Decide steps may be added.
    pub decide: bool,
    /// Write steps may be added.
    pub write: bool,
    /// Agent steps may be added.
    pub agent: bool,
    /// Agent steps may use this computer's screen.
    pub screen: bool,
    /// The vault values' names. Their values never reach the LLM.
    pub vault: Vec<String>,
    /// This computer: its system, shell and home folder.
    pub computer: String,
}

/// What one tool call did.
#[derive(Debug, PartialEq)]
pub struct Applied {
    /// For the model.
    pub result: String,
    /// For the user, when the flow changed.
    pub change: Option<String>,
    /// Steps or lines came or went, so the flow is laid out again.
    pub reshaped: bool,
}

impl Applied {
    fn said(result: impl Into<String>) -> Self {
        Self { result: result.into(), change: None, reshaped: false }
    }

    fn changed(result: impl Into<String>, change: impl Into<String>, reshaped: bool) -> Self {
        Self { result: result.into(), change: Some(change.into()), reshaped }
    }
}

const STEPS: &str = r#"Steps, as JSON objects with a "type":
- command: runs on saved servers. {"type":"command","hosts":{"names":["web-1"]} or {"tag":"web"},"command":"df -h /","one_by_one":false,"sudo":false}. Ways out: ok, failed.
- local: runs on this computer. {"type":"local","command":"./backup.sh","dir":"~/app"}. Ways out: ok, failed.
- if: {"type":"if","value":"{{nodes.check.output}}","test":"contains","against":"error"}. test is one of equals, not_equals, contains, not_contains, matches (a regex), above, below, empty, not_empty. Ways out: yes, no.
- wait: {"type":"wait","seconds":10}. Way out: ok.
- notify: a system notification. {"type":"notify","title":"Backup","body":"{{nodes.backup.output}}"}. Way out: ok.
- copy: one file over SFTP. {"type":"copy","host":{"names":["db-1"]},"direction":"download","local":"~/Backups/db.sql.gz","remote":"/var/backups/db.sql.gz"}. direction is upload (to the server) or download. Ways out: ok, failed.
- ask: asks the user to Allow or Deny before going on. {"type":"ask","title":"Restart nginx on web-1?","message":"{{nodes.plan.text}}"}. Ways out: allowed, denied.
- http: {"type":"http","method":"GET","url":"https://example.com/health","headers":"Authorization: Bearer {{vault.API_TOKEN}}","body":""}. Fails on an answer from 400 up. Ways out: ok, failed. Later steps use {{nodes.<id>.status}}, {{nodes.<id>.body}} and {{nodes.<id>.json.<field>}}.
- tls: a server's certificate. {"type":"tls","host":"example.com","port":443}. Ways out: ok, failed. Later steps use {{nodes.<id>.days_left}}, {{nodes.<id>.expires}} and {{nodes.<id>.trusted}}.
- dns: {"type":"dns","name":"example.com","record":"MX"}. record is A, AAAA, CNAME, MX, TXT or NS. Ways out: ok, failed. Later steps use {{nodes.<id>.output}}, one record a line."#;

const DECIDE: &str = r#"
- decide: a yes or no question answered by AI. {"type":"decide","yes_no":"Is any disk over 90% full?","about":"{{nodes.check.output}}"}. Ways out: yes, no, unsure."#;

const WRITE: &str = r#"
- write: text the LLM writes when it runs. {"type":"write","prompt":"Say in 3 short lines what is wrong:\n{{nodes.check.output}}"}. Ways out: ok, failed. Later steps use {{nodes.<id>.text}}."#;

const AGENT: &str = r#"
- agent: an AI that works on its own toward a goal: it runs commands, reads files and changes them, on the servers it may use and, with "local":true, this computer. {"type":"agent","goal":"Find why the site is slow and tell me. Change nothing.","hosts":{"names":["web-1"]},"local":false,"ask_first":true,"max_steps":20}. ask_first makes it ask the user before each command or change; keep it true unless the user says otherwise. Use it when each run needs judgment that fixed steps can't give, like finding why something broke; give it a clear goal, what it may change, and when to stop. Ways out: ok, failed. Later steps use {{nodes.<id>.answer}}."#;

const SCREEN: &str = r#" An agent can also see and use this computer's screen with "computer":true and "apps":["Safari"], the apps it may click and type in. Use it only for work in apps that commands can't do."#;

const RULES: &str = r#"Values: {{nodes.<id>.output}}, {{nodes.<id>.exit_code}}, {{nodes.<id>.json.<field>}} for JSON output, {{input.<name>}}, and {{vault.<NAME>}} for a saved secret.
Never write a password, token or key into a step: use a vault value. Vault values are never sent to AI, so never put them in decide or write steps.
Somme fills in every {{...}} itself, so never write another tool's {{ }} template into a command: use its JSON output instead.
A step runs when a line into it is taken. A step that fails with no "failed" line stops the run, so add one to a notify step when the user wants to hear about failures.
Optional step fields: label (a short title), timeout_seconds, retries, retry_wait_seconds.
Use short ids like check_disk, and saved servers' names only.
To make it run on its own, set a schedule with set_start: five cron fields (minute hour day month weekday) like "0 9 * * 1-5", or @hourly or @daily.
Before a command that runs text a write or agent step made, add an ask step showing that text, so the user allows it first.
Nothing runs until the user presses Run, and commands you write are marked until the user checks them."#;

/// How steps are written, for the tool that adds them.
pub fn steps_text(facts: &Facts) -> String {
    let mut steps = STEPS.to_owned();
    if facts.decide {
        steps.push_str(DECIDE);
    }
    if facts.write {
        steps.push_str(WRITE);
    }
    if facts.agent {
        steps.push_str(AGENT);
        if facts.screen {
            steps.push_str(SCREEN);
        }
    }
    format!("{steps}\n\n{RULES}\n{}", vault_text(facts))
}

fn vault_text(facts: &Facts) -> String {
    if facts.vault.is_empty() {
        "No vault values are saved. If a step needs a secret, use {{vault.NAME}} and tell the user to add it under Vault values.".to_owned()
    } else {
        format!("Saved vault values: {}.", facts.vault.join(", "))
    }
}

/// The flow as JSON, without where steps sit or its id.
fn flow_text(automation: &Automation) -> String {
    let mut value = serde_json::to_value(automation).unwrap_or(Value::Null);
    if let Some(fields) = value.as_object_mut() {
        fields.remove("id");
        fields.remove("schema");
        fields.remove("scope_ok");
    }
    if let Some(nodes) = value["nodes"].as_array_mut() {
        for node in nodes.iter_mut().filter_map(Value::as_object_mut) {
            node.remove("at");
            node.remove("ai_wrote");
            node.remove("by_ai");
        }
    }
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

fn tool(name: &str, description: &str, parameters: Value) -> Tool {
    Tool { name: name.to_owned(), description: description.to_owned(), parameters }
}

pub fn tools() -> Vec<Tool> {
    vec![
        tool(
            "add_node",
            "Adds a step. With `after`, a line joins that step to the new one.",
            json!({
                "type": "object",
                "properties": {
                    "node": { "type": "object", "description": "The step, like {\"id\":\"check\",\"type\":\"command\",\"hosts\":{\"tag\":\"web\"},\"command\":\"df -h /\"}." },
                    "after": { "type": "string", "description": "Id of the step it follows." },
                    "port": { "type": "string", "description": "Which way out of `after`: ok, failed, yes, no, unsure, allowed or denied. Default: the first." }
                },
                "required": ["node"]
            }),
        ),
        tool(
            "update_node",
            "Changes fields of a step. Its type stays.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "changes": { "type": "object", "description": "Fields to set, like {\"command\":\"df -h\"} or {\"label\":\"Check disks\"}." }
                },
                "required": ["id", "changes"]
            }),
        ),
        tool(
            "connect",
            "Adds a line from one step's way out to another step.",
            json!({
                "type": "object",
                "properties": {
                    "from": { "type": "string" },
                    "to": { "type": "string" },
                    "port": { "type": "string", "description": "Which way out of `from`. Default: the first." }
                },
                "required": ["from", "to"]
            }),
        ),
        tool(
            "remove",
            "Removes a step with its lines, or with `from` and `to`, the line between them.",
            json!({
                "type": "object",
                "properties": { "id": { "type": "string" }, "from": { "type": "string" }, "to": { "type": "string" } }
            }),
        ),
        tool(
            "set_start",
            "Sets the automation's name, description, schedule, and the inputs asked for at each run.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "schedule": { "type": "string", "description": "Cron, like \"0 * * * *\" for every hour. Empty: only by hand." },
                    "missed": { "type": "string", "enum": ["run_once", "skip"], "description": "A run the computer slept through: run it once on waking, or skip it." },
                    "inputs": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": { "name": { "type": "string" }, "default": { "type": "string" }, "label": { "type": "string" } },
                            "required": ["name"]
                        }
                    }
                }
            }),
        ),
        tool(
            "look",
            "Runs a command that only looks (lists, sizes, reads, status) and shows what it printed. Changing, sending, sudo and secrets are refused.",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Like `uname -a`, `ls -la ~/some/folder` or `systemctl status nginx`." },
                    "on": { "type": "string", "description": "\"local\" for this computer, or a saved server's name. Default: local." }
                },
                "required": ["command"]
            }),
        ),
        tool(
            "write_plan",
            "Writes down what the automation is for, before or while building. Later checks grade the automation against it.",
            json!({
                "type": "object",
                "properties": {
                    "goal": { "type": "string" },
                    "done_when": { "type": "array", "items": { "type": "string" }, "description": "How to tell it worked, each a thing a command could check." },
                    "never": { "type": "array", "items": { "type": "string" }, "description": "What must never happen." },
                    "assumptions": { "type": "array", "items": { "type": "string" } },
                    "found": { "type": "array", "items": { "type": "string" }, "description": "What looking showed." }
                },
                "required": ["goal", "done_when", "never"]
            }),
        ),
        tool(
            "set_scope",
            "Sets what the automation may change while it runs. The user approves it, and runs are kept inside it.",
            json!({
                "type": "object",
                "properties": {
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Globs it may write or delete in, as narrow as can be, like ~/Work/**/build." },
                    "delete": { "type": "boolean", "description": "It deletes, inside `paths` only." },
                    "sends": { "type": "array", "items": { "type": "string" }, "description": "Hosts it sends data to." },
                    "services": { "type": "array", "items": { "type": "string" }, "description": "Services it may stop, start or restart." },
                    "sudo": { "type": "boolean" }
                }
            }),
        ),
        tool(
            "ask_user",
            "Asks the user one question and waits for the answer. Use it when the request is unclear, before building on a guess.",
            json!({
                "type": "object",
                "properties": {
                    "question": { "type": "string" },
                    "choices": { "type": "array", "items": { "type": "string" }, "description": "Two to four short answers. The user can also type their own." }
                },
                "required": ["question"]
            }),
        ),
        tool(
            "find_templates",
            "Finds the user's saved scripts and Somme's ready-made automations that match some words, to start from tried work.",
            json!({ "type": "object", "properties": { "words": { "type": "string" } }, "required": ["words"] }),
        ),
        tool(
            "read_skill",
            "Reads a skill: know-how for a kind of job, like what is safe to remove or how to deploy. With `file`, reads one of the files beside it.",
            json!({
                "type": "object",
                "properties": { "name": { "type": "string" }, "file": { "type": "string", "description": "A file the skill lists, like references/setup.md." } },
                "required": ["name"]
            }),
        ),
        tool("list_hosts", "Lists the saved servers: names and tags.", json!({ "type": "object", "properties": {} })),
        tool("list_snippets", "Lists the user's saved snippets: names and commands.", json!({ "type": "object", "properties": {} })),
        tool(
            "dry_run",
            "Shows every step in order with its values filled in, and anything that stops it from running. Runs nothing.",
            json!({ "type": "object", "properties": {} }),
        ),
    ]
}

/// Does what `call` asks to `automation`. An error is told to the model.
pub fn apply(automation: &mut Automation, call: &ToolCall, facts: &Facts) -> Applied {
    let args = call.args();
    let done = match call.name.as_str() {
        "add_node" => add_node(automation, &args, facts),
        "update_node" => update_node(automation, &args, facts),
        "connect" => connect(automation, &args),
        "remove" => remove(automation, &args),
        "set_start" => set_start(automation, &args),
        "list_hosts" => Ok(list_hosts(facts)),
        "list_snippets" => Ok(list_snippets(facts)),
        "dry_run" => Ok(dry_run(automation)),
        "write_plan" => write_plan(automation, &args),
        "set_scope" => set_scope(automation, &args),
        "find_templates" => Ok(find_templates(&args, facts)),
        other => Err(format!("There is no tool called {other}.")),
    };
    done.unwrap_or_else(|err| Applied::said(format!("Error: {err}")))
}

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty())
}

/// Whether the AI may add steps of `kind`.
fn may_add(kind: &NodeKind, facts: &Facts) -> Result<(), String> {
    match kind {
        NodeKind::Decide { .. } if !facts.decide => Err("Decide steps are off in this Somme. Use an if step.".into()),
        NodeKind::Write { .. } if !facts.write => Err("Write steps are off in this Somme.".into()),
        NodeKind::Agent { .. } if !facts.agent => Err("Agent steps are off in this Somme.".into()),
        NodeKind::Agent { computer: true, .. } if !facts.screen => Err("Agents may not use the screen in this Somme.".into()),
        _ => Ok(()),
    }
}

/// `given`'s fields over a blank step of its type.
fn read_node(given: &Value, id: &str) -> Result<Node, String> {
    let fields = given.as_object().ok_or("`node` must be an object.")?;
    let kind = fields.get("type").and_then(Value::as_str).ok_or("The step needs a \"type\".")?;
    let blank = schema::blank(kind).ok_or_else(|| format!("There is no \"{kind}\" step."))?;
    let mut value = serde_json::to_value(Node::new(id, blank)).map_err(|err| err.to_string())?;
    for (key, field) in fields {
        // Only the user marks a step checked or lets it make changes.
        if !matches!(key.as_str(), "id" | "at" | "ai_wrote" | "by_ai" | "mode") {
            value[key] = field.clone();
        }
    }
    serde_json::from_value(value).map_err(|err| format!("The step's fields don't fit a {kind} step: {err}"))
}

/// A step for the user: its type, then its name or id in quotes.
fn named(node: &Node) -> String {
    let name = if node.label.trim().is_empty() { node.id.as_str() } else { node.label.trim() };
    format!("{} \u{201c}{name}\u{201d}", node.kind.name())
}

/// Marks a step the AI set up that runs, changes or sends something. A
/// command that does more than look only reports what it would do, until the
/// user lets it: judged from the command itself, not from what the AI says.
fn mark(node: &mut Node) {
    node.ai_wrote = node.kind.acts() && node.kind.command().is_none_or(|command| !command.trim().is_empty());
    node.by_ai = node.kind.command().is_some_and(|command| !command.trim().is_empty());
    node.mode = match node.kind.command() {
        Some(command) if node.ai_wrote && readonly::check(command).is_err() => Mode::Report,
        _ => Mode::Do,
    };
}

fn add_node(automation: &mut Automation, args: &Value, facts: &Facts) -> Result<Applied, String> {
    let given = args.get("node").ok_or("Give the step as `node`.")?;
    let wanted = given.get("id").and_then(Value::as_str).unwrap_or_default();
    let kind = given.get("type").and_then(Value::as_str).unwrap_or("step");
    let id = free_id(automation, if wanted.trim().is_empty() { kind } else { wanted });
    let mut node = read_node(given, &id)?;
    may_add(&node.kind, facts)?;
    let after = text(args, "after");
    let edge = match after {
        Some(after) => {
            let from = automation.node(after).ok_or_else(|| format!("There is no step \"{after}\"."))?;
            let port = text(args, "port").unwrap_or(from.kind.ports()[0]);
            if !from.kind.ports().contains(&port) {
                return Err(format!("\"{after}\" has no \"{port}\" way out. It has: {}.", from.kind.ports().join(", ")));
            }
            Some(if port == from.kind.ports()[0] { Edge::new(after, id.clone()) } else { Edge::on(after, port, id.clone()) })
        }
        None => None,
    };
    mark(&mut node);
    node.at = layout::place_new(automation, after);
    let title = named(&node);
    automation.nodes.push(node);
    automation.edges.extend(edge);
    let renamed = if wanted.trim().is_empty() || wanted == id { String::new() } else { format!(" (\"{wanted}\" was taken)") };
    Ok(Applied::changed(format!("Added \"{id}\"{renamed}."), format!("Added {title}"), true))
}

fn update_node(automation: &mut Automation, args: &Value, facts: &Facts) -> Result<Applied, String> {
    let id = text(args, "id").ok_or("Say which step as `id`.")?;
    let changes = args.get("changes").and_then(Value::as_object).ok_or("Give `changes` as an object.")?;
    let ix = automation.nodes.iter().position(|node| node.id == id).ok_or_else(|| format!("There is no step \"{id}\"."))?;
    let old = &automation.nodes[ix];
    let mut value = serde_json::to_value(old).map_err(|err| err.to_string())?;
    let kind = value["type"].clone();
    if changes.get("type").is_some_and(|given| *given != kind) {
        return Err("A step's type can't change. Remove it and add a new one.".into());
    }
    for (key, field) in changes {
        if !matches!(key.as_str(), "id" | "at" | "ai_wrote" | "by_ai" | "type" | "mode") {
            value[key] = field.clone();
        }
    }
    let mut node: Node = serde_json::from_value(value).map_err(|err| format!("Those changes don't fit this step: {err}"))?;
    may_add(&node.kind, facts)?;
    if node.kind != old.kind {
        mark(&mut node);
    } else {
        node.ai_wrote = old.ai_wrote;
        node.by_ai = old.by_ai;
        node.mode = old.mode;
    }
    let title = named(&node);
    automation.nodes[ix] = node;
    Ok(Applied::changed(format!("Changed \"{id}\"."), format!("Changed {title}"), false))
}

fn connect(automation: &mut Automation, args: &Value) -> Result<Applied, String> {
    let (from, to) = (text(args, "from").ok_or("Give `from`.")?, text(args, "to").ok_or("Give `to`.")?);
    let start = automation.node(from).ok_or_else(|| format!("There is no step \"{from}\"."))?;
    automation.node(to).ok_or_else(|| format!("There is no step \"{to}\"."))?;
    let ports = start.kind.ports();
    let port = text(args, "port").unwrap_or(ports[0]);
    if !ports.contains(&port) {
        return Err(format!("\"{from}\" has no \"{port}\" way out. It has: {}.", ports.join(", ")));
    }
    if from == to {
        return Err("A step can't lead to itself.".into());
    }
    let edge = if port == ports[0] { Edge::new(from, to) } else { Edge::on(from, port, to) };
    if automation.edges.iter().any(|e| e.from == from && e.to == to && e.port_of(&start.kind) == port) {
        return Ok(Applied::said("That line is already there."));
    }
    let mut with = automation.clone();
    with.edges.push(edge.clone());
    if problems(&with).iter().any(|problem| problem.contains("loop")) {
        return Err("That line would make a loop. Steps can't lead back to earlier steps.".into());
    }
    automation.edges.push(edge);
    Ok(Applied::changed(format!("Joined \"{from}\" ({port}) to \"{to}\"."), format!("Joined {from} to {to}"), true))
}

fn remove(automation: &mut Automation, args: &Value) -> Result<Applied, String> {
    if let Some(id) = text(args, "id") {
        let node = automation.node(id).ok_or_else(|| format!("There is no step \"{id}\"."))?;
        let title = named(node);
        automation.nodes.retain(|node| node.id != id);
        automation.edges.retain(|edge| edge.from != id && edge.to != id);
        return Ok(Applied::changed(format!("Removed \"{id}\" and its lines."), format!("Removed {title}"), true));
    }
    let (from, to) = (text(args, "from").ok_or("Give `id`, or `from` and `to`.")?, text(args, "to").ok_or("Give `to`.")?);
    let before = automation.edges.len();
    automation.edges.retain(|edge| !(edge.from == from && edge.to == to));
    if automation.edges.len() == before {
        return Err(format!("There is no line from \"{from}\" to \"{to}\"."));
    }
    Ok(Applied::changed(format!("Removed the line from \"{from}\" to \"{to}\"."), format!("Removed the line {from} to {to}"), true))
}

fn set_start(automation: &mut Automation, args: &Value) -> Result<Applied, String> {
    let mut set = Vec::new();
    if let Some(name) = text(args, "name") {
        automation.name = name.to_owned();
        set.push("name");
    }
    if let Some(description) = args.get("description").and_then(Value::as_str) {
        automation.description = description.trim().to_owned();
        set.push("description");
    }
    if let Some(schedule) = args.get("schedule").and_then(Value::as_str) {
        let schedule = schedule.trim();
        if !schedule.is_empty() {
            somme_automation::schedule::Cron::parse(schedule)?;
        }
        automation.start.schedule = Some(schedule.to_owned()).filter(|schedule| !schedule.is_empty());
        set.push("schedule");
    }
    if let Some(missed) = text(args, "missed") {
        automation.start.missed = serde_json::from_value(json!(missed)).map_err(|_| "`missed` is run_once or skip.")?;
        set.push("missed runs");
    }
    if let Some(inputs) = args.get("inputs").and_then(Value::as_array) {
        automation.inputs = inputs
            .iter()
            .filter_map(|input| {
                let name = text(input, "name")?;
                let name: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
                Some(Input {
                    name,
                    label: text(input, "label").unwrap_or_default().to_owned(),
                    default: input.get("default").and_then(Value::as_str).unwrap_or_default().to_owned(),
                })
            })
            .collect();
        set.push("inputs");
    }
    if set.is_empty() {
        return Err("Give a name, description, schedule or inputs.".into());
    }
    let set = set.join(", ");
    Ok(Applied::changed(format!("Set the {set}."), format!("Set the {set}"), false))
}

/// Strings from a JSON list, trimmed, empty ones left out.
fn texts(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::trim).filter(|item| !item.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default()
}

fn write_plan(automation: &mut Automation, args: &Value) -> Result<Applied, String> {
    let goal = text(args, "goal").ok_or("Give the `goal`.")?;
    let plan = Plan {
        goal: goal.to_owned(),
        done_when: texts(args, "done_when"),
        never: texts(args, "never"),
        assumptions: texts(args, "assumptions"),
        found: texts(args, "found"),
    };
    if plan.done_when.is_empty() {
        return Err("Say how to tell it worked, in `done_when`.".into());
    }
    automation.plan = plan;
    Ok(Applied::changed("Wrote the plan.", "Wrote the plan", false))
}

fn set_scope(automation: &mut Automation, args: &Value) -> Result<Applied, String> {
    let scope = Scope {
        paths: texts(args, "paths"),
        delete: args.get("delete").and_then(Value::as_bool).unwrap_or(false),
        sends: texts(args, "sends"),
        services: texts(args, "services"),
        sudo: args.get("sudo").and_then(Value::as_bool).unwrap_or(false),
    };
    if scope.delete && scope.paths.is_empty() {
        return Err("Deleting needs `paths` to delete in.".into());
    }
    if let Some(wide) = scope.paths.iter().find(|path| too_wide(path)) {
        return Err(format!("{wide} is too wide. Name the folders it really needs."));
    }
    // A new scope waits for the user again.
    automation.scope_ok.clear();
    automation.scope = scope;
    Ok(Applied::changed("Set the scope. The user approves it before runs.", "Set what it may change", false))
}

/// A path that covers a whole home, disk or system folder.
fn too_wide(path: &str) -> bool {
    let path = path.trim().trim_end_matches("/**").trim_end_matches("/*").trim_end_matches('/');
    matches!(path, "" | "~" | "$HOME" | "/" | "*" | "**" | "/Users" | "/home" | "/etc" | "/usr" | "/var" | "/System" | "/Library" | "/opt" | "/tmp" | "/private")
}

/// Saved scripts and ready-made automations that share words with `words`.
fn find_templates(args: &Value, facts: &Facts) -> Applied {
    let words: Vec<String> = text(args, "words").unwrap_or_default().to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|word| word.len() > 2).map(str::to_owned).collect();
    let score = |text: &str| {
        let text = text.to_lowercase();
        words.iter().filter(|word| text.contains(word.as_str())).count()
    };
    let mut found: Vec<(usize, Value)> = Vec::new();
    for (name, command) in &facts.snippets {
        let hits = score(&format!("{name} {command}"));
        if hits > 0 {
            found.push((hits, json!({ "saved_script": name, "command": command.chars().take(SNIPPET_TEXT).collect::<String>() })));
        }
    }
    for template in templates::all() {
        let hits = score(&format!("{} {}", template.name, template.description));
        if hits > 0 {
            found.push((hits, json!({ "template": template.name, "description": template.description, "flow": serde_json::from_str::<Value>(&flow_text(&template)).unwrap_or(Value::Null) })));
        }
    }
    found.sort_by_key(|(hits, _)| std::cmp::Reverse(*hits));
    found.truncate(3);
    if found.is_empty() {
        return Applied::said("Nothing matches. Build it from the steps.");
    }
    Applied::said(Value::Array(found.into_iter().map(|(_, value)| value).collect()).to_string())
}

fn list_hosts(facts: &Facts) -> Applied {
    if facts.hosts.is_empty() {
        return Applied::said("No saved servers. The user adds them under SSH \u{2192} Hosts. Local steps still work.");
    }
    let hosts: Vec<Value> = facts.hosts.iter().map(|(name, tags)| json!({ "name": name, "tags": tags })).collect();
    Applied::said(Value::Array(hosts).to_string())
}

fn list_snippets(facts: &Facts) -> Applied {
    if facts.snippets.is_empty() {
        return Applied::said("No saved snippets.");
    }
    let snippets: Vec<Value> = facts
        .snippets
        .iter()
        .map(|(name, command)| {
            let mut end = command.len().min(SNIPPET_TEXT);
            while !command.is_char_boundary(end) {
                end -= 1;
            }
            json!({ "name": name, "command": &command[..end] })
        })
        .collect();
    Applied::said(Value::Array(snippets).to_string())
}

fn dry_run(automation: &Automation) -> Applied {
    let steps = dry::dry_run(automation, &Default::default());
    let mut out = if steps.is_empty() { "No steps yet.".to_owned() } else { dry::as_text(&steps) };
    let found = problems(automation);
    if !found.is_empty() {
        out.push_str("\n\nBefore it can run:\n");
        out.push_str(&found.join("\n"));
    }
    Applied::said(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall { id: "1".into(), name: name.into(), arguments: args.to_string() }
    }

    #[test]
    fn a_schedule_is_checked_before_it_is_set() {
        let mut flow = Automation::new("Health");
        let bad = apply(&mut flow, &call("set_start", json!({ "schedule": "every hour" })), &facts());
        assert!(bad.result.starts_with("Error:") && flow.start.schedule.is_none());
        let good = apply(&mut flow, &call("set_start", json!({ "schedule": "0 * * * *", "missed": "skip" })), &facts());
        assert_eq!(good.change.as_deref(), Some("Set the schedule, missed runs"));
        assert_eq!(flow.start.schedule.as_deref(), Some("0 * * * *"));
        assert_eq!(flow.start.missed, somme_automation::schema::Missed::Skip);
    }

    fn facts() -> Facts {
        Facts { hosts: vec![("web-1".into(), vec!["web".into()])], write: true, computer: "macos (aarch64), zsh, home /Users/me".into(), ..Facts::default() }
    }

    #[test]
    fn steps_are_added_joined_and_marked() {
        let mut flow = Automation::new("Disk");
        let facts = facts();
        let added = apply(&mut flow, &call("add_node", json!({ "node": { "id": "check", "type": "command", "hosts": { "tag": "web" }, "command": "df -h /" } })), &facts);
        assert_eq!(added.result, "Added \"check\".");
        assert!(flow.nodes[0].ai_wrote, "commands the AI wrote are marked");
        apply(&mut flow, &call("add_node", json!({ "node": { "type": "notify", "title": "Disk" }, "after": "check", "port": "failed" })), &facts);
        assert_eq!(flow.edges, [Edge::on("check", "failed", "notify")]);
        assert!(flow.nodes[1].at[1] > flow.nodes[0].at[1], "placed below the step it follows");
        // A missing field falls back to the blank step's.
        assert!(matches!(&flow.nodes[1].kind, NodeKind::Notify { body, .. } if body.is_empty()));
        assert!(!flow.nodes[1].ai_wrote, "a notification sends nothing out");
        apply(&mut flow, &call("add_node", json!({ "node": { "id": "send", "type": "http", "url": "https://example.test" } })), &facts);
        assert!(flow.nodes[2].ai_wrote, "requests the AI set up are marked");
        flow.nodes[2].ai_wrote = false;
        apply(&mut flow, &call("update_node", json!({ "id": "send", "changes": { "url": "https://other.example.test" } })), &facts);
        assert!(flow.nodes[2].ai_wrote, "and marked again when the AI changes them");
        apply(&mut flow, &call("add_node", json!({ "node": { "id": "mx", "type": "dns", "name": "example.test" } })), &facts);
        assert!(flow.nodes[3].ai_wrote, "a lookup's name can carry a value out");
    }

    #[test]
    fn bad_calls_say_what_is_wrong() {
        let mut flow = Automation::new("Disk");
        let facts = facts();
        let said = |flow: &mut Automation, name: &str, args: Value| apply(flow, &call(name, args), &facts).result;
        assert!(said(&mut flow, "add_node", json!({ "node": { "type": "teleport" } })).contains("no \"teleport\" step"));
        assert!(said(&mut flow, "add_node", json!({ "node": { "type": "decide", "yes_no": "?" } })).contains("Decide steps are off"));
        said(&mut flow, "add_node", json!({ "node": { "id": "a", "type": "wait" } }));
        said(&mut flow, "add_node", json!({ "node": { "id": "b", "type": "wait" }, "after": "a" }));
        assert!(said(&mut flow, "connect", json!({ "from": "b", "to": "a" })).contains("loop"));
        assert!(said(&mut flow, "connect", json!({ "from": "a", "to": "b", "port": "yes" })).contains("no \"yes\" way out"));
        assert!(said(&mut flow, "update_node", json!({ "id": "a", "changes": { "type": "notify" } })).contains("can't change"));
        assert!(said(&mut flow, "nope", json!({})).starts_with("Error:"));
        assert_eq!(flow.edges.len(), 1);
    }

    #[test]
    fn changing_a_command_marks_it_again() {
        let mut flow = Automation::new("Disk");
        let facts = facts();
        apply(&mut flow, &call("add_node", json!({ "node": { "id": "a", "type": "local", "command": "ls" } })), &facts);
        flow.nodes[0].ai_wrote = false;
        apply(&mut flow, &call("update_node", json!({ "id": "a", "changes": { "label": "List" } })), &facts);
        assert!(!flow.nodes[0].ai_wrote, "a new label leaves it checked");
        assert_eq!(flow.nodes[0].label, "List");
        apply(&mut flow, &call("update_node", json!({ "id": "a", "changes": { "command": "rm -rf build" } })), &facts);
        assert!(flow.nodes[0].ai_wrote);
        let removed = apply(&mut flow, &call("remove", json!({ "id": "a" })), &facts);
        assert!(removed.reshaped && flow.nodes.is_empty());
    }

    #[test]
    fn start_and_dry_run() {
        let mut flow = Automation::new("Deploy");
        let facts = facts();
        apply(&mut flow, &call("set_start", json!({ "name": "Deploy app", "inputs": [{ "name": "branch", "default": "main" }] })), &facts);
        apply(&mut flow, &call("add_node", json!({ "node": { "id": "pull", "type": "local", "command": "git pull origin {{input.branch}}" } })), &facts);
        let dry = apply(&mut flow, &call("dry_run", json!({})), &facts).result;
        assert!(dry.contains("git pull origin main"), "{dry}");
        assert!(dry.contains("Looks right"), "the unchecked command is listed: {dry}");
        assert_eq!(flow.name, "Deploy app");
        assert!(apply(&mut flow, &call("list_hosts", json!({})), &facts).result.contains("web-1"));
    }
}
