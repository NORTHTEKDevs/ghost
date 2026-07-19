pub mod error;
pub mod locator;
pub mod element;
pub mod reflection;
pub mod session;
pub mod shell;
pub mod tiers;
pub mod vision;

/// Returns true only if the env var is set AND non-empty/non-whitespace.
/// `std::env::var::is_ok()` returns true for `Ok("")`, which looks SET but
/// produces an unauthenticated request (provider 500). This helper is the
/// single source of truth for "key is usable".
pub(crate) fn env_key_is_set(name: &str) -> bool {
    matches!(std::env::var(name), Ok(v) if !v.trim().is_empty())
}

pub use session::{GhostSession, Region};
pub use locator::By;
pub use element::GhostElement;
pub use error::GhostError;
pub use ghost_core::uia::{ElementDescriptor, WindowInfo};
pub use ghost_core::input::EditCommand;
pub use reflection::{ReflectionBuffer, ActionOutcome, ReflectionEntry, hash_obs};
pub use ghost_ground::types::{Grounded, Target, Tier};
pub use ghost_ground::engine::LocateMode;

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
