//! Linux backend — SCAFFOLD (not yet functional).
//!
//! Compiles as an inert placeholder so the three-version architecture is real and
//! `ghost-platform` builds on Linux. The native engine must be implemented and
//! VERIFIED on a Linux desktop before flipping `functional` to true in
//! `capabilities_for(Platform::Linux)`.
//!
//! # Implementation map (what each capability needs on Linux)
//! - Element discovery / roles / states: **AT-SPI2** over D-Bus (the `atspi`
//!   crate). Roles via `Accessible.GetRole`, states (enabled/sensitive) via
//!   `Accessible.GetState`, geometry via `Component.GetExtents`. Requires the
//!   accessibility bus to be enabled and toolkits (GTK/Qt) exposing a11y.
//! - Act (click/press): AT-SPI `Action.DoAction` (e.g. the "click"/"press"
//!   action). For value fields, `EditableText.SetTextContents` (background-safe).
//! - Background dispatch (no focus steal): AT-SPI actions generally don't require
//!   raising the window, so this may work well — but MEASURE it. There is no exact
//!   analogue of Windows posted messages; some apps still raise on action.
//! - Screenshot / window capture: **X11** `XGetImage` / XShm, or on **Wayland**
//!   the `org.freedesktop.portal.Screenshot` XDG portal (Wayland forbids raw
//!   screen reads — capture is portal-gated and may prompt).
//! - Key/mouse input: **XTest** (`XTestFakeKeyEvent`) on X11; on Wayland use
//!   `libei` / the RemoteDesktop portal (`uinput` as a fallback with permissions).
//! - Vision grounding: reuse `ghost-ground` (already OS-agnostic).
//!
//! Wayland vs X11 is the key fork: input injection and capture differ sharply.
//! Suggested crates: `atspi`, `x11rb` / `xcb`, `ashpd` (XDG portals), `input`/
//! `libei` bindings. Build+test target: `x86_64-unknown-linux-gnu` on a desktop
//! session. See `docs/cross-platform.md`.

use crate::{capabilities_for, Backend, Capabilities, Platform};

pub struct LinuxBackend;

impl Backend for LinuxBackend {
    fn platform(&self) -> Platform {
        Platform::Linux
    }
    fn capabilities(&self) -> Capabilities {
        capabilities_for(Platform::Linux) // functional: false until built on-device
    }
}
