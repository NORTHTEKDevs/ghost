//! Ghost's macOS engine surface.
//!
//! `ghost-session` is platform-neutral orchestration written once against
//! *an engine* (see `crates/ghost-session/src/engine.rs`): `uia`, `input`,
//! `capture`, `system`, `process`, `ocr`, `error`, with identical type and
//! function signatures on every platform. Windows gets `ghost_core`, Linux
//! gets `ghost_linux`. This crate is the third arm: the same module tree,
//! so `ghost-session`, `ghost-cli`, `ghost-mcp` and `ghost-http` compile on
//! macOS instead of failing on an empty `engine` module.
//!
//! **What this is not**: a native macOS automation backend. That needs
//! AXUIElement (element discovery/act), CGEvent (keyboard/mouse), and
//! ScreenCaptureKit (capture) -- all C/Objective-C FFI, multi-week work, and
//! explicitly out of scope here (see `crates/ghost-platform/src/macos.rs` for
//! the implementation map when someone picks that up).
//!
//! **What this is**: every function ghost-session/-cli/-mcp/-http actually
//! call, with the exact signatures those call sites need, split into two
//! honest categories:
//!
//! 1. Logic that has zero OS dependency -- role-id lookup tables, the
//!    emergency-stop flag, screen-delta math over already-captured pixels,
//!    RGBA cropping, key-name parsing -- is ported for REAL. It works today,
//!    not just "will work once someone writes the backend."
//! 2. Anything that would need AXUIElement/CGEvent/ScreenCaptureKit returns
//!    `Err(CoreError::Unsupported { .. })`. Never `Ok(())`, never a default
//!    struct dressed up as a result -- this crate's whole reason to exist is
//!    that Ghost does not claim work it did not do. See `error.rs`.
//!
//! The practical upshot for `GhostSession::new()`: COM/UIA init and the
//! emergency-stop flag are real no-cost setup, so the session constructs
//! successfully and CDP browser automation (`ghost-browser`, no OS
//! dependency at all) and `ghost_shell` (gated only on
//! `input::hotkey::is_stopped()`, now real) both work. Desktop element
//! discovery, click/type, screenshots and window management do not, and say
//! so on every call.
#![cfg(target_os = "macos")]

pub mod capture;
pub mod error;
pub mod input;
pub mod ocr;
pub mod process;
pub mod system;
pub mod uia;
