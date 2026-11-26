#[derive(Debug, thiserror::Error)]
pub enum GhostError {
    #[error("Element not found: {query}")]
    ElementNotFound { query: String, screenshot: Option<Vec<u8>> },

    #[error("Element not interactable: {element} - {reason}")]
    ElementNotInteractable { element: String, reason: String },

    #[error("emergency stop active (Ctrl+Alt+G or ghost_stop): Ghost is stopped and refuses actions until ghost_reset")]
    Stopped,

    #[error("Timeout after {ms}ms waiting for: {action}")]
    Timeout { action: String, ms: u64 },

    #[error("UIA unavailable for app: {app}")]
    UiaUnavailable { app: String },

    #[error("Process not found: {name}")]
    ProcessNotFound { name: String },

    #[error("Core error: {0}")]
    Core(#[from] crate::engine::error::CoreError),

    #[error("Cache error: {0}")]
    Cache(String),

    #[error("Intent error: {0}")]
    Intent(String),

    #[error("Vision error: {0}")]
    Vision(String),

    #[error("Configuration error: {0}")]
    Config(String),
}

impl From<ghost_cache::error::CacheError> for GhostError {
    fn from(e: ghost_cache::error::CacheError) -> Self {
        GhostError::Cache(e.to_string())
    }
}

impl From<ghost_intent::error::IntentError> for GhostError {
    fn from(e: ghost_intent::error::IntentError) -> Self {
        GhostError::Intent(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, GhostError>;

/// Whether the emergency stop is active, here or in any other Ghost process.
pub fn is_stopped() -> bool {
    crate::engine::input::hotkey::is_stopped()
}

/// `Err(GhostError::Stopped)` while the emergency stop is active, raised here
/// or in any other Ghost process. Every state-changing path calls this first;
/// reads do not, so an agent can still look at what it was doing.
pub fn ensure_not_stopped() -> Result<()> {
    if is_stopped() {
        Err(GhostError::Stopped)
    } else {
        Ok(())
    }
}
