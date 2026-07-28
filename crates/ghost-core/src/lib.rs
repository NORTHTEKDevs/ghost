// Ghost's engine is Windows-only today. Off Windows this crate compiles to
// nothing and its build script fails with a one-line explanation; see
// docs/cross-platform.md and docs/plans/2026-07-cross-platform-plan.md.
#![cfg(windows)]

pub mod error;
pub mod input;
pub mod uia;
pub mod capture;
pub mod ocr;
pub mod process;
pub mod system;
