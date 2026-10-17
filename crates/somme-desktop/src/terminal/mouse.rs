use alacritty_terminal::term::TermMode;
use gpui_kit::Modifiers;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportButton {
    Left,
    Middle,
    Right,
    /// Motion with no button held.
    NoButton,
    WheelUp,
    WheelDown,
}

impl ReportButton {
    fn code(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
            Self::NoButton => 3,
            Self::WheelUp => 64,
            Self::WheelDown => 65,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportKind {
    Press,
    Release,
    Motion,
}

/// Whether the running program asked for this kind of mouse event.
pub fn wants(mode: &TermMode, kind: ReportKind, button: ReportButton) -> bool {
    match kind {
        ReportKind::Press | ReportKind::Release => mode.intersects(TermMode::MOUSE_MODE),
        ReportKind::Motion if button == ReportButton::NoButton => mode.contains(TermMode::MOUSE_MOTION),
        ReportKind::Motion => mode.intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG),
    }
}

/// Encodes a mouse event for the program, in SGR form when it asked for that.
/// `column` and `line` are zero-based cell positions.
pub fn encode(
    mode: &TermMode,
    kind: ReportKind,
    button: ReportButton,
    modifiers: &Modifiers,
    column: usize,
    line: usize,
) -> Option<Vec<u8>> {
    if !wants(mode, kind, button) {
        return None;
    }
    let mut code = button.code();
    if modifiers.shift {
        code += 4;
    }
    if modifiers.alt {
        code += 8;
    }
    if modifiers.control {
        code += 16;
    }
    if kind == ReportKind::Motion {
        code += 32;
    }

    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if kind == ReportKind::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{end}", column + 1, line + 1).into_bytes());
    }

    // Classic X10 encoding cannot say which button was released.
    if kind == ReportKind::Release {
        code = 3 + (code & !3);
    }
    let encode_pos = |pos: usize| -> Option<Vec<u8>> {
        let value = pos + 1 + 32;
        if mode.contains(TermMode::UTF8_MOUSE) {
            let ch = char::from_u32(u32::try_from(value).ok()?)?;
            Some(ch.to_string().into_bytes())
        } else if value <= 255 {
            Some(vec![u8::try_from(value).ok()?])
        } else {
            None
        }
    };
    let mut out = vec![0x1b, b'[', b'M', 32 + code];
    out.extend(encode_pos(column)?);
    out.extend(encode_pos(line)?);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_press_and_release() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let none = Modifiers::default();
        let press = encode(&mode, ReportKind::Press, ReportButton::Left, &none, 4, 9);
        assert_eq!(press.as_deref(), Some(&b"\x1b[<0;5;10M"[..]));
        let release = encode(&mode, ReportKind::Release, ReportButton::Left, &none, 4, 9);
        assert_eq!(release.as_deref(), Some(&b"\x1b[<0;5;10m"[..]));
    }

    #[test]
    fn nothing_is_sent_without_mouse_mode() {
        let none = Modifiers::default();
        assert!(encode(&TermMode::NONE, ReportKind::Press, ReportButton::Left, &none, 0, 0).is_none());
    }

    #[test]
    fn x10_encoding() {
        let none = Modifiers::default();
        let out = encode(&TermMode::MOUSE_REPORT_CLICK, ReportKind::Press, ReportButton::WheelUp, &none, 0, 0);
        assert_eq!(out, Some(vec![0x1b, b'[', b'M', 32 + 64, 33, 33]));
    }
}
