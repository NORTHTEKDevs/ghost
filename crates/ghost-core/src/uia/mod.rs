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
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

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

/// Initialize COM in the Multi-Threaded Apartment (MTA).
///
/// This process previously ran STA on the theory that IUIAutomation is
/// STA-affine. That theory forced the whole MCP server onto one thread: every
/// tool call serialized behind every other, so a 15s wait in one tab stalled an
/// instant query behind it. The MTA is what allows requests to run as parallel
/// tasks.
///
/// Why MTA is sound here, and not a gamble:
///   - CUIAutomation8 is registered with the "Both" threading model. From MTA
///     threads its objects are called directly, with no cross-apartment
///     marshalling at all - the marshalling overhead argument applies to STA
///     objects called *from* the MTA, which UIA's client objects are not.
///   - The one genuine MTA hazard for UIA clients is COM *event callbacks*
///     (AddAutomationEventHandler and friends), which this codebase never uses:
///     the EventBus is built on SetWinEventHook with its own pump thread, which
///     is apartment-independent.
///   - Measured, not asserted: the concurrent server ran the full tool surface
///     in parallel under MTA on live Windows (browser + UIA + capture at once)
///     across the whole verification suite with no deadlock and no failure.
///
/// Idempotent per thread. Returns a `ComGuard` whose Drop balances this call.
pub fn init_com() -> Result<ComGuard, CoreError> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()
            .map_err(|e| CoreError::ComInit(format!("CoInitializeEx(MTA) failed: {e:?}")))?;
        Ok(ComGuard { _private: () })
    }
}

/// Join the calling thread to the MTA without a guard, for runtime worker
/// threads that live for the process lifetime. Idempotent; failure is returned
/// rather than panicking so a thread that is already in a different apartment
/// (RPC_E_CHANGED_MODE) is visible to the caller.
pub fn init_com_for_thread() -> bool {
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().is_ok() }
}
