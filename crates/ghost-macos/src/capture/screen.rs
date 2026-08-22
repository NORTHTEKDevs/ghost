//! Whole-frame/region capture and encoding.
//!
//! Every function that would actually grab pixels needs ScreenCaptureKit --
//! out of scope -- and honestly refuses. `crop_rgba` is pure buffer slicing
//! over an already-captured frame (no OS call), so it is ported verbatim;
//! it is what `ghost-session/src/session.rs` calls directly
//! (`crate::engine::capture::screen::crop_rgba`) to re-crop a cached frame on
//! a capture timeout -- unreachable here since captures never succeed, but
//! correct and tested on its own merits regardless.

use crate::error::CoreError;

/// Image output format for `capture_screen_region`. Pure data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFormat {
    Png,
    Jpeg(u8),
}

pub fn capture_screen() -> Result<Vec<u8>, CoreError> {
    Err(CoreError::Unsupported { op: "capture_screen", needs: "ScreenCaptureKit" })
}

pub fn capture_screen_region(
    _rect: Option<(i32, i32, i32, i32)>,
    _max_dim: Option<u32>,
    _format: CaptureFormat,
) -> Result<Vec<u8>, CoreError> {
    Err(CoreError::Unsupported { op: "capture_screen_region", needs: "ScreenCaptureKit" })
}

pub fn capture_window_printwindow(_hwnd_raw: isize) -> Result<(Vec<u8>, usize, usize), CoreError> {
    Err(CoreError::Unsupported { op: "capture_window_printwindow", needs: "ScreenCaptureKit (window capture)" })
}

pub fn capture_region_marked_jpeg(
    _rect: Option<(i32, i32, i32, i32)>,
    _marks_native: &[super::marks::Mark],
    _max_dim: u32,
    _quality: u8,
) -> Result<Vec<u8>, CoreError> {
    // ghost-core's version captures first, then draws marks, then encodes.
    // The capture step is the one that needs an OS call (ScreenCaptureKit),
    // so this fails at exactly that point -- the mark-drawing/JPEG-encoding
    // steps are unreachable either way and are not implemented here.
    Err(CoreError::Unsupported { op: "capture_region_marked_jpeg", needs: "ScreenCaptureKit" })
}

/// Crop an already-packed RGBA frame (`full_w` wide) to a sub-rect. Pure
/// buffer slicing -- ported verbatim from `ghost_core::capture::screen::crop_rgba`.
pub fn crop_rgba(full: &[u8], full_w: usize, l: usize, t: usize, cw: usize, ch: usize) -> Vec<u8> {
    let mut out = vec![0u8; cw * ch * 4];
    for y in 0..ch {
        let src = ((t + y) * full_w + l) * 4;
        let dst = y * cw * 4;
        if src + cw * 4 <= full.len() {
            out[dst..dst + cw * 4].copy_from_slice(&full[src..src + cw * 4]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_screen_fails_honestly() {
        assert!(matches!(capture_screen(), Err(CoreError::Unsupported { .. })));
    }

    #[test]
    fn crop_rgba_extracts_the_requested_subrect() {
        // 2x2 frame, each pixel a distinct color.
        let full: Vec<u8> = vec![
            1, 1, 1, 255, 2, 2, 2, 255, //
            3, 3, 3, 255, 4, 4, 4, 255,
        ];
        let cropped = crop_rgba(&full, 2, 1, 0, 1, 1);
        assert_eq!(cropped, vec![2, 2, 2, 255]);
    }
}
