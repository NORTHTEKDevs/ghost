//! UIA control patterns - the primary background automation path.
//!
//! A UIA control pattern call goes straight to the target application's automation
//! provider. It does not move the cursor, does not change keyboard focus, and does
//! not raise the window, so it composes freely with the user working on the same
//! machine and with other ghost processes driving other windows.
//!
//! Each public action tries a *chain* of patterns, most specific first, and only
//! falls back to real input (`SendInput`) when the focus policy allows it. Under the
//! default `Background` policy a target that supports no pattern fails loudly with
//! `BackgroundUnsupported` rather than silently grabbing the screen.

use super::element::UiaElement;
use crate::error::CoreError;
use crate::focus;
use windows::core::Interface;
use windows::Win32::UI::Accessibility::*;

/// How an action was actually carried out. Returned so callers (and the MCP layer)
/// can report honestly whether the screen was touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionRoute {
    Invoke,
    Toggle,
    SelectionItem,
    ExpandCollapse,
    LegacyDefaultAction,
    ValuePattern,
    LegacySetValue,
    ScrollPattern,
    RangeValue,
    /// Real input was injected: the cursor/keyboard was used.
    Foreground,
}

impl ActionRoute {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActionRoute::Invoke => "uia:invoke",
            ActionRoute::Toggle => "uia:toggle",
            ActionRoute::SelectionItem => "uia:selection_item",
            ActionRoute::ExpandCollapse => "uia:expand_collapse",
            ActionRoute::LegacyDefaultAction => "uia:legacy_default_action",
            ActionRoute::ValuePattern => "uia:value",
            ActionRoute::LegacySetValue => "uia:legacy_set_value",
            ActionRoute::ScrollPattern => "uia:scroll",
            ActionRoute::RangeValue => "uia:range_value",
            ActionRoute::Foreground => "foreground:sendinput",
        }
    }

    /// True when this route did not touch the user's cursor, focus, or foreground.
    pub fn is_background(&self) -> bool {
        !matches!(self, ActionRoute::Foreground)
    }
}

fn pattern<T: Interface>(el: &UiaElement, id: UIA_PATTERN_ID) -> Option<T> {
    unsafe { el.0.GetCurrentPattern(id).ok().and_then(|p| p.cast::<T>().ok()) }
}

// ---------------------------------------------------------------------------
// Activation (the "click" family)
// ---------------------------------------------------------------------------

/// Attempt every background activation pattern in order. `Ok(None)` means the
/// element exposes no background activation path (caller decides what to do next).
pub fn try_background_activate(el: &UiaElement) -> Result<Option<ActionRoute>, CoreError> {
    unsafe {
        if let Some(p) = pattern::<IUIAutomationInvokePattern>(el, UIA_InvokePatternId) {
            if p.Invoke().is_ok() {
                return Ok(Some(ActionRoute::Invoke));
            }
        }
        // Tabs, list items, radio buttons: activation means "select me".
        if let Some(p) =
            pattern::<IUIAutomationSelectionItemPattern>(el, UIA_SelectionItemPatternId)
        {
            if p.Select().is_ok() {
                return Ok(Some(ActionRoute::SelectionItem));
            }
        }
        // Checkboxes and toggle buttons rarely support Invoke.
        if let Some(p) = pattern::<IUIAutomationTogglePattern>(el, UIA_TogglePatternId) {
            if p.Toggle().is_ok() {
                return Ok(Some(ActionRoute::Toggle));
            }
        }
        // Combo boxes, tree items, split buttons: activation means "open me".
        if let Some(p) =
            pattern::<IUIAutomationExpandCollapsePattern>(el, UIA_ExpandCollapsePatternId)
        {
            let state = p.CurrentExpandCollapseState().unwrap_or(ExpandCollapseState_LeafNode);
            let ok = if state == ExpandCollapseState_Expanded {
                p.Collapse().is_ok()
            } else {
                p.Expand().is_ok()
            };
            if ok {
                return Ok(Some(ActionRoute::ExpandCollapse));
            }
        }
        // MSAA bridge: covers a large tail of old Win32/WinForms controls that
        // expose no modern pattern at all.
        if let Some(p) =
            pattern::<IUIAutomationLegacyIAccessiblePattern>(el, UIA_LegacyIAccessiblePatternId)
        {
            if p.DoDefaultAction().is_ok() {
                return Ok(Some(ActionRoute::LegacyDefaultAction));
            }
        }
    }
    Ok(None)
}

/// Activate an element ("click" it), honouring the focus policy.
pub fn invoke(element: &UiaElement) -> Result<ActionRoute, CoreError> {
    if let Some(route) = try_background_activate(element)? {
        return Ok(route);
    }
    coordinate_fallback(element, "click", |cx, cy| crate::input::mouse::click(cx, cy))
}

/// Explicit toggle. Errors if the element is not toggleable.
pub fn toggle(element: &UiaElement) -> Result<ActionRoute, CoreError> {
    if let Some(p) = pattern::<IUIAutomationTogglePattern>(element, UIA_TogglePatternId) {
        unsafe {
            p.Toggle().map_err(|e| CoreError::Win32 {
                code: e.code().0 as u32,
                context: "TogglePattern.Toggle",
            })?;
        }
        return Ok(ActionRoute::Toggle);
    }
    Err(CoreError::BackgroundUnsupported { action: "toggle", hwnd: 0 })
}

/// Explicit selection (tab items, list items, radio buttons).
pub fn select(element: &UiaElement) -> Result<ActionRoute, CoreError> {
    if let Some(p) =
        pattern::<IUIAutomationSelectionItemPattern>(element, UIA_SelectionItemPatternId)
    {
        unsafe {
            p.Select().map_err(|e| CoreError::Win32 {
                code: e.code().0 as u32,
                context: "SelectionItemPattern.Select",
            })?;
        }
        return Ok(ActionRoute::SelectionItem);
    }
    Err(CoreError::BackgroundUnsupported { action: "select", hwnd: 0 })
}

/// Expand or collapse a combo box / tree item / split button.
pub fn expand_collapse(element: &UiaElement, expand: bool) -> Result<ActionRoute, CoreError> {
    if let Some(p) =
        pattern::<IUIAutomationExpandCollapsePattern>(element, UIA_ExpandCollapsePatternId)
    {
        unsafe {
            let r = if expand { p.Expand() } else { p.Collapse() };
            r.map_err(|e| CoreError::Win32 {
                code: e.code().0 as u32,
                context: "ExpandCollapsePattern",
            })?;
        }
        return Ok(ActionRoute::ExpandCollapse);
    }
    Err(CoreError::BackgroundUnsupported { action: "expand_collapse", hwnd: 0 })
}

// ---------------------------------------------------------------------------
// Text entry
// ---------------------------------------------------------------------------

/// Attempt every background text-entry pattern. `Ok(None)` means no background path.
pub fn try_background_set_value(el: &UiaElement, value: &str) -> Result<Option<ActionRoute>, CoreError> {
    unsafe {
        if let Some(p) = pattern::<IUIAutomationValuePattern>(el, UIA_ValuePatternId) {
            // A read-only ValuePattern silently no-ops on SetValue in some providers,
            // which would look like success. Skip it so the chain keeps searching.
            let read_only = p.CurrentIsReadOnly().map(|b| b.as_bool()).unwrap_or(false);
            if !read_only {
                let bstr = windows::core::BSTR::from(value);
                if p.SetValue(&bstr).is_ok() {
                    return Ok(Some(ActionRoute::ValuePattern));
                }
            }
        }
        if let Some(p) =
            pattern::<IUIAutomationLegacyIAccessiblePattern>(el, UIA_LegacyIAccessiblePatternId)
        {
            let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
            if p.SetValue(windows::core::PCWSTR(wide.as_ptr())).is_ok() {
                return Ok(Some(ActionRoute::LegacySetValue));
            }
        }
    }
    Ok(None)
}

/// Set an element's text, honouring the focus policy.
pub fn set_value(element: &UiaElement, value: &str) -> Result<ActionRoute, CoreError> {
    if let Some(route) = try_background_set_value(element, value)? {
        return Ok(route);
    }
    // No background path: fall back to click-then-type, policy permitting.
    focus::require_foreground_allowed("type")?;
    crate::system::with_foreground_lease(30_000, || {
        if let Some(rect) = element.bounding_rect() {
            let (cx, cy) = rect.center();
            crate::input::mouse::click(cx, cy)?;
        }
        crate::input::keyboard::type_text(value)?;
        Ok(ActionRoute::Foreground)
    })
}

// ---------------------------------------------------------------------------
// Scrolling and ranges
// ---------------------------------------------------------------------------

/// Scroll a scrollable container in the background. `direction` is
/// "up"/"down"/"left"/"right"; `amount` counts large increments.
pub fn scroll(element: &UiaElement, direction: &str, amount: i32) -> Result<ActionRoute, CoreError> {
    let p = pattern::<IUIAutomationScrollPattern>(element, UIA_ScrollPatternId)
        .ok_or(CoreError::BackgroundUnsupported { action: "scroll", hwnd: 0 })?;
    let (h, v) = match direction {
        "up" => (ScrollAmount_NoAmount, ScrollAmount_LargeDecrement),
        "down" => (ScrollAmount_NoAmount, ScrollAmount_LargeIncrement),
        "left" => (ScrollAmount_LargeDecrement, ScrollAmount_NoAmount),
        "right" => (ScrollAmount_LargeIncrement, ScrollAmount_NoAmount),
        _ => return Err(CoreError::Win32 { code: 0, context: "invalid scroll direction" }),
    };
    unsafe {
        for _ in 0..amount.max(1) {
            p.Scroll(h, v).map_err(|e| CoreError::Win32 {
                code: e.code().0 as u32,
                context: "ScrollPattern.Scroll",
            })?;
        }
    }
    Ok(ActionRoute::ScrollPattern)
}

/// Bring an element into view without moving the cursor or focusing the window.
pub fn scroll_into_view(element: &UiaElement) -> Result<ActionRoute, CoreError> {
    let p = pattern::<IUIAutomationScrollItemPattern>(element, UIA_ScrollItemPatternId)
        .ok_or(CoreError::BackgroundUnsupported { action: "scroll_into_view", hwnd: 0 })?;
    unsafe {
        p.ScrollIntoView().map_err(|e| CoreError::Win32 {
            code: e.code().0 as u32,
            context: "ScrollItemPattern.ScrollIntoView",
        })?;
    }
    Ok(ActionRoute::ScrollPattern)
}

/// Set a slider / spinner / progress value in the background.
pub fn set_range_value(element: &UiaElement, value: f64) -> Result<ActionRoute, CoreError> {
    let p = pattern::<IUIAutomationRangeValuePattern>(element, UIA_RangeValuePatternId)
        .ok_or(CoreError::BackgroundUnsupported { action: "set_range_value", hwnd: 0 })?;
    unsafe {
        p.SetValue(value).map_err(|e| CoreError::Win32 {
            code: e.code().0 as u32,
            context: "RangeValuePattern.SetValue",
        })?;
    }
    Ok(ActionRoute::RangeValue)
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Full text of a document/edit via TextPattern, which reads far more than
/// ValuePattern (whole document body rather than a single control value).
pub fn document_text(element: &UiaElement, max_chars: i32) -> Option<String> {
    let p = pattern::<IUIAutomationTextPattern>(element, UIA_TextPatternId)?;
    unsafe {
        let range = p.DocumentRange().ok()?;
        range.GetText(max_chars).ok().map(|s| s.to_string())
    }
}

/// Names of the background actions this element supports. Used for diagnostics and
/// for telling an agent why an action has no background path.
pub fn supported_actions(el: &UiaElement) -> Vec<&'static str> {
    let mut out = Vec::new();
    if pattern::<IUIAutomationInvokePattern>(el, UIA_InvokePatternId).is_some() {
        out.push("invoke");
    }
    if pattern::<IUIAutomationTogglePattern>(el, UIA_TogglePatternId).is_some() {
        out.push("toggle");
    }
    if pattern::<IUIAutomationSelectionItemPattern>(el, UIA_SelectionItemPatternId).is_some() {
        out.push("select");
    }
    if pattern::<IUIAutomationExpandCollapsePattern>(el, UIA_ExpandCollapsePatternId).is_some() {
        out.push("expand_collapse");
    }
    if pattern::<IUIAutomationValuePattern>(el, UIA_ValuePatternId).is_some() {
        out.push("set_value");
    }
    if pattern::<IUIAutomationScrollPattern>(el, UIA_ScrollPatternId).is_some() {
        out.push("scroll");
    }
    if pattern::<IUIAutomationScrollItemPattern>(el, UIA_ScrollItemPatternId).is_some() {
        out.push("scroll_into_view");
    }
    if pattern::<IUIAutomationRangeValuePattern>(el, UIA_RangeValuePatternId).is_some() {
        out.push("set_range_value");
    }
    if pattern::<IUIAutomationTextPattern>(el, UIA_TextPatternId).is_some() {
        out.push("document_text");
    }
    if pattern::<IUIAutomationLegacyIAccessiblePattern>(el, UIA_LegacyIAccessiblePatternId)
        .is_some()
    {
        out.push("legacy_default_action");
    }
    out
}

// ---------------------------------------------------------------------------

/// Shared tail for actions whose only remaining option is real input at the
/// element's centre. Enforces the focus policy and serializes across processes.
fn coordinate_fallback(
    element: &UiaElement,
    action: &'static str,
    f: impl FnOnce(i32, i32) -> Result<(), CoreError>,
) -> Result<ActionRoute, CoreError> {
    focus::require_foreground_allowed(action)?;
    let rect = element
        .bounding_rect()
        .ok_or(CoreError::BackgroundUnsupported { action, hwnd: 0 })?;
    let (cx, cy) = rect.center();
    crate::system::with_foreground_lease(30_000, || {
        f(cx, cy)?;
        Ok(ActionRoute::Foreground)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_routes_are_marked_background() {
        for r in [
            ActionRoute::Invoke,
            ActionRoute::Toggle,
            ActionRoute::SelectionItem,
            ActionRoute::ExpandCollapse,
            ActionRoute::LegacyDefaultAction,
            ActionRoute::ValuePattern,
            ActionRoute::LegacySetValue,
            ActionRoute::ScrollPattern,
            ActionRoute::RangeValue,
        ] {
            assert!(r.is_background(), "{} should be background", r.as_str());
        }
        assert!(!ActionRoute::Foreground.is_background());
    }

    #[test]
    fn route_names_are_prefixed_by_mechanism() {
        assert_eq!(ActionRoute::Invoke.as_str(), "uia:invoke");
        assert_eq!(ActionRoute::Foreground.as_str(), "foreground:sendinput");
    }
}
