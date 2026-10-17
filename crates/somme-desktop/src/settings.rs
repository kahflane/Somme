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
    pub font_family: String,
    pub font_size: f32,
    pub line_height: f32,
    pub terminal_theme: String,
    pub cursor_shape: CursorShapeSetting,
    pub cursor_blink: bool,
    pub scrollback_lines: usize,
    pub copy_on_select: bool,
    pub paste_on_right_click: bool,
    pub option_as_meta: bool,
    pub bell: BellSetting,
    pub log_sessions: bool,
    pub confirm_close_tab: bool,
    pub keep_history: bool,
    pub auto_lock_minutes: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            appearance: Appearance::Dark,
            font_family: default_font().to_owned(),
            font_size: 14.,
            line_height: 1.25,
            terminal_theme: "Somme Dark".to_owned(),
            cursor_shape: CursorShapeSetting::Block,
            cursor_blink: true,
            scrollback_lines: 10_000,
            copy_on_select: false,
            paste_on_right_click: false,
            option_as_meta: true,
            bell: BellSetting::Visual,
            log_sessions: false,
            confirm_close_tab: true,
            keep_history: true,
            auto_lock_minutes: 0,
        }
    }
}

fn default_font() -> &'static str {
    if cfg!(target_os = "macos") {
        "Menlo"
    } else if cfg!(target_os = "windows") {
        "Consolas"
    } else {
        "DejaVu Sans Mono"
    }
}

impl Global for Settings {}

/// Where the settings file lives, kept next to the vault database.
pub struct SettingsFile(pub PathBuf);

impl Global for SettingsFile {}

impl Settings {
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
