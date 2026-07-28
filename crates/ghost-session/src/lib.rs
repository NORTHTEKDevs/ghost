//! Ghost's session API.
//!
//! Windows is the verified platform. macOS now compiles here too and gets the subset
//! of the API that [`backend::SessionBackend`] describes — see that module for what is
//! in the subset and why the rest is not. Linux has no native code at all, so this
//! crate still compiles to nothing there.
//!
//! `capabilities_for(Platform::MacOS).functional` is `false`: the macOS path builds
//! and links against Apple's SDK in CI but has not been run on a Mac. See
//! docs/mac-testing.md and docs/plans/2026-07-cross-platform-plan.md.
#![cfg(any(windows, target_os = "macos"))]

// Portable: neutral types, or pure logic with no OS calls.
pub mod backend;
pub mod error;
pub mod locator;
pub mod reflection;
pub mod vision;

// Win32/UIA. Every one of these reaches COM through ghost-core, ghost-cache or
// ghost-intent, which are Windows-only crates.
#[cfg(windows)]
pub mod element;
#[cfg(windows)]
pub mod session;
#[cfg(windows)]
pub mod shell;
#[cfg(windows)]
pub mod tiers;
#[cfg(windows)]
pub mod win_backend;

#[cfg(target_os = "macos")]
pub mod mac_backend;

/// Returns true only if the env var is set AND non-empty/non-whitespace.
/// `std::env::var::is_ok()` returns true for `Ok("")`, which looks SET but
/// produces an unauthenticated request (provider 500). This helper is the
/// single source of truth for "key is usable".
pub(crate) fn env_key_is_set(name: &str) -> bool {
    matches!(std::env::var(name), Ok(v) if !v.trim().is_empty())
}

// Portable surface.
pub use backend::{Session, SessionBackend};
pub use error::GhostError;
pub use ghost_platform::{Capabilities, ElementInfo, Feature, Locator, Platform, Point, WindowRef};
pub use locator::By;
pub use reflection::{hash_obs, ActionOutcome, ReflectionBuffer, ReflectionEntry};
pub use ghost_ground::engine::LocateMode;
pub use ghost_ground::types::{Grounded, Target, Tier};

// The Windows engine and the Win32 vocabulary it speaks. `GhostSession` is
// deliberately still its own type rather than an alias for [`Session`]; see
// [`backend::Session`] for why.
#[cfg(windows)]
pub use element::GhostElement;
#[cfg(windows)]
pub use ghost_core::input::EditCommand;
#[cfg(windows)]
pub use ghost_core::uia::{ElementDescriptor, WindowInfo};
#[cfg(windows)]
pub use session::{GhostSession, Region};
#[cfg(windows)]
pub use win_backend::WinBackend;

#[cfg(target_os = "macos")]
pub use mac_backend::MacSessionBackend;

#[cfg(test)]
mod tests {
    use super::env_key_is_set;

    // Each test uses a unique env var name to avoid parallel-test races.

    #[test]
    fn env_key_is_set_unset_var_returns_false() {
        std::env::remove_var("_GHOST_KEY_TEST_UNSET");
        assert!(!env_key_is_set("_GHOST_KEY_TEST_UNSET"));
    }

    #[test]
    fn env_key_is_set_empty_string_returns_false() {
        std::env::set_var("_GHOST_KEY_TEST_EMPTY", "");
        let result = env_key_is_set("_GHOST_KEY_TEST_EMPTY");
        std::env::remove_var("_GHOST_KEY_TEST_EMPTY");
        assert!(!result, "empty string must be treated as unset");
    }

    #[test]
    fn env_key_is_set_whitespace_only_returns_false() {
        std::env::set_var("_GHOST_KEY_TEST_WS", "   ");
        let result = env_key_is_set("_GHOST_KEY_TEST_WS");
        std::env::remove_var("_GHOST_KEY_TEST_WS");
        assert!(!result, "whitespace-only must be treated as unset");
    }

    #[test]
    fn env_key_is_set_nonempty_returns_true() {
        std::env::set_var("_GHOST_KEY_TEST_NONEMPTY", "sk-test-key");
        let result = env_key_is_set("_GHOST_KEY_TEST_NONEMPTY");
        std::env::remove_var("_GHOST_KEY_TEST_NONEMPTY");
        assert!(result);
    }
}
