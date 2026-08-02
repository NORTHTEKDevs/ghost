//! Ghost's Linux automation engine.
//!
//! This crate is the Linux counterpart to `ghost-core` (Windows/UI Automation).
//! It exposes the same module shape and symbol names so that `ghost-session` and
//! `ghost-mcp` can switch engines with a single `cfg` alias and reuse all of
//! their orchestration, locator, grounding and MCP-protocol logic unchanged.
//!
//! # The architecture, and why it differs from Windows
//!
//! On Windows, Ghost's differentiator is *background dispatch*: driving an app
//! without stealing foreground or moving the cursor, implemented with posted
//! window messages. Linux has an exact and in fact cleaner analogue:
//! **AT-SPI2 accessibility actions**. `Action.DoAction`,
//! `EditableText.SetTextContents`, `Value.SetCurrentValue` and
//! `Component.GrabFocus` drive an application through its own toolkit without
//! synthetic input, without moving the pointer, and without a consent prompt --
//! and they behave identically under X11 and Wayland.
//!
//! So the priority is inverted relative to Windows:
//!
//! 1. **AT-SPI action** -- preferred. No focus steal, no cursor movement.
//! 2. **Synthetic input** -- fallback, only for elements that expose no usable
//!    action (custom canvases, some Electron/Chromium surfaces). X11 uses XTEST;
//!    Wayland uses the RemoteDesktop portal, or `uinput` when
//!    `GHOST_INPUT=uinput` is set for unattended machines.
//!
//! # Honesty
//!
//! Capabilities that genuinely cannot work on a given session type are reported
//! as unsupported rather than silently faked. See [`session::SessionKind`].
//! Notably: native-Wayland per-window capture and root-window capture under
//! XWayland are not available, and an application that exposes no accessibility
//! tree exposes nothing to AT-SPI -- vision grounding (`ghost-ground`) is the
//! fallback there, exactly as on Windows.
//!
//! Everything here is pure Rust: no `at-spi2-core-devel`, `libX11-devel`,
//! `pipewire-devel` or other `-devel` packages are required to build.
//!
//! # Platform gating
//!
//! The modules that talk to Linux APIs are gated to `target_os = "linux"`. The
//! platform-independent logic -- role vocabulary, alias rules, handle interning,
//! keysym mapping, screen-delta verification -- is deliberately **not** gated, so
//! its tests run on any host. That matters: the role-id table below must stay
//! byte-identical to the Windows engine's, and a test that only compiles on
//! Linux cannot guard that on a Windows CI machine.

pub mod a11y;
pub mod capture;
pub mod error;
pub mod keysym;
pub mod session;

#[cfg(target_os = "linux")]
pub mod bridge;
#[cfg(target_os = "linux")]
pub mod input;
#[cfg(target_os = "linux")]
pub mod ocr;
#[cfg(target_os = "linux")]
pub mod process;
#[cfg(target_os = "linux")]
pub mod system;
#[cfg(target_os = "linux")]
pub mod wm;

/// `ghost-core` names its accessibility module `uia` (Windows UI Automation).
/// Ghost's shared layers refer to it by that name, so alias it here. On Linux
/// the underlying technology is AT-SPI2 -- see [`a11y`].
pub use a11y as uia;

pub use error::CoreError;
pub use session::{session_kind, SessionKind};
