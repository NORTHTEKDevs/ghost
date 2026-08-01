//! Windows automation engine (Win32 UI Automation).
//!
//! The whole crate is gated to Windows so it can stay a plain workspace
//! member: on Linux it compiles to nothing and `ghost-linux` takes its place
//! behind `ghost_session::engine`.
#![cfg(windows)]

pub mod error;
pub mod input;
pub mod uia;
pub mod capture;
pub mod ocr;
pub mod process;
pub mod system;
