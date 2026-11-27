pub mod hotkey;
pub mod keyboard;
pub mod mouse;
pub mod postmessage;

/// The typing read-back judge, one source shared with `ghost_core::input`.
#[path = "../../../ghost-core/src/input/readback.rs"]
pub mod readback;
pub use readback::typed_ok;

pub use postmessage::{BackgroundClicker, EditCommand};

pub use hotkey::{
    human_stop_active, is_stopped, register_emergency_stop, release_all_modifiers, reset_human_stop, reset_stop,
    trigger_human_stop, trigger_stop, STOP_FLAG,
};
pub use keyboard::{key_down, key_up, name_to_vk, press_key, type_text, VIRTUAL_KEY};
pub use mouse::{click, double_click, drag, hover, right_click, scroll};
