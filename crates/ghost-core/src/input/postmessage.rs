//! Background click via PostMessage - delivers WM_LBUTTONDOWN/UP without stealing foreground.

use crate::error::CoreError;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyW, MAPVK_VK_TO_VSC};
use windows::Win32::UI::WindowsAndMessaging::{
    GetGUIThreadInfo, GetWindowThreadProcessId, IsWindow, PostMessageW, SendMessageTimeoutW,
    GUITHREADINFO, SEND_MESSAGE_TIMEOUT_FLAGS, WM_LBUTTONDOWN, WM_LBUTTONUP,
};

// Message constants not re-exported by the enabled `windows` features.
const BM_CLICK: u32 = 0x00F5;
const WM_SETTEXT: u32 = 0x000C;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const MK_LBUTTON: usize = 0x0001;
const MK_RBUTTON: usize = 0x0002;
const SMTO_ABORTIFHUNG: u32 = 0x0002;

fn hwnd_of(raw: isize) -> HWND {
    HWND(raw as *mut core::ffi::c_void)
}

/// Pack a client-relative (x, y) into an LPARAM as Windows mouse messages expect.
fn mouse_lparam(x: i32, y: i32) -> LPARAM {
    LPARAM(((y & 0xFFFF) << 16 | (x & 0xFFFF)) as isize)
}

pub struct BackgroundClicker;

impl BackgroundClicker {
    /// Post a left-click pair to `hwnd` at client-relative `(x, y)`.
    /// Returns `CoreError::WindowGone` if `hwnd` is invalid by the time we check.
    pub fn click(hwnd: HWND, client_xy: (i32, i32)) -> Result<(), CoreError> {
        unsafe {
            if !IsWindow(hwnd).as_bool() {
                return Err(CoreError::WindowGone);
            }
            let (x, y) = client_xy;
            let lparam = LPARAM(((y & 0xFFFF) << 16 | (x & 0xFFFF)) as isize);
            PostMessageW(hwnd, WM_LBUTTONDOWN, WPARAM(0x0001), lparam)
                .map_err(|e| CoreError::ComInit(format!("PostMessage down: {e}")))?;
            PostMessageW(hwnd, WM_LBUTTONUP, WPARAM(0x0000), lparam)
                .map_err(|e| CoreError::ComInit(format!("PostMessage up: {e}")))?;
            Ok(())
        }
    }

    /// Post a left-click pair to `hwnd` at a SCREEN point, converting to the
    /// window's client coordinates first. Use this when the resolved handle is a
    /// container/ancestor window (list, tree, toolbar) rather than a control that
    /// owns its own client area — the element's screen-space centre maps to the
    /// correct client point via ScreenToClient, whereas a size-derived offset
    /// would land in the container's corner.
    pub fn click_screen(hwnd_raw: isize, screen_x: i32, screen_y: i32) -> Result<(), CoreError> {
        let hwnd = hwnd_of(hwnd_raw);
        unsafe {
            if !IsWindow(hwnd).as_bool() {
                return Err(CoreError::WindowGone);
            }
            let mut pt = POINT { x: screen_x, y: screen_y };
            let _ = ScreenToClient(hwnd, &mut pt);
            Self::click(hwnd, (pt.x, pt.y))
        }
    }

    /// Screen point -> client coords for `hwnd` (returns WindowGone if invalid).
    fn client_pt(hwnd: HWND, screen_x: i32, screen_y: i32) -> Result<POINT, CoreError> {
        unsafe {
            if !IsWindow(hwnd).as_bool() {
                return Err(CoreError::WindowGone);
            }
            let mut pt = POINT { x: screen_x, y: screen_y };
            let _ = ScreenToClient(hwnd, &mut pt);
            Ok(pt)
        }
    }

    /// Post a double-click (down, up, dblclk, up) at a SCREEN point — no cursor,
    /// no foreground. `hwnd_raw` is the control/container window handle.
    pub fn double_click_screen(hwnd_raw: isize, screen_x: i32, screen_y: i32) -> Result<(), CoreError> {
        let hwnd = hwnd_of(hwnd_raw);
        let pt = Self::client_pt(hwnd, screen_x, screen_y)?;
        let lp = mouse_lparam(pt.x, pt.y);
        unsafe {
            let post = |msg: u32, wp: usize| PostMessageW(hwnd, msg, WPARAM(wp), lp)
                .map_err(|e| CoreError::ComInit(format!("PostMessage {msg:#x}: {e}")));
            post(WM_LBUTTONDOWN, MK_LBUTTON)?;
            post(WM_LBUTTONUP, 0)?;
            post(WM_LBUTTONDBLCLK, MK_LBUTTON)?;
            post(WM_LBUTTONUP, 0)?;
        }
        Ok(())
    }

    /// Post a right-click (rbutton down, up) at a SCREEN point — no cursor,
    /// no foreground.
    pub fn right_click_screen(hwnd_raw: isize, screen_x: i32, screen_y: i32) -> Result<(), CoreError> {
        let hwnd = hwnd_of(hwnd_raw);
        let pt = Self::client_pt(hwnd, screen_x, screen_y)?;
        let lp = mouse_lparam(pt.x, pt.y);
        unsafe {
            PostMessageW(hwnd, WM_RBUTTONDOWN, WPARAM(MK_RBUTTON), lp)
                .map_err(|e| CoreError::ComInit(format!("PostMessage RBUTTONDOWN: {e}")))?;
            PostMessageW(hwnd, WM_RBUTTONUP, WPARAM(0), lp)
                .map_err(|e| CoreError::ComInit(format!("PostMessage RBUTTONUP: {e}")))?;
        }
        Ok(())
    }

    /// Post a mouse-move (hover) at a SCREEN point — no cursor, no foreground.
    /// Note: many controls only reveal hover state under a real cursor; a posted
    /// WM_MOUSEMOVE reaches the control's message queue but won't move the OS
    /// cursor, so visual hover effects may not appear.
    pub fn hover_screen(hwnd_raw: isize, screen_x: i32, screen_y: i32) -> Result<(), CoreError> {
        let hwnd = hwnd_of(hwnd_raw);
        let pt = Self::client_pt(hwnd, screen_x, screen_y)?;
        unsafe {
            PostMessageW(hwnd, WM_MOUSEMOVE, WPARAM(0), mouse_lparam(pt.x, pt.y))
                .map_err(|e| CoreError::ComInit(format!("PostMessage MOUSEMOVE: {e}")))?;
        }
        Ok(())
    }

    /// Click a standard button-class control via BM_CLICK — the cleanest
    /// non-activating way to press a real Win32 button (no cursor, no foreground).
    /// SendMessageTimeout with ABORTIFHUNG so a wedged target can't block us.
    pub fn button_click(hwnd_raw: isize) -> Result<(), CoreError> {
        let hwnd = hwnd_of(hwnd_raw);
        unsafe {
            if !IsWindow(hwnd).as_bool() {
                return Err(CoreError::WindowGone);
            }
            let mut result: usize = 0;
            // Nonzero function return = delivered; 0 = timed out / target hung.
            let ret = SendMessageTimeoutW(
                hwnd, BM_CLICK, WPARAM(0), LPARAM(0),
                SEND_MESSAGE_TIMEOUT_FLAGS(SMTO_ABORTIFHUNG), 2000, Some(&mut result),
            );
            if ret.0 == 0 {
                return Err(CoreError::Win32 { code: 0, context: "BM_CLICK send timed out / target hung" });
            }
        }
        Ok(())
    }

    /// The HWND that holds keyboard focus within `window_hwnd`'s GUI thread. A
    /// thread keeps its own focus even while its window is in the background, so
    /// this is where posted keystrokes should go. Falls back to the window itself.
    pub fn focused_control(window_hwnd_raw: isize) -> isize {
        let hwnd = hwnd_of(window_hwnd_raw);
        unsafe {
            let tid = GetWindowThreadProcessId(hwnd, None);
            if tid == 0 {
                return window_hwnd_raw;
            }
            let mut gui = GUITHREADINFO {
                cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            if GetGUIThreadInfo(tid, &mut gui).is_ok() && !gui.hwndFocus.is_invalid() {
                gui.hwndFocus.0 as isize
            } else {
                window_hwnd_raw
            }
        }
    }

    /// Post a single virtual-key press (WM_KEYDOWN + WM_KEYUP) to `hwnd` WITHOUT
    /// foreground or cursor movement. Works for un-modified keys (Enter, Tab,
    /// Escape, arrows, F-keys, Delete, single chars). NOTE: modifier combos
    /// (Ctrl+C, Alt+F4) are NOT reliable via posted messages — apps read the real
    /// keyboard state (GetKeyState) for modifiers, which posting doesn't set; the
    /// caller rejects combos in background mode instead of sending a broken combo.
    pub fn send_key(hwnd_raw: isize, vk: u16) -> Result<(), CoreError> {
        let hwnd = hwnd_of(hwnd_raw);
        unsafe {
            if !IsWindow(hwnd).as_bool() {
                return Err(CoreError::WindowGone);
            }
            let scan = (MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) & 0xFF) as isize;
            // Extended-key bit (24) is REQUIRED for the enhanced-keyboard nav cluster
            // (arrows, Home/End, PageUp/Down, Insert/Delete) — without it apps that
            // branch on the extended flag mistake them for numpad keys.
            let ext = matches!(vk, 0x21 | 0x22 | 0x23 | 0x24 | 0x25 | 0x26 | 0x27 | 0x28 | 0x2D | 0x2E) as isize;
            let base = (scan << 16) | 1 | (ext << 24);
            let down = LPARAM(base);
            let up = LPARAM(base | (1 << 30) | (1 << 31));
            PostMessageW(hwnd, WM_KEYDOWN, WPARAM(vk as usize), down)
                .map_err(|e| CoreError::ComInit(format!("PostMessage KEYDOWN: {e}")))?;
            // We post directly (no message pump to run TranslateMessage), so EDIT
            // controls won't insert \r/\t or delete-on-backspace from WM_KEYDOWN
            // alone — synthesize the WM_CHAR they expect for these three keys.
            let ch: Option<u16> = match vk {
                0x0D => Some(b'\r' as u16), // VK_RETURN
                0x09 => Some(b'\t' as u16), // VK_TAB
                0x08 => Some(0x08),         // VK_BACK
                _ => None,
            };
            if let Some(c) = ch {
                PostMessageW(hwnd, WM_CHAR, WPARAM(c as usize), LPARAM(1))
                    .map_err(|e| CoreError::ComInit(format!("PostMessage CHAR (key): {e}")))?;
            }
            PostMessageW(hwnd, WM_KEYUP, WPARAM(vk as usize), up)
                .map_err(|e| CoreError::ComInit(format!("PostMessage KEYUP: {e}")))?;
        }
        Ok(())
    }

    /// Post a character (WM_CHAR) to `hwnd` — for text a virtual key can't express
    /// directly. No foreground, no cursor.
    pub fn send_char(hwnd_raw: isize, ch: char) -> Result<(), CoreError> {
        let hwnd = hwnd_of(hwnd_raw);
        unsafe {
            if !IsWindow(hwnd).as_bool() {
                return Err(CoreError::WindowGone);
            }
            let mut buf = [0u16; 2];
            for unit in ch.encode_utf16(&mut buf).iter() {
                PostMessageW(hwnd, WM_CHAR, WPARAM(*unit as usize), LPARAM(1))
                    .map_err(|e| CoreError::ComInit(format!("PostMessage CHAR: {e}")))?;
            }
        }
        Ok(())
    }

    /// Set a control's text via WM_SETTEXT (replace semantics, like ValuePattern)
    /// WITHOUT activating the window or moving the cursor. Must be a synchronous
    /// SendMessage (the text pointer has to outlive the call), bounded by a
    /// timeout so a hung target can't wedge us.
    pub fn set_text(hwnd_raw: isize, text: &str) -> Result<(), CoreError> {
        let hwnd = hwnd_of(hwnd_raw);
        unsafe {
            if !IsWindow(hwnd).as_bool() {
                return Err(CoreError::WindowGone);
            }
            let mut wide: Vec<u16> = text.encode_utf16().collect();
            wide.push(0);
            let mut result: usize = 0;
            let ret = SendMessageTimeoutW(
                hwnd, WM_SETTEXT, WPARAM(0), LPARAM(wide.as_ptr() as isize),
                SEND_MESSAGE_TIMEOUT_FLAGS(SMTO_ABORTIFHUNG), 2000, Some(&mut result),
            );
            // WM_SETTEXT returns TRUE on success; a 0 function return means the
            // send timed out. On timeout the OS may STILL deliver the queued
            // message later using this lParam pointer — so we must NOT free the
            // buffer, or the delayed WM_SETTEXT reads freed memory in our own
            // process. Leak it (a small one-time leak per hung send) to stay sound.
            if ret.0 == 0 {
                std::mem::forget(wide);
                return Err(CoreError::Win32 { code: 0, context: "WM_SETTEXT send timed out" });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_returns_error_when_hwnd_is_zero() {
        let err = BackgroundClicker::click(HWND(std::ptr::null_mut()), (10, 10));
        assert!(matches!(err, Err(CoreError::WindowGone)));
    }

    #[test]
    fn button_click_returns_error_when_hwnd_is_zero() {
        assert!(matches!(BackgroundClicker::button_click(0), Err(CoreError::WindowGone)));
    }

    #[test]
    fn set_text_returns_error_when_hwnd_is_zero() {
        assert!(matches!(BackgroundClicker::set_text(0, "x"), Err(CoreError::WindowGone)));
    }

    #[test]
    fn click_screen_returns_error_when_hwnd_is_zero() {
        assert!(matches!(BackgroundClicker::click_screen(0, 5, 5), Err(CoreError::WindowGone)));
    }

    #[test]
    fn double_click_screen_returns_error_when_hwnd_is_zero() {
        assert!(matches!(BackgroundClicker::double_click_screen(0, 5, 5), Err(CoreError::WindowGone)));
    }

    #[test]
    fn right_click_screen_returns_error_when_hwnd_is_zero() {
        assert!(matches!(BackgroundClicker::right_click_screen(0, 5, 5), Err(CoreError::WindowGone)));
    }

    #[test]
    fn hover_screen_returns_error_when_hwnd_is_zero() {
        assert!(matches!(BackgroundClicker::hover_screen(0, 5, 5), Err(CoreError::WindowGone)));
    }

    #[test]
    fn send_key_returns_error_when_hwnd_is_zero() {
        assert!(matches!(BackgroundClicker::send_key(0, 0x0D), Err(CoreError::WindowGone)));
    }

    #[test]
    fn send_char_returns_error_when_hwnd_is_zero() {
        assert!(matches!(BackgroundClicker::send_char(0, 'a'), Err(CoreError::WindowGone)));
    }

    #[test]
    fn focused_control_of_zero_hwnd_returns_zero() {
        // GetWindowThreadProcessId(NULL) == 0 -> falls back to the input handle (0).
        assert_eq!(BackgroundClicker::focused_control(0), 0);
    }
}
