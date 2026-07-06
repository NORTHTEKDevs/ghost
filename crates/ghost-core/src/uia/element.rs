use windows::Win32::UI::Accessibility::IUIAutomationElement;
use windows::core::Interface;

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

pub struct UiaElement(pub IUIAutomationElement);

impl UiaElement {
    pub fn name(&self) -> String {
        unsafe {
            self.0
                .CurrentName()
                .map(|s| s.to_string())
                .unwrap_or_default()
        }
    }

    pub fn control_type(&self) -> u32 {
        unsafe {
            self.0
                .CurrentControlType()
                .map(|ct| ct.0 as u32)
                .unwrap_or(0)
        }
    }

    pub fn bounding_rect(&self) -> Option<BoundingRect> {
        unsafe {
            self.0.CurrentBoundingRectangle().ok().map(|r| BoundingRect {
                left: r.left,
                top: r.top,
                right: r.right,
                bottom: r.bottom,
            })
        }
    }

    pub fn is_enabled(&self) -> bool {
        unsafe { self.0.CurrentIsEnabled().map(|b| b.as_bool()).unwrap_or(false) }
    }

    /// True if the element is off-screen (UIA IsOffscreen: scrolled out of view,
    /// collapsed, or on a hidden tab). Such elements have stale bounding rects.
    pub fn is_offscreen(&self) -> bool {
        unsafe { self.0.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) }
    }

    /// Best-effort scroll-into-view via ScrollItemPattern. Elements inside a
    /// scrolled container (long lists, virtualized grids) report a rect that's
    /// off-screen until scrolled to; without this a click lands on empty space.
    /// No-op (Ok) when the pattern is unavailable — caller proceeds regardless.
    pub fn scroll_into_view(&self) -> Result<(), crate::error::CoreError> {
        use windows::Win32::UI::Accessibility::{
            IUIAutomationScrollItemPattern, UIA_ScrollItemPatternId,
        };
        unsafe {
            if let Ok(pattern) = self.0.GetCurrentPattern(UIA_ScrollItemPatternId) {
                if let Ok(sip) = pattern.cast::<IUIAutomationScrollItemPattern>() {
                    let _ = sip.ScrollIntoView();
                }
            }
        }
        Ok(())
    }

    /// Set UIA focus to this element via IUIAutomationElement::SetFocus().
    pub fn set_focus(&self) -> Result<(), crate::error::CoreError> {
        unsafe {
            self.0.SetFocus()
                .map_err(|e| crate::error::CoreError::Win32 {
                    code: e.code().0 as u32,
                    context: "IUIAutomationElement::SetFocus",
                })
        }
    }

    /// The element's native Win32 window handle (HWND) as isize, or 0 if the
    /// element is windowless (UWP/WinUI XAML controls, most Chromium content).
    /// Standard Win32 controls (buttons, edits, list items in classic apps)
    /// return their own control HWND — the handle needed to drive them with
    /// window messages (background dispatch) instead of UIA patterns.
    pub fn native_window_handle(&self) -> isize {
        unsafe {
            self.0.CurrentNativeWindowHandle()
                .map(|h| h.0 as isize)
                .unwrap_or(0)
        }
    }

    /// Get the current text value. Tries ValuePattern first, falls back to element name.
    pub fn get_text(&self) -> String {
        use windows::Win32::UI::Accessibility::{
            IUIAutomationValuePattern, UIA_ValuePatternId,
        };
        unsafe {
            if let Ok(pattern) = self.0.GetCurrentPattern(UIA_ValuePatternId) {
                if let Ok(vp) = pattern.cast::<IUIAutomationValuePattern>() {
                    if let Ok(val) = vp.CurrentValue() {
                        return val.to_string();
                    }
                }
            }
            self.name()
        }
    }
}

/// Map UIA control type IDs to human-readable role names.
/// Real UIA control-type IDs (UIA_*ControlTypeId constants).
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

/// Roles included in describe_screen output.
pub const INTERACTIVE_ROLES: &[&str] = &[
    "button", "edit", "checkbox", "combobox", "menu", "menuitem",
    "tab", "tabitem", "list", "listitem", "toolbar", "radiobutton",
    "hyperlink", "treeitem", "document",
];

#[derive(Debug, Clone)]
pub struct ElementDescriptor {
    pub name: String,
    pub role: String,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    /// UIA IsEnabled — false for greyed-out controls. Lets an agent avoid trying
    /// to click a disabled button/field. Defaults to true when unknown.
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Corrected role ID tests (real UIA control-type IDs)
    #[test]
    fn role_button_is_50000() {
        assert_eq!(role_id_to_name(50000), "button");
    }

    #[test]
    fn role_edit_is_50004() {
        assert_eq!(role_id_to_name(50004), "edit");
    }

    #[test]
    fn role_checkbox_is_50002() {
        assert_eq!(role_id_to_name(50002), "checkbox");
    }

    #[test]
    fn role_tabitem_is_50019() {
        assert_eq!(role_id_to_name(50019), "tabitem");
    }

    #[test]
    fn role_legacy_50_is_unknown() {
        // Old bogus ID must now return unknown
        assert_eq!(role_id_to_name(50), "unknown");
    }

    #[test]
    fn unknown_role_returns_unknown() {
        assert_eq!(role_id_to_name(99999), "unknown");
    }

    #[test]
    fn role_combobox_is_50003() {
        assert_eq!(role_id_to_name(50003), "combobox");
    }

    #[test]
    fn role_radiobutton_is_50013() {
        assert_eq!(role_id_to_name(50013), "radiobutton");
    }

    #[test]
    fn role_document_is_50030() {
        assert_eq!(role_id_to_name(50030), "document");
    }

    #[test]
    fn role_window_is_50032() {
        assert_eq!(role_id_to_name(50032), "window");
    }

    #[test]
    fn bounding_rect_center_is_correct() {
        let r = BoundingRect {
            left: 100,
            top: 200,
            right: 300,
            bottom: 400,
        };
        assert_eq!(r.center(), (200, 300));
    }

    #[test]
    fn interactive_roles_include_button_and_edit() {
        assert!(INTERACTIVE_ROLES.contains(&"button"));
        assert!(INTERACTIVE_ROLES.contains(&"edit"));
    }

    #[test]
    fn interactive_roles_include_new_types() {
        assert!(INTERACTIVE_ROLES.contains(&"combobox"));
        assert!(INTERACTIVE_ROLES.contains(&"radiobutton"));
        assert!(INTERACTIVE_ROLES.contains(&"hyperlink"));
        assert!(INTERACTIVE_ROLES.contains(&"listitem"));
        assert!(INTERACTIVE_ROLES.contains(&"menuitem"));
        assert!(INTERACTIVE_ROLES.contains(&"tabitem"));
        assert!(INTERACTIVE_ROLES.contains(&"treeitem"));
        assert!(INTERACTIVE_ROLES.contains(&"document"));
    }
}
