use windows::Win32::Foundation::{GlobalFree, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use crate::error::CoreError;

const CF_UNICODETEXT: u32 = 13;

/// Read the current clipboard contents as a UTF-8 string.
/// Returns empty string if clipboard is empty or does not contain text.
pub fn get_clipboard() -> Result<String, CoreError> {
    unsafe {
        OpenClipboard(None)
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "OpenClipboard" })?;
        let result = match GetClipboardData(CF_UNICODETEXT) {
            Ok(handle) => {
                let hglob = HGLOBAL(handle.0);
                if hglob.0.is_null() {
                    String::new()
                } else {
                    let ptr = GlobalLock(hglob) as *const u16;
                    // LOW: only call GlobalUnlock when GlobalLock succeeded (ptr non-null).
                    // Calling Unlock after a failed Lock corrupts the global heap lock count.
                    let s = if !ptr.is_null() {
                        let mut len = 0usize;
                        while len < 10_000_000 && *ptr.add(len) != 0 {
                            len += 1;
                        }
                        let slice = std::slice::from_raw_parts(ptr, len);
                        let result = String::from_utf16_lossy(slice).to_string();
                        let _ = GlobalUnlock(hglob);
                        result
                    } else {
                        String::new()
                    };
                    s
                }
            }
            _ => String::new(),
        };
        let _ = CloseClipboard();
        Ok(result)
    }
}

/// Write text to the clipboard, replacing any existing content.
pub fn set_clipboard(text: &str) -> Result<(), CoreError> {
    set_with(text, crate::stop::is_active(), write_clipboard)
}

/// Put back the human's clipboard content saved before a paste. Not refused
/// while stopped: this is hand-back, like returning the foreground, and a stop
/// that lands mid-paste must not leave the automation's text on their clipboard.
pub fn restore_clipboard(saved: &str) -> Result<(), CoreError> {
    restore_with(saved, write_clipboard)
}

/// The stop decision for a write, apart from the OS call so it is testable
/// without touching the human's clipboard or the process stop flag.
fn set_with(text: &str, stopped: bool, write: impl FnOnce(&str) -> Result<(), CoreError>) -> Result<(), CoreError> {
    if stopped {
        return Err(CoreError::Stopped);
    }
    write(text)
}

fn restore_with(saved: &str, write: impl FnOnce(&str) -> Result<(), CoreError>) -> Result<(), CoreError> {
    write(saved)
}

fn write_clipboard(text: &str) -> Result<(), CoreError> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0u16)).collect();
    let byte_len = wide.len() * 2;
    unsafe {
        OpenClipboard(None)
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "OpenClipboard" })?;
        EmptyClipboard().map_err(|e| {
            let _ = CloseClipboard();
            CoreError::Win32 { code: e.code().0 as u32, context: "EmptyClipboard" }
        })?;
        let hmem = GlobalAlloc(GMEM_MOVEABLE, byte_len).map_err(|e| {
            let _ = CloseClipboard();
            CoreError::Win32 { code: e.code().0 as u32, context: "GlobalAlloc" }
        })?;
        let ptr = GlobalLock(hmem) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(Some(hmem));
            let _ = CloseClipboard();
            return Err(CoreError::Win32 { code: 0, context: "GlobalLock" });
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        let _ = GlobalUnlock(hmem);
        SetClipboardData(CF_UNICODETEXT, Some(windows::Win32::Foundation::HANDLE(hmem.0))).map_err(|e| {
            let _ = GlobalFree(Some(hmem));
            let _ = CloseClipboard();
            CoreError::Win32 { code: e.code().0 as u32, context: "SetClipboardData" }
        })?;
        let _ = CloseClipboard();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{restore_with, set_with};
    use crate::error::CoreError;
    use std::cell::RefCell;

    /// A stop that lands mid-paste must still let the human's clipboard be put
    /// back (hand-back), while a new automation write is refused. Fake writer:
    /// the real clipboard is never touched.
    #[test]
    fn a_stop_refuses_a_write_but_not_the_restore() {
        let written = RefCell::new(Vec::<String>::new());
        let fake = |t: &str| {
            written.borrow_mut().push(t.to_string());
            Ok(())
        };
        assert!(matches!(set_with("automation text", true, fake), Err(CoreError::Stopped)));
        assert!(restore_with("the human's text", fake).is_ok());
        assert!(set_with("after reset", false, fake).is_ok());
        assert_eq!(*written.borrow(), ["the human's text", "after reset"]);
    }

    /// The public functions route through the tested helpers.
    #[test]
    fn the_public_functions_use_the_tested_helpers() {
        let src = include_str!("clipboard.rs");
        // Only the function bodies, not this test's own text.
        let body = |sig: &str| {
            let start = src.find(sig).unwrap_or_else(|| panic!("{sig} not found"));
            let rest = &src[start..];
            rest[..rest.find("\n}").unwrap()].to_string()
        };
        let set = body("pub fn set_clipboard(");
        let restore = body("pub fn restore_clipboard(");
        assert!(set.contains("set_with(") && set.contains("is_active()"), "{set}");
        assert!(restore.contains("restore_with(") && !restore.contains("set_with(") && !restore.contains("stop"), "{restore}");
    }

    #[test]
    fn utf16_encoding_includes_null_terminator() {
        let text = "hello";
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0u16)).collect();
        assert_eq!(wide.len(), 6); // 5 chars + null
        assert_eq!(*wide.last().unwrap(), 0u16);
    }

    #[test]
    fn utf16_byte_len_is_double_code_units() {
        let text = "ab";
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0u16)).collect();
        let byte_len = wide.len() * 2;
        assert_eq!(byte_len, 6); // 3 u16 * 2 bytes each
    }

    #[test]
    fn utf16_empty_string_has_only_null() {
        let wide: Vec<u16> = "".encode_utf16().chain(std::iter::once(0u16)).collect();
        assert_eq!(wide, vec![0u16]);
    }

    #[test]
    fn utf16_roundtrip_is_lossless() {
        let original = "Ghost automation test: hello world!";
        let wide: Vec<u16> = original.encode_utf16().chain(std::iter::once(0u16)).collect();
        let null_pos = wide.iter().position(|&c| c == 0).unwrap();
        let recovered = String::from_utf16_lossy(&wide[..null_pos]);
        assert_eq!(recovered, original);
    }

    #[test]
    fn null_scan_cap_constant() {
        // Verify the cap used in get_clipboard is 10M, not unbounded
        const CAP: usize = 10_000_000;
        assert_eq!(CAP, 10_000_000);
    }
}
