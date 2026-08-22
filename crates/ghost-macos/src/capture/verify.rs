//! Act-then-verify screen-delta math, and the raw capture it consumes.
//!
//! `Verification` and `compute_verification` do pure arithmetic over two
//! already-captured RGBA buffers -- no OS call anywhere in them -- so they
//! are ported verbatim from `ghost_core::capture::verify`, not stubbed.
//! `capture_region_raw` is the one function here that DOES need an OS call
//! (ScreenCaptureKit), so it honestly refuses; `compute_verification` still
//! works correctly on whatever pixels a caller hands it directly (useful for
//! testing the delta math independent of capture, exactly as ghost-core's own
//! test suite does).

use crate::error::CoreError;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Verification {
    pub changed: bool,
    pub delta_score: f32,
    pub foreground_ok: bool,
}

const VERIFY_GRID_DIM: usize = 32;
const CELL_TOLERANCE: u8 = 2;

/// Downsample an RGBA buffer to a `dim`x`dim` grid of per-cell channel
/// averages. Ported verbatim from `ghost_core::capture::idle::downsample_grid`
/// (pure math, no OS dependency).
fn downsample_grid(pixels: &[u8], width: usize, height: usize, dim: usize) -> Vec<u8> {
    let dim = dim.max(1);
    let mut out = vec![0u8; dim * dim * 4];
    if width == 0 || height == 0 || pixels.len() < width * height * 4 {
        return out;
    }
    let cell_w = (width / dim).max(1);
    let cell_h = (height / dim).max(1);
    for by in 0..dim {
        for bx in 0..dim {
            let mut rgba = [0u64; 4];
            let mut n: u64 = 0;
            for y in (by * cell_h)..(((by + 1) * cell_h).min(height)) {
                for x in (bx * cell_w)..(((bx + 1) * cell_w).min(width)) {
                    let idx = (y * width + x) * 4;
                    if idx + 3 < pixels.len() {
                        for c in 0..4 {
                            rgba[c] += pixels[idx + c] as u64;
                        }
                        n += 1;
                    }
                }
            }
            let dst = (by * dim + bx) * 4;
            for c in 0..4 {
                out[dst + c] = rgba[c].checked_div(n).unwrap_or(0) as u8;
            }
        }
    }
    out
}

/// Pure math over two already-captured RGBA buffers. See the module docs.
pub fn compute_verification(
    before_rgba: &[u8],
    after_rgba: &[u8],
    width: usize,
    height: usize,
    foreground_ok: bool,
) -> Verification {
    let before_ds = downsample_grid(before_rgba, width, height, VERIFY_GRID_DIM);
    let after_ds = downsample_grid(after_rgba, width, height, VERIFY_GRID_DIM);

    let total_cells = VERIFY_GRID_DIM * VERIFY_GRID_DIM;
    let mut changed_cells = 0usize;
    for cell in 0..total_cells {
        let base = cell * 4;
        let cell_changed = (0..4).any(|c| before_ds[base + c].abs_diff(after_ds[base + c]) > CELL_TOLERANCE);
        if cell_changed {
            changed_cells += 1;
        }
    }
    let delta_score = changed_cells as f32 / total_cells as f32;
    let changed = changed_cells > 0;

    Verification { changed, delta_score, foreground_ok }
}

/// Capture a raw RGBA snapshot of `rect` (or the full screen if `None`).
/// Needs ScreenCaptureKit -- out of scope here.
pub fn capture_region_raw(_rect: Option<(i32, i32, i32, i32)>) -> Result<(Vec<u8>, usize, usize), CoreError> {
    Err(CoreError::Unsupported { op: "capture_region_raw", needs: "ScreenCaptureKit" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_rgba(r: u8, g: u8, b: u8, a: u8, w: usize, h: usize) -> Vec<u8> {
        (0..w * h).flat_map(|_| [r, g, b, a]).collect()
    }

    #[test]
    fn identical_frames_produce_zero_delta() {
        let frame = solid_rgba(100, 150, 200, 255, 64, 64);
        let v = compute_verification(&frame, &frame, 64, 64, true);
        assert!(!v.changed);
        assert_eq!(v.delta_score, 0.0);
    }

    #[test]
    fn different_frames_produce_nonzero_delta() {
        let before = solid_rgba(0, 0, 0, 255, 64, 64);
        let after = solid_rgba(255, 255, 255, 255, 64, 64);
        let v = compute_verification(&before, &after, 64, 64, true);
        assert!(v.changed);
        assert!(v.delta_score > 0.0);
    }

    #[test]
    fn capture_region_raw_fails_honestly() {
        assert!(matches!(capture_region_raw(None), Err(CoreError::Unsupported { .. })));
    }

    #[test]
    fn verification_serializes_to_json() {
        let v = Verification { changed: true, delta_score: 0.5, foreground_ok: false };
        let s = serde_json::to_string(&v).unwrap_or_default();
        assert!(s.contains("\"changed\":true"));
    }
}
