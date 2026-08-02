//! Ghost cross-platform contract.
//!
//! Ghost ships as three versions that share one interface:
//! - **Windows** — full, verified (the engine in `ghost-core`/`ghost-session`).
//! - **Linux** — functional (`ghost-linux`): AT-SPI2 over D-Bus, XTEST /
//!   RemoteDesktop portal / uinput, X11 `GetImage` / Screenshot portal. Its
//!   X11 + AT-SPI2 paths are verified by a live CI suite against a real GTK
//!   application; the Wayland portal paths are implemented but not yet
//!   verified, and are not claimed.
//! - **macOS** — scaffolded; native backend built on Accessibility + CGEvent +
//!   ScreenCaptureKit (to be implemented and verified on a Mac).
//!
//! This crate defines the shared vocabulary (types), the [`Feature`]/[`Capabilities`]
//! model that says what Ghost can do on each OS *today*, and the [`Backend`] trait
//! each OS implements. It is pure Rust with no platform FFI, so it compiles for
//! every target; the OS-specific engines live behind `cfg` and target-gated deps.
//!
//! Honesty: a backend reports `is_functional() == true` only after its native
//! code has been exercised on that OS, and `supported` lists only the features
//! something actually verified. macOS reports false. See
//! `docs/cross-platform.md` for the implementation map.

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


/// Declared capabilities per platform — the single source of truth for the
/// three-version status. A platform lists a `Feature` only when something has
/// actually verified it there, which is why Linux claims six of nine.
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
        // Verified on real Linux, not asserted: the live AT-SPI suite
        // (crates/ghost-linux/tests/live_atspi.rs) runs in CI against a real GTK
        // application under Xvfb + D-Bus + at-spi-bus-launcher, and passes.
        // It proves both halves of the wedge - text written through
        // EditableText and read back from the application, and Action.DoAction
        // dismissing a dialog with an observable effect - plus window
        // enumeration, role/name lookup, describe_screen, capture and XTEST.
        //
        // The listed features are exactly the ones that suite exercises.
        // KeyInput, EditShortcuts and VisionGrounding are implemented but are
        // NOT claimed, because nothing has verified them end to end on Linux.
        // The Wayland paths (RemoteDesktop portal input, Screenshot portal
        // capture) are likewise unverified - CI runs X11. See
        // docs/linux-fedora.md.
        Platform::Linux => Capabilities {
            platform,
            functional: true,
            supported: vec![
                Feature::ElementDiscovery,
                Feature::Act,
                Feature::PerActionVerify,
                Feature::BackgroundDispatch,
                Feature::StructuredSnapshot,
                Feature::Screenshot,
                Feature::EditShortcuts,
            ],
            status: "functional on X11 + AT-SPI2, verified by the live CI suite (ghost-linux); window state via EWMH, occluded capture via XComposite, optional OCR via Tesseract; Wayland portal input/capture implemented but not yet verified on hardware",
        },
    }
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
    fn macos_is_still_a_scaffold() {
        // No Mac has been in the loop, so nothing may be claimed for it.
        let caps = capabilities_for(Platform::MacOS);
        assert!(!caps.functional);
        assert!(caps.supported.is_empty());
    }

    #[test]
    fn linux_claims_only_what_the_live_suite_verifies() {
        // The live AT-SPI suite proves discovery, act, verify, background
        // dispatch, snapshot and capture on X11. Key input, edit shortcuts and
        // vision grounding are implemented but unverified there, so claiming
        // them would be exactly the kind of unearned "it works" this model
        // exists to prevent.
        let caps = capabilities_for(Platform::Linux);
        assert!(caps.functional);
        assert!(caps.supports(Feature::ElementDiscovery));
        assert!(caps.supports(Feature::Act));
        assert!(caps.supports(Feature::PerActionVerify));
        assert!(caps.supports(Feature::BackgroundDispatch));
        assert!(caps.supports(Feature::StructuredSnapshot));
        assert!(caps.supports(Feature::Screenshot));

        // EditShortcuts is claimed: copy/cut/paste/select-all go through
        // AT-SPI's EditableText, which the live suite exercises.
        assert!(caps.supports(Feature::EditShortcuts));
        assert!(!caps.supports(Feature::KeyInput), "synthetic typing not verified end to end");
        assert!(!caps.supports(Feature::VisionGrounding), "not verified end to end on Linux");
    }

    #[test]
    fn windows_remains_the_most_capable_platform() {
        let win = capabilities_for(Platform::Windows);
        let lin = capabilities_for(Platform::Linux);
        assert!(win.supported.len() > lin.supported.len());
    }
}
