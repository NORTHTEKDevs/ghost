#[derive(Debug, thiserror::Error)]
pub enum GhostError {
    #[error("Element not found: {query}")]
    ElementNotFound { query: String, screenshot: Option<Vec<u8>> },

    #[error("Element not interactable: {element} - {reason}")]
    ElementNotInteractable { element: String, reason: String },

    /// Says which stop is in force (see `stop_refusal`).
    #[error("{}", stop_refusal())]
    Stopped,

    /// The stop was raised by the human (Ctrl+Alt+G): an agent cannot lift it.
    #[error("{HUMAN_STOPPED}")]
    HumanStopped,

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

/// Why this process cannot read the shared stop event, when it cannot (it
/// then refuses every action: fail closed). `None` off Windows, where there is
/// no shared event.
pub fn stop_event_error() -> Option<String> {
    #[cfg(windows)]
    return crate::engine::input::hotkey::stop_event_error();
    #[cfg(not(windows))]
    None
}

/// Whether the stop in force was raised by the human (Ctrl+Alt+G); an agent's
/// `ghost_reset` cannot lift it.
pub fn human_stop_active() -> bool {
    crate::engine::input::hotkey::human_stop_active()
}

/// The text an entry point (MCP, CLI, HTTP) refuses with while stopped: when
/// the shared event cannot be read, that and the fix; when the human raised
/// the stop, that and how they resume; otherwise the stop.
pub fn stop_refusal() -> String {
    if let Some(why) = stop_event_error() {
        return format!("Ghost is stopped (fail closed) - {why}");
    }
    if human_stop_active() {
        return HUMAN_STOPPED.to_string();
    }
    STOPPED.to_string()
}

const STOPPED: &str = "emergency stop active (Ctrl+Alt+G or ghost_stop): Ghost is stopped and refuses actions \
     until the stop is lifted; a stop the human raised is theirs to lift";

/// Agent-facing: never says how the human resumes (see ghost-core stop.rs).
const HUMAN_STOPPED: &str = "the human stopped Ghost (Ctrl+Alt+G); only the human can resume it - wait for them";

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
