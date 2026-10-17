use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, ClickEvent, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, px,
};
use somme_application::dto::KnownHostView;

use crate::store::{Store, mutate, report};
use crate::ui::common::{Lucide, ago, card, confirm_delete, copy_text, empty_state, host_badge, page_header, subtle};

/// Server keys the user has trusted. A host whose key changes asks again.
pub struct KnownHostsPage {
    search: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl KnownHostsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search known hosts"));
        let subscriptions = vec![
            cx.observe(&Store::global(cx), |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        Self {
            search,
            _subscriptions: subscriptions,
        }
    }

    fn delete(&mut self, known: KnownHostView, window: &mut Window, cx: &mut Context<Self>) {
        let page = cx.entity().downgrade();
        let id = known.id;
        confirm_delete(
            format!("Forget {}?", address(&known)),
            "You will be asked to check its key again on the next connect.",
            move |window, cx| {
                let _ = page.update(cx, |_, cx| {
                    mutate(
                        window,
                        cx,
                        move |services| async move { services.known_hosts.delete(id).await },
                        report("Could not forget host"),
                    );
                });
            },
            window,
            cx,
        );
    }

    fn render_row(&self, known: KnownHostView, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let id = known.id;
        let fingerprint = known.fingerprint.clone();
        card(cx)
            .id(SharedString::from(format!("known-{id}")))
            .group("known")
            .p_3()
            .flex()
            .items_center()
            .gap_3()
            .hover(|el| el.bg(theme.muted))
            .child(host_badge(None, Icon::new(Lucide::ShieldCheck), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().font_medium().truncate().child(address(&known)))
                    .child(
                        div()
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(known.fingerprint.clone()),
                    ),
            )
            .child(
                v_flex()
                    .flex_none()
                    .items_end()
                    .gap_0p5()
                    .child(div().text_xs().child(key_type(&known.key_type)))
                    .child(subtle(format!("added {}", ago(known.created_at)), cx)),
            )
            .child(
                h_flex()
                    .gap_0p5()
                    .invisible()
                    .group_hover("known", |el| el.visible())
                    .child(
                        Button::new(SharedString::from(format!("copy-known-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Copy)
                            .tooltip("Copy fingerprint")
                            .on_click(move |_: &ClickEvent, _, cx| {
                                copy_text(fingerprint.clone(), cx);
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("forget-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Delete)
                            .tooltip("Forget")
                            .on_click(cx.listener(move |page, _, window, cx| page.delete(known.clone(), window, cx))),
                    ),
            )
    }
}

fn address(known: &KnownHostView) -> String {
    if known.port == 22 { known.host.clone() } else { format!("{}:{}", known.host, known.port) }
}

fn key_type(key_type: &str) -> String {
    key_type.trim_start_matches("ssh-").to_uppercase()
}

impl Render for KnownHostsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search.read(cx).value().trim().to_lowercase();
        let store = Store::global(cx);
        let (total, rows) = {
            let store = store.read(cx);
            let rows: Vec<KnownHostView> = store
                .known_hosts
                .iter()
                .filter(|k| {
                    query.is_empty()
                        || address(k).to_lowercase().contains(&query)
                        || k.key_type.to_lowercase().contains(&query)
                        || k.fingerprint.to_lowercase().contains(&query)
                })
                .cloned()
                .collect();
            (store.known_hosts.len(), rows)
        };

        let body = if total == 0 {
            empty_state(
                Icon::new(Lucide::ShieldCheck),
                "No known hosts yet",
                "When you trust a server's key on first connect, it is saved here. If the key ever changes, you are warned.",
                cx,
            )
            .into_any_element()
        } else if rows.is_empty() {
            empty_state(IconName::Search, "Nothing matches", "Try a different search.", cx).into_any_element()
        } else {
            v_flex()
                .px_5()
                .py_4()
                .gap_2()
                .children(rows.into_iter().map(|k| self.render_row(k, cx)))
                .into_any_element()
        };

        v_flex()
            .size_full()
            .child(
                page_header("Known Hosts", cx).child(
                    div().w(px(280.)).child(
                        Input::new(&self.search)
                            .small()
                            .cleanable(true)
                            .prefix(Icon::new(IconName::Search).size(px(14.))),
                    ),
                ),
            )
            .child(div().flex_1().min_h_0().overflow_y_scrollbar().child(body))
    }
}
