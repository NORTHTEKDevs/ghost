pub mod idle;
pub mod screen;
pub use idle::IdleDetector;
pub use screen::{capture_screen, capture_screen_region, CaptureFormat};
