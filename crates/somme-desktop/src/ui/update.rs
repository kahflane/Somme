//! The update button on the rail, below the profile picture, and the
//! popover it opens.

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::{Popover, PopoverState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    Anchor, AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, ViewElement, Window, div, prelude::FluentBuilder as _, px,
};

use crate::ui::common::{APP_NAME, Lucide, human_size};
use crate::ui::native;
use crate::update::install::Way;
use crate::update::release::Version;
use crate::update::{self, Stage, Updates};

/// The button, while an update is out. `unsaved` says whether quitting now
/// would lose changes to open files.
pub fn rail_button(unsaved: impl Fn(&App) -> bool + 'static, cx: &App) -> Option<AnyElement> {
    let found = Updates::shown(cx)?;
    let version = found.release.version;
    let unsaved: Rc<dyn Fn(&App) -> bool> = Rc::new(unsaved);
    Some(
        Popover::new("update-popover")
            .anchor(Anchor::LeftCenter)
            .offset(px(10.))
            .trigger(RailButton { version, open: false })
            .content(move |_, window, cx| content(unsaved.clone(), window, cx))
            .into_any_element(),
    )
}

/// The rail icon, with a dot to catch the eye.
struct RailButton {
    version: Version,
    open: bool,
}

impl Selectable for RailButton {
    fn selected(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }
}

impl IntoElement for RailButton {
    type Element = ViewElement<Self>;

    fn into_element(self) -> Self::Element {
        ViewElement::new(self)
    }
}

impl RenderOnce for RailButton {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let tip: SharedString = format!("Update to v{}", self.version).into();
        div()
            .id("rail-update")
            .relative()
            .size(px(34.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_lg()
            .cursor_pointer()
            .when(self.open, |el| el.bg(theme.foreground.opacity(0.1)))
            .when(!self.open, |el| el.hover(|el| el.bg(theme.foreground.opacity(0.05))))
            .child(Icon::new(Lucide::CircleArrowDown).size(px(18.)).text_color(theme.foreground))
            .child(
                div()
                    .absolute()
                    .top(px(5.))
                    .right(px(5.))
                    .size(px(8.))
                    .rounded_full()
                    .border_1()
                    .border_color(theme.sidebar)
                    .bg(theme.primary),
            )
            .when(!self.open, |el| el.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)))
    }
}

fn content(unsaved: Rc<dyn Fn(&App) -> bool>, _: &mut Window, cx: &mut Context<PopoverState>) -> AnyElement {
    let Some(found) = Updates::shown(cx).cloned() else { return div().into_any_element() };
    let popover = cx.entity();
    let stage = Updates::stage(cx);
    let theme = cx.theme().clone();
    let busy = matches!(stage, Stage::Downloading { .. } | Stage::Checking | Stage::Restarting);
    let installs = matches!(found.way, Way::Install(_));
    let small = |text: SharedString| div().text_xs().text_color(theme.muted_foreground).child(text);

    let status: Option<AnyElement> = match stage {
        Stage::Downloading { done, total } => {
            let percent = if total == 0 { 0. } else { done as f32 / total as f32 * 100. };
            Some(
                v_flex()
                    .gap_1()
                    .child(Progress::new("update-progress").value(percent))
                    .child(small(format!("Downloading… {} of {}", human_size(done), human_size(total)).into()))
                    .into_any_element(),
            )
        }
        Stage::Checking => Some(small("Checking the download…".into()).into_any_element()),
        Stage::Restarting => Some(small(format!("Opening the new {APP_NAME}…").into()).into_any_element()),
        Stage::Failed(why) => Some(div().text_xs().text_color(theme.danger).child(why).into_any_element()),
        Stage::Idle => match &found.way {
            Way::Page(why) => Some(small((*why).into()).into_any_element()),
            Way::Install(_) => None,
        },
    };

    let action = if installs {
        Button::new("update-install")
            .small()
            .primary()
            .label("Update & relaunch")
            .loading(busy)
            .disabled(busy)
            .on_click(move |_, window, cx| {
                if !unsaved(cx) {
                    return update::install(cx);
                }
                native::confirm(
                    "Update and lose unsaved changes?",
                    &format!("Some open files have changes that aren't saved. {APP_NAME} closes to update, and the changes will be lost."),
                    "Update",
                    |_, cx| update::install(cx),
                    window,
                    cx,
                );
            })
    } else {
        Button::new("update-page")
            .small()
            .primary()
            .icon(Icon::new(Lucide::ExternalLink))
            .label("Open release page")
            .on_click(move |_, window, cx| {
                update::open_page(cx);
                popover.update(cx, |popover, cx| popover.dismiss(window, cx));
            })
    };

    v_flex()
        .w(px(300.))
        .gap_3()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(Icon::new(Lucide::CircleArrowDown).size(px(22.)).text_color(theme.primary))
                .child(
                    v_flex()
                        .child(div().font_semibold().child(format!("{APP_NAME} {} is out", found.release.version)))
                        .child(small(format!("You have {}.", Version::running()).into())),
                ),
        )
        .when(!found.release.notes.is_empty(), |el| {
            el.child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().font_semibold().child("What's new"))
                    .child(
                        div()
                            .id("update-notes")
                            .max_h(px(160.))
                            .overflow_y_scroll()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(found.release.notes.clone()),
                    ),
            )
        })
        .children(status)
        .child(
            h_flex()
                .justify_end()
                .gap_2()
                .child(
                    Button::new("update-later")
                        .small()
                        .ghost()
                        .label("Later")
                        .disabled(busy)
                        .on_click(|_, _, cx| update::later(cx)),
                )
                .child(action),
        )
        .when(installs && !busy, |el| {
            el.child(small(format!("{APP_NAME} closes and opens again. Open terminals will close.").into()))
        })
        .into_any_element()
}
