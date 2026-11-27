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

    #[error("Window '{name}' is minimized; restore it first without taking the user's focus (ghost_window op=state name={name} state=restore)")]
    WindowMinimized { name: String },

    #[error("Could not confirm foreground for window: {window}")]
    FocusFailed { window: String },

    #[error("Element not actionable in background mode: {what}")]
    NotActionableInBackground { what: &'static str },
    #[error("'{action}' has no background path on the user's desktop and the focus policy is 'background'. Work where no policy is needed: launch the app with ghost_window op=launch (it lands on a hidden desktop) or the browser with ghost_browser_launch, then drive the anchored window. Real input on the user's desktop is the operator's call: GHOST_FOCUS_LOCK=off in the server environment, then ghost_set_focus_policy")]
    NoBackgroundPath { action: &'static str },

    #[error("focus policy is locked to 'background'; '{requested}' would let this process take the user's mouse, keyboard or foreground. Work where no policy is needed: launch the app with ghost_window op=launch (it lands on a hidden desktop) or the browser with ghost_browser_launch, then drive the anchored window. Only the operator can hand over real input, with GHOST_FOCUS_LOCK=off (and optionally GHOST_FOCUS_POLICY) in the MCP server's environment")]
    FocusLocked { requested: &'static str },

    #[error("no message-postable text control in that window; on an isolated desktop there is no real keyboard input to fall back to, so this target cannot be typed into. Try ghost_act type with the control's name (UIA), or for a browser the CDP route (ghost_browser_launch + ghost_tab_type); real keyboard input on the user's desktop needs the operator's GHOST_FOCUS_LOCK=off and the 'foreground' policy")]
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

    #[error("the point ({x}, {y}) is on no monitor (monitors: {monitors}); nothing was clicked - re-read the element's position")]
    PointOffScreen { x: i32, y: i32, monitors: String },

    /// The emergency stop is active (Ctrl+Alt+G or `ghost_stop`, in this or
    /// any other Ghost process). Nothing that changes state runs until reset.
    #[error("{}", crate::stop::refusal_text())]
    Stopped,
}
