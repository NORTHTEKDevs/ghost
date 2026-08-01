//! Act-then-verify screen delta.
//!
//! Mirrors the Windows engine's approach and its grid resolution. The 32x32 grid
//! matters: a coarser grid averages a single typed word away and, because it
//! effectively hashes the grid, collapses `delta_score` to a binary 0 or 1.

/// Identical in shape to `ghost_core::capture::Verification`.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Verification {
    /// True if the screen changed detectably after the action.
    pub changed: bool,
    /// 0.0 (identical) to 1.0 (completely different).
    pub delta_score: f32,
    /// True if the foreground window is the intended target window.
    pub foreground_ok: bool,
}

const VERIFY_GRID_DIM: usize = 32;

/// Per-cell mean-luma difference (0-255) above which a cell counts as changed.
/// Low enough to catch one word of text, high enough to ignore antialiasing and
/// cursor blink.
const CELL_DELTA_THRESHOLD: f32 = 6.0;

/// How many grid cells must change before the screen counts as changed.
///
/// This is a **cell count, not a fraction**, and it is 1 on purpose. With a
/// 32x32 grid there are 1024 cells, so a single changed cell is a fraction of
/// 0.00098 -- any fractional threshold above that silently discards exactly the
/// small, local change the fine grid exists to detect (one typed word in a large
/// window). The per-cell luma threshold above is what rejects noise; the cell
/// count must not also gate it.
const MIN_CHANGED_CELLS: usize = 1;

/// Reduce an RGBA buffer to a 32x32 grid of mean luma values.
///
/// Returns `None` for an empty or malformed buffer rather than panicking: a
/// failed capture must degrade to "cannot verify", never crash the server.
pub fn luma_grid(rgba: &[u8], width: usize, height: usize) -> Option<Vec<f32>> {
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return None;
    }
    let mut sums = vec![0f64; VERIFY_GRID_DIM * VERIFY_GRID_DIM];
    let mut counts = vec![0u32; VERIFY_GRID_DIM * VERIFY_GRID_DIM];

    for y in 0..height {
        let gy = y * VERIFY_GRID_DIM / height;
        for x in 0..width {
            let gx = x * VERIFY_GRID_DIM / width;
            let i = (y * width + x) * 4;
            let r = rgba[i] as f64;
            let g = rgba[i + 1] as f64;
            let b = rgba[i + 2] as f64;
            // Rec. 601 luma - cheap and adequate for change detection.
            let luma = 0.299 * r + 0.587 * g + 0.114 * b;
            let cell = gy * VERIFY_GRID_DIM + gx;
            sums[cell] += luma;
            counts[cell] += 1;
        }
    }

    Some(
        sums.iter()
            .zip(counts.iter())
            .map(|(s, c)| if *c == 0 { 0.0 } else { (*s / *c as f64) as f32 })
            .collect(),
    )
}

/// Number of grid cells whose mean luma moved beyond the per-cell threshold.
pub fn changed_cells(before: &[f32], after: &[f32]) -> usize {
    if before.len() != after.len() {
        return 0;
    }
    before
        .iter()
        .zip(after.iter())
        .filter(|(a, b)| (*a - *b).abs() > CELL_DELTA_THRESHOLD)
        .count()
}

/// Compare two luma grids into a 0.0-1.0 delta score.
pub fn grid_delta(before: &[f32], after: &[f32]) -> f32 {
    if before.len() != after.len() || before.is_empty() {
        return 0.0;
    }
    changed_cells(before, after) as f32 / before.len() as f32
}

/// Build a [`Verification`] from before/after captures.
pub fn compute_verification(
    before: &[u8],
    after: &[u8],
    width: usize,
    height: usize,
    foreground_ok: bool,
) -> Verification {
    let (Some(b), Some(a)) = (luma_grid(before, width, height), luma_grid(after, width, height))
    else {
        // Could not compare - report "not changed" but do not claim success.
        return Verification { changed: false, delta_score: 0.0, foreground_ok };
    };
    Verification {
        changed: changed_cells(&b, &a) >= MIN_CHANGED_CELLS,
        delta_score: grid_delta(&b, &a),
        foreground_ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, v: u8) -> Vec<u8> {
        vec![v; w * h * 4]
    }

    #[test]
    fn identical_frames_report_no_change() {
        let a = solid(64, 64, 10);
        let v = compute_verification(&a, &a, 64, 64, true);
        assert!(!v.changed);
        assert_eq!(v.delta_score, 0.0);
    }

    #[test]
    fn a_fully_different_frame_scores_one() {
        let a = solid(64, 64, 0);
        let b = solid(64, 64, 255);
        let v = compute_verification(&a, &b, 64, 64, true);
        assert!(v.changed);
        assert_eq!(v.delta_score, 1.0);
    }

    #[test]
    fn a_small_local_change_is_still_detected() {
        // The reason the grid is 32x32 rather than 4x4: one word of text must
        // not be averaged into invisibility.
        let a = solid(64, 64, 0);
        let mut b = a.clone();
        for y in 0..2 {
            for x in 0..2 {
                let i = (y * 64 + x) * 4;
                b[i] = 255;
                b[i + 1] = 255;
                b[i + 2] = 255;
            }
        }
        let v = compute_verification(&a, &b, 64, 64, true);
        assert!(v.changed, "a 2x2 pixel change must register");
        assert!(v.delta_score > 0.0 && v.delta_score < 0.05);
    }

    #[test]
    fn tiny_uniform_noise_does_not_count_as_change() {
        let a = solid(64, 64, 100);
        let b = solid(64, 64, 102); // below CELL_DELTA_THRESHOLD
        let v = compute_verification(&a, &b, 64, 64, true);
        assert!(!v.changed);
    }

    #[test]
    fn a_malformed_buffer_degrades_instead_of_panicking() {
        let v = compute_verification(&[], &[], 64, 64, false);
        assert!(!v.changed);
        assert!(!v.foreground_ok);
    }

    #[test]
    fn zero_sized_capture_is_handled() {
        assert!(luma_grid(&[], 0, 0).is_none());
    }

    #[test]
    fn foreground_flag_is_passed_through_untouched() {
        let a = solid(8, 8, 1);
        assert!(compute_verification(&a, &a, 8, 8, true).foreground_ok);
        assert!(!compute_verification(&a, &a, 8, 8, false).foreground_ok);
    }
}
