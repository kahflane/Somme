use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, StyledExt as _, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};

use crate::runtime;
use crate::store::Store;
use crate::ui::common::Lucide;

const MIN_PASSWORD: usize = 8;

/// Shown until the vault is open: creates a new vault or unlocks the old one.
pub struct LockScreen {
    password: Entity<InputState>,
    confirm: Entity<InputState>,
    remember: bool,
    busy: bool,
    error: Option<String>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl LockScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let password = cx.new(|cx| InputState::new(window, cx).placeholder("Master password").masked(true));
        let confirm = cx.new(|cx| InputState::new(window, cx).placeholder("Repeat password").masked(true));
        let submit = |screen: &mut Self, _: &Entity<InputState>, event: &InputEvent, window: &mut Window, cx: &mut Context<Self>| {
            if let InputEvent::PressEnter { .. } = event {
                screen.submit(window, cx);
            }
        };
        let subscriptions = vec![
            cx.subscribe_in(&password, window, submit),
            cx.subscribe_in(&confirm, window, submit),
            cx.observe(&Store::global(cx), |_, _, cx| cx.notify()),
        ];
        password.update(cx, |input, cx| input.focus(window, cx));
        Self {
            password,
            confirm,
            remember: true,
            busy: false,
            error: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    pub fn focus_password(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.password.update(cx, |input, cx| input.focus(window, cx));
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let store = Store::global(cx);
        let Some(status) = store.read(cx).vault else { return };
        let password = self.password.read(cx).value().to_string();
        if status.initialized {
            if password.is_empty() {
                self.error = Some("Enter your master password.".into());
                cx.notify();
                return;
            }
        } else {
            let confirm = self.confirm.read(cx).value().to_string();
            if password.chars().count() < MIN_PASSWORD {
                self.error = Some(format!("Use at least {MIN_PASSWORD} characters."));
                cx.notify();
                return;
            }
            if password != confirm {
                self.error = Some("The passwords do not match.".into());
                cx.notify();
                return;
            }
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let vault = store.read(cx).services.vault.clone();
        let remember = self.remember;
        let initialized = status.initialized;
        cx.spawn_in(window, async move |this, cx| {
            let result = runtime::io(async move {
                if initialized {
                    vault.unlock(&password, remember).await
                } else {
                    vault.setup(&password, remember).await
                }
            })
            .await;
            let _ = this.update_in(cx, |screen, window, cx| {
                screen.busy = false;
                match result {
                    Ok(status) => {
                        screen.password.update(cx, |input, cx| input.set_value("", window, cx));
                        screen.confirm.update(cx, |input, cx| input.set_value("", window, cx));
                        Store::global(cx).update(cx, |store, cx| store.set_vault(status, cx));
                    }
                    Err(err) => {
                        screen.error = Some(err.to_string());
                        screen.focus_password(window, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Focusable for LockScreen {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LockScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = Store::global(cx);
        let store = store.read(cx);
        let theme = cx.theme();
        let Some(status) = store.vault else {
            let body = match &store.load_error {
                Some(err) => div().text_color(theme.danger).child(err.clone()),
                None => div().child(Spinner::new()),
            };
            return v_flex().size_full().items_center().justify_center().child(body);
        };
        let setup = !status.initialized;
        let (title, detail) = if setup {
            (
                "Create your vault",
                "Hosts, keys and passwords are encrypted with a master password. It never leaves this computer.",
            )
        } else {
            ("Unlock Somme", "Enter your master password to open the vault.")
        };

        let form = v_flex()
            .w(px(380.))
            .gap_3()
            .p_6()
            .rounded_xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_xl()
            .child(
                div()
                    .size(px(48.))
                    .rounded_lg()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme.primary.opacity(0.15))
                    .child(Icon::new(Lucide::LockKeyhole).size(px(24.)).text_color(theme.primary)),
            )
            .child(div().text_xl().font_semibold().child(title))
            .child(div().text_sm().text_color(theme.muted_foreground).child(detail))
            .child(Input::new(&self.password).mask_toggle())
            .when(setup, |form| form.child(Input::new(&self.confirm).mask_toggle()))
            .child(
                Checkbox::new("remember")
                    .checked(self.remember)
                    .label("Unlock automatically on this computer")
                    .on_click(cx.listener(|screen, checked: &bool, _, cx| {
                        screen.remember = *checked;
                        cx.notify();
                    })),
            )
            .when_some(self.error.clone(), |form, err| {
                form.child(div().text_sm().text_color(theme.danger).child(err))
            })
            .child(
                Button::new("submit")
                    .primary()
                    .w_full()
                    .loading(self.busy)
                    .label(if setup { "Create Vault" } else { "Unlock" })
                    .on_click(cx.listener(|screen, _, window, cx| screen.submit(window, cx))),
            )
            .when(setup, |form| {
                form.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("If you forget this password, the vault cannot be recovered."),
                )
            });

        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .bg(theme.background)
            .child(form)
    }
}
