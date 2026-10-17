//! The system's own alerts and notifications, so the app does not feel like a web page.
//!
//! macOS and Windows have an alert built in. Linux does not, so there it runs zenity or
//! kdialog when one is installed, and draws the alert itself when neither is.

use futures::channel::oneshot;
use gpui_kit::{App, PromptButton, PromptLevel, SystemNotification, Window};

/// Shows an alert with `buttons` and gives back the index of the one picked.
/// The first button is the default. A `PromptButton::cancel` one also answers Escape.
pub fn ask(
    level: PromptLevel,
    title: &str,
    detail: Option<&str>,
    buttons: &[PromptButton],
    window: &mut Window,
    cx: &mut App,
) -> oneshot::Receiver<usize> {
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(answer) = dialog_tool::ask(level, title, detail, buttons) {
        return answer;
    }
    window.prompt(level, title, detail, buttons, cx)
}

/// Asks before doing something. `on_ok` runs when the user picks `ok`.
pub fn confirm(
    title: &str,
    detail: &str,
    ok: &str,
    on_ok: impl FnOnce(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let buttons = [PromptButton::ok(ok.to_owned()), PromptButton::cancel("Cancel")];
    let answer = ask(PromptLevel::Warning, title, Some(detail), &buttons, window, cx);
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        if answer.await == Ok(0) {
            let _ = handle.update(cx, |_, window, cx| on_ok(window, cx));
        }
    })
    .detach();
}

/// Tells the user something went wrong, with one OK button.
pub fn error(title: &str, detail: &str, window: &mut Window, cx: &mut App) {
    // Nothing to wait for: the alert closes itself.
    drop(ask(PromptLevel::Warning, title, Some(detail), &[PromptButton::ok("OK")], window, cx));
}

/// Posts a note to the system's notification center. A newer note with the same
/// title replaces the older one.
pub fn notify(title: &str, body: &str, cx: &App) {
    cx.show_system_notification(SystemNotification {
        tag: title.to_owned().into(),
        title: title.to_owned().into(),
        body: body.to_owned().into(),
        actions: Vec::new(),
    });
}

/// zenity and kdialog, the alert programs most Linux desktops ship.
#[cfg(any(all(unix, not(target_os = "macos")), test))]
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
mod dialog_tool {
    use std::process::{Command, Stdio};

    use futures::channel::oneshot;
    use gpui_kit::{PromptButton, PromptLevel};

    use crate::ui::common::APP_NAME;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Tool {
        Zenity,
        Kdialog,
    }

    /// None when no tool is installed, or the alert needs more than two buttons.
    pub fn ask(
        level: PromptLevel,
        title: &str,
        detail: Option<&str>,
        buttons: &[PromptButton],
    ) -> Option<oneshot::Receiver<usize>> {
        if buttons.is_empty() || buttons.len() > 2 {
            return None;
        }
        let tool = find_tool()?;
        // The tools have a yes button and a no button, and Escape means no.
        let cancel = buttons.iter().position(PromptButton::is_cancel);
        let (yes, no) = match (buttons.len(), cancel) {
            (1, _) => (0, None),
            (_, Some(cancel)) => (1 - cancel, Some(cancel)),
            (_, None) => (0, Some(1)),
        };
        let label = |ix: usize| buttons[ix].label().to_string();
        let no_label = no.map(label);
        let mut command = command(tool, level, &text(title, detail), &label(yes), no_label.as_deref(), no == Some(0));
        let (tx, rx) = oneshot::channel();
        std::thread::spawn(move || {
            let said_yes = command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            let _ = tx.send(if said_yes { yes } else { no.unwrap_or(yes) });
        });
        Some(rx)
    }

    fn find_tool() -> Option<Tool> {
        let kde = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| desktop.to_uppercase().contains("KDE"));
        let order = if kde { [Tool::Kdialog, Tool::Zenity] } else { [Tool::Zenity, Tool::Kdialog] };
        order.into_iter().find(|tool| on_path(tool.program()))
    }

    impl Tool {
        fn program(self) -> &'static str {
            match self {
                Tool::Zenity => "zenity",
                Tool::Kdialog => "kdialog",
            }
        }
    }

    fn on_path(program: &str) -> bool {
        std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
    }

    fn text(title: &str, detail: Option<&str>) -> String {
        match detail.map(str::trim).filter(|detail| !detail.is_empty()) {
            Some(detail) => format!("{title}\n\n{detail}"),
            None => title.to_owned(),
        }
    }

    /// The command for a yes button and maybe a no button. Yes exits with success.
    /// `default_no` puts the keyboard on the no button.
    pub fn command(
        tool: Tool,
        level: PromptLevel,
        text: &str,
        yes: &str,
        no: Option<&str>,
        default_no: bool,
    ) -> Command {
        let mut command = Command::new(tool.program());
        match tool {
            Tool::Zenity => {
                let kind = match (no, level) {
                    (None, PromptLevel::Info) => "--info",
                    (None, PromptLevel::Warning) => "--warning",
                    (None, PromptLevel::Critical) => "--error",
                    (Some(_), _) => "--question",
                };
                // `=` keeps text that starts with a dash from being read as an option.
                command
                    .arg(kind)
                    .arg("--no-markup")
                    .arg(format!("--title={APP_NAME}"))
                    .arg(format!("--text={text}"))
                    .arg(format!("--ok-label={yes}"));
                if let Some(no) = no {
                    command.arg(format!("--cancel-label={no}"));
                    if default_no {
                        command.arg("--default-cancel");
                    }
                }
            }
            Tool::Kdialog => {
                command.arg("--title").arg(APP_NAME);
                match (no, level) {
                    (None, level) => {
                        let kind = match level {
                            PromptLevel::Info => "--msgbox",
                            PromptLevel::Warning => "--sorry",
                            PromptLevel::Critical => "--error",
                        };
                        command.arg(kind).arg(text).arg("--ok-label").arg(yes);
                    }
                    (Some(no), PromptLevel::Info) => {
                        command.arg("--yesno").arg(text).arg("--yes-label").arg(yes).arg("--no-label").arg(no);
                    }
                    (Some(no), _) => {
                        command
                            .arg("--warningcontinuecancel")
                            .arg(text)
                            .arg("--continue-label")
                            .arg(yes)
                            .arg("--cancel-label")
                            .arg(no);
                    }
                }
            }
        }
        command
    }
}

#[cfg(test)]
mod tests {
    use super::dialog_tool::{Tool, command};
    use gpui_kit::PromptLevel;

    fn args(tool: Tool, level: PromptLevel, yes: &str, no: Option<&str>, default_no: bool) -> Vec<String> {
        command(tool, level, "--rm everything", yes, no, default_no)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn zenity_keeps_text_out_of_options() {
        let args = args(Tool::Zenity, PromptLevel::Warning, "Close", Some("Cancel"), false);
        assert_eq!(args[0], "--question");
        assert!(args.contains(&"--no-markup".to_owned()));
        assert!(args.contains(&"--text=--rm everything".to_owned()));
        assert!(args.contains(&"--ok-label=Close".to_owned()));
        assert!(args.contains(&"--cancel-label=Cancel".to_owned()));
        assert!(!args.contains(&"--default-cancel".to_owned()));
    }

    #[test]
    fn risky_choices_start_on_cancel() {
        let args = args(Tool::Zenity, PromptLevel::Critical, "Trust New Key", Some("Cancel"), true);
        assert!(args.contains(&"--default-cancel".to_owned()));
    }

    #[test]
    fn one_button_is_a_plain_message() {
        assert_eq!(args(Tool::Zenity, PromptLevel::Critical, "OK", None, false)[0], "--error");
        let kdialog = args(Tool::Kdialog, PromptLevel::Warning, "OK", None, false);
        assert_eq!(kdialog[2..4], ["--sorry", "--rm everything"]);
    }

    #[test]
    fn kdialog_names_both_buttons() {
        let args = args(Tool::Kdialog, PromptLevel::Warning, "Delete", Some("Cancel"), false);
        assert_eq!(
            args[2..],
            ["--warningcontinuecancel", "--rm everything", "--continue-label", "Delete", "--cancel-label", "Cancel"]
        );
    }
}
