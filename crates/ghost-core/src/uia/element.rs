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

// Safety: same MTA justification as `UiaTree`. An element handle crosses threads in
// practice because a tokio task may migrate workers between finding an element and
// acting on it; in the multithreaded apartment that is a legal COM call pattern.
unsafe impl Send for UiaElement {}

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

    /// The keyboard shortcut this element advertises, e.g. "Ctrl+Z" on Edit > Undo.
    ///
    /// This is how ghost performs a keyboard shortcut in the background: window
    /// messages cannot set modifier key state, so a posted Ctrl+Z arrives as a
    /// literal "z". Finding the command that owns the accelerator and invoking it
    /// does the real thing instead.
    pub fn accelerator_key(&self) -> String {
        unsafe {
            self.0
                .CurrentAcceleratorKey()
                .map(|s| s.to_string())
                .unwrap_or_default()
        }
    }

    /// The UIA automation id, a stable programmatic identifier where the app sets one.
    pub fn automation_id(&self) -> String {
        unsafe {
            self.0
                .CurrentAutomationId()
                .map(|s| s.to_string())
                .unwrap_or_default()
        }
    }

    /// Read a property that was pre-fetched by a cache request, falling back to a
    /// live read when this element did not come from a cached search.
    ///
    /// The cached accessors are the whole point of batching: they read from the local
    /// snapshot, so filtering a thousand elements costs no cross-process calls at all.
    pub fn cached_name(&self) -> String {
        unsafe {
            self.0
                .CachedName()
                .map(|s| s.to_string())
                .unwrap_or_else(|_| self.name())
        }
    }

    pub fn cached_control_type(&self) -> u32 {
        unsafe {
            self.0
                .CachedControlType()
                .map(|ct| ct.0 as u32)
                .unwrap_or_else(|_| self.control_type())
        }
    }

    pub fn cached_bounding_rect(&self) -> Option<BoundingRect> {
        unsafe {
            match self.0.CachedBoundingRectangle() {
                Ok(r) => Some(BoundingRect {
                    left: r.left,
                    top: r.top,
                    right: r.right,
                    bottom: r.bottom,
                }),
                Err(_) => self.bounding_rect(),
            }
        }
    }

    pub fn cached_accelerator_key(&self) -> String {
        unsafe {
            self.0
                .CachedAcceleratorKey()
                .map(|s| s.to_string())
                .unwrap_or_else(|_| self.accelerator_key())
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
///
/// These are the `UIA_*ControlTypeId` constants from UIAutomationClient.h, which run
/// contiguously from 50000. Getting them wrong is not a cosmetic problem: a bad id
/// makes `find_by_role("edit")` miss the text box entirely, which pushes callers back
/// onto pixel coordinates and the foreground.
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
        50039 => "semanticzoom",
        50040 => "appbar",
        _ => "unknown",
    }
}

/// Reverse of `role_id_to_name`: a role name to its UIA control type id.
///
/// Needed so a role lookup can be expressed as a UIA property condition and resolved
/// inside UI Automation, instead of walking the tree here and comparing strings.
pub fn role_name_to_id(role: &str) -> Option<u32> {
    let want = role.trim().to_lowercase();
    (50000..=50040).find(|id| role_id_to_name(*id) == want)
}

/// Roles included in describe_screen output.
///
/// Deliberately broad: an agent that cannot see a `listitem`, `menuitem`, or
/// `hyperlink` in the element list has no background way to act on it and falls back
/// to clicking pixels, which is the behavior this whole layer exists to remove.
pub const INTERACTIVE_ROLES: &[&str] = &[
    "button",
    "checkbox",
    "combobox",
    "edit",
    "hyperlink",
    "list",
    "listitem",
    "menu",
    "menubar",
    "menuitem",
    "radiobutton",
    "slider",
    "spinner",
    "splitbutton",
    "tab",
    "tabitem",
    "toolbar",
    "tree",
    "treeitem",
    "document",
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

    // The UIA control type ids are contiguous from 50000. Pinning the boundaries and
    // the ones automation actually depends on catches an off-by-one across the whole
    // table, which is what previously made find_by_role("edit") silently never match.
    #[test]
    fn role_ids_match_the_uia_control_type_constants() {
        assert_eq!(role_id_to_name(50000), "button", "UIA_ButtonControlTypeId");
        assert_eq!(role_id_to_name(50002), "checkbox", "UIA_CheckBoxControlTypeId");
        assert_eq!(role_id_to_name(50003), "combobox", "UIA_ComboBoxControlTypeId");
        assert_eq!(role_id_to_name(50004), "edit", "UIA_EditControlTypeId");
        assert_eq!(role_id_to_name(50005), "hyperlink", "UIA_HyperlinkControlTypeId");
        assert_eq!(role_id_to_name(50008), "list", "UIA_ListControlTypeId");
        assert_eq!(role_id_to_name(50011), "menuitem", "UIA_MenuItemControlTypeId");
        assert_eq!(role_id_to_name(50030), "document", "UIA_DocumentControlTypeId");
        assert_eq!(role_id_to_name(50032), "window", "UIA_WindowControlTypeId");
        assert_eq!(role_id_to_name(50033), "pane", "UIA_PaneControlTypeId");
        assert_eq!(role_id_to_name(50040), "appbar", "last id in the range");
    }

    #[test]
    fn ids_outside_the_uia_range_are_unknown() {
        // The old table used ids like 42 and 50, which are not control types at all.
        assert_eq!(role_id_to_name(42), "unknown");
        assert_eq!(role_id_to_name(50), "unknown");
        assert_eq!(role_id_to_name(49999), "unknown");
        assert_eq!(role_id_to_name(50041), "unknown");
    }

    #[test]
    fn role_names_round_trip_through_ids() {
        // A condition built from the wrong id would silently match nothing, so the
        // reverse mapping has to agree with the forward one for every role.
        for id in 50000..=50040u32 {
            let name = role_id_to_name(id);
            assert_eq!(role_name_to_id(name), Some(id), "round trip failed for {name}");
        }
    }

    #[test]
    fn role_name_lookup_is_case_insensitive_and_rejects_unknowns() {
        assert_eq!(role_name_to_id("Button"), Some(50000));
        assert_eq!(role_name_to_id("  edit "), Some(50004));
        assert_eq!(role_name_to_id("not-a-role"), None);
        assert_eq!(role_name_to_id("unknown"), None);
    }

    #[test]
    fn every_named_role_is_reachable_from_some_id() {
        // Guards against a typo that names a role no id maps to, which would make
        // that role permanently unfindable via By::role.
        for role in INTERACTIVE_ROLES {
            let hit = (50000..=50040).any(|id| role_id_to_name(id) == *role);
            assert!(hit, "interactive role '{role}' maps from no control type id");
        }
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
