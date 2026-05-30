pub mod idle;
pub mod screen;
pub mod verify;
pub use idle::IdleDetector;
pub use screen::{capture_screen, capture_screen_full_rgba, capture_screen_region, capture_screen_downsample_raw, CaptureFormat};
pub use verify::{Verification, compute_verification, capture_region_raw};
