//! Emergency stop.
//!
//! On Windows this module also owns a global `RegisterHotKey(Ctrl+Alt+G)` and
//! a cross-process broadcast (a named kernel event, so every ghost process on
//! the machine stops together). Both of those need a real OS hook -- the
//! Windows message-loop registration has no macOS equivalent without a
//! CGEventTap (Accessibility permission, C FFI, explicitly out of scope).
//!
//! What genuinely has **no** OS dependency is the flag itself: `STOP_FLAG` is
//! a plain `AtomicBool`, and `is_stopped()`/`trigger_stop()`/`reset_stop()`
//! are pure reads/writes of it. `ghost-session/src/shell.rs` polls
//! `is_stopped()` between `ghost_shell` output lines and nowhere else touches
//! the engine -- that is the ENTIRE reason `ghost_shell` can work on macOS
//! today even with zero native automation. So this is a real, working
//! cooperative-cancellation flag, not a stub: call `trigger_stop()` (from
//! `ghost_stop` over MCP, or in-process) and every in-flight `ghost_shell`
//! read loop observes it on its next poll.
//!
//! What it does NOT do: broadcast to other ghost processes (no named
//! cross-process primitive without either a C dependency or unsafe
//! platform-specific IPC this crate does not add), and it is not bound to a
//! physical Ctrl+Alt+G keypress (no global hotkey capture without
//! Accessibility). Both are honestly reported as absent by
//! `crates/ghost-platform/src/macos.rs`'s empty `supported` list; nothing
//! here claims otherwise.

use std::sync::atomic::{AtomicBool, Ordering};

pub static STOP_FLAG: AtomicBool = AtomicBool::new(false);

pub fn is_stopped() -> bool {
    STOP_FLAG.load(Ordering::Acquire)
}

/// Trip the flag for this process. Every `is_stopped()` poll (in particular
/// `ghost-session/src/shell.rs`'s read loop) observes it immediately. Does
/// NOT reach other ghost processes -- see the module docs.
pub fn trigger_stop() {
    STOP_FLAG.store(true, Ordering::Release);
}

pub fn reset_stop() {
    STOP_FLAG.store(false, Ordering::Release);
}

/// Install the emergency-stop flag for this process.
///
/// Always succeeds. This deliberately does NOT try (and fail) to register a
/// system-wide Ctrl+Alt+G hotkey -- there is no such hook available without
/// Accessibility/CGEventTap. Returning `Err` here would be worse than
/// honest: `GhostSession::new()` calls this during construction and
/// propagates any error, so a failure would make the session
/// unconstructible on macOS entirely -- taking down `ghost_browser_*` and
/// `ghost_shell`, which have no dependency on this flag's OS binding at all.
/// The part of "emergency stop" that macOS can actually deliver today (the
/// in-process flag, reachable via `ghost_stop` over MCP) IS installed for
/// real by the mere existence of `STOP_FLAG`; the part it cannot deliver (a
/// physical hotkey) is reported absent by `ghost-platform`'s capability list,
/// not silently promised here.
pub fn register_emergency_stop() -> Result<(), crate::error::CoreError> {
    Ok(())
}

/// Release every modifier key, unconditionally.
///
/// On Windows/Linux this sends real key-up events because real key-down
/// events could have been sent (a stuck Ctrl/Shift/Alt from an interrupted
/// `ghost_key` would otherwise corrupt all later input). On macOS nothing in
/// this crate ever sends a synthetic key event -- every keyboard/mouse
/// primitive below returns `CoreError::Unsupported` before touching
/// anything -- so there is no modifier that could be physically stuck. A
/// no-op here is not a fake "I released your keys"; it is the correct
/// consequence of never having pressed any.
pub fn release_all_modifiers() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_flag_round_trips() {
        reset_stop();
        assert!(!is_stopped());
        trigger_stop();
        assert!(is_stopped());
        reset_stop();
        assert!(!is_stopped());
    }

    #[test]
    fn registration_always_succeeds_and_is_idempotent() {
        register_emergency_stop().expect("first");
        register_emergency_stop().expect("second call must also succeed");
    }
}
