//! The panel beside the canvas. Settings: the picked step's form, the picked
//! line, or with nothing picked the flow's name, inputs and problems. Runs:
//! start a run, the past runs, and what each step of the shown run said.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::IndexPath;
use gpui_kit::component::combobox::{Combobox, ComboboxEvent, ComboboxState};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use somme_automation::dry;
use somme_automation::history::StepStatus;
use somme_automation::schema::{self, Direction, HostPick, Input as FlowInput, Missed, Test, problems};
use somme_automation::{Automation, Node, NodeKind, RunStatus};

use super::canvas::{kind_icon, took};
use super::editor::{FlowEditor, Picked, run_words};
use crate::automations::Automations;
use crate::settings::Settings;
use crate::store::{Store, mutate};
use crate::ui::common::{Lucide, ago, ink_button};
use crate::ui::fields::{Choice, ChoiceState, choice_select, selected};

/// The fields of the picked step.
pub(super) struct StepForm {
    id: String,
    label: Entity<InputState>,
    fields: Fields,
    timeout: Entity<InputState>,
    retries: Entity<InputState>,
    retry_wait: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

/// Saved servers to pick from, by id; more than one may be picked.
type ServersState = Entity<ComboboxState<SearchableVec<Choice<String>>>>;
/// Saved scripts to start a command from, by id.
type ScriptsState = Entity<SelectState<SearchableVec<Choice<String>>>>;

enum Fields {
    Command { command: Entity<TextareaState>, scripts: ScriptsState, servers: ServersState, tag: Entity<InputState> },
    Local { command: Entity<TextareaState>, scripts: ScriptsState, dir: Entity<InputState> },
    If { value: Entity<InputState>, test: ChoiceState<Test>, against: Entity<InputState> },
    Wait { seconds: Entity<InputState> },
    Notify { title: Entity<InputState>, body: Entity<TextareaState> },
    Decide { question: Entity<TextareaState>, about: Entity<TextareaState>, yes_from: Entity<InputState>, no_to: Entity<InputState> },
    Write { prompt: Entity<TextareaState> },
    Copy { server: ChoiceState<Option<String>>, direction: ChoiceState<Direction>, local: Entity<InputState>, remote: Entity<InputState> },
    Ask { title: Entity<InputState>, message: Entity<TextareaState> },
    Http { method: ChoiceState<String>, url: Entity<InputState>, headers: Entity<TextareaState>, body: Entity<TextareaState> },
    Tls { host: Entity<InputState>, port: Entity<InputState> },
    Dns { name: Entity<InputState>, record: ChoiceState<String> },
    Agent {
        goal: Entity<TextareaState>,
        servers: ServersState,
        tag: Entity<InputState>,
        max_steps: Entity<InputState>,
        max_tokens: Entity<InputState>,
        apps: Entity<InputState>,
    },
}

const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"];

/// Choices from plain words, each its own label.
fn word_choices(words: &[&str]) -> Vec<Choice<String>> {
    words.iter().map(|word| Choice { value: (*word).to_owned(), label: (*word).into() }).collect()
}

/// A chance as a whole percent, for a field.
fn percent(chance: f64) -> String {
    format!("{}", (chance * 100.0).round())
}

fn line(text: impl Into<String>, placeholder: &str, window: &mut Window, cx: &mut Context<FlowEditor>) -> Entity<InputState> {
    let (text, placeholder) = (text.into(), placeholder.to_owned());
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(text))
}

fn lines(text: impl Into<String>, placeholder: &str, window: &mut Window, cx: &mut Context<FlowEditor>) -> Entity<TextareaState> {
    let (text, placeholder) = (text.into(), placeholder.to_owned());
    cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 14).placeholder(placeholder).default_value(text))
}

/// A dropdown of the saved servers, with the ones `hosts` names picked.
/// Picking changes the step at once.
fn servers_pick(
    step: &str,
    hosts: &HostPick,
    pick: fn(&mut NodeKind) -> Option<&mut HostPick>,
    subscriptions: &mut Vec<Subscription>,
    window: &mut Window,
    cx: &mut Context<FlowEditor>,
) -> ServersState {
    let mut saved: Vec<Choice<String>> =
        Store::global(cx).read(cx).hosts.iter().map(|host| Choice { value: host.id.to_string(), label: host.label.clone().into() }).collect();
    saved.sort_by_key(|choice| choice.label.to_lowercase());
    let picked: Vec<IndexPath> = saved
        .iter()
        .enumerate()
        .filter(|(_, choice)| hosts.ids.contains(&choice.value) || hosts.names.iter().any(|name| *name == choice.label.as_ref()))
        .map(|(ix, _)| IndexPath::new(ix))
        .collect();
    let servers = cx.new(|cx| ComboboxState::new(SearchableVec::new(saved), picked, window, cx).multiple(true).searchable(true));
    let step = step.to_owned();
    subscriptions.push(cx.subscribe(&servers, move |editor, _, event: &ComboboxEvent<_>, cx| {
        let (ComboboxEvent::Change(ids) | ComboboxEvent::Confirm(ids)) = event;
        editor.set_servers(&step, ids, pick, cx);
    }));
    servers
}

/// A dropdown of the saved scripts. Picking one puts it in the command.
fn scripts_pick(subscriptions: &mut Vec<Subscription>, window: &mut Window, cx: &mut Context<FlowEditor>) -> ScriptsState {
    let scripts = cx.new(|cx| SelectState::new(SearchableVec::new(script_choices(cx)), None, window, cx).searchable(true));
    subscriptions.push(cx.subscribe_in(&scripts, window, |editor, _, event: &SelectEvent<_>, window, cx| {
        if let SelectEvent::Confirm(Some(id)) = event {
            editor.use_script(id, window, cx);
        }
    }));
    scripts
}

fn script_choices(cx: &gpui_kit::App) -> Vec<Choice<String>> {
    let mut scripts: Vec<Choice<String>> =
        Store::global(cx).read(cx).snippets.iter().map(|snippet| Choice { value: snippet.id.to_string(), label: snippet.label.clone().into() }).collect();
    scripts.sort_by_key(|choice| choice.label.to_lowercase());
    scripts
}

/// Puts what was typed into the draft on every change.
fn on_change<T>(input: &Entity<T>, apply: fn(&mut FlowEditor, &mut Context<FlowEditor>), cx: &mut Context<FlowEditor>) -> Subscription
where
    T: gpui_kit::EventEmitter<InputEvent> + 'static,
{
    cx.subscribe(input, move |editor, _, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
            apply(editor, cx);
        }
    })
}

impl StepForm {
    pub(super) fn new(node: &Node, window: &mut Window, cx: &mut Context<FlowEditor>) -> Self {
        let label = line(node.label.clone(), node.kind.name(), window, cx);
        let timeout = line(node.timeout_seconds.map(|s| s.to_string()).unwrap_or_default(), &node.kind.default_timeout().to_string(), window, cx);
        let retries = line(node.retries.to_string(), "0", window, cx);
        let retry_wait = line(node.retry_wait_seconds.to_string(), "0", window, cx);
        let apply = FlowEditor::apply_step_form;
        let mut subscriptions = vec![
            on_change(&label, apply, cx),
            on_change(&timeout, apply, cx),
            on_change(&retries, apply, cx),
            on_change(&retry_wait, apply, cx),
        ];
        let fields = match &node.kind {
            NodeKind::Command { hosts, command, .. } => {
                let command = lines(command.clone(), "df -h /", window, cx);
                let tag = line(hosts.tag.clone().unwrap_or_default(), "web", window, cx);
                subscriptions.extend([on_change(&command, apply, cx), on_change(&tag, apply, cx)]);
                let servers = servers_pick(&node.id, hosts, command_hosts, &mut subscriptions, window, cx);
                let scripts = scripts_pick(&mut subscriptions, window, cx);
                Fields::Command { command, scripts, servers, tag }
            }
            NodeKind::Local { command, dir } => {
                let command = lines(command.clone(), "./backup.sh", window, cx);
                let dir = line(dir.clone().unwrap_or_default(), "~", window, cx);
                subscriptions.extend([on_change(&command, apply, cx), on_change(&dir, apply, cx)]);
                let scripts = scripts_pick(&mut subscriptions, window, cx);
                Fields::Local { command, scripts, dir }
            }
            NodeKind::If { value, test, against } => {
                let value = line(value.clone(), "{{nodes.check.output}}", window, cx);
                let against = line(against.clone(), "", window, cx);
                let choices = Test::ALL.iter().map(|test| Choice { value: *test, label: test.words().into() }).collect();
                let test = choice_select(choices, test, window, cx);
                subscriptions.extend([on_change(&value, apply, cx), on_change(&against, apply, cx)]);
                subscriptions.push(cx.subscribe(&test, |editor, _, _: &SelectEvent<_>, cx| editor.apply_step_form(cx)));
                Fields::If { value, test, against }
            }
            NodeKind::Wait { seconds } => {
                let seconds = line(seconds.to_string(), "10", window, cx);
                subscriptions.push(on_change(&seconds, apply, cx));
                Fields::Wait { seconds }
            }
            NodeKind::Notify { title, body } => {
                let title = line(title.clone(), "Backup done", window, cx);
                let body = lines(body.clone(), "{{nodes.check.output}}", window, cx);
                subscriptions.extend([on_change(&title, apply, cx), on_change(&body, apply, cx)]);
                Fields::Notify { title, body }
            }
            NodeKind::Decide { yes_no, about, yes_from, no_to } => {
                let question = lines(yes_no.clone(), "Is any server unhealthy: disk over 90%, or failed services?", window, cx);
                let about = lines(about.clone(), "{{nodes.check.output}}", window, cx);
                let yes_from = line(percent(*yes_from), "80", window, cx);
                let no_to = line(percent(*no_to), "20", window, cx);
                subscriptions.extend([
                    on_change(&question, apply, cx),
                    on_change(&about, apply, cx),
                    on_change(&yes_from, apply, cx),
                    on_change(&no_to, apply, cx),
                ]);
                Fields::Decide { question, about, yes_from, no_to }
            }
            NodeKind::Write { prompt } => {
                let prompt = lines(prompt.clone(), "Say in 3 short lines what is wrong:\n{{nodes.check.output}}", window, cx);
                subscriptions.push(on_change(&prompt, apply, cx));
                Fields::Write { prompt }
            }
            NodeKind::Copy { host, direction, local, remote } => {
                let mut servers: Vec<Choice<Option<String>>> =
                    Store::global(cx).read(cx).hosts.iter().map(|saved| Choice { value: Some(saved.id.to_string()), label: saved.label.clone().into() }).collect();
                servers.sort_by_key(|choice| choice.label.to_lowercase());
                let current = host.ids.first().cloned().or_else(|| {
                    let name = host.names.first()?;
                    servers.iter().find(|choice| choice.label.as_ref() == name.as_str()).and_then(|choice| choice.value.clone())
                });
                let server = choice_select(servers, &current, window, cx);
                let step = node.id.clone();
                subscriptions.push(cx.subscribe(&server, move |editor, _, event: &SelectEvent<_>, cx| {
                    if let SelectEvent::Confirm(picked) = event
                        && let Some(NodeKind::Copy { host, .. }) = editor.step_mut(&step).map(|node| &mut node.kind)
                    {
                        *host = HostPick { ids: picked.clone().flatten().into_iter().collect(), ..HostPick::default() };
                        editor.changed(cx);
                    }
                }));
                let choices = vec![
                    Choice { value: Direction::Upload, label: "From this computer to the server".into() },
                    Choice { value: Direction::Download, label: "From the server to this computer".into() },
                ];
                let direction = choice_select(choices, direction, window, cx);
                let local = line(local.clone(), "~/backups/db.sql.gz", window, cx);
                let remote = line(remote.clone(), "/var/backups/db.sql.gz", window, cx);
                subscriptions.extend([on_change(&local, apply, cx), on_change(&remote, apply, cx)]);
                subscriptions.push(cx.subscribe(&direction, |editor, _, _: &SelectEvent<_>, cx| editor.apply_step_form(cx)));
                Fields::Copy { server, direction, local, remote }
            }
            NodeKind::Ask { title, message } => {
                let title = line(title.clone(), "Restart nginx on web-1?", window, cx);
                let message = lines(message.clone(), "{{nodes.fix.text}}", window, cx);
                subscriptions.extend([on_change(&title, apply, cx), on_change(&message, apply, cx)]);
                Fields::Ask { title, message }
            }
            NodeKind::Http { method, url, headers, body } => {
                let method = choice_select(word_choices(METHODS), &method.to_uppercase(), window, cx);
                let url = line(url.clone(), "https://example.com/health", window, cx);
                let headers = lines(headers.clone(), "Authorization: Bearer {{vault.API_TOKEN}}", window, cx);
                let body = lines(body.clone(), "", window, cx);
                subscriptions.extend([on_change(&url, apply, cx), on_change(&headers, apply, cx), on_change(&body, apply, cx)]);
                subscriptions.push(cx.subscribe(&method, |editor, _, _: &SelectEvent<_>, cx| editor.apply_step_form(cx)));
                Fields::Http { method, url, headers, body }
            }
            NodeKind::Tls { host, port } => {
                let host = line(host.clone(), "example.com", window, cx);
                let port = line(port.to_string(), "443", window, cx);
                subscriptions.extend([on_change(&host, apply, cx), on_change(&port, apply, cx)]);
                Fields::Tls { host, port }
            }
            NodeKind::Dns { name, record } => {
                let record = choice_select(word_choices(somme_automation::net::RECORDS), &record.to_uppercase(), window, cx);
                let name = line(name.clone(), "example.com", window, cx);
                subscriptions.push(on_change(&name, apply, cx));
                subscriptions.push(cx.subscribe(&record, |editor, _, _: &SelectEvent<_>, cx| editor.apply_step_form(cx)));
                Fields::Dns { name, record }
            }
            NodeKind::Agent { goal, hosts, max_steps, max_tokens, apps, .. } => {
                let goal = lines(goal.clone(), "Find why nginx answers 502 on web-1 and fix it.", window, cx);
                let apps = line(apps.join(", "), "Safari, Notes", window, cx);
                let tag = line(hosts.tag.clone().unwrap_or_default(), "web", window, cx);
                let max_steps = line(max_steps.to_string(), &schema::AGENT_STEPS.to_string(), window, cx);
                let max_tokens = line(max_tokens.to_string(), &schema::AGENT_TOKENS.to_string(), window, cx);
                subscriptions.extend([
                    on_change(&goal, apply, cx),
                    on_change(&tag, apply, cx),
                    on_change(&max_steps, apply, cx),
                    on_change(&max_tokens, apply, cx),
                    on_change(&apps, apply, cx),
                ]);
                let servers = servers_pick(&node.id, hosts, agent_hosts, &mut subscriptions, window, cx);
                Fields::Agent { goal, servers, tag, max_steps, max_tokens, apps }
            }
        };
        Self { id: node.id.clone(), label, fields, timeout, retries, retry_wait, _subscriptions: subscriptions }
    }
}

/// Schedules offered with a click.
const SCHEDULES: &[(&str, &str)] = &[
    ("Every hour", "0 * * * *"),
    ("Every day at 9:00", "0 9 * * *"),
    ("Weekdays at 9:00", "0 9 * * 1-5"),
    ("Mondays at 9:00", "0 9 * * 1"),
];

/// The flow's own fields: name, description, schedule and inputs.
pub(super) struct StartForm {
    name: Entity<InputState>,
    description: Entity<TextareaState>,
    schedule: Entity<InputState>,
    missed: ChoiceState<Missed>,
    /// Name and default value of each input.
    inputs: Vec<(Entity<InputState>, Entity<InputState>)>,
    _subscriptions: Vec<Subscription>,
}

impl StartForm {
    pub(super) fn new(automation: &Automation, window: &mut Window, cx: &mut Context<FlowEditor>) -> Self {
        let apply = FlowEditor::apply_start_form;
        let name = line(automation.name.clone(), "Server health", window, cx);
        let description = lines(automation.description.clone(), "What it does, in a line", window, cx);
        let schedule = line(automation.start.schedule.clone().unwrap_or_default(), "Only by hand, like 0 9 * * 1-5", window, cx);
        let missed = choice_select(
            vec![
                Choice { value: Missed::RunOnce, label: "Run once when it wakes".into() },
                Choice { value: Missed::Skip, label: "Skip it".into() },
            ],
            &automation.start.missed,
            window,
            cx,
        );
        let mut subscriptions = vec![on_change(&name, apply, cx), on_change(&description, apply, cx), on_change(&schedule, apply, cx)];
        subscriptions.push(cx.subscribe(&missed, |editor, _, _: &SelectEvent<_>, cx| editor.apply_start_form(cx)));
        let inputs = automation
            .inputs
            .iter()
            .map(|input| {
                let key = line(input.name.clone(), "branch", window, cx);
                let default = line(input.default.clone(), "main", window, cx);
                subscriptions.extend([on_change(&key, apply, cx), on_change(&default, apply, cx)]);
                (key, default)
            })
            .collect();
        Self { name, description, schedule, missed, inputs, _subscriptions: subscriptions }
    }
}

fn number<T: std::str::FromStr>(input: &Entity<InputState>, cx: &gpui_kit::App) -> Option<T> {
    input.read(cx).value().trim().parse().ok()
}

fn text(input: &Entity<InputState>, cx: &gpui_kit::App) -> String {
    input.read(cx).value().to_string()
}

fn area(input: &Entity<TextareaState>, cx: &gpui_kit::App) -> String {
    input.read(cx).value().to_string()
}

impl FlowEditor {
    pub(super) fn apply_step_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.step_form else { return };
        let id = form.id.clone();
        let label = text(&form.label, cx).trim().to_owned();
        let timeout = text(&form.timeout, cx);
        let timeout = if timeout.trim().is_empty() { Some(None) } else { number::<u64>(&form.timeout, cx).map(Some) };
        let retries = number::<u32>(&form.retries, cx);
        let retry_wait = number::<u64>(&form.retry_wait, cx);
        enum Typed {
            Command(String, String),
            Local(String, String),
            If(String, Option<Test>, String),
            Wait(Option<u64>),
            Notify(String, String),
            Decide(String, String, Option<f64>, Option<f64>),
            Write(String),
            Copy(Option<Direction>, String, String),
            Ask(String, String),
            Http(Option<String>, String, String, String),
            Tls(String, Option<u16>),
            Dns(String, Option<String>),
            Agent(String, String, Option<u32>, Option<u64>, String),
        }
        let typed = match &form.fields {
            Fields::Command { command, tag, .. } => Typed::Command(area(command, cx), text(tag, cx)),
            Fields::Local { command, dir, .. } => Typed::Local(area(command, cx), text(dir, cx)),
            Fields::If { value, test, against } => Typed::If(text(value, cx), selected(test, cx), text(against, cx)),
            Fields::Wait { seconds } => Typed::Wait(number(seconds, cx)),
            Fields::Notify { title, body } => Typed::Notify(text(title, cx), area(body, cx)),
            Fields::Decide { question, about, yes_from, no_to } => Typed::Decide(
                area(question, cx),
                area(about, cx),
                number::<f64>(yes_from, cx).map(|p| p / 100.0),
                number::<f64>(no_to, cx).map(|p| p / 100.0),
            ),
            Fields::Write { prompt } => Typed::Write(area(prompt, cx)),
            Fields::Copy { direction, local, remote, .. } => Typed::Copy(selected(direction, cx), text(local, cx), text(remote, cx)),
            Fields::Ask { title, message } => Typed::Ask(text(title, cx), area(message, cx)),
            Fields::Http { method, url, headers, body } => Typed::Http(selected(method, cx), text(url, cx), area(headers, cx), area(body, cx)),
            Fields::Tls { host, port } => Typed::Tls(text(host, cx), number(port, cx)),
            Fields::Dns { name, record } => Typed::Dns(text(name, cx), selected(record, cx)),
            Fields::Agent { goal, tag, max_steps, max_tokens, apps, .. } => {
                Typed::Agent(area(goal, cx), text(tag, cx), number(max_steps, cx), number(max_tokens, cx), text(apps, cx))
            }
        };
        let Some(node) = self.step_mut(&id) else { return };
        node.label = label;
        if let Some(timeout) = timeout {
            node.timeout_seconds = timeout;
        }
        if let Some(retries) = retries {
            node.retries = retries.min(10);
        }
        if let Some(wait) = retry_wait {
            node.retry_wait_seconds = wait;
        }
        let before = node.kind.command().map(str::to_owned);
        match (&mut node.kind, typed) {
            (NodeKind::Command { hosts, command, .. }, Typed::Command(typed, tag)) => {
                *command = typed;
                hosts.tag = Some(tag.trim().to_owned()).filter(|tag| !tag.is_empty());
            }
            (NodeKind::Local { command, dir }, Typed::Local(typed, folder)) => {
                *command = typed;
                *dir = Some(folder.trim().to_owned()).filter(|dir| !dir.is_empty());
            }
            (NodeKind::If { value, test, against }, Typed::If(typed, picked, other)) => {
                *value = typed;
                if let Some(picked) = picked {
                    *test = picked;
                }
                *against = other;
            }
            (NodeKind::Wait { seconds }, Typed::Wait(Some(typed))) => *seconds = typed,
            (NodeKind::Notify { title, body }, Typed::Notify(typed, message)) => {
                *title = typed;
                *body = message;
            }
            (NodeKind::Decide { yes_no, about, yes_from, no_to }, Typed::Decide(question, data, from, to)) => {
                *yes_no = question;
                *about = data;
                if let Some(from) = from {
                    *yes_from = from;
                }
                if let Some(to) = to {
                    *no_to = to;
                }
            }
            (NodeKind::Write { prompt }, Typed::Write(typed)) => *prompt = typed,
            (NodeKind::Copy { direction, local, remote, .. }, Typed::Copy(picked, here, there)) => {
                if let Some(picked) = picked {
                    *direction = picked;
                }
                *local = here.trim().to_owned();
                *remote = there.trim().to_owned();
            }
            (NodeKind::Ask { title, message }, Typed::Ask(typed, details)) => {
                *title = typed;
                *message = details;
            }
            (NodeKind::Http { method, url, headers, body }, Typed::Http(picked, address, typed_headers, typed_body)) => {
                if let Some(picked) = picked {
                    *method = picked;
                }
                *url = address.trim().to_owned();
                *headers = typed_headers;
                *body = typed_body;
            }
            (NodeKind::Tls { host, port }, Typed::Tls(typed, number)) => {
                *host = typed.trim().to_owned();
                if let Some(number) = number.filter(|port| *port > 0) {
                    *port = number;
                }
            }
            (NodeKind::Dns { name, record }, Typed::Dns(typed, picked)) => {
                *name = typed.trim().to_owned();
                if let Some(picked) = picked {
                    *record = picked;
                }
            }
            (NodeKind::Agent { goal, hosts, max_steps, max_tokens, apps, .. }, Typed::Agent(typed, tag, steps, tokens, typed_apps)) => {
                *goal = typed;
                *apps = typed_apps.split([',', '\n']).map(str::trim).filter(|app| !app.is_empty()).map(str::to_owned).collect();
                hosts.tag = Some(tag.trim().to_owned()).filter(|tag| !tag.is_empty());
                if let Some(steps) = steps {
                    *max_steps = steps.clamp(1, schema::MOST_AGENT_STEPS);
                }
                if let Some(tokens) = tokens {
                    *max_tokens = tokens.max(1000);
                }
            }
            _ => {}
        }
        // Changing a command the AI wrote counts as checking it.
        if node.kind.command().map(str::to_owned) != before {
            node.ai_wrote = false;
        }
        self.changed(cx);
    }

    pub(super) fn apply_start_form(&mut self, cx: &mut Context<Self>) {
        let form = &self.start_form;
        let name = text(&form.name, cx);
        let description = area(&form.description, cx);
        let typed: Vec<(String, String)> = form.inputs.iter().map(|(key, default)| (text(key, cx), text(default, cx))).collect();
        let schedule = text(&form.schedule, cx);
        let missed = selected(&form.missed, cx);
        self.draft.name = if name.trim().is_empty() { "Untitled".to_owned() } else { name.trim().to_owned() };
        self.draft.description = description;
        self.draft.start.schedule = Some(schedule.trim().to_owned()).filter(|schedule| !schedule.is_empty());
        if let Some(missed) = missed {
            self.draft.start.missed = missed;
        }
        for (input, (key, default)) in self.draft.inputs.iter_mut().zip(typed) {
            input.name = key.trim().to_owned();
            input.default = default;
        }
        self.changed(cx);
    }

    fn add_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let taken = |name: &str| self.draft.inputs.iter().any(|input| input.name == name);
        let name = (1..).map(|n| if n == 1 { "value".to_owned() } else { format!("value_{n}") }).find(|name| !taken(name)).unwrap_or_default();
        self.draft.inputs.push(FlowInput { name, label: String::new(), default: String::new() });
        self.start_form = StartForm::new(&self.draft, window, cx);
        self.changed(cx);
    }

    fn remove_input(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.draft.inputs.len() {
            self.draft.inputs.remove(ix);
            self.start_form = StartForm::new(&self.draft, window, cx);
            self.changed(cx);
        }
    }

    /// The servers picked in a step's dropdown.
    fn set_servers(&mut self, step: &str, ids: &[String], pick: fn(&mut NodeKind) -> Option<&mut HostPick>, cx: &mut Context<Self>) {
        let saved: Vec<String> = Store::global(cx).read(cx).hosts.iter().map(|host| host.label.clone()).collect();
        let Some(hosts) = self.step_mut(step).and_then(|node| pick(&mut node.kind)) else { return };
        if hosts.ids == ids {
            return;
        }
        hosts.ids = ids.to_vec();
        // Saved servers named by name are in the dropdown now, by id.
        hosts.names.retain(|name| !saved.contains(name));
        self.changed(cx);
    }

    /// Puts a saved script into the step's command.
    fn use_script(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(script) = Store::global(cx).read(cx).snippets.iter().find(|snippet| snippet.id.to_string() == id).map(|snippet| snippet.script.clone()) else { return };
        let Some(form) = &self.step_form else { return };
        let (Fields::Command { command, .. } | Fields::Local { command, .. }) = &form.fields else { return };
        command.update(cx, |input, cx| input.set_value(script, window, cx));
        self.script_note = None;
        self.apply_step_form(cx);
    }

    /// Saves the step's command as a new script, named after the step.
    fn save_script(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.step_form else { return };
        let (Fields::Command { command, scripts, .. } | Fields::Local { command, scripts, .. }) = &form.fields else { return };
        let script = command.read(cx).value().trim_end().to_owned();
        if script.trim().is_empty() {
            self.script_note = Some((false, "Write the command first.".into()));
            cx.notify();
            return;
        }
        let scripts = scripts.clone();
        let mut label = form.label.read(cx).value().trim().to_owned();
        if label.is_empty() {
            label = script.lines().next().unwrap_or_default().chars().take(40).collect();
        }
        let input = somme_application::dto::SnippetInput { id: None, label: label.clone(), script };
        mutate(
            window,
            cx,
            move |services| async move { services.snippets.save(input).await },
            move |editor, result, window, cx| {
                editor.script_note = Some(match result {
                    Ok(_) => (true, format!("Saved as the script \u{201c}{label}\u{201d}.")),
                    Err(err) => (false, format!("Could not save the script: {err}")),
                });
                // The list reloads a moment later; show the new script once it does.
                cx.spawn_in(window, async move |_, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
                    let _ = cx.update(|window, cx| {
                        let choices = script_choices(cx);
                        scripts.update(cx, |state, cx| state.set_items(SearchableVec::new(choices), window, cx));
                    });
                })
                .detach();
                cx.notify();
            },
        );
    }

    /// Puts `{{…}}` into the step's main field, where the cursor is.
    fn insert_value(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.step_form else { return };
        match &form.fields {
            Fields::Command { command, .. } | Fields::Local { command, .. } => {
                command.update(cx, |input, cx| {
                    input.focus(window, cx);
                    input.insert(value, window, cx);
                });
            }
            Fields::Notify { body, .. } => body.update(cx, |input, cx| {
                input.focus(window, cx);
                input.insert(value, window, cx);
            }),
            Fields::If { value: field, .. } => field.update(cx, |input, cx| {
                input.focus(window, cx);
                input.insert(value, window, cx);
            }),
            Fields::Decide { about: field, .. } | Fields::Write { prompt: field } | Fields::Ask { message: field, .. } | Fields::Agent { goal: field, .. } => field.update(cx, |input, cx| {
                input.focus(window, cx);
                input.insert(value, window, cx);
            }),
            Fields::Copy { remote: field, .. } | Fields::Http { url: field, .. } | Fields::Tls { host: field, .. } | Fields::Dns { name: field, .. } => {
                field.update(cx, |input, cx| {
                    input.focus(window, cx);
                    input.insert(value, window, cx);
                })
            }
            Fields::Wait { .. } => {}
        }
    }

    /// The values a step can use: the flow's inputs and every step before it.
    fn values_for(&self, id: &str) -> Vec<String> {
        let mut values: Vec<String> = self.draft.inputs.iter().map(|input| format!("{{{{input.{}}}}}", input.name)).collect();
        let mut before = Vec::new();
        let mut look = vec![id.to_owned()];
        while let Some(at) = look.pop() {
            for edge in self.draft.edges.iter().filter(|edge| edge.to == at) {
                if !before.contains(&edge.from) {
                    before.push(edge.from.clone());
                    look.push(edge.from.clone());
                }
            }
        }
        values.extend(before.iter().map(|step| format!("{{{{nodes.{step}.output}}}}")));
        values
    }

    /// The vault values the flow uses that aren't saved.
    fn missing_vault(&self, cx: &gpui_kit::App) -> Vec<String> {
        let Some(saved) = Automations::global(cx).read(cx).vault_names.clone() else { return Vec::new() };
        schema::vault_names(&self.draft).into_iter().filter(|name| !saved.contains(name)).collect()
    }

    pub(super) fn render_settings(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.picked.clone() {
            Picked::Step(id) if self.step_form.is_some() => match self.draft.node(&id).cloned() {
                Some(node) => self.render_step(&node, cx),
                None => self.render_start(cx),
            },
            Picked::Line(ix) => self.render_line(ix, cx),
            _ => self.render_start(cx),
        }
    }

    fn render_line(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(edge) = self.draft.edges.get(ix) else { return div().into_any_element() };
        let title = |id: &str| self.draft.node(id).map_or_else(|| id.to_owned(), |node| node.title());
        let port = self.draft.node(&edge.from).map(|node| edge.port_of(&node.kind).to_owned()).unwrap_or_default();
        v_flex()
            .p_3()
            .gap_3()
            .child(heading(Lucide::Spline, "Line", cx))
            .child(div().text_sm().child(format!(
                "When \u{201c}{}\u{201d} ends with {port}, \u{201c}{}\u{201d} runs next.",
                title(&edge.from),
                title(&edge.to)
            )))
            .child(
                Button::new("line-remove")
                    .outline()
                    .small()
                    .icon(Icon::new(Lucide::Trash))
                    .label("Remove line")
                    .on_click(cx.listener(|editor, _, window, cx| editor.remove_picked(window, cx))),
            )
            .into_any_element()
    }

    fn render_step(&self, node: &Node, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.step_form else { return div().into_any_element() };
        let theme = cx.theme().clone();
        let mono = Settings::get(cx).mono_font();
        let id = node.id.clone();
        let kind_fields: AnyElement = match (&form.fields, &node.kind) {
            (Fields::Command { command, scripts, servers, tag }, NodeKind::Command { one_by_one, sudo, .. }) => {
                let (step_a, step_b) = (id.clone(), id.clone());
                v_flex()
                    .gap_3()
                    .child(field("Servers", servers_field(servers, cx)))
                    .child(field("And every server tagged", Input::new(tag).small()))
                    .child(field("Command", self.command_field(command, scripts, &mono, cx)))
                    .child(toggle("step-one-by-one", "One server at a time, stopping at the first failure", *one_by_one, cx, move |editor, on| {
                        if let Some(NodeKind::Command { one_by_one, .. }) = editor.step_mut(&step_a).map(|node| &mut node.kind) {
                            *one_by_one = on;
                        }
                    }))
                    .child(toggle("step-sudo", "Run with sudo (works only without a password)", *sudo, cx, move |editor, on| {
                        if let Some(NodeKind::Command { sudo, .. }) = editor.step_mut(&step_b).map(|node| &mut node.kind) {
                            *sudo = on;
                        }
                    }))
                    .into_any_element()
            }
            (Fields::Local { command, scripts, dir }, _) => v_flex()
                .gap_3()
                .child(field("Command", self.command_field(command, scripts, &mono, cx)))
                .child(field("In folder", Input::new(dir).small()))
                .into_any_element(),
            (Fields::If { value, test, against }, NodeKind::If { test: picked, .. }) => v_flex()
                .gap_3()
                .child(field("Value", Input::new(value).small()))
                .child(field("Test", Select::new(test).small()))
                .when(picked.needs_against(), |form| form.child(field("Compare with", Input::new(against).small())))
                .child(note("Goes on by yes when the test passes, and by no when it doesn't.", cx))
                .into_any_element(),
            (Fields::Wait { seconds }, _) => field("Seconds", Input::new(seconds).small()).into_any_element(),
            (Fields::Notify { title, body }, _) => v_flex()
                .gap_3()
                .child(field("Title", Input::new(title).small()))
                .child(field("Message", Textarea::new(body)))
                .into_any_element(),
            (Fields::Decide { question, about, yes_from, no_to }, _) => v_flex()
                .gap_3()
                .child(field("Question (yes or no)", Textarea::new(question)))
                .child(field("About", div().font_family(mono.clone()).child(Textarea::new(about))))
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(field("Sure yes from (%)", Input::new(yes_from).small())))
                        .child(div().flex_1().child(field("Sure no up to (%)", Input::new(no_to).small()))),
                )
                .child(note("Jev answers first, then your LLM if Jev isn't sure. Goes on by yes, no, or unsure when neither is sure.", cx))
                .into_any_element(),
            (Fields::Write { prompt }, _) => v_flex()
                .gap_3()
                .child(field("What to write", Textarea::new(prompt)))
                .child(note("Your LLM writes it. A command that uses this text asks you before it runs, unless an Ask me step comes first.", cx))
                .into_any_element(),
            (Fields::Copy { server, direction, local, remote }, _) => {
                let picks: AnyElement = if Store::global(cx).read(cx).hosts.is_empty() {
                    note("No saved servers yet. Add one under SSH \u{2192} Hosts.", cx).into_any_element()
                } else {
                    Select::new(server).small().placeholder("Pick a server").search_placeholder("Find a server").into_any_element()
                };
                v_flex()
                    .gap_3()
                    .child(field("Server", picks))
                    .child(field("Copy", Select::new(direction).small()))
                    .child(field("File on this computer", div().font_family(mono.clone()).child(Input::new(local).small())))
                    .child(field("File on the server", div().font_family(mono.clone()).child(Input::new(remote).small())))
                    .child(note("Copies one file over SFTP and replaces what is there.", cx))
                    .into_any_element()
            }
            (Fields::Ask { title, message }, _) => v_flex()
                .gap_3()
                .child(field("Question", Input::new(title).small()))
                .child(field("Details", div().font_family(mono.clone()).child(Textarea::new(message))))
                .child(note("Somme asks you to Allow or Deny, then goes on by allowed or denied. A run that starts while Somme is closed stops here.", cx))
                .into_any_element(),
            (Fields::Http { method, url, headers, body }, _) => v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().w(px(104.)).child(field("Method", Select::new(method).small())))
                        .child(div().flex_1().child(field("Address", Input::new(url).small()))),
                )
                .child(field("Headers, one Name: value a line", div().font_family(mono.clone()).child(Textarea::new(headers))))
                .child(field("Body", div().font_family(mono.clone()).child(Textarea::new(body))))
                .child(note("Fails on an answer from 400 up. Later steps use its status, body, and json when the answer is JSON.", cx))
                .into_any_element(),
            (Fields::Tls { host, port }, _) => v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(field("Host", Input::new(host).small())))
                        .child(div().w(px(80.)).child(field("Port", Input::new(port).small()))),
                )
                .child(note("Gives days_left, expires, issuer and trusted. Check days_left with an If step.", cx))
                .into_any_element(),
            (Fields::Dns { name, record }, _) => v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(field("Name", Input::new(name).small())))
                        .child(div().w(px(96.)).child(field("Record", Select::new(record).small()))),
                )
                .child(note("Gives the records, one a line.", cx))
                .into_any_element(),
            (Fields::Agent { goal, servers, tag, max_steps, max_tokens, apps }, NodeKind::Agent { local, ask_first, computer, .. }) => {
                let (step_a, step_b, step_c) = (id.clone(), id.clone(), id.clone());
                v_flex()
                    .gap_3()
                    .child(field("Goal", Textarea::new(goal)))
                    .child(field("Servers it may use", servers_field(servers, cx)))
                    .child(field("And every server tagged", Input::new(tag).small()))
                    .child(toggle("step-agent-local", "It may use this computer too", *local, cx, move |editor, on| {
                        if let Some(NodeKind::Agent { local, .. }) = editor.step_mut(&step_a).map(|node| &mut node.kind) {
                            *local = on;
                        }
                    }))
                    .child(toggle("step-agent-screen", "It may use this computer's screen", *computer, cx, move |editor, on| {
                        if let Some(NodeKind::Agent { computer, .. }) = editor.step_mut(&step_c).map(|node| &mut node.kind) {
                            *computer = on;
                        }
                    }))
                    .when(*computer, |form| {
                        form.child(field("Apps it may use", Input::new(apps).small())).child(note(
                            "It sees the screen and clicks and types in these apps only, never in Somme. Screenshots go to your LLM and are not kept. A bar at the top shows when it happens; press Esc to stop.",
                            cx,
                        ))
                    })
                    .child(toggle("step-agent-ask", "Ask me before each command, change or click", *ask_first, cx, move |editor, on| {
                        if let Some(NodeKind::Agent { ask_first, .. }) = editor.step_mut(&step_b).map(|node| &mut node.kind) {
                            *ask_first = on;
                        }
                    }))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(field("Most tool calls", Input::new(max_steps).small())))
                            .child(div().flex_1().child(field("Most tokens", Input::new(max_tokens).small()))),
                    )
                    .child(note(
                        "Your LLM runs commands, reads files and changes them with patches, only on the places picked here. The guard checks each thing it does. What commands give goes to your LLM. Its answer is in answer, the files it changed in changed.",
                        cx,
                    ))
                    .into_any_element()
            }
            _ => div().into_any_element(),
        };
        let settings = Settings::get(cx);
        let ai_off = match node.kind {
            NodeKind::Decide { .. } => !crate::ai::allowed(settings, crate::ai::Feature::Decide),
            NodeKind::Write { .. } => !crate::ai::allowed(settings, crate::ai::Feature::Write),
            NodeKind::Agent { computer, .. } => {
                !crate::ai::allowed(settings, crate::ai::Feature::Agent) || (computer && !crate::ai::allowed(settings, crate::ai::Feature::Computer))
            }
            _ => false,
        };
        let warning = |text: &'static str| {
            h_flex()
                .gap_2()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.muted)
                .text_sm()
                .child(Icon::new(Lucide::Info).size(px(14.)).text_color(theme.muted_foreground))
                .child(div().flex_1().min_w_0().child(text))
        };
        let ai_off_box = ai_off.then(|| warning("AI or this kind of step is off, so it will fail. Turn it on in Settings \u{2192} AI."));
        let ai_wrote_box = (node.ai_wrote && node.kind.acts()).then(|| {
            let step = id.clone();
            v_flex()
                .gap_2()
                .child(warning("An AI set up this step. Read it before it runs."))
                .child(
                    Button::new("step-looks-right")
                        .small()
                        .icon(Icon::new(Lucide::Check))
                        .label("Looks right")
                        .on_click(cx.listener(move |editor, _, _, cx| {
                            if let Some(node) = editor.step_mut(&step) {
                                node.ai_wrote = false;
                            }
                            editor.changed(cx);
                        })),
                )
        });
        let mut values = self.values_for(&node.id);
        // Secrets go only into steps that don't send them to AI.
        if !node.kind.uses_ai() {
            let saved = Automations::global(cx).read(cx).vault_names.clone().unwrap_or_default();
            values.extend(saved.iter().map(|name| format!("{{{{vault.{name}}}}}")));
        }
        let can_insert = !matches!(form.fields, Fields::Wait { .. });
        let values_box = (can_insert && !values.is_empty()).then(|| {
            field(
                "Use a value",
                h_flex().flex_wrap().gap_1().children(values.into_iter().map(|value| {
                    let shown = value.clone();
                    div()
                        .id(SharedString::from(format!("value-{value}")))
                        .px_1p5()
                        .py_0p5()
                        .rounded_md()
                        .bg(theme.secondary)
                        .font_family(mono.clone())
                        .text_xs()
                        .cursor_pointer()
                        .hover(|chip| chip.bg(theme.secondary_hover))
                        .child(shown)
                        .on_click(cx.listener(move |editor, _, window, cx| editor.insert_value(value.clone(), window, cx)))
                })),
            )
        });
        let can_retry = matches!(
            node.kind,
            NodeKind::Command { .. } | NodeKind::Local { .. } | NodeKind::Copy { .. } | NodeKind::Http { .. } | NodeKind::Tls { .. } | NodeKind::Dns { .. }
        ) || node.kind.uses_ai();
        let used_as = match node.kind {
            NodeKind::Write { .. } => format!("{{{{nodes.{}.text}}}}", node.id),
            NodeKind::Decide { .. } | NodeKind::Ask { .. } | NodeKind::Agent { .. } => format!("{{{{nodes.{}.answer}}}}", node.id),
            NodeKind::Http { .. } => format!("{{{{nodes.{}.status}}}}", node.id),
            NodeKind::Tls { .. } => format!("{{{{nodes.{}.days_left}}}}", node.id),
            _ => format!("{{{{nodes.{}.output}}}}", node.id),
        };
        v_flex()
            .p_3()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(heading(kind_icon(&node.kind), node.kind.name(), cx))
                    .child(div().flex_1())
                    .child(
                        Button::new("step-remove")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::Trash))
                            .tooltip("Remove this step")
                            .on_click(cx.listener(|editor, _, window, cx| editor.remove_picked(window, cx))),
                    ),
            )
            .children(ai_off_box)
            .children(ai_wrote_box)
            .child(field("Name", Input::new(&form.label).small()))
            .child(kind_fields)
            .children(values_box)
            .child(
                v_flex()
                    .gap_3()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(div().text_sm().font_semibold().child(if can_retry { "If it fails" } else { "Time limit" }))
                    .child(field("Time limit for each try (seconds)", Input::new(&form.timeout).small()))
                    .when(can_retry, |el| {
                        el.child(
                            h_flex()
                                .gap_2()
                                .child(div().flex_1().child(field("Tries again", Input::new(&form.retries).small())))
                                .child(div().flex_1().child(field("Waiting (seconds)", Input::new(&form.retry_wait).small()))),
                        )
                    }),
            )
            .child(note(format!("Its id is {}. Later steps use it as {used_as}.", node.id), cx))
            .into_any_element()
    }

    fn render_start(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let form = &self.start_form;
        let mut found = problems(&self.draft);
        found.extend(self.missing_vault(cx).into_iter().map(|name| format!("No vault value named \u{201c}{name}\u{201d}. Add it under Vault values.")));
        let agents = self.draft.start.agents;
        let inputs = v_flex().gap_2().children(form.inputs.iter().enumerate().map(|(ix, (key, default))| {
            h_flex()
                .gap_1()
                .items_center()
                .child(div().flex_1().child(Input::new(key).small()))
                .child(div().flex_1().child(Input::new(default).small()))
                .child(
                    Button::new(SharedString::from(format!("input-remove-{ix}")))
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(Lucide::X))
                        .tooltip("Remove")
                        .on_click(cx.listener(move |editor, _, window, cx| editor.remove_input(ix, window, cx))),
                )
        }));
        v_flex()
            .p_3()
            .gap_3()
            .child(heading(Lucide::Workflow, "Automation", cx))
            .child(field("Name", Input::new(&form.name).small()))
            .child(field("Description", Textarea::new(&form.description)))
            .child(
                field(
                    "Inputs",
                    v_flex()
                        .gap_2()
                        .child(note("Values asked for at each run, with a default. Steps use them as {{input.name}}.", cx))
                        .child(inputs)
                        .child(
                            Button::new("input-add")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(Lucide::Plus))
                                .label("Add input")
                                .on_click(cx.listener(|editor, _, window, cx| editor.add_input(window, cx))),
                        ),
                ),
            )
            .child(self.render_schedule(cx))
            .child(toggle("flow-agents", "AI agents can run this", agents, cx, |editor, on| editor.draft.start.agents = on))
            .when(!found.is_empty(), |el| {
                el.child(
                    v_flex()
                        .gap_1()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.muted)
                        .child(div().text_sm().font_semibold().child("Before it can run"))
                        .children(found.into_iter().take(5).map(|problem| div().text_sm().child(problem))),
                )
            })
            .child(note("Click a step to change it. Drag from a step's dot to another step to join them.", cx))
            .into_any_element()
    }

    fn render_schedule(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let form = &self.start_form;
        let next = crate::automations::scheduler::next_run(&self.draft);
        let presets = h_flex().flex_wrap().gap_1().children(SCHEDULES.iter().map(|(words, cron)| {
            div()
                .id(SharedString::from(format!("schedule-{cron}")))
                .px_1p5()
                .py_0p5()
                .rounded_md()
                .bg(theme.secondary)
                .text_xs()
                .cursor_pointer()
                .hover(|chip| chip.bg(theme.secondary_hover))
                .child(*words)
                .on_click(cx.listener(move |editor, _, window, cx| {
                    editor.start_form.schedule.update(cx, |input, cx| input.set_value(*cron, window, cx));
                    editor.apply_start_form(cx);
                }))
        }));
        let said = match &next {
            None => note("Runs only when you or an agent start it.", cx).into_any_element(),
            Some(Ok(when)) => note(format!("{when}."), cx).into_any_element(),
            Some(Err(err)) => div().text_xs().text_color(theme.foreground).child(err.clone()).into_any_element(),
        };
        field(
            "Schedule",
            v_flex()
                .gap_2()
                .child(div().font_family(Settings::get(cx).mono_font()).child(Input::new(&form.schedule).small()))
                .child(said)
                .child(presets)
                .when(next.is_some(), |el| el.child(field("When the computer slept through a run", Select::new(&form.missed).small()))),
        )
        .into_any_element()
    }

    pub(super) fn render_runs(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mono = Settings::get(cx).mono_font();
        let running = self.is_running(cx);
        let runs = self.runs(cx);
        let shown = self.shown_run(cx);
        let inputs = (!self.run_inputs.is_empty()).then(|| {
            v_flex().gap_2().children(self.run_inputs.iter().map(|(name, input)| field_owned(name.clone(), Input::new(input).small())))
        });
        let start = if running {
            Button::new("runs-stop").outline().small().w_full().icon(Icon::new(Lucide::Square)).label("Stop").on_click(cx.listener(|editor, _, _, cx| editor.stop(cx)))
        } else {
            Button::new("runs-run").custom(ink_button(cx)).small().w_full().icon(Icon::new(Lucide::Play)).label("Run now").on_click(cx.listener(|editor, _, window, cx| editor.run(window, cx)))
        };
        let start = h_flex().gap_2().child(div().flex_1().child(start)).child(
            Button::new("runs-dry")
                .outline()
                .small()
                .label("Dry run")
                .tooltip("Show every step with its values filled in. Runs nothing.")
                .on_click(cx.listener(|editor, _, _, cx| {
                    editor.dry = !editor.dry;
                    cx.notify();
                })),
        );
        let dry = self.dry.then(|| {
            let inputs = self.run_inputs.iter().map(|(name, input)| (name.clone(), input.read(cx).value().to_string())).collect();
            let steps = dry::dry_run(&self.draft, &inputs);
            v_flex()
                .gap_2()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .child(div().text_sm().font_semibold().child("Dry run: nothing runs"))
                .when(steps.is_empty(), |el| el.child(note("No steps yet.", cx)))
                .children(steps.into_iter().enumerate().map(|(ix, step)| {
                    v_flex()
                        .gap_0p5()
                        .child(div().text_sm().child(format!("{}. {}", ix + 1, step.title)))
                        .child(div().font_family(mono.clone()).text_xs().text_color(theme.muted_foreground).whitespace_normal().child(step.does))
                }))
        });
        let list = v_flex().gap_0p5().children(runs.iter().map(|run| {
            let id = run.head.id.clone();
            let on = shown.as_ref().is_some_and(|shown| shown.head.id == id);
            let length = run.ended.map(|ended| took(ended - run.started)).unwrap_or_default();
            h_flex()
                .id(SharedString::from(format!("run-{id}")))
                .px_2()
                .py_1()
                .gap_2()
                .items_center()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .when(on, |row| row.bg(theme.secondary))
                .when(!on, |row| row.hover(|row| row.bg(theme.secondary.opacity(0.5))))
                .child(status_icon(run.status, cx))
                .child(div().flex_1().min_w_0().truncate().child(format!("{} · {}", run.head.by.words(), if run.started > 0 { ago(run.started) } else { "just now".to_owned() })))
                .child(div().text_xs().text_color(theme.muted_foreground).child(length))
                .on_click(cx.listener(move |editor, _, _, cx| {
                    editor.run = Some(id.clone());
                    cx.notify();
                }))
        }));
        let details = shown.map(|run| {
            let picked = match &self.picked {
                Picked::Step(id) => Some(id.clone()),
                _ => None,
            };
            let step = picked.as_ref().and_then(|id| run.steps.get(id).map(|step| (id.clone(), step.clone())));
            let step_view = match (picked, step) {
                (None, _) => note("Click a step on the canvas to see what it said.", cx).into_any_element(),
                (Some(id), None) => {
                    let title = self.draft.node(&id).map_or(id, |node| node.title());
                    note(format!("\u{201c}{title}\u{201d} did not run."), cx).into_any_element()
                }
                (Some(id), Some((_, step))) => {
                    let title = self.draft.node(&id).map_or(id, |node| node.title());
                    let output = step.result.get("output").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
                    let error = step.result.get("error").and_then(|v| v.as_str()).map(str::to_owned);
                    let words = match step.status {
                        StepStatus::Running => "Running".to_owned(),
                        StepStatus::Done => format!("Done, by {}", step.port.clone().unwrap_or_default()),
                        StepStatus::Failed => "Failed".to_owned(),
                        StepStatus::Skipped => "Skipped: no line led here".to_owned(),
                    };
                    let length = step.ended.zip(step.started).map(|(end, start)| took(end - start));
                    let did = (!step.notes.is_empty()).then(|| {
                        v_flex()
                            .id("step-notes")
                            .max_h(px(240.))
                            .overflow_y_scroll()
                            .gap_1()
                            .children(step.notes.iter().map(|line| {
                                let failed = line.starts_with("Not done");
                                div()
                                    .font_family(mono.clone())
                                    .text_xs()
                                    .whitespace_normal()
                                    .text_color(if failed { theme.foreground } else { theme.muted_foreground })
                                    .child(line.clone())
                            }))
                    });
                    let changed: Vec<String> =
                        step.result.get("changed").and_then(|v| v.as_array()).map(|files| files.iter().filter_map(|file| file.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().font_semibold().child(title))
                        .child(
                            div().text_sm().text_color(theme.muted_foreground).child(
                                [Some(words), length, (step.attempt > 1).then(|| format!("try {}", step.attempt))].into_iter().flatten().collect::<Vec<_>>().join(" · "),
                            ),
                        )
                        .children(error.map(|error| div().text_sm().text_color(theme.foreground).child(error)))
                        .children(did.map(|did| field("What it did", did)))
                        .when(!changed.is_empty(), |el| el.child(note(format!("Changed: {}", changed.join(", ")), cx)))
                        .when(!output.trim().is_empty(), |el| {
                            el.child(
                                div()
                                    .id("step-output")
                                    .max_h(px(320.))
                                    .overflow_y_scroll()
                                    .p_2()
                                    .rounded_md()
                                    .bg(theme.secondary)
                                    .font_family(mono.clone())
                                    .text_xs()
                                    .whitespace_normal()
                                    .child(output),
                            )
                        })
                        .into_any_element()
                }
            };
            v_flex()
                .gap_2()
                .pt_2()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(status_icon(run.status, cx))
                        .child(div().text_sm().font_semibold().child(run_words(run.status)))
                        .child(div().text_sm().text_color(theme.muted_foreground).child(run.head.by.words()))
                        .children(ai_tokens(&run).map(|tokens| {
                            div().flex_1().text_right().text_xs().text_color(theme.muted_foreground).child(format!("AI: {tokens} tokens"))
                        })),
                )
                .children(run.error.clone().map(|error| div().text_sm().text_color(theme.foreground).child(error)))
                .child(step_view)
        });
        v_flex()
            .p_3()
            .gap_3()
            .children(inputs)
            .child(start)
            .children(dry)
            .when(runs.is_empty(), |el| el.child(note("No runs yet.", cx)))
            .when(!runs.is_empty(), |el| el.child(list))
            .children(details)
            .into_any_element()
    }
}

/// Tokens a run's AI steps used, when the server said.
fn ai_tokens(run: &somme_automation::RunRecord) -> Option<u64> {
    let total: u64 = run.steps.values().filter_map(|step| step.result.get("tokens").and_then(|tokens| tokens.as_u64())).sum();
    (total > 0).then_some(total)
}

fn command_hosts(kind: &mut NodeKind) -> Option<&mut HostPick> {
    match kind {
        NodeKind::Command { hosts, .. } => Some(hosts),
        _ => None,
    }
}

fn agent_hosts(kind: &mut NodeKind) -> Option<&mut HostPick> {
    match kind {
        NodeKind::Agent { hosts, .. } => Some(hosts),
        _ => None,
    }
}

/// The servers dropdown, or where to add servers when there are none.
fn servers_field(servers: &ServersState, cx: &gpui_kit::App) -> AnyElement {
    if Store::global(cx).read(cx).hosts.is_empty() {
        return note("No saved servers yet. Add one under SSH \u{2192} Hosts.", cx).into_any_element();
    }
    Combobox::new(servers)
        .small()
        .placeholder("Pick one or more servers")
        .search_placeholder("Find a server")
        .cleanable(true)
        .into_any_element()
}

impl FlowEditor {
    /// A command box, with a saved script to start from and a way to save it as one.
    fn command_field(&self, command: &Entity<TextareaState>, scripts: &ScriptsState, mono: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let has_scripts = !Store::global(cx).read(cx).snippets.is_empty();
        let said = self.script_note.clone().map(|(ok, text)| {
            let theme = cx.theme();
            div().text_xs().text_color(if ok { theme.muted_foreground } else { theme.foreground }).child(text)
        });
        v_flex()
            .gap_1p5()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div().flex_1().min_w_0().child(
                            Select::new(scripts)
                                .small()
                                .placeholder(if has_scripts { "Start from a saved script" } else { "No saved scripts yet" })
                                .search_placeholder("Find a script")
                                .disabled(!has_scripts),
                        ),
                    )
                    .child(
                        Button::new("step-save-script")
                            .small()
                            .ghost()
                            .icon(Icon::new(Lucide::Save))
                            .label("Save as script")
                            .on_click(cx.listener(|editor, _, window, cx| editor.save_script(window, cx))),
                    ),
            )
            .child(div().font_family(mono.clone()).child(Textarea::new(command)))
            .children(said)
            .into_any_element()
    }
}

fn heading(icon: Lucide, title: &str, cx: &gpui_kit::App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(Icon::new(icon).size(px(16.)).text_color(cx.theme().muted_foreground))
        .child(div().font_semibold().child(title.to_owned()))
}

fn field(label: &'static str, input: impl IntoElement) -> gpui_kit::Div {
    v_flex().gap_1().child(div().text_sm().font_medium().child(label)).child(input)
}

fn field_owned(label: String, input: impl IntoElement) -> gpui_kit::Div {
    v_flex().gap_1().child(div().text_sm().font_medium().child(label)).child(input)
}

fn note(text: impl Into<SharedString>, cx: &gpui_kit::App) -> gpui_kit::Div {
    div().text_xs().text_color(cx.theme().muted_foreground).child(text.into())
}

fn toggle(
    id: &'static str,
    label: &'static str,
    on: bool,
    cx: &mut Context<FlowEditor>,
    set: impl Fn(&mut FlowEditor, bool) + 'static,
) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(Switch::new(id).checked(on).small().color(cx.theme().foreground).on_click(cx.listener(move |editor, on: &bool, _, cx| {
            set(editor, *on);
            editor.changed(cx);
        })))
        .child(div().text_sm().child(label))
}

fn status_icon(status: RunStatus, cx: &gpui_kit::App) -> impl IntoElement {
    let theme = cx.theme();
    match status {
        RunStatus::Running => Icon::new(Lucide::LoaderCircle).size(px(14.)).text_color(theme.foreground),
        RunStatus::Ok => Icon::new(Lucide::CircleCheck).size(px(14.)).text_color(theme.muted_foreground),
        RunStatus::Failed => Icon::new(Lucide::CircleX).size(px(14.)).text_color(theme.foreground),
        RunStatus::Cancelled => Icon::new(Lucide::CircleMinus).size(px(14.)).text_color(theme.muted_foreground),
    }
}
