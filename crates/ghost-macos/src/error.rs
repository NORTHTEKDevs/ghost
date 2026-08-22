//! Error type mirroring `ghost_core::error::CoreError` (Windows) and
//! `ghost_linux::error::CoreError` (Linux) variant-for-variant.
//!
//! `ghost-session`'s own error type does `Core(#[from] crate::engine::error::CoreError)`
//! and, in several places, *constructs* specific variants directly (e.g.
//! `CoreError::Win32 { code: 0, context: "unknown key name" }` for a
//! synthetic/logic error that has nothing to do with an actual Win32 call --
//! see `session.rs`). Those call sites are shared, unconditional source code:
//! if a variant's name or fields drifted here, ghost-session would stop
//! compiling on macOS the same way it does today with no macOS arm at all.
//! So every original variant is reproduced with the identical name, fields,
//! and message.
//!
//! `Unsupported` is the one addition: the honest way to say "this needs
//! AXUIElement / CGEvent / ScreenCaptureKit, which this pure-Rust proxy does
//! not implement" instead of returning a fake `Ok`. Prefer reusing an
//! existing variant where one already fits (e.g. `WindowGone` for "no such
//! window" once window handles exist); reach for `Unsupported` only when no
//! existing variant honestly describes the gap.

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
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

    #[error("'{action}' has no background path and the focus policy is 'background'; call ghost_set_focus_policy with 'prefer_background' or 'foreground' to allow real input")]
    NoBackgroundPath { action: &'static str },

    #[error("no message-postable text control in that window; on an isolated desktop there is no real keyboard input to fall back to, so this target cannot be typed into. Drive it on the user's desktop with the 'foreground' focus policy instead")]
    NoTextControl,

    #[error("typed {text:?} but the control's value did not change; the keystrokes did not land")]
    TypeNotVerified { text: String },

    #[error("typed {wanted:?} but the control only holds {got:?}; the target accepted some input and dropped the rest")]
    TypePartial { wanted: String, got: String },

    #[error("another ghost process held the foreground input lease for {ms}ms")]
    ForegroundBusy { ms: u32 },

    #[error("background {action} not supported by target window (hwnd {hwnd:#x})")]
    BackgroundUnsupported { action: &'static str, hwnd: usize },

    #[error("window capture failed: {0}")]
    CaptureFailed(String),

    #[error("desktop error: {0}")]
    Desktop(String),

    /// Not a Windows/Linux variant. The honest signal that a call needs a
    /// native macOS API this pure-Rust crate does not implement (see the
    /// module docs). `op` is the thing that was asked for; `needs` names the
    /// concrete API that a real backend would call instead.
    #[error("'{op}' is not implemented on macOS yet (needs {needs}); see docs/macos-build.md")]
    Unsupported { op: &'static str, needs: &'static str },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These constructions appear verbatim in ghost-session (session.rs,
    /// error.rs, tiers.rs). If a shape drifts here, ghost-session stops
    /// compiling on macOS -- this test is the tripwire for that, even though
    /// it can only run when this crate is actually built (i.e. on macOS, or
    /// via `cargo check --target aarch64-apple-darwin` from anywhere).
    #[test]
    fn variant_shapes_match_the_windows_and_linux_engines() {
        let _ = CoreError::Win32 { code: 0, context: "unknown key name" };
        let _ = CoreError::WorkerPanic("x".into());
        let _ = CoreError::JobTimeout;
        let _ = CoreError::WindowGone;
        let _ = CoreError::FocusFailed { window: "w".into() };
        let _ = CoreError::ProcessNotFound { name: "p".into() };
    }

    #[test]
    fn unsupported_message_names_the_missing_api() {
        let e = CoreError::Unsupported { op: "click", needs: "CGEvent" };
        let msg = e.to_string();
        assert!(msg.contains("click"));
        assert!(msg.contains("CGEvent"));
    }
}
