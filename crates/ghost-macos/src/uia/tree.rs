//! Element search and window management.
//!
//! Every real query here (find-by-name, find-by-role, describe, element-at-
//! point, list/focus/resize windows) needs AXUIElement -- out of scope --
//! and returns `Err(Unsupported)` rather than a fabricated empty result: a
//! silent `Ok(None)`/`Ok(vec![])` would read as "searched thoroughly, found
//! nothing" when no search happened at all, which is exactly the fake-success
//! pattern this crate exists to avoid. Downstream, the grounding cascade
//! (`ghost-session/src/tiers.rs`) already treats a tier `Err` as a `Miss` and
//! falls through to the next tier, so this degrades gracefully rather than
//! panicking or hanging.
//!
//! `role_alias_matches` and `WindowState::from_str` are pure data/parsing --
//! ported for real. `UiaTree::new()` succeeds (it does no real query on
//! Windows either -- it just wraps a COM automation object), which matters:
//! `GhostSession::new()` calls it and propagates any error, so failing here
//! would take down `ghost_shell`/`ghost_browser_*` along with the desktop
//! features that actually need it.

use super::element::{ElementDescriptor, UiaElement};
use crate::error::CoreError;

/// Roles that are acceptable substitutes when no exact match exists. Pure
/// data -- identical to the Windows/Linux tables.
pub fn role_alias_matches(searched: &str, el_role: &str) -> bool {
    match searched {
        "tab" => el_role == "tabitem",
        "list" => el_role == "listitem",
        "edit" => el_role == "document",
        _ => false,
    }
}

pub struct UiaTree {
    _private: (),
}

impl UiaTree {
    pub fn new() -> Result<Self, CoreError> {
        Ok(Self { _private: () })
    }

    pub fn find_by_name(&self, _name: &str) -> Result<Option<UiaElement>, CoreError> {
        Err(CoreError::Unsupported { op: "find_by_name", needs: "AXUIElementCopyAttributeValue" })
    }

    pub fn find_by_role(&self, _role: &str) -> Result<Option<UiaElement>, CoreError> {
        Err(CoreError::Unsupported { op: "find_by_role", needs: "AXUIElementCopyAttributeValue" })
    }

    pub fn find_by_name_in_hwnd(&self, _hwnd: isize, _name: &str) -> Result<Option<UiaElement>, CoreError> {
        Err(CoreError::Unsupported { op: "find_by_name_in_hwnd", needs: "AXUIElementCopyAttributeValue" })
    }

    pub fn find_by_role_in_hwnd(&self, _hwnd: isize, _role: &str) -> Result<Option<UiaElement>, CoreError> {
        Err(CoreError::Unsupported { op: "find_by_role_in_hwnd", needs: "AXUIElementCopyAttributeValue" })
    }

    pub fn find_by_name_fast(&self, _name: &str) -> Result<Option<UiaElement>, CoreError> {
        Err(CoreError::Unsupported { op: "find_by_name_fast", needs: "AXUIElementCopyAttributeValue" })
    }

    pub fn find_by_role_fast(&self, _role: &str) -> Result<Option<UiaElement>, CoreError> {
        Err(CoreError::Unsupported { op: "find_by_role_fast", needs: "AXUIElementCopyAttributeValue" })
    }

    pub fn describe_screen_fast(&self) -> Result<Vec<ElementDescriptor>, CoreError> {
        Err(CoreError::Unsupported { op: "describe_screen_fast", needs: "AXUIElementCopyAttributeValue" })
    }

    pub fn describe_screen(&self, _window_name: Option<&str>) -> Result<Vec<ElementDescriptor>, CoreError> {
        Err(CoreError::Unsupported { op: "describe_screen", needs: "AXUIElementCopyAttributeValue" })
    }

    pub fn find_all_in_hwnd(
        &self,
        _hwnd: isize,
        _name: Option<&str>,
        _role: Option<&str>,
        _cap: usize,
    ) -> Result<Vec<UiaElement>, CoreError> {
        Err(CoreError::Unsupported { op: "find_all_in_hwnd", needs: "AXUIElementCopyAttributeValue" })
    }

    pub fn collect_text(&self, _window_name: Option<&str>, _max_chars: usize) -> Result<(String, bool), CoreError> {
        Err(CoreError::Unsupported { op: "collect_text", needs: "AXUIElementCopyAttributeValue" })
    }

    pub fn element_from_point(&self, _x: i32, _y: i32) -> Result<Option<UiaElement>, CoreError> {
        Err(CoreError::Unsupported { op: "element_from_point", needs: "AXUIElementCopyElementAtPosition" })
    }
}

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub name: String,
    pub pid: u32,
    pub focused: bool,
    pub hwnd: isize,
    pub state: &'static str,
}

pub enum WindowState {
    Maximize,
    Minimize,
    Restore,
    Close,
}

impl WindowState {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "maximize" => Some(Self::Maximize),
            "minimize" => Some(Self::Minimize),
            "restore" => Some(Self::Restore),
            "close" => Some(Self::Close),
            _ => None,
        }
    }
}

pub fn list_windows() -> Result<Vec<WindowInfo>, CoreError> {
    Err(CoreError::Unsupported { op: "list_windows", needs: "AXUIElementCopyAttributeValue (kAXWindowsAttribute)" })
}

pub fn focus_window(_name: &str) -> Result<(), CoreError> {
    Err(CoreError::Unsupported { op: "focus_window", needs: "AXUIElementSetAttributeValue (kAXMainAttribute)" })
}

pub fn set_window_state(_name: &str, _state: WindowState) -> Result<(), CoreError> {
    Err(CoreError::Unsupported { op: "set_window_state", needs: "AXUIElementSetAttributeValue" })
}

pub fn focus_window_under_point(_x: i32, _y: i32) -> Result<bool, CoreError> {
    Err(CoreError::Unsupported { op: "focus_window_under_point", needs: "AXUIElementCopyElementAtPosition" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_state_from_str_parses_all_variants() {
        assert!(matches!(WindowState::from_str("maximize"), Some(WindowState::Maximize)));
        assert!(matches!(WindowState::from_str("close"), Some(WindowState::Close)));
        assert!(WindowState::from_str("invalid").is_none());
    }

    #[test]
    fn role_alias_tab_matches_tabitem() {
        assert!(role_alias_matches("tab", "tabitem"));
        assert!(!role_alias_matches("tab", "button"));
    }

    #[test]
    fn uia_tree_constructs_even_though_queries_are_unsupported() {
        let tree = UiaTree::new().expect("UiaTree::new must succeed so GhostSession can construct");
        assert!(matches!(tree.find_by_name_fast("x"), Err(CoreError::Unsupported { .. })));
    }
}
