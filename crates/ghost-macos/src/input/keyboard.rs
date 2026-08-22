//! Keyboard input.
//!
//! Sending a real keystroke needs `CGEventCreateKeyboardEvent` +
//! `CGEventPost` (Accessibility-gated, C FFI) -- out of scope here, so every
//! function that would actually type something returns
//! `CoreError::Unsupported`.
//!
//! `name_to_vk` is different: on Windows it is a pure lookup table from a key
//! name to a `VIRTUAL_KEY` constant -- no API call, just data. The macOS
//! equivalent of that table exists too (the well-known `kVK_*` virtual
//! keycodes from Carbon/HIToolbox `Events.h`), and porting it for real -- not
//! stubbing it to always return `None` -- matters for honesty in the other
//! direction: `GhostSession::press("enter")` should fail with "not
//! implemented on macOS yet" (from `press_key`), not "unknown key name" (which
//! would be true of Windows but false here -- "enter" IS a known key, macOS
//! just can't send it yet). The same table also means a future native
//! backend only has to implement `CGEventPost`, not rebuild this mapping.

/// Mirrors `windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY`'s shape
/// (a `pub` tuple struct wrapping the raw code) so `session.rs`'s `vk.0`
/// field access compiles unchanged. The wrapped value is a macOS virtual
/// keycode (`kVK_*` from Carbon `Events.h`), not a Windows VK code -- the two
/// engines were never required to share numeric spaces, only the shape.
// Name matches `windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY`
// verbatim (not Rust naming convention) so it reads as the same concept on
// both platforms -- the type this mirrors is itself named that way.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VIRTUAL_KEY(pub u16);

/// Map a human-readable key name to a macOS virtual keycode. Case-insensitive.
/// Real, working lookup -- see the module docs for why this is not a stub.
pub fn name_to_vk(name: &str) -> Option<VIRTUAL_KEY> {
    let vk = match name.to_lowercase().as_str() {
        "enter" | "return" => 0x24,
        "tab" => 0x30,
        "escape" | "esc" => 0x35,
        "backspace" => 0x33,
        "delete" | "del" => 0x75,
        "home" => 0x73,
        "end" => 0x77,
        "pageup" => 0x74,
        "pagedown" => 0x79,
        "arrowup" | "up" => 0x7E,
        "arrowdown" | "down" => 0x7D,
        "arrowleft" | "left" => 0x7B,
        "arrowright" | "right" => 0x7C,
        "space" => 0x31,
        "f1" => 0x7A,
        "f2" => 0x78,
        "f3" => 0x63,
        "f4" => 0x76,
        "f5" => 0x60,
        "f6" => 0x61,
        "f7" => 0x62,
        "f8" => 0x64,
        "f9" => 0x65,
        "f10" => 0x6D,
        "f11" => 0x67,
        "f12" => 0x6F,
        "ctrl" | "control" => 0x3B,
        "shift" => 0x38,
        "alt" => 0x3A,
        "win" | "windows" => 0x37, // Command: the macOS analogue of the OS key.
        "a" => 0x00, "b" => 0x0B, "c" => 0x08,
        "d" => 0x02, "e" => 0x0E, "f" => 0x03,
        "g" => 0x05, "h" => 0x04, "i" => 0x22,
        "j" => 0x26, "k" => 0x28, "l" => 0x25,
        "m" => 0x2E, "n" => 0x2D, "o" => 0x1F,
        "p" => 0x23, "q" => 0x0C, "r" => 0x0F,
        "s" => 0x01, "t" => 0x11, "u" => 0x20,
        "v" => 0x09, "w" => 0x0D, "x" => 0x07,
        "y" => 0x10, "z" => 0x06,
        "0" => 0x1D, "1" => 0x12, "2" => 0x13,
        "3" => 0x14, "4" => 0x15, "5" => 0x17,
        "6" => 0x16, "7" => 0x1A, "8" => 0x1C,
        "9" => 0x19,
        "+" | "oem_plus" | "plus" => 0x18, // ANSI_Equal: '=' key, '+' when shifted.
        "numpad+" | "add" => 0x45,         // ANSI_KeypadPlus.
        _ => return None,
    };
    Some(VIRTUAL_KEY(vk))
}

pub fn type_text(_text: &str) -> Result<(), crate::error::CoreError> {
    Err(crate::error::CoreError::Unsupported { op: "type_text", needs: "CGEventCreateKeyboardEvent" })
}

pub fn clear_focused_field() -> Result<(), crate::error::CoreError> {
    Err(crate::error::CoreError::Unsupported { op: "clear_focused_field", needs: "CGEventCreateKeyboardEvent" })
}

pub fn press_key(_vk: VIRTUAL_KEY) -> Result<(), crate::error::CoreError> {
    Err(crate::error::CoreError::Unsupported { op: "press_key", needs: "CGEventCreateKeyboardEvent" })
}

pub fn key_down(_vk: VIRTUAL_KEY) -> Result<(), crate::error::CoreError> {
    Err(crate::error::CoreError::Unsupported { op: "key_down", needs: "CGEventCreateKeyboardEvent" })
}

pub fn key_up(_vk: VIRTUAL_KEY) -> Result<(), crate::error::CoreError> {
    Err(crate::error::CoreError::Unsupported { op: "key_up", needs: "CGEventCreateKeyboardEvent" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_to_vk_enter_maps_to_return() {
        assert_eq!(name_to_vk("Enter"), Some(VIRTUAL_KEY(0x24)));
    }

    #[test]
    fn name_to_vk_is_case_insensitive() {
        assert_eq!(name_to_vk("ESCAPE"), Some(VIRTUAL_KEY(0x35)));
    }

    #[test]
    fn name_to_vk_unknown_returns_none() {
        assert_eq!(name_to_vk("blarg"), None);
    }

    #[test]
    fn every_recognized_name_fails_honestly_at_dispatch() {
        let vk = name_to_vk("enter").unwrap();
        assert!(matches!(press_key(vk), Err(crate::error::CoreError::Unsupported { .. })));
    }
}
