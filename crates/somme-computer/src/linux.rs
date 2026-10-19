//! Linux on X11. Wayland keeps other apps' windows and input away from
//! Somme, so it isn't supported yet. Password boxes can't be told apart.

use std::fmt::Display;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder, MapState, Window};
use x11rb::rust_connection::RustConnection;

use crate::{Platform, Shot};

/// Esc's key symbol.
const XK_ESCAPE: u32 = 0xff1b;

pub struct X11 {
    conn: RustConnection,
    root: Window,
    /// Esc's key code on this keyboard.
    escape: Option<u8>,
}

static X11_SCREEN: OnceLock<X11> = OnceLock::new();

fn text<T, E: Display>(result: Result<T, E>) -> Result<T, String> {
    result.map_err(|err| format!("X11: {err}"))
}

impl X11 {
    pub fn connect() -> Result<&'static dyn Platform, String> {
        if let Some(x11) = X11_SCREEN.get() {
            return Ok(x11);
        }
        let wayland = std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind.eq_ignore_ascii_case("wayland"));
        if wayland || std::env::var_os("DISPLAY").is_none() {
            return Err("On Linux, Somme can only use the screen in an X11 session. Wayland isn't supported yet.".into());
        }
        let (conn, screen) = text(x11rb::connect(None))?;
        let root = conn.setup().roots.get(screen).ok_or("X11: no screen")?.root;
        let escape = escape_code(&conn).ok().flatten();
        Ok(X11_SCREEN.get_or_init(|| Self { conn, root, escape }))
    }
}

fn escape_code(conn: &RustConnection) -> Result<Option<u8>, String> {
    let setup = conn.setup();
    let (first, last) = (setup.min_keycode, setup.max_keycode);
    let mapping = text(text(conn.get_keyboard_mapping(first, last - first + 1))?.reply())?;
    let per = usize::from(mapping.keysyms_per_keycode.max(1));
    Ok(mapping.keysyms.chunks(per).position(|syms| syms.contains(&XK_ESCAPE)).map(|ix| first + ix as u8))
}

fn atom(conn: &RustConnection, name: &str) -> Result<u32, String> {
    Ok(text(text(conn.intern_atom(false, name.as_bytes()))?.reply())?.atom)
}

/// The windows' class, like `Firefox`.
fn class(conn: &RustConnection, window: Window) -> Option<String> {
    let reply = conn.get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 1024).ok()?.reply().ok()?;
    let parts: Vec<&[u8]> = reply.value.split(|byte| *byte == 0).filter(|part| !part.is_empty()).collect();
    parts.last().map(|class| String::from_utf8_lossy(class).into_owned())
}

fn pid(conn: &RustConnection, window: Window) -> Option<u32> {
    let wm_pid = atom(conn, "_NET_WM_PID").ok()?;
    conn.get_property(false, window, wm_pid, AtomEnum::CARDINAL, 0, 1).ok()?.reply().ok()?.value32()?.next()
}

impl Platform for X11 {
    fn ready(&self) -> Result<(), String> {
        Ok(())
    }

    fn capture(&self) -> Result<Shot, String> {
        let (conn, root) = (&self.conn, self.root);
        let size = text(text(conn.get_geometry(root))?.reply())?;
        let image = text(text(conn.get_image(ImageFormat::Z_PIXMAP, root, 0, 0, size.width, size.height, !0))?.reply())?;
        let setup = conn.setup();
        let bits = setup.pixmap_formats.iter().find(|format| format.depth == image.depth).map(|format| format.bits_per_pixel);
        if bits != Some(32) || setup.image_byte_order != ImageOrder::LSB_FIRST {
            return Err("This screen's pixel format isn't supported.".into());
        }
        // Blue, green, red, unused; to red, green, blue, opaque.
        let rgba: Vec<u8> = image.data.chunks_exact(4).flat_map(|pixel| [pixel[2], pixel[1], pixel[0], 255]).collect();
        let (width, height) = (u32::from(size.width), u32::from(size.height));
        let image = image::RgbaImage::from_raw(width, height, rgba).ok_or("The screenshot came out the wrong size.")?;
        Ok(Shot { image, x: 0, y: 0, width, height })
    }

    fn front_app(&self) -> Option<String> {
        let (conn, root) = (&self.conn, self.root);
        let active = atom(conn, "_NET_ACTIVE_WINDOW").ok()?;
        let window = conn.get_property(false, root, active, AtomEnum::WINDOW, 0, 1).ok()?.reply().ok()?.value32()?.next()?;
        class(conn, window)
    }

    fn app_at(&self, x: i32, y: i32) -> Option<String> {
        let (conn, root) = (&self.conn, self.root);
        let stacking = atom(conn, "_NET_CLIENT_LIST_STACKING").ok()?;
        let windows: Vec<Window> = conn.get_property(false, root, stacking, AtomEnum::WINDOW, 0, 4096).ok()?.reply().ok()?.value32()?.collect();
        // Bottom to top, so the last one at the point is in front.
        for window in windows.into_iter().rev() {
            let viewable = conn.get_window_attributes(window).ok().and_then(|cookie| cookie.reply().ok()).is_some_and(|attrs| attrs.map_state == MapState::VIEWABLE);
            if !viewable {
                continue;
            }
            let Some(size) = conn.get_geometry(window).ok().and_then(|cookie| cookie.reply().ok()) else { continue };
            let Some(at) = conn.translate_coordinates(window, root, 0, 0).ok().and_then(|cookie| cookie.reply().ok()) else { continue };
            let (left, top) = (i32::from(at.dst_x), i32::from(at.dst_y));
            if x < left || y < top || x >= left + i32::from(size.width) || y >= top + i32::from(size.height) {
                continue;
            }
            if pid(conn, window) == Some(std::process::id()) {
                return Some(crate::own_name());
            }
            return class(conn, window);
        }
        None
    }

    fn escape_down(&self) -> bool {
        let Some(code) = self.escape else { return false };
        self.conn.query_keymap().ok().and_then(|cookie| cookie.reply().ok()).is_some_and(|map| map.keys[usize::from(code / 8)] & (1 << (code % 8)) != 0)
    }

    fn open_app(&self, name: &str) -> Result<(), String> {
        let program = name.trim().to_lowercase().replace(' ', "-");
        Command::new(&program)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|err| format!("Could not open {name}: {err}"))
    }
}
