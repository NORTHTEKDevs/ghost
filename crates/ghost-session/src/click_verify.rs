//! Did a background click have an effect?
//!
//! A click's effect is usually NOT in the control's own pixels: a button looks
//! the same after it is pressed, and what it did shows up in the window title,
//! in the element's own state, or elsewhere in the window (a status line, a
//! pane, a tab). Until 2026-11-25 the user-desktop path compared only the
//! control's own rect and the hidden-desktop path took one whole-window diff
//! after a fixed 80 ms sleep; they reported `verified=false` for 40/40 and 20/40
//! real clicks (docs/benches/2026-11-25-baseline). Both paths now poll at
//! [`POLL_AT_MS`] and stop at the first signal that moved.

/// When the observations are due, in ms after the click was dispatched. 80 ms
/// is the old fixed wait, so a click with no visible effect costs what it did,
/// and one with an effect returns at the first change. These are deadlines from
/// one instant, not sleeps between polls: Windows rounds every sleep up to its
/// ~15.6 ms timer tick, and four relative sleeps of 10/10/20/40 ms measured
/// 112 ms in total, 30 ms more than the old single sleep (stage-2 timings).
pub const POLL_AT_MS: [u64; 4] = [10, 20, 40, 80];

/// The window title is a local read and is checked on every poll. The
/// element's UIA state (2-3 cross-process calls) and the pixels (PrintWindow
/// captures, 15-25 ms each) are read once, at 80 ms, when the old code compared
/// pixels: polling them every time made a no-effect click 30-60 ms slower
/// (measured in docs/benches/2026-11-25-stage-2).
pub fn slow_signals_due(poll: usize) -> bool {
    poll + 1 == POLL_AT_MS.len()
}

/// Which observation showed the click took effect. Reported as `verified_by`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// The target window's title changed.
    Title,
    /// The clicked element's UIA name or value changed.
    Uia,
    /// The window's pixels changed, and stayed changed (see [`persisted`]).
    Pixels,
}

impl Signal {
    pub fn as_str(self) -> &'static str {
        match self {
            Signal::Title => "title",
            Signal::Uia => "uia",
            Signal::Pixels => "pixels",
        }
    }
}

/// One observation against the state captured before the click.
/// `pixels` is `None` when the surface could not be compared.
pub fn decide(title_changed: bool, uia_changed: bool, pixels: Option<bool>) -> (Option<bool>, Option<Signal>) {
    if title_changed {
        return (Some(true), Some(Signal::Title));
    }
    if uia_changed {
        return (Some(true), Some(Signal::Uia));
    }
    match pixels {
        Some(true) => (Some(true), Some(Signal::Pixels)),
        Some(false) => (Some(false), None),
        // Unchanged title and state prove nothing on their own; without a
        // pixel comparison the click stays unverifiable, as it always was.
        None => (None, None),
    }
}

/// Cells the window is divided into when locating a change (32x32).
const GRID: usize = 32;
/// A pixel has changed when a channel moved by more than this. PrintWindow
/// renders unchanged content bit-identically, so this only has to absorb
/// blending, not capture noise.
const PIXEL_TOLERANCE: u8 = 16;

/// Which grid cells contain at least one changed pixel between two same-size
/// RGBA frames. Per pixel, not per cell average: averaging a 16x8 cell of a
/// 520x260 window hid one digit of a status line changing ("noted 2" to
/// "noted 3"), and the cells only grow with the window.
pub fn changed_cells(before: &[u8], after: &[u8], width: usize, height: usize) -> Vec<bool> {
    let mut cells = vec![false; GRID * GRID];
    if width == 0 || height == 0 {
        return cells;
    }
    for (i, (b, a)) in before.chunks_exact(4).zip(after.chunks_exact(4)).enumerate() {
        if b.iter().zip(a).any(|(b, a)| b.abs_diff(*a) > PIXEL_TOLERANCE) {
            let (x, y) = (i % width, i / width);
            if y < height {
                cells[(y * GRID / height) * GRID + x * GRID / width] = true;
            }
        }
    }
    cells
}

/// A change counts only when the same cell differs from the pre-click frame in
/// two captures [`PERSIST_GAP_MS`] apart. An idle window is sometimes caught
/// mid-repaint: one ~10-25 ms frame that differs, then reverts
/// (idle-noise-hidden*.txt), and a whole-window diff that took that frame for
/// the click's effect reported `verified=true` for a click that did nothing.
pub fn persisted(first: &[bool], second: &[bool]) -> bool {
    first.iter().zip(second).any(|(a, b)| *a && *b)
}

/// Gap between the capture that saw a change and the one that confirms it.
/// Back-to-back captures (~10 ms apart) still let one long repaint span both:
/// 1 false `verified=true` in 80 no-effect hidden-desktop clicks.
pub const PERSIST_GAP_MS: u64 = 20;

/// First half of [`pixel_effect`]: `None` when the surface cannot be compared,
/// otherwise the cells that differ from `before` right now.
fn cells_now(hwnd: isize, before: &(Vec<u8>, usize, usize)) -> Option<Vec<bool>> {
    let after = crate::engine::capture::capture_window_printwindow(hwnd).ok();
    Some(changed_cells(&before.0, comparable(before, &after)?, before.1, before.2))
}

/// A capture that can be compared against `before`: same size, not blank.
fn comparable<'a>(before: &(Vec<u8>, usize, usize), after: &'a Option<(Vec<u8>, usize, usize)>) -> Option<&'a [u8]> {
    let (b, bw, bh) = before;
    let (a, aw, ah) = after.as_ref()?;
    if bw != aw || bh != ah || b.is_empty() || b.iter().all(|&p| p == 0) || a.iter().all(|&p| p == 0) {
        return None;
    }
    Some(a)
}

/// Whole-window pixel effect of a click: `Some(true)` when part of the window
/// changed against `before` and was still changed [`PERSIST_GAP_MS`] later,
/// `None` when the surface cannot be compared. The confirming capture is taken
/// only when the first shows a change, so a click with no effect pays for one
/// capture, as before. Blocks; for the hidden desktop's worker thread.
pub fn pixel_effect(hwnd: isize, before: &Option<(Vec<u8>, usize, usize)>) -> Option<bool> {
    let before = before.as_ref()?;
    let first = cells_now(hwnd, before)?;
    if !first.contains(&true) {
        return Some(false);
    }
    std::thread::sleep(std::time::Duration::from_millis(PERSIST_GAP_MS));
    Some(persisted(&first, &cells_now(hwnd, before)?))
}

/// [`pixel_effect`] for async callers: the gap is awaited, not slept.
pub async fn pixel_effect_async(hwnd: isize, before: &Option<(Vec<u8>, usize, usize)>) -> Option<bool> {
    let before = before.as_ref()?;
    let first = cells_now(hwnd, before)?;
    if !first.contains(&true) {
        return Some(false);
    }
    tokio::time::sleep(std::time::Duration::from_millis(PERSIST_GAP_MS)).await;
    Some(persisted(&first, &cells_now(hwnd, before)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 64;
    const H: usize = 64;

    fn frame(fill: u8) -> Vec<u8> {
        vec![fill; W * H * 4]
    }

    /// `base` with a solid square painted at (x, y), side `n`.
    fn with_square(base: &[u8], x: usize, y: usize, n: usize, v: u8) -> Vec<u8> {
        let mut f = base.to_vec();
        for row in y..y + n {
            for col in x..x + n {
                let i = (row * W + col) * 4;
                f[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        f
    }

    #[test]
    fn a_title_change_verifies_even_when_pixels_did_not_move() {
        assert_eq!(decide(true, false, Some(false)), (Some(true), Some(Signal::Title)));
    }

    #[test]
    fn a_uia_state_change_verifies_even_without_a_pixel_comparison() {
        assert_eq!(decide(false, true, None), (Some(true), Some(Signal::Uia)));
    }

    #[test]
    fn pixels_alone_still_verify() {
        assert_eq!(decide(false, false, Some(true)), (Some(true), Some(Signal::Pixels)));
    }

    #[test]
    fn nothing_moved_is_unverified_not_unknown() {
        assert_eq!(decide(false, false, Some(false)), (Some(false), None));
    }

    #[test]
    fn nothing_comparable_stays_unknown() {
        assert_eq!(decide(false, false, None), (None, None));
    }

    #[test]
    fn title_outranks_the_other_signals_in_the_report() {
        assert_eq!(decide(true, true, Some(true)).1, Some(Signal::Title));
        assert_eq!(decide(false, true, Some(true)).1, Some(Signal::Uia));
    }

    #[test]
    fn polling_ends_at_the_old_fixed_settle_and_never_goes_back_in_time() {
        assert_eq!(POLL_AT_MS.last(), Some(&80));
        assert!(POLL_AT_MS.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn slow_signals_are_read_once_at_the_old_settle_point() {
        let due: Vec<bool> = (0..POLL_AT_MS.len()).map(slow_signals_due).collect();
        assert_eq!(due, [false, false, false, true]);
    }

    #[test]
    fn signal_names_are_the_wire_values() {
        let names: Vec<_> = [Signal::Title, Signal::Uia, Signal::Pixels].iter().map(|s| s.as_str()).collect();
        assert_eq!(names, ["title", "uia", "pixels"]);
    }

    #[test]
    fn identical_frames_change_no_cell() {
        assert!(!changed_cells(&frame(200), &frame(200), W, H).contains(&true));
    }

    #[test]
    fn a_change_far_from_the_clicked_control_is_seen() {
        let before = frame(200);
        let after = with_square(&before, 50, 50, 8, 0);
        let cells = changed_cells(&before, &after, W, H);
        assert!(cells.contains(&true));
        assert!(!cells[0], "the top-left cell did not change");
    }

    #[test]
    fn a_change_within_tolerance_is_noise() {
        assert!(!changed_cells(&frame(200), &frame(216), W, H).contains(&true));
        assert!(changed_cells(&frame(200), &frame(217), W, H).contains(&true));
    }

    #[test]
    fn two_changed_pixels_in_a_large_window_are_seen() {
        // 512x256: a 16x8 cell. Two pixels of a glyph changing would move that
        // cell's average by under 1 - invisible to a per-cell-average diff.
        let (w, h) = (512, 256);
        let before = vec![200u8; w * h * 4];
        let mut after = before.clone();
        for x in [300, 301] {
            let i = (200 * w + x) * 4;
            after[i..i + 3].copy_from_slice(&[40, 40, 40]);
        }
        let cells = changed_cells(&before, &after, w, h);
        assert_eq!(cells.iter().filter(|c| **c).count(), 1);
        assert!(cells[(200 * GRID / h) * GRID + 300 * GRID / w]);
    }

    #[test]
    fn a_one_frame_blip_does_not_persist() {
        let before = frame(200);
        let blip = changed_cells(&before, &with_square(&before, 4, 4, 6, 0), W, H);
        let reverted = changed_cells(&before, &before, W, H);
        assert!(!persisted(&blip, &reverted));
    }

    #[test]
    fn a_real_effect_persists() {
        let before = frame(200);
        let after = with_square(&before, 40, 10, 10, 0);
        let first = changed_cells(&before, &after, W, H);
        let second = changed_cells(&before, &after, W, H);
        assert!(persisted(&first, &second));
    }

    #[test]
    fn changes_in_different_places_do_not_add_up_to_an_effect() {
        let before = frame(200);
        let first = changed_cells(&before, &with_square(&before, 0, 0, 4, 0), W, H);
        let second = changed_cells(&before, &with_square(&before, 56, 56, 4, 0), W, H);
        assert!(!persisted(&first, &second));
    }
}
