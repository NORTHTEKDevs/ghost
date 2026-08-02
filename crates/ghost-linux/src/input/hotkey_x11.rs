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

/// Whether the grab is currently held and its listener alive.
///
/// Idempotent, but NOT permanent. If the listener thread exits -- the X server
/// restarted, the display was reconfigured, a suspend/resume dropped the
/// connection -- the grab is gone and Ctrl+Alt+G is dead. Latching this forever
/// would make that silently unrecoverable, and on Linux this is the only
/// out-of-band kill switch. The listener clears it on the way out so the next
/// `register()` re-arms.
static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Is the emergency-stop hotkey currently armed?
///
/// `ghost doctor` must call THIS rather than `register()`. Re-registering to
/// test the state re-grabs the same combination on a second connection, which
/// the X server refuses with `BadAccess` precisely *because* the first grab is
/// working -- so a healthy hotkey would be reported as held by another
/// application.
pub fn is_registered() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

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
    // Idempotent, matching the Windows engine, which treats
    // ERROR_HOTKEY_ALREADY_REGISTERED as success. Without this every call grabs
    // the key again and spawns another listener thread holding another X11
    // connection -- and GhostSession::new() calls this, so a process that builds
    // more than one session leaks both.
    // Idempotent, but NOT permanent. If the listener thread exits -- the X
    // server restarted, the display was reconfigured, a suspend/resume dropped
    // the connection -- the grab is gone and Ctrl+Alt+G is dead. Latching a
    // "registered" flag forever would make that silently unrecoverable, and on
    // Linux this is the only out-of-band kill switch. The flag is cleared by the
    // listener on the way out so the next call re-arms.
    if ACTIVE.load(Ordering::SeqCst) {
        return Ok(());
    }

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
        (ModMask::from(0u16), "none"),
        (ModMask::LOCK, "caps lock"),
        (ModMask::M2, "num lock"),
        (ModMask::LOCK | ModMask::M2, "caps+num lock"),
    ];

    // The plain variant is the one that decides success. Accepting a partial
    // grab reports a working hotkey that silently does nothing whenever Caps
    // Lock or Num Lock happens to be on.
    let mut grabbed = false;
    let mut missing: Vec<&str> = Vec::new();
    for (extra, label) in lock_variants {
        let ok = conn
            .grab_key(true, root, base | extra, keycode, GrabMode::ASYNC, GrabMode::ASYNC)
            .map(|c| c.check().is_ok())
            .unwrap_or(false);
        if label == "none" {
            grabbed = ok;
        } else if !ok {
            missing.push(label);
        }
    }
    if grabbed && !missing.is_empty() {
        tracing::warn!(
            "emergency-stop hotkey Ctrl+Alt+G registered, but not for these lock states: {}. It \
             will not fire while they are on",
            missing.join(", ")
        );
    }
    conn.flush().map_err(|e| CoreError::platform(format!("x11 flush: {e}")))?;

    if !grabbed {
        return Err(CoreError::platform(
            "could not grab Ctrl+Alt+G; another application may already hold it. ghost_stop over \
             MCP still works",
        ));
    }

    ACTIVE.store(true, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("ghost-hotkey".into())
        .spawn(move || {
            // Any KeyPress delivered here is our grab: nothing else is grabbed.
            while let Ok(event) = conn.wait_for_event() {
                if let Event::KeyPress(_) = event {
                    super::hotkey::STOP_FLAG.store(true, Ordering::SeqCst);
                }
            }
            // The connection dropped. Clear the flag so a later call re-arms
            // instead of assuming a hotkey that no longer exists, and say so:
            // an emergency stop that dies silently is the one failure this
            // module exists to prevent.
            ACTIVE.store(false, Ordering::SeqCst);
            tracing::warn!(
                "the emergency-stop hotkey listener exited (the X11 connection dropped); \
                 Ctrl+Alt+G is no longer armed. ghost_stop over MCP still works"
            );
        })
        .map_err(|e| {
            ACTIVE.store(false, Ordering::SeqCst);
            CoreError::platform(format!("could not spawn hotkey thread: {e}"))
        })?;

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
