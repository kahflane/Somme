//! Windows: no access to ask for. Password boxes can't be told apart yet.

use std::time::Duration;

use device_query::{DeviceQuery, DeviceState, Keycode};
use enigo::Key;

use crate::{Input, Platform, Shot};

pub struct Windows;

impl Platform for Windows {
    fn ready(&self) -> Result<(), String> {
        Ok(())
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
        xcap::Window::all().ok()?.into_iter().find(|window| window.is_focused().unwrap_or(false)).and_then(|window| window.app_name().ok())
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
            return window.app_name().ok();
        }
        None
    }

    fn escape_down(&self) -> bool {
        DeviceState::new().get_keys().contains(&Keycode::Escape)
    }

    fn open_app(&self, name: &str) -> Result<(), String> {
        // The Start menu finds apps by name.
        self.send(&[
            Input::KeyDown(Key::Meta),
            Input::KeyUp(Key::Meta),
            Input::Pause(Duration::from_millis(700)),
            Input::Text(name.to_owned()),
            Input::Pause(Duration::from_millis(900)),
            Input::KeyDown(Key::Return),
            Input::KeyUp(Key::Return),
        ])
    }
}
