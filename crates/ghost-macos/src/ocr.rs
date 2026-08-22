//! Local OCR text search.
//!
//! Windows uses `Windows.Media.Ocr` (WinRT, on-device). There is no
//! equivalent wired up here -- a native backend would use `Vision.framework`
//! (`VNRecognizeTextRequest`), also on-device and free, but still C/ObjC FFI
//! and out of scope. Also needs a real screen capture as its first step
//! regardless, which is unavailable for the same reason (`ScreenCaptureKit`).

use crate::error::CoreError;

pub fn find_text_local(_needle: &str, _region: Option<(i32, i32, i32, i32)>) -> Result<Option<(i32, i32)>, CoreError> {
    Err(CoreError::Unsupported { op: "find_text_local", needs: "Vision.framework (VNRecognizeTextRequest) + ScreenCaptureKit" })
}
