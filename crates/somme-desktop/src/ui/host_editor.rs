use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::form::{field, v_form};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::Select;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use somme_application::dto::{HostInput, HostView, SecretUpdate};
use somme_domain::Id;

use crate::store::mutate;
use crate::ui::common::{HOST_COLORS, notify_error, parse_color};
use crate::ui::fields::{
    ChoiceState, choice_select, group_choices, host_choices, identity_choices, key_choices, selected, snippet_choices,
};

/// The form for adding or changing a host, shown in a side sheet.
pub struct HostEditor {
    id: Option<Id>,
    had_password: bool,
    clear_password: bool,
    label: Entity<InputState>,
    address: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    tags: Entity<InputState>,
    group: ChoiceState<Option<Id>>,
    identity: ChoiceState<Option<Id>>,
    key: ChoiceState<Option<Id>>,
    jump: ChoiceState<Option<Id>>,
    snippet: ChoiceState<Option<Id>>,
    color: Option<String>,
    error: Option<String>,
    saving: bool,
}

impl HostEditor {
    /// Opens the editor. `host` is the host to change; `None` adds a new one.
    pub fn open(host: Option<HostView>, group: Option<Id>, duplicate: bool, window: &mut Window, cx: &mut App) {
        let editor = cx.new(|cx| Self::new(host, group, duplicate, window, cx));
        let title = {
            let editor = editor.read(cx);
            if editor.id.is_some() { "Edit Host" } else { "New Host" }
        };
        let first = std::cell::Cell::new(true);
        window.open_sheet(cx, move |sheet, window, cx| {
            if first.replace(false) {
                let input = editor.read(cx).address.clone();
                input.update(cx, |input, cx| input.focus(window, cx));
            }
            let save = editor.clone();
            sheet
                .title(title)
                .size(px(460.))
                .child(editor.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("host-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_sheet(cx)),
                        )
                        .child(
                            Button::new("host-save")
                                .primary()
                                .label("Save")
                                .loading(editor.read(cx).saving)
                                .on_click(move |_, window, cx| save.update(cx, |e, cx| e.save(window, cx))),
                        ),
                )
        });
    }

    fn new(host: Option<HostView>, group: Option<Id>, duplicate: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text = |value: String, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value))
        };
        let h = host.as_ref();
        let label_value = h
            .map(|h| if duplicate { format!("{} copy", h.label) } else { h.label.clone() })
            .unwrap_or_default();
        let label = text(label_value, "e.g. Production web", window, cx);
        let address = text(h.map(|h| h.address.clone()).unwrap_or_default(), "IP address or host name", window, cx);
        let port = text(h.map(|h| h.port).unwrap_or(22).to_string(), "22", window, cx);
        let username = text(h.and_then(|h| h.username.clone()).unwrap_or_default(), "root", window, cx);
        let had_password = h.is_some_and(|h| h.has_password) && !duplicate;
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(if had_password { "Saved — type to replace" } else { "Optional" })
        });
        let tags = text(h.map(|h| h.tags.join(", ")).unwrap_or_default(), "web, prod", window, cx);
        let id = h.filter(|_| !duplicate).map(|h| h.id);
        let group_id = h.map(|h| h.group_id).unwrap_or(group);
        let identity_id = h.and_then(|h| h.identity_id);
        let key_id = h.and_then(|h| h.key_id);
        let jump_id = h.and_then(|h| h.jump_host_id);
        let snippet_id = h.and_then(|h| h.startup_snippet_id);
        let color = h.and_then(|h| h.color.clone());
        Self {
            id,
            had_password,
            clear_password: false,
            label,
            address,
            port,
            username,
            password,
            tags,
            group: choice_select(group_choices(cx), &group_id, window, cx),
            identity: choice_select(identity_choices(cx), &identity_id, window, cx),
            key: choice_select(key_choices(cx), &key_id, window, cx),
            jump: choice_select(host_choices(id, cx), &jump_id, window, cx),
            snippet: choice_select(snippet_choices(cx), &snippet_id, window, cx),
            color,
            error: None,
            saving: false,
        }
    }

    fn input(&self, cx: &App) -> Result<HostInput, String> {
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let address = value(&self.address);
        if address.is_empty() {
            return Err("Enter an address.".into());
        }
        let port_text = value(&self.port);
        let port = if port_text.is_empty() {
            22
        } else {
            port_text
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or("Port must be a number from 1 to 65535.")?
        };
        let mut label = value(&self.label);
        if label.is_empty() {
            label = address.clone();
        }
        let username = Some(value(&self.username)).filter(|u| !u.is_empty());
        let typed = self.password.read(cx).value().to_string();
        let password = if !typed.is_empty() {
            SecretUpdate::Set(typed)
        } else if self.clear_password {
            SecretUpdate::Clear
        } else {
            SecretUpdate::Keep
        };
        let tags = value(&self.tags)
            .split(',')
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect();
        Ok(HostInput {
            id: self.id,
            label,
            address,
            port,
            group_id: selected(&self.group, cx).flatten(),
            identity_id: selected(&self.identity, cx).flatten(),
            username,
            password,
            key_id: selected(&self.key, cx).flatten(),
            jump_host_id: selected(&self.jump, cx).flatten(),
            startup_snippet_id: selected(&self.snippet, cx).flatten(),
            tags,
            color: self.color.clone(),
        })
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let input = match self.input(cx) {
            Ok(input) => input,
            Err(err) => {
                self.error = Some(err);
                cx.notify();
                return;
            }
        };
        self.saving = true;
        self.error = None;
        cx.notify();
        mutate(
            window,
            cx,
            move |services| async move { services.hosts.save(input).await },
            |editor, result, window, cx| {
                editor.saving = false;
                match result {
                    Ok(_) => window.close_sheet(cx),
                    Err(err) => {
                        editor.error = Some(err.to_string());
                        notify_error(window, cx, "Could not save host", err);
                    }
                }
                cx.notify();
            },
        );
    }
}

impl Render for HostEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let swatches = h_flex()
            .gap_1p5()
            .child(
                div()
                    .id("color-none")
                    .size(px(22.))
                    .rounded_full()
                    .border_1()
                    .border_color(theme.border)
                    .when(self.color.is_none(), |el| el.border_2().border_color(theme.foreground))
                    .cursor_pointer()
                    .on_click(cx.listener(|editor, _, _, cx| {
                        editor.color = None;
                        cx.notify();
                    })),
            )
            .children(HOST_COLORS.iter().map(|hex| {
                let chosen = self.color.as_deref() == Some(*hex);
                div()
                    .id(*hex)
                    .size(px(22.))
                    .rounded_full()
                    .bg(parse_color(Some(hex)).unwrap_or(theme.primary))
                    .when(chosen, |el| el.border_2().border_color(theme.foreground))
                    .cursor_pointer()
                    .on_click(cx.listener(move |editor, _, _, cx| {
                        editor.color = Some((*hex).to_owned());
                        cx.notify();
                    }))
            }));

        let password_field = field().label("Password").child(
            v_flex()
                .gap_1()
                .child(Input::new(&self.password).mask_toggle())
                .when(self.had_password, |col| {
                    col.child(
                        h_flex().justify_end().child(
                            Button::new("clear-password")
                                .link()
                                .xsmall()
                                .label(if self.clear_password { "Password will be removed" } else { "Remove saved password" })
                                .on_click(cx.listener(|editor, _, _, cx| {
                                    editor.clear_password = !editor.clear_password;
                                    cx.notify();
                                })),
                        ),
                    )
                }),
        );

        v_flex()
            .size_full()
            .overflow_y_scrollbar()
            .child(
                v_form()
                    .child(field().label("Address").required(true).child(Input::new(&self.address)))
                    .child(field().label("Label").child(Input::new(&self.label)))
                    .child(field().label("Group").child(Select::new(&self.group)))
                    .child(field().label("Tags").description("Separate with commas").child(Input::new(&self.tags)))
                    .child(field().label("Color").child(swatches))
                    .child(field().label("Port").child(Input::new(&self.port)))
                    .child(field().label("Username").child(Input::new(&self.username)))
                    .child(password_field)
                    .child(field().label("SSH Key").child(Select::new(&self.key)))
                    .child(
                        field()
                            .label("Identity")
                            .description("Uses the identity's username, password and key")
                            .child(Select::new(&self.identity)),
                    )
                    .child(field().label("Jump Host").child(Select::new(&self.jump)))
                    .child(
                        field()
                            .label("Startup Snippet")
                            .description("Runs right after connecting")
                            .child(Select::new(&self.snippet)),
                    ),
            )
            .when_some(self.error.clone(), |col, err| {
                col.child(div().pt_2().text_sm().text_color(theme.danger).child(err))
            })
    }
}
