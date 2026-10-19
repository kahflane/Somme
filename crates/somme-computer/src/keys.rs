//! Key names, the same on every system: `cmd` is the Command, Windows or
//! Super key.

use enigo::Key;

/// Keys pressed together, like `cmd+shift+t`, first to last.
pub fn parse(keys: &str) -> Result<Vec<Key>, String> {
    let keys = keys.trim();
    if keys.is_empty() {
        return Err("no keys given.".into());
    }
    // A lone `+`, or one at the end like `cmd++`, is the plus key.
    let mut names: Vec<&str> = keys.split('+').map(str::trim).collect();
    if keys.ends_with('+') {
        names.retain(|name| !name.is_empty());
        names.push("+");
    }
    names.iter().map(|name| key(name)).collect()
}

/// True when `key` is Esc.
pub fn is_escape(key: &Key) -> bool {
    *key == Key::Escape
}

fn key(name: &str) -> Result<Key, String> {
    let lower = name.to_lowercase();
    let named = match lower.as_str() {
        "cmd" | "command" | "meta" | "super" | "win" | "windows" => Key::Meta,
        "ctrl" | "control" => Key::Control,
        "shift" => Key::Shift,
        "alt" | "option" | "opt" => Key::Alt,
        "return" | "enter" => Key::Return,
        "esc" | "escape" => Key::Escape,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "page_up" | "pgup" => Key::PageUp,
        "pagedown" | "page_down" | "pgdn" => Key::PageDown,
        "up" | "arrowup" | "up_arrow" => Key::UpArrow,
        "down" | "arrowdown" | "down_arrow" => Key::DownArrow,
        "left" | "arrowleft" | "left_arrow" => Key::LeftArrow,
        "right" | "arrowright" | "right_arrow" => Key::RightArrow,
        "plus" => Key::Unicode('+'),
        "minus" => Key::Unicode('-'),
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        _ => {
            let mut chars = lower.chars();
            match (chars.next(), chars.next()) {
                (Some(char), None) => Key::Unicode(char),
                _ => return Err(format!("{name} is not a key Somme knows.")),
            }
        }
    };
    Ok(named)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_read_the_same_everywhere() {
        assert_eq!(parse("cmd+shift+T").unwrap(), [Key::Meta, Key::Shift, Key::Unicode('t')]);
        assert_eq!(parse("Return").unwrap(), [Key::Return]);
        assert_eq!(parse("ctrl + c").unwrap(), [Key::Control, Key::Unicode('c')]);
        assert_eq!(parse("cmd++").unwrap(), [Key::Meta, Key::Unicode('+')]);
        assert!(parse("cmd+banana").unwrap_err().contains("banana"));
        assert!(parse(" ").is_err());
        assert!(is_escape(&parse("esc").unwrap()[0]));
    }
}
