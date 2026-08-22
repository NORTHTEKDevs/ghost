//! Clipboard access.
//!
//! `NSPasteboard` needs no special permission on macOS, but it does need the
//! Objective-C runtime -- `objc2`/`core-graphics`-family FFI, explicitly out
//! of scope for this pure-Rust crate (see the crate docs). So this refuses
//! honestly rather than silently reading/writing nothing.

use crate::error::CoreError;

pub fn get_clipboard() -> Result<String, CoreError> {
    Err(CoreError::Unsupported { op: "get_clipboard", needs: "NSPasteboard" })
}

pub fn set_clipboard(_text: &str) -> Result<(), CoreError> {
    Err(CoreError::Unsupported { op: "set_clipboard", needs: "NSPasteboard" })
}
