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
}
