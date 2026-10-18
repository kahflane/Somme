//! Settings → Agents: the switch for the MCP server, how to connect an
//! agent, and the way to the script log.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    App, Context, Entity, Hsla, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription,
    WeakEntity, Window, div, px,
};

use crate::mcp::Agents;
use crate::settings::Settings;
use crate::ui::common::{APP_NAME, Lucide, ago, copy_text, notify_ok};
use crate::ui::settings_page::SettingsPage;

pub fn agents_page(page: WeakEntity<SettingsPage>) -> SettingPage {
    // One group, so the sidebar shows Agents alone, without sub items.
    SettingPage::new("Agents").icon(Icon::new(Lucide::Bot)).group(
        SettingGroup::new()
            .title("MCP server")
            .description(format!(
                "Let AI agents, like Claude Code, run scripts on your saved servers through MCP. They reach only servers whose key you already trust, with the passwords and keys saved in {APP_NAME}. Every script is logged."
            ))
            .item(
                SettingItem::new(
                    "Let agents run scripts",
                    SettingField::switch(
                        |cx| Settings::get(cx).agent_access,
                        |on, cx| Settings::update(cx, |s| s.agent_access = on),
                    )
                    .default_value(false),
                )
                .description("Turning it off disconnects every agent and stops the scripts they are running."),
            )
            .item(
                SettingItem::new(
                    "Keep working when Somme is closed",
                    SettingField::switch(
                        |cx| Settings::get(cx).agent_service,
                        |on, cx| Settings::update(cx, |s| s.agent_service = on),
                    )
                    .default_value(false),
                )
                .description(format!(
                    "Starts {APP_NAME} in the background when you log in, so agents can run scripts while the app is closed. It can unlock only if you chose to remember the master password."
                )),
            )
            .item(SettingItem::render(move |_, window, cx| {
                let page = page.clone();
                div().w_full().child(window.use_keyed_state("agent-setup", cx, |_, cx| AgentSetup::new(page, cx)))
            })),
    )
}

/// The status, the connect steps and the log button. Redraws as agents work.
pub struct AgentSetup {
    agents: Entity<Agents>,
    page: WeakEntity<SettingsPage>,
    _watch: Subscription,
}

impl AgentSetup {
    fn new(page: WeakEntity<SettingsPage>, cx: &mut Context<Self>) -> Self {
        let agents = Agents::global(cx);
        let watch = cx.observe(&agents, |_, _, cx| cx.notify());
        Self {
            agents,
            page,
            _watch: watch,
        }
    }
}

/// `text` in single quotes, for a shell.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// What to paste into an agent's setup.
fn connect_lines() -> (String, String) {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| APP_NAME.to_owned());
    // Test runs keep their data elsewhere, and the agent needs to find it too.
    let data_dir = std::env::var("SOMME_DATA_DIR").ok();
    let env_flag = data_dir.as_ref().map(|dir| format!(" -e SOMME_DATA_DIR={}", quoted(dir))).unwrap_or_default();
    let claude = format!("claude mcp add somme{env_flag} -- {} --mcp", quoted(&exe));
    let mut server = serde_json::json!({ "command": exe, "args": ["--mcp"] });
    if let Some(dir) = data_dir {
        server["env"] = serde_json::json!({ "SOMME_DATA_DIR": dir });
    }
    // One line, so it stays short.
    let json = serde_json::json!({ "mcpServers": { "somme": server } }).to_string();
    (claude, json)
}

fn code_box(id: &'static str, text: String, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let copied = text.clone();
    h_flex()
        .w_full()
        .gap_2()
        .items_start()
        .p_2()
        .rounded_md()
        .bg(theme.muted)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .font_family(Settings::get(cx).font_family.clone())
                .child(SharedString::from(text)),
        )
        .child(
            Button::new(id)
                .ghost()
                .xsmall()
                .icon(Icon::new(Lucide::Copy))
                .tooltip("Copy")
                .on_click(move |_, _, cx| {
                    copy_text(copied.clone(), cx);
                    notify_ok(cx, "Copied");
                }),
        )
}

impl Render for AgentSetup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let agents = self.agents.read(cx);
        let status: (Hsla, String) = match (&agents.problem, agents.is_on(), agents.connections) {
            (Some(problem), _, _) => (theme.danger, problem.clone()),
            (None, false, _) => (theme.muted_foreground, "Off. Agents can not connect.".into()),
            (None, true, 0) => (theme.success, "On. No agent is connected.".into()),
            (None, true, 1) => (theme.success, "On. 1 agent is connected.".into()),
            (None, true, count) => (theme.success, format!("On. {count} agents are connected.")),
        };
        let summary = match (agents.runs.len(), agents.runs.first()) {
            (_, None) => "Agents have not run any scripts yet.".to_owned(),
            (1, Some(last)) => format!("1 script, {}.", ago(last.started)),
            (count, Some(last)) => format!("{count} scripts. The last one ran {}.", ago(last.started)),
        };
        let service_line = match (&agents.service_problem, agents.service_busy()) {
            (_, true) => Some(
                div()
                    .pt_1()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Setting up the background service…"),
            ),
            (Some(problem), false) => {
                Some(div().pt_1().text_sm().text_color(theme.danger).child(SharedString::from(problem.clone())))
            }
            (None, false) => agents.service_on().then(|| {
                div()
                    .pt_1()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Agents keep working when Somme is closed.")
            }),
        };
        let (claude, json) = connect_lines();
        let heading = |text: &'static str| div().pt_3().pb_1().text_sm().font_semibold().child(text);
        let hint = |text: &'static str| div().pb_1().text_xs().text_color(theme.muted_foreground).child(text);
        let page = self.page.clone();
        v_flex()
            .w_full()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().size(px(8.)).rounded_full().bg(status.0))
                    .child(div().text_sm().child(SharedString::from(status.1))),
            )
            .children(service_line)
            .child(heading("Script log"))
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().text_sm().text_color(theme.muted_foreground).child(summary))
                    .child(
                        Button::new("agent-open-logs")
                            .small()
                            .icon(Icon::new(Lucide::ScrollText))
                            .label("Logs")
                            .on_click(move |_, window, cx| {
                                let _ = page.update(cx, |page, cx| page.open_logs(window, cx));
                            }),
                    ),
            )
            .child(heading("Connect Claude Code"))
            .child(hint("Run this in a terminal:"))
            .child(code_box("agent-copy-claude", claude, cx))
            .child(heading("Connect another agent"))
            .child(hint("Add this to the agent's MCP settings:"))
            .child(code_box("agent-copy-json", json, cx))
    }
}
