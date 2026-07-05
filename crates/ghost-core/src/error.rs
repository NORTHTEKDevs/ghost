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
}
