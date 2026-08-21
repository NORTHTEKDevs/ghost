pub mod dpi;
pub mod clipboard;
pub mod window;
pub mod lease;
pub mod observer;
pub use clipboard::{get_clipboard, set_clipboard};
pub use window::{cursor_pos, foreground_window, foreground_window_rect, window_rect};
pub use lease::{with_foreground_lease, ForegroundLease};
pub use observer::{DesktopDelta, DesktopSnapshot};
