//! X11 synthetic input via the XTEST extension.
//!
//! Authoritative on a real X11 session. Under Wayland this reaches **XWayland
//! clients only** and silently does nothing for native Wayland windows, which is
//! why [`crate::input::backend`] refuses to select it there.

use std::sync::Mutex;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt as XProtoExt;
use x11rb::protocol::xtest::ConnectionExt as XTestExt;
use x11rb::rust_connection::RustConnection;

use crate::error::{CoreError, Result};

// XTEST event type codes (from the core X protocol).
const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const MOTION_NOTIFY: u8 = 6;

pub struct X11Backend {
    conn: Mutex<RustConnection>,
    root: x11rb::protocol::xproto::Window,
    /// keysym -> keycode, built once from the server's keyboard mapping.
    keymap: Vec<(u32, u8)>,
}

impl X11Backend {
    pub fn new() -> Result<Self> {
        let (conn, screen_num) =
            x11rb::connect(None).map_err(|e| CoreError::platform(format!("X11 connect: {e}")))?;
        let root = conn.setup().roots[screen_num].root;
        let keymap = build_keymap(&conn)?;
        Ok(Self { conn: Mutex::new(conn), root, keymap })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, RustConnection> {
        match self.conn.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    fn fake(&self, kind: u8, detail: u8, x: i32, y: i32) -> Result<()> {
        let conn = self.conn();
        conn.xtest_fake_input(kind, detail, 0, self.root, x as i16, y as i16, 0)
            .map_err(|e| CoreError::platform(format!("xtest_fake_input: {e}")))?
            .check()
            .map_err(|e| CoreError::platform(format!("xtest_fake_input rejected: {e}")))?;
        conn.flush().map_err(|e| CoreError::platform(format!("x11 flush: {e}")))?;
        Ok(())
    }

    pub fn move_to(&self, x: i32, y: i32) -> Result<()> {
        self.fake(MOTION_NOTIFY, 0, x, y)
    }

    /// `button` is an X11 button number: 1 left, 2 middle, 3 right, 4/5 wheel.
    pub fn button(&self, button: u8, down: bool) -> Result<()> {
        self.fake(if down { BUTTON_PRESS } else { BUTTON_RELEASE }, button, 0, 0)
    }

    pub fn key(&self, keysym: u32, down: bool) -> Result<()> {
        let code = self.keycode_for(keysym).ok_or_else(|| {
            CoreError::Unsupported(format!(
                "keysym {keysym:#x} is not in the active X11 keymap; \
                 use AT-SPI text entry (ghost_act) or clipboard paste for this text"
            ))
        })?;
        self.fake(if down { KEY_PRESS } else { KEY_RELEASE }, code, 0, 0)
    }

    pub fn scroll(&self, clicks: i32) -> Result<()> {
        // X11 encodes wheel as buttons 4 (up) and 5 (down).
        let button = if clicks > 0 { 4 } else { 5 };
        for _ in 0..clicks.abs() {
            self.button(button, true)?;
            self.button(button, false)?;
        }
        Ok(())
    }

    pub fn cursor_pos(&self) -> Option<(i32, i32)> {
        let conn = self.conn();
        let reply = conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((reply.root_x as i32, reply.root_y as i32))
    }

    fn keycode_for(&self, keysym: u32) -> Option<u8> {
        self.keymap.iter().find(|(sym, _)| *sym == keysym).map(|(_, code)| *code)
    }
}

fn build_keymap(conn: &RustConnection) -> Result<Vec<(u32, u8)>> {
    let setup = conn.setup();
    let min = setup.min_keycode;
    let max = setup.max_keycode;
    let count = max - min + 1;

    let mapping = conn
        .get_keyboard_mapping(min, count)
        .map_err(|e| CoreError::platform(format!("get_keyboard_mapping: {e}")))?
        .reply()
        .map_err(|e| CoreError::platform(format!("get_keyboard_mapping reply: {e}")))?;

    let per = mapping.keysyms_per_keycode as usize;
    let mut out = Vec::new();
    for (i, chunk) in mapping.keysyms.chunks(per).enumerate() {
        let keycode = min + i as u8;
        for sym in chunk {
            if *sym != 0 && !out.iter().any(|(s, _)| *s == *sym) {
                out.push((*sym, keycode));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn xtest_event_codes_match_the_core_protocol() {
        // Getting these wrong produces silent no-ops, not errors.
        assert_eq!(super::KEY_PRESS, 2);
        assert_eq!(super::KEY_RELEASE, 3);
        assert_eq!(super::BUTTON_PRESS, 4);
        assert_eq!(super::BUTTON_RELEASE, 5);
        assert_eq!(super::MOTION_NOTIFY, 6);
    }
}
