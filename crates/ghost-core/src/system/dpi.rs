//! Process DPI awareness.
//!
//! Coordinates only line up if everyone agrees on what a pixel is. UI Automation
//! bounding rectangles, `GetWindowRect`, and DXGI screen captures are all in physical
//! pixels. A DPI-unaware process, however, is lied to by Windows: `GetSystemMetrics`
//! and `GetWindowRect` come back in virtualized logical pixels, scaled down by the
//! display's DPI factor.
//!
//! On a 150%-scaled display that mismatch puts every coordinate off by a third: a
//! click computed from a UIA rectangle lands somewhere else entirely, and a screen
//! crop of a window rectangle captures the wrong region. Declaring per-monitor
//! awareness up front makes every coordinate in the process physical and consistent.
//!
//! Must be called before the process creates any window, so `GhostSession::new` does
//! it first.

use std::sync::Once;

static INIT: Once = Once::new();

/// Declare this process per-monitor DPI aware (v2). Idempotent and safe to call from
/// anywhere; only the first call has any effect.
///
/// Failure is deliberately not an error: the call fails when awareness was already
/// set - by a manifest, by the host application, or by an earlier call - and in every
/// one of those cases the process already has an awareness mode and forcing a change
/// is neither possible nor desirable.
pub fn ensure_per_monitor_aware() -> bool {
    let mut applied = false;
    INIT.call_once(|| {
        applied = set_awareness();
    });
    applied || is_per_monitor_aware()
}

fn set_awareness() -> bool {
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_ok() }
}

/// Whether this process currently reports per-monitor-v2 DPI awareness.
pub fn is_per_monitor_aware() -> bool {
    use windows::Win32::UI::HiDpi::{
        AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext,
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    unsafe {
        AreDpiAwarenessContextsEqual(
            GetThreadDpiAwarenessContext(),
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
        .as_bool()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declaring_awareness_is_idempotent_and_leaves_the_process_aware() {
        // Two calls must not disagree, and must not panic on the second.
        let first = ensure_per_monitor_aware();
        let second = ensure_per_monitor_aware();
        assert_eq!(first, second);
        assert!(
            is_per_monitor_aware(),
            "coordinates from UIA and GetWindowRect only agree under per-monitor awareness"
        );
    }
}
