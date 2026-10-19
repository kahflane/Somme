//! Theme and font choices follow the account. They are kept in the synced
//! database, so every signed-in device shows the same look.

use std::sync::Arc;

use gpui_kit::{App, Global};
use serde::{Deserialize, Serialize};

use crate::runtime;
use crate::services::Services;
use crate::settings::{Appearance, Settings};

const KEY: &str = "look";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Look {
    appearance: Appearance,
    dark_theme: String,
    light_theme: String,
    terminal_matches_theme: bool,
    terminal_theme: String,
    font_family: String,
    font_everywhere: bool,
    font_size: f32,
    ui_font_size: f32,
    line_height: f32,
}

impl Look {
    fn of(s: &Settings) -> Self {
        Self {
            appearance: s.appearance,
            dark_theme: s.dark_theme.clone(),
            light_theme: s.light_theme.clone(),
            terminal_matches_theme: s.terminal_matches_theme,
            terminal_theme: s.terminal_theme.clone(),
            font_family: s.font_family.clone(),
            font_everywhere: s.font_everywhere,
            font_size: s.font_size,
            ui_font_size: s.ui_font_size,
            line_height: s.line_height,
        }
    }

    fn apply(self, s: &mut Settings) {
        s.appearance = self.appearance;
        s.dark_theme = self.dark_theme;
        s.light_theme = self.light_theme;
        s.terminal_matches_theme = self.terminal_matches_theme;
        s.terminal_theme = self.terminal_theme;
        s.font_family = self.font_family;
        s.font_everywhere = self.font_everywhere;
        s.font_size = self.font_size;
        s.ui_font_size = self.ui_font_size;
        s.line_height = self.line_height;
        // Another device may still have a theme this version dropped.
        s.replace_retired_themes();
    }
}

impl Default for Look {
    fn default() -> Self {
        Self::of(&Settings::default())
    }
}

struct Shared {
    services: Arc<Services>,
    /// The look last read from or written to the database, so a change that
    /// came from there is not written back.
    last: Option<Look>,
    /// Goes up on every local change, so an older read never undoes it.
    version: u64,
}

impl Global for Shared {}

pub fn init(services: Arc<Services>, cx: &mut App) {
    let last = Some(Look::of(Settings::get(cx)));
    cx.set_global(Shared {
        services,
        last,
        version: 0,
    });
    cx.observe_global::<Settings>(save).detach();
    pull(cx);
}

/// Writes the look to the database when it changed here.
fn save(cx: &mut App) {
    let look = Look::of(Settings::get(cx));
    let shared = cx.global_mut::<Shared>();
    if shared.last.as_ref() == Some(&look) {
        return;
    }
    let raw = match serde_json::to_string(&look) {
        Ok(raw) => raw,
        Err(err) => return tracing::warn!(%err, "could not encode the look"),
    };
    shared.last = Some(look);
    shared.version += 1;
    let services = shared.services.clone();
    runtime::handle().spawn(async move {
        if let Err(err) = services.preferences.set(KEY, &raw).await {
            tracing::warn!(%err, "could not save the look");
        }
    });
}

/// Reads the look from the database and uses it here. Runs at start and
/// whenever synced data arrives.
pub fn pull(cx: &mut App) {
    let shared = cx.global::<Shared>();
    let (services, version) = (shared.services.clone(), shared.version);
    cx.spawn(async move |cx| {
        let stored = runtime::io(async move { services.preferences.get(KEY).await }).await;
        let _ = cx.update(|cx| {
            if cx.global::<Shared>().version != version {
                return;
            }
            match stored.map(|raw| raw.map(|raw| serde_json::from_str::<Look>(&raw))) {
                Ok(Some(Ok(look))) => {
                    cx.global_mut::<Shared>().last = Some(look.clone());
                    if look != Look::of(Settings::get(cx)) {
                        Settings::update(cx, |s| look.apply(s));
                        crate::apply_appearance(cx);
                    }
                }
                // Nothing saved yet, so this device's look becomes the account's.
                Ok(None) => {
                    cx.global_mut::<Shared>().last = None;
                    save(cx);
                }
                Ok(Some(Err(err))) => tracing::warn!(%err, "could not read the saved look"),
                Err(err) => tracing::warn!(%err, "could not load the saved look"),
            }
        });
    })
    .detach();
}
