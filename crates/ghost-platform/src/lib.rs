//! Ghost cross-platform contract.
//!
//! Ghost ships as three versions that share one interface:
//! - **Windows** — full, verified (the engine in `ghost-core`/`ghost-session`).
//! - **macOS** — scaffolded; native backend built on Accessibility + CGEvent +
//!   ScreenCaptureKit (to be implemented and verified on a Mac).
//! - **Linux** — scaffolded; native backend built on AT-SPI (D-Bus) + XTest/libei
//!   + X11/portal capture (to be implemented and verified on Linux).
//!
//! This crate defines the shared vocabulary (types), the [`Feature`]/[`Capabilities`]
//! model that says what Ghost can do on each OS *today*, and the [`Backend`] trait
//! each OS implements. It is pure Rust with no platform FFI, so it compiles for
//! every target; the OS-specific engines live behind `cfg` and target-gated deps.
//!
//! Honesty: only the Windows backend is functional and verified. macOS/Linux
//! backends report `is_functional() == false` until their native code is built and
//! tested on-device — see `docs/cross-platform.md` for the implementation map.

use serde::{Deserialize, Serialize};

pub mod types;
pub use types::{ActionKind, ElementInfo, Locator, Point, Rect, WindowRef};

// ---------------------------------------------------------------------------
// Platform + capability model
// ---------------------------------------------------------------------------

/// The operating systems Ghost targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    Windows,
    MacOS,
    Linux,
}

impl Platform {
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Windows => "windows",
            Platform::MacOS => "macos",
            Platform::Linux => "linux",
        }
    }
}

/// A capability Ghost may or may not have on a given OS. This is the honest,
/// per-platform feature matrix — see [`Capabilities`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Feature {
    /// Discover UI elements via the OS accessibility tree.
    ElementDiscovery,
    /// Click / type / etc. on a discovered element.
    Act,
    /// Confirm an action actually happened (act-then-verify).
    PerActionVerify,
    /// Drive an app WITHOUT taking foreground or moving the cursor.
    BackgroundDispatch,
    /// Structured, agent-planning snapshot (id/role/rect/enabled/actions).
    StructuredSnapshot,
    /// Screen / window capture.
    Screenshot,
    /// Keyboard input.
    KeyInput,
    /// Clipboard/edit shortcuts (copy/cut/paste/undo/select-all).
    EditShortcuts,
    /// VLM vision grounding for description-based targets.
    VisionGrounding,
}

/// What Ghost can do on one platform right now, plus a human note.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub platform: Platform,
    /// True only when the native backend is implemented and verified on-device.
    pub functional: bool,
    /// Features supported by the (current) backend. Empty for a pure scaffold.
    pub supported: Vec<Feature>,
    /// Honest one-line status.
    pub status: &'static str,
}

impl Capabilities {
    pub fn supports(&self, f: Feature) -> bool {
        self.supported.contains(&f)
    }
}

/// The full feature set — every capability Ghost offers (as on Windows today).
pub fn all_features() -> Vec<Feature> {
    use Feature::*;
    vec![
        ElementDiscovery, Act, PerActionVerify, BackgroundDispatch,
        StructuredSnapshot, Screenshot, KeyInput, EditShortcuts, VisionGrounding,
    ]
}

// ---------------------------------------------------------------------------
// Backend contract
// ---------------------------------------------------------------------------

/// The interface every OS backend fulfils. The Windows engine
/// (`ghost-core`/`ghost-session`) is the reference implementation; the macOS and
/// Linux backends implement the same contract using their native APIs.
pub trait Backend {
    fn platform(&self) -> Platform;
    fn capabilities(&self) -> Capabilities;
    /// True only when the native engine is present and verified on this OS.
    fn is_functional(&self) -> bool {
        self.capabilities().functional
    }
}

// Per-OS backend modules. Each compiles only on its target; `current()` picks the
// one for the OS this build runs on.
#[cfg(windows)]
pub mod windows;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "linux")]
pub mod linux;

/// The backend for the OS this build was compiled for.
pub fn current() -> Box<dyn Backend> {
    #[cfg(windows)]
    { Box::new(windows::WindowsBackend) }
    #[cfg(target_os = "macos")]
    { Box::new(macos::MacBackend) }
    #[cfg(target_os = "linux")]
    { Box::new(linux::LinuxBackend) }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    { compile_error!("Ghost supports Windows, macOS, and Linux only") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_backend_matches_host_os() {
        let b = current();
        #[cfg(windows)]
        assert_eq!(b.platform(), Platform::Windows);
        #[cfg(target_os = "macos")]
        assert_eq!(b.platform(), Platform::MacOS);
        #[cfg(target_os = "linux")]
        assert_eq!(b.platform(), Platform::Linux);
        // Capabilities are self-consistent.
        let caps = b.capabilities();
        assert_eq!(caps.functional, b.is_functional());
    }

    #[test]
    fn windows_reports_full_and_functional() {
        // The capability descriptor for Windows is complete + functional,
        // regardless of the host we compile the test on.
        let caps = capabilities_for(Platform::Windows);
        assert!(caps.functional);
        assert_eq!(caps.supported.len(), all_features().len());
        assert!(caps.supports(Feature::BackgroundDispatch));
    }

    #[test]
    fn scaffold_platforms_report_not_functional() {
        for p in [Platform::MacOS, Platform::Linux] {
            let caps = capabilities_for(p);
            assert!(!caps.functional, "{:?} must not claim functional yet", p);
        }
    }
}

/// Declared capabilities per platform — the single source of truth for the
/// three-version status. Windows is full + functional; macOS/Linux are scaffolds
/// (functional = false) until their native backends are built and verified.
pub fn capabilities_for(platform: Platform) -> Capabilities {
    match platform {
        Platform::Windows => Capabilities {
            platform,
            functional: true,
            supported: all_features(),
            status: "full and verified (ghost-core/ghost-session over Win32 UIA + window messages)",
        },
        Platform::MacOS => Capabilities {
            platform,
            functional: false,
            supported: vec![],
            status: "scaffold — native backend (Accessibility/AXUIElement + CGEvent + ScreenCaptureKit) not yet implemented/verified",
        },
        Platform::Linux => Capabilities {
            platform,
            functional: false,
            supported: vec![],
            status: "scaffold — native backend (AT-SPI over D-Bus + XTest/libei + X11/portal capture) not yet implemented/verified",
        },
    }
}
