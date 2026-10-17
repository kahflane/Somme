use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Hsla, Rgba};

use crate::settings::Settings;

/// A terminal color scheme: 16 ANSI colors plus the default text, background,
/// cursor and selection colors.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalTheme {
    pub name: &'static str,
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection: Rgb,
    pub ansi: [Rgb; 16],
}

const fn rgb(hex: u32) -> Rgb {
    Rgb {
        r: (hex >> 16) as u8,
        g: (hex >> 8) as u8,
        b: hex as u8,
    }
}

/// Colors in order: text, background, cursor, selection, then the 16 ANSI colors.
const fn scheme(name: &'static str, colors: [u32; 20]) -> TerminalTheme {
    let mut ansi = [rgb(0); 16];
    let mut ix = 0;
    while ix < 16 {
        ansi[ix] = rgb(colors[ix + 4]);
        ix += 1;
    }
    TerminalTheme {
        name,
        foreground: rgb(colors[0]),
        background: rgb(colors[1]),
        cursor: rgb(colors[2]),
        selection: rgb(colors[3]),
        ansi,
    }
}

#[rustfmt::skip]
pub const THEMES: &[TerminalTheme] = &[
    TerminalTheme {
        name: "Somme Dark",
        foreground: rgb(0xe6e8f0),
        background: rgb(0x14161f),
        cursor: rgb(0x4a99ff),
        selection: rgb(0x2f4a78),
        ansi: [
            rgb(0x1d1f2b),
            rgb(0xf0525a),
            rgb(0x5fd38d),
            rgb(0xf5c25b),
            rgb(0x4a99ff),
            rgb(0xc678dd),
            rgb(0x4fd0e0),
            rgb(0xc8ccd8),
            rgb(0x5c6070),
            rgb(0xff6b72),
            rgb(0x7ee2a6),
            rgb(0xffd479),
            rgb(0x74b1ff),
            rgb(0xd99cf0),
            rgb(0x78e3f0),
            rgb(0xffffff),
        ],
    },
    TerminalTheme {
        name: "Termius",
        foreground: rgb(0xffffff),
        background: rgb(0x141729),
        cursor: rgb(0x478fef),
        selection: rgb(0x2c4f84),
        ansi: [
            rgb(0x1c1e30),
            rgb(0xff5f5f),
            rgb(0x2ed47a),
            rgb(0xffc94a),
            rgb(0x478fef),
            rgb(0xb57cf6),
            rgb(0x47c8ef),
            rgb(0xc5c8d6),
            rgb(0x60647a),
            rgb(0xff8080),
            rgb(0x5ae29a),
            rgb(0xffd86e),
            rgb(0x6ea8f5),
            rgb(0xc89bf8),
            rgb(0x6fd6f5),
            rgb(0xffffff),
        ],
    },
    TerminalTheme {
        name: "Solarized Dark",
        foreground: rgb(0x93a1a1),
        background: rgb(0x002b36),
        cursor: rgb(0x93a1a1),
        selection: rgb(0x0f4a5a),
        ansi: [
            rgb(0x073642),
            rgb(0xdc322f),
            rgb(0x859900),
            rgb(0xb58900),
            rgb(0x268bd2),
            rgb(0xd33682),
            rgb(0x2aa198),
            rgb(0xeee8d5),
            rgb(0x586e75),
            rgb(0xcb4b16),
            rgb(0x93a1a1),
            rgb(0x839496),
            rgb(0x657b83),
            rgb(0x6c71c4),
            rgb(0x93a1a1),
            rgb(0xfdf6e3),
        ],
    },
    TerminalTheme {
        name: "Dracula",
        foreground: rgb(0xf8f8f2),
        background: rgb(0x282a36),
        cursor: rgb(0xf8f8f2),
        selection: rgb(0x44475a),
        ansi: [
            rgb(0x21222c),
            rgb(0xff5555),
            rgb(0x50fa7b),
            rgb(0xf1fa8c),
            rgb(0xbd93f9),
            rgb(0xff79c6),
            rgb(0x8be9fd),
            rgb(0xf8f8f2),
            rgb(0x6272a4),
            rgb(0xff6e6e),
            rgb(0x69ff94),
            rgb(0xffffa5),
            rgb(0xd6acff),
            rgb(0xff92df),
            rgb(0xa4ffff),
            rgb(0xffffff),
        ],
    },
    TerminalTheme {
        name: "Monokai",
        foreground: rgb(0xf8f8f2),
        background: rgb(0x272822),
        cursor: rgb(0xf8f8f0),
        selection: rgb(0x49483e),
        ansi: [
            rgb(0x272822),
            rgb(0xf92672),
            rgb(0xa6e22e),
            rgb(0xf4bf75),
            rgb(0x66d9ef),
            rgb(0xae81ff),
            rgb(0xa1efe4),
            rgb(0xf8f8f2),
            rgb(0x75715e),
            rgb(0xf92672),
            rgb(0xa6e22e),
            rgb(0xf4bf75),
            rgb(0x66d9ef),
            rgb(0xae81ff),
            rgb(0xa1efe4),
            rgb(0xf9f8f5),
        ],
    },
    TerminalTheme {
        name: "Nord",
        foreground: rgb(0xd8dee9),
        background: rgb(0x2e3440),
        cursor: rgb(0xd8dee9),
        selection: rgb(0x434c5e),
        ansi: [
            rgb(0x3b4252),
            rgb(0xbf616a),
            rgb(0xa3be8c),
            rgb(0xebcb8b),
            rgb(0x81a1c1),
            rgb(0xb48ead),
            rgb(0x88c0d0),
            rgb(0xe5e9f0),
            rgb(0x4c566a),
            rgb(0xbf616a),
            rgb(0xa3be8c),
            rgb(0xebcb8b),
            rgb(0x81a1c1),
            rgb(0xb48ead),
            rgb(0x8fbcbb),
            rgb(0xeceff4),
        ],
    },
    TerminalTheme {
        name: "GitHub Light",
        foreground: rgb(0x24292f),
        background: rgb(0xffffff),
        cursor: rgb(0x0969da),
        selection: rgb(0xb6d6fb),
        ansi: [
            rgb(0x24292f),
            rgb(0xcf222e),
            rgb(0x116329),
            rgb(0x9a6700),
            rgb(0x0969da),
            rgb(0x8250df),
            rgb(0x1b7c83),
            rgb(0x6e7781),
            rgb(0x57606a),
            rgb(0xa40e26),
            rgb(0x1a7f37),
            rgb(0x633c01),
            rgb(0x218bff),
            rgb(0xa475f9),
            rgb(0x3192aa),
            rgb(0x8c959f),
        ],
    },
    scheme("One Dark", [0xabb2bf, 0x282c34, 0x528bff, 0x3e4451, 0x3f4451, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff]),
    scheme("Tokyo Night", [0xc0caf5, 0x1a1b26, 0xc0caf5, 0x33467c, 0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, 0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5]),
    scheme("Gruvbox Dark", [0xebdbb2, 0x282828, 0xebdbb2, 0x504945, 0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984, 0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2]),
    scheme("Catppuccin Mocha", [0xcdd6f4, 0x1e1e2e, 0xf5e0dc, 0x585b70, 0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, 0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8]),
    scheme("Catppuccin Latte", [0x4c4f69, 0xeff1f5, 0xdc8a78, 0xacb0be, 0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be, 0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc]),
    scheme("Default Dark", [0xfafafa, 0x0a0a0a, 0xfafafa, 0x404040, 0x262626, 0xf87171, 0x4ade80, 0xfacc15, 0x60a5fa, 0xc084fc, 0x22d3ee, 0xd4d4d4, 0x525252, 0xfca5a5, 0x86efac, 0xfde047, 0x93c5fd, 0xd8b4fe, 0x67e8f9, 0xfafafa]),
    scheme("Default Light", [0x0a0a0a, 0xffffff, 0x171717, 0xd4d4d4, 0x171717, 0xdc2626, 0x16a34a, 0xca8a04, 0x2563eb, 0x9333ea, 0x0891b2, 0xa3a3a3, 0x525252, 0xef4444, 0x22c55e, 0xeab308, 0x3b82f6, 0xa855f7, 0x06b6d4, 0xe5e5e5]),
    scheme("Somme Light", [0x1c1f26, 0xffffff, 0x2f6fed, 0xc9daf8, 0x1c1f26, 0xd93843, 0x1f9d55, 0xb7791f, 0x2f6fed, 0x9b51e0, 0x0e9aa7, 0xa0a4ad, 0x666b78, 0xe5535d, 0x2bb46a, 0xd08d2a, 0x4f86f0, 0xae6ce6, 0x1cb3c1, 0xe2e4e9]),
    scheme("Solarized Light", [0x657b83, 0xfdf6e3, 0x586e75, 0xe3dcc6, 0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, 0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3]),
];

fn find(name: &str) -> Option<&'static TerminalTheme> {
    THEMES.iter().find(|theme| theme.name == name)
}

pub fn theme_named(name: &str) -> &'static TerminalTheme {
    // Older settings saved the GitHub scheme as "Light".
    let name = if name == "Light" { "GitHub Light" } else { name };
    find(name).unwrap_or(&THEMES[0])
}

/// The colors terminals use now: the app theme's own, unless the user picked others.
pub fn active_theme(cx: &App) -> &'static TerminalTheme {
    let settings = Settings::get(cx);
    if !settings.terminal_matches_theme {
        return theme_named(&settings.terminal_theme);
    }
    let theme = cx.theme();
    find(theme.theme_name()).unwrap_or_else(|| theme_named(if theme.is_dark() { "Somme Dark" } else { "Somme Light" }))
}

pub fn to_hsla(color: Rgb) -> Hsla {
    Rgba {
        r: f32::from(color.r) / 255.,
        g: f32::from(color.g) / 255.,
        b: f32::from(color.b) / 255.,
        a: 1.,
    }
    .into()
}

fn dim(color: Rgb) -> Rgb {
    let scale = |v: u8| ((u16::from(v) * 2) / 3) as u8;
    Rgb {
        r: scale(color.r),
        g: scale(color.g),
        b: scale(color.b),
    }
}

/// Maps xterm's 256-color index to RGB: 16 theme colors, a 6x6x6 cube, then grays.
fn indexed(index: u8, theme: &TerminalTheme) -> Rgb {
    match index {
        0..=15 => theme.ansi[usize::from(index)],
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
            Rgb {
                r: level(i / 36),
                g: level((i / 6) % 6),
                b: level(i % 6),
            }
        }
        _ => {
            let v = (index - 232) * 10 + 8;
            Rgb { r: v, g: v, b: v }
        }
    }
}

/// Resolves a cell color, honoring colors the remote program changed with OSC 4/10/11.
pub fn resolve(color: Color, overrides: &Colors, theme: &TerminalTheme) -> Rgb {
    match color {
        Color::Spec(rgb) => rgb,
        Color::Indexed(index) => overrides[usize::from(index)].unwrap_or_else(|| indexed(index, theme)),
        Color::Named(named) => {
            if let Some(rgb) = overrides[named] {
                return rgb;
            }
            match named {
                NamedColor::Foreground | NamedColor::BrightForeground => theme.foreground,
                NamedColor::Background => theme.background,
                NamedColor::Cursor => theme.cursor,
                NamedColor::DimForeground => dim(theme.foreground),
                NamedColor::DimBlack => dim(theme.ansi[0]),
                NamedColor::DimRed => dim(theme.ansi[1]),
                NamedColor::DimGreen => dim(theme.ansi[2]),
                NamedColor::DimYellow => dim(theme.ansi[3]),
                NamedColor::DimBlue => dim(theme.ansi[4]),
                NamedColor::DimMagenta => dim(theme.ansi[5]),
                NamedColor::DimCyan => dim(theme.ansi[6]),
                NamedColor::DimWhite => dim(theme.ansi[7]),
                other => theme.ansi[(other as usize).min(15)],
            }
        }
    }
}

/// Bold text in one of the 8 normal colors is shown in the bright version.
pub fn brighten(color: Color) -> Color {
    match color {
        Color::Named(named) if (named as usize) < 8 => Color::Indexed(named as u8 + 8),
        Color::Indexed(index) if index < 8 => Color::Indexed(index + 8),
        other => other,
    }
}

pub fn dimmed(color: Color) -> Color {
    match color {
        Color::Named(NamedColor::Foreground) => Color::Named(NamedColor::DimForeground),
        Color::Named(named) if (named as usize) < 8 => match named as usize {
            0 => Color::Named(NamedColor::DimBlack),
            1 => Color::Named(NamedColor::DimRed),
            2 => Color::Named(NamedColor::DimGreen),
            3 => Color::Named(NamedColor::DimYellow),
            4 => Color::Named(NamedColor::DimBlue),
            5 => Color::Named(NamedColor::DimMagenta),
            6 => Color::Named(NamedColor::DimCyan),
            _ => Color::Named(NamedColor::DimWhite),
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_and_gray_ramp_match_xterm() {
        let theme = &THEMES[0];
        assert_eq!(indexed(16, theme), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(indexed(196, theme), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(indexed(231, theme), Rgb { r: 255, g: 255, b: 255 });
        assert_eq!(indexed(232, theme), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(indexed(255, theme), Rgb { r: 238, g: 238, b: 238 });
    }

    #[test]
    fn bold_uses_bright_colors() {
        assert_eq!(brighten(Color::Named(NamedColor::Red)), Color::Indexed(9));
        assert_eq!(brighten(Color::Indexed(12)), Color::Indexed(12));
    }
}
