pub mod element;
pub mod patterns;
pub mod sta_pool;
pub mod tree;

pub use element::{BoundingRect, ElementDescriptor, UiaElement, INTERACTIVE_ROLES};
pub use tree::{UiaTree, WindowInfo, WindowState, list_windows, focus_window, set_window_state};

use crate::error::CoreError;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// Initialize COM in multithreaded apartment mode.
/// Must be called once per thread before using UIA.
pub fn init_com() -> Result<(), CoreError> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()
            .map_err(|e| CoreError::ComInit(format!("CoInitializeEx failed: {e:?}")))
    }
}
