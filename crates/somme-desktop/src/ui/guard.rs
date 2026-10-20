//! Settings → Agents → Guard: rules in the user's own words that stop
//! agents' commands or ask first, and a box to try a command against them.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::setting::{SettingGroup, SettingItem};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, PromptButton,
    PromptLevel, Render, SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};
use somme_ai::guard::{self, Action, CheckedBy, Decision, Rule, RuleAction, Strictness, Verdict};

use crate::ai::{self, Feature, GuardAsk};
use crate::runtime;
use crate::settings::Settings;
use crate::store::Store;
use crate::ui::common::{APP_NAME, Lucide, ok_color};
use crate::ui::native;
use crate::ui::settings_page::{SettingsPage, choice};

/// Script text shown when asking, at most.
const SHOWN_SCRIPT: usize = 1200;

const STRICTNESS: &[(Strictness, &str)] =
    &[(Strictness::Relaxed, "Relaxed"), (Strictness::Normal, "Normal"), (Strictness::Strict, "Strict")];

pub fn guard_group(page: WeakEntity<SettingsPage>, cx: &App) -> SettingGroup {
    let ready = ai::is_configured(Settings::get(cx), cx);
    let group = SettingGroup::new()
        .title("Guard")
        .description(format!(
            "{APP_NAME} checks every agent command against these rules before it runs. A quick check on this computer catches deletes, config changes and production servers, and Jev or your LLM checks every rule."
        ));
    let group = if ready { group } else { group.item(SettingItem::render(move |_, _, cx| set_up_ai_first(page.clone(), cx))) };
    group
        .item(
            SettingItem::render(|_, window, cx| div().w_full().child(window.use_keyed_state("guard-setup", cx, GuardSetup::new)))
                .disabled(!ready),
        )
        .item(
            SettingItem::new("Strictness", choice(STRICTNESS, |s| s.guard_strictness, |s, v| s.guard_strictness = v))
                .description("Strict stops more, and asks you more often.")
                .disabled(!ready),
        )
}

/// Shown instead of the rules until AI is set up.
fn set_up_ai_first(page: WeakEntity<SettingsPage>, cx: &App) -> gpui_kit::Div {
    let theme = cx.theme();
    h_flex()
        .w_full()
        .gap_3()
        .p_3()
        .items_center()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .child(Icon::new(Lucide::Sparkles).text_color(theme.muted_foreground))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_sm().font_semibold().child("Set up AI to use the guard"))
                .child(div().text_xs().text_color(theme.muted_foreground).child(
                    "The guard needs AI to read your rules. Turn on Use AI and add an LLM or a Jev key in Settings \u{203a} AI. Until then, only the quick check on this computer runs.",
                )),
        )
        .child(Button::new("guard-set-up-ai").small().label("Set up AI").on_click(move |_, _, cx| {
            let _ = page.update(cx, |page, cx| page.show_ai(cx));
        }))
}

/// Asks in the front Somme window whether an action may run. Deny comes
/// first, so Return does not allow it by accident. With no window open,
/// the answer is dropped, which counts as no one to ask.
pub fn ask_user(ask: GuardAsk, cx: &mut App) {
    let GuardAsk { who, host, script, reason, question, answer } = ask;
    let mut shown = script.trim().to_owned();
    if shown.chars().count() > SHOWN_SCRIPT {
        shown = shown.chars().take(SHOWN_SCRIPT).collect::<String>() + "\n[…]";
    }
    let (title, said, detail) = match question {
        Some(question) => {
            let detail = if shown.is_empty() { who.clone() } else { format!("{who}:\n\n{shown}") };
            (question.clone(), format!("{who} asks: {question}"), detail)
        }
        None => (
            "Allow this command?".to_owned(),
            format!("{who} wants to run a command on {host}."),
            format!("{who} wants to run this on {host}:\n\n{shown}\n\n{reason}"),
        ),
    };
    native::notify(&format!("{APP_NAME} is waiting for you"), &said, cx);
    // Never in the bar that shows an agent is using the screen.
    let is_bar = crate::ui::control_bar::is_bar;
    let Some(window) = cx.active_window().filter(|window| !is_bar(window)).or_else(|| cx.windows().into_iter().find(|window| !is_bar(window))) else { return };
    let picked = window.update(cx, |_, window, cx| {
        let buttons = [PromptButton::cancel("Deny"), PromptButton::ok("Allow")];
        native::ask(PromptLevel::Warning, &title, Some(&detail), &buttons, window, cx)
    });
    let Ok(picked) = picked else { return };
    cx.spawn(async move |_| {
        let _ = answer.send(picked.await == Ok(1));
    })
    .detach();
}

/// Who settled a guard check, in words.
pub fn checked_by(by: CheckedBy) -> &'static str {
    match by {
        CheckedBy::Nothing => "No rules are set.",
        CheckedBy::Local => "Caught by the quick check on this computer.",
        CheckedBy::Jev => "Checked by Jev.",
        CheckedBy::Llm => "Checked by your LLM.",
        CheckedBy::LocalOnly => "Only the quick check ran.",
        CheckedBy::Failed => "Jev and the LLM did not answer.",
    }
}

fn rules_text(rules: &[Rule]) -> String {
    rules.iter().map(|rule| rule.text.as_str()).collect::<Vec<_>>().join("\n")
}

pub struct GuardSetup {
    rules: Entity<TextareaState>,
    tried: Entity<InputState>,
    checking: bool,
    verdict: Option<Verdict>,
}

impl GuardSetup {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = rules_text(&Settings::get(cx).guard_rules);
        let rules = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(3, 12).placeholder("Block all drop, delete or truncate").default_value(text)
        });
        // Save shows only when the text changed.
        cx.subscribe(&rules, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        let tried = cx.new(|cx| InputState::new(window, cx).placeholder("Try a command, like: DROP TABLE users"));
        cx.subscribe_in(&tried, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event
                && ai::is_configured(Settings::get(cx), cx)
            {
                this.check(window, cx);
            }
        })
        .detach();
        // Settings can change from another window or computer.
        cx.observe_global::<Settings>(|_, cx| cx.notify()).detach();
        Self { rules, tried, checking: false, verdict: None }
    }

    /// The rules as typed, keeping the Block or Ask picked for unchanged ones.
    fn typed(&self, cx: &Context<Self>) -> Vec<Rule> {
        guard::parse_rules(&self.rules.read(cx).value(), &Settings::get(cx).guard_rules)
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let rules = self.typed(cx);
        Settings::update(cx, |s| s.guard_rules = rules);
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rules = guard::default_rules();
        self.rules.update(cx, |input, cx| input.set_value(rules_text(&rules), window, cx));
        Settings::update(cx, |s| s.guard_rules = rules);
    }

    fn flip(&mut self, ix: usize, cx: &mut Context<Self>) {
        Settings::update(cx, |s| {
            if let Some(rule) = s.guard_rules.get_mut(ix) {
                rule.action = match rule.action {
                    RuleAction::Block => RuleAction::Ask,
                    RuleAction::Ask => RuleAction::Block,
                };
            }
        });
    }

    /// Checks the typed command against the typed rules, as an agent's would be.
    fn check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let command = self.tried.read(cx).value().trim().to_owned();
        if command.is_empty() || self.checking {
            return;
        }
        let mut settings = Settings::get(cx).clone();
        settings.guard_rules = self.typed(cx);
        let services = Store::services(cx);
        self.checking = true;
        self.verdict = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let action = Action::command(command, None, "you, trying a rule");
            let verdict = runtime::io(async move { ai::guard_with(&services, &settings, action).await }).await;
            let _ = this.update(cx, |this, cx| {
                this.checking = false;
                this.verdict = Some(verdict);
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for GuardSetup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let settings = Settings::get(cx);
        let off = !ai::is_configured(settings, cx);
        let changed = self.rules.read(cx).value().trim() != rules_text(&settings.guard_rules).trim();
        let hint = |text: &'static str| div().text_xs().text_color(theme.muted_foreground).child(text);
        let saved = settings.guard_rules.iter().enumerate().map(|(ix, rule)| {
            let (word, color) = match rule.action {
                RuleAction::Block => ("Block", theme.danger),
                RuleAction::Ask => ("Ask", theme.warning),
            };
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .id(("guard-rule-action", ix))
                        .w(px(52.))
                        .flex_none()
                        .py_0p5()
                        .rounded_md()
                        .border_1()
                        .border_color(color)
                        .text_color(color)
                        .text_xs()
                        .font_semibold()
                        .text_center()
                        .when(off, |chip| chip.opacity(0.5))
                        .when(!off, |chip| chip.cursor_pointer())
                        .child(word)
                        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Click to switch Block and Ask").build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !off {
                                this.flip(ix, cx);
                            }
                        })),
                )
                .child(div().flex_1().min_w_0().text_sm().child(SharedString::from(rule.text.clone())))
        });
        let ai_note = (!off && !ai::allowed(settings, Feature::Guard))
            .then(|| hint("Guard checks are off in Settings › AI, so only the quick check runs."));
        let verdict = self.verdict.as_ref().map(|verdict| {
            let (icon, word, color) = match verdict.decision {
                Decision::Run => (Lucide::CircleCheck, "Would run.", ok_color(cx)),
                Decision::Ask => (Lucide::CircleQuestionMark, "Would ask you first.", theme.warning),
                Decision::Block => (Lucide::ShieldX, "Would be blocked.", theme.danger),
            };
            let sure = verdict.probability.map(|p| format!(" {:.0}% sure the rule applies.", p * 100.0)).unwrap_or_default();
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .text_color(color)
                        .child(Icon::new(icon).size(px(14.)))
                        .child(div().text_sm().font_semibold().child(word)),
                )
                .child(div().text_sm().child(SharedString::from(verdict.reason.clone())))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(format!("{}{sure}", checked_by(verdict.by)))),
                )
        });
        v_flex()
            .w_full()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().font_semibold().child(format!("What should {APP_NAME} stop?")))
                    .child(hint("One rule per line, in your own words. Rules that start with Ask, Confirm or Check with me ask you first. The rest block.")),
            )
            .child(Textarea::new(&self.rules).disabled(off))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("guard-save")
                            .small()
                            .primary()
                            .label("Save rules")
                            .disabled(off || !changed)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    )
                    .child(
                        Button::new("guard-reset")
                            .small()
                            .ghost()
                            .label("Use the starting rules")
                            .disabled(off)
                            .on_click(cx.listener(|this, _, window, cx| this.reset(window, cx))),
                    ),
            )
            .child(v_flex().gap_1p5().children(saved))
            .children(ai_note)
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.tried).disabled(off)))
                    .child(
                        Button::new("guard-check")
                            .small()
                            .icon(Icon::new(Lucide::Shield))
                            .label("Check")
                            .loading(self.checking)
                            .disabled(off || self.checking)
                            .on_click(cx.listener(|this, _, window, cx| this.check(window, cx))),
                    ),
            )
            .children(verdict)
    }
}
