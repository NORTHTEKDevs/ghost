//! Linux backend — implemented.
//!
//! The engine lives in the `ghost-linux` crate; this is the `ghost-platform`
//! descriptor for it. `ghost-session` and `ghost-mcp` reach the engine through
//! `ghost_session::engine`, a `cfg` alias that resolves to `ghost-core` on
//! Windows and `ghost-linux` here, so the shared layers are identical on both.
//!
//! # What the engine does
//! - **Element discovery / roles / states:** AT-SPI2 over D-Bus (`atspi`).
//!   `Accessible.GetRole`, `GetState`, `Component.GetExtents`. AT-SPI roles are
//!   mapped onto the same numeric id space the Windows engine uses, so role
//!   names cannot drift between platforms. An editable field is identified by
//!   the `EditableText` **interface** rather than its role name, because
//!   toolkits disagree about whether an entry is `entry` or `text`.
//! - **Act:** `Action.DoAction`, with the action chosen by name rather than
//!   assuming index 0. Text goes through `EditableText.SetTextContents`, numeric
//!   widgets through `Value.SetCurrentValue`.
//! - **Background dispatch:** this is the notable result. AT-SPI actions make
//!   the application perform the operation through its own toolkit, so there is
//!   no pointer to move and no window to raise — a *cleaner* guarantee than the
//!   posted window messages Windows relies on, and identical under X11 and
//!   Wayland. Measured, not assumed: the live suite writes text and invokes a
//!   button with observable effects and no synthetic input.
//! - **Capture:** X11 `GetImage`; on Wayland the one-shot `Screenshot` portal.
//!   ScreenCast + PipeWire was deliberately avoided — Ghost captures stills, not
//!   video, so it would add C linkage and DMA-BUF handling for nothing.
//! - **Synthetic input (fallback only):** XTEST on X11, the RemoteDesktop portal
//!   on Wayland (with a persisted, rotated restore token so consent is asked
//!   once), and `uinput` for unattended machines via `GHOST_INPUT=uinput`.
//!
//! # What is not claimed
//! `KeyInput`, `EditShortcuts` and `VisionGrounding` are implemented but absent
//! from `capabilities_for(Platform::Linux).supported`, because nothing has
//! verified them end to end on Linux. The Wayland portal paths are likewise
//! unverified — CI runs X11. See `docs/linux-fedora.md` for the on-device
//! checklist that signs them off.
//!
//! The whole engine is pure Rust (`atspi`/`zbus`, `x11rb`, `ashpd`, `evdev`), so
//! it needs no `-devel` packages and cross-compiles from other hosts.

use crate::{capabilities_for, Backend, Capabilities, Platform};

pub struct LinuxBackend;

impl Backend for LinuxBackend {
    fn platform(&self) -> Platform {
        Platform::Linux
    }
    fn capabilities(&self) -> Capabilities {
        capabilities_for(Platform::Linux)
    }
}
