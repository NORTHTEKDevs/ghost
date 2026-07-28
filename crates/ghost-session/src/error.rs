#[derive(Debug, thiserror::Error)]
pub enum GhostError {
    #[error("Element not found: {query}")]
    ElementNotFound { query: String, screenshot: Option<Vec<u8>> },

    #[error("Element not interactable: {element} - {reason}")]
    ElementNotInteractable { element: String, reason: String },

    #[error("Ghost stopped by emergency stop (Ctrl+Alt+G)")]
    Stopped,

    #[error("Timeout after {ms}ms waiting for: {action}")]
    Timeout { action: String, ms: u64 },

    #[error("UIA unavailable for app: {app}")]
    UiaUnavailable { app: String },

    #[error("Process not found: {name}")]
    ProcessNotFound { name: String },

    /// A failure reported by a platform backend that has no more specific variant.
    ///
    /// Distinct from [`GhostError::Core`] because that one wraps `ghost-core`, which
    /// is Win32-only; this one carries an already-rendered message from whichever
    /// backend is compiled in.
    #[error("Platform error: {0}")]
    Platform(String),

    // `ghost-core`, `ghost-cache` and `ghost-intent` are Win32-only crates, so the
    // variants that name their error types can only exist on Windows.
    #[cfg(windows)]
    #[error("Core error: {0}")]
    Core(#[from] ghost_core::error::CoreError),

    #[error("Cache error: {0}")]
    Cache(String),

    #[error("Intent error: {0}")]
    Intent(String),

    #[error("Vision error: {0}")]
    Vision(String),

    #[error("Configuration error: {0}")]
    Config(String),
}

#[cfg(windows)]
impl From<ghost_cache::error::CacheError> for GhostError {
    fn from(e: ghost_cache::error::CacheError) -> Self {
        GhostError::Cache(e.to_string())
    }
}

#[cfg(windows)]
impl From<ghost_intent::error::IntentError> for GhostError {
    fn from(e: ghost_intent::error::IntentError) -> Self {
        GhostError::Intent(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, GhostError>;
