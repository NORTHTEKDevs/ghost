//! ghost-intent: JSON intent compiler + JSONLogic + FSM executor.
// Ghost's engine is Windows-only today. Off Windows this crate compiles to
// nothing and its build script fails with a one-line explanation; see
// docs/cross-platform.md and docs/plans/2026-07-cross-platform-plan.md.
#![cfg(windows)]

pub mod compiler;
pub mod executor;
pub mod jsonlogic;
pub mod error;

pub use error::IntentError;
