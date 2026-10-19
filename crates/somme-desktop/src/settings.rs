use std::path::{Path, PathBuf};

use gpui_kit::{App, Global};
use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "desktop-settings.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum CursorShapeSetting {
    #[default]
    Block,
    Bar,
    Underline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Appearance {
    #[default]
    Dark,
    Light,
    System,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum BellSetting {
    #[default]
    Visual,
    Sound,
    Off,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub appearance: Appearance,
    /// App theme used in dark mode, by name.
    pub dark_theme: String,
    /// App theme used in light mode, by name.
    pub light_theme: String,
    /// Shows the snippets and themes panel next to terminals.
    pub side_panel: bool,
    /// Terminals use the app theme's own colors. When off, `terminal_theme` is used.
    pub terminal_matches_theme: bool,
    pub font_family: String,
    /// The terminal font is used for the whole app, not only terminals.
    pub font_everywhere: bool,
    /// Terminal text size.
    pub font_size: f32,
    /// Base text size for the rest of the app. Menus, lists and forms scale with it.
    pub ui_font_size: f32,
    pub line_height: f32,
    pub terminal_theme: String,
    pub cursor_shape: CursorShapeSetting,
    pub cursor_blink: bool,
    pub scrollback_lines: usize,
    /// Local terminals show each command as a block, with a text editor for typing commands.
    pub command_blocks: bool,
    pub copy_on_select: bool,
    pub paste_on_right_click: bool,
    /// Lets a server set the clipboard (OSC 52), as tmux and vim do over SSH.
    pub remote_clipboard_write: bool,
    pub option_as_meta: bool,
    pub bell: BellSetting,
    pub log_sessions: bool,
    pub confirm_close_tab: bool,
    pub keep_history: bool,
    pub auto_lock_minutes: u32,
    /// Folders where the file editor may start language servers. Servers can run
    /// code from the project, like build scripts, so each folder is trusted by hand.
    pub trusted_folders: Vec<String>,
    /// Opens the editor files left open last time, scratch files too.
    pub reopen_files: bool,
    /// Shows who last changed the line the cursor is on, and when.
    pub line_blame: bool,
    /// Lets agents connect over MCP and run scripts on saved hosts.
    pub agent_access: bool,
    /// Keeps a background service taking agents while the app is closed.
    pub agent_service: bool,
    /// Asks GitHub for a new version of Somme every few hours.
    pub check_updates: bool,
    /// The version put off with Later. Only a newer one shows again.
    pub update_later: Option<String>,
    /// The Whisper model voice typing uses.
    pub voice_model: crate::voice::VoiceModel,
    /// The language spoken, by Whisper's code, or "auto".
    pub voice_language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            appearance: Appearance::Dark,
            dark_theme: "Somme Dark".to_owned(),
            light_theme: "Somme Light".to_owned(),
            side_panel: false,
            terminal_matches_theme: true,
            font_family: crate::fonts::BUNDLED.to_owned(),
            font_everywhere: false,
            font_size: 12.,
            ui_font_size: 14.,
            line_height: 1.25,
            terminal_theme: "Somme Dark".to_owned(),
            cursor_shape: CursorShapeSetting::Block,
            cursor_blink: true,
            scrollback_lines: 10_000,
            command_blocks: true,
            copy_on_select: false,
            paste_on_right_click: false,
            remote_clipboard_write: false,
            option_as_meta: true,
            bell: BellSetting::Visual,
            log_sessions: false,
            confirm_close_tab: true,
            keep_history: true,
            auto_lock_minutes: 0,
            trusted_folders: Vec::new(),
            reopen_files: true,
            line_blame: true,
            agent_access: false,
            agent_service: false,
            check_updates: true,
            update_later: None,
            voice_model: crate::voice::VoiceModel::default(),
            voice_language: "auto".to_owned(),
        }
    }
}

impl Global for Settings {}

/// Where the settings file lives, kept next to the vault database.
pub struct SettingsFile(pub PathBuf);

impl Global for SettingsFile {}

impl Settings {
    /// The font terminals and code are drawn in: the chosen one, or the one
    /// Somme carries when this computer doesn't have it.
    pub fn mono_font(&self) -> gpui_kit::SharedString {
        crate::fonts::or_bundled(&self.font_family)
    }

    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join(FILE_NAME))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Changes settings, saves them, and redraws every window.
    pub fn update(cx: &mut App, change: impl FnOnce(&mut Self)) {
        change(cx.global_mut::<Self>());
        let settings = cx.global::<Self>().clone();
        if let Some(file) = cx.try_global::<SettingsFile>() {
            let path = file.0.join(FILE_NAME);
            match serde_json::to_string_pretty(&settings) {
                Ok(raw) => {
                    if let Err(err) = std::fs::write(&path, raw) {
                        tracing::warn!(%err, "could not save settings");
                    }
                }
                Err(err) => tracing::warn!(%err, "could not encode settings"),
            }
        }
        cx.refresh_windows();
    }
}
