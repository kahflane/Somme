use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Icon, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, ClipboardItem, Div, FocusHandle, Hsla, IntoElement, ParentElement as _, SharedString, Styled as _,
    Window, div, px, rgb,
};

pub use gpui_kit::assets::IconName as Lucide;

/// Shows a backend error as a red toast.
pub fn notify_error(window: &mut Window, cx: &mut App, title: &str, err: impl std::fmt::Display) {
    window.push_notification(Notification::error(err.to_string()).title(title.to_owned()), cx);
}

pub fn notify_ok(window: &mut Window, cx: &mut App, message: impl Into<SharedString>) {
    window.push_notification(Notification::success(message.into()), cx);
}

/// The header row at the top of a page: title on the left, buttons on the right.
pub fn page_header(title: impl Into<SharedString>, cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .h(px(52.))
        .px_5()
        .gap_2()
        .items_center()
        .border_b_1()
        .border_color(theme.border)
        .child(div().text_lg().font_semibold().child(title.into()))
        .child(div().flex_1())
}

/// Shown when a list has nothing in it yet.
pub fn empty_state(
    icon: impl Into<Icon>,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_2()
        .py_16()
        .child(
            div()
                .p_3()
                .rounded_full()
                .bg(theme.muted)
                .child(icon.into().size(px(28.)).text_color(theme.muted_foreground)),
        )
        .child(div().font_semibold().child(title.into()))
        .child(
            div()
                .text_sm()
                .max_w(px(360.))
                .text_center()
                .text_color(theme.muted_foreground)
                .child(detail.into()),
        )
}

/// A rounded box used for list rows and cards.
pub fn card(cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
}

/// Colors offered for hosts, as hex strings saved with the host.
pub const HOST_COLORS: &[&str] = &[
    "#3b82f6", "#22c55e", "#eab308", "#f97316", "#ef4444", "#a855f7", "#ec4899", "#14b8a6", "#64748b",
];

pub fn parse_color(hex: Option<&str>) -> Option<Hsla> {
    let hex = hex?.trim().trim_start_matches('#');
    let value = u32::from_str_radix(hex, 16).ok()?;
    (hex.len() == 6).then(|| rgb(value).into())
}

/// A square badge with an icon, tinted by the host color.
pub fn host_badge(color: Option<&str>, icon: impl Into<Icon>, cx: &App) -> Div {
    let theme = cx.theme();
    let tint = parse_color(color).unwrap_or(theme.primary);
    div()
        .flex_none()
        .size(px(34.))
        .rounded_md()
        .flex()
        .items_center()
        .justify_center()
        .bg(tint.opacity(0.18))
        .child(icon.into().size(px(18.)).text_color(tint))
}

pub fn ago(timestamp: i64) -> String {
    let now = crate::terminal::log::now_secs();
    let ts = if timestamp > 10_000_000_000 { timestamp / 1000 } else { timestamp };
    let diff = (now - ts).max(0);
    match diff {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", diff / 60),
        3600..86_400 => format!("{} h ago", diff / 3600),
        _ => format!("{} days ago", diff / 86_400),
    }
}

/// Small grey text under a title.
pub fn subtle(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .truncate()
        .child(text.into())
}

/// Opens a dialog with Cancel and an OK button. `submit` returns true when the
/// dialog should close. `body` is called on every redraw.
pub fn form_dialog(
    title: impl Into<SharedString>,
    ok_label: impl Into<SharedString>,
    width: f32,
    focus: Option<FocusHandle>,
    body: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    submit: impl Fn(&mut Window, &mut App) -> bool + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let title = title.into();
    let ok_label = ok_label.into();
    let submit = Rc::new(submit);
    let first = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first.replace(false)
            && let Some(focus) = &focus
        {
            window.focus(focus, cx);
        }
        let on_ok = submit.clone();
        let on_click = submit.clone();
        dialog
            .title(title.clone())
            .w(px(width))
            .child(body(window, cx))
            .on_ok(move |_, window, cx| on_ok(window, cx))
            .footer(
                DialogFooter::new()
                    .child(Button::new("dialog-cancel").label("Cancel").on_click(|_, w, cx| w.close_dialog(cx)))
                    .child(Button::new("dialog-ok").primary().label(ok_label.clone()).on_click(
                        move |_, window, cx| {
                            if on_click(window, cx) {
                                window.close_dialog(cx);
                            }
                        },
                    )),
            )
    });
}

/// Asks before deleting something. `delete` runs when the user confirms.
pub fn confirm_delete(
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    delete: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let title = title.into();
    let detail = detail.into();
    let delete = Rc::new(delete);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let delete = delete.clone();
        alert
            .confirm()
            .title(title.clone())
            .description(detail.clone())
            .ok_text("Delete")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, window, cx| {
                delete(window, cx);
                true
            })
    });
}

/// A label above a form control, for dialogs.
pub fn labeled(label: impl Into<SharedString>, control: impl IntoElement) -> Div {
    v_flex().gap_1().child(div().text_sm().child(label.into())).child(control)
}

pub fn human_size(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.),
        1_048_576..1_073_741_824 => format!("{:.1} MB", bytes as f64 / 1_048_576.),
        _ => format!("{:.1} GB", bytes as f64 / 1_073_741_824.),
    }
}

pub fn copy_text(text: impl Into<String>, message: &str, window: &mut Window, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.into()));
    notify_ok(window, cx, message.to_owned());
}
