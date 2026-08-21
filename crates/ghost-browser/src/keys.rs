//! Key descriptors for `Input.dispatchKeyEvent`.
//!
//! CDP will happily accept a key event with a missing `code` or virtual key code and
//! then quietly do nothing: many sites read `event.key`, others read `event.keyCode`,
//! and frameworks read `event.code`. Sending all three is what makes a synthetic
//! Enter/Tab actually submit a form.

/// A fully-specified key for CDP: `key`, `code`, virtual key code, and the text the
/// key produces (empty for non-printing keys).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyDescriptor {
    pub key: String,
    pub code: String,
    pub windows_virtual_key_code: i64,
    pub text: String,
}

/// Resolve a friendly key name ("Enter", "Tab", "ArrowDown", "a") to a descriptor.
pub fn describe(name: &str) -> Option<KeyDescriptor> {
    let d = |key: &str, code: &str, vk: i64, text: &str| {
        Some(KeyDescriptor {
            key: key.to_string(),
            code: code.to_string(),
            windows_virtual_key_code: vk,
            text: text.to_string(),
        })
    };
    match name.trim().to_lowercase().as_str() {
        "enter" | "return" => d("Enter", "Enter", 13, "\r"),
        "tab" => d("Tab", "Tab", 9, "\t"),
        "escape" | "esc" => d("Escape", "Escape", 27, ""),
        "backspace" => d("Backspace", "Backspace", 8, ""),
        "delete" | "del" => d("Delete", "Delete", 46, ""),
        "space" => d(" ", "Space", 32, " "),
        "arrowup" | "up" => d("ArrowUp", "ArrowUp", 38, ""),
        "arrowdown" | "down" => d("ArrowDown", "ArrowDown", 40, ""),
        "arrowleft" | "left" => d("ArrowLeft", "ArrowLeft", 37, ""),
        "arrowright" | "right" => d("ArrowRight", "ArrowRight", 39, ""),
        "home" => d("Home", "Home", 36, ""),
        "end" => d("End", "End", 35, ""),
        "pageup" => d("PageUp", "PageUp", 33, ""),
        "pagedown" => d("PageDown", "PageDown", 34, ""),
        other => {
            // Single printable character: derive the descriptor from the character
            // itself so callers can press "a" or "/" without a lookup table entry.
            let mut chars = other.chars();
            let c = chars.next()?;
            if chars.next().is_some() || !c.is_ascii_graphic() {
                return None;
            }
            let upper = c.to_ascii_uppercase();
            let code = if c.is_ascii_alphabetic() {
                format!("Key{upper}")
            } else if c.is_ascii_digit() {
                format!("Digit{c}")
            } else {
                String::new()
            };
            // Preserve the caller's original casing for the emitted text.
            let text = name.trim().to_string();
            d(&text, &code, upper as i64, &text)
        }
    }
}

/// CDP modifier bitmask: Alt=1, Ctrl=2, Meta=4, Shift=8.
pub fn modifier_mask(modifiers: &[String]) -> i64 {
    let mut mask = 0;
    for m in modifiers {
        mask |= match m.trim().to_lowercase().as_str() {
            "alt" => 1,
            "ctrl" | "control" => 2,
            "meta" | "cmd" | "win" => 4,
            "shift" => 8,
            _ => 0,
        };
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_carries_key_code_and_text() {
        let k = describe("Enter").unwrap();
        assert_eq!(k.key, "Enter");
        assert_eq!(k.code, "Enter");
        assert_eq!(k.windows_virtual_key_code, 13);
        assert_eq!(k.text, "\r", "sites listening for text input need the CR");
    }

    #[test]
    fn key_names_are_case_insensitive() {
        assert_eq!(describe("ENTER"), describe("enter"));
        assert_eq!(describe("ArrowDown").unwrap().code, "ArrowDown");
        assert_eq!(describe("down").unwrap().key, "ArrowDown");
    }

    #[test]
    fn non_printing_keys_emit_no_text() {
        for name in ["Escape", "ArrowUp", "Delete", "Home"] {
            assert_eq!(describe(name).unwrap().text, "", "{name} must not insert text");
        }
    }

    #[test]
    fn single_characters_are_derived_not_table_driven() {
        let a = describe("a").unwrap();
        assert_eq!(a.code, "KeyA");
        assert_eq!(a.windows_virtual_key_code, 'A' as i64);
        assert_eq!(a.text, "a", "casing must survive");

        let upper = describe("Z").unwrap();
        assert_eq!(upper.code, "KeyZ");
        assert_eq!(upper.text, "Z");

        assert_eq!(describe("7").unwrap().code, "Digit7");
    }

    #[test]
    fn unknown_multi_character_names_are_rejected() {
        // Returning a bogus descriptor would silently do nothing at runtime; failing
        // here gives the agent an error it can act on.
        assert!(describe("NotAKey").is_none());
        assert!(describe("").is_none());
    }

    #[test]
    fn modifier_mask_matches_the_cdp_bit_layout() {
        assert_eq!(modifier_mask(&["Alt".into()]), 1);
        assert_eq!(modifier_mask(&["Ctrl".into()]), 2);
        assert_eq!(modifier_mask(&["Meta".into()]), 4);
        assert_eq!(modifier_mask(&["Shift".into()]), 8);
        assert_eq!(modifier_mask(&["ctrl".into(), "shift".into()]), 10);
        assert_eq!(modifier_mask(&["bogus".into()]), 0);
    }
}
