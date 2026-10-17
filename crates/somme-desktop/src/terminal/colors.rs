use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui_kit::{Hsla, Rgba};

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
        name: "Light",
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
];

pub fn theme_named(name: &str) -> &'static TerminalTheme {
    THEMES.iter().find(|theme| theme.name == name).unwrap_or(&THEMES[0])
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
