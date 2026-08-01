//! Keyboard, mirroring `ghost_core::input::keyboard`.
//!
//! Re-exports the shared implementations from the parent module so shared code
//! can use either `input::type_text` or `input::keyboard::type_text`, exactly as
//! it can on Windows.

pub use crate::keysym::{name_to_vk, VirtualKey};

pub use super::{clear_focused_field, key_down, key_up, press_key, type_text};
