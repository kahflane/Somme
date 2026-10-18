//! Small pieces drawn the way GitHub draws them: state icons and badges,
//! label chips, profile pictures and times.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::{ActiveTheme as _, Icon, h_flex};
use gpui_kit::{
    AnyElement, App, Div, Global, Hsla, Image, IntoElement, ParentElement as _, Pixels, SharedString, Styled as _,
    div, img, px, rgb,
};

use super::api::{self, Detail, Item, Label};
use crate::runtime;
use crate::store::image_format;
use crate::ui::common::{Lucide, ago};

/// GitHub's own colours for what state something is in.
#[derive(Clone, Copy)]
pub enum Tone {
    Open,
    Merged,
    Closed,
    Muted,
}

impl Tone {
    pub fn color(self, cx: &App) -> Hsla {
        let dark = cx.theme().is_dark();
        let hex = match (self, dark) {
            (Tone::Open, false) => 0x1a7f37,
            (Tone::Open, true) => 0x3fb950,
            (Tone::Merged, false) => 0x8250df,
            (Tone::Merged, true) => 0xa371f7,
            (Tone::Closed, false) => 0xcf222e,
            (Tone::Closed, true) => 0xf85149,
            (Tone::Muted, false) => 0x59636e,
            (Tone::Muted, true) => 0x9198a1,
        };
        rgb(hex).into()
    }
}

/// The icon a list row starts with.
pub fn item_state(item: &Item, pull: bool) -> (Lucide, Tone) {
    match (pull, item.state.as_str()) {
        (true, "MERGED") => (Lucide::GitMerge, Tone::Merged),
        (true, "CLOSED") => (Lucide::GitPullRequestClosed, Tone::Closed),
        (true, _) if item.draft => (Lucide::GitPullRequestDraft, Tone::Muted),
        (true, _) => (Lucide::GitPullRequest, Tone::Open),
        (false, "CLOSED") if item.state_reason.as_deref() == Some("NOT_PLANNED") => (Lucide::CircleSlash, Tone::Muted),
        (false, "CLOSED") => (Lucide::CircleCheck, Tone::Merged),
        (false, _) => (Lucide::CircleDot, Tone::Open),
    }
}

/// The icon, word and colour for the badge under a title.
pub fn detail_state(detail: &Detail) -> (Lucide, &'static str, Tone) {
    let pull = detail.is_pull();
    let open = detail.state == "open";
    match (pull, open) {
        (true, _) if detail.merged => (Lucide::GitMerge, "Merged", Tone::Merged),
        (true, false) => (Lucide::GitPullRequestClosed, "Closed", Tone::Closed),
        (true, true) if detail.draft => (Lucide::GitPullRequestDraft, "Draft", Tone::Muted),
        (true, true) => (Lucide::GitPullRequest, "Open", Tone::Open),
        (false, true) => (Lucide::CircleDot, "Open", Tone::Open),
        (false, false) if detail.state_reason.as_deref() == Some("not_planned") => {
            (Lucide::CircleSlash, "Closed", Tone::Muted)
        }
        (false, false) => (Lucide::CircleCheck, "Closed", Tone::Merged),
    }
}

/// A filled pill like GitHub's "Open" badge.
pub fn state_badge(icon: Lucide, word: &'static str, tone: Tone, cx: &App) -> Div {
    h_flex()
        .flex_none()
        .h(px(26.))
        .px_2p5()
        .gap_1()
        .items_center()
        .rounded_full()
        .bg(tone.color(cx))
        .text_color(gpui_kit::white())
        .text_sm()
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .child(Icon::new(icon).size(px(14.)).text_color(gpui_kit::white()))
        .child(word)
}

/// A label in its own colour, with text that stays readable on it.
pub fn label_chip(label: &Label, cx: &App) -> Div {
    let color = u32::from_str_radix(label.color.trim_start_matches('#'), 16).unwrap_or(0xededed);
    let (r, g, b) = ((color >> 16) & 0xff, (color >> 8) & 0xff, color & 0xff);
    let light = (r * 299 + g * 587 + b * 114) / 1000 > 150;
    let (bg, fg) = if cx.theme().is_dark() {
        // GitHub's dark pages draw a tint and a light text of the same colour.
        let base: Hsla = rgb(color).into();
        (base.opacity(0.25), Hsla { l: base.l.max(0.7), ..base })
    } else {
        (rgb(color).into(), if light { rgb(0x1f2328).into() } else { gpui_kit::white() })
    };
    div()
        .flex_none()
        .px_2()
        .h(px(20.))
        .flex()
        .items_center()
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_xs()
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .child(SharedString::from(label.name.clone()))
}

/// "3 days ago" from GitHub's time stamps.
pub fn when(stamp: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(stamp) {
        Ok(time) => ago(time.timestamp()),
        Err(_) => String::new(),
    }
}

/// Pictures fetched this run. `None` while one loads or when it failed.
#[derive(Default)]
struct Avatars(HashMap<String, Option<Arc<Image>>>);

impl Global for Avatars {}

/// A round profile picture. A grey circle shows until it is fetched.
pub fn avatar(url: Option<&str>, size: Pixels, cx: &mut App) -> AnyElement {
    let muted = cx.theme().muted;
    let blank = || div().flex_none().size(size).rounded_full().bg(muted).into_any_element();
    let Some(url) = url else { return blank() };
    let avatars = cx.default_global::<Avatars>();
    match avatars.0.get(url) {
        Some(Some(picture)) => img(picture.clone()).flex_none().size(size).rounded_full().into_any_element(),
        Some(None) => blank(),
        None => {
            avatars.0.insert(url.to_owned(), None);
            let url = url.to_owned();
            cx.spawn(async move |cx| {
                let fetch = url.clone();
                let bytes = runtime::io(async move { api::avatar(&fetch).await }).await.ok();
                let picture = bytes
                    .and_then(|bytes| image_format(&bytes).map(|format| Arc::new(Image::from_bytes(format, bytes))));
                if picture.is_some() {
                    cx.update(|cx| {
                        cx.default_global::<Avatars>().0.insert(url, picture);
                        cx.refresh_windows();
                    });
                }
            })
            .detach();
            blank()
        }
    }
}

/// An icon in a soft circle, for lines like "ann added the bug label".
pub fn event_icon(icon: Lucide, tint: Option<Hsla>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .flex_none()
        .size(px(28.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(tint.unwrap_or(theme.muted))
        .child(
            Icon::new(icon)
                .size(px(14.))
                .text_color(if tint.is_some() { gpui_kit::white() } else { theme.muted_foreground }),
        )
}
