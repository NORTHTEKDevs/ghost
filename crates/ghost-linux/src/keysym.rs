//! Key naming.
//!
//! Ghost's MCP contract uses names like `enter`, `tab`, `f5`, `ctrl`. Windows
//! maps those to virtual-key codes; Linux maps them to **X11 keysyms**, which
//! are the right common currency here: the RemoteDesktop portal accepts keysyms
//! directly (`NotifyKeyboardKeysym`), and X11 can translate a keysym to a
//! keycode through the active keymap.

/// An X11 keysym. Named `VirtualKey` to mirror the Windows engine's type role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualKey(pub u32);

/// Mirrors `ghost_core::input::keyboard::name_to_vk`.
pub fn name_to_vk(name: &str) -> Option<VirtualKey> {
    let n = name.trim().to_ascii_lowercase();
    let sym = match n.as_str() {
        "enter" | "return" => 0xFF0D,
        "tab" => 0xFF09,
        "escape" | "esc" => 0xFF1B,
        "backspace" => 0xFF08,
        "delete" | "del" => 0xFFFF,
        "insert" => 0xFF63,
        "home" => 0xFF50,
        "end" => 0xFF57,
        "pageup" | "page_up" | "pgup" => 0xFF55,
        "pagedown" | "page_down" | "pgdn" => 0xFF56,
        "left" => 0xFF51,
        "up" => 0xFF52,
        "right" => 0xFF53,
        "down" => 0xFF54,
        "space" => 0x0020,
        "ctrl" | "control" => 0xFFE3,
        "alt" => 0xFFE9,
        "shift" => 0xFFE1,
        "super" | "win" | "meta" => 0xFFEB,
        "capslock" => 0xFFE5,
        "printscreen" | "print" => 0xFF61,
        "menu" | "apps" => 0xFF67,
        _ => {
            // f1..f24
            if let Some(rest) = n.strip_prefix('f') {
                if let Ok(i) = rest.parse::<u32>() {
                    if (1..=24).contains(&i) {
                        return Some(VirtualKey(0xFFBE + (i - 1)));
                    }
                }
            }
            // A single character maps to its own keysym.
            let mut chars = n.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else { return None };
            return char_to_keysym(c).map(VirtualKey);
        }
    };
    Some(VirtualKey(sym))
}

/// Map a character to an X11 keysym.
///
/// Latin-1 characters are their own keysym; everything else uses the Unicode
/// keysym range (`0x01000000 + codepoint`), which both XKB and the portal
/// understand. This is what lets Ghost type non-ASCII text without shipping a
/// keymap table.
pub fn char_to_keysym(c: char) -> Option<u32> {
    let cp = c as u32;
    if (0x20..=0x7E).contains(&cp) || (0xA0..=0xFF).contains(&cp) {
        return Some(cp);
    }
    match c {
        '\n' | '\r' => Some(0xFF0D),
        '\t' => Some(0xFF09),
        '\u{8}' => Some(0xFF08),
        _ => Some(0x0100_0000 + cp),
    }
}

/// Whether a key name is a modifier.
pub fn is_modifier(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "ctrl" | "control" | "alt" | "shift" | "super" | "win" | "meta"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_keys_map_to_the_documented_keysyms() {
        assert_eq!(name_to_vk("enter"), Some(VirtualKey(0xFF0D)));
        assert_eq!(name_to_vk("Tab"), Some(VirtualKey(0xFF09)));
        assert_eq!(name_to_vk("ESC"), Some(VirtualKey(0xFF1B)));
    }

    #[test]
    fn function_keys_are_contiguous_from_f1() {
        assert_eq!(name_to_vk("f1"), Some(VirtualKey(0xFFBE)));
        assert_eq!(name_to_vk("f5"), Some(VirtualKey(0xFFBE + 4)));
        assert_eq!(name_to_vk("f12"), Some(VirtualKey(0xFFBE + 11)));
    }

    #[test]
    fn f0_and_f25_are_rejected() {
        assert_eq!(name_to_vk("f0"), None);
        assert_eq!(name_to_vk("f25"), None);
    }

    #[test]
    fn single_characters_map_to_themselves() {
        assert_eq!(name_to_vk("a"), Some(VirtualKey(0x61)));
    }

    #[test]
    fn unknown_multi_character_names_are_rejected_not_guessed() {
        assert_eq!(name_to_vk("wibble"), None);
    }

    #[test]
    fn ascii_chars_are_their_own_keysym() {
        assert_eq!(char_to_keysym('A'), Some(0x41));
        assert_eq!(char_to_keysym(' '), Some(0x20));
    }

    #[test]
    fn non_latin_chars_use_the_unicode_keysym_range() {
        // Without this, typing anything outside Latin-1 would silently fail.
        assert_eq!(char_to_keysym('\u{4e2d}'), Some(0x0100_0000 + 0x4e2d));
        assert_eq!(char_to_keysym('\u{1F600}'), Some(0x0100_0000 + 0x1F600));
    }

    #[test]
    fn newline_types_as_enter() {
        assert_eq!(char_to_keysym('\n'), Some(0xFF0D));
    }

    #[test]
    fn modifiers_are_recognised_case_insensitively() {
        assert!(is_modifier("Ctrl"));
        assert!(is_modifier("SHIFT"));
        assert!(!is_modifier("a"));
    }
}
