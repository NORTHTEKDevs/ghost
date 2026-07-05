pub mod idle;
pub mod marks;
pub mod screen;
pub mod verify;
pub use idle::IdleDetector;
pub use marks::{Mark, draw_marks};
pub use screen::{capture_screen, capture_screen_full_rgba, capture_screen_region, capture_screen_region_fast, capture_screen_downsample_raw, capture_region_marked_jpeg, CaptureFormat};
pub use verify::{Verification, compute_verification, capture_region_raw};
