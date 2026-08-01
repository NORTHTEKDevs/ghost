//! The platform engine.
//!
//! `ghost-session` is platform-neutral orchestration: locator tiers, the
//! grounding cascade, reflection, waits, act-then-verify. All of it is written
//! once and runs on both platforms by talking to *an engine* rather than to an
//! OS directly.
//!
//! - **Windows** -> [`ghost_core`]: Win32 UI Automation, SendInput, posted
//!   window messages, DXGI/GDI capture.
//! - **Linux** -> [`ghost_linux`]: AT-SPI2 over D-Bus, XTEST / RemoteDesktop
//!   portal / uinput, X11 `GetImage` / Screenshot portal.
//!
//! Both crates expose the same module tree (`uia`, `input`, `capture`,
//! `system`, `process`, `ocr`, `error`) and the same type and function
//! signatures, so everything above this line is identical on both.
//!
//! Window handles are `isize` on both platforms (`0` means none). On Windows
//! that is an `HWND`; on Linux it is an interned AT-SPI `(bus name, object
//! path)` pair.

#[cfg(windows)]
pub use ghost_core::*;

#[cfg(target_os = "linux")]
pub use ghost_linux::*;
