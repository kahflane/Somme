//! The flow format: steps (nodes), the lines between them (edges), and how
//! it starts. Saved as JSON with a schema number, so old flows keep loading.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// The format this version writes.
pub const SCHEMA: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Automation {
    pub schema: u32,
    /// Set when it is first saved.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub start: Start,
    /// Asked for when it is run by hand, and given by agents.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Input>,
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

impl Automation {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema: SCHEMA,
            id: String::new(),
            name: name.into(),
            description: String::new(),
            start: Start::default(),
            inputs: Vec::new(),
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let automation: Self = serde_json::from_str(text).map_err(|err| format!("Not an automation: {err}"))?;
        if automation.schema > SCHEMA {
            return Err(format!(
                "This automation was made by a newer Somme (format {}). Update Somme to open it.",
                automation.schema
            ));
        }
        Ok(automation)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Start {
    /// Cron, like `0 * * * *`. None: only by hand or by an agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    #[serde(default)]
    pub missed: Missed,
    /// Agents may start it over MCP.
    #[serde(default)]
    pub agents: bool,
}

/// A run the schedule missed while the computer slept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Missed {
    /// Run once when it wakes.
    #[default]
    RunOnce,
    Skip,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Input {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(flatten)]
    pub kind: NodeKind,
    /// Where it sits on the canvas.
    #[serde(default)]
    pub at: [f32; 2],
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Each try, in seconds. None: the step's own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Tries again this many times when it fails.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub retries: u32,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub retry_wait_seconds: u64,
    /// An AI wrote its command, and the user has not checked it yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ai_wrote: bool,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_zero_u64(n: &u64) -> bool {
    *n == 0
}

impl Node {
    pub fn new(id: impl Into<String>, kind: NodeKind) -> Self {
        Self {
            id: id.into(),
            kind,
            at: [0.0, 0.0],
            label: String::new(),
            timeout_seconds: None,
            retries: 0,
            retry_wait_seconds: 0,
            ai_wrote: false,
        }
    }

    /// The label, or a name from the step's type.
    pub fn title(&self) -> String {
        if self.label.trim().is_empty() { self.kind.name().to_owned() } else { self.label.clone() }
    }
}

/// What a step does. Its fields can hold `{{…}}` values from earlier steps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeKind {
    /// A command on saved servers.
    Command {
        hosts: HostPick,
        command: String,
        /// One server after another, stopping at the first failure. Otherwise all at once.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        one_by_one: bool,
        /// Runs it with `sudo -n`, which works only without a password.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        sudo: bool,
    },
    /// A command on this computer.
    Local {
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dir: Option<String>,
    },
    /// Goes on by `yes` or `no`.
    If {
        value: String,
        test: Test,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        against: String,
    },
    Wait {
        seconds: u64,
    },
    /// A system notification.
    Notify {
        title: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        body: String,
    },
    /// A yes or no question about earlier output, answered by Jev, else
    /// the LLM. Goes on by `yes`, `no`, or `unsure` when neither is sure.
    Decide {
        yes_no: String,
        /// What the question is about, like `{{nodes.check.output}}`.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        about: String,
        /// The chance of yes at or above which it is a yes.
        #[serde(default = "default_yes_from")]
        yes_from: f64,
        /// At or below: a no.
        #[serde(default = "default_no_to")]
        no_to: f64,
    },
    /// Text the LLM writes, like a short summary. Later steps use
    /// `{{nodes.<id>.text}}`.
    Write { prompt: String },
    /// Copies one file between this computer and a server, over SFTP.
    Copy {
        /// The first server it names.
        host: HostPick,
        direction: Direction,
        /// The file on this computer.
        local: String,
        /// The file on the server.
        remote: String,
    },
    /// Asks the user to allow or deny before it goes on.
    Ask {
        title: String,
        /// What they are asked, like the command that runs next.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        message: String,
    },
    Http {
        #[serde(default = "default_method")]
        method: String,
        url: String,
        /// One `Name: value` per line.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        headers: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        body: String,
    },
    /// The certificate a server shows: who it is for and when it expires.
    Tls {
        host: String,
        #[serde(default = "default_tls_port")]
        port: u16,
    },
    Dns {
        name: String,
        /// A, AAAA, CNAME, MX, TXT or NS.
        #[serde(default = "default_record")]
        record: String,
    },
    /// An AI with a goal, like Claude Code or Codex. It runs commands, reads
    /// and changes files, and asks you, until it is done or hits a limit.
    /// The guard checks each thing it does.
    Agent {
        goal: String,
        /// The servers it may use.
        #[serde(default, skip_serializing_if = "HostPick::is_empty")]
        hosts: HostPick,
        /// It may also work on this computer.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        local: bool,
        /// Asks you before each command or change. Looking needs no OK.
        #[serde(default = "default_true")]
        ask_first: bool,
        /// Tool calls it may make.
        #[serde(default = "default_agent_steps")]
        max_steps: u32,
        /// Tokens it may use, sent and received.
        #[serde(default = "default_agent_tokens")]
        max_tokens: u64,
        /// It may see and use this computer's screen, in `apps` only.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        computer: bool,
        /// The apps it may use on the screen, by name, like `Safari`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        apps: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// From this computer to the server.
    #[default]
    Upload,
    /// From the server to this computer.
    Download,
}

fn default_method() -> String {
    "GET".into()
}

fn default_tls_port() -> u16 {
    443
}

fn default_record() -> String {
    "A".into()
}

fn default_true() -> bool {
    true
}

pub const AGENT_STEPS: u32 = 20;
pub const AGENT_TOKENS: u64 = 200_000;
/// The most tool calls an Agent step may be given.
pub const MOST_AGENT_STEPS: u32 = 200;

fn default_agent_steps() -> u32 {
    AGENT_STEPS
}

fn default_agent_tokens() -> u64 {
    AGENT_TOKENS
}

fn default_yes_from() -> f64 {
    0.8
}

fn default_no_to() -> f64 {
    0.2
}

impl NodeKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Command { .. } => "Run a command",
            Self::Local { .. } => "Run on this computer",
            Self::If { .. } => "If",
            Self::Wait { .. } => "Wait",
            Self::Notify { .. } => "Notify",
            Self::Decide { .. } => "Decide",
            Self::Write { .. } => "Write",
            Self::Copy { .. } => "Copy a file",
            Self::Ask { .. } => "Ask me",
            Self::Http { .. } => "HTTP request",
            Self::Tls { .. } => "Certificate",
            Self::Dns { .. } => "DNS lookup",
            Self::Agent { .. } => "Agent",
        }
    }

    /// The ports lines can leave from. The first is the one a line with no
    /// port follows.
    pub fn ports(&self) -> &'static [&'static str] {
        match self {
            Self::Command { .. }
            | Self::Local { .. }
            | Self::Write { .. }
            | Self::Copy { .. }
            | Self::Http { .. }
            | Self::Tls { .. }
            | Self::Dns { .. }
            | Self::Agent { .. } => &[OK, FAILED],
            Self::Ask { .. } => &[ALLOWED, DENIED],
            Self::If { .. } => &[YES, NO],
            Self::Decide { .. } => &[YES, NO, UNSURE],
            Self::Wait { .. } | Self::Notify { .. } => &[OK],
        }
    }

    /// Whether it sends text to an AI service.
    pub fn uses_ai(&self) -> bool {
        matches!(self, Self::Decide { .. } | Self::Write { .. } | Self::Agent { .. })
    }

    /// Whether what it gives later steps was written by an AI.
    pub fn writes_ai_text(&self) -> bool {
        matches!(self, Self::Write { .. } | Self::Agent { .. })
    }

    /// Whether it runs, changes or sends something, so a version an AI set
    /// up must be read before it runs. A lookup counts: its name can carry
    /// a value out.
    pub fn acts(&self) -> bool {
        matches!(
            self,
            Self::Command { .. } | Self::Local { .. } | Self::Copy { .. } | Self::Http { .. } | Self::Tls { .. } | Self::Dns { .. } | Self::Agent { .. }
        )
    }

    /// The fields where text an AI wrote in a run could do harm: what runs,
    /// where files go, and where requests and lookups are sent.
    pub fn sink_texts(&self) -> Vec<&str> {
        match self {
            Self::Command { command, .. } => vec![command],
            Self::Local { command, dir } => [Some(command.as_str()), dir.as_deref()].into_iter().flatten().collect(),
            Self::Copy { local, remote, .. } => vec![local, remote],
            Self::Http { url, headers, body, .. } => vec![url, headers, body],
            Self::Tls { host, .. } => vec![host],
            Self::Dns { name, .. } => vec![name],
            Self::Agent { goal, ask_first: false, .. } => vec![goal],
            _ => Vec::new(),
        }
    }

    /// The command it runs, for steps that run one.
    pub fn command(&self) -> Option<&str> {
        match self {
            Self::Command { command, .. } | Self::Local { command, .. } => Some(command),
            _ => None,
        }
    }

    /// How long one try may take, when the step sets none.
    pub fn default_timeout(&self) -> u64 {
        match self {
            Self::Command { .. } | Self::Local { .. } => 10 * 60,
            // Its own length, plus a little.
            Self::Wait { seconds } => seconds.saturating_add(5),
            Self::If { .. } | Self::Notify { .. } => 30,
            // Jev, then the LLM, each with a try again.
            Self::Decide { .. } => 90,
            Self::Write { .. } => 3 * 60,
            Self::Copy { .. } => 30 * 60,
            // As long as it takes the user to see it.
            Self::Ask { .. } => 60 * 60,
            Self::Http { .. } | Self::Tls { .. } | Self::Dns { .. } => 60,
            Self::Agent { .. } => 30 * 60,
        }
    }
}

pub const OK: &str = "ok";
pub const FAILED: &str = "failed";
pub const YES: &str = "yes";
pub const NO: &str = "no";
pub const UNSURE: &str = "unsure";
pub const ALLOWED: &str = "allowed";
pub const DENIED: &str = "denied";

/// Which servers a command runs on. All the ones named, by id, name or tag.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostPick {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

impl HostPick {
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty() && self.names.is_empty() && self.tag.as_deref().is_none_or(|tag| tag.trim().is_empty())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Test {
    Equals,
    NotEquals,
    Contains,
    NotContains,
    /// A regular expression.
    Matches,
    /// As numbers.
    Above,
    Below,
    Empty,
    NotEmpty,
}

impl Test {
    pub const ALL: &[Test] = &[
        Test::Equals,
        Test::NotEquals,
        Test::Contains,
        Test::NotContains,
        Test::Matches,
        Test::Above,
        Test::Below,
        Test::Empty,
        Test::NotEmpty,
    ];

    pub fn words(self) -> &'static str {
        match self {
            Test::Equals => "is",
            Test::NotEquals => "is not",
            Test::Contains => "contains",
            Test::NotContains => "does not contain",
            Test::Matches => "matches",
            Test::Above => "is more than",
            Test::Below => "is less than",
            Test::Empty => "is empty",
            Test::NotEmpty => "is not empty",
        }
    }

    pub fn needs_against(self) -> bool {
        !matches!(self, Test::Empty | Test::NotEmpty)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    /// None: the step's first port, like `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    pub to: String,
}

impl Edge {
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self { from: from.into(), port: None, to: to.into() }
    }

    pub fn on(from: impl Into<String>, port: &str, to: impl Into<String>) -> Self {
        Self { from: from.into(), port: Some(port.to_owned()), to: to.into() }
    }

    /// The port it leaves from, given its step.
    pub fn port_of<'a>(&'a self, from: &NodeKind) -> &'a str {
        self.port.as_deref().unwrap_or_else(|| from.ports()[0])
    }
}

/// What is wrong with a flow, in words for the user. Empty when it can run.
pub fn problems(automation: &Automation) -> Vec<String> {
    let mut found = Vec::new();
    if automation.nodes.is_empty() {
        found.push("Add a step.".to_owned());
    }
    if let Some(schedule) = automation.start.schedule.as_deref().filter(|text| !text.trim().is_empty())
        && let Err(err) = crate::schedule::Cron::parse(schedule)
    {
        found.push(err);
    }
    let mut ids = BTreeSet::new();
    for node in &automation.nodes {
        if node.id.trim().is_empty() {
            found.push("A step has no id.".to_owned());
        } else if !ids.insert(node.id.as_str()) {
            found.push(format!("Two steps are called \u{201c}{}\u{201d}.", node.id));
        }
        match &node.kind {
            NodeKind::Command { hosts, command, .. } => {
                if hosts.is_empty() {
                    found.push(format!("\u{201c}{}\u{201d} has no servers.", node.title()));
                }
                if command.trim().is_empty() {
                    found.push(format!("\u{201c}{}\u{201d} has no command.", node.title()));
                }
            }
            NodeKind::Local { command, .. } if command.trim().is_empty() => {
                found.push(format!("\u{201c}{}\u{201d} has no command.", node.title()));
            }
            NodeKind::If { test: Test::Contains | Test::NotContains | Test::Matches | Test::Above | Test::Below, against, .. }
                if against.trim().is_empty() =>
            {
                found.push(format!("\u{201c}{}\u{201d} has nothing to compare with.", node.title()));
            }
            NodeKind::If { test: Test::Matches, against, .. } if regex::Regex::new(against).is_err() => {
                found.push(format!("\u{201c}{}\u{201d} has a pattern that does not work.", node.title()));
            }
            NodeKind::Notify { title, .. } if title.trim().is_empty() => {
                found.push(format!("\u{201c}{}\u{201d} has no title.", node.title()));
            }
            NodeKind::Decide { yes_no, yes_from, no_to, .. } => {
                if yes_no.trim().is_empty() {
                    found.push(format!("\u{201c}{}\u{201d} has no question.", node.title()));
                }
                if !(0.0..=1.0).contains(yes_from) || !(0.0..=1.0).contains(no_to) || no_to >= yes_from {
                    found.push(format!("\u{201c}{}\u{201d} needs \u{201c}sure no\u{201d} below \u{201c}sure yes\u{201d}.", node.title()));
                }
            }
            NodeKind::Write { prompt } if prompt.trim().is_empty() => {
                found.push(format!("\u{201c}{}\u{201d} has nothing to write about.", node.title()));
            }
            NodeKind::Copy { host, local, remote, .. } => {
                if host.is_empty() {
                    found.push(format!("\u{201c}{}\u{201d} has no server.", node.title()));
                }
                if local.trim().is_empty() || remote.trim().is_empty() {
                    found.push(format!("\u{201c}{}\u{201d} needs both files.", node.title()));
                }
            }
            NodeKind::Ask { title, .. } if title.trim().is_empty() => {
                found.push(format!("\u{201c}{}\u{201d} has no question.", node.title()));
            }
            NodeKind::Http { url, .. } if !(url.trim().starts_with("http://") || url.trim().starts_with("https://") || url.trim().starts_with("{{")) => {
                found.push(format!("\u{201c}{}\u{201d} needs an address starting with https://.", node.title()));
            }
            NodeKind::Tls { host, .. } if host.trim().is_empty() => {
                found.push(format!("\u{201c}{}\u{201d} has no host.", node.title()));
            }
            NodeKind::Dns { name, record } => {
                if name.trim().is_empty() {
                    found.push(format!("\u{201c}{}\u{201d} has no name to look up.", node.title()));
                }
                if !crate::net::RECORDS.contains(&record.trim().to_uppercase().as_str()) {
                    found.push(format!("\u{201c}{}\u{201d} asks for a record type Somme doesn't look up.", node.title()));
                }
            }
            NodeKind::Agent { goal, hosts, local, max_steps, max_tokens, computer, apps, .. } => {
                if goal.trim().is_empty() {
                    found.push(format!("\u{201c}{}\u{201d} has no goal.", node.title()));
                }
                if hosts.is_empty() && !local && !computer {
                    found.push(format!("\u{201c}{}\u{201d} has nowhere to work. Pick servers or this computer.", node.title()));
                }
                if *computer && apps.iter().all(|app| app.trim().is_empty()) {
                    found.push(format!("\u{201c}{}\u{201d} may use the screen but names no apps. Name the apps it may use.", node.title()));
                }
                if !(1..=MOST_AGENT_STEPS).contains(max_steps) {
                    found.push(format!("\u{201c}{}\u{201d} needs from 1 to {MOST_AGENT_STEPS} steps.", node.title()));
                }
                if *max_tokens < 1000 {
                    found.push(format!("\u{201c}{}\u{201d} needs at least 1000 tokens.", node.title()));
                }
            }
            _ => {}
        }
        if node.ai_wrote && node.kind.acts() {
            found.push(format!("An AI set up \u{201c}{}\u{201d}. Read it, then press Looks right.", node.title()));
        }
    }
    for edge in &automation.edges {
        let (Some(from), Some(_)) = (automation.node(&edge.from), automation.node(&edge.to)) else {
            found.push(format!("A line goes to or from a missing step ({} → {}).", edge.from, edge.to));
            continue;
        };
        if let Some(port) = &edge.port
            && !from.kind.ports().contains(&port.as_str())
        {
            found.push(format!("\u{201c}{}\u{201d} has no \u{201c}{port}\u{201d} way out.", from.title()));
        }
        if edge.from == edge.to {
            found.push(format!("\u{201c}{}\u{201d} goes to itself.", from.title()));
        }
    }
    if found.is_empty() && has_loop(automation) {
        found.push("The steps go round in a loop.".to_owned());
    }
    found
}

fn has_loop(automation: &Automation) -> bool {
    // Kahn's way: take steps with nothing left coming in; a loop never empties.
    let mut incoming: BTreeMap<&str, usize> = automation.nodes.iter().map(|node| (node.id.as_str(), 0)).collect();
    for edge in &automation.edges {
        *incoming.entry(edge.to.as_str()).or_default() += 1;
    }
    let mut ready: Vec<&str> = incoming.iter().filter(|(_, n)| **n == 0).map(|(id, _)| *id).collect();
    let mut taken = 0;
    while let Some(id) = ready.pop() {
        taken += 1;
        for edge in automation.edges.iter().filter(|edge| edge.from == id) {
            if let Some(n) = incoming.get_mut(edge.to.as_str()) {
                *n -= 1;
                if *n == 0 {
                    ready.push(edge.to.as_str());
                }
            }
        }
    }
    taken < incoming.len()
}

/// A step of type `kind`, as the format names it, with empty fields.
pub fn blank(kind: &str) -> Option<NodeKind> {
    Some(match kind {
        "command" => NodeKind::Command { hosts: HostPick::default(), command: String::new(), one_by_one: false, sudo: false },
        "local" => NodeKind::Local { command: String::new(), dir: None },
        "if" => NodeKind::If { value: String::new(), test: Test::Contains, against: String::new() },
        "wait" => NodeKind::Wait { seconds: 10 },
        "notify" => NodeKind::Notify { title: String::new(), body: String::new() },
        "decide" => NodeKind::Decide { yes_no: String::new(), about: String::new(), yes_from: 0.8, no_to: 0.2 },
        "write" => NodeKind::Write { prompt: String::new() },
        "copy" => NodeKind::Copy { host: HostPick::default(), direction: Direction::Upload, local: String::new(), remote: String::new() },
        "ask" => NodeKind::Ask { title: String::new(), message: String::new() },
        "http" => NodeKind::Http { method: default_method(), url: String::new(), headers: String::new(), body: String::new() },
        "tls" => NodeKind::Tls { host: String::new(), port: default_tls_port() },
        "dns" => NodeKind::Dns { name: String::new(), record: default_record() },
        "agent" => NodeKind::Agent {
            goal: String::new(),
            hosts: HostPick::default(),
            local: false,
            ask_first: true,
            max_steps: AGENT_STEPS,
            max_tokens: AGENT_TOKENS,
            computer: false,
            apps: Vec::new(),
        },
        _ => return None,
    })
}

/// Whether `node` acts on text an AI wrote in this run, like
/// `{{nodes.summary.text}}` from a Write step, with no Ask me step between
/// that the user answered first.
pub fn runs_ai_text(automation: &Automation, node: &Node) -> bool {
    let writers: BTreeSet<&str> = node
        .kind
        .sink_texts()
        .iter()
        .flat_map(|text| crate::values::paths(text))
        .filter_map(|path| {
            let mut parts = path.split('.');
            (parts.next() == Some("nodes")).then(|| parts.next()).flatten().map(str::to_owned)
        })
        .filter_map(|id| automation.node(&id).filter(|from| from.kind.writes_ai_text()).map(|from| from.id.as_str()))
        .collect();
    if writers.is_empty() {
        return false;
    }
    // Walk back from the step, stopping at Ask me steps: a writer reached
    // this way gets to the command without the user seeing it.
    let mut seen = BTreeSet::new();
    let mut look = vec![node.id.as_str()];
    while let Some(at) = look.pop() {
        for edge in automation.edges.iter().filter(|edge| edge.to == at) {
            let Some(from) = automation.node(&edge.from) else { continue };
            if matches!(from.kind, NodeKind::Ask { .. }) || !seen.insert(from.id.as_str()) {
                continue;
            }
            if writers.contains(from.id.as_str()) {
                return true;
            }
            look.push(from.id.as_str());
        }
    }
    false
}

/// Every text field of a step, to find the values it uses.
pub fn texts(node: &Node) -> Vec<String> {
    fn walk(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::String(text) => out.push(text.clone()),
            serde_json::Value::Array(items) => items.iter().for_each(|item| walk(item, out)),
            serde_json::Value::Object(fields) => fields.values().for_each(|field| walk(field, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Ok(value) = serde_json::to_value(&node.kind) {
        walk(&value, &mut out);
    }
    out
}

/// The vault values a flow uses, like `DB_PASSWORD` for `{{vault.DB_PASSWORD}}`.
pub fn vault_names(automation: &Automation) -> BTreeSet<String> {
    automation
        .nodes
        .iter()
        .flat_map(texts)
        .flat_map(|text| crate::values::paths(&text))
        .filter_map(|path| path.strip_prefix("vault.").map(str::to_owned))
        .collect()
}

/// An id not used yet, from `base`, like `check`, `check_2`.
pub fn free_id(automation: &Automation, base: &str) -> String {
    let base: String = base
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_owned();
    let base = if base.is_empty() { "step".to_owned() } else { base };
    if automation.node(&base).is_none() {
        return base;
    }
    (2..).map(|n| format!("{base}_{n}")).find(|id| automation.node(id).is_none()).unwrap_or(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"{
      "schema": 1,
      "name": "Disk check",
      "start": { "schedule": "0 * * * *", "missed": "run_once" },
      "nodes": [
        { "id": "check", "type": "command", "at": [0, 0], "hosts": { "tag": "web" }, "command": "df -h /" },
        { "id": "full", "type": "if", "at": [0, 140], "value": "{{nodes.check.output}}", "test": "matches", "against": "(9[0-9]|100)%" },
        { "id": "tell", "type": "notify", "at": [0, 280], "title": "Disk", "body": "{{nodes.check.output}}" }
      ],
      "edges": [
        { "from": "check", "to": "full" },
        { "from": "full", "port": "yes", "to": "tell" }
      ]
    }"#;

    #[test]
    fn the_format_reads_and_writes_back_the_same() {
        let automation = Automation::from_json(EXAMPLE).unwrap();
        assert_eq!(automation.nodes.len(), 3);
        assert!(matches!(&automation.nodes[0].kind, NodeKind::Command { hosts, .. } if hosts.tag.as_deref() == Some("web")));
        assert_eq!(automation.start.schedule.as_deref(), Some("0 * * * *"));
        assert!(problems(&automation).is_empty(), "{:?}", problems(&automation));
        let again = Automation::from_json(&automation.to_json()).unwrap();
        assert_eq!(again, automation);
    }

    #[test]
    fn a_newer_format_is_refused_plainly() {
        let newer = EXAMPLE.replace("\"schema\": 1", "\"schema\": 9");
        assert!(Automation::from_json(&newer).unwrap_err().contains("newer Somme"));
    }

    #[test]
    fn problems_are_found() {
        let mut automation = Automation::from_json(EXAMPLE).unwrap();
        automation.edges.push(Edge::new("tell", "check"));
        assert_eq!(problems(&automation), ["The steps go round in a loop."]);
        automation.edges.pop();
        automation.edges.push(Edge::on("check", "maybe", "tell"));
        automation.edges.push(Edge::new("check", "gone"));
        let found = problems(&automation);
        assert!(found.iter().any(|p| p.contains("no \u{201c}maybe\u{201d}")), "{found:?}");
        assert!(found.iter().any(|p| p.contains("missing step")), "{found:?}");
    }

    #[test]
    fn ai_steps_read_and_check() {
        let text = r#"{ "schema": 1, "name": "Health", "nodes": [
            { "id": "check", "type": "local", "command": "df -h /" },
            { "id": "bad", "type": "decide", "yes_no": "Is a disk nearly full?", "about": "{{nodes.check.output}}" },
            { "id": "say", "type": "write", "prompt": "Say what is full:\n{{nodes.check.output}}" },
            { "id": "fix", "type": "local", "command": "echo {{nodes.say.text}}" }
        ], "edges": [
            { "from": "check", "to": "bad" }, { "from": "bad", "port": "yes", "to": "say" }, { "from": "say", "to": "fix" }
        ] }"#;
        let mut automation = Automation::from_json(text).unwrap();
        assert!(matches!(automation.nodes[1].kind, NodeKind::Decide { yes_from, no_to, .. } if yes_from == 0.8 && no_to == 0.2));
        assert_eq!(automation.nodes[1].kind.ports(), [YES, NO, UNSURE]);
        assert!(problems(&automation).is_empty(), "{:?}", problems(&automation));
        assert!(runs_ai_text(&automation, &automation.nodes[3]));
        assert!(!runs_ai_text(&automation, &automation.nodes[0]));
        automation.nodes[3].ai_wrote = true;
        assert!(problems(&automation)[0].contains("Looks right"));
        automation.nodes[3].ai_wrote = false;
        if let NodeKind::Decide { no_to, .. } = &mut automation.nodes[1].kind {
            *no_to = 0.9;
        }
        assert!(problems(&automation)[0].contains("sure no"));
    }

    #[test]
    fn an_ask_me_step_between_lets_ai_text_run() {
        let text = r#"{ "schema": 1, "name": "Fix", "nodes": [
            { "id": "say", "type": "write", "prompt": "a fix" },
            { "id": "ok", "type": "ask", "title": "Run this?", "message": "{{nodes.say.text}}" },
            { "id": "fix", "type": "local", "command": "{{nodes.say.text}} && echo {{vault.TOKEN}}" }
        ], "edges": [ { "from": "say", "to": "ok" }, { "from": "ok", "port": "allowed", "to": "fix" } ] }"#;
        let mut automation = Automation::from_json(text).unwrap();
        assert!(problems(&automation).is_empty(), "{:?}", problems(&automation));
        assert!(!runs_ai_text(&automation, &automation.nodes[2]));
        assert_eq!(vault_names(&automation).into_iter().collect::<Vec<_>>(), ["TOKEN"]);
        // A second way in that skips the question.
        automation.edges.push(Edge::new("say", "fix"));
        assert!(runs_ai_text(&automation, &automation.nodes[2]));
    }

    #[test]
    fn ai_text_sent_or_copied_needs_an_ok_too() {
        let text = r#"{ "schema": 1, "name": "Send", "nodes": [
            { "id": "say", "type": "write", "prompt": "a place" },
            { "id": "get", "type": "http", "url": "https://example.test/?q={{nodes.say.text}}" },
            { "id": "up", "type": "copy", "host": { "names": ["web"] }, "direction": "upload", "local": "~/a", "remote": "{{nodes.say.text}}" },
            { "id": "mx", "type": "dns", "name": "{{nodes.say.text}}.example.test" },
            { "id": "tell", "type": "notify", "title": "{{nodes.say.text}}" }
        ], "edges": [ { "from": "say", "to": "get" }, { "from": "say", "to": "up" }, { "from": "say", "to": "mx" }, { "from": "say", "to": "tell" } ] }"#;
        let automation = Automation::from_json(text).unwrap();
        let gated: Vec<bool> = automation.nodes.iter().map(|node| runs_ai_text(&automation, node)).collect();
        assert_eq!(gated, [false, true, true, true, false]);
    }

    #[test]
    fn network_steps_read_with_defaults() {
        let text = r#"{ "schema": 1, "name": "Web", "nodes": [
            { "id": "get", "type": "http", "url": "https://example.com/health" },
            { "id": "cert", "type": "tls", "host": "example.com" },
            { "id": "mx", "type": "dns", "name": "example.com", "record": "mx" },
            { "id": "up", "type": "copy", "host": { "names": ["web"] }, "direction": "download", "local": "~/a", "remote": "/a" }
        ] }"#;
        let automation = Automation::from_json(text).unwrap();
        assert!(matches!(&automation.nodes[0].kind, NodeKind::Http { method, .. } if method == "GET"));
        assert!(matches!(automation.nodes[1].kind, NodeKind::Tls { port: 443, .. }));
        assert!(problems(&automation).is_empty(), "{:?}", problems(&automation));
    }

    #[test]
    fn ids_stay_unique() {
        let automation = Automation::from_json(EXAMPLE).unwrap();
        assert_eq!(free_id(&automation, "Check"), "check_2");
        assert_eq!(free_id(&automation, "Back up!"), "back_up");
    }
}
