use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
use crate::error::CoreError;
use super::hotkey::is_stopped;

/// Convert pixel coordinates to Windows absolute mouse coordinates (0-65535 range).
pub fn to_absolute(x: i32, y: i32) -> (i32, i32) {
    unsafe {
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        if sw == 0 || sh == 0 {
            return (0, 0);
        }
        ((x * 65535) / sw, (y * 65535) / sh)
    }
}

pub fn move_event(x: i32, y: i32) -> INPUT {
    let (ax, ay) = to_absolute(x, y);
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: ax,
                dy: ay,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
                ..Default::default()
            },
        },
    }
}

pub fn click_event(up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: if up { MOUSEEVENTF_LEFTUP } else { MOUSEEVENTF_LEFTDOWN },
                ..Default::default()
            },
        },
    }
}

/// Move mouse to pixel coordinates (x, y) and left-click.
pub fn click(x: i32, y: i32) -> Result<(), CoreError> {
    if is_stopped() {
        return Err(CoreError::Win32 { code: 0, context: "stopped" });
    }
    let inputs = [move_event(x, y), click_event(false), click_event(true)];
    unsafe {
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != inputs.len() as u32 {
            return Err(CoreError::Win32 { code: 0, context: "SendInput: click failed" });
        }
    }
    Ok(())
}

/// Move mouse to pixel coordinates without clicking.
pub fn move_to(x: i32, y: i32) -> Result<(), CoreError> {
    if is_stopped() {
        return Err(CoreError::Win32 { code: 0, context: "stopped" });
    }
    let inputs = [move_event(x, y)];
    unsafe {
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != 1 {
            return Err(CoreError::Win32 { code: 0, context: "SendInput: move_to failed" });
        }
    }
    Ok(())
}

pub fn right_click_event(up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: if up { MOUSEEVENTF_RIGHTUP } else { MOUSEEVENTF_RIGHTDOWN },
                ..Default::default()
            },
        },
    }
}

pub fn scroll_event(delta: i32, horizontal: bool) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                mouseData: delta as u32,
                dwFlags: if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL },
                ..Default::default()
            },
        },
    }
}

/// Move mouse to coordinates without clicking. Triggers hover states and dropdown menus.
pub fn hover(x: i32, y: i32) -> Result<(), CoreError> {
    if is_stopped() {
        return Err(CoreError::Win32 { code: 0, context: "stopped" });
    }
    let inputs = [move_event(x, y)];
    unsafe {
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != 1 {
            return Err(CoreError::Win32 { code: 0, context: "SendInput: hover failed" });
        }
    }
    Ok(())
}

/// Right-click at pixel coordinates.
pub fn right_click(x: i32, y: i32) -> Result<(), CoreError> {
    if is_stopped() {
        return Err(CoreError::Win32 { code: 0, context: "stopped" });
    }
    let inputs = [move_event(x, y), right_click_event(false), right_click_event(true)];
    unsafe {
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != inputs.len() as u32 {
            return Err(CoreError::Win32 { code: 0, context: "SendInput: right_click failed" });
        }
    }
    Ok(())
}

/// Double-click at pixel coordinates.
pub fn double_click(x: i32, y: i32) -> Result<(), CoreError> {
    if is_stopped() {
        return Err(CoreError::Win32 { code: 0, context: "stopped" });
    }
    let inputs = [
        move_event(x, y),
        click_event(false), click_event(true),
        click_event(false), click_event(true),
    ];
    unsafe {
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != inputs.len() as u32 {
            return Err(CoreError::Win32 { code: 0, context: "SendInput: double_click failed" });
        }
    }
    Ok(())
}

/// Click-hold at from, move to to, release. Used for drag-and-drop and selections.
pub fn drag(from_x: i32, from_y: i32, to_x: i32, to_y: i32) -> Result<(), CoreError> {
    if is_stopped() {
        return Err(CoreError::Win32 { code: 0, context: "stopped" });
    }
    let inputs = [
        move_event(from_x, from_y),
        click_event(false),
        move_event(to_x, to_y),
        click_event(true),
    ];
    unsafe {
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != inputs.len() as u32 {
            return Err(CoreError::Win32 { code: 0, context: "SendInput: drag failed" });
        }
    }
    Ok(())
}

/// Scroll wheel at (x, y). direction: "up"/"down"/"left"/"right". amount = notches (1 notch = 120 units).
pub fn scroll(x: i32, y: i32, direction: &str, amount: i32) -> Result<(), CoreError> {
    if is_stopped() {
        return Err(CoreError::Win32 { code: 0, context: "stopped" });
    }
    let delta = match direction {
        "up" => 120 * amount,
        "down" => -(120 * amount),
        "right" => 120 * amount,
        "left" => -(120 * amount),
        _ => return Err(CoreError::Win32 { code: 0, context: "invalid scroll direction" }),
    };
    let horizontal = direction == "left" || direction == "right";
    let inputs = [move_event(x, y), scroll_event(delta, horizontal)];
    unsafe {
        let sent = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != inputs.len() as u32 {
            return Err(CoreError::Win32 { code: 0, context: "SendInput: scroll failed" });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_event_uses_absolute_flag() {
        let input = move_event(500, 400);
        unsafe {
            assert!(input.Anonymous.mi.dwFlags.contains(MOUSEEVENTF_MOVE));
            assert!(input.Anonymous.mi.dwFlags.contains(MOUSEEVENTF_ABSOLUTE));
        }
    }

    #[test]
    fn click_down_uses_leftdown_flag() {
        let input = click_event(false);
        unsafe {
            assert!(input.Anonymous.mi.dwFlags.contains(MOUSEEVENTF_LEFTDOWN));
        }
    }

    #[test]
    fn click_up_uses_leftup_flag() {
        let input = click_event(true);
        unsafe {
            assert!(input.Anonymous.mi.dwFlags.contains(MOUSEEVENTF_LEFTUP));
        }
    }

    #[test]
    fn to_absolute_maps_zero_to_zero() {
        // At x=0, absolute coord should be 0
        // (this tests the formula direction, not exact values since screen size varies)
        let (ax, _ay) = to_absolute(0, 0);
        assert_eq!(ax, 0);
    }

    #[test]
    fn right_click_event_down_uses_rightdown_flag() {
        let input = right_click_event(false);
        unsafe {
            assert!(input.Anonymous.mi.dwFlags.contains(MOUSEEVENTF_RIGHTDOWN));
        }
    }

    #[test]
    fn right_click_event_up_uses_rightup_flag() {
        let input = right_click_event(true);
        unsafe {
            assert!(input.Anonymous.mi.dwFlags.contains(MOUSEEVENTF_RIGHTUP));
        }
    }

    #[test]
    fn scroll_event_vertical_uses_wheel_flag() {
        let input = scroll_event(120, false);
        unsafe {
            assert!(input.Anonymous.mi.dwFlags.contains(MOUSEEVENTF_WHEEL));
            assert_eq!(input.Anonymous.mi.mouseData, 120u32);
        }
    }

    #[test]
    fn scroll_event_horizontal_uses_hwheel_flag() {
        let input = scroll_event(120, true);
        unsafe {
            assert!(input.Anonymous.mi.dwFlags.contains(MOUSEEVENTF_HWHEEL));
        }
    }
}
