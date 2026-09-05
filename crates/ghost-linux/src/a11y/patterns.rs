//! Interface/pattern operations, mirroring `ghost_core::uia::patterns`.
//!
//! On Windows these drive UIA control patterns (Invoke, Value, Text). On Linux
//! the equivalents are AT-SPI interfaces: `Action`, `EditableText`, `Value` and
//! `Text`. The signatures are identical so `ghost-session` calls them unchanged.
//!
//! The `_ex` variants take `allow_fallback`. On Windows that permits dropping
//! from a pattern to a synthetic click when the pattern is missing. The same
//! meaning applies here: with fallback disabled, an element that exposes no
//! usable AT-SPI action is an error rather than a coordinate click -- which
//! matters because a coordinate click moves the pointer and breaks the
//! no-focus-steal guarantee.

use super::roles::role_id_to_name;
#[cfg(target_os = "linux")]
use crate::error::Result;

#[cfg(target_os = "linux")]
use super::element::A11yElement;

/// Mirrors `ghost_core::uia::patterns::UIA_E_ELEMENTNOTAVAILABLE`. The Windows
/// engine reports a stale element with this HRESULT; the shared session layer
/// matches on it. AT-SPI reports staleness as a D-Bus error instead, so this
/// value is never produced here - it only has to exist.
pub const UIA_E_ELEMENTNOTAVAILABLE: u32 = 0x8004_0201;

/// Identical to `ghost_core::uia::patterns::is_interactive_control`.
pub fn is_interactive_control(control_type: u32) -> bool {
    super::roles::is_interactive_role(role_id_to_name(control_type))
}

/// Identical to `ghost_core::uia::patterns::role_name`.
pub fn role_name(control_type: u32) -> &'static str {
    role_id_to_name(control_type)
}

/// Identical to `ghost_core::uia::patterns::is_editable_role`.
pub fn is_editable_role(control_type: u32) -> bool {
    matches!(role_id_to_name(control_type), "edit" | "document" | "combobox")
}

/// Roles that a click should target directly rather than descending into.
pub fn is_leaf_actionable_role(control_type: u32) -> bool {
    matches!(
        role_id_to_name(control_type),
        "button"
            | "checkbox"
            | "radiobutton"
            | "menuitem"
            | "listitem"
            | "treeitem"
            | "tabitem"
            | "hyperlink"
            | "splitbutton"
            | "dataitem"
    )
}

#[cfg(target_os = "linux")]
pub fn invoke(element: &A11yElement) -> Result<()> {
    invoke_ex(element, true)
}

#[cfg(target_os = "linux")]
pub fn invoke_ex(element: &A11yElement, allow_fallback: bool) -> Result<()> {
    match element.invoke() {
        Ok(_) => Ok(()),
        Err(e) => {
            if !allow_fallback {
                return Err(e);
            }
            // Fallback: focus then activate with the keyboard. Deliberately not
            // a coordinate click -- that would move the pointer and break the
            // guarantee that background dispatch disturbs nothing.
            element.set_focus()?;
            crate::input::press_key(crate::keysym::VirtualKey(0xFF0D)) // Return
        }
    }
}

#[cfg(target_os = "linux")]
pub fn set_value(element: &A11yElement, value: &str) -> Result<()> {
    set_value_ex(element, value, true)
}

#[cfg(target_os = "linux")]
pub fn set_value_ex(element: &A11yElement, value: &str, allow_fallback: bool) -> Result<()> {
    match element.set_value(value) {
        Ok(_) => Ok(()),
        Err(e) => {
            if !allow_fallback {
                return Err(e);
            }
            // Fallback: focus, clear, and type. Needs the element focused, so
            // it is a real behaviour change from the direct path -- which is
            // exactly why it is gated behind `allow_fallback`.
            element.set_focus()?;

            // Only clear (Ctrl+A + Delete) when we are confident focus is a text
            // field. This guard mirrors the Windows engine's and exists for the
            // same reason: if the locator resolved to a non-editable control --
            // a file-manager pane, a list, the desktop -- those two keystrokes
            // are select-all + delete, i.e. real data loss. Typing into the
            // wrong place is recoverable; deleting the user's files is not.
            if is_editable_role(element.control_type()) {
                let _ = crate::input::clear_focused_field();
            }
            crate::input::type_text(value)
        }
    }
}

#[cfg(target_os = "linux")]
pub fn get_selection(element: &A11yElement) -> Result<String> {
    element.selected_text()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editable_roles_match_the_windows_engine() {
        assert!(is_editable_role(50004)); // edit
        assert!(is_editable_role(50030)); // document
        assert!(is_editable_role(50003)); // combobox
        assert!(!is_editable_role(50000)); // button
    }

    #[test]
    fn leaf_actionable_covers_split_button_and_data_item() {
        assert!(is_leaf_actionable_role(50031));
        assert!(is_leaf_actionable_role(50029));
        assert!(!is_leaf_actionable_role(50033)); // pane
    }
}
