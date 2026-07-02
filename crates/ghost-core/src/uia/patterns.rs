use windows::Win32::UI::Accessibility::*;
use windows::core::Interface;
use super::element::UiaElement;
use crate::error::CoreError;

/// Invoke an element via InvokePattern (buttons, links).
/// Falls back to clicking center of bounding rect if InvokePattern unavailable.
pub fn invoke(element: &UiaElement) -> Result<(), CoreError> {
    unsafe {
        if let Ok(pattern) = element.0.GetCurrentPattern(UIA_InvokePatternId) {
            let invoke: IUIAutomationInvokePattern = pattern.cast()
                .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "InvokePattern cast" })?;
            invoke.Invoke()
                .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "InvokePattern.Invoke" })?;
            return Ok(());
        }
    }
    // Coordinate fallback
    if let Some(rect) = element.bounding_rect() {
        let (cx, cy) = rect.center();
        crate::input::mouse::click(cx, cy)?;
    }
    Ok(())
}

/// Set value via ValuePattern (text inputs).
/// Falls back to clicking + typing if ValuePattern unavailable.
pub fn set_value(element: &UiaElement, value: &str) -> Result<(), CoreError> {
    unsafe {
        if let Ok(pattern) = element.0.GetCurrentPattern(UIA_ValuePatternId) {
            let vp: IUIAutomationValuePattern = pattern.cast()
                .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "ValuePattern cast" })?;
            let bstr = windows::core::BSTR::from(value);
            vp.SetValue(&bstr)
                .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "ValuePattern.SetValue" })?;
            return Ok(());
        }
    }
    // Fallback: click to focus, clear existing content, then type. Clearing
    // matches ValuePattern.SetValue's replace semantics — without it the
    // keyboard path appends at the cursor instead of replacing the field.
    if let Some(rect) = element.bounding_rect() {
        let (cx, cy) = rect.center();
        crate::input::mouse::click(cx, cy)?;
    }
    // Only clear (Ctrl+A + Delete) when we're confident focus is a text field.
    // On a non-editable control (button, list, or the desktop/Explorer) those
    // keystrokes could select-all + delete files — a destructive mis-fire.
    if is_editable_role(element.control_type()) {
        let _ = crate::input::keyboard::clear_focused_field();
    }
    crate::input::keyboard::type_text(value)
}

/// True for UIA control types that accept typed text (safe to Ctrl+A+Delete).
pub fn is_editable_role(control_type: u32) -> bool {
    matches!(
        super::element::role_id_to_name(control_type),
        "edit" | "document" | "combobox"
    )
}
