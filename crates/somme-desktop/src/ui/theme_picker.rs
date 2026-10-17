//! Clickable previews for picking an app theme or a terminal color scheme.
//! Used on the settings page and in the terminal side panel.

use gpui_kit::component::{ActiveTheme as _, Icon, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    App, Div, Hsla, InteractiveElement as _, ParentElement as _, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px, rgb,
};

use crate::settings::Settings;
use crate::terminal::colors::{THEMES, TerminalTheme, active_theme, to_hsla};
use crate::ui::common::Lucide;
use crate::ui::themes::{APP_THEMES, AppTheme, choose};

const CARD_WIDTH: f32 = 120.;

/// A row of cards that wraps to fit the space.
fn grid() -> Div {
    h_flex().w_full().flex_wrap().gap_2()
}

/// The preview with an outline when picked, and the name below it.
fn card(id: SharedString, name: &'static str, picked: bool, preview: Div, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    v_flex()
        .id(id)
        .w(px(CARD_WIDTH))
        .gap_1()
        .cursor_pointer()
        .child(
            preview
                .h(px(62.))
                .rounded_md()
                .overflow_hidden()
                .border_2()
                .border_color(if picked { theme.primary } else { theme.border }),
        )
        .child(
            h_flex()
                .gap_1()
                .text_xs()
                .text_color(if picked { theme.foreground } else { theme.muted_foreground })
                .child(div().truncate().child(name))
                .when(picked, |row| {
                    row.child(Icon::new(Lucide::Check).size(px(12.)).text_color(theme.primary))
                }),
        )
}

fn bar(color: impl Into<Hsla>, width: f32) -> Div {
    div().w(px(width)).h(px(4.)).rounded_full().bg(color.into())
}

fn app_card(app: &'static AppTheme, cx: &App) -> Stateful<Div> {
    let color = |hex: u32| -> Hsla { rgb(hex).into() };
    let preview = h_flex()
        .bg(color(app.background))
        .child(
            v_flex()
                .w(px(30.))
                .h_full()
                .p_1p5()
                .gap_1()
                .bg(color(app.surface))
                .child(bar(color(app.muted), 16.))
                .child(bar(color(app.primary), 12.))
                .child(bar(color(app.muted), 14.)),
        )
        .child(
            v_flex()
                .flex_1()
                .p_2()
                .gap_1p5()
                .child(bar(color(app.foreground), 52.))
                .child(bar(color(app.muted), 36.))
                .child(div().mt_0p5().w(px(26.)).h(px(9.)).rounded_sm().bg(color(app.primary))),
        );
    card(format!("app-theme-{}", app.name).into(), app.name, app.is_chosen(cx), preview, cx)
        .on_click(move |_, _, cx| choose(app, cx))
}

/// Every app theme for one mode.
pub fn app_themes(dark: bool, cx: &App) -> Div {
    grid().children(APP_THEMES.iter().filter(|t| t.dark == dark).map(|t| app_card(t, cx)))
}

fn scheme_card(scheme: &'static TerminalTheme, cx: &App) -> Stateful<Div> {
    let text = to_hsla(scheme.foreground);
    let ansi = |ix: usize| to_hsla(scheme.ansi[ix]);
    let word = |color: Hsla, word: &'static str| div().text_color(color).child(word);
    let preview = v_flex()
        .p_2()
        .gap_0p5()
        .bg(to_hsla(scheme.background))
        .font_family(SharedString::from(Settings::get(cx).font_family.clone()))
        .text_size(px(10.))
        .line_height(px(13.))
        .child(h_flex().child(word(ansi(2), "user@host")).child(word(text, ":~$ ls")))
        .child(
            h_flex()
                .gap_1p5()
                .child(word(ansi(4), "docs"))
                .child(word(ansi(2), "run.sh"))
                .child(word(text, "a.txt")),
        )
        .child(h_flex().mt_1().gap_1().children((1..=6).map(|ix| div().size(px(7.)).rounded_full().bg(ansi(ix)))));
    let picked = active_theme(cx).name == scheme.name;
    card(format!("terminal-theme-{}", scheme.name).into(), scheme.name, picked, preview, cx).on_click(move |_, _, cx| {
        // Picking colors by hand stops them following the app theme.
        Settings::update(cx, |s| {
            s.terminal_theme = scheme.name.to_owned();
            s.terminal_matches_theme = false;
        });
    })
}

/// Every terminal color scheme. The one in use now is marked.
pub fn terminal_schemes(cx: &App) -> Div {
    grid().children(THEMES.iter().map(|scheme| scheme_card(scheme, cx)))
}

/// A small heading over a set of cards.
pub fn section(title: &'static str, cards: Div, cx: &App) -> Div {
    v_flex()
        .w_full()
        .gap_2()
        .child(div().text_sm().font_medium().text_color(cx.theme().foreground).child(title))
        .child(cards)
}
