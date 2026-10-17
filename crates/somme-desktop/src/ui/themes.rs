//! App color themes. Each is a small palette that becomes a full UI theme, so
//! the sidebar, menus, buttons and lists all follow it.

use gpui_kit::component::{Theme, ThemeRegistry};
use gpui_kit::{App, Global, SharedString, px};
use serde_json::{Map, Value, json};

use crate::settings::{Appearance, Settings};

pub const DEFAULT_DARK: &str = "Default Dark";
pub const DEFAULT_LIGHT: &str = "Default Light";

#[derive(Clone, Copy, Debug)]
pub struct AppTheme {
    pub name: &'static str,
    pub dark: bool,
    /// Main content.
    pub background: u32,
    /// Sidebar, title bar and menus.
    pub surface: u32,
    /// Hovered and selected rows, secondary buttons.
    pub raised: u32,
    pub foreground: u32,
    pub muted: u32,
    pub border: u32,
    /// Buttons, links, focus rings and selections.
    pub primary: u32,
    pub on_primary: u32,
    pub red: u32,
    pub green: u32,
    pub yellow: u32,
    pub blue: u32,
    pub magenta: u32,
    pub cyan: u32,
}

/// Colors in order: background, surface, raised, foreground, muted, border,
/// primary, text on primary, then red, green, yellow, blue, magenta, cyan.
const fn theme(name: &'static str, dark: bool, c: [u32; 14]) -> AppTheme {
    AppTheme {
        name,
        dark,
        background: c[0],
        surface: c[1],
        raised: c[2],
        foreground: c[3],
        muted: c[4],
        border: c[5],
        primary: c[6],
        on_primary: c[7],
        red: c[8],
        green: c[9],
        yellow: c[10],
        blue: c[11],
        magenta: c[12],
        cyan: c[13],
    }
}

#[rustfmt::skip]
pub const APP_THEMES: &[AppTheme] = &[
    // The two themes that ship with the UI library. Listed only for their previews.
    theme(DEFAULT_DARK, true, [0x0a0a0a, 0x171717, 0x262626, 0xfafafa, 0xa3a3a3, 0x262626, 0xfafafa, 0x171717, 0xf87171, 0x4ade80, 0xfacc15, 0x60a5fa, 0xc084fc, 0x22d3ee]),
    theme("Somme Dark", true, [0x14161f, 0x1b1e2a, 0x262a39, 0xe6e8f0, 0x8b90a3, 0x2a2e3d, 0x4a99ff, 0xffffff, 0xf0525a, 0x5fd38d, 0xf5c25b, 0x4a99ff, 0xc678dd, 0x4fd0e0]),
    theme("Termius", true, [0x141729, 0x1c1f35, 0x262a45, 0xe8eaf2, 0x8c90a8, 0x2a2e4a, 0x478fef, 0xffffff, 0xff5f5f, 0x2ed47a, 0xffc94a, 0x478fef, 0xb57cf6, 0x47c8ef]),
    theme("Nord", true, [0x2e3440, 0x3b4252, 0x434c5e, 0xeceff4, 0x9ea7b8, 0x4c566a, 0x88c0d0, 0x2e3440, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb]),
    theme("Dracula", true, [0x282a36, 0x21222c, 0x44475a, 0xf8f8f2, 0xa3a8c3, 0x3a3c4e, 0xbd93f9, 0x282a36, 0xff5555, 0x50fa7b, 0xf1fa8c, 0x8be9fd, 0xff79c6, 0x8be9fd]),
    theme("Monokai", true, [0x272822, 0x1e1f1c, 0x3e3d32, 0xf8f8f2, 0xa59f85, 0x49483e, 0xa6e22e, 0x272822, 0xf92672, 0xa6e22e, 0xe6db74, 0x66d9ef, 0xae81ff, 0xa1efe4]),
    theme("One Dark", true, [0x282c34, 0x21252b, 0x2c313c, 0xabb2bf, 0x7f848e, 0x3e4451, 0x61afef, 0x282c34, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2]),
    theme("Tokyo Night", true, [0x1a1b26, 0x16161e, 0x292e42, 0xc0caf5, 0x787c99, 0x2f3549, 0x7aa2f7, 0x1a1b26, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff]),
    theme("Gruvbox Dark", true, [0x282828, 0x1d2021, 0x3c3836, 0xebdbb2, 0xa89984, 0x504945, 0xfabd2f, 0x282828, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c]),
    theme("Catppuccin Mocha", true, [0x1e1e2e, 0x181825, 0x313244, 0xcdd6f4, 0xa6adc8, 0x45475a, 0xcba6f7, 0x1e1e2e, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5]),
    theme("Solarized Dark", true, [0x002b36, 0x073642, 0x0b3d4a, 0x93a1a1, 0x6c8288, 0x0f4654, 0x268bd2, 0xfdf6e3, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198]),
    theme(DEFAULT_LIGHT, false, [0xffffff, 0xfafafa, 0xf5f5f5, 0x0a0a0a, 0x737373, 0xe5e5e5, 0x171717, 0xfafafa, 0xef4444, 0x22c55e, 0xeab308, 0x3b82f6, 0xa855f7, 0x06b6d4]),
    theme("Somme Light", false, [0xffffff, 0xf5f6f8, 0xeceef2, 0x1c1f26, 0x666b78, 0xe2e4e9, 0x2f6fed, 0xffffff, 0xd93843, 0x1f9d55, 0xb7791f, 0x2f6fed, 0x9b51e0, 0x0e9aa7]),
    theme("GitHub Light", false, [0xffffff, 0xf6f8fa, 0xeaeef2, 0x1f2328, 0x656d76, 0xd0d7de, 0x0969da, 0xffffff, 0xcf222e, 0x1a7f37, 0x9a6700, 0x0969da, 0x8250df, 0x1b7c83]),
    theme("Catppuccin Latte", false, [0xeff1f5, 0xe6e9ef, 0xdce0e8, 0x4c4f69, 0x6c6f85, 0xccd0da, 0x8839ef, 0xeff1f5, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299]),
    theme("Solarized Light", false, [0xfdf6e3, 0xeee8d5, 0xe6dfc8, 0x586e75, 0x839496, 0xddd6c1, 0x268bd2, 0xfdf6e3, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198]),
];

fn hex(color: u32) -> String {
    format!("#{color:06x}")
}

fn with_alpha(color: u32, alpha: u8) -> String {
    format!("#{color:06x}{alpha:02x}")
}

/// `a` moved toward `b` by `t` (0 to 1).
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let channel = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xff) as f32, ((b >> shift) & 0xff) as f32);
        ((x + (y - x) * t).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

impl AppTheme {
    fn is_builtin(&self) -> bool {
        self.name == DEFAULT_DARK || self.name == DEFAULT_LIGHT
    }

    /// The theme in the UI library's theme file format.
    fn config(&self) -> Value {
        let toward = if self.dark { 0xffffff } else { 0x000000 };
        let raised_hover = mix(self.raised, toward, 0.06);
        let primary_hover = mix(self.primary, toward, 0.12);
        let light = |color: u32| hex(mix(color, 0xffffff, 0.3));
        let pairs = [
            ("background", hex(self.background)),
            ("foreground", hex(self.foreground)),
            ("border", hex(self.border)),
            ("window.border", hex(self.border)),
            ("input.border", hex(self.border)),
            ("ring", hex(self.primary)),
            ("caret", hex(self.foreground)),
            ("accent.background", hex(self.raised)),
            ("accent.foreground", hex(self.foreground)),
            ("muted.background", hex(self.raised)),
            ("muted.foreground", hex(self.muted)),
            ("secondary.background", hex(self.raised)),
            ("secondary.hover.background", hex(raised_hover)),
            ("secondary.active.background", hex(raised_hover)),
            ("secondary.foreground", hex(self.foreground)),
            ("primary.background", hex(self.primary)),
            ("primary.hover.background", hex(primary_hover)),
            ("primary.active.background", hex(primary_hover)),
            ("primary.foreground", hex(self.on_primary)),
            ("popover.background", hex(if self.dark { self.surface } else { self.background })),
            ("popover.foreground", hex(self.foreground)),
            ("accordion.background", hex(self.background)),
            ("group_box.background", hex(self.surface)),
            ("group_box.foreground", hex(self.foreground)),
            ("description_list_label.background", hex(self.surface)),
            ("description_list_label.foreground", hex(self.foreground)),
            ("list.background", hex(self.background)),
            ("list.active.background", with_alpha(self.primary, 0x33)),
            ("list.active.border", hex(self.primary)),
            ("list.even.background", with_alpha(self.surface, 0x66)),
            ("list.head.background", hex(self.surface)),
            ("table.background", hex(self.background)),
            ("table.head.foreground", hex(self.muted)),
            ("table.row.border", hex(self.border)),
            ("sidebar.background", hex(self.surface)),
            ("sidebar.foreground", hex(self.foreground)),
            ("sidebar.border", hex(self.border)),
            ("sidebar.accent.background", hex(self.raised)),
            ("sidebar.accent.foreground", hex(self.foreground)),
            ("sidebar.primary.background", hex(self.primary)),
            ("sidebar.primary.foreground", hex(self.on_primary)),
            ("title_bar.background", hex(self.surface)),
            ("title_bar.border", hex(self.border)),
            ("status_bar.background", hex(self.surface)),
            ("status_bar.border", hex(self.border)),
            ("tab_bar.background", hex(self.surface)),
            ("tab_bar.segmented.background", hex(self.surface)),
            ("tab.background", "#00000000".to_owned()),
            ("tab.foreground", hex(self.muted)),
            ("tab.active.background", hex(self.background)),
            ("tab.active.foreground", hex(self.foreground)),
            ("selection.background", with_alpha(self.primary, 0x55)),
            ("scrollbar.background", "#00000000".to_owned()),
            ("scrollbar.thumb.background", with_alpha(self.muted, 0x66)),
            ("scrollbar.thumb.hover.background", with_alpha(self.muted, 0x99)),
            ("skeleton.background", hex(self.raised)),
            ("switch.background", hex(self.border)),
            ("slider.bar.background", hex(self.primary)),
            ("slider.thumb.background", hex(self.background)),
            ("progress_bar.background", hex(self.primary)),
            ("link.foreground", hex(self.primary)),
            ("link.hover.foreground", hex(primary_hover)),
            ("link.active.foreground", hex(self.primary)),
            ("drag_border", hex(self.primary)),
            ("drop_target.background", with_alpha(self.primary, 0x19)),
            ("danger.background", hex(self.red)),
            ("success.background", hex(self.green)),
            ("warning.background", hex(self.yellow)),
            ("info.background", hex(self.cyan)),
            ("base.red", hex(self.red)),
            ("base.red.light", light(self.red)),
            ("base.green", hex(self.green)),
            ("base.green.light", light(self.green)),
            ("base.yellow", hex(self.yellow)),
            ("base.yellow.light", light(self.yellow)),
            ("base.blue", hex(self.blue)),
            ("base.blue.light", light(self.blue)),
            ("base.magenta", hex(self.magenta)),
            ("base.magenta.light", light(self.magenta)),
            ("base.cyan", hex(self.cyan)),
            ("base.cyan.light", light(self.cyan)),
        ];
        let colors: Map<String, Value> = pairs.into_iter().map(|(key, value)| (key.to_owned(), Value::String(value))).collect();
        json!({
            "name": self.name,
            "mode": if self.dark { "dark" } else { "light" },
            "colors": colors,
            "highlight": self.highlight(),
        })
    }
}

impl AppTheme {
    /// Code colours for the file editor, from the same palette.
    fn highlight(&self) -> Value {
        let style = |color: u32| json!({ "color": hex(color) });
        let italic = |color: u32| json!({ "color": hex(color), "font_style": "italic" });
        let comment = mix(self.muted, self.background, 0.15);
        let syntax: Map<String, Value> = [
            ("attribute", style(self.yellow)),
            ("boolean", style(self.magenta)),
            ("comment", italic(comment)),
            ("comment_doc", italic(comment)),
            ("constant", style(self.magenta)),
            ("constructor", style(self.yellow)),
            ("embedded", style(self.foreground)),
            ("enum", style(self.yellow)),
            ("function", style(self.blue)),
            ("keyword", style(self.magenta)),
            ("label", style(self.cyan)),
            ("link_text", style(self.blue)),
            ("link_uri", style(self.cyan)),
            ("number", style(self.yellow)),
            ("operator", style(self.cyan)),
            ("preproc", style(self.magenta)),
            ("property", style(self.red)),
            ("punctuation", style(self.muted)),
            ("punctuation.bracket", style(self.muted)),
            ("punctuation.delimiter", style(self.muted)),
            ("string", style(self.green)),
            ("string.escape", style(self.cyan)),
            ("string.regex", style(self.cyan)),
            ("string.special", style(self.cyan)),
            ("tag", style(self.red)),
            ("title", json!({ "color": hex(self.blue), "font_weight": 700 })),
            ("type", style(self.yellow)),
            ("variable", style(self.foreground)),
            ("variable.special", style(self.red)),
            ("variant", style(self.yellow)),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
        json!({
            "editor.background": hex(self.background),
            "editor.foreground": hex(self.foreground),
            "editor.active_line.background": with_alpha(self.foreground, 0x0c),
            "editor.line_number": hex(mix(self.muted, self.background, 0.35)),
            "editor.active_line_number": hex(self.foreground),
            "editor.invisible": hex(mix(self.muted, self.background, 0.5)),
            "editor.gutter.background": hex(self.background),
            "error": hex(self.red),
            "warning": hex(self.yellow),
            "info": hex(self.blue),
            "hint": hex(self.muted),
            "success": hex(self.green),
            "syntax": syntax,
        })
    }
}

/// The UI font the app starts with, put back when the terminal font is kept to terminals.
struct SystemFont(SharedString);

impl Global for SystemFont {}

/// Adds the bundled themes to the UI library. Runs once at start.
pub fn register(cx: &mut App) {
    let system_font = Theme::global(cx).font_family.clone();
    cx.set_global(SystemFont(system_font));
    let themes: Vec<Value> = APP_THEMES.iter().filter(|t| !t.is_builtin()).map(AppTheme::config).collect();
    let set = json!({ "name": "Somme", "themes": themes });
    if let Err(err) = ThemeRegistry::global_mut(cx).load_themes_from_str(&set.to_string()) {
        tracing::warn!(%err, "could not load app themes");
    }
}

/// Points the app at the chosen light and dark themes. The caller then sets the mode.
pub fn install(cx: &mut App) {
    let settings = Settings::get(cx);
    let registry = ThemeRegistry::global(cx);
    let pick = |name: &str, dark: bool| {
        registry
            .themes()
            .get(name)
            .filter(|theme| theme.mode.is_dark() == dark)
            .cloned()
            .unwrap_or_else(|| {
                if dark {
                    registry.default_dark_theme().clone()
                } else {
                    registry.default_light_theme().clone()
                }
            })
    };
    let light = pick(&settings.light_theme, false);
    let dark = pick(&settings.dark_theme, true);
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
}

impl AppTheme {
    pub fn is_chosen(&self, cx: &App) -> bool {
        let settings = Settings::get(cx);
        let chosen = if self.dark { &settings.dark_theme } else { &settings.light_theme };
        *chosen == self.name
    }
}

/// Makes `theme` the one for its mode. Unless the app follows the system, it
/// also switches to that mode so the pick shows right away.
pub fn choose(theme: &AppTheme, cx: &mut App) {
    Settings::update(cx, |s| {
        if theme.dark {
            s.dark_theme = theme.name.to_owned();
        } else {
            s.light_theme = theme.name.to_owned();
        }
        if s.appearance != Appearance::System {
            s.appearance = if theme.dark { Appearance::Dark } else { Appearance::Light };
        }
    });
    crate::apply_appearance(cx);
}

/// Uses the chosen font in the whole app, or only in terminals and code.
/// Runs whenever settings change, so it does nothing when the fonts are already right.
pub fn apply_font(cx: &mut App) {
    let settings = Settings::get(cx);
    let mono = SharedString::from(settings.font_family.clone());
    let ui = if settings.font_everywhere { mono.clone() } else { cx.global::<SystemFont>().0.clone() };
    let size = px(settings.ui_font_size.clamp(12., 24.));
    let theme = Theme::global(cx);
    if theme.font_family != ui || theme.mono_font_family != mono || theme.font_size != size {
        Theme::update(cx, |theme| {
            theme.font_family = ui;
            theme.mono_font_family = mono;
            theme.font_size = size;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_moves_each_channel() {
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
        assert_eq!(mix(0x102030, 0x102030, 0.7), 0x102030);
    }

    #[test]
    fn names_are_unique_and_config_has_mode() {
        let mut names: Vec<_> = APP_THEMES.iter().map(|t| t.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), APP_THEMES.len());
        let config = APP_THEMES[1].config();
        assert_eq!(config["mode"], "dark");
        assert_eq!(config["colors"]["background"], "#14161f");
    }
}
