//! Sees and uses this computer's screen, mouse and keys, for Somme's agents.
//! Nothing here asks the system for access by itself: [`Platform::ready`]
//! only checks, and [`Platform::ask_for_access`] is for a button the user
//! presses.

mod keys;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "windows")]
mod windows;

use std::time::Duration;

pub use enigo::Key;
pub use image::{self, Rgba, RgbaImage};
pub use keys::{is_escape, parse as parse_keys};

/// The main screen, and where it is in the units input uses.
pub struct Shot {
    pub image: RgbaImage,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mouse {
    Left,
    Right,
    Middle,
}

/// One piece of input. Points are in the units of [`Shot`]'s `x` and `width`.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    MoveTo(i32, i32),
    Click(Mouse),
    Press(Mouse),
    Release(Mouse),
    Text(String),
    KeyDown(Key),
    KeyUp(Key),
    /// Lines to scroll: down and right are more than 0.
    Scroll { down: i32, right: i32 },
    Pause(Duration),
}

/// One system's screen, mouse and keys.
pub trait Platform: Send + Sync {
    /// Ok when Somme may see the screen and send input. Only checks; never
    /// asks the system.
    fn ready(&self) -> Result<(), String>;

    /// Asks the system for access with its own dialogs. Only when the user
    /// pressed a button for it.
    fn ask_for_access(&self) {}

    fn capture(&self) -> Result<Shot, String>;

    /// The app in front, by name.
    fn front_app(&self) -> Option<String>;

    /// The app whose window is at a point, or None when it can't tell.
    fn app_at(&self, x: i32, y: i32) -> Option<String>;

    /// True when it can tell password boxes apart.
    fn knows_password_boxes(&self) -> bool {
        false
    }

    /// True when typing would go into a password box.
    fn in_password_box(&self) -> bool {
        false
    }

    /// True while Esc is held down on the keyboard.
    fn escape_down(&self) -> bool;

    /// Opens an app, or brings it to the front.
    fn open_app(&self, name: &str) -> Result<(), String>;

    fn send(&self, input: &[Input]) -> Result<(), String> {
        send_with_enigo(input)
    }
}

/// This computer, or why its screen can't be used.
pub fn system() -> Result<&'static dyn Platform, String> {
    #[cfg(target_os = "macos")]
    return Ok(&mac::Mac);
    #[cfg(target_os = "windows")]
    return Ok(&windows::Windows);
    #[cfg(target_os = "linux")]
    return linux::X11::connect();
    #[allow(unreachable_code)]
    Err("Somme can't use the screen on this system yet.".into())
}

impl Shot {
    /// The screenshot as a PNG at most `width` wide, for an LLM, with its size.
    pub fn png(&self, width: u32) -> Result<(Vec<u8>, u32, u32), String> {
        let (from_width, from_height) = self.image.dimensions();
        let small;
        let image = if from_width > width {
            let height = (u64::from(from_height) * u64::from(width) / u64::from(from_width.max(1))).max(1) as u32;
            small = image::imageops::resize(&self.image, width, height, image::imageops::FilterType::Triangle);
            &small
        } else {
            &self.image
        };
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|err| format!("Could not make the screenshot a PNG: {err}"))?;
        Ok((png, image.width(), image.height()))
    }
}

/// The name Somme's own windows go by.
pub fn own_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_stem().map(|stem| stem.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "Somme".into())
}

fn send_with_enigo(input: &[Input]) -> Result<(), String> {
    use enigo::{Axis, Button, Coordinate, Direction, Enigo, Keyboard as _, Mouse as _, Settings};
    let settings = Settings { open_prompt_to_get_permissions: false, ..Settings::default() };
    let mut enigo = Enigo::new(&settings).map_err(|err| format!("Somme can't send input: {err}"))?;
    let button = |mouse: &Mouse| match mouse {
        Mouse::Left => Button::Left,
        Mouse::Right => Button::Right,
        Mouse::Middle => Button::Middle,
    };
    for step in input {
        let done = match step {
            Input::MoveTo(x, y) => enigo.move_mouse(*x, *y, Coordinate::Abs),
            Input::Click(mouse) => enigo.button(button(mouse), Direction::Click),
            Input::Press(mouse) => enigo.button(button(mouse), Direction::Press),
            Input::Release(mouse) => enigo.button(button(mouse), Direction::Release),
            Input::Text(text) => enigo.text(text),
            Input::KeyDown(key) => enigo.key(*key, Direction::Press),
            Input::KeyUp(key) => enigo.key(*key, Direction::Release),
            Input::Scroll { down, right } => {
                let mut done = Ok(());
                if *down != 0 {
                    done = enigo.scroll(*down, Axis::Vertical);
                }
                if *right != 0 && done.is_ok() {
                    done = enigo.scroll(*right, Axis::Horizontal);
                }
                done
            }
            Input::Pause(time) => {
                std::thread::sleep(*time);
                Ok(())
            }
        };
        done.map_err(|err| format!("Input failed: {err}"))?;
        // Apps miss input that comes too fast.
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshots_shrink_for_the_llm() {
        let shot = Shot { image: RgbaImage::new(2560, 1600), x: 0, y: 0, width: 1280, height: 800 };
        let (png, width, height) = shot.png(1280).unwrap();
        assert_eq!((width, height), (1280, 800));
        assert!(png.starts_with(b"\x89PNG"));
        let small = Shot { image: RgbaImage::new(640, 400), ..shot };
        assert_eq!(small.png(1280).unwrap().1, 640);
    }
}
