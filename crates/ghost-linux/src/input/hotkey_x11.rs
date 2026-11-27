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
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt, GrabMode, ModMask};
use x11rb::protocol::Event;

/// Decides whether a Ctrl+Alt+Shift+G the grab delivered was typed by a person.
/// Core X events cannot tell a real key from one injected through XTEST
/// (`xdotool key ctrl+alt+shift+g`), so the chord is paired, by server
/// timestamp, with the XInput2 raw event of the same key press, which names
/// its source device: a press from the XTEST device - or one no raw event
/// vouches for - does not resume.
/// Whether the grabbed chord is the resume (Shift held) or the stop. The Shift
/// bit on the core event is not enough - an XTEST-held Shift sets it too, which
/// would turn the human's second Ctrl+Alt+G into a resume - so Shift must also
/// be physically down, as tracked from non-XTEST raw key events.
fn is_resume_chord(shift_bit: bool, shift_physically_down: bool) -> bool {
    shift_bit && shift_physically_down
}

#[derive(Default)]
struct ResumeGate {
    /// The core resume press waiting for its raw event (server time).
    pending: Option<u32>,
    /// The last raw press of the G key: (server time, from XTEST).
    last_raw: Option<(u32, bool)>,
}

impl ResumeGate {
    /// A core Ctrl+Alt+Shift+G press. `Some(true)` = resume now.
    fn on_press(&mut self, time: u32) -> Option<bool> {
        match self.last_raw {
            Some((t, xtest)) if t == time => Some(!xtest),
            _ => {
                self.pending = Some(time);
                None
            }
        }
    }

    /// A raw press of the G key. `Some(true)` = the pending press may resume.
    fn on_raw(&mut self, time: u32, from_xtest: bool) -> Option<bool> {
        self.last_raw = Some((time, from_xtest));
        (self.pending == Some(time)).then(|| {
            self.pending = None;
            !from_xtest
        })
    }
}

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
    // Ctrl+Alt+Shift+G is the human's resume - only with XInput2, which can
    // tell a real key from an injected one (see ResumeGate). Without it the
    // chord is not grabbed at all: an injectable resume key would let any
    // program lift the human's stop.
    let xtest_devices: Option<Vec<u16>> = conn
        .xinput_xi_query_version(2, 0)
        .ok()
        .and_then(|c| c.reply().ok())
        .filter(|v| v.major_version >= 2)
        .and_then(|_| conn.xinput_xi_query_device(xinput::Device::ALL).ok())
        .and_then(|c| c.reply().ok())
        .map(|r| {
            r.infos
                .iter()
                .filter(|d| String::from_utf8_lossy(&d.name).contains("XTEST"))
                .map(|d| d.deviceid)
                .collect()
        });
    // Shift's keycodes, to track whether it is physically held.
    let shift_keys: Vec<u8> = [0xffe1u32, 0xffe2].iter().filter_map(|&ks| keycode_for(&conn, ks)).collect();
    let mut resume_grabbed = xtest_devices.is_some()
        && conn
            .xinput_xi_select_events(
                root,
                &[xinput::EventMask {
                    deviceid: xinput::Device::ALL.into(),
                    mask: vec![xinput::XIEventMask::RAW_KEY_PRESS | xinput::XIEventMask::RAW_KEY_RELEASE],
                }],
            )
            .map(|c| c.check().is_ok())
            .unwrap_or(false);
    if resume_grabbed {
        for (extra, _) in lock_variants {
            resume_grabbed &= conn
                .grab_key(true, root, base | ModMask::SHIFT | extra, keycode, GrabMode::ASYNC, GrabMode::ASYNC)
                .map(|c| c.check().is_ok())
                .unwrap_or(false);
        }
    }
    if !resume_grabbed {
        tracing::warn!("the resume key is unavailable here (needs XInput2 and a free Ctrl+Alt+Shift+G)");
    }
    let xtest_devices = xtest_devices.unwrap_or_default();
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
            let mut gate = ResumeGate::default();
            let resume = |ok: Option<bool>| {
                if ok == Some(true) && super::hotkey::human_stop_active() {
                    super::hotkey::reset_human_stop();
                } else if ok == Some(false) {
                    tracing::warn!("ignored an injected resume key");
                }
            };
            let mut shift_down = false;
            while let Ok(event) = conn.wait_for_event() {
                match event {
                    Event::KeyPress(press) => {
                        let shift_bit = u16::from(press.state) & u16::from(ModMask::SHIFT) != 0;
                        if is_resume_chord(shift_bit, shift_down) {
                            resume(gate.on_press(press.time));
                        } else {
                            super::hotkey::trigger_human_stop();
                        }
                    }
                    Event::XinputRawKeyPress(raw) => {
                        let real = !xtest_devices.contains(&raw.sourceid);
                        if real && shift_keys.iter().any(|&k| u32::from(k) == raw.detail) {
                            shift_down = true;
                        }
                        if raw.detail == u32::from(keycode) {
                            resume(gate.on_raw(raw.time, !real));
                        }
                    }
                    Event::XinputRawKeyRelease(raw)
                        if !xtest_devices.contains(&raw.sourceid)
                            && shift_keys.iter().any(|&k| u32::from(k) == raw.detail) =>
                    {
                        shift_down = false;
                    }
                    _ => {}
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
    /// An injected (XTEST) Shift does not turn the human's Ctrl+Alt+G into a
    /// resume: Shift must be physically down as well.
    #[test]
    fn a_resume_needs_a_physically_held_shift() {
        assert!(super::is_resume_chord(true, true));
        assert!(!super::is_resume_chord(true, false), "an XTEST-held Shift made it a resume");
        assert!(!super::is_resume_chord(false, true));
    }

    /// The resume chord resumes only when its raw event shows a real device,
    /// whichever of the two events arrives first; an XTEST press never does.
    #[test]
    fn only_a_real_key_press_resumes() {
        let mut g = super::ResumeGate::default();
        assert_eq!(g.on_press(100), None, "waits for the raw event");
        assert_eq!(g.on_raw(100, false), Some(true), "real keyboard");

        let mut g = super::ResumeGate::default();
        assert_eq!(g.on_raw(200, true), None);
        assert_eq!(g.on_press(200), Some(false), "XTEST (xdotool) is refused");

        let mut g = super::ResumeGate::default();
        assert_eq!(g.on_raw(300, false), None);
        assert_eq!(g.on_press(301), None, "a raw event of another press does not vouch");
    }

    #[test]
    fn g_keysym_is_ascii_lowercase_g() {
        assert_eq!(super::KEYSYM_G, 'g' as u32);
    }
}
