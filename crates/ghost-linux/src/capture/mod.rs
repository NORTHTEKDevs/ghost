//! Capture and verification, mirroring `ghost_core::capture`.

// Pure image maths - compiles everywhere so its tests run on any host.
pub mod verify;
pub use verify::{compute_verification, Verification};

#[cfg(target_os = "linux")]
pub mod idle;
#[cfg(target_os = "linux")]
pub mod marks;
#[cfg(target_os = "linux")]
pub mod screen;

#[cfg(target_os = "linux")]
pub use idle::IdleDetector;
#[cfg(target_os = "linux")]
pub use marks::{draw_marks, Mark};
#[cfg(target_os = "linux")]
pub use screen::{
    capture_region_marked_jpeg, capture_region_raw, capture_screen, capture_screen_downsample_raw,
    capture_screen_full_rgba, capture_screen_region, capture_screen_region_fast,
    capture_window_encoded, capture_window_printwindow, crop_rgba, CaptureFormat,
};
