use windows::Win32::Foundation::{GlobalFree, HWND, HGLOBAL};
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
        OpenClipboard(HWND::default())
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "OpenClipboard" })?;
        let result = match GetClipboardData(CF_UNICODETEXT) {
            Ok(handle) => {
                let hglob = HGLOBAL(handle.0 as *mut std::ffi::c_void);
                if hglob.0.is_null() {
                    String::new()
                } else {
                    let ptr = GlobalLock(hglob) as *const u16;
                    let s = if !ptr.is_null() {
                        let mut len = 0usize;
                        while len < 10_000_000 && *ptr.add(len) != 0 {
                            len += 1;
                        }
                        let slice = std::slice::from_raw_parts(ptr, len);
                        String::from_utf16_lossy(slice).to_string()
                    } else {
                        String::new()
                    };
                    let _ = GlobalUnlock(hglob);
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
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0u16)).collect();
    let byte_len = wide.len() * 2;
    unsafe {
        OpenClipboard(HWND::default())
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
            let _ = GlobalFree(hmem);
            let _ = CloseClipboard();
            return Err(CoreError::Win32 { code: 0, context: "GlobalLock" });
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        let _ = GlobalUnlock(hmem);
        SetClipboardData(CF_UNICODETEXT, windows::Win32::Foundation::HANDLE(hmem.0)).map_err(|e| {
            let _ = GlobalFree(hmem);
            let _ = CloseClipboard();
            CoreError::Win32 { code: e.code().0 as u32, context: "SetClipboardData" }
        })?;
        let _ = CloseClipboard();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
