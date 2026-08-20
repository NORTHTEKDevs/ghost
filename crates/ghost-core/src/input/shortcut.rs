//! Editing shortcuts as standard control messages.
//!
//! A background Ctrl+Z cannot be delivered as key messages: `PostMessage(WM_KEYDOWN,
//! VK_CONTROL)` does not change the target thread's key state, so the app's own
//! translation loop sees an unmodified keystroke and inserts a literal "z". That
//! failure is silent and destructive - every call reports success while the document
//! fills with junk.
//!
//! But Windows never required key simulation for these operations in the first place.
//! Edit, RichEdit, and ComboBox controls implement undo, cut, copy, paste, clear, and
//! select-all as *messages*. Sending `WM_UNDO` performs a real undo, in the
//! background, with no keyboard involved at all - which is both correct and faster
//! than faking a keystroke would have been.
//!
//! `SendMessageTimeout` rather than `PostMessage`: these operations have a result
//! worth reading (`WM_UNDO` returns whether anything was undone), and a caller that
//! cannot tell whether the undo happened is back to guessing.

use crate::error::CoreError;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindow, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_CLEAR, WM_COPY, WM_CUT, WM_PASTE, WM_UNDO,
};

/// `EM_SETSEL`: select a character range in an edit control.
const EM_SETSEL: u32 = 0x00B1;
/// `EM_CANUNDO`: whether the control has anything to undo.
const EM_CANUNDO: u32 = 0x00C6;

const DEFAULT_TIMEOUT_MS: u32 = 5_000;

/// A shortcut that has a real message equivalent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    Undo,
    Cut,
    Copy,
    Paste,
    Clear,
    SelectAll,
}

impl Shortcut {
    /// Resolve a shortcut from either a friendly name ("undo") or a key combination
    /// ("Ctrl+Z"). Accepting the combination matters: callers reach for a shortcut by
    /// the keystroke they know, and silently failing to recognise "Ctrl+Z" would push
    /// them back onto the key-message path this module exists to replace.
    pub fn parse(s: &str) -> Option<Self> {
        let norm = crate::uia::tree::normalize_accelerator(s);
        let key = if norm.is_empty() { s.trim().to_lowercase() } else { norm };
        match key.as_str() {
            "undo" | "ctrl+z" => Some(Shortcut::Undo),
            "cut" | "ctrl+x" => Some(Shortcut::Cut),
            "copy" | "ctrl+c" => Some(Shortcut::Copy),
            "paste" | "ctrl+v" => Some(Shortcut::Paste),
            "clear" | "delete" | "del" => Some(Shortcut::Clear),
            "selectall" | "select_all" | "select-all" | "ctrl+a" => Some(Shortcut::SelectAll),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Shortcut::Undo => "undo",
            Shortcut::Cut => "cut",
            Shortcut::Copy => "copy",
            Shortcut::Paste => "paste",
            Shortcut::Clear => "clear",
            Shortcut::SelectAll => "select_all",
        }
    }

    /// The window message and parameters implementing this shortcut.
    fn message(&self) -> (u32, WPARAM, LPARAM) {
        match self {
            Shortcut::Undo => (WM_UNDO, WPARAM(0), LPARAM(0)),
            Shortcut::Cut => (WM_CUT, WPARAM(0), LPARAM(0)),
            Shortcut::Copy => (WM_COPY, WPARAM(0), LPARAM(0)),
            Shortcut::Paste => (WM_PASTE, WPARAM(0), LPARAM(0)),
            Shortcut::Clear => (WM_CLEAR, WPARAM(0), LPARAM(0)),
            // Start 0, end -1 means "to the end of the text".
            Shortcut::SelectAll => (EM_SETSEL, WPARAM(0), LPARAM(-1)),
        }
    }

    /// Names of every supported shortcut, for error messages and tool schemas.
    pub fn all() -> &'static [&'static str] {
        &["undo", "cut", "copy", "paste", "clear", "select_all"]
    }
}

fn send(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM, timeout_ms: u32) -> Option<usize> {
    unsafe {
        let mut result: usize = 0;
        let r = SendMessageTimeoutW(hwnd, msg, w, l, SMTO_ABORTIFHUNG, timeout_ms, Some(&mut result));
        if r.0 == 0 {
            None
        } else {
            Some(result)
        }
    }
}

/// Apply a shortcut to a window (or its focused control) in the background.
///
/// Targets the focused child rather than the top-level frame: a frame window does not
/// own the text, the edit control inside it does, and `WM_UNDO` to the frame is a
/// no-op that looks like success.
pub fn apply(hwnd: HWND, name: &str) -> Result<(), CoreError> {
    let shortcut = Shortcut::parse(name).ok_or(CoreError::BackgroundUnsupported {
        action: "shortcut",
        hwnd: 0,
    })?;
    apply_shortcut(hwnd, shortcut, DEFAULT_TIMEOUT_MS)
}

pub fn apply_shortcut(hwnd: HWND, shortcut: Shortcut, timeout_ms: u32) -> Result<(), CoreError> {
    unsafe {
        if !IsWindow(hwnd).as_bool() {
            return Err(CoreError::WindowGone);
        }
    }
    let target = crate::input::postmessage::focused_child(hwnd)?;
    let (msg, w, l) = shortcut.message();
    match send(target, msg, w, l, timeout_ms) {
        Some(_) => Ok(()),
        None => Err(CoreError::BackgroundUnsupported {
            action: "shortcut",
            hwnd: target.0 as usize,
        }),
    }
}

/// Whether the focused control reports that it has something to undo.
///
/// Lets a caller verify an undo actually did something instead of trusting a message
/// that a non-edit control may have silently ignored.
pub fn can_undo(hwnd: HWND) -> bool {
    let Ok(target) = crate::input::postmessage::focused_child(hwnd) else {
        return false;
    };
    send(target, EM_CANUNDO, WPARAM(0), LPARAM(0), DEFAULT_TIMEOUT_MS)
        .map(|r| r != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_parse_from_key_combinations() {
        assert_eq!(Shortcut::parse("Ctrl+Z"), Some(Shortcut::Undo));
        assert_eq!(Shortcut::parse("ctrl+z"), Some(Shortcut::Undo));
        assert_eq!(Shortcut::parse("Control + Z"), Some(Shortcut::Undo));
        assert_eq!(Shortcut::parse("Ctrl+A"), Some(Shortcut::SelectAll));
        assert_eq!(Shortcut::parse("Ctrl+V"), Some(Shortcut::Paste));
    }

    #[test]
    fn shortcuts_parse_from_friendly_names() {
        assert_eq!(Shortcut::parse("undo"), Some(Shortcut::Undo));
        assert_eq!(Shortcut::parse("SELECT_ALL"), Some(Shortcut::SelectAll));
        assert_eq!(Shortcut::parse("select-all"), Some(Shortcut::SelectAll));
        assert_eq!(Shortcut::parse("copy"), Some(Shortcut::Copy));
    }

    #[test]
    fn unsupported_shortcuts_are_rejected_rather_than_approximated() {
        // Ctrl+S has no standard control message. Returning some near-miss message
        // would be worse than saying no.
        assert_eq!(Shortcut::parse("Ctrl+S"), None);
        assert_eq!(Shortcut::parse("Ctrl+Shift+P"), None);
        assert_eq!(Shortcut::parse("nonsense"), None);
    }

    #[test]
    fn select_all_selects_to_the_end_of_the_text() {
        let (msg, w, l) = Shortcut::SelectAll.message();
        assert_eq!(msg, EM_SETSEL);
        assert_eq!(w.0, 0, "selection starts at character 0");
        assert_eq!(l.0, -1, "-1 means end of text");
    }

    #[test]
    fn each_shortcut_maps_to_its_documented_message() {
        assert_eq!(Shortcut::Undo.message().0, WM_UNDO);
        assert_eq!(Shortcut::Cut.message().0, WM_CUT);
        assert_eq!(Shortcut::Copy.message().0, WM_COPY);
        assert_eq!(Shortcut::Paste.message().0, WM_PASTE);
        assert_eq!(Shortcut::Clear.message().0, WM_CLEAR);
    }

    #[test]
    fn every_advertised_name_actually_parses() {
        for name in Shortcut::all() {
            assert!(Shortcut::parse(name).is_some(), "advertised '{name}' does not parse");
        }
    }

    #[test]
    fn applying_to_a_dead_window_is_an_error() {
        let h = HWND(std::ptr::null_mut());
        assert!(matches!(apply(h, "undo"), Err(CoreError::WindowGone)));
        assert!(!can_undo(h));
    }

    #[test]
    fn an_unknown_shortcut_name_fails_before_touching_the_window() {
        // Ordering matters: an unknown name must not be reported as "window gone".
        let h = HWND(std::ptr::null_mut());
        assert!(matches!(
            apply(h, "ctrl+shift+q"),
            Err(CoreError::BackgroundUnsupported { action: "shortcut", .. })
        ));
    }
}
