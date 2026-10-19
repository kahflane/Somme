//! The Automations section: the saved automations on the left, and the one
//! picked open on a canvas. Templates and vault values open from the list.

mod canvas;
mod chat;
mod editor;
mod panel;
mod vault_page;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, PromptButton,
    PromptLevel, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use somme_automation::{Automation, RunStatus};

use crate::automations::Automations;
use crate::settings::Settings;
use crate::ui::common::{APP_NAME, Lucide, ago, ink_button, notify_error};
use crate::ui::native;
use editor::{EditorEvent, FlowEditor};
use vault_page::VaultPage;

const LIST_WIDTH: f32 = 240.;

/// What the right side shows.
enum Shown {
    /// The open automation, or the empty page.
    Flow,
    Templates,
    Vault(Entity<VaultPage>),
}

pub struct AutomationsPage {
    open: Option<(Entity<FlowEditor>, Subscription)>,
    shown: Shown,
    _observe: Subscription,
}

/// Asks once, after the first schedule is saved, whether schedules should
/// also run while Somme is closed. Saying yes sets up the background service.
pub(super) fn offer_background(cx: &mut App) {
    // Scripted test runs never set up a login item.
    #[cfg(feature = "snapshot")]
    if std::env::var_os("SOMME_SCRIPT").is_some() {
        return;
    }
    if Settings::get(cx).automation_service_asked {
        return;
    }
    let Some(window) = cx.active_window() else { return };
    Settings::update(cx, |s| s.automation_service_asked = true);
    let detail = format!(
        "{APP_NAME} would start a small helper in the background when you log in. It runs your schedules, and nothing else unless you turn on agents. You can change this in Settings \u{2192} Agents."
    );
    let picked = window.update(cx, |_, window, cx| {
        let buttons = [PromptButton::cancel("Not now"), PromptButton::ok("Run them")];
        native::ask(PromptLevel::Info, "Run scheduled automations while Somme is closed?", Some(&detail), &buttons, window, cx)
    });
    let Ok(picked) = picked else { return };
    cx.spawn(async move |cx| {
        if picked.await == Ok(1) {
            cx.update(|cx| Settings::update(cx, |s| s.automation_service = true));
        }
    })
    .detach();
}

impl AutomationsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe_in(&Automations::global(cx), window, |page, automations, window, cx| {
            // Open the first one when nothing is open yet.
            if page.open.is_none()
                && let Some(first) = automations.read(cx).saved.first().map(|saved| saved.automation.clone())
            {
                page.show(first, window, cx);
            }
            cx.notify();
        });
        let mut page = Self { open: None, shown: Shown::Flow, _observe: observe };
        if let Some(first) = Automations::global(cx).read(cx).saved.first().map(|saved| saved.automation.clone()) {
            page.show(first, window, cx);
        }
        page
    }

    fn show(&mut self, automation: Automation, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = Shown::Flow;
        if self.open.as_ref().is_some_and(|(editor, _)| editor.read(cx).id() == automation.id) {
            cx.notify();
            return;
        }
        let editor = cx.new(|cx| FlowEditor::new(automation, window, cx));
        let subscription = cx.subscribe_in(&editor, window, |page, _, event, window, cx| match event {
            EditorEvent::Deleted => {
                page.open = None;
                let next = Automations::global(cx).read(cx).saved.first().map(|saved| saved.automation.clone());
                if let Some(next) = next {
                    page.show(next, window, cx);
                }
                cx.notify();
            }
        });
        self.open = Some((editor, subscription));
        cx.notify();
    }

    fn new_automation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.create(Automation::new("New automation"), true, window, cx);
    }

    /// Saves `automation` as a new one, with a name not taken, and opens it.
    fn create(&mut self, mut automation: Automation, chat: bool, window: &mut Window, cx: &mut Context<Self>) {
        let taken = |name: &str| Automations::global(cx).read(cx).saved.iter().any(|saved| saved.automation.name == name);
        let base = automation.name.clone();
        automation.name = (1..).map(|n| if n == 1 { base.clone() } else { format!("{base} {n}") }).find(|name| !taken(name)).unwrap_or(base);
        automation.id.clear();
        let task = Automations::global(cx).update(cx, |automations, cx| automations.save(&automation, cx));
        cx.spawn_in(window, async move |page, cx| {
            let saved = task.await;
            let _ = page.update_in(cx, |page, window, cx| match saved {
                Ok(id) => {
                    let mut automation = automation;
                    automation.id = id.to_string();
                    page.show(automation, window, cx);
                    if chat && let Some((editor, _)) = &page.open {
                        editor.update(cx, |editor, cx| editor.focus_chat(window, cx));
                    }
                }
                Err(err) => notify_error(window, cx, "Can't make an automation", err),
            });
        })
        .detach();
    }

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let automations = Automations::global(cx);
        let automations = automations.read(cx);
        let open = matches!(self.shown, Shown::Flow).then(|| self.open.as_ref().map(|(editor, _)| editor.read(cx).id().to_owned())).flatten();
        let rows = automations.saved.iter().map(|saved| {
            let automation = saved.automation.clone();
            let id = automation.id.clone();
            let on = open.as_deref() == Some(id.as_str());
            let latest = automations.latest.get(&id);
            let (dot, words) = match latest {
                None => (theme.muted_foreground.opacity(0.4), "Never run".to_owned()),
                Some(run) => {
                    let when = if run.started > 0 { ago(run.started) } else { "just now".to_owned() };
                    match run.status {
                        RunStatus::Running => (theme.foreground, "Running now".to_owned()),
                        RunStatus::Ok => (theme.muted_foreground, format!("Done {when}")),
                        RunStatus::Failed => (theme.foreground, format!("Failed {when}")),
                        RunStatus::Cancelled => (theme.muted_foreground, format!("Stopped {when}")),
                    }
                }
            };
            h_flex()
                .id(SharedString::from(format!("automation-{id}")))
                .mx_2()
                .px_2()
                .py_1p5()
                .gap_2()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .when(on, |row| row.bg(theme.sidebar_accent).text_color(theme.sidebar_accent_foreground))
                .when(!on, |row| row.hover(|row| row.bg(theme.sidebar_accent.opacity(0.5))))
                .child(div().flex_none().size(px(8.)).rounded_full().bg(dot))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().truncate().child(SharedString::from(automation.name.clone())))
                        .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(words)),
                )
                .on_click(cx.listener(move |page, _, window, cx| page.show(automation.clone(), window, cx)))
        });
        let unreadable = automations.unreadable.len();
        let link = |id: &'static str, icon: Lucide, label: &'static str, on: bool| {
            h_flex()
                .id(id)
                .mx_2()
                .px_2()
                .py_1p5()
                .gap_2()
                .items_center()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .when(on, |row| row.bg(theme.sidebar_accent).text_color(theme.sidebar_accent_foreground))
                .when(!on, |row| row.hover(|row| row.bg(theme.sidebar_accent.opacity(0.5))))
                .child(Icon::new(icon).size_4().text_color(theme.muted_foreground))
                .child(label)
        };
        let links = v_flex()
            .flex_none()
            .py_2()
            .gap_0p5()
            .border_t_1()
            .border_color(theme.sidebar_border)
            .child(
                link("automation-templates", Lucide::LayoutTemplate, "Templates", matches!(self.shown, Shown::Templates))
                    .on_click(cx.listener(|page, _, _, cx| page.show_templates(cx))),
            )
            .child(
                link("automation-vault", Lucide::KeyRound, "Vault values", matches!(self.shown, Shown::Vault(_)))
                    .on_click(cx.listener(|page, _, window, cx| page.show_vault(window, cx))),
            );
        v_flex()
            .flex_none()
            .w(px(LIST_WIDTH))
            .h_full()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex()
                    .flex_none()
                    .h(px(44.))
                    .pl_4()
                    .pr_2()
                    .items_center()
                    .child(div().flex_1().font_semibold().child("Automations"))
                    .child(
                        Button::new("automation-new")
                            .ghost()
                            .small()
                            .icon(Icon::new(Lucide::Plus))
                            .tooltip("New automation")
                            .on_click(cx.listener(|page, _, window, cx| page.new_automation(window, cx))),
                    ),
            )
            .child(v_flex().id("automation-list").flex_1().min_h_0().overflow_y_scroll().gap_0p5().children(rows))
            .when(unreadable > 0, |list| {
                list.child(div().p_3().text_xs().text_color(theme.muted_foreground).child(match unreadable {
                    1 => "1 automation is from a newer Somme. Update to open it.".to_owned(),
                    n => format!("{n} automations are from a newer Somme. Update to open them."),
                }))
            })
            .child(links)
    }

    fn show_templates(&mut self, cx: &mut Context<Self>) {
        self.shown = Shown::Templates;
        cx.notify();
    }

    fn show_vault(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.shown, Shown::Vault(_)) {
            self.shown = Shown::Vault(cx.new(|cx| VaultPage::new(window, cx)));
        }
        cx.notify();
    }

    fn render_templates(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let cards = somme_automation::templates::all().into_iter().enumerate().map(|(ix, template)| {
            let when = match template.start.schedule.as_deref() {
                Some(schedule) => somme_automation::schedule::words(schedule),
                None => "By hand".to_owned(),
            };
            let steps = template.nodes.len();
            let ai = template.nodes.iter().any(|node| node.kind.uses_ai());
            let meta = format!("{steps} steps \u{00b7} {when}{}", if ai { " \u{00b7} Uses AI" } else { "" });
            v_flex()
                .id(SharedString::from(format!("template-{ix}")))
                .w(px(272.))
                .p_3()
                .gap_1p5()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .cursor_pointer()
                .hover(|card| card.border_color(theme.muted_foreground))
                .child(div().font_semibold().child(SharedString::from(template.name.clone())))
                .child(div().text_sm().text_color(theme.muted_foreground).child(SharedString::from(template.description.clone())))
                .child(div().text_xs().text_color(theme.muted_foreground).child(meta))
                .on_click(cx.listener(move |page, _, window, cx| page.create(template.clone(), false, window, cx)))
        });
        v_flex()
            .id("automation-templates-page")
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .gap_4()
            .child(h_flex().gap_2().items_center().child(Icon::new(Lucide::LayoutTemplate)).child(div().text_lg().font_semibold().child("Start from a template")))
            .child(div().text_sm().text_color(theme.muted_foreground).child(
                "Each one opens as your own copy. Its steps use servers by tag, like web or db: tag your servers, or pick them in each step.",
            ))
            .child(h_flex().flex_wrap().items_start().gap_3().children(cards))
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(Icon::new(Lucide::Workflow).size(px(40.)).text_color(theme.muted_foreground))
            .child(div().font_semibold().child("Automate the jobs you repeat"))
            .child(
                div()
                    .max_w(px(420.))
                    .text_sm()
                    .text_center()
                    .text_color(theme.muted_foreground)
                    .child("Run commands on your servers or this computer, check what they say, and get told when something is wrong."),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("automation-first")
                            .custom(ink_button(cx))
                            .icon(Icon::new(Lucide::Plus))
                            .label("New automation")
                            .on_click(cx.listener(|page, _, window, cx| page.new_automation(window, cx))),
                    )
                    .child(
                        Button::new("automation-from-template")
                            .icon(Icon::new(Lucide::LayoutTemplate))
                            .label("Start from a template")
                            .on_click(cx.listener(|page, _, _, cx| page.show_templates(cx))),
                    ),
            )
    }
}

impl Render for AutomationsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match (&self.shown, &self.open) {
            (Shown::Templates, _) => self.render_templates(cx).into_any_element(),
            (Shown::Vault(vault), _) => vault.clone().into_any_element(),
            (Shown::Flow, Some((editor, _))) => editor.clone().into_any_element(),
            (Shown::Flow, None) => self.render_empty(cx).into_any_element(),
        };
        h_flex().size_full().child(self.render_list(cx)).child(div().flex_1().min_w_0().h_full().child(body))
    }
}
