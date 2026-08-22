pub mod idle;
pub mod marks;
pub mod screen;
pub mod verify;

pub use idle::IdleDetector;
pub use marks::Mark;
pub use screen::{
    capture_region_marked_jpeg, capture_screen, capture_screen_region, capture_window_printwindow,
    CaptureFormat,
};
pub use verify::{capture_region_raw, compute_verification, Verification};
