//! Desktop-state primitives, mirroring `ghost_core::system::window`.
//!
//! Same signatures as the Windows engine so `ghost-session` compiles unchanged.
//! The semantics differ in one honest way: **pointer position is only knowable
//! on X11**. Neither the RemoteDesktop portal nor uinput will report where the
//! cursor is, so [`cursor_pos`] returns `None` there.
//!
//! That is not a gap in practice. `cursor_pos` exists so background dispatch can
//! prove it did not move the pointer, and on Linux the primary dispatch path is
//! an AT-SPI action, which has no pointer involvement at all. Callers treat
//! `None` as "unknown", never as "moved".

use crate::a11y::tree::A11yTree;

/// Handle of the active window, or 0 when there is none.
pub fn foreground_window() -> isize {
    match A11yTree::new() {
        Ok(tree) => tree.foreground_window(),
        Err(_) => 0,
    }
}

/// Pointer position in screen coordinates. `None` on Wayland and uinput, where
/// no API exposes it to a client.
pub fn cursor_pos() -> Option<(i32, i32)> {
    crate::input::backend().ok()?.cursor_pos()
}

/// Screen rectangle of a window as (left, top, right, bottom).
pub fn window_rect(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    if hwnd == 0 {
        return None;
    }
    A11yTree::new().ok()?.window_rect(hwnd)
}

/// Rectangle of the active window.
pub fn foreground_window_rect() -> Option<(i32, i32, i32, i32)> {
    window_rect(foreground_window())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_handle_has_no_rectangle() {
        assert!(window_rect(0).is_none());
    }
}
