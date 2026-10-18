use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, StyledExt as _, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};

use crate::runtime;
use crate::store::Store;
use crate::ui::common::{logo, notify_error, notify_ok};

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
        let services = store.read(cx).services.clone();
        let remember = self.remember;
        let initialized = status.initialized;
        cx.spawn_in(window, async move |this, cx| {
            let result = runtime::io(async move {
                if initialized {
                    let status = services.vault.unlock(&password, remember).await?;
                    // What this device had before it joined the account moves in now
                    // that the account's vault is open.
                    let merged = services.sync.merge_previous(&password).await;
                    Ok((status, Some(merged)))
                } else {
                    services.vault.setup(&password, remember).await.map(|status| (status, None))
                }
            })
            .await;
            let _ = this.update_in(cx, |screen, window, cx| {
                screen.busy = false;
                match result {
                    Ok((status, merged)) => {
                        screen.password.update(cx, |input, cx| input.set_value("", window, cx));
                        screen.confirm.update(cx, |input, cx| input.set_value("", window, cx));
                        Store::global(cx).update(cx, |store, cx| store.set_vault(status, cx));
                        match merged {
                            Some(Ok(report)) if report.secrets_lost > 0 => notify_error(
                                window,
                                cx,
                                "Some saved passwords stayed behind",
                                format!(
                                    "{} passwords or keys from this device use a different master password, so they were not added to your account.",
                                    report.secrets_lost
                                ),
                            ),
                            Some(Ok(report)) if report.added > 0 => {
                                let items = if report.added == 1 { "item" } else { "items" };
                                notify_ok(cx, format!("Added {} {items} from this device to your account.", report.added))
                            }
                            Some(Err(err)) => notify_error(window, cx, "Could not add this device's data", err),
                            _ => {}
                        }
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
                SharedString::from("Set a master password"),
                "It encrypts your saved servers, SSH keys and passwords. It never leaves this computer.",
            )
        } else {
            ("Enter your master password".into(), "It unlocks your saved servers, SSH keys and passwords.")
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
            .child(logo(px(48.)))
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
                    .label(if setup { "Set Password" } else { "Unlock" })
                    .on_click(cx.listener(|screen, _, window, cx| screen.submit(window, cx))),
            )
            .when(setup, |form| {
                form.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("If you forget this password, your saved servers and keys cannot be recovered."),
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
