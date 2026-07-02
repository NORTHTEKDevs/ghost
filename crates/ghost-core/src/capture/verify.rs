//! Act-then-verify: screen-delta confirmation for mutating actions.
//!
//! Computes a perceptual hash delta between a BEFORE and AFTER frame of the
//! target region using the raw downsample path (no PNG encode). Returns
//! `Verification { changed, delta_score, foreground_ok }`.

use crate::error::CoreError;
use crate::capture::idle::downsample_grid;

/// Result of act-then-verify screen-delta check.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Verification {
    /// True if the screen changed detectably after the action.
    pub changed: bool,
    /// 0.0 (identical) to 1.0 (completely different) perceptual delta score.
    pub delta_score: f32,
    /// True if the foreground window is the intended target window.
    pub foreground_ok: bool,
}

/// Verification grid resolution. 32x32 cells sees a single typed word in a
/// full-size window (the old 4x4 grid averaged small text changes away and,
/// because it hashed the grid, produced a binary 0-or-1 delta_score).
const VERIFY_GRID_DIM: usize = 32;

/// Per-channel tolerance when comparing cell averages — absorbs capture noise
/// without masking real changes (a typed word shifts its cells by far more).
const CELL_TOLERANCE: u8 = 2;

/// Compute a perceptual verification by comparing a BEFORE and AFTER raw RGBA buffer.
///
/// Both buffers are downsampled to a 32x32 grid of per-cell channel averages,
/// then compared cell-by-cell with a small tolerance. `delta_score` = fraction
/// of cells that changed (0.0-1.0), a real perceptual distance.
///
/// This is the pure, COM-free computation used by tests. The session layer
/// calls this after capturing BEFORE and AFTER frames.
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
        let cell_changed = (0..4).any(|c| {
            before_ds[base + c].abs_diff(after_ds[base + c]) > CELL_TOLERANCE
        });
        if cell_changed {
            changed_cells += 1;
        }
    }
    let delta_score = changed_cells as f32 / total_cells as f32;
    let changed = changed_cells > 0;

    Verification { changed, delta_score, foreground_ok }
}

/// Capture a raw RGBA snapshot of the given rect (or full screen if None).
/// Returns (rgba_bytes, width, height).
pub fn capture_region_raw(
    rect: Option<(i32, i32, i32, i32)>,
) -> Result<(Vec<u8>, usize, usize), CoreError> {
    let (full_rgba, full_w, full_h) = super::screen::capture_screen_full_rgba()?;
    match rect {
        None => Ok((full_rgba, full_w, full_h)),
        Some((l, t, r, b)) => {
            let l = l.max(0) as usize;
            let t = t.max(0) as usize;
            let r = (r as usize).min(full_w);
            let b = (b as usize).min(full_h);
            if r <= l || b <= t {
                // MEDIUM-1: return Err instead of silently falling back to full-screen.
                // Callers in session.rs use .ok(), so this cleanly skips verification.
                return Err(CoreError::Win32 { code: 0, context: "capture_region_raw: degenerate rect after clamping" });
            }
            let cw = r - l;
            let ch = b - t;
            let mut crop = vec![0u8; cw * ch * 4];
            for y in 0..ch {
                let src_off = ((t + y) * full_w + l) * 4;
                let dst_off = y * cw * 4;
                crop[dst_off..dst_off + cw * 4]
                    .copy_from_slice(&full_rgba[src_off..src_off + cw * 4]);
            }
            Ok((crop, cw, ch))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_rgba(r: u8, g: u8, b: u8, a: u8, w: usize, h: usize) -> Vec<u8> {
        (0..w * h).flat_map(|_| [r, g, b, a]).collect()
    }

    /// Identical before/after should produce delta_score=0 and changed=false.
    #[test]
    fn identical_frames_produce_zero_delta() {
        let frame = solid_rgba(100, 150, 200, 255, 64, 64);
        let v = compute_verification(&frame, &frame, 64, 64, true);
        assert!(!v.changed, "identical frames must not be 'changed'");
        assert_eq!(v.delta_score, 0.0);
        assert!(v.foreground_ok);
    }

    /// Completely different frames should produce delta_score > 0 and changed=true.
    #[test]
    fn different_frames_produce_nonzero_delta() {
        let before = solid_rgba(0, 0, 0, 255, 64, 64);
        let after = solid_rgba(255, 255, 255, 255, 64, 64);
        let v = compute_verification(&before, &after, 64, 64, true);
        assert!(v.changed, "completely different frames must be 'changed'");
        assert!(v.delta_score > 0.0, "delta_score must be > 0 for different frames");
    }

    /// foreground_ok=false is passed through unchanged.
    #[test]
    fn foreground_ok_is_passed_through() {
        let frame = solid_rgba(0, 0, 0, 255, 4, 4);
        let v = compute_verification(&frame, &frame, 4, 4, false);
        assert!(!v.foreground_ok);
    }

    /// delta_score is bounded to [0.0, 1.0].
    #[test]
    fn delta_score_is_bounded() {
        let before = solid_rgba(0, 0, 0, 255, 16, 16);
        let after = solid_rgba(255, 255, 255, 255, 16, 16);
        let v = compute_verification(&before, &after, 16, 16, true);
        assert!(v.delta_score >= 0.0 && v.delta_score <= 1.0);
    }

    /// A small single-pixel change in a large uniform frame must register: with a
    /// 32x32 grid, one pixel of a 64x64 frame is 1/4 of its 2x2 cell — a 255-value
    /// flip shifts the cell average by ~64, far beyond tolerance.
    #[test]
    fn small_change_in_uniform_frame_is_detected() {
        let before = solid_rgba(128, 128, 128, 255, 64, 64);
        let mut after = before.clone();
        after[0] = 0; // change one pixel's red channel
        let v = compute_verification(&before, &after, 64, 64, true);
        assert!(v.changed, "single-pixel change must be detected (delta={})", v.delta_score);
        assert!(v.delta_score > 0.0 && v.delta_score < 0.05, "small change must have small delta, got {}", v.delta_score);
    }

    /// A typed word (small dark run on a light background) must be detected —
    /// this is the exact case the old 4x4 grid averaged away.
    #[test]
    fn typed_text_sized_change_is_detected() {
        let w = 1280usize;
        let h = 720usize;
        let before = solid_rgba(250, 250, 250, 255, w, h);
        let mut after = before.clone();
        // Simulate ~10 characters: an 80x14 dark strip near the top-left.
        for y in 100..114 {
            for x in 200..280 {
                let idx = (y * w + x) * 4;
                after[idx] = 20; after[idx + 1] = 20; after[idx + 2] = 20;
            }
        }
        let v = compute_verification(&before, &after, w, h, true);
        assert!(v.changed, "typed-text-sized change must be detected (delta={})", v.delta_score);
    }

    /// Capture noise (±1 per channel everywhere) must NOT count as a change.
    #[test]
    fn capture_noise_within_tolerance_is_ignored() {
        let before = solid_rgba(128, 128, 128, 255, 64, 64);
        let after = solid_rgba(129, 129, 129, 255, 64, 64);
        let v = compute_verification(&before, &after, 64, 64, true);
        assert!(!v.changed, "1-value global noise must be within tolerance (delta={})", v.delta_score);
    }

    /// Verification result serializes/deserializes as JSON.
    #[test]
    fn verification_serializes_to_json() {
        let v = Verification { changed: true, delta_score: 0.5, foreground_ok: false };
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains("\"changed\":true"));
        assert!(s.contains("\"delta_score\":0.5"));
        assert!(s.contains("\"foreground_ok\":false"));
    }
}
