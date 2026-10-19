//! Vault values: secrets steps use as `{{vault.NAME}}`. Only their names
//! show; the values stay sealed with the master password.

use std::collections::BTreeMap;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::automations::{Automations, vault};
use crate::runtime;
use crate::settings::Settings;
use crate::store::Store;
use crate::ui::common::{Lucide, ink_button};

pub struct VaultPage {
    /// None until read.
    names: Option<Vec<String>>,
    problem: Option<String>,
    name: Entity<InputState>,
    value: Entity<InputState>,
    busy: bool,
}

impl VaultPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("DB_PASSWORD"));
        let value = cx.new(|cx| InputState::new(window, cx).placeholder("Value").masked(true));
        for input in [&name, &value] {
            cx.subscribe_in(input, window, |page, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => page.add(window, cx),
                InputEvent::Change => {
                    page.problem = None;
                    cx.notify();
                }
                _ => {}
            })
            .detach();
        }
        let mut page = Self { names: None, problem: None, name, value, busy: false };
        page.read(cx);
        page
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        let services = Store::services(cx);
        cx.spawn(async move |page, cx| {
            let loaded = runtime::io(async move { vault::load(&services).await }).await;
            let _ = page.update(cx, |page, cx| {
                match loaded {
                    Ok(values) => page.names = Some(values.into_keys().collect()),
                    Err(err) => page.problem = Some(err),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads every value, changes them, and saves them back.
    fn change(&mut self, change: impl FnOnce(&mut BTreeMap<String, String>) + Send + 'static, done: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        cx.notify();
        let services = Store::services(cx);
        cx.spawn_in(window, async move |page, cx| {
            let saved = runtime::io(async move {
                let mut values = vault::load(&services).await?;
                change(&mut values);
                vault::save(&services, &values).await?;
                Ok::<_, String>(values.into_keys().collect::<Vec<_>>())
            })
            .await;
            let _ = page.update_in(cx, |page, window, cx| {
                page.busy = false;
                match saved {
                    Ok(names) => {
                        page.names = Some(names);
                        done(page, window, cx);
                        Automations::global(cx).update(cx, |automations, cx| automations.refresh_vault(cx));
                    }
                    Err(err) => page.problem = Some(err),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_owned();
        let value = self.value.read(cx).value().to_string();
        if let Some(problem) = vault::check(&name, &value) {
            self.problem = Some(problem);
            cx.notify();
            return;
        }
        self.change(
            move |values| {
                values.insert(name, value);
            },
            |page, window, cx| {
                page.name.update(cx, |input, cx| input.set_value("", window, cx));
                page.value.update(cx, |input, cx| input.set_value("", window, cx));
            },
            window,
            cx,
        );
    }

    fn remove(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.change(
            move |values| {
                values.remove(&name);
            },
            |_, _, _| {},
            window,
            cx,
        );
    }
}

impl Render for VaultPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mono = Settings::get(cx).mono_font();
        let rows = self.names.clone().unwrap_or_default().into_iter().map(|name| {
            let gone = name.clone();
            h_flex()
                .px_3()
                .py_2()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(theme.border)
                .child(div().flex_1().min_w_0().truncate().font_family(mono.clone()).text_sm().child(format!("{{{{vault.{name}}}}}")))
                .child(div().text_sm().text_color(theme.muted_foreground).child("\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}"))
                .child(
                    Button::new(SharedString::from(format!("vault-remove-{name}")))
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(Lucide::Trash))
                        .tooltip("Remove")
                        .disabled(self.busy)
                        .on_click(cx.listener(move |page, _, window, cx| page.remove(gone.clone(), window, cx))),
                )
        });
        let empty = self.names.as_ref().is_some_and(Vec::is_empty);
        v_flex()
            .size_full()
            .items_center()
            .p_6()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(560.))
                    .gap_4()
                    .child(h_flex().gap_2().items_center().child(Icon::new(Lucide::KeyRound)).child(div().text_lg().font_semibold().child("Vault values")))
                    .child(div().text_sm().text_color(theme.muted_foreground).child(
                        "Passwords and tokens your automations use, like {{vault.DB_PASSWORD}}. Sealed with your master password, hidden in run logs, and never sent to AI.",
                    ))
                    .child(
                        v_flex()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border)
                            .children(rows)
                            .when(empty, |list| list.child(div().p_3().text_sm().text_color(theme.muted_foreground).child("No values yet.")))
                            .child(
                                h_flex()
                                    .p_2()
                                    .gap_2()
                                    .child(div().w(px(180.)).font_family(mono.clone()).child(Input::new(&self.name).small()))
                                    .child(div().flex_1().child(Input::new(&self.value).small()))
                                    .child(
                                        Button::new("vault-add")
                                            .custom(ink_button(cx))
                                            .small()
                                            .icon(Icon::new(Lucide::Plus))
                                            .label("Add")
                                            .disabled(self.busy)
                                            .on_click(cx.listener(|page, _, window, cx| page.add(window, cx))),
                                    ),
                            ),
                    )
                    .children(self.problem.clone().map(|problem| div().text_sm().text_color(theme.foreground).child(problem)))
                    .child(div().text_xs().text_color(theme.muted_foreground).child("Adding a name that is already here replaces its value.")),
            )
    }
}
