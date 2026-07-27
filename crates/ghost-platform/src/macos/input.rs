//! Keyboard and mouse synthesis, over CoreGraphics events.
//!
//! | Ghost operation | Apple API |
//! | --- | --- |
//! | key down/up | `CGEventCreateKeyboardEvent` + `CGEventPost` |
//! | typing arbitrary text | `CGEventKeyboardSetUnicodeString` |
//! | modifiers (Cmd/Shift/…) | `CGEventSetFlags` with `CGEventFlags` |
//! | mouse click / move | `CGEventCreateMouseEvent` + `CGEventPost` |
//!
//! **Hotkeys are expressed as flags, never as synthetic modifier keystrokes.**
//! Posting a Cmd keydown, then a key, then a Cmd keyup is the intuitive approach
//! and it is wrong: the modifier state an app reads comes from the *event's*
//! flags, and interleaved real input from the user can land between the posts,
//! leaving a modifier stuck down system-wide. Setting `CGEventFlags` on the key
//! event itself is atomic and cannot leak state. (This is also why Ghost's
//! Windows backend refuses modifier combos in background mode — see the README.)
//!
//! All of this synthesis is **foreground input**: CGEvent posts to the window
//! server's session-wide queue, so it goes to whatever is focused. There is no
//! macOS equivalent of Windows' posted window messages, which is why
//! `BackgroundDispatch` is not claimed on macOS.

use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

use super::error::{MacError, MacResult};
use super::perms::require_accessibility;
use crate::types::Point;

/// A named modifier key, so callers do not pass raw bit patterns around.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    /// The Command key. Ghost maps a caller's "Ctrl" to this on macOS — see
    /// [`modifier_from_str`].
    Command,
    Shift,
    /// The actual Control key (rarely a shortcut modifier on macOS).
    Control,
    /// The Option/Alt key.
    Option,
    /// The `fn` key.
    Function,
}

impl Modifier {
    /// The `CGEventFlags` bit for this modifier.
    pub fn flag(&self) -> CGEventFlags {
        match self {
            Modifier::Command => CGEventFlags::CGEventFlagCommand,
            Modifier::Shift => CGEventFlags::CGEventFlagShift,
            Modifier::Control => CGEventFlags::CGEventFlagControl,
            Modifier::Option => CGEventFlags::CGEventFlagAlternate,
            Modifier::Function => CGEventFlags::CGEventFlagSecondaryFn,
        }
    }
}

/// Parse a modifier name as a caller (or an intent JSON file) would write it.
///
/// **"Ctrl" maps to Command.** Ghost's cross-platform vocabulary and every intent
/// file written against the Windows backend say `Ctrl` for the edit shortcuts
/// (`Ctrl+C`, `Ctrl+V`, `Ctrl+A`). The macOS equivalent of all of those is
/// Command, so mapping it here is what makes an existing flow work unchanged.
/// A caller who genuinely wants the physical Control key asks for `"control"`.
pub fn modifier_from_str(name: &str) -> Option<Modifier> {
    match name.to_lowercase().as_str() {
        "cmd" | "command" | "meta" | "super" | "win" | "ctrl" => Some(Modifier::Command),
        "shift" => Some(Modifier::Shift),
        "control" | "ctl" => Some(Modifier::Control),
        "alt" | "option" | "opt" => Some(Modifier::Option),
        "fn" | "function" => Some(Modifier::Function),
        _ => None,
    }
}

/// Combine modifiers into a single `CGEventFlags` mask.
pub fn flags_for(modifiers: &[Modifier]) -> CGEventFlags {
    let mut flags = CGEventFlags::CGEventFlagNull;
    for m in modifiers {
        flags |= m.flag();
    }
    flags
}

/// Parse and combine modifier names in one step, rejecting unknown names rather
/// than silently dropping them — a dropped modifier turns `Cmd+Q` into `Q`.
pub fn flags_from_names(names: &[String]) -> MacResult<CGEventFlags> {
    let mut mods = Vec::with_capacity(names.len());
    for name in names {
        let Some(m) = modifier_from_str(name) else {
            return Err(MacError::InvalidArgument(format!(
                "unknown modifier {name:?} (expected cmd/ctrl, shift, control, alt/option, or fn)"
            )));
        };
        mods.push(m);
    }
    Ok(flags_for(&mods))
}

/// Virtual keycode for a named key.
///
/// These are the layout-independent `kVK_*` codes from Carbon's `Events.h`. They
/// identify a physical key position, so the letter codes below are only correct
/// for a US-ANSI layout — which is why [`type_text`] uses the Unicode path
/// instead of composing letters from keycodes.
pub fn keycode_for(key: &str) -> Option<u16> {
    let normalized = key.to_lowercase();
    let code = match normalized.as_str() {
        "a" => 0x00,
        "s" => 0x01,
        "d" => 0x02,
        "f" => 0x03,
        "h" => 0x04,
        "g" => 0x05,
        "z" => 0x06,
        "x" => 0x07,
        "c" => 0x08,
        "v" => 0x09,
        "b" => 0x0B,
        "q" => 0x0C,
        "w" => 0x0D,
        "e" => 0x0E,
        "r" => 0x0F,
        "y" => 0x10,
        "t" => 0x11,
        "1" => 0x12,
        "2" => 0x13,
        "3" => 0x14,
        "4" => 0x15,
        "6" => 0x16,
        "5" => 0x17,
        "9" => 0x19,
        "7" => 0x1A,
        "8" => 0x1C,
        "0" => 0x1D,
        "o" => 0x1F,
        "u" => 0x20,
        "i" => 0x22,
        "p" => 0x23,
        "l" => 0x25,
        "j" => 0x26,
        "k" => 0x28,
        "n" => 0x2D,
        "m" => 0x2E,
        "return" | "enter" => 0x24,
        "tab" => 0x30,
        "space" => 0x31,
        "delete" | "backspace" => 0x33,
        "escape" | "esc" => 0x35,
        "left" => 0x7B,
        "right" => 0x7C,
        "down" => 0x7D,
        "up" => 0x7E,
        "home" => 0x73,
        "end" => 0x77,
        "pageup" => 0x74,
        "pagedown" => 0x79,
        "forwarddelete" => 0x75,
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
        _ => return None,
    };
    Some(code)
}

/// A fresh `CGEventSource` — `CGEventSourceCreate(kCGEventSourceStateHIDSystemState)`.
///
/// HID system state is what a real keyboard uses, so synthesized events carry the
/// same modifier and caps-lock context an app would see from hardware.
fn event_source() -> MacResult<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| MacError::EventSource)
}

/// Press and release one key, with optional modifiers —
/// `CGEventCreateKeyboardEvent` twice, flags set on both.
///
/// The flags go on the key events themselves rather than being posted as separate
/// modifier keystrokes; see the module docs for why that distinction matters.
pub fn press_key(key: &str, modifiers: &[Modifier]) -> MacResult<()> {
    require_accessibility()?;
    let Some(code) = keycode_for(key) else {
        return Err(MacError::InvalidArgument(format!("unknown key {key:?}")));
    };
    post_keycode(code, flags_for(modifiers))
}

/// Press and release a keycode with an explicit flag mask.
pub fn post_keycode(code: u16, flags: CGEventFlags) -> MacResult<()> {
    let source = event_source()?;
    let down = CGEvent::new_keyboard_event(source, code, true)
        .map_err(|_| MacError::EventCreation("key down"))?;
    down.set_flags(flags);
    down.post(CGEventTapLocation::HID);

    let source = event_source()?;
    let up = CGEvent::new_keyboard_event(source, code, false)
        .map_err(|_| MacError::EventCreation("key up"))?;
    up.set_flags(flags);
    up.post(CGEventTapLocation::HID);
    Ok(())
}

/// A keyboard shortcut given as modifier names plus a key, e.g. `["cmd"], "c"`.
pub fn hotkey(modifier_names: &[String], key: &str) -> MacResult<()> {
    require_accessibility()?;
    let flags = flags_from_names(modifier_names)?;
    let Some(code) = keycode_for(key) else {
        return Err(MacError::InvalidArgument(format!("unknown key {key:?}")));
    };
    post_keycode(code, flags)
}

/// Type arbitrary text — `CGEventKeyboardSetUnicodeString`.
///
/// This posts the *characters* rather than physical key positions, so it is
/// correct on any keyboard layout and handles text that has no keycode at all
/// (accents, emoji, CJK). Text is sent in chunks because the Unicode string
/// attached to a single event is bounded.
pub fn type_text(text: &str) -> MacResult<()> {
    require_accessibility()?;
    if text.is_empty() {
        return Ok(());
    }
    for chunk in chunk_utf16(text, UNICODE_CHUNK) {
        let source = event_source()?;
        // Keycode 0 is a placeholder: the Unicode string overrides it.
        let event = CGEvent::new_keyboard_event(source, 0, true)
            .map_err(|_| MacError::EventCreation("unicode key down"))?;
        event.set_string_from_utf16_unchecked(&chunk);
        event.post(CGEventTapLocation::HID);

        let source = event_source()?;
        let up = CGEvent::new_keyboard_event(source, 0, false)
            .map_err(|_| MacError::EventCreation("unicode key up"))?;
        up.set_string_from_utf16_unchecked(&chunk);
        up.post(CGEventTapLocation::HID);
    }
    Ok(())
}

/// How many UTF-16 code units to attach to one keyboard event.
///
/// `CGEventKeyboardSetUnicodeString` accepts a bounded string; 20 is comfortably
/// under any observed limit and keeps a long paste from being dropped silently.
const UNICODE_CHUNK: usize = 20;

/// Split text into UTF-16 chunks without ever splitting a surrogate pair.
///
/// Splitting a pair would post half of an astral character (an emoji, say) and the
/// app would render a replacement glyph, so the boundary check is load-bearing.
pub fn chunk_utf16(text: &str, max: usize) -> Vec<Vec<u16>> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < units.len() {
        let mut end = (start + max).min(units.len());
        // 0xD800..=0xDBFF is a high surrogate; it must keep its low partner.
        if end < units.len() && (0xD800..=0xDBFF).contains(&units[end - 1]) {
            end -= 1;
        }
        chunks.push(units[start..end].to_vec());
        start = end;
    }
    chunks
}

/// Move the cursor and click — `CGEventCreateMouseEvent` down then up.
pub fn click_at(point: Point, button: MouseButton, count: u8) -> MacResult<()> {
    require_accessibility()?;
    let location = CGPoint {
        x: point.x as f64,
        y: point.y as f64,
    };
    let (down_type, up_type, cg_button) = button.event_types();

    for _ in 0..count.max(1) {
        let source = event_source()?;
        let down = CGEvent::new_mouse_event(source, down_type, location, cg_button)
            .map_err(|_| MacError::EventCreation("mouse down"))?;
        down.post(CGEventTapLocation::HID);

        let source = event_source()?;
        let up = CGEvent::new_mouse_event(source, up_type, location, cg_button)
            .map_err(|_| MacError::EventCreation("mouse up"))?;
        up.post(CGEventTapLocation::HID);
    }
    Ok(())
}

/// Move the cursor without pressing anything — a `MouseMoved` CGEvent.
pub fn move_cursor(point: Point) -> MacResult<()> {
    require_accessibility()?;
    let source = event_source()?;
    let location = CGPoint {
        x: point.x as f64,
        y: point.y as f64,
    };
    let event = CGEvent::new_mouse_event(
        source,
        CGEventType::MouseMoved,
        location,
        CGMouseButton::Left,
    )
    .map_err(|_| MacError::EventCreation("mouse move"))?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

/// Which physical mouse button to synthesize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
}

impl MouseButton {
    fn event_types(&self) -> (CGEventType, CGEventType, CGMouseButton) {
        match self {
            MouseButton::Left => (
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseUp,
                CGMouseButton::Left,
            ),
            MouseButton::Right => (
                CGEventType::RightMouseDown,
                CGEventType::RightMouseUp,
                CGMouseButton::Right,
            ),
        }
    }
}

#[cfg(all(test, feature = "mac-headless-tests"))]
mod tests {
    use super::*;

    #[test]
    fn every_modifier_maps_to_its_documented_cgeventflag_bit() {
        // These bit values are ABI: they are what apps read to decide whether a
        // shortcut fired. A typo here silently turns Cmd+C into a bare C.
        assert_eq!(Modifier::Command.flag().bits(), 0x0010_0000);
        assert_eq!(Modifier::Shift.flag().bits(), 0x0002_0000);
        assert_eq!(Modifier::Control.flag().bits(), 0x0004_0000);
        assert_eq!(Modifier::Option.flag().bits(), 0x0008_0000);
        assert_eq!(Modifier::Function.flag().bits(), 0x0080_0000);
    }

    #[test]
    fn ctrl_is_mapped_to_command_so_existing_intents_keep_working() {
        // Every intent JSON written against the Windows backend says Ctrl+C.
        assert_eq!(modifier_from_str("ctrl"), Some(Modifier::Command));
        assert_eq!(modifier_from_str("Ctrl"), Some(Modifier::Command));
        assert_eq!(modifier_from_str("cmd"), Some(Modifier::Command));
        // ...but the physical Control key stays reachable under its full name.
        assert_eq!(modifier_from_str("control"), Some(Modifier::Control));
    }

    #[test]
    fn modifier_parsing_is_case_insensitive_and_rejects_nonsense() {
        assert_eq!(modifier_from_str("SHIFT"), Some(Modifier::Shift));
        assert_eq!(modifier_from_str("Option"), Some(Modifier::Option));
        assert_eq!(modifier_from_str("hyper"), None);
        assert_eq!(modifier_from_str(""), None);
    }

    #[test]
    fn combined_flags_are_the_union_of_their_bits() {
        let flags = flags_for(&[Modifier::Command, Modifier::Shift]);
        assert!(flags.contains(CGEventFlags::CGEventFlagCommand));
        assert!(flags.contains(CGEventFlags::CGEventFlagShift));
        assert!(!flags.contains(CGEventFlags::CGEventFlagControl));
        assert_eq!(flags_for(&[]), CGEventFlags::CGEventFlagNull);
    }

    #[test]
    fn an_unknown_modifier_name_is_an_error_not_a_silent_drop() {
        // Dropping a modifier would turn Cmd+Q into Q, i.e. type a letter into
        // the user's document instead of quitting.
        let err = flags_from_names(&["cmd".into(), "hyper".into()]).unwrap_err();
        assert!(err.to_string().contains("hyper"), "{err}");

        let ok = flags_from_names(&["cmd".into(), "shift".into()]).unwrap();
        assert!(ok.contains(CGEventFlags::CGEventFlagCommand));
        assert!(ok.contains(CGEventFlags::CGEventFlagShift));
    }

    #[test]
    fn the_edit_shortcut_keys_all_resolve_to_keycodes() {
        // Cmd+A/C/V/X/Z are the EditShortcuts capability; a missing code here
        // means that capability is a lie.
        for key in ["a", "c", "v", "x", "z"] {
            assert!(keycode_for(key).is_some(), "no keycode for {key}");
        }
        assert_eq!(keycode_for("a"), Some(0x00));
        assert_eq!(keycode_for("c"), Some(0x08));
        assert_eq!(keycode_for("v"), Some(0x09));
    }

    #[test]
    fn named_keys_resolve_case_insensitively() {
        assert_eq!(keycode_for("Return"), keycode_for("enter"));
        assert_eq!(keycode_for("ESC"), keycode_for("escape"));
        assert_eq!(keycode_for("Tab"), Some(0x30));
        assert_eq!(keycode_for("space"), Some(0x31));
        assert_eq!(keycode_for("F1"), Some(0x7A));
        assert_eq!(keycode_for("nope"), None);
    }

    #[test]
    fn keycodes_are_unique_so_no_two_keys_alias() {
        let keys = [
            "a", "s", "d", "f", "h", "g", "z", "x", "c", "v", "b", "q", "w", "e", "r", "y", "t",
            "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "o", "u", "i", "p", "l", "j", "k",
            "n", "m", "return", "tab", "space", "delete", "escape", "left", "right", "up", "down",
            "home", "end", "pageup", "pagedown", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8",
            "f9", "f10", "f11", "f12",
        ];
        let mut seen = std::collections::HashMap::new();
        for key in keys {
            let code = keycode_for(key).unwrap_or_else(|| panic!("no keycode for {key}"));
            if let Some(previous) = seen.insert(code, key) {
                panic!("{key} and {previous} both map to keycode {code:#04x}");
            }
        }
    }

    #[test]
    fn text_is_chunked_without_splitting_a_surrogate_pair() {
        // "😀" is one astral char = two UTF-16 units. Splitting it would post
        // half a character and the app would show a replacement glyph.
        let text = "ab😀cd";
        let chunks = chunk_utf16(text, 3);
        for chunk in &chunks {
            if let Some(&last) = chunk.last() {
                assert!(
                    !(0xD800..=0xDBFF).contains(&last),
                    "chunk ended on a high surrogate: {chunk:?}"
                );
            }
        }
        // Nothing is lost or duplicated in the process.
        let rejoined: Vec<u16> = chunks.iter().flatten().copied().collect();
        assert_eq!(rejoined, text.encode_utf16().collect::<Vec<_>>());
    }

    #[test]
    fn chunking_covers_the_whole_string_for_many_sizes() {
        for text in ["", "a", "hello ghost", "café — naïve", "😀😀😀😀", "日本語テキスト"] {
            for max in 1..=8 {
                let chunks = chunk_utf16(text, max);
                let rejoined: Vec<u16> = chunks.iter().flatten().copied().collect();
                assert_eq!(
                    rejoined,
                    text.encode_utf16().collect::<Vec<_>>(),
                    "lost data chunking {text:?} at {max}"
                );
                assert!(
                    chunks.iter().all(|c| !c.is_empty()),
                    "produced an empty chunk for {text:?} at {max}"
                );
                let reassembled = String::from_utf16(&rejoined)
                    .unwrap_or_else(|e| panic!("chunks of {text:?} at {max} are not valid UTF-16: {e}"));
                assert_eq!(reassembled, text, "chunks do not reassemble into {text:?}");
            }
        }
    }

    #[test]
    fn empty_text_produces_no_events() {
        assert!(chunk_utf16("", 20).is_empty());
    }
}
