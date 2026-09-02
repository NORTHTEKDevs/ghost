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
use windows::core::Interface;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{CUIAutomation8, IUIAutomation, IUIAutomation2};

/// Default per-call deadlines for cross-process UIA work, in milliseconds.
///
/// Every walker call blocks on the target app's UI thread. When that thread is
/// busy serving the human, one call can block for as long as the app likes and a
/// client-side timeout cannot cancel it (the same bug class as an outer timeout
/// around an uncancellable blocking closure). `IUIAutomation2` is where the real
/// deadlines live: the connection timeout bounds how long UIA waits for a provider
/// to answer at all, the transaction timeout bounds a single request. Both are
/// overridable through `GHOST_UIA_CONNECTION_TIMEOUT_MS` /
/// `GHOST_UIA_TRANSACTION_TIMEOUT_MS`.
const DEFAULT_CONNECTION_TIMEOUT_MS: u32 = 3_000;
const DEFAULT_TRANSACTION_TIMEOUT_MS: u32 = 5_000;
/// Keep an explicit floor so an env override cannot turn every call into an
/// instant failure.
const MIN_TIMEOUT_MS: u32 = 250;

fn parse_timeout(raw: Option<&str>, default: u32) -> u32 {
    raw.and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(default)
        .max(MIN_TIMEOUT_MS)
}

fn timeout_from_env(var: &str, default: u32) -> u32 {
    parse_timeout(std::env::var(var).ok().as_deref(), default)
}

/// The connection deadline applied to every automation object Ghost creates.
pub fn uia_connection_timeout_ms() -> u32 {
    timeout_from_env("GHOST_UIA_CONNECTION_TIMEOUT_MS", DEFAULT_CONNECTION_TIMEOUT_MS)
}

/// The per-request deadline applied to every automation object Ghost creates.
pub fn uia_transaction_timeout_ms() -> u32 {
    timeout_from_env("GHOST_UIA_TRANSACTION_TIMEOUT_MS", DEFAULT_TRANSACTION_TIMEOUT_MS)
}

/// Create the UIA client object with deadlines set.
///
/// This is the ONE constructor for `IUIAutomation` in the workspace: the main
/// session tree, the STA pool workers and the hidden-desktop trees all go through
/// it, so no automation object can exist without a deadline. `CUIAutomation8`
/// already implements `IUIAutomation2`; the earlier code asked for the plain
/// `IUIAutomation` interface and so never reached the timeout setters.
///
/// The calling thread must already be in a COM apartment.
pub fn create_automation() -> Result<IUIAutomation, CoreError> {
    unsafe {
        let a2: IUIAutomation2 = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| CoreError::ComInit(format!("CoCreateInstance(CUIAutomation8): {e}")))?;
        // A failure to set a deadline is worth knowing about, but the object is
        // still usable - keep the old (deadline-less) behaviour rather than
        // refusing to boot.
        if let Err(e) = a2.SetConnectionTimeout(uia_connection_timeout_ms()) {
            tracing::warn!("UIA SetConnectionTimeout failed: {e}");
        }
        if let Err(e) = a2.SetTransactionTimeout(uia_transaction_timeout_ms()) {
            tracing::warn!("UIA SetTransactionTimeout failed: {e}");
        }
        a2.cast::<IUIAutomation>()
            .map_err(|e| CoreError::ComInit(format!("IUIAutomation2 -> IUIAutomation: {e}")))
    }
}

/// Read the deadlines back from an automation object (test and doctor support).
pub fn automation_timeouts(a: &IUIAutomation) -> Result<(u32, u32), CoreError> {
    unsafe {
        let a2: IUIAutomation2 = a
            .cast()
            .map_err(|e| CoreError::ComInit(format!("IUIAutomation -> IUIAutomation2: {e}")))?;
        let conn = a2
            .ConnectionTimeout()
            .map_err(|e| CoreError::ComInit(format!("ConnectionTimeout: {e}")))?;
        let txn = a2
            .TransactionTimeout()
            .map_err(|e| CoreError::ComInit(format!("TransactionTimeout: {e}")))?;
        Ok((conn, txn))
    }
}

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

#[cfg(test)]
mod deadline_tests {
    use super::*;

    #[test]
    fn timeout_parsing_defaults_floors_and_accepts_overrides() {
        assert_eq!(parse_timeout(None, 3_000), 3_000);
        assert_eq!(parse_timeout(Some("garbage"), 3_000), 3_000);
        assert_eq!(parse_timeout(Some(" 7000 "), 3_000), 7_000);
        // An override cannot make every call fail instantly.
        assert_eq!(parse_timeout(Some("1"), 3_000), MIN_TIMEOUT_MS);
        assert_eq!(parse_timeout(Some("0"), 3_000), MIN_TIMEOUT_MS);
    }

    /// The whole point of `create_automation`: the object Ghost hands out is an
    /// `IUIAutomation2` with deadlines set. Reads them back through COM, so the
    /// test fails if the cast or the setters ever silently stop working.
    #[test]
    fn automation_objects_carry_deadlines() {
        let _guard = init_com().expect("COM init");
        let automation = create_automation().expect("create_automation");
        let (conn, txn) = automation_timeouts(&automation).expect("read timeouts");
        assert_eq!(conn, uia_connection_timeout_ms());
        assert_eq!(txn, uia_transaction_timeout_ms());
        assert!(conn >= MIN_TIMEOUT_MS && txn >= MIN_TIMEOUT_MS);
        drop(automation);
    }
}
