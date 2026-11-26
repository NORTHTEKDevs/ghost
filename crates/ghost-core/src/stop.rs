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

/// `Err(CoreError::Stopped)` while the stop is active.
pub fn ensure_not_stopped() -> Result<(), CoreError> {
    if is_active() {
        Err(CoreError::Stopped)
    } else {
        Ok(())
    }
}
