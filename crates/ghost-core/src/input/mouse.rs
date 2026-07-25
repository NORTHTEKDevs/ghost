use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};
use crate::error::CoreError;
use super::hotkey::is_stopped;

/// Pixel -> absolute-mouse conversion, with the virtual-desktop bounds passed in
/// as `(origin_x, origin_y, width, height)`. Split out from [`to_absolute`] so the
/// arithmetic is unit-testable on a single-monitor build machine.
///
/// The coordinates Ghost clicks come from UIA bounding rectangles, which are
/// VIRTUAL-DESKTOP coordinates: negative for monitors left of or above the
/// primary, and larger than the primary's size for monitors right of or below it.
/// Scaling those against the primary monitor's dimensions (SM_CXSCREEN) put every
/// click on a secondary monitor in the wrong place.
///
/// Out-of-range inputs are clamped rather than allowed to wrap: a stale rect
/// should misclick at a screen edge, never at an arbitrary wrapped coordinate.
pub(crate) fn to_absolute_in(x: i32, y: i32, virt: (i32, i32, i32, i32)) -> (i32, i32) {
    let (vx, vy, vw, vh) = virt;
    if vw <= 1 || vh <= 1 {
        return (0, 0);
    }
    // (vw - 1) so the last pixel column maps exactly onto 65535 instead of
    // stopping one step short.
    let ax = ((x - vx) as i64 * 65535) / (vw - 1) as i64;
    let ay = ((y - vy) as i64 * 65535) / (vh - 1) as i64;
    (ax.clamp(0, 65535) as i32, ay.clamp(0, 65535) as i32)
}

/// Convert pixel coordinates to Windows absolute mouse coordinates (0-65535),
/// normalised across the whole virtual desktop.
pub fn to_absolute(x: i32, y: i32) -> (i32, i32) {
    unsafe {
        to_absolute_in(
            x,
            y,
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            ),
        )
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
                // VIRTUALDESK is required for the 0-65535 space to span every
                // monitor. Without it SendInput maps that range onto the primary
                // display alone, so no absolute coordinate can reach a second
                // monitor no matter how it was computed.
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
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
    fn to_absolute_maps_origin_of_virtual_desktop_to_zero() {
        // Single-monitor desktop rooted at (0,0): origin maps to 0.
        let (ax, ay) = to_absolute_in(0, 0, (0, 0, 1920, 1080));
        assert_eq!((ax, ay), (0, 0));
    }

    #[test]
    fn to_absolute_maps_far_corner_to_full_scale() {
        let (ax, ay) = to_absolute_in(1919, 1079, (0, 0, 1920, 1080));
        assert_eq!((ax, ay), (65535, 65535), "last pixel must reach full scale");
    }

    #[test]
    fn to_absolute_handles_monitor_right_of_primary() {
        // Second 1920-wide monitor to the RIGHT: virtual desktop is 3840 wide,
        // still rooted at 0. A point on the second monitor is past the primary's
        // width - the old SM_CXSCREEN formula produced > 65535 here and the click
        // landed off-screen or clamped onto the primary.
        let virt = (0, 0, 3840, 1080);
        let (ax, _) = to_absolute_in(2880, 540, virt); // middle of monitor 2
        assert!(
            (ax - 49145).abs() < 32,
            "midpoint of the second monitor should land near 3/4 scale, got {ax}"
        );
        assert!(ax <= 65535, "must stay within the absolute range");
    }

    #[test]
    fn to_absolute_handles_monitor_left_of_primary_negative_origin() {
        // Second monitor to the LEFT: virtual desktop origin is NEGATIVE. UIA
        // hands out negative x for elements there. The old formula produced a
        // negative absolute coordinate, which SendInput interprets as garbage.
        let virt = (-1920, 0, 3840, 1080);
        let (ax, _) = to_absolute_in(-1920, 540, virt); // leftmost pixel
        assert_eq!(ax, 0, "leftmost pixel of the virtual desktop must map to 0");

        let (ax2, _) = to_absolute_in(-960, 540, virt); // middle of left monitor
        assert!(
            ax2 > 0 && ax2 < 32768,
            "a point on the left monitor must land in the first half, got {ax2}"
        );
    }

    #[test]
    fn to_absolute_clamps_out_of_range_rather_than_wrapping() {
        // A stale or bogus rect must not produce a wild coordinate.
        let virt = (0, 0, 1920, 1080);
        assert_eq!(to_absolute_in(999_999, 999_999, virt), (65535, 65535));
        assert_eq!(to_absolute_in(-999_999, -999_999, virt), (0, 0));
    }

    #[test]
    fn to_absolute_degenerate_bounds_do_not_panic() {
        assert_eq!(to_absolute_in(10, 10, (0, 0, 0, 0)), (0, 0));
        assert_eq!(to_absolute_in(10, 10, (0, 0, 1, 1)), (0, 0));
    }

    #[test]
    fn move_event_targets_the_whole_virtual_desktop() {
        // Without MOUSEEVENTF_VIRTUALDESK, SendInput maps 0-65535 onto the
        // PRIMARY monitor only, so no absolute coordinate can ever reach a
        // second monitor.
        let input = move_event(500, 400);
        unsafe {
            assert!(input.Anonymous.mi.dwFlags.contains(MOUSEEVENTF_VIRTUALDESK));
        }
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
