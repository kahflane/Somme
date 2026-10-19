//! macOS: Screen Recording access to see, Accessibility access to click
//! and type. Somme only checks for them; System Settings grants them.

use std::process::Command;

use accessibility::{AXAttribute, AXUIElement, AXUIElementAttributes};
use core_foundation::string::CFString;
use objc2_app_kit::NSWorkspace;
use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};
use objc2_foundation::NSString;

use crate::{Platform, Shot};

/// Esc's key code.
const ESCAPE: u16 = 53;
/// Owners of windows that cover the screen or the menu bar, not an app's own.
const SYSTEM_WINDOWS: &[&str] = &["Window Server", "Dock", "WindowManager"];

pub struct Mac;

impl Platform for Mac {
    fn ready(&self) -> Result<(), String> {
        if !CGPreflightScreenCaptureAccess() {
            return Err("Somme needs Screen Recording access to see the screen. Allow it in System Settings \u{2192} Privacy & Security \u{2192} Screen Recording, then run it again.".into());
        }
        let settings = enigo::Settings { open_prompt_to_get_permissions: false, ..enigo::Settings::default() };
        if enigo::Enigo::new(&settings).is_err() {
            return Err("Somme needs Accessibility access to click and type. Allow it in System Settings \u{2192} Privacy & Security \u{2192} Accessibility, then run it again.".into());
        }
        Ok(())
    }

    fn ask_for_access(&self) {
        CGRequestScreenCaptureAccess();
        let _ = enigo::Enigo::new(&enigo::Settings::default());
    }

    fn capture(&self) -> Result<Shot, String> {
        let failed = |err: xcap::XCapError| format!("Could not take a screenshot: {err}");
        let monitors = xcap::Monitor::all().map_err(failed)?;
        let monitor = monitors
            .iter()
            .find(|monitor| monitor.is_primary().unwrap_or(false))
            .or(monitors.first())
            .ok_or("There is no screen to look at.")?;
        Ok(Shot {
            image: monitor.capture_image().map_err(failed)?,
            x: monitor.x().map_err(failed)?,
            y: monitor.y().map_err(failed)?,
            width: monitor.width().map_err(failed)?,
            height: monitor.height().map_err(failed)?,
        })
    }

    fn front_app(&self) -> Option<String> {
        // activeApplication is up to date off the main thread; frontmostApplication isn't.
        #[allow(deprecated)]
        let active = NSWorkspace::sharedWorkspace().activeApplication()?;
        let name = active.valueForKey(&NSString::from_str("NSApplicationName"))?;
        name.downcast::<NSString>().ok().map(|name| name.to_string())
    }

    fn app_at(&self, x: i32, y: i32) -> Option<String> {
        let own = std::process::id();
        for window in xcap::Window::all().ok()? {
            if window.is_minimized().unwrap_or(true) {
                continue;
            }
            let (Ok(left), Ok(top), Ok(width), Ok(height)) = (window.x(), window.y(), window.width(), window.height()) else { continue };
            if x < left || y < top || x >= left + width as i32 || y >= top + height as i32 {
                continue;
            }
            if window.pid().ok() == Some(own) {
                return Some(crate::own_name());
            }
            let Ok(app) = window.app_name() else { continue };
            if !SYSTEM_WINDOWS.contains(&app.as_str()) {
                return Some(app);
            }
        }
        None
    }

    fn knows_password_boxes(&self) -> bool {
        true
    }

    fn in_password_box(&self) -> bool {
        let focused = AXUIElement::system_wide().attribute(&AXAttribute::new(&CFString::from_static_string("AXFocusedUIElement")));
        let Some(element) = focused.ok().and_then(|focused| focused.downcast::<AXUIElement>()) else { return false };
        let secure = |text: Result<CFString, accessibility::Error>| text.is_ok_and(|text| text == "AXSecureTextField");
        secure(element.subrole()) || secure(element.role())
    }

    fn escape_down(&self) -> bool {
        // The keyboard itself, not input Somme sent.
        CGEventSource::key_state(CGEventSourceStateID::HIDSystemState, ESCAPE)
    }

    fn open_app(&self, name: &str) -> Result<(), String> {
        let status = Command::new("/usr/bin/open").arg("-a").arg(name).status().map_err(|err| format!("Could not open {name}: {err}"))?;
        if status.success() { Ok(()) } else { Err(format!("Could not open {name}. Is it installed?")) }
    }
}
