//! Background click via PostMessage - delivers WM_LBUTTONDOWN/UP without stealing foreground.

use crate::error::CoreError;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindow, PostMessageW, WM_LBUTTONDOWN, WM_LBUTTONUP,
};

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_returns_error_when_hwnd_is_zero() {
        let err = BackgroundClicker::click(HWND(std::ptr::null_mut()), (10, 10));
        assert!(matches!(err, Err(CoreError::WindowGone)));
    }
}
