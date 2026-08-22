pub mod element;
pub mod event_bus;
pub mod patterns;
pub mod tree;

pub use element::{BoundingRect, ElementDescriptor, UiaElement, INTERACTIVE_ROLES};
pub use event_bus::EventBus;
pub use tree::{focus_window, list_windows, set_window_state, UiaTree, WindowInfo, WindowState};

/// Mirrors `ghost_core::uia::ComGuard`. Windows uses this to balance
/// `CoInitializeEx`/`CoUninitialize`; macOS has no COM apartment model, so
/// this is an inert placeholder that exists purely so `GhostSession` can hold
/// the same field shape on every platform.
pub struct ComGuard {
    _private: (),
}

impl Drop for ComGuard {
    fn drop(&mut self) {}
}

/// Mirrors `ghost_core::uia::init_com`. Always succeeds -- there is no COM
/// apartment to join on macOS, so there is nothing that can fail. See
/// `input::hotkey::register_emergency_stop` for why `GhostSession::new()`
/// must not fail on setup steps that have no bearing on browser/shell.
pub fn init_com() -> Result<ComGuard, crate::error::CoreError> {
    Ok(ComGuard { _private: () })
}
