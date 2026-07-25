//! Process DPI awareness.
//!
//! Ghost clicks at coordinates taken from UIA bounding rectangles, which are
//! always PHYSICAL pixels. A process that is not DPI-aware is lied to by
//! Windows: it sees a virtualised, scaled coordinate space, so on any display
//! above 100% scaling every computed click lands off-target. The bug is
//! invisible on a 100% display, which is why it survives development and
//! surfaces only on a user's laptop.
//!
//! Must be called before any window or DC is touched - awareness cannot be
//! changed once the process has started using them.

use std::sync::Once;

static INIT: Once = Once::new();

/// Opt this process into per-monitor DPI awareness (v2). Idempotent and safe to
/// call from every binary's `main`. Silently does nothing if the OS refuses,
/// which only happens when awareness was already set, e.g. by a manifest.
pub fn ensure_process_dpi_aware() {
    INIT.call_once(|| {
        #[cfg(windows)]
        unsafe {
            use windows::Win32::UI::HiDpi::{
                SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            };
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    });
}

/// True when this process sees real physical pixels.
#[cfg(windows)]
pub fn is_process_dpi_aware() -> bool {
    unsafe {
        use windows::Win32::UI::HiDpi::{
            GetAwarenessFromDpiAwarenessContext, GetThreadDpiAwarenessContext,
            DPI_AWARENESS_UNAWARE,
        };
        GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext()) != DPI_AWARENESS_UNAWARE
    }
}

#[cfg(not(windows))]
pub fn is_process_dpi_aware() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_is_idempotent_and_makes_the_process_aware() {
        ensure_process_dpi_aware();
        ensure_process_dpi_aware();
        #[cfg(windows)]
        assert!(
            is_process_dpi_aware(),
            "after ensure_process_dpi_aware the process must report DPI awareness; \
             without it every click above 100% scaling lands off-target"
        );
    }
}
