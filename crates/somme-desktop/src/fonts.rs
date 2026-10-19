//! Somme carries its own fixed-width font, JetBrains Mono, so terminals and
//! code line up on any computer. The chosen font follows the account, and a
//! computer that doesn't have it shows this one instead of a system font,
//! which is often not fixed-width.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::OnceLock;

use gpui_kit::{App, SharedString};

pub const BUNDLED: &str = "JetBrains Mono";

const FILES: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-BoldItalic.ttf"),
];

/// Font families on this computer, in lowercase. Read once at start.
static INSTALLED: OnceLock<HashSet<String>> = OnceLock::new();

/// Loads the bundled font and notes which fonts this computer has.
pub fn init(cx: &App) {
    let text = cx.text_system();
    if let Err(err) = text.add_fonts(FILES.iter().map(|file| Cow::Borrowed(*file)).collect()) {
        tracing::warn!(%err, "could not load the bundled font");
    }
    let mut names: HashSet<String> = text.all_font_names().iter().map(|name| name.to_lowercase()).collect();
    names.insert(BUNDLED.to_lowercase());
    let _ = INSTALLED.set(names);
}

/// Whether this computer can show `family`. Before `init`, every font counts.
pub fn installed(family: &str) -> bool {
    INSTALLED.get().is_none_or(|names| names.contains(&family.to_lowercase()))
}

/// `family`, or the bundled font when this computer doesn't have it.
pub fn or_bundled(family: &str) -> SharedString {
    if installed(family) { SharedString::from(family.to_owned()) } else { SharedString::new_static(BUNDLED) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_font_is_fixed_width_and_named_as_used() {
        let mut db = fontdb::Database::new();
        for file in FILES {
            db.load_font_data(file.to_vec());
        }
        let faces: Vec<_> = db.faces().collect();
        assert_eq!(faces.len(), 4);
        for face in faces {
            assert!(face.monospaced, "{face:?}");
            assert_eq!(face.families.first().map(|(name, _)| name.as_str()), Some(BUNDLED));
        }
    }

    #[test]
    fn a_font_this_computer_lacks_gives_way_to_the_bundled_one() {
        let _ = INSTALLED.set(["menlo".to_owned(), BUNDLED.to_lowercase()].into());
        assert_eq!(or_bundled("Menlo"), "Menlo");
        assert_eq!(or_bundled("Missing Mono Font"), BUNDLED);
    }
}
