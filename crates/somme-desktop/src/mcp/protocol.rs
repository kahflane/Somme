//! MCP messages: JSON-RPC in, answers out. Agents get `list_hosts` and
//! `run_script`, and the automations the user lets them run:
//! `list_automations`, `run_automation` and `automation_status`.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};

/// Newest first. An agent asking for another one gets the newest.
const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const DEFAULT_TIMEOUT: u64 = 120;
const MAX_TIMEOUT: u64 = 60 * 60;
const MAX_SCRIPT: usize = 1024 * 1024;

const INSTRUCTIONS: &str = "Somme is the user's SSH app. list_hosts shows the servers they saved; \
run_script runs a shell script on one of them over SSH and returns the exit code and output. \
Scripts run without a terminal and get no input, so use non-interactive flags (for example `sudo -n`, `apt-get -y`). \
Somme writes every script to a log the user can see. \
list_automations shows the automations the user lets agents start; run_automation starts one and automation_status tells how it went.";

/// The longest run_automation waits for a run to end.
const MAX_WAIT: u64 = 10 * 60;

/// A saved server, as agents see it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HostInfo {
    pub id: String,
    pub name: String,
    pub address: String,
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptRun {
    pub exit_status: Option<u32>,
    pub exit_signal: Option<String>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub seconds: f64,
}

/// An automation agents may start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AutomationInfo {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Each input's name and default.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
}

/// What the tools reach: the saved servers and SSH. Errors are written for
/// the agent to read.
#[async_trait]
pub trait Backend: Send + Sync {
    async fn hosts(&self) -> Result<Vec<HostInfo>, String>;
    async fn run(&self, agent: &str, host: &HostInfo, script: &str, timeout: Duration) -> Result<ScriptRun, String>;

    /// The automations the user lets agents start.
    async fn automations(&self) -> Result<Vec<AutomationInfo>, String> {
        Ok(Vec::new())
    }

    /// Starts one and gives the run's id.
    async fn run_automation(&self, _agent: &str, _id: &str, _inputs: BTreeMap<String, String>) -> Result<String, String> {
        Err("Automations are not available here.".into())
    }

    /// Whether a run ended, and how it went, as text.
    async fn automation_status(&self, _run: &str) -> Result<(bool, String), String> {
        Err("Automations are not available here.".into())
    }
}

/// One agent's connection.
#[derive(Default)]
pub struct Session {
    agent: parking_lot::Mutex<Option<String>>,
}

impl Session {
    /// The agent's name and version, as it gave them.
    pub fn agent(&self) -> String {
        self.agent.lock().clone().unwrap_or_else(|| "unknown agent".to_owned())
    }

    /// The answer to one message or a batch. Notifications get none.
    pub async fn handle(&self, backend: &dyn Backend, message: Value) -> Option<Value> {
        match message {
            Value::Array(batch) if !batch.is_empty() => {
                let mut answers = Vec::new();
                for message in batch {
                    answers.extend(self.handle_one(backend, message).await);
                }
                (!answers.is_empty()).then_some(Value::Array(answers))
            }
            message => self.handle_one(backend, message).await,
        }
    }

    async fn handle_one(&self, backend: &dyn Backend, message: Value) -> Option<Value> {
        let Some(fields) = message.as_object() else {
            return Some(error(Value::Null, -32600, "Invalid request"));
        };
        let id = fields.get("id").cloned();
        let Some(method) = fields.get("method").and_then(Value::as_str) else {
            // An answer to a request; this server sends none.
            if fields.contains_key("result") || fields.contains_key("error") {
                return None;
            }
            return Some(error(id.unwrap_or(Value::Null), -32600, "Invalid request"));
        };
        // Notifications, like `notifications/initialized`, need nothing back.
        let id = id?;
        let params = fields.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => self.call(backend, &params).await,
            _ => Err((-32601, format!("Method not found: {method}"))),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => error(id, code, &message),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let client = &params["clientInfo"];
        let name = client["name"].as_str().unwrap_or("unknown agent");
        let agent = match client["version"].as_str() {
            Some(version) => format!("{name} {version}"),
            None => name.to_owned(),
        };
        // Names show in the log, so they stay short and on one line.
        *self.agent.lock() = Some(agent.chars().filter(|c| !c.is_control()).take(80).collect());
        let asked = params["protocolVersion"].as_str().unwrap_or_default();
        let version = VERSIONS.iter().find(|v| **v == asked).unwrap_or(&VERSIONS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "somme", "title": "Somme", "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS,
        })
    }

    async fn call(&self, backend: &dyn Backend, params: &Value) -> Result<Value, (i64, String)> {
        let args = &params["arguments"];
        match params["name"].as_str().unwrap_or_default() {
            "list_hosts" => Ok(tool_result(list_hosts(backend).await)),
            "run_script" => Ok(tool_result(self.run_script(backend, args).await)),
            "list_automations" => Ok(tool_result(list_automations(backend).await)),
            "run_automation" => Ok(tool_result(self.run_automation(backend, args).await)),
            "automation_status" => Ok(tool_result(automation_status(backend, args).await)),
            other => Err((-32602, format!("Unknown tool: {other}"))),
        }
    }

    async fn run_script(&self, backend: &dyn Backend, args: &Value) -> Result<String, String> {
        let wanted = args["host"].as_str().map(str::trim).filter(|h| !h.is_empty());
        let wanted = wanted.ok_or("`host` is required: a server's name or id from list_hosts.")?;
        let script = args["script"].as_str().filter(|s| !s.trim().is_empty()).ok_or("`script` is required.")?;
        if script.len() > MAX_SCRIPT {
            return Err(format!("The script is over {} KB. Send a shorter one.", MAX_SCRIPT / 1024));
        }
        let seconds = match &args["timeout_seconds"] {
            Value::Null => DEFAULT_TIMEOUT,
            value => value
                .as_u64()
                .filter(|s| (1..=MAX_TIMEOUT).contains(s))
                .ok_or(format!("`timeout_seconds` must be a whole number from 1 to {MAX_TIMEOUT}."))?,
        };
        let hosts = backend.hosts().await?;
        let host = resolve(&hosts, wanted)?;
        let run = backend.run(&self.agent(), host, script, Duration::from_secs(seconds)).await?;
        Ok(report(&run, seconds))
    }
}

impl Session {
    async fn run_automation(&self, backend: &dyn Backend, args: &Value) -> Result<String, String> {
        let wanted = args["automation"].as_str().map(str::trim).filter(|a| !a.is_empty());
        let wanted = wanted.ok_or("`automation` is required: a name or id from list_automations.")?;
        let mut inputs = BTreeMap::new();
        match &args["inputs"] {
            Value::Null => {}
            Value::Object(fields) => {
                for (name, value) in fields {
                    let text = match value {
                        Value::String(text) => text.clone(),
                        Value::Null => String::new(),
                        other => other.to_string(),
                    };
                    inputs.insert(name.clone(), text);
                }
            }
            _ => return Err("`inputs` must be an object of names to values.".into()),
        }
        let wait = match &args["wait_seconds"] {
            Value::Null => 0,
            value => value.as_u64().filter(|s| *s <= MAX_WAIT).ok_or(format!("`wait_seconds` must be a whole number from 0 to {MAX_WAIT}."))?,
        };
        let automations = backend.automations().await?;
        let automation = automations
            .iter()
            .find(|a| a.id == wanted)
            .or_else(|| automations.iter().find(|a| a.name.eq_ignore_ascii_case(wanted)))
            .ok_or(format!("No automation agents may run is called \"{wanted}\". Call list_automations to see them."))?;
        if let Some(unknown) = inputs.keys().find(|name| !automation.inputs.contains_key(*name)) {
            return Err(format!("\"{}\" has no input called \"{unknown}\".", automation.name));
        }
        let run = backend.run_automation(&self.agent(), &automation.id, inputs).await?;
        let began = std::time::Instant::now();
        while began.elapsed().as_secs() < wait {
            let (done, text) = backend.automation_status(&run).await?;
            if done {
                return Ok(text);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Ok(format!("Started run {run} of \"{}\". Call automation_status with this run id to see how it goes.", automation.name))
    }
}

async fn list_automations(backend: &dyn Backend) -> Result<String, String> {
    let automations = backend.automations().await?;
    if automations.is_empty() {
        return Ok("The user has not let agents run any automation. They can turn on \"AI agents can run this\" on one in Somme.".to_owned());
    }
    serde_json::to_string_pretty(&automations).map_err(|err| err.to_string())
}

async fn automation_status(backend: &dyn Backend, args: &Value) -> Result<String, String> {
    let run = args["run"].as_str().map(str::trim).filter(|r| !r.is_empty()).ok_or("`run` is required: the run id run_automation gave.")?;
    Ok(backend.automation_status(run).await?.1)
}

async fn list_hosts(backend: &dyn Backend) -> Result<String, String> {
    let hosts = backend.hosts().await?;
    if hosts.is_empty() {
        return Ok("No servers are saved in Somme yet.".to_owned());
    }
    serde_json::to_string_pretty(&hosts).map_err(|err| err.to_string())
}

/// The host an agent means: by id, else by name, else by address.
fn resolve<'a>(hosts: &'a [HostInfo], wanted: &str) -> Result<&'a HostInfo, String> {
    if let Some(host) = hosts.iter().find(|h| h.id == wanted) {
        return Ok(host);
    }
    let named: Vec<&HostInfo> = hosts.iter().filter(|h| h.name.eq_ignore_ascii_case(wanted)).collect();
    let found = if named.is_empty() {
        hosts.iter().filter(|h| h.address.eq_ignore_ascii_case(wanted)).collect()
    } else {
        named
    };
    match found.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("No saved server is called \"{wanted}\". Call list_hosts to see them.")),
        many => Err(format!(
            "{} servers are called \"{wanted}\". Use one of these ids: {}",
            many.len(),
            many.iter().map(|h| h.id.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

fn report(run: &ScriptRun, timeout: u64) -> String {
    let mut text = if run.timed_out {
        format!("Stopped: the script ran past its {timeout} s limit.")
    } else if let Some(code) = run.exit_status {
        format!("Exit code {code}.")
    } else if let Some(signal) = &run.exit_signal {
        format!("Stopped by signal {signal}.")
    } else {
        "The server did not say how the script ended.".to_owned()
    };
    text.push_str(&format!(" Took {:.1} s.", run.seconds));
    for (name, output) in [("stdout", &run.stdout), ("stderr", &run.stderr)] {
        if !output.is_empty() {
            text.push_str(&format!("\n\n{name}:\n{output}"));
        }
    }
    if run.stdout.is_empty() && run.stderr.is_empty() {
        text.push_str("\n\n(no output)");
    }
    text
}

fn tool_result(result: Result<String, String>) -> Value {
    let (text, failed) = match result {
        Ok(text) => (text, false),
        Err(text) => (text, true),
    };
    json!({ "content": [{ "type": "text", "text": text }], "isError": failed })
}

fn tools() -> Value {
    json!([
        {
            "name": "list_hosts",
            "title": "List servers",
            "description": "Lists the SSH servers saved in Somme: name, id, address, user, group and tags. Pass a server's name or id to run_script.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "run_script",
            "title": "Run a script on a server",
            "description": "Runs a shell script on a saved server over SSH with the user's login shell, without a terminal and with no input. Returns the exit code, stdout and stderr; very long output is cut in the middle. Somme logs every script for the user.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "host": { "type": "string", "description": "The server's name or id, from list_hosts." },
                    "script": { "type": "string", "description": "The shell script to run." },
                    "timeout_seconds": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_TIMEOUT,
                        "default": DEFAULT_TIMEOUT,
                        "description": "Stop the script after this many seconds."
                    }
                },
                "required": ["host", "script"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "openWorldHint": true }
        },
        {
            "name": "list_automations",
            "title": "List automations",
            "description": "Lists the Somme automations the user lets agents start: id, name, what it does and its inputs with their defaults.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "run_automation",
            "title": "Run an automation",
            "description": "Starts one of the user's automations, from list_automations. Its steps run as the user built them, checked by the user's guard rules. Gives a run id for automation_status, or the result when it ends within wait_seconds.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "automation": { "type": "string", "description": "The automation's name or id, from list_automations." },
                    "inputs": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Values for its inputs, by name. Missing ones use their defaults." },
                    "wait_seconds": { "type": "integer", "minimum": 0, "maximum": MAX_WAIT, "default": 0, "description": "Wait this long for the run to end and give its result." }
                },
                "required": ["automation"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "openWorldHint": true }
        },
        {
            "name": "automation_status",
            "title": "How an automation run went",
            "description": "Tells whether a run started by run_automation is still going, and what each step said.",
            "inputSchema": {
                "type": "object",
                "properties": { "run": { "type": "string", "description": "The run id from run_automation." } },
                "required": ["run"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        }
    ])
}

pub fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use super::*;

    /// An agent, an automation and its inputs.
    type Started = (String, String, BTreeMap<String, String>);

    #[derive(Default)]
    struct Fake {
        ran: Mutex<Vec<(String, String, String, u64)>>,
        started: Mutex<Vec<Started>>,
    }

    #[async_trait]
    impl Backend for Fake {
        async fn hosts(&self) -> Result<Vec<HostInfo>, String> {
            let host = |id: &str, name: &str| HostInfo {
                id: id.into(),
                name: name.into(),
                address: format!("{name}.example.test"),
                port: 22,
                user: Some("deploy".into()),
                group: None,
                tags: Vec::new(),
            };
            Ok(vec![host("1", "web"), host("2", "db"), host("3", "db")])
        }

        async fn run(&self, agent: &str, host: &HostInfo, script: &str, timeout: Duration) -> Result<ScriptRun, String> {
            self.ran.lock().push((agent.into(), host.id.clone(), script.into(), timeout.as_secs()));
            Ok(ScriptRun {
                exit_status: Some(0),
                stdout: "up 3 days\n".into(),
                seconds: 0.4,
                ..ScriptRun::default()
            })
        }

        async fn automations(&self) -> Result<Vec<AutomationInfo>, String> {
            Ok(vec![AutomationInfo {
                id: "a1".into(),
                name: "Deploy".into(),
                description: String::new(),
                inputs: BTreeMap::from([("branch".to_owned(), "main".to_owned())]),
            }])
        }

        async fn run_automation(&self, agent: &str, id: &str, inputs: BTreeMap<String, String>) -> Result<String, String> {
            self.started.lock().push((agent.into(), id.into(), inputs));
            Ok("r1".into())
        }

        async fn automation_status(&self, run: &str) -> Result<(bool, String), String> {
            Ok((true, format!("Run {run}: done.")))
        }
    }

    fn call(id: u64, name: &str, arguments: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } })
    }

    fn text(answer: &Value) -> &str {
        answer["result"]["content"][0]["text"].as_str().unwrap()
    }

    #[tokio::test]
    async fn hello_then_tools() {
        let fake = Fake::default();
        let session = Session::default();
        let hello = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "claude-code", "version": "2.1.0" }
        }});
        let answer = session.handle(&fake, hello).await.unwrap();
        assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(answer["result"]["serverInfo"]["name"], "somme");
        assert_eq!(session.agent(), "claude-code 2.1.0");

        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(session.handle(&fake, note).await.is_none());

        let list = session.handle(&fake, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).await.unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["list_hosts", "run_script", "list_automations", "run_automation", "automation_status"]);
    }

    #[tokio::test]
    async fn runs_a_script_by_name() {
        let fake = Fake::default();
        let session = Session::default();
        let answer = session.handle(&fake, call(3, "run_script", json!({ "host": "WEB", "script": "uptime" }))).await.unwrap();
        assert_eq!(answer["result"]["isError"], false);
        assert!(text(&answer).starts_with("Exit code 0."));
        assert!(text(&answer).contains("stdout:\nup 3 days"));
        assert_eq!(fake.ran.lock()[0], ("unknown agent".into(), "1".into(), "uptime".into(), DEFAULT_TIMEOUT));
    }

    #[tokio::test]
    async fn agents_start_allowed_automations_by_name() {
        let fake = Fake::default();
        let session = Session::default();
        let list = session.handle(&fake, call(1, "list_automations", json!({}))).await.unwrap();
        assert!(text(&list).contains("\"Deploy\""));
        let wrong = session.handle(&fake, call(2, "run_automation", json!({ "automation": "deploy", "inputs": { "tag": "v2" } }))).await.unwrap();
        assert_eq!(wrong["result"]["isError"], true);
        let missing = session.handle(&fake, call(3, "run_automation", json!({ "automation": "Backup" }))).await.unwrap();
        assert!(text(&missing).contains("list_automations"));
        assert!(fake.started.lock().is_empty());
        let run = json!({ "automation": "deploy", "inputs": { "branch": "dev" }, "wait_seconds": 5 });
        let done = session.handle(&fake, call(4, "run_automation", run)).await.unwrap();
        assert_eq!(text(&done), "Run r1: done.");
        assert_eq!(fake.started.lock()[0], ("unknown agent".into(), "a1".into(), BTreeMap::from([("branch".to_owned(), "dev".to_owned())])));
    }

    #[tokio::test]
    async fn unclear_or_bad_requests_run_nothing() {
        let fake = Fake::default();
        let session = Session::default();
        let two = session.handle(&fake, call(4, "run_script", json!({ "host": "db", "script": "id" }))).await.unwrap();
        assert_eq!(two["result"]["isError"], true);
        assert!(text(&two).contains("2, 3"));
        let missing = session.handle(&fake, call(5, "run_script", json!({ "host": "mail", "script": "id" }))).await.unwrap();
        assert!(text(&missing).contains("No saved server"));
        let slow = json!({ "host": "web", "script": "id", "timeout_seconds": 0 });
        assert_eq!(session.handle(&fake, call(6, "run_script", slow)).await.unwrap()["result"]["isError"], true);
        let empty = session.handle(&fake, call(7, "run_script", json!({ "host": "web", "script": "  " }))).await.unwrap();
        assert_eq!(empty["result"]["isError"], true);
        assert!(fake.ran.lock().is_empty());

        let unknown = session.handle(&fake, call(8, "rm_rf", json!({}))).await.unwrap();
        assert_eq!(unknown["error"]["code"], -32602);
        let method = session.handle(&fake, json!({ "jsonrpc": "2.0", "id": 9, "method": "resources/list" })).await.unwrap();
        assert_eq!(method["error"]["code"], -32601);
    }
}
