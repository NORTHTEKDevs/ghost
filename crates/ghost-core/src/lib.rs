// Ghost's engine is Windows-only today. Off Windows this crate compiles to
// nothing and its build script fails with a one-line explanation; see
// docs/cross-platform.md and docs/plans/2026-07-cross-platform-plan.md.
#![cfg(windows)]
// Pre-existing lints in Win32-adjacent code, out of scope for the macOS-backend
// PR that surfaced them via a newer clippy. Tracked as a follow-up cleanup pass.
#![allow(
    clippy::too_many_arguments,
    clippy::should_implement_trait,
    clippy::manual_checked_ops,
    clippy::unnecessary_cast,
)]

pub mod error;
pub mod input;
pub mod uia;
pub mod capture;
pub mod ocr;
pub mod process;
pub mod system;
