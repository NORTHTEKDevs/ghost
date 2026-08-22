//! Foreground window / cursor / window-rect queries.
//!
//! All four functions need a real window-server query (AXUIElement for
//! window rects, `CGEventGetLocation`/`NSEvent.mouseLocation` for the
//! cursor) -- out of scope. None of these four are allowed to return
//! `Result` though (`foreground_window` returns a bare `isize`, matching the
//! Windows/Linux signatures ghost-session calls unconditionally), so the
//! honest answer has to fit the existing type. It does: this codebase
//! already defines `0` as the window-handle "none" sentinel ("Window handles
//! are `isize` on both platforms (`0` means none)" -- `ghost-session/src/engine.rs`)
//! and `None` as the pointer-position "not knowable on this platform"
//! sentinel (`ghost-session/src/session.rs`'s own doc comment on
//! `cursor_unchanged`). Returning those sentinels here is not a fabricated
//! answer dressed as data -- it is the exact vocabulary this codebase already
//! uses for "I cannot report this," and every caller (e.g. `hwnd == 0 ->
//! WindowGone`) already handles it correctly.

/// Always `0` (no foreground window known) -- see the module docs.
pub fn foreground_window() -> isize {
    0
}

/// Always `None` (pointer position not knowable here) -- see the module docs.
pub fn cursor_pos() -> Option<(i32, i32)> {
    None
}

/// Always `None`. Also correctly returns `None` for the `hwnd == 0` case via
/// `foreground_window_rect`, matching the Windows engine's own short-circuit.
pub fn window_rect(_hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    None
}

pub fn foreground_window_rect() -> Option<(i32, i32, i32, i32)> {
    window_rect(foreground_window())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_window_is_the_none_sentinel() {
        assert_eq!(foreground_window(), 0);
    }

    #[test]
    fn cursor_pos_is_unknown_not_a_fabricated_origin() {
        assert_eq!(cursor_pos(), None);
    }
}
