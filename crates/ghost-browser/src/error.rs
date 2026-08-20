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

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, BrowserError>;
