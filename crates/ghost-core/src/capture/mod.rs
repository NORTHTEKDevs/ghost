pub mod idle;
pub mod screen;
pub mod window;
pub use idle::IdleDetector;
pub use screen::capture_screen;
pub use window::{capture_window, is_capturable};
