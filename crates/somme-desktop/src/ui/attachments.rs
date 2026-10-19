//! Files picked with "Attach as context" in the file tree, kept until an AI
//! chat takes them.

use std::path::PathBuf;

use gpui_kit::{App, Global};

use crate::terminal::disk::Disk;
use crate::ui::common::notify_ok;

pub struct Attached {
    pub path: PathBuf,
    /// Where the file is: this computer or a server.
    pub disk: Disk,
}

#[derive(Default)]
pub struct Attachments(pub Vec<Attached>);

impl Global for Attachments {}

pub fn attach(path: PathBuf, disk: Disk, cx: &mut App) {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned());
    let attachments = cx.default_global::<Attachments>();
    if !attachments.0.iter().any(|attached| attached.path == path && attached.disk.same(&disk)) {
        attachments.0.push(Attached { path, disk });
    }
    notify_ok(cx, format!("Attached {name}. It goes with your next AI chat."));
}

/// Everything attached, emptying the list.
pub fn take(cx: &mut App) -> Vec<Attached> {
    std::mem::take(&mut cx.default_global::<Attachments>().0)
}
