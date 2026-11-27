//! The emergency-stop gate every state-changing path calls.
//!
//! `ghost_stop` (or Ctrl+Alt+G, in any Ghost process) raises the stop; until
//! `ghost_reset`, nothing that clicks, types, scrolls, launches, focuses or
//! otherwise changes state may run. Reads stay available, and so do the
//! things that are part of stopping safely: releasing held modifier keys,
//! handing the foreground back to the human, and ending processes.

use crate::error::CoreError;

/// Whether the stop is active in this process or any other Ghost process.
pub fn is_active() -> bool {
    crate::input::hotkey::is_stopped()
}

/// What a refusal says, accurate in each state: the shared stop event cannot
/// be read (fail closed: no stop was raised, a reset will not help), the human
/// raised the stop (only they resume it), or a stop is in force.
pub fn refusal_text() -> String {
    use crate::input::hotkey;
    if let Some(why) = hotkey::stop_event_error() {
        return format!("Ghost is stopped (fail closed) - {why}");
    }
    if hotkey::human_stop_active() {
        return HUMAN_STOPPED.to_string();
    }
    STOPPED.to_string()
}

/// A stop is in force (Ctrl+Alt+G or ghost_stop).
pub const STOPPED: &str =
    "emergency stop active (Ctrl+Alt+G or ghost_stop): Ghost is stopped and refuses actions until the stop is lifted";

/// The human raised the stop: an agent cannot lift it. Agent-facing, so it
/// does not say HOW the human resumes (an agent with a shell of its own would
/// use it); the README and `ghost reset --help` tell the human.
pub const HUMAN_STOPPED: &str = "the human stopped Ghost (Ctrl+Alt+G); only the human can resume it - wait for them";

/// `Err(CoreError::Stopped)` while the stop is active.
pub fn ensure_not_stopped() -> Result<(), CoreError> {
    if is_active() {
        Err(CoreError::Stopped)
    } else {
        Ok(())
    }
}
