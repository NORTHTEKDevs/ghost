//! Background click via PostMessage - delivers WM_LBUTTONDOWN/UP without stealing foreground.

use crate::error::CoreError;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindow, PostMessageW, SendMessageTimeoutW, SEND_MESSAGE_TIMEOUT_FLAGS,
    WM_LBUTTONDOWN, WM_LBUTTONUP,
};

// Message constants not re-exported by the enabled `windows` features.
const BM_CLICK: u32 = 0x00F5;
const WM_SETTEXT: u32 = 0x000C;
const SMTO_ABORTIFHUNG: u32 = 0x0002;

fn hwnd_of(raw: isize) -> HWND {
    HWND(raw as *mut core::ffi::c_void)
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
}
