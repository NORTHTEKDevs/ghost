//! macOS backend — SCAFFOLD (not yet functional).
//!
//! This compiles as an inert placeholder so the three-version architecture is
//! real and `ghost-platform` builds on macOS. The native engine must be
//! implemented and VERIFIED on a Mac before flipping `functional` to true in
//! `capabilities_for(Platform::MacOS)`.
//!
//! # Implementation map (what each capability needs on macOS)
//! - Element discovery / roles / enabled state: **Accessibility API** —
//!   `AXUIElement` (kAXChildrenAttribute, kAXRoleAttribute, kAXEnabledAttribute,
//!   kAXValueAttribute). Requires the app to be granted Accessibility permission
//!   in System Settings > Privacy & Security.
//! - Act (click/press): `AXUIElementPerformAction(kAXPressAction)`. NOTE: verify
//!   whether this activates the target window — if it does, background dispatch
//!   needs `CGEvent` posted to the window's connection, or `AXUIElementSetAttribute`
//!   for value changes.
//! - Type: `AXUIElementSetAttributeValue(kAXValueAttribute)` (background-safe,
//!   like ValuePattern.SetValue on Windows), or synthesized `CGEvent` keystrokes.
//! - Background dispatch (no focus steal): AX value-set + press are the closest
//!   analogue; there is no exact equivalent of Windows posted window messages —
//!   this capability may be PARTIAL on macOS. Measure it before claiming it.
//! - Screenshot / window capture: **ScreenCaptureKit** (`SCScreenshotManager`) or
//!   legacy `CGWindowListCreateImage`. Needs Screen Recording permission.
//! - Key input: `CGEventCreateKeyboardEvent` + `CGEventPost`.
//! - Vision grounding: reuse `ghost-ground` (already OS-agnostic).
//!
//! Suggested crates: `accessibility` / `accessibility-sys`, `core-graphics`,
//! `core-foundation`, `objc2` + `objc2-app-kit`. Build+test target:
//! `aarch64-apple-darwin` on a Mac. See `docs/cross-platform.md`.

use crate::{capabilities_for, Backend, Capabilities, Platform};

pub struct MacBackend;

impl Backend for MacBackend {
    fn platform(&self) -> Platform {
        Platform::MacOS
    }
    fn capabilities(&self) -> Capabilities {
        capabilities_for(Platform::MacOS) // functional: false until built on-device
    }
}
