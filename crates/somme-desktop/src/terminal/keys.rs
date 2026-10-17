use std::borrow::Cow;

use alacritty_terminal::term::TermMode;
use gpui_kit::{Keystroke, Modifiers};

/// xterm's modifier parameter: 1 + shift + 2*alt + 4*ctrl.
fn modifier_code(m: &Modifiers) -> u8 {
    1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.control)
}

fn has_modifiers(m: &Modifiers) -> bool {
    m.shift || m.alt || m.control
}

/// `CSI 1 ; m X` when modified, otherwise `CSI X` (or `SS3 X` in application cursor mode).
fn cursor_key(letter: char, m: &Modifiers, mode: &TermMode) -> String {
    if has_modifiers(m) {
        format!("\x1b[1;{}{letter}", modifier_code(m))
    } else if mode.contains(TermMode::APP_CURSOR) {
        format!("\x1bO{letter}")
    } else {
        format!("\x1b[{letter}")
    }
}

/// `CSI n ~`, or `CSI n ; m ~` when modified.
fn tilde_key(code: u8, m: &Modifiers) -> String {
    if has_modifiers(m) {
        format!("\x1b[{code};{}~", modifier_code(m))
    } else {
        format!("\x1b[{code}~")
    }
}

fn function_key(n: u8, m: &Modifiers) -> Option<String> {
    let letter = match n {
        1 => 'P',
        2 => 'Q',
        3 => 'R',
        4 => 'S',
        _ => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                13 => 25,
                14 => 26,
                15 => 28,
                16 => 29,
                17 => 31,
                18 => 32,
                19 => 33,
                20 => 34,
                _ => return None,
            };
            return Some(tilde_key(code, m));
        }
    };
    Some(if has_modifiers(m) {
        format!("\x1b[1;{}{letter}", modifier_code(m))
    } else {
        format!("\x1bO{letter}")
    })
}

fn control_byte(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return match key {
            "space" => Some(0),
            _ => None,
        };
    }
    match ch {
        'a'..='z' => Some(ch as u8 - b'a' + 1),
        '@' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '7' => Some(0x1f),
        '8' | '?' => Some(0x7f),
        _ => None,
    }
}

/// Bytes to send for keys that do not type plain text, like arrows, Enter or Ctrl-C.
/// Plain characters return `None` and arrive through the text input path instead,
/// so input methods (Chinese, Japanese, accents) keep working.
pub fn to_esc_str(keystroke: &Keystroke, mode: &TermMode, option_as_meta: bool) -> Option<Cow<'static, str>> {
    let m = &keystroke.modifiers;
    if m.platform {
        return None;
    }
    let key = keystroke.key.as_str();

    let special: Option<Cow<'static, str>> = match key {
        "enter" if m.alt => Some("\x1b\r".into()),
        "enter" => Some("\r".into()),
        "tab" if m.shift => Some("\x1b[Z".into()),
        "tab" if !m.control && !m.alt => Some("\t".into()),
        "escape" => Some("\x1b".into()),
        "backspace" if m.control => Some("\x08".into()),
        "backspace" if m.alt => Some("\x1b\x7f".into()),
        "backspace" => Some("\x7f".into()),
        "up" => Some(cursor_key('A', m, mode).into()),
        "down" => Some(cursor_key('B', m, mode).into()),
        "right" => Some(cursor_key('C', m, mode).into()),
        "left" => Some(cursor_key('D', m, mode).into()),
        "home" => Some(cursor_key('H', m, mode).into()),
        "end" => Some(cursor_key('F', m, mode).into()),
        "insert" => Some(tilde_key(2, m).into()),
        "delete" => Some(tilde_key(3, m).into()),
        "pageup" => Some(tilde_key(5, m).into()),
        "pagedown" => Some(tilde_key(6, m).into()),
        _ => None,
    };
    if special.is_some() {
        return special;
    }

    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return function_key(n, m).map(Cow::Owned);
    }

    if m.control && !m.alt {
        return control_byte(key).map(|byte| Cow::Owned(char::from(byte).to_string()));
    }

    if m.alt && option_as_meta {
        if m.control {
            return control_byte(key).map(|byte| Cow::Owned(format!("\x1b{}", char::from(byte))));
        }
        let text = match key {
            "space" => " ".to_owned(),
            _ if key.chars().count() == 1 && m.shift => key.to_uppercase(),
            _ if key.chars().count() == 1 => key.to_owned(),
            _ => return None,
        };
        return Some(Cow::Owned(format!("\x1b{text}")));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(source: &str) -> Keystroke {
        Keystroke::parse(source).expect("valid keystroke")
    }

    fn esc(source: &str) -> Option<String> {
        to_esc_str(&stroke(source), &TermMode::NONE, true).map(Cow::into_owned)
    }

    #[test]
    fn control_letters_become_c0_bytes() {
        assert_eq!(esc("ctrl-c").as_deref(), Some("\x03"));
        assert_eq!(esc("ctrl-a").as_deref(), Some("\x01"));
        assert_eq!(esc("ctrl-space").as_deref(), Some("\x00"));
        assert_eq!(esc("ctrl-[").as_deref(), Some("\x1b"));
    }

    #[test]
    fn arrows_follow_cursor_mode_and_modifiers() {
        assert_eq!(esc("up").as_deref(), Some("\x1b[A"));
        let app = to_esc_str(&stroke("up"), &TermMode::APP_CURSOR, true).map(Cow::into_owned);
        assert_eq!(app.as_deref(), Some("\x1bOA"));
        assert_eq!(esc("ctrl-left").as_deref(), Some("\x1b[1;5D"));
        assert_eq!(esc("shift-right").as_deref(), Some("\x1b[1;2C"));
    }

    #[test]
    fn function_and_editing_keys() {
        assert_eq!(esc("f1").as_deref(), Some("\x1bOP"));
        assert_eq!(esc("f5").as_deref(), Some("\x1b[15~"));
        assert_eq!(esc("shift-f5").as_deref(), Some("\x1b[15;2~"));
        assert_eq!(esc("delete").as_deref(), Some("\x1b[3~"));
        assert_eq!(esc("pageup").as_deref(), Some("\x1b[5~"));
    }

    #[test]
    fn option_works_as_meta() {
        assert_eq!(esc("alt-b").as_deref(), Some("\x1bb"));
        assert_eq!(esc("alt-shift-b").as_deref(), Some("\x1bB"));
        let typed = to_esc_str(&stroke("alt-b"), &TermMode::NONE, false);
        assert!(typed.is_none());
    }

    #[test]
    fn plain_text_and_command_keys_are_left_alone() {
        assert_eq!(esc("a"), None);
        assert_eq!(esc("cmd-c"), None);
        assert_eq!(esc("shift-a"), None);
    }
}
