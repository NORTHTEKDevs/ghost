//! Clipboard access on X11 and Wayland.
//!
//! `arboard` handles both: X11 selections directly, and Wayland through
//! `wl-clipboard-rs` (the `wayland-data-control` feature).
//!
//! One Linux-specific hazard worth knowing: on X11 the clipboard is owned by a
//! *process*, and its contents vanish when that process exits. `arboard` keeps
//! the selection alive for the lifetime of the `Clipboard` value, so a fresh
//! instance is created per call for reads but a written value is flushed before
//! the handle drops.

use crate::error::{CoreError, Result};

pub fn get_clipboard() -> Result<String> {
    let mut cb = arboard::Clipboard::new()
        .map_err(|e| CoreError::platform(format!("clipboard open failed: {e}")))?;
    match cb.get_text() {
        Ok(s) => Ok(s),
        // An empty or non-text clipboard is not an error: it is empty.
        Err(arboard::Error::ContentNotAvailable) => Ok(String::new()),
        Err(e) => Err(CoreError::platform(format!("clipboard read failed: {e}"))),
    }
}

pub fn set_clipboard(text: &str) -> Result<()> {
    let mut cb = arboard::Clipboard::new()
        .map_err(|e| CoreError::platform(format!("clipboard open failed: {e}")))?;
    cb.set_text(text.to_string())
        .map_err(|e| CoreError::platform(format!("clipboard write failed: {e}")))
}
