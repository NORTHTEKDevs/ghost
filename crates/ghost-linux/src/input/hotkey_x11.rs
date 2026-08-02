//! Global emergency-stop hotkey on X11.
//!
//! The Windows engine registers Ctrl+Alt+G with `RegisterHotKey` so a user can
//! always abort a runaway agent, even when Ghost is not the focused application.
//! Linux had no equivalent, which made `ghost_stop` over MCP the only way
//! out -- useless precisely when it matters most, because a wedged or spinning
//! agent is exactly when the MCP channel is least responsive.
//!
//! X11 does support this: `GrabKey` on the root window delivers the combination
//! to us no matter which client has focus. This registers Ctrl+Alt+G and sets
//! the same stop flag `ghost_stop` sets.
//!
//! Native Wayland deliberately has no such mechanism -- a client cannot grab
//! keys globally, which is a security property, not an oversight. There the
//! GlobalShortcuts portal would be the route, and `ghost_stop` remains the
//! supported path; this module reports that rather than pretending.

use std::sync::atomic::Ordering;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, GrabMode, ModMask};
use x11rb::protocol::Event;

use crate::error::{CoreError, Result};
use crate::session::{session_kind, SessionKind};

/// X11 keysym for `g`.
const KEYSYM_G: u32 = 0x67;

/// Register the global Ctrl+Alt+G emergency stop.
///
/// Spawns a listener thread that lives for the process. Returns an error on
/// Wayland or when the grab is refused (another client already holds the
/// combination), rather than reporting success for a hotkey that will never
/// fire.
pub fn register() -> Result<()> {
    if session_kind() != SessionKind::X11 {
        return Err(CoreError::Unsupported(
            "a global hotkey needs an X11 session; Wayland does not allow clients to grab keys \
             globally. Use ghost_stop over MCP"
                .into(),
        ));
    }

    let (conn, screen_num) =
        x11rb::connect(None).map_err(|e| CoreError::platform(format!("X11 connect: {e}")))?;
    let root = conn.setup().roots[screen_num].root;

    let keycode = keycode_for(&conn, KEYSYM_G)
        .ok_or_else(|| CoreError::platform("no keycode for 'g' in the active X11 keymap"))?;

    // Grab with every combination of the "don't care" lock modifiers. Without
    // this the hotkey silently stops working the moment Caps Lock or Num Lock is
    // on -- a classic X11 trap, and one a panicking user is very likely to hit.
    let base = ModMask::CONTROL | ModMask::M1;
    let lock_variants = [
        ModMask::from(0u16),
        ModMask::LOCK,          // Caps Lock
        ModMask::M2,            // Num Lock
        ModMask::LOCK | ModMask::M2,
    ];

    let mut grabbed = false;
    for extra in lock_variants {
        let ok = conn
            .grab_key(true, root, base | extra, keycode, GrabMode::ASYNC, GrabMode::ASYNC)
            .map(|c| c.check().is_ok())
            .unwrap_or(false);
        grabbed |= ok;
    }
    conn.flush().map_err(|e| CoreError::platform(format!("x11 flush: {e}")))?;

    if !grabbed {
        return Err(CoreError::platform(
            "could not grab Ctrl+Alt+G; another application may already hold it. ghost_stop over \
             MCP still works",
        ));
    }

    std::thread::Builder::new()
        .name("ghost-hotkey".into())
        .spawn(move || {
            // Any KeyPress delivered here is our grab: nothing else is grabbed.
            while let Ok(event) = conn.wait_for_event() {
                if let Event::KeyPress(_) = event {
                    super::hotkey::STOP_FLAG.store(true, Ordering::SeqCst);
                }
            }
        })
        .map_err(|e| CoreError::platform(format!("could not spawn hotkey thread: {e}")))?;

    Ok(())
}

/// First keycode producing `keysym` in the active keymap.
fn keycode_for(conn: &impl Connection, keysym: u32) -> Option<u8> {
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let mapping = conn.get_keyboard_mapping(min, max - min + 1).ok()?.reply().ok()?;
    let per = mapping.keysyms_per_keycode as usize;

    mapping
        .keysyms
        .chunks(per)
        .enumerate()
        .find(|(_, syms)| syms.contains(&keysym))
        .map(|(i, _)| min + i as u8)
}

#[cfg(test)]
mod tests {
    #[test]
    fn g_keysym_is_ascii_lowercase_g() {
        assert_eq!(super::KEYSYM_G, 'g' as u32);
    }
}
