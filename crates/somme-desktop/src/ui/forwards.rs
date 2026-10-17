use std::collections::HashSet;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::{Select, SelectEvent};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClickEvent, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use somme_application::dto::{PortForwardInput, PortForwardView};
use somme_domain::Id;

use crate::runtime;
use crate::store::{Store, mutate, mutate_weak, report};
use crate::ui::common::{
    Lucide, card, confirm_delete, empty_state, host_badge, labeled, notify_error, page_header, subtle,
};
use crate::ui::fields::{Choice, ChoiceState, choice_select, selected};

/// Port forwarding rules, each tied to a host, that can be switched on and off.
pub struct ForwardsPage {
    /// Rules that are connecting right now.
    starting: HashSet<Id>,
    _subscriptions: Vec<Subscription>,
}

/// One line that says what a rule does, e.g. "127.0.0.1:8080 → db:5432".
pub fn describe(rule: &PortForwardView) -> String {
    let bind = format!("{}:{}", rule.bind_address, rule.bind_port);
    let target = format!(
        "{}:{}",
        rule.target_host.as_deref().unwrap_or("?"),
        rule.target_port.map(|p| p.to_string()).unwrap_or_else(|| "?".into())
    );
    match rule.kind.as_str() {
        "local" => format!("{bind} → {target}"),
        "remote" => format!("server {bind} → {target}"),
        _ => format!("SOCKS proxy on {bind}"),
    }
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "local" => "Local",
        "remote" => "Remote",
        _ => "Dynamic",
    }
}

fn kind_icon(kind: &str) -> Lucide {
    match kind {
        "local" => Lucide::ArrowRight,
        "remote" => Lucide::ArrowLeft,
        _ => Lucide::Globe,
    }
}

impl ForwardsPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            starting: HashSet::new(),
            _subscriptions: vec![cx.observe(&Store::global(cx), |_, _, cx| cx.notify())],
        }
    }

    fn toggle(&mut self, rule: &PortForwardView, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let id = rule.id;
        let label = rule.label.clone();
        let services = Store::services(cx);
        if !on {
            cx.spawn(async move |_, cx| {
                runtime::io(async move {
                    services.forwards.stop(id).await;
                    Ok::<_, ()>(())
                })
                .await
                .ok();
                let _ = cx.update(|cx| Store::global(cx).update(cx, |store, cx| store.refresh_forwards(cx)));
            })
            .detach();
            return;
        }
        if !self.starting.insert(id) {
            return;
        }
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = runtime::io(async move { services.forwards.start(id).await }).await;
            let _ = this.update_in(cx, |page, window, cx| {
                page.starting.remove(&id);
                Store::global(cx).update(cx, |store, cx| store.refresh_forwards(cx));
                if let Err(err) = result {
                    notify_error(window, cx, &format!("Could not start {label}"), err);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn delete(&mut self, rule: PortForwardView, window: &mut Window, cx: &mut Context<Self>) {
        let page = cx.entity().downgrade();
        let id = rule.id;
        confirm_delete(
            format!("Delete rule {}?", rule.label),
            "It is stopped first if it is on. This cannot be undone.",
            move |window, cx| {
                mutate_weak(
                    &page,
                    window,
                    cx,
                    move |services| async move { services.forwards.delete(id).await },
                    report("Could not delete rule"),
                );
            },
            window,
            cx,
        );
    }

    fn render_rule(&self, rule: PortForwardView, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let store = Store::global(cx);
        let (active, host) = {
            let store = store.read(cx);
            let host = store.host(rule.host_id).map(|h| h.label.clone()).unwrap_or_else(|| "missing host".into());
            (store.active_forwards.contains(&rule.id), host)
        };
        let starting = self.starting.contains(&rule.id);
        let id = rule.id;
        let page = cx.entity().downgrade();
        let edit = rule.clone();
        let delete = rule.clone();
        let toggle = rule.clone();
        card(cx)
            .id(SharedString::from(format!("forward-{id}")))
            .group("forward")
            .p_3()
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .hover(|el| el.bg(theme.muted))
            .child(host_badge(
                active.then_some("#22c55e"),
                Icon::new(kind_icon(&rule.kind)),
                cx,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().font_medium().truncate().child(rule.label.clone()))
                            .child(
                                div()
                                    .px_1p5()
                                    .rounded_sm()
                                    .text_xs()
                                    .bg(theme.muted)
                                    .text_color(theme.muted_foreground)
                                    .child(kind_label(&rule.kind)),
                            ),
                    )
                    .child(subtle(format!("{} · via {host}", describe(&rule)), cx)),
            )
            .child(
                h_flex()
                    .gap_0p5()
                    .invisible()
                    .group_hover("forward", |el| el.visible())
                    .child(
                        Button::new(SharedString::from(format!("forward-delete-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Delete)
                            .tooltip("Delete")
                            .on_click({
                                let page = page.clone();
                                move |_: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    let _ = page.update(cx, |page, cx| page.delete(delete.clone(), window, cx));
                                }
                            }),
                    ),
            )
            .child(
                div()
                    .w(px(44.))
                    .flex()
                    .justify_end()
                    .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .when(starting, |el| el.child(Spinner::new().small()))
                    .when(!starting, |el| {
                        el.child(
                            Switch::new(SharedString::from(format!("forward-switch-{id}")))
                                .checked(active)
                                .disabled(!Store::global(cx).read(cx).is_unlocked())
                                .on_click(move |on: &bool, window, cx| {
                                    cx.stop_propagation();
                                    let on = *on;
                                    let _ = page.update(cx, |page, cx| page.toggle(&toggle, on, window, cx));
                                }),
                        )
                    }),
            )
            .on_click(move |_, window, cx| ForwardEditor::open(Some(edit.clone()), window, cx))
    }
}

impl Render for ForwardsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let store = Store::global(cx);
        let (rules, has_hosts) = {
            let store = store.read(cx);
            (store.forwards.clone(), !store.hosts.is_empty())
        };
        let active = store.read(cx).active_forwards.len();

        let header = page_header("Port Forwarding", cx)
            .when(active > 0, |row| row.child(subtle(format!("{active} on"), cx)))
            .child(
                Button::new("new-forward")
                    .primary()
                    .small()
                    .icon(IconName::Plus)
                    .label("New Rule")
                    .disabled(!has_hosts)
                    .on_click(|_, window, cx| ForwardEditor::open(None, window, cx)),
            );

        let body = if rules.is_empty() {
            let detail = if has_hosts {
                "Reach a service behind a server (local), share a local port with the server (remote), or route traffic through it as a SOCKS proxy (dynamic)."
            } else {
                "Add a host first. Each rule tunnels through one of your hosts."
            };
            empty_state(Icon::new(Lucide::ArrowLeftRight), "No forwarding rules yet", detail, cx)
                .when(has_hosts, |el| {
                    el.child(
                        div().pt_2().child(
                            Button::new("empty-forward")
                                .primary()
                                .icon(IconName::Plus)
                                .label("New Rule")
                                .on_click(|_, window, cx| ForwardEditor::open(None, window, cx)),
                        ),
                    )
                })
                .into_any_element()
        } else {
            v_flex()
                .px_5()
                .py_4()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(theme.muted_foreground)
                        .child(format!("Rules ({})", rules.len())),
                )
                .child(
                    div()
                        .grid()
                        .grid_cols(2)
                        .gap_3()
                        .children(rules.into_iter().map(|r| self.render_rule(r, cx))),
                )
                .into_any_element()
        };

        v_flex()
            .size_full()
            .child(header)
            .child(div().flex_1().min_h_0().overflow_y_scrollbar().child(body))
    }
}

// ---- editor ----

/// The form for adding or changing a forwarding rule, shown in a side sheet.
struct ForwardEditor {
    id: Option<Id>,
    label: Entity<InputState>,
    kind: ChoiceState<&'static str>,
    host: ChoiceState<Option<Id>>,
    bind_address: Entity<InputState>,
    bind_port: Entity<InputState>,
    target_host: Entity<InputState>,
    target_port: Entity<InputState>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl ForwardEditor {
    fn open(rule: Option<PortForwardView>, window: &mut Window, cx: &mut App) {
        let editor = cx.new(|cx| Self::new(rule, window, cx));
        let title = if editor.read(cx).id.is_some() { "Edit Rule" } else { "New Rule" };
        let first = std::cell::Cell::new(true);
        window.open_sheet(cx, move |sheet, window, cx| {
            if first.replace(false) {
                let input = editor.read(cx).label.clone();
                input.update(cx, |input, cx| input.focus(window, cx));
            }
            let save = editor.clone();
            sheet
                .title(title)
                .size(px(440.))
                .child(editor.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("forward-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_sheet(cx)),
                        )
                        .child(
                            Button::new("forward-save")
                                .primary()
                                .label("Save")
                                .on_click(move |_, window, cx| save.update(cx, |e, cx| e.save(window, cx))),
                        ),
                )
        });
    }

    fn new(rule: Option<PortForwardView>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let r = rule.as_ref();
        let text = |value: String, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value))
        };
        let kinds = vec![
            Choice {
                value: "local",
                label: "Local — reach a service behind the server".into(),
            },
            Choice {
                value: "remote",
                label: "Remote — let the server reach a local service".into(),
            },
            Choice {
                value: "dynamic",
                label: "Dynamic — SOCKS proxy through the server".into(),
            },
        ];
        let kind_value: &'static str = match r.map(|r| r.kind.as_str()) {
            Some("remote") => "remote",
            Some("dynamic") => "dynamic",
            _ => "local",
        };
        let kind = choice_select(kinds, &kind_value, window, cx);
        let store = Store::global(cx);
        let mut hosts: Vec<Choice<Option<Id>>> = store
            .read(cx)
            .hosts
            .iter()
            .map(|h| Choice {
                value: Some(h.id),
                label: h.label.clone().into(),
            })
            .collect();
        hosts.sort_by_key(|c| c.label.to_lowercase());
        let host_value = r.map(|r| r.host_id).or_else(|| hosts.first().and_then(|c| c.value));
        let host = choice_select(hosts, &host_value, window, cx);
        let subscriptions = vec![cx.subscribe(&kind, |_, _, _: &SelectEvent<_>, cx| cx.notify())];
        Self {
            id: r.map(|r| r.id),
            label: text(r.map(|r| r.label.clone()).unwrap_or_default(), "e.g. Database", window, cx),
            kind,
            host,
            bind_address: text(
                r.map(|r| r.bind_address.clone()).unwrap_or_else(|| "127.0.0.1".into()),
                "127.0.0.1",
                window,
                cx,
            ),
            bind_port: text(r.map(|r| r.bind_port.to_string()).unwrap_or_default(), "8080", window, cx),
            target_host: text(r.and_then(|r| r.target_host.clone()).unwrap_or_default(), "localhost", window, cx),
            target_port: text(
                r.and_then(|r| r.target_port).map(|p| p.to_string()).unwrap_or_default(),
                "5432",
                window,
                cx,
            ),
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn kind(&self, cx: &App) -> &'static str {
        selected(&self.kind, cx).unwrap_or("local")
    }

    fn input(&self, cx: &App) -> Result<PortForwardInput, String> {
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        let port = |input: &Entity<InputState>, what: &str| -> Result<u16, String> {
            value(input)
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| format!("{what} must be a number from 1 to 65535."))
        };
        let host_id = selected(&self.host, cx).flatten().ok_or("Pick the host to tunnel through.")?;
        let kind = self.kind(cx);
        let bind_port = port(&self.bind_port, "Port")?;
        let bind_address = Some(value(&self.bind_address))
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| "127.0.0.1".into());
        let (target_host, target_port) = if kind == "dynamic" {
            (None, None)
        } else {
            let host = value(&self.target_host);
            if host.is_empty() {
                return Err("Enter the destination address.".into());
            }
            (Some(host), Some(port(&self.target_port, "Destination port")?))
        };
        let mut label = value(&self.label);
        if label.is_empty() {
            label = match kind {
                "dynamic" => format!("SOCKS {bind_port}"),
                _ => format!("Port {bind_port}"),
            };
        }
        Ok(PortForwardInput {
            id: self.id,
            label,
            host_id,
            kind: kind.to_owned(),
            bind_address,
            bind_port,
            target_host,
            target_port,
        })
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = match self.input(cx) {
            Ok(input) => input,
            Err(err) => {
                self.error = Some(err);
                cx.notify();
                return;
            }
        };
        self.error = None;
        mutate(
            window,
            cx,
            move |services| async move { services.forwards.save(input).await },
            |editor, result, window, cx| {
                match result {
                    Ok(_) => window.close_sheet(cx),
                    Err(err) => editor.error = Some(err.to_string()),
                }
                cx.notify();
            },
        );
    }
}

impl Render for ForwardEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let kind = self.kind(cx);
        let (bind_label, bind_hint, target_label) = match kind {
            "remote" => (
                "Server address and port",
                "Where the server listens. 127.0.0.1 keeps it private to the server.",
                "Local destination",
            ),
            "dynamic" => (
                "Local address and port",
                "Point your browser or app at this SOCKS5 proxy.",
                "",
            ),
            _ => (
                "Local address and port",
                "Where this computer listens. 127.0.0.1 keeps it private to you.",
                "Destination (as seen from the server)",
            ),
        };
        let address_port = |address: &Entity<InputState>, port: &Entity<InputState>| {
            h_flex()
                .gap_2()
                .child(div().flex_1().child(Input::new(address)))
                .child(div().w(px(110.)).child(Input::new(port)))
        };
        v_flex()
            .size_full()
            .gap_4()
            .overflow_y_scrollbar()
            .child(labeled("Label", Input::new(&self.label)))
            .child(labeled("Type", Select::new(&self.kind)))
            .child(labeled("Host", Select::new(&self.host)))
            .child(
                labeled(bind_label, address_port(&self.bind_address, &self.bind_port)).child(subtle(bind_hint, cx)),
            )
            .when(kind != "dynamic", |col| {
                col.child(labeled(target_label, address_port(&self.target_host, &self.target_port)))
            })
            .when_some(self.error.clone(), |col, err| {
                col.child(div().text_sm().text_color(theme.danger).child(err))
            })
    }
}
