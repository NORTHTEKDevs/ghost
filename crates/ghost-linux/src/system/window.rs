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

/// The owning process id and title of a Ghost window handle.
///
/// The bridge between Ghost's AT-SPI handles and anything that speaks X11:
/// window-manager control and occluded capture both need a real X11 window, and
/// PID + title is what identifies it.
pub fn window_identity(handle: isize) -> Option<(u32, Option<String>)> {
    // Bounded for the same reason `foreground_info_fast` is: this sits on the
    // occluded-capture path, which is how an agent verifies a background
    // action, and the full 20s walk budget would let one slow application add
    // twenty seconds to every screenshot.
    const BUDGET: std::time::Duration = std::time::Duration::from_secs(3);
    let tree = A11yTree::new().ok()?;
    let w = tree.list_windows_within(BUDGET).ok()?.into_iter().find(|w| w.hwnd == handle)?;
    Some((w.pid, Some(w.name)))
}

/// Handle and title of the active window, on a short leash.
///
/// `ghost-mcp` attaches this to **every** tool response. On Windows the
/// equivalent is one `GetForegroundWindow` call; on Linux it is a full AT-SPI
/// registry walk, so using the normal 20s walk budget here would let a single
/// slow application add up to twenty seconds of latency to every response the
/// server sends -- while blocking its only runtime thread.
///
/// Metadata is worth a few hundred milliseconds and not one millisecond more:
/// on timeout this returns `None` and the caller reports "unknown" rather than
/// stalling the session.
pub fn foreground_info_fast() -> Option<(isize, String)> {
    const BUDGET: std::time::Duration = std::time::Duration::from_millis(400);
    let tree = A11yTree::new().ok()?;
    let windows = tree.list_windows_within(BUDGET).ok()?;
    windows.into_iter().find(|w| w.focused).map(|w| (w.hwnd, w.name))
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
