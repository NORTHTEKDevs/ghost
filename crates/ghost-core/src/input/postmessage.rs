//! Window-message input: deliver mouse and keyboard events directly to a specific
//! window's message queue instead of through the shared system input queue.
//!
//! `SendInput` writes into the one global input stream, so it moves the user's real
//! cursor and lands wherever focus happens to be. `PostMessageW` writes into *one
//! window's* queue: the user's cursor never moves, the foreground window never
//! changes, and N ghost processes driving N windows never collide.
//!
//! Scope, honestly: this works for Win32, WinForms, MFC, and most WPF controls that
//! keep real child HWNDs. It does **not** work for Chromium/Electron (one HWND, input
//! routed through the browser's own pipeline - use the CDP backend) and is unreliable
//! for UWP/WinUI (use the UIA pattern backend). The action chain tries UIA patterns
//! first for exactly this reason.

use crate::error::CoreError;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VK_TO_VSC};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetGUIThreadInfo, GetWindowThreadProcessId, IsWindow, PostMessageW,
    RealChildWindowFromPoint, SendMessageTimeoutW, GUITHREADINFO, SMTO_ABORTIFHUNG, WHEEL_DELTA,
    WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETTEXT,
};

/// Mouse-button flags carried in the wParam of mouse messages. Not re-exported by
/// the `windows` crate, so declared here against the Win32 header values.
const MK_LBUTTON: usize = 0x0001;
const MK_RBUTTON: usize = 0x0002;

/// Extended-key flag in a WM_KEYDOWN/WM_KEYUP lParam (bit 24).
const KF_EXTENDED_BIT: isize = 1 << 24;
/// Transition-state (key released) flag, bit 31.
const KF_UP_BIT: isize = 1 << 31;
/// Previous-key-state flag, bit 30. Set on key-up alongside the transition bit.
const KF_REPEAT_BIT: isize = 1 << 30;

/// Virtual keys that must carry the extended-key flag for apps to interpret them
/// correctly (arrows, navigation cluster, right-hand modifiers, numpad divide/enter).
fn is_extended_key(vk: u16) -> bool {
    matches!(
        vk,
        0x21..=0x28 // PRIOR NEXT END HOME LEFT UP RIGHT DOWN
            | 0x2D    // INSERT
            | 0x2E    // DELETE
            | 0x5B | 0x5C // LWIN RWIN
            | 0x6F    // DIVIDE
            | 0xA1    // RSHIFT
            | 0xA3    // RCONTROL
            | 0xA5 // RMENU
    )
}

/// Pack a client-relative point into an lParam the way Windows mouse messages expect.
/// Coordinates are 16-bit signed, so negatives must be masked, not truncated.
pub fn point_lparam(x: i32, y: i32) -> LPARAM {
    LPARAM((((y & 0xFFFF) << 16) | (x & 0xFFFF)) as isize)
}

/// Build the lParam for a key message: repeat count 1, hardware scan code, and the
/// extended/transition flags. Controls that read the scan code (games, terminals,
/// text editors doing their own key handling) reject messages without it.
pub fn key_lparam(vk: u16, up: bool) -> LPARAM {
    let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as isize;
    let mut l: isize = 1 | (scan << 16);
    if is_extended_key(vk) {
        l |= KF_EXTENDED_BIT;
    }
    if up {
        l |= KF_UP_BIT | KF_REPEAT_BIT;
    }
    LPARAM(l)
}

fn ensure_window(hwnd: HWND) -> Result<(), CoreError> {
    unsafe {
        if IsWindow(hwnd).as_bool() {
            Ok(())
        } else {
            Err(CoreError::WindowGone)
        }
    }
}

/// Descend from `top` to the deepest child window containing the client point
/// `(x, y)`, returning that child and the point rebased to *its* client area.
///
/// This matters: posting WM_LBUTTONDOWN to a top-level frame usually does nothing
/// because the frame does not own the button - the child control does.
pub fn resolve_target(top: HWND, x: i32, y: i32) -> Result<(HWND, i32, i32), CoreError> {
    ensure_window(top)?;
    unsafe {
        // Convert the caller's top-level client point to screen space once, then walk
        // down, rebasing into each child's client space as we go.
        let mut screen = POINT { x, y };
        let mut current = top;
        // ClientToScreen is the inverse of ScreenToClient; do it by offsetting through
        // the top window's client origin.
        let mut origin = POINT { x: 0, y: 0 };
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(top, &mut origin);
        screen.x += origin.x;
        screen.y += origin.y;

        // Bounded descent: a pathological or self-referential hierarchy must not spin.
        for _ in 0..16 {
            let mut local = screen;
            if !ScreenToClient(current, &mut local).as_bool() {
                break;
            }
            let child = RealChildWindowFromPoint(current, local);
            if child.0.is_null() || child == current {
                break;
            }
            current = child;
        }

        let mut local = screen;
        if !ScreenToClient(current, &mut local).as_bool() {
            return Err(CoreError::BackgroundUnsupported {
                action: "resolve_target",
                hwnd: current.0 as usize,
            });
        }
        Ok((current, local.x, local.y))
    }
}

fn post(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM, ctx: &'static str) -> Result<(), CoreError> {
    unsafe {
        PostMessageW(hwnd, msg, w, l).map_err(|e| CoreError::Win32 {
            code: e.code().0 as u32,
            context: ctx,
        })
    }
}

/// Background left click at a point in `top`'s client area.
///
/// The WM_MOUSEMOVE first is not decorative: many controls (menus, toolbars, hover
/// buttons, owner-drawn widgets) only arm their hit-test state on a move message and
/// ignore a button-down that arrives cold.
pub fn click(hwnd: HWND, client_xy: (i32, i32)) -> Result<(), CoreError> {
    let (target, x, y) = resolve_target(hwnd, client_xy.0, client_xy.1)?;
    let l = point_lparam(x, y);
    post(target, WM_MOUSEMOVE, WPARAM(0), l, "PostMessage move")?;
    post(target, WM_LBUTTONDOWN, WPARAM(MK_LBUTTON), l, "PostMessage down")?;
    post(target, WM_LBUTTONUP, WPARAM(0), l, "PostMessage up")
}

/// Background right click (context menus).
pub fn right_click(hwnd: HWND, client_xy: (i32, i32)) -> Result<(), CoreError> {
    let (target, x, y) = resolve_target(hwnd, client_xy.0, client_xy.1)?;
    let l = point_lparam(x, y);
    post(target, WM_MOUSEMOVE, WPARAM(0), l, "PostMessage move")?;
    post(target, WM_RBUTTONDOWN, WPARAM(MK_RBUTTON), l, "PostMessage rdown")?;
    post(target, WM_RBUTTONUP, WPARAM(0), l, "PostMessage rup")
}

/// Background double click.
pub fn double_click(hwnd: HWND, client_xy: (i32, i32)) -> Result<(), CoreError> {
    let (target, x, y) = resolve_target(hwnd, client_xy.0, client_xy.1)?;
    let l = point_lparam(x, y);
    post(target, WM_LBUTTONDOWN, WPARAM(MK_LBUTTON), l, "PostMessage down")?;
    post(target, WM_LBUTTONUP, WPARAM(0), l, "PostMessage up")?;
    post(target, WM_LBUTTONDBLCLK, WPARAM(MK_LBUTTON), l, "PostMessage dblclk")?;
    post(target, WM_LBUTTONUP, WPARAM(0), l, "PostMessage up2")
}

/// Background mouse move (hover states, tooltips, menu tracking).
pub fn hover(hwnd: HWND, client_xy: (i32, i32)) -> Result<(), CoreError> {
    let (target, x, y) = resolve_target(hwnd, client_xy.0, client_xy.1)?;
    post(target, WM_MOUSEMOVE, WPARAM(0), point_lparam(x, y), "PostMessage hover")
}

/// Background wheel scroll. `notches` is positive for up/right.
///
/// Unlike the button messages, WM_MOUSEWHEEL carries **screen** coordinates in
/// lParam, not client coordinates - a classic source of "scroll goes to the wrong
/// pane" bugs.
pub fn scroll(hwnd: HWND, client_xy: (i32, i32), notches: i32, horizontal: bool) -> Result<(), CoreError> {
    let (target, x, y) = resolve_target(hwnd, client_xy.0, client_xy.1)?;
    let mut pt = POINT { x, y };
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(target, &mut pt);
    }
    let delta = (WHEEL_DELTA as i32) * notches;
    let wparam = WPARAM(((delta as u32 as usize) << 16) & 0xFFFF_0000);
    let msg = if horizontal { 0x020E /* WM_MOUSEHWHEEL */ } else { WM_MOUSEWHEEL };
    post(target, msg, wparam, point_lparam(pt.x, pt.y), "PostMessage wheel")
}

/// The window that currently owns keyboard focus *within the target's own thread*.
///
/// `GetFocus` only reports focus for the calling thread, and `AttachThreadInput`
/// would entangle our input queue with the target's (and can drag focus around).
/// `GetGUIThreadInfo` reads the target thread's focus without either side effect.
pub fn focused_child(hwnd: HWND) -> Result<HWND, CoreError> {
    unsafe {
        let tid = GetWindowThreadProcessId(hwnd, None);
        if tid == 0 {
            return Err(CoreError::WindowGone);
        }
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(tid, &mut info).is_ok() && !info.hwndFocus.0.is_null() {
            Ok(info.hwndFocus)
        } else {
            // No in-thread focus (common for a background window that has never been
            // activated): the top-level window is the best available target.
            Ok(hwnd)
        }
    }
}

/// Post a key press+release to a background window's focused control.
pub fn press_key(hwnd: HWND, vk: u16) -> Result<(), CoreError> {
    let target = focused_child(hwnd)?;
    post(target, WM_KEYDOWN, WPARAM(vk as usize), key_lparam(vk, false), "PostMessage keydown")?;
    post(target, WM_KEYUP, WPARAM(vk as usize), key_lparam(vk, true), "PostMessage keyup")
}

/// Hold a key down without releasing (modifier sequences).
pub fn key_down(hwnd: HWND, vk: u16) -> Result<(), CoreError> {
    let target = focused_child(hwnd)?;
    post(target, WM_KEYDOWN, WPARAM(vk as usize), key_lparam(vk, false), "PostMessage keydown")
}

/// Release a held key.
pub fn key_up(hwnd: HWND, vk: u16) -> Result<(), CoreError> {
    let target = focused_child(hwnd)?;
    post(target, WM_KEYUP, WPARAM(vk as usize), key_lparam(vk, true), "PostMessage keyup")
}

/// Type text into a background window's focused control as WM_CHAR messages.
///
/// WM_CHAR carries the already-translated character, so this is layout-independent -
/// it types the same text whether the user is on QWERTY, AZERTY, or a Cyrillic layout,
/// which synthetic virtual-key events are not.
pub fn type_text(hwnd: HWND, text: &str) -> Result<(), CoreError> {
    let target = focused_child(hwnd)?;
    for unit in text.encode_utf16() {
        post(target, WM_CHAR, WPARAM(unit as usize), LPARAM(1), "PostMessage char")?;
    }
    Ok(())
}

/// Replace a control's whole text in one shot via WM_SETTEXT.
///
/// Uses `SendMessageTimeoutW` rather than `PostMessageW`: WM_SETTEXT reads a buffer
/// the caller owns, so the message must be delivered synchronously while that buffer
/// is still alive. `SMTO_ABORTIFHUNG` keeps a wedged target from blocking us forever.
pub fn set_text(hwnd: HWND, text: &str, timeout_ms: u32) -> Result<(), CoreError> {
    ensure_window(hwnd)?;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut result: usize = 0;
        let r = SendMessageTimeoutW(
            hwnd,
            WM_SETTEXT,
            WPARAM(0),
            LPARAM(wide.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            timeout_ms,
            Some(&mut result),
        );
        if r.0 == 0 {
            return Err(CoreError::BackgroundUnsupported {
                action: "set_text",
                hwnd: hwnd.0 as usize,
            });
        }
    }
    Ok(())
}

/// Client-area size of a window, for translating fractional/relative coordinates.
pub fn client_size(hwnd: HWND) -> Result<(i32, i32), CoreError> {
    ensure_window(hwnd)?;
    unsafe {
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect).map_err(|e| CoreError::Win32 {
            code: e.code().0 as u32,
            context: "GetClientRect",
        })?;
        Ok((rect.right - rect.left, rect.bottom - rect.top))
    }
}

/// Translate an absolute screen point into `hwnd`'s client space, so callers holding
/// UIA bounding rectangles (which are screen-relative) can drive message input.
pub fn screen_to_client(hwnd: HWND, sx: i32, sy: i32) -> Result<(i32, i32), CoreError> {
    ensure_window(hwnd)?;
    unsafe {
        let mut pt = POINT { x: sx, y: sy };
        if !ScreenToClient(hwnd, &mut pt).as_bool() {
            return Err(CoreError::BackgroundUnsupported {
                action: "screen_to_client",
                hwnd: hwnd.0 as usize,
            });
        }
        Ok((pt.x, pt.y))
    }
}

/// Retained for source compatibility with the original background-click API.
pub struct BackgroundClicker;

impl BackgroundClicker {
    pub fn click(hwnd: HWND, client_xy: (i32, i32)) -> Result<(), CoreError> {
        self::click(hwnd, client_xy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_returns_error_when_hwnd_is_zero() {
        let err = click(HWND(std::ptr::null_mut()), (10, 10));
        assert!(matches!(err, Err(CoreError::WindowGone)));
    }

    #[test]
    fn key_messages_reject_dead_windows() {
        let h = HWND(std::ptr::null_mut());
        assert!(press_key(h, 0x0D).is_err());
        assert!(type_text(h, "x").is_err());
        assert!(set_text(h, "x", 100).is_err());
    }

    #[test]
    fn point_lparam_packs_x_low_y_high() {
        let l = point_lparam(0x1234, 0x5678);
        assert_eq!(l.0 & 0xFFFF, 0x1234);
        assert_eq!((l.0 >> 16) & 0xFFFF, 0x5678);
    }

    #[test]
    fn point_lparam_masks_negative_coordinates_into_16_bits() {
        // A point above/left of the client origin is legal and must not corrupt
        // the neighbouring field by sign-extending across it.
        let l = point_lparam(-1, -2);
        assert_eq!(l.0 & 0xFFFF, 0xFFFF);
        assert_eq!((l.0 >> 16) & 0xFFFF, 0xFFFE);
    }

    #[test]
    fn key_lparam_sets_repeat_count_and_transition_bits() {
        let down = key_lparam(0x41, false); // 'A'
        assert_eq!(down.0 & 0xFFFF, 1, "repeat count must be 1");
        assert_eq!(down.0 & KF_UP_BIT, 0, "keydown must not set the transition bit");

        let up = key_lparam(0x41, true);
        assert_ne!(up.0 & KF_UP_BIT, 0, "keyup must set the transition bit");
        assert_ne!(up.0 & KF_REPEAT_BIT, 0, "keyup must set the previous-state bit");
    }

    #[test]
    fn key_lparam_carries_a_scan_code() {
        // 'A' has a real scan code on every layout; a zero here means we posted a
        // message that scan-code-reading controls will drop.
        let l = key_lparam(0x41, false);
        assert_ne!((l.0 >> 16) & 0xFF, 0, "scan code must be present");
    }

    #[test]
    fn arrow_and_right_modifier_keys_are_flagged_extended() {
        assert!(is_extended_key(0x25), "VK_LEFT");
        assert!(is_extended_key(0x28), "VK_DOWN");
        assert!(is_extended_key(0x2E), "VK_DELETE");
        assert!(is_extended_key(0xA3), "VK_RCONTROL");
        assert!(!is_extended_key(0x41), "'A' is not extended");
        assert!(!is_extended_key(0xA0), "VK_LSHIFT is not extended");
        assert_ne!(key_lparam(0x25, false).0 & KF_EXTENDED_BIT, 0);
        assert_eq!(key_lparam(0x41, false).0 & KF_EXTENDED_BIT, 0);
    }
}
