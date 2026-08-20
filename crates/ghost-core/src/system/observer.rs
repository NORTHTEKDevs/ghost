//! Observable state that proves whether automation stayed out of the user's way.
//!
//! "It runs in the background" is a claim, and claims need a measurement. The three
//! things a user actually notices when automation misbehaves are: the window they
//! were working in lost focus, the mouse pointer jumped, and their typing went
//! somewhere else. All three are observable, so ghost measures them.
//!
//! Take a snapshot before an operation, compare after. Any difference means the
//! operation reached out and touched the user's session.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowTextW,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopSnapshot {
    /// The window the user is currently working in.
    pub foreground_hwnd: isize,
    pub foreground_title: String,
    /// The real mouse pointer position, in screen pixels.
    pub cursor: (i32, i32),
}

/// What changed between two snapshots. Empty means the user saw nothing happen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DesktopDelta {
    pub foreground_changed: bool,
    pub cursor_moved: bool,
    pub from_title: String,
    pub to_title: String,
    pub cursor_from: (i32, i32),
    pub cursor_to: (i32, i32),
}

impl DesktopDelta {
    /// True when nothing the user can perceive was disturbed.
    pub fn is_undisturbed(&self) -> bool {
        !self.foreground_changed && !self.cursor_moved
    }

    /// Human-readable summary for tool output and test failures.
    pub fn describe(&self) -> String {
        if self.is_undisturbed() {
            return "undisturbed: foreground window and cursor unchanged".to_string();
        }
        let mut parts = Vec::new();
        if self.foreground_changed {
            parts.push(format!(
                "foreground moved from '{}' to '{}'",
                self.from_title, self.to_title
            ));
        }
        if self.cursor_moved {
            parts.push(format!(
                "cursor jumped from {:?} to {:?}",
                self.cursor_from, self.cursor_to
            ));
        }
        parts.join("; ")
    }
}

pub fn foreground_window() -> HWND {
    unsafe { GetForegroundWindow() }
}

pub fn window_title(hwnd: HWND) -> String {
    if hwnd.0.is_null() {
        return String::new();
    }
    unsafe {
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut buf);
        if len <= 0 {
            String::new()
        } else {
            String::from_utf16_lossy(&buf[..len as usize])
        }
    }
}

pub fn cursor_pos() -> (i32, i32) {
    unsafe {
        let mut p = POINT::default();
        if GetCursorPos(&mut p).is_ok() {
            (p.x, p.y)
        } else {
            // A failed read must not look like "cursor at origin", which would make a
            // real move at (0,0) invisible and a non-move look like a jump.
            (i32::MIN, i32::MIN)
        }
    }
}

impl DesktopSnapshot {
    pub fn capture() -> Self {
        let hwnd = foreground_window();
        Self {
            foreground_hwnd: hwnd.0 as isize,
            foreground_title: window_title(hwnd),
            cursor: cursor_pos(),
        }
    }

    /// Compare this snapshot (taken before) against the state now.
    pub fn delta_now(&self) -> DesktopDelta {
        self.delta(&Self::capture())
    }

    pub fn delta(&self, after: &Self) -> DesktopDelta {
        DesktopDelta {
            foreground_changed: self.foreground_hwnd != after.foreground_hwnd,
            cursor_moved: self.cursor != after.cursor,
            from_title: self.foreground_title.clone(),
            to_title: after.foreground_title.clone(),
            cursor_from: self.cursor,
            cursor_to: after.cursor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_compared_to_itself_is_undisturbed() {
        let s = DesktopSnapshot::capture();
        let d = s.delta(&s);
        assert!(d.is_undisturbed());
        assert_eq!(d.describe(), "undisturbed: foreground window and cursor unchanged");
    }

    #[test]
    fn a_foreground_change_is_reported_with_both_titles() {
        let before = DesktopSnapshot {
            foreground_hwnd: 1,
            foreground_title: "User's editor".into(),
            cursor: (10, 10),
        };
        let after = DesktopSnapshot {
            foreground_hwnd: 2,
            foreground_title: "Automated app".into(),
            cursor: (10, 10),
        };
        let d = before.delta(&after);
        assert!(!d.is_undisturbed());
        assert!(d.foreground_changed && !d.cursor_moved);
        assert!(d.describe().contains("User's editor"), "{}", d.describe());
        assert!(d.describe().contains("Automated app"), "{}", d.describe());
    }

    #[test]
    fn a_cursor_jump_is_reported_even_when_focus_is_untouched() {
        let before = DesktopSnapshot {
            foreground_hwnd: 1,
            foreground_title: "same".into(),
            cursor: (10, 10),
        };
        let after = DesktopSnapshot { cursor: (900, 400), ..before.clone() };
        let d = before.delta(&after);
        assert!(!d.is_undisturbed());
        assert!(d.cursor_moved && !d.foreground_changed);
        assert!(d.describe().contains("(900, 400)"), "{}", d.describe());
    }

    #[test]
    fn window_title_of_a_null_handle_is_empty_not_a_panic() {
        assert_eq!(window_title(HWND(std::ptr::null_mut())), "");
    }
}
