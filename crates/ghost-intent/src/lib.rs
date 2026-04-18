//! ghost-intent: JSON intent compiler + JSONLogic + FSM executor.
pub mod compiler;
pub mod executor;
pub mod jsonlogic;
pub mod error;

pub use error::IntentError;
