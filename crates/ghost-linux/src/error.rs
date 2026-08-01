//! Error type mirroring `ghost_core::error::CoreError`.
//!
//! The first eleven variants are **identical in name and shape** to the Windows
//! engine's, because `ghost-session` and `ghost-mcp` construct and match them
//! directly. Divergence here would mean the shared layers no longer compile on
//! both platforms, which is the whole point of this crate.
//!
//! The variants after them are Linux-only additions. Adding variants is safe:
//! shared code constructs specific ones and never matches exhaustively.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    /// Kept for shape-compatibility with the Windows engine. On Linux, prefer
    /// [`CoreError::Platform`] for free-form platform failures.
    #[error("Win32 error {code:#010x} in {context}")]
    Win32 { code: u32, context: &'static str },

    #[error("COM initialization failed: {0}")]
    ComInit(String),

    #[error("UIA not available for process: {process}")]
    UiaUnavailable { process: String },

    #[error("Process not found: {name}")]
    ProcessNotFound { name: String },

    #[error("STA worker panicked: {0}")]
    WorkerPanic(String),

    #[error("STA job exceeded timeout")]
    JobTimeout,

    #[error("STA pool circuit breaker open after repeated panics")]
    CircuitOpen,

    #[error("Target window is gone")]
    WindowGone,

    #[error("Window '{name}' is minimized; restore it first (ghost_window op=focus name={name})")]
    WindowMinimized { name: String },

    #[error("Could not confirm foreground for window: {window}")]
    FocusFailed { window: String },

    #[error("Element not actionable in background mode: {what}")]
    NotActionableInBackground { what: &'static str },

    // ---------------------------------------------------------------- Linux

    /// A platform call failed: D-Bus, AT-SPI, X11 or portal.
    #[error("platform error: {0}")]
    Platform(String),

    /// The capability does not exist on this session type. Returned instead of
    /// pretending -- e.g. per-window capture under native Wayland.
    #[error("unsupported on this session: {0}")]
    Unsupported(String),

    /// The element exists but exposes no way to perform the requested action.
    #[error("element not actionable: {0}")]
    NotActionable(String),

    /// No element matched the locator.
    #[error("element not found: {0}")]
    NotFound(String),
}

impl CoreError {
    pub fn platform(msg: impl std::fmt::Display) -> Self {
        CoreError::Platform(msg.to_string())
    }
    pub fn focus_failed(msg: impl std::fmt::Display) -> Self {
        CoreError::FocusFailed { window: msg.to_string() }
    }
}

#[cfg(target_os = "linux")]
impl From<zbus::Error> for CoreError {
    fn from(e: zbus::Error) -> Self {
        CoreError::Platform(format!("dbus: {e}"))
    }
}

#[cfg(target_os = "linux")]
impl From<atspi::AtspiError> for CoreError {
    fn from(e: atspi::AtspiError) -> Self {
        CoreError::Platform(format!("atspi: {e}"))
    }
}

impl From<std::io::Error> for CoreError {
    fn from(e: std::io::Error) -> Self {
        CoreError::Platform(format!("io: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_shapes_match_the_windows_engine() {
        // These constructions appear verbatim in ghost-session. If the shapes
        // drift, the shared layer stops compiling on one platform.
        let _ = CoreError::Win32 { code: 0, context: "unknown key name" };
        let _ = CoreError::WorkerPanic("x".into());
        let _ = CoreError::JobTimeout;
        let _ = CoreError::WindowGone;
        let _ = CoreError::FocusFailed { window: "w".into() };
        let _ = CoreError::WindowMinimized { name: "n".into() };
        let _ = CoreError::ProcessNotFound { name: "p".into() };
        let _ = CoreError::NotActionableInBackground { what: "w" };
    }

    #[test]
    fn platform_helper_produces_a_readable_message() {
        assert!(CoreError::platform("boom").to_string().contains("boom"));
    }
}
