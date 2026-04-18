use windows::Win32::UI::Input::KeyboardAndMouse::*;
use crate::error::CoreError;
use super::hotkey::is_stopped;

pub fn key_event(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                dwFlags: if key_up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                ..Default::default()
            },
        },
    }
}

/// Convert text to a sequence of Unicode key events (down+up per char).
pub fn text_to_inputs(text: &str) -> Vec<INPUT> {
    let mut inputs = Vec::new();
    for ch in text.chars() {
        let down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wScan: ch as u16,
                    dwFlags: KEYEVENTF_UNICODE,
                    ..Default::default()
                },
            },
        };
        let up = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wScan: ch as u16,
                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    ..Default::default()
                },
            },
        };
        inputs.push(down);
        inputs.push(up);
    }
    inputs
}

/// Type a string into the focused application using Unicode input events.
/// Checks STOP_FLAG between characters.
pub fn type_text(text: &str) -> Result<(), CoreError> {
    for ch in text.chars() {
        if is_stopped() {
            return Err(CoreError::Win32 { code: 0, context: "stopped" });
        }
        let inputs = text_to_inputs(&ch.to_string());
        unsafe {
            let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
            if sent != inputs.len() as u32 {
                return Err(CoreError::Win32 { code: 0, context: "SendInput: partial delivery" });
            }
        }
    }
    Ok(())
}

/// Press and release a virtual key (non-Unicode, for special keys like Enter, Tab, etc.)
pub fn press_key(vk: VIRTUAL_KEY) -> Result<(), CoreError> {
    if is_stopped() {
        return Err(CoreError::Win32 { code: 0, context: "stopped" });
    }
    let inputs = [key_event(vk, false), key_event(vk, true)];
    unsafe {
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != inputs.len() as u32 {
            return Err(CoreError::Win32 { code: 0, context: "SendInput: press_key failed" });
        }
    }
    Ok(())
}

/// Hold a key down without releasing (pair with key_up for held modifiers).
pub fn key_down(vk: VIRTUAL_KEY) -> Result<(), CoreError> {
    if is_stopped() {
        return Err(CoreError::Win32 { code: 0, context: "stopped" });
    }
    let inputs = [key_event(vk, false)];
    unsafe {
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != 1 {
            return Err(CoreError::Win32 { code: 0, context: "SendInput: key_down failed" });
        }
    }
    Ok(())
}

/// Release a key held by key_down.
pub fn key_up(vk: VIRTUAL_KEY) -> Result<(), CoreError> {
    if is_stopped() {
        return Err(CoreError::Win32 { code: 0, context: "stopped" });
    }
    let inputs = [key_event(vk, true)];
    unsafe {
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != 1 {
            return Err(CoreError::Win32 { code: 0, context: "SendInput: key_up failed" });
        }
    }
    Ok(())
}

/// Map a human-readable key name to a VIRTUAL_KEY constant.
/// Case-insensitive. Returns None for unrecognized names.
pub fn name_to_vk(name: &str) -> Option<VIRTUAL_KEY> {
    match name.to_lowercase().as_str() {
        "enter" | "return" => Some(VK_RETURN),
        "tab" => Some(VK_TAB),
        "escape" | "esc" => Some(VK_ESCAPE),
        "backspace" => Some(VK_BACK),
        "delete" | "del" => Some(VK_DELETE),
        "home" => Some(VK_HOME),
        "end" => Some(VK_END),
        "pageup" => Some(VK_PRIOR),
        "pagedown" => Some(VK_NEXT),
        "arrowup" | "up" => Some(VK_UP),
        "arrowdown" | "down" => Some(VK_DOWN),
        "arrowleft" | "left" => Some(VK_LEFT),
        "arrowright" | "right" => Some(VK_RIGHT),
        "space" => Some(VK_SPACE),
        "f1" => Some(VK_F1),
        "f2" => Some(VK_F2),
        "f3" => Some(VK_F3),
        "f4" => Some(VK_F4),
        "f5" => Some(VK_F5),
        "f6" => Some(VK_F6),
        "f7" => Some(VK_F7),
        "f8" => Some(VK_F8),
        "f9" => Some(VK_F9),
        "f10" => Some(VK_F10),
        "f11" => Some(VK_F11),
        "f12" => Some(VK_F12),
        "ctrl" | "control" => Some(VK_CONTROL),
        "shift" => Some(VK_SHIFT),
        "alt" => Some(VK_MENU),
        "win" | "windows" => Some(VK_LWIN),
        "a" => Some(VK_A), "b" => Some(VK_B), "c" => Some(VK_C),
        "d" => Some(VK_D), "e" => Some(VK_E), "f" => Some(VK_F),
        "g" => Some(VK_G), "h" => Some(VK_H), "i" => Some(VK_I),
        "j" => Some(VK_J), "k" => Some(VK_K), "l" => Some(VK_L),
        "m" => Some(VK_M), "n" => Some(VK_N), "o" => Some(VK_O),
        "p" => Some(VK_P), "q" => Some(VK_Q), "r" => Some(VK_R),
        "s" => Some(VK_S), "t" => Some(VK_T), "u" => Some(VK_U),
        "v" => Some(VK_V), "w" => Some(VK_W), "x" => Some(VK_X),
        "y" => Some(VK_Y), "z" => Some(VK_Z),
        "0" => Some(VK_0), "1" => Some(VK_1), "2" => Some(VK_2),
        "3" => Some(VK_3), "4" => Some(VK_4), "5" => Some(VK_5),
        "6" => Some(VK_6), "7" => Some(VK_7), "8" => Some(VK_8),
        "9" => Some(VK_9),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_down_event_has_correct_vk() {
        let input = key_event(VK_A, false);
        unsafe {
            assert_eq!(input.Anonymous.ki.wVk, VK_A);
            assert_eq!(input.Anonymous.ki.dwFlags, KEYBD_EVENT_FLAGS(0));
        }
    }

    #[test]
    fn key_up_event_has_keyup_flag() {
        let input = key_event(VK_A, true);
        unsafe {
            assert_eq!(input.Anonymous.ki.dwFlags, KEYEVENTF_KEYUP);
        }
    }

    #[test]
    fn text_to_inputs_produces_pairs() {
        // "ab" = down(A) + up(A) + down(B) + up(B) = 4 inputs
        let inputs = text_to_inputs("ab");
        assert_eq!(inputs.len(), 4);
    }

    #[test]
    fn empty_text_produces_no_inputs() {
        assert_eq!(text_to_inputs("").len(), 0);
    }

    #[test]
    fn name_to_vk_enter_maps_to_return() {
        assert_eq!(name_to_vk("Enter"), Some(VK_RETURN));
    }

    #[test]
    fn name_to_vk_is_case_insensitive() {
        assert_eq!(name_to_vk("ESCAPE"), Some(VK_ESCAPE));
    }

    #[test]
    fn name_to_vk_unknown_returns_none() {
        assert_eq!(name_to_vk("blarg"), None);
    }

    #[test]
    fn name_to_vk_f5_maps_correctly() {
        assert_eq!(name_to_vk("F5"), Some(VK_F5));
    }

    #[test]
    fn name_to_vk_arrow_aliases_work() {
        assert_eq!(name_to_vk("up"), Some(VK_UP));
        assert_eq!(name_to_vk("ArrowDown"), Some(VK_DOWN));
    }
}
