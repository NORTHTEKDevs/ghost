//! Background (non-activating) dispatch.
//!
//! On Windows this posts real `WM_LBUTTONDOWN`/`WM_SETTEXT`/`WM_CHAR`/etc
//! window messages -- a Win32-specific mechanism with no macOS analogue at
//! all (AXUIElement actions are the closest equivalent, and those are the
//! out-of-scope native backend). Every dispatch method here honestly refuses.
//!
//! `EditCommand` and `focused_control` are the two exceptions, and neither
//! needs an OS call: `EditCommand` is plain data (which clipboard/edit
//! command a Ctrl+<key> maps to), and `focused_control` already has a
//! defined "I don't know" fallback in the original implementation (return the
//! window handle itself when no focus info is available) -- taking that
//! fallback unconditionally here, since macOS has no way to query GUI thread
//! focus, is the same honest degradation the Windows engine uses when its own
//! query fails, not a new invented behavior.

use crate::error::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditCommand {
    Copy,
    Cut,
    Paste,
    Undo,
    SelectAll,
}

impl EditCommand {
    /// Map a Ctrl+<key> shortcut to its edit command, if supported.
    pub fn from_ctrl_key(key: &str) -> Option<Self> {
        match key.to_lowercase().as_str() {
            "c" => Some(Self::Copy),
            "x" => Some(Self::Cut),
            "v" => Some(Self::Paste),
            "z" => Some(Self::Undo),
            "a" => Some(Self::SelectAll),
            _ => None,
        }
    }
}

pub struct BackgroundClicker;

impl BackgroundClicker {
    pub fn click(_hwnd_raw: isize, _client_xy: (i32, i32)) -> Result<(), CoreError> {
        Err(CoreError::Unsupported { op: "background click", needs: "AXUIElementPerformAction" })
    }

    pub fn click_screen(_hwnd_raw: isize, _screen_x: i32, _screen_y: i32) -> Result<(), CoreError> {
        Err(CoreError::Unsupported { op: "background click_screen", needs: "AXUIElementPerformAction" })
    }

    pub fn double_click_screen(_hwnd_raw: isize, _screen_x: i32, _screen_y: i32) -> Result<(), CoreError> {
        Err(CoreError::Unsupported { op: "background double_click_screen", needs: "AXUIElementPerformAction" })
    }

    pub fn right_click_screen(_hwnd_raw: isize, _screen_x: i32, _screen_y: i32) -> Result<(), CoreError> {
        Err(CoreError::Unsupported { op: "background right_click_screen", needs: "AXUIElementPerformAction" })
    }

    pub fn hover_screen(_hwnd_raw: isize, _screen_x: i32, _screen_y: i32) -> Result<(), CoreError> {
        Err(CoreError::Unsupported { op: "background hover_screen", needs: "CGEventCreateMouseEvent" })
    }

    pub fn button_click(_hwnd_raw: isize) -> Result<(), CoreError> {
        Err(CoreError::Unsupported { op: "background button_click", needs: "AXUIElementPerformAction" })
    }

    /// Where posted keystrokes would go for `window_hwnd_raw`. The Windows
    /// implementation queries the GUI thread's focus and falls back to the
    /// window handle itself when that query fails. macOS has no such query
    /// available here, so this always takes that same documented fallback --
    /// it is the existing "unknown, use the window itself" answer, not a new
    /// invented one.
    pub fn focused_control(window_hwnd_raw: isize) -> isize {
        window_hwnd_raw
    }

    pub fn send_key(_hwnd_raw: isize, _vk: u16) -> Result<(), CoreError> {
        Err(CoreError::Unsupported { op: "background send_key", needs: "CGEventCreateKeyboardEvent" })
    }

    pub fn send_char(_hwnd_raw: isize, _ch: char) -> Result<(), CoreError> {
        Err(CoreError::Unsupported { op: "background send_char", needs: "CGEventCreateKeyboardEvent" })
    }

    pub fn set_text(_hwnd_raw: isize, _text: &str) -> Result<(), CoreError> {
        Err(CoreError::Unsupported { op: "background set_text", needs: "AXUIElementSetAttributeValue" })
    }

    pub fn edit_command(_hwnd_raw: isize, _cmd: EditCommand) -> Result<(), CoreError> {
        Err(CoreError::Unsupported { op: "background edit_command", needs: "AXUIElementPerformAction" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_command_maps_ctrl_shortcuts() {
        assert_eq!(EditCommand::from_ctrl_key("c"), Some(EditCommand::Copy));
        assert_eq!(EditCommand::from_ctrl_key("A"), Some(EditCommand::SelectAll));
        assert_eq!(EditCommand::from_ctrl_key("s"), None);
    }

    #[test]
    fn focused_control_falls_back_to_the_window_itself() {
        assert_eq!(BackgroundClicker::focused_control(42), 42);
    }

    #[test]
    fn dispatch_methods_fail_honestly() {
        assert!(matches!(BackgroundClicker::click(0, (0, 0)), Err(CoreError::Unsupported { .. })));
        assert!(matches!(BackgroundClicker::button_click(0), Err(CoreError::Unsupported { .. })));
    }
}
