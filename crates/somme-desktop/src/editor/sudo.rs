//! Opening and saving server files as root, for files the login user may not
//! touch, such as after `sudo su` in the terminal: the editor's file access
//! still logs in as that user.

use gpui_kit::{AsyncWindowContext, PromptButton, PromptLevel};
use somme_application::ports::PromptAnswer;

use crate::terminal::disk::{Disk, DiskError};
use crate::ui::{native, prompts};

/// Asks whether to try again with sudo.
pub async fn ask(title: &str, detail: &str, ok: &str, cx: &mut AsyncWindowContext) -> bool {
    let buttons = [PromptButton::ok(ok.to_owned()), PromptButton::cancel("Cancel")];
    let Ok(answer) = cx.update(|window, cx| native::ask(PromptLevel::Warning, title, Some(detail), &buttons, window, cx)) else {
        return false;
    };
    answer.await == Ok(0)
}

/// Runs `work` as root with sudo. Asks for the user's sudo password when sudo
/// wants one, and again when it was wrong.
pub async fn as_root<T: Send + 'static>(
    disk: &Disk,
    work: impl Fn(&Disk, Option<&str>) -> Result<T, DiskError> + Clone + Send + 'static,
    cx: &mut AsyncWindowContext,
) -> Result<T, DiskError> {
    let mut password: Option<String> = None;
    loop {
        let attempt = {
            let (work, password) = (work.clone(), password.clone());
            disk.clone().run(move |disk| work(disk, password.as_deref()))
        };
        let wrong = match attempt.await {
            Err(DiskError::Password { wrong }) => wrong,
            done => return done,
        };
        let detail = if wrong {
            "That password did not work. Try again."
        } else {
            "sudo on the server asks for your account's password."
        };
        let answer = cx
            .update(|window, cx| prompts::ask_secret("Password for sudo", detail, "Password", window, cx))
            .map_err(|_| DiskError::Cancelled)?;
        match answer.await {
            Ok(PromptAnswer::Answers(values)) => password = values.into_iter().next().filter(|value| !value.is_empty()),
            _ => return Err(DiskError::Cancelled),
        }
    }
}
