//! Element actions (invoke/set-value/selection).
//!
//! On Windows these call UIA patterns (`IUIAutomationInvokePattern`, etc.) on
//! a live `IUIAutomationElement`, with a coordinate-click/keyboard fallback
//! when a pattern is missing. macOS has no live element handle here at all
//! (see `element.rs`) and no pattern equivalent without AXUIElement, so every
//! action honestly refuses. `is_editable_role` is pure data (delegates to
//! `element::role_id_to_name`) and is ported for real.

use super::element::UiaElement;
use crate::error::CoreError;

pub fn invoke(element: &UiaElement) -> Result<(), CoreError> {
    invoke_ex(element, true)
}

pub fn invoke_ex(_element: &UiaElement, _allow_fallback: bool) -> Result<(), CoreError> {
    Err(CoreError::Unsupported { op: "invoke", needs: "AXUIElementPerformAction" })
}

pub fn get_selection(_element: &UiaElement) -> Result<String, CoreError> {
    Err(CoreError::Unsupported { op: "get_selection", needs: "AXUIElementCopyAttributeValue" })
}

pub fn set_value(element: &UiaElement, value: &str) -> Result<(), CoreError> {
    set_value_ex(element, value, true)
}

pub fn set_value_ex(_element: &UiaElement, _value: &str, _allow_fallback: bool) -> Result<(), CoreError> {
    Err(CoreError::Unsupported { op: "set_value", needs: "AXUIElementSetAttributeValue" })
}

/// True for roles that accept typed text. Pure data -- see the module docs.
pub fn is_editable_role(control_type: u32) -> bool {
    matches!(
        super::element::role_id_to_name(control_type),
        "edit" | "document" | "combobox"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_editable_role_matches_edit_document_combobox() {
        assert!(is_editable_role(50004)); // edit
        assert!(is_editable_role(50030)); // document
        assert!(is_editable_role(50003)); // combobox
        assert!(!is_editable_role(50000)); // button
    }
}
