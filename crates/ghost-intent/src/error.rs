#[derive(thiserror::Error, Debug, Clone)]
pub enum IntentError {
    #[error("invalid intent: {0}")]
    Invalid(String),

    #[error("op failed: {0}")]
    OpFailed(String),

    #[error("aborted: {0}")]
    Aborted(String),

    #[error("intent timed out")]
    Timeout,
}
