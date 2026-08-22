//! Element shapes and the role vocabulary.
//!
//! `role_id_to_name`, `is_editable_role` (in `patterns.rs`) and
//! `INTERACTIVE_ROLES` are pure lookup tables -- no OS call, just data -- so
//! they are ported for real, in the same UIA control-type-id numeric space
//! Windows uses (`ghost-linux` makes the identical choice for AT-SPI roles,
//! for the same reason: an agent's `role="button"` must mean the same thing
//! on every platform). `UiaElement` itself cannot be ported for real: it
//! wraps a live accessibility handle (`IUIAutomationElement` on Windows), and
//! macOS has no such handle without AXUIElement. It is kept as an inert
//! placeholder purely so the type exists for the type checker -- nothing in
//! this crate ever constructs one (`UiaTree::find_by_name_fast` and friends
//! all return `Err(Unsupported)` before they would need to), so its methods
//! are unreachable in practice, not silently wrong.

#[derive(Debug, Clone)]
pub struct BoundingRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl BoundingRect {
    pub fn center(&self) -> (i32, i32) {
        ((self.left + self.right) / 2, (self.top + self.bottom) / 2)
    }
}

/// Never constructed by real logic in this crate (every `UiaTree` query
/// returns `Err` before it would need to) -- see the module docs. A plain
/// public unit struct rather than a private-field placeholder, so the
/// compile-time API-surface proof in `tests/api_surface.rs` can construct one
/// to exercise `patterns::invoke`/`set_value`/etc without needing crate-internal
/// access.
#[derive(Debug, Default)]
pub struct UiaElement;

impl UiaElement {
    pub fn name(&self) -> String {
        String::new()
    }

    pub fn control_type(&self) -> u32 {
        0
    }

    pub fn bounding_rect(&self) -> Option<BoundingRect> {
        None
    }

    pub fn is_enabled(&self) -> bool {
        false
    }

    pub fn is_offscreen(&self) -> bool {
        false
    }

    /// No-op success, mirroring the Windows implementation's own
    /// "unsupported pattern -> Ok, caller proceeds regardless" contract --
    /// not a new leniency invented for macOS.
    pub fn scroll_into_view(&self) -> Result<(), crate::error::CoreError> {
        Ok(())
    }

    pub fn set_focus(&self) -> Result<(), crate::error::CoreError> {
        Err(crate::error::CoreError::Unsupported { op: "set_focus", needs: "AXUIElementSetAttributeValue" })
    }

    pub fn native_window_handle(&self) -> isize {
        0
    }

    pub fn get_text(&self) -> String {
        String::new()
    }
}

/// Identical to `ghost_core::uia::element::role_id_to_name` / the
/// `ghost_linux` mirror of the same table -- see the module docs.
pub fn role_id_to_name(id: u32) -> &'static str {
    match id {
        50000 => "button",
        50001 => "calendar",
        50002 => "checkbox",
        50003 => "combobox",
        50004 => "edit",
        50005 => "hyperlink",
        50006 => "image",
        50007 => "listitem",
        50008 => "list",
        50009 => "menu",
        50010 => "menubar",
        50011 => "menuitem",
        50012 => "progressbar",
        50013 => "radiobutton",
        50014 => "scrollbar",
        50015 => "slider",
        50016 => "spinner",
        50017 => "statusbar",
        50018 => "tab",
        50019 => "tabitem",
        50020 => "text",
        50021 => "toolbar",
        50022 => "tooltip",
        50023 => "tree",
        50024 => "treeitem",
        50025 => "custom",
        50026 => "group",
        50027 => "thumb",
        50028 => "datagrid",
        50029 => "dataitem",
        50030 => "document",
        50031 => "splitbutton",
        50032 => "window",
        50033 => "pane",
        50034 => "header",
        50035 => "headeritem",
        50036 => "table",
        50037 => "titlebar",
        50038 => "separator",
        _ => "unknown",
    }
}

pub const INTERACTIVE_ROLES: &[&str] = &[
    "button", "edit", "checkbox", "combobox", "menu", "menuitem",
    "tab", "tabitem", "list", "listitem", "toolbar", "radiobutton",
    "hyperlink", "treeitem", "document",
    "splitbutton", "dataitem",
];

#[derive(Debug, Clone)]
pub struct ElementDescriptor {
    pub name: String,
    pub role: String,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_button_is_50000() {
        assert_eq!(role_id_to_name(50000), "button");
    }

    #[test]
    fn unknown_role_returns_unknown() {
        assert_eq!(role_id_to_name(99999), "unknown");
    }

    #[test]
    fn bounding_rect_center_is_correct() {
        let r = BoundingRect { left: 100, top: 200, right: 300, bottom: 400 };
        assert_eq!(r.center(), (200, 300));
    }

    #[test]
    fn interactive_roles_include_button_and_edit() {
        assert!(INTERACTIVE_ROLES.contains(&"button"));
        assert!(INTERACTIVE_ROLES.contains(&"edit"));
    }
}
