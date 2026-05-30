pub mod cached_walker;
pub mod element;
pub mod event_bus;
pub mod patterns;
pub mod sta_pool;
pub mod tree;

pub use element::{BoundingRect, ElementDescriptor, UiaElement, INTERACTIVE_ROLES};
pub use event_bus::EventBus;
pub use tree::{UiaTree, WindowInfo, WindowState, list_windows, focus_window, set_window_state};

use crate::error::CoreError;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

/// RAII guard that calls CoUninitialize on drop, balancing a successful CoInitializeEx call.
/// Store in GhostSession to tie COM lifetime to the session lifetime.
pub struct ComGuard {
    _private: (),
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

/// Initialize COM in Single-Threaded Apartment (STA) mode.
///
/// IUIAutomation is STA-affine — it internally uses hidden windows and message
/// pumps. Initializing as MTA causes cross-apartment marshaling overhead and
/// intermittent deadlocks. STA is correct because:
///   - All UIA calls are synchronous and on the same thread.
///   - GhostElement/UiaElement hold COM pointers that are !Send (documented),
///     so they never cross thread boundaries.
///   - tokio::main uses a multi-thread runtime but we never send COM objects
///     to worker threads; all UIA calls originate from the MCP main loop thread.
///
/// Must be called once per thread before using UIA.
/// Returns a `ComGuard` whose Drop calls CoUninitialize, balancing this call.
pub fn init_com() -> Result<ComGuard, CoreError> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()
            .map_err(|e| CoreError::ComInit(format!("CoInitializeEx(STA) failed: {e:?}")))?;
        Ok(ComGuard { _private: () })
    }
}
