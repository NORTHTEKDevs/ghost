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
pub fn role_id_to_name(id: u32) -> &'static str {
    match id {
        50 => "button",
        42 => "edit",
        50023 => "document",
        50004 => "checkbox",
        50034 => "list",
        50008 => "menu",
        50020 => "tab",
        50033 => "toolbar",
        50021 => "text",
        50025 => "window",
        50030 => "pane",
        _ => "unknown",
    }
}

/// Roles included in describe_screen output.
pub const INTERACTIVE_ROLES: &[&str] = &[
    "button", "edit", "checkbox", "menu", "tab", "list", "toolbar",
];

#[derive(Debug, Clone)]
pub struct ElementDescriptor {
    pub name: String,
    pub role: String,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_id_button_maps_correctly() {
        assert_eq!(role_id_to_name(50), "button");
    }

    #[test]
    fn role_id_edit_maps_correctly() {
        assert_eq!(role_id_to_name(42), "edit");
    }

    #[test]
    fn unknown_role_returns_unknown() {
        assert_eq!(role_id_to_name(99999), "unknown");
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
}
