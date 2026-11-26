#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error("no Chrome or Edge installation found; set GHOST_BROWSER_PATH to the executable")]
    BrowserNotFound,

    #[error("failed to launch browser: {0}")]
    Launch(String),

    #[error("browser did not publish a DevTools port within {ms}ms")]
    DevToolsTimeout { ms: u64 },

    #[error("DevTools transport error: {0}")]
    Transport(String),

    #[error("CDP call '{method}' failed: {message}")]
    Cdp { method: String, message: String },

    #[error("CDP call '{method}' timed out after {ms}ms")]
    CdpTimeout { method: String, ms: u64 },

    #[error("no tab matching '{0}'")]
    TabNotFound(String),

    #[error("selector '{selector}' not found within {ms}ms")]
    SelectorNotFound { selector: String, ms: u64 },

    #[error("element '{selector}' has no layout box (hidden or zero-sized)")]
    NotVisible { selector: String },

    #[error("unexpected CDP response shape for '{method}': {detail}")]
    Protocol { method: String, detail: String },

    #[error("browser connection is closed")]
    Closed,

    #[error("emergency stop active: Ghost is stopped and refuses actions until ghost_reset")]
    Stopped,

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, BrowserError>;

/// Where this crate asks whether Ghost's emergency stop is active. The crate
/// cannot read the stop itself (it is platform-neutral and the stop lives in
/// the platform engines), so the session installs the check at start-up.
static STOP_CHECK: std::sync::OnceLock<fn() -> bool> = std::sync::OnceLock::new();

/// Install the emergency-stop check. The first call wins.
pub fn set_stop_check(check: fn() -> bool) {
    let _ = STOP_CHECK.set(check);
}

/// `Err(BrowserError::Stopped)` while the installed check says stopped. Every
/// page-changing call runs this first; reads (title, text, describe, the
/// `eval` they are built on) do not.
pub(crate) fn ensure_not_stopped() -> Result<()> {
    match STOP_CHECK.get() {
        Some(stopped) if stopped() => Err(BrowserError::Stopped),
        _ => Ok(()),
    }
}
