use windows::Win32::UI::Accessibility::*;
use windows::core::Interface;
use super::element::UiaElement;
use crate::error::CoreError;

/// Invoke an element via InvokePattern (buttons, links).
/// Falls back to clicking center of bounding rect if InvokePattern unavailable.
pub fn invoke(element: &UiaElement) -> Result<(), CoreError> {
    invoke_ex(element, true)
}

/// Invoke via InvokePattern. When `allow_fallback` is false (background mode),
/// this NEVER falls back to a coordinate click — a coordinate click needs the
/// window foreground and moves the real cursor, defeating background dispatch.
/// If the element has no InvokePattern, returns `NotActionableInBackground`.
pub fn invoke_ex(element: &UiaElement, allow_fallback: bool) -> Result<(), CoreError> {
    unsafe {
        if let Ok(pattern) = element.0.GetCurrentPattern(UIA_InvokePatternId) {
            let invoke: IUIAutomationInvokePattern = pattern.cast()
                .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "InvokePattern cast" })?;
            invoke.Invoke()
                .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "InvokePattern.Invoke" })?;
            return Ok(());
        }
    }
    if !allow_fallback {
        return Err(CoreError::NotActionableInBackground {
            what: "click (no InvokePattern; a coordinate click needs foreground)",
        });
    }
    // Coordinate fallback
    if let Some(rect) = element.bounding_rect() {
        let (cx, cy) = rect.center();
        crate::input::mouse::click(cx, cy)?;
    }
    Ok(())
}

/// Read the current text SELECTION of an element via TextPattern
/// (`GetSelection`). Returns the concatenated selected text across all ranges,
/// or an empty string when nothing is selected or the element doesn't expose
/// TextPattern. Lets an agent confirm/read a selection WITHOUT clobbering the
/// clipboard via Ctrl+C. Works well on native edit/RichEdit/document controls;
/// many Chromium/browser controls don't expose TextPattern faithfully.
pub fn get_selection(element: &UiaElement) -> Result<String, CoreError> {
    unsafe {
        let pattern = match element.0.GetCurrentPattern(UIA_TextPatternId) {
            Ok(p) => p,
            Err(_) => return Ok(String::new()),
        };
        let tp: IUIAutomationTextPattern = match pattern.cast() {
            Ok(t) => t,
            Err(_) => return Ok(String::new()),
        };
        let ranges = match tp.GetSelection() {
            Ok(r) => r,
            Err(_) => return Ok(String::new()),
        };
        let count = ranges.Length().unwrap_or(0);
        let mut out = String::new();
        for i in 0..count {
            if let Ok(range) = ranges.GetElement(i) {
                if let Ok(bstr) = range.GetText(-1) {
                    out.push_str(&bstr.to_string());
                }
            }
        }
        Ok(out)
    }
}

/// Set value via ValuePattern (text inputs).
/// Falls back to clicking + typing if ValuePattern unavailable.
pub fn set_value(element: &UiaElement, value: &str) -> Result<(), CoreError> {
    set_value_ex(element, value, true)
}

/// Set value via ValuePattern. When `allow_fallback` is false (background mode),
/// this NEVER falls back to click+keyboard (which needs foreground and moves the
/// cursor). If the element has no ValuePattern, returns `NotActionableInBackground`.
pub fn set_value_ex(element: &UiaElement, value: &str, allow_fallback: bool) -> Result<(), CoreError> {
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
    if !allow_fallback {
        return Err(CoreError::NotActionableInBackground {
            what: "type (no ValuePattern; keyboard entry needs foreground)",
        });
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
