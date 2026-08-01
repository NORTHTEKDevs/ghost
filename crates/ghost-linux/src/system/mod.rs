pub mod clipboard;
pub mod dpi;
pub mod window;

pub use clipboard::{get_clipboard, set_clipboard};
pub use window::{cursor_pos, foreground_window, foreground_window_rect, window_rect};
