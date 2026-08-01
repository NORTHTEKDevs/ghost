//! Desktop-state primitives shared with the Linux engine.
//!
//! `ghost-session` needs three things from the OS that are not element
//! operations: which window has focus, where the pointer is, and a window's
//! screen rectangle. It uses them to prove that background dispatch did not
//! disturb the desktop.
//!
//! They are expressed here in platform-neutral types (`isize` handles, plain
//! tuples) so `ghost-linux` can offer the identical API and the shared session
//! layer compiles unchanged on both. A handle of `0` means "none", matching the
//! `HWND(0)` convention used throughout.

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowRect,
};

/// Handle of the foreground window, or 0 when there is none.
pub fn foreground_window() -> isize {
    unsafe {
        let h = GetForegroundWindow();
        if h.is_invalid() {
            0
        } else {
            h.0 as isize
        }
    }
}

/// Pointer position in screen coordinates.
pub fn cursor_pos() -> Option<(i32, i32)> {
    unsafe {
        let mut p = POINT::default();
        if GetCursorPos(&mut p).is_ok() {
            Some((p.x, p.y))
        } else {
            None
        }
    }
}

/// Screen rectangle of a window as (left, top, right, bottom).
pub fn window_rect(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    if hwnd == 0 {
        return None;
    }
    unsafe {
        let mut r = RECT::default();
        if GetWindowRect(HWND(hwnd as *mut _), &mut r).is_ok() {
            Some((r.left, r.top, r.right, r.bottom))
        } else {
            None
        }
    }
}

/// Rectangle of the foreground window.
pub fn foreground_window_rect() -> Option<(i32, i32, i32, i32)> {
    window_rect(foreground_window())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_handle_has_no_rectangle() {
        // 0 is the "no window" sentinel; asking the OS about it would be a
        // meaningless call at best.
        assert!(window_rect(0).is_none());
    }
}
