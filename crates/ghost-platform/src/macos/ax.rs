//! Element discovery and acting, over the macOS Accessibility API.
//!
//! This is the macOS counterpart to Windows UI Automation. The mapping Ghost uses:
//!
//! | Ghost operation | Apple API |
//! | --- | --- |
//! | app handle | `AXUIElementCreateApplication(pid)` |
//! | children | `AXUIElementCopyAttributeValue(kAXChildrenAttribute)` |
//! | windows | `AXUIElementCopyAttributeValue(kAXWindowsAttribute)` |
//! | role | `kAXRoleAttribute` |
//! | name | `kAXTitleAttribute`, falling back to `kAXDescriptionAttribute` then `kAXValueAttribute` |
//! | enabled | `kAXEnabledAttribute` |
//! | rect | `kAXPositionAttribute` + `kAXSizeAttribute` via `AXValueGetValue` |
//! | available actions | `AXUIElementCopyActionNames` |
//! | click / press | `AXUIElementPerformAction(kAXPressAction)` |
//! | set text | `AXUIElementSetAttributeValue(kAXValueAttribute)` |
//! | read text back | `AXUIElementCopyAttributeValue(kAXValueAttribute)` |
//!
//! **Coordinates are points, not pixels.** `kAXPositionAttribute` is in a
//! top-left-origin point space — the same space `CGWindowListCreateImage` takes,
//! and *not* the bottom-left-origin space AppKit's `NSScreen` uses. Ghost keeps
//! everything in points and converts to pixels only inside [`super::capture`].

use std::ffi::c_void;

use accessibility_sys::{
    kAXChildrenAttribute, kAXDescriptionAttribute, kAXEnabledAttribute, kAXFocusedWindowAttribute,
    kAXPositionAttribute, kAXPressAction, kAXRaiseAction, kAXRoleAttribute, kAXSizeAttribute,
    kAXTitleAttribute, kAXValueAttribute, kAXValueTypeCGPoint, kAXValueTypeCGSize,
    kAXWindowsAttribute, AXUIElementCopyActionNames, AXUIElementCopyAttributeValue,
    AXUIElementCreateApplication, AXUIElementGetPid, AXUIElementPerformAction,
    AXUIElementSetAttributeValue, AXUIElementSetMessagingTimeout, AXUIElementRef, AXValueGetValue,
    AXValueRef,
};
use core_foundation::base::{CFType, TCFType};
use core_foundation::string::CFString;
use core_foundation_sys::base::{CFHash, CFRelease, CFRetain, CFTypeRef};
use core_graphics::geometry::{CGPoint, CGSize};

use super::error::{check_ax, AxStatus, MacError, MacResult};
use super::ffi::{as_array, as_bool, as_string, cfstr, owned};
use super::perms::require_accessibility;
use crate::types::{ActionKind, ElementInfo, Rect};

/// How long to wait on a single AX round trip before giving up.
///
/// AX is synchronous IPC into the target app: if that app is busy or wedged, a
/// call blocks. Windows UIA has the same hazard and Ghost bounds it there too.
/// Two seconds is long enough for a launching app and short enough that an agent
/// gets a typed error instead of appearing to hang.
const AX_TIMEOUT_SECONDS: f32 = 2.0;

/// How deep to walk an accessibility tree.
///
/// Real app trees are ~10-20 deep; deeply nested web content can be far deeper.
/// The cap exists because a malformed provider can report a cycle, and an
/// unbounded walk would then never return.
const MAX_DEPTH: usize = 40;

/// An owned reference to an `AXUIElement`.
///
/// Wraps the retain/release discipline: `AXUIElement` is a CFType, so this holds
/// exactly one reference and drops it in `Drop`.
pub struct AxElement {
    raw: AXUIElementRef,
}

// AXUIElement is a thread-safe CFType (AX calls are synchronous IPC and may be
// made from any thread), so an owned handle can move between threads.
unsafe impl Send for AxElement {}

impl Drop for AxElement {
    fn drop(&mut self) {
        unsafe { CFRelease(self.raw as CFTypeRef) }
    }
}

impl Clone for AxElement {
    fn clone(&self) -> Self {
        unsafe { CFRetain(self.raw as CFTypeRef) };
        AxElement { raw: self.raw }
    }
}

impl AxElement {
    /// Take ownership of a raw element from a Create/Copy-rule AX function.
    ///
    /// # Safety
    /// `raw` must be a non-null `AXUIElementRef` the caller owns a reference to.
    pub unsafe fn from_create_rule(raw: AXUIElementRef) -> Self {
        AxElement { raw }
    }

    /// The accessibility handle for a running process —
    /// `AXUIElementCreateApplication`.
    ///
    /// Requires the Accessibility grant; the check happens here so no caller can
    /// forget it.
    pub fn for_app(pid: i32) -> MacResult<Self> {
        require_accessibility()?;
        let raw = unsafe { AXUIElementCreateApplication(pid) };
        if raw.is_null() {
            return Err(MacError::InvalidArgument(format!(
                "AXUIElementCreateApplication returned null for pid {pid}"
            )));
        }
        let el = unsafe { AxElement::from_create_rule(raw) };
        el.set_messaging_timeout(AX_TIMEOUT_SECONDS)?;
        Ok(el)
    }

    pub fn as_raw(&self) -> AXUIElementRef {
        self.raw
    }

    /// Bound every subsequent AX call on this element —
    /// `AXUIElementSetMessagingTimeout`.
    pub fn set_messaging_timeout(&self, seconds: f32) -> MacResult<()> {
        let status = unsafe { AXUIElementSetMessagingTimeout(self.raw, seconds) };
        check_ax("AXUIElementSetMessagingTimeout", status)
    }

    /// The owning process id — `AXUIElementGetPid`.
    pub fn pid(&self) -> MacResult<i32> {
        let mut pid: i32 = 0;
        let status = unsafe { AXUIElementGetPid(self.raw, &mut pid) };
        check_ax("AXUIElementGetPid", status)?;
        Ok(pid)
    }

    /// Read one attribute — `AXUIElementCopyAttributeValue`.
    ///
    /// Returns `Ok(None)` when the attribute is simply absent from this element
    /// (`kAXErrorNoValue` / `kAXErrorAttributeUnsupported`), which is ordinary
    /// while walking a tree, and `Err` only for a real failure.
    pub fn attribute(&self, name: &str) -> MacResult<Option<CFType>> {
        let key = cfstr(name);
        let mut value: CFTypeRef = std::ptr::null();
        let status =
            unsafe { AXUIElementCopyAttributeValue(self.raw, key.as_concrete_TypeRef(), &mut value) };

        let classified = AxStatus::from_raw(status);
        if classified.is_absent() {
            return Ok(None);
        }
        check_ax("AXUIElementCopyAttributeValue", status)?;
        Ok(unsafe { owned(value) })
    }

    /// Read a string attribute, treating an absent or non-string value as `None`.
    pub fn string_attribute(&self, name: &str) -> MacResult<Option<String>> {
        Ok(self.attribute(name)?.as_ref().and_then(as_string))
    }

    /// Read a boolean attribute.
    pub fn bool_attribute(&self, name: &str) -> MacResult<Option<bool>> {
        Ok(self.attribute(name)?.as_ref().and_then(as_bool))
    }

    /// `kAXRoleAttribute`, e.g. `AXButton`, `AXTextArea`, `AXWindow`.
    pub fn role(&self) -> MacResult<String> {
        Ok(self
            .string_attribute(kAXRoleAttribute)?
            .unwrap_or_else(|| "AXUnknown".to_string()))
    }

    /// The best human label for this element.
    ///
    /// `kAXTitleAttribute` is the primary, but many controls (icon buttons in
    /// particular) carry only `kAXDescriptionAttribute`, and static text carries
    /// its label in `kAXValueAttribute`. Ghost tries all three so name-based
    /// lookup finds the same things a person would point at.
    pub fn name(&self) -> MacResult<String> {
        for attr in [kAXTitleAttribute, kAXDescriptionAttribute, kAXValueAttribute] {
            if let Some(s) = self.string_attribute(attr)? {
                if !s.is_empty() {
                    return Ok(s);
                }
            }
        }
        Ok(String::new())
    }

    /// `kAXEnabledAttribute`. Absent means "not a disableable control", which
    /// Ghost reports as enabled — the same choice the Windows backend makes.
    pub fn enabled(&self) -> MacResult<bool> {
        Ok(self.bool_attribute(kAXEnabledAttribute)?.unwrap_or(true))
    }

    /// Screen rect in points, from `kAXPositionAttribute` + `kAXSizeAttribute`.
    ///
    /// Both come back as an `AXValueRef` that must be unpacked with
    /// `AXValueGetValue`; they are not `CFNumber`s.
    pub fn rect(&self) -> MacResult<Option<Rect>> {
        let (Some(pos), Some(size)) = (
            self.attribute(kAXPositionAttribute)?,
            self.attribute(kAXSizeAttribute)?,
        ) else {
            return Ok(None);
        };

        let mut point = CGPoint { x: 0.0, y: 0.0 };
        let mut extent = CGSize {
            width: 0.0,
            height: 0.0,
        };

        let got_point = unsafe {
            AXValueGetValue(
                pos.as_CFTypeRef() as AXValueRef,
                kAXValueTypeCGPoint,
                &mut point as *mut CGPoint as *mut c_void,
            )
        };
        let got_size = unsafe {
            AXValueGetValue(
                size.as_CFTypeRef() as AXValueRef,
                kAXValueTypeCGSize,
                &mut extent as *mut CGSize as *mut c_void,
            )
        };
        if !got_point || !got_size {
            return Ok(None);
        }

        Ok(Some(rect_from_point_size(point, extent)))
    }

    /// The action names this element accepts — `AXUIElementCopyActionNames`.
    pub fn action_names(&self) -> MacResult<Vec<String>> {
        let mut raw = std::ptr::null();
        let status = unsafe { AXUIElementCopyActionNames(self.raw, &mut raw) };

        let classified = AxStatus::from_raw(status);
        if classified.is_absent() {
            return Ok(Vec::new());
        }
        check_ax("AXUIElementCopyActionNames", status)?;

        let Some(value) = (unsafe { owned(raw as CFTypeRef) }) else {
            return Ok(Vec::new());
        };
        let Some(array) = as_array(&value) else {
            return Ok(Vec::new());
        };
        Ok(array.iter().filter_map(as_string).collect())
    }

    /// Child elements — `kAXChildrenAttribute`.
    pub fn children(&self) -> MacResult<Vec<AxElement>> {
        self.element_array(kAXChildrenAttribute)
    }

    /// Top-level windows of an application element — `kAXWindowsAttribute`.
    pub fn windows(&self) -> MacResult<Vec<AxElement>> {
        self.element_array(kAXWindowsAttribute)
    }

    /// The app's focused window — `kAXFocusedWindowAttribute`.
    pub fn focused_window(&self) -> MacResult<Option<AxElement>> {
        self.element_attribute(kAXFocusedWindowAttribute)
    }

    /// An attribute whose value is itself an element.
    pub fn element_attribute(&self, name: &str) -> MacResult<Option<AxElement>> {
        let Some(value) = self.attribute(name)? else {
            return Ok(None);
        };
        Ok(Some(retain_as_element(&value)))
    }

    /// The application's menu bar — `kAXMenuBarAttribute`.
    ///
    /// The menu bar is reachable only from the *application* element, never from a
    /// window, which is why `ghost doctor --mac` walks down from
    /// [`AxElement::for_app`] to drive File > New.
    pub fn menu_bar(&self) -> MacResult<Option<AxElement>> {
        self.element_attribute(accessibility_sys::kAXMenuBarAttribute)
    }

    /// The first descendant whose accessible name matches, breadth-first, bounded.
    ///
    /// Breadth-first because menus are shallow and wide: a depth-first walk into the
    /// first menu would open and traverse every item under it before reaching the
    /// second title. `max_depth` bounds a tree that an app is free to make cyclic.
    pub fn find_child_named(&self, needle: &str, max_depth: u32) -> MacResult<Option<AxElement>> {
        self.find_descendant(max_depth, |child| Ok(name_matches(&child.name()?, needle)))
    }

    /// The first descendant whose *raw* `kAXRoleAttribute` equals `raw_role`.
    ///
    /// Raw rather than [`ghost_role`]-normalised: `ghost doctor --mac` asserts on the
    /// Apple role string itself (`AXTextArea`), because the point of that check is to
    /// prove the accessibility tree came back as Apple documents it, not that Ghost's
    /// own mapping table is self-consistent.
    pub fn find_child_with_role(
        &self,
        raw_role: &str,
        max_depth: u32,
    ) -> MacResult<Option<AxElement>> {
        self.find_descendant(max_depth, |child| Ok(child.role()? == raw_role))
    }

    /// Breadth-first bounded search shared by the finders above.
    ///
    /// Breadth-first because menus are shallow and wide: a depth-first walk into the
    /// first menu would open and traverse every item under it before reaching the
    /// second title. `max_depth` bounds a tree that an app is free to make cyclic.
    fn find_descendant(
        &self,
        max_depth: u32,
        mut matches: impl FnMut(&AxElement) -> MacResult<bool>,
    ) -> MacResult<Option<AxElement>> {
        let mut frontier = vec![self.clone()];
        for _ in 0..max_depth {
            let mut next = Vec::new();
            for element in frontier {
                for child in element.children()? {
                    if matches(&child)? {
                        return Ok(Some(child));
                    }
                    next.push(child);
                }
            }
            if next.is_empty() {
                return Ok(None);
            }
            frontier = next;
        }
        Ok(None)
    }

    fn element_array(&self, attr: &str) -> MacResult<Vec<AxElement>> {
        let Some(value) = self.attribute(attr)? else {
            return Ok(Vec::new());
        };
        let Some(array) = as_array(&value) else {
            return Ok(Vec::new());
        };
        Ok(array.iter().map(retain_as_element).collect())
    }

    /// Perform an action — `AXUIElementPerformAction`.
    ///
    /// Note for anyone extending this: `kAXPressAction` may bring the target
    /// window forward. Whether it does is provider-specific, which is exactly why
    /// Ghost does not claim `BackgroundDispatch` on macOS. See
    /// `docs/cross-platform.md`.
    pub fn perform(&self, action: &str) -> MacResult<()> {
        require_accessibility()?;
        let name = cfstr(action);
        let status = unsafe { AXUIElementPerformAction(self.raw, name.as_concrete_TypeRef()) };
        check_ax("AXUIElementPerformAction", status)
    }

    /// `kAXPressAction` — the accessibility equivalent of a click.
    pub fn press(&self) -> MacResult<()> {
        self.perform(kAXPressAction)
    }

    /// `kAXRaiseAction` — bring a window to the front of its app.
    pub fn raise(&self) -> MacResult<()> {
        self.perform(kAXRaiseAction)
    }

    /// Set the element's text — `AXUIElementSetAttributeValue(kAXValueAttribute)`.
    ///
    /// This is the analogue of Windows `ValuePattern.SetValue`: it replaces the
    /// whole value rather than typing keys, so it neither needs focus nor fires
    /// per-character key handlers. Apps that only listen for key events will not
    /// react to it — use [`super::input`] for those.
    pub fn set_value(&self, text: &str) -> MacResult<()> {
        require_accessibility()?;
        let key = cfstr(kAXValueAttribute);
        let value = CFString::new(text);
        let status = unsafe {
            AXUIElementSetAttributeValue(
                self.raw,
                key.as_concrete_TypeRef(),
                value.as_CFTypeRef(),
            )
        };
        check_ax("AXUIElementSetAttributeValue", status)
    }

    /// Read the element's text back — the verification half of act-then-verify.
    pub fn value_string(&self) -> MacResult<Option<String>> {
        self.string_attribute(kAXValueAttribute)
    }

    /// A stable identifier for this element within one Ghost run.
    ///
    /// Composed from the owning **pid** and `CFHash` of the `AXUIElement`.
    /// `AXUIElement` implements `CFEqual`/`CFHash` such that two handles to the
    /// same UI object hash equally, so the id survives re-reading the tree.
    ///
    /// It is deliberately **not** durable across app restarts: a relaunched app
    /// has a new pid and new AX objects. Mixing the pid in means two different
    /// apps cannot collide on a bare hash.
    pub fn stable_id(&self) -> MacResult<usize> {
        let pid = self.pid()?;
        let hash = unsafe { CFHash(self.raw as CFTypeRef) };
        Ok(mix_stable_id(pid, hash as u64))
    }

    /// Flatten this element's subtree into the platform-neutral snapshot shape an
    /// agent plans over.
    pub fn snapshot(&self) -> MacResult<Vec<ElementInfo>> {
        let mut out = Vec::new();
        self.collect(0, &mut out)?;
        Ok(out)
    }

    fn collect(&self, depth: usize, out: &mut Vec<ElementInfo>) -> MacResult<()> {
        if depth >= MAX_DEPTH {
            return Ok(());
        }

        if let Some(info) = self.describe()? {
            out.push(info);
        }
        for child in self.children()? {
            child.collect(depth + 1, out)?;
        }
        Ok(())
    }

    /// Describe just this element. `Ok(None)` for an element with no geometry,
    /// which an agent cannot target anyway.
    pub fn describe(&self) -> MacResult<Option<ElementInfo>> {
        let Some(rect) = self.rect()? else {
            return Ok(None);
        };
        let actions = self.action_names()?;
        let role = self.role()?;
        Ok(Some(ElementInfo {
            id: self.stable_id()?,
            name: self.name()?,
            role: ghost_role(&role).to_string(),
            rect,
            enabled: self.enabled()?,
            actionable: !actions.is_empty(),
            actions: action_kinds(&actions),
        }))
    }
}

/// Retain a `CFType` that is really an `AXUIElement` into an owned [`AxElement`].
///
/// Elements inside a `CFArray` are borrowed under the get rule, so they must be
/// retained before the array is dropped.
fn retain_as_element(value: &CFType) -> AxElement {
    let raw = value.as_CFTypeRef();
    unsafe {
        CFRetain(raw);
        AxElement::from_create_rule(raw as AXUIElementRef)
    }
}

/// Convert an AX point + size into Ghost's integer [`Rect`].
///
/// AX reports fractional points; Ghost's `Rect` is integral. Rounding (rather
/// than truncating) keeps a centre point on the intended pixel for odd-sized
/// controls, which matters because a click targets `Rect::center()`.
pub fn rect_from_point_size(origin: CGPoint, size: CGSize) -> Rect {
    let left = origin.x.round() as i32;
    let top = origin.y.round() as i32;
    let right = (origin.x + size.width).round() as i32;
    let bottom = (origin.y + size.height).round() as i32;
    Rect {
        left,
        top,
        right: right.max(left),
        bottom: bottom.max(top),
    }
}

/// Combine a pid and a `CFHash` into one `usize` element id.
///
/// Split so it can be tested without a window server. The pid is folded into the
/// high bits so that elements from different processes cannot collide even if
/// their `CFHash` values coincide.
pub fn mix_stable_id(pid: i32, hash: u64) -> usize {
    let pid_part = (pid as u32 as u64) << 32;
    // Fold the 64-bit hash into 32 bits so the pid half is never overwritten.
    let hash_part = (hash ^ (hash >> 32)) & 0xFFFF_FFFF;
    (pid_part | hash_part) as usize
}

/// Map an AX role to Ghost's platform-neutral role vocabulary.
///
/// The vocabulary is the one the Windows backend already exposes (`button`,
/// `edit`, `text`, …) so that `By::role("edit")` means the same thing on both
/// OSes. Unrecognised roles pass through lowercased with the `AX` prefix
/// stripped, rather than being dropped — an agent can still match on them.
pub fn ghost_role(ax_role: &str) -> &str {
    match ax_role {
        "AXButton" | "AXMenuButton" | "AXPopUpButton" | "AXToolbarButton" => "button",
        "AXTextField" | "AXTextArea" | "AXComboBox" | "AXSearchField" => "edit",
        "AXStaticText" | "AXHeading" => "text",
        "AXCheckBox" => "checkbox",
        "AXRadioButton" => "radio",
        "AXWindow" | "AXSheet" | "AXDrawer" => "window",
        "AXMenu" => "menu",
        "AXMenuItem" => "menuitem",
        "AXMenuBar" => "menubar",
        "AXMenuBarItem" => "menubaritem",
        "AXList" | "AXTable" | "AXOutline" => "list",
        "AXRow" => "listitem",
        "AXTabGroup" => "tabs",
        "AXSlider" => "slider",
        "AXImage" => "image",
        "AXLink" => "link",
        "AXScrollArea" => "scrollarea",
        "AXGroup" | "AXSplitGroup" => "group",
        "AXToolbar" => "toolbar",
        other => other.strip_prefix("AX").unwrap_or(other),
    }
}

/// Map AX action names to Ghost's [`ActionKind`] set.
///
/// Only `kAXPressAction` has a true AX equivalent; double-click, right-click and
/// hover are synthesized with CGEvent in [`super::input`], so they are reported
/// as available whenever the element can be pressed at all.
pub fn action_kinds(ax_actions: &[String]) -> Vec<ActionKind> {
    let mut kinds = Vec::new();
    if ax_actions.iter().any(|a| a == kAXPressAction) {
        kinds.push(ActionKind::Click);
        kinds.push(ActionKind::DoubleClick);
        kinds.push(ActionKind::RightClick);
        kinds.push(ActionKind::Hover);
    }
    if ax_actions.iter().any(|a| a == "AXSetValue") {
        kinds.push(ActionKind::Type);
    }
    kinds
}

/// True when `haystack` contains `needle` case-insensitively — the same
/// substring-and-case-insensitive matching `By::name` uses on Windows.
pub fn name_matches(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

#[cfg(all(test, feature = "mac-headless-tests"))]
mod tests {
    use super::*;

    #[test]
    fn rect_conversion_rounds_and_never_inverts() {
        let r = rect_from_point_size(
            CGPoint { x: 10.0, y: 20.0 },
            CGSize {
                width: 100.0,
                height: 40.0,
            },
        );
        assert_eq!(
            r,
            Rect {
                left: 10,
                top: 20,
                right: 110,
                bottom: 60
            }
        );
        assert_eq!(r.center(), crate::types::Point { x: 60, y: 40 });

        // Fractional points round rather than truncate, so the centre of an
        // odd-sized control still lands on the control.
        let frac = rect_from_point_size(
            CGPoint { x: 10.4, y: 20.6 },
            CGSize {
                width: 99.5,
                height: 39.5,
            },
        );
        assert_eq!(frac.left, 10);
        assert_eq!(frac.top, 21);
        assert_eq!(frac.right, 110);
        assert_eq!(frac.bottom, 60);
    }

    #[test]
    fn a_zero_or_negative_size_yields_an_empty_not_inverted_rect() {
        let r = rect_from_point_size(
            CGPoint { x: 50.0, y: 50.0 },
            CGSize {
                width: -10.0,
                height: 0.0,
            },
        );
        assert!(r.right >= r.left, "{r:?}");
        assert!(r.bottom >= r.top, "{r:?}");
        assert_eq!(r.width(), 0);
        assert_eq!(r.height(), 0);
    }

    #[test]
    fn offscreen_negative_coordinates_survive_conversion() {
        // A window on a display left of the main one has negative x. Ghost must
        // not clamp it to zero, or clicks land on the wrong monitor.
        let r = rect_from_point_size(
            CGPoint { x: -1920.0, y: -100.0 },
            CGSize {
                width: 800.0,
                height: 600.0,
            },
        );
        assert_eq!(r.left, -1920);
        assert_eq!(r.top, -100);
        assert_eq!(r.right, -1120);
        assert_eq!(r.bottom, 500);
    }

    #[test]
    fn stable_id_is_deterministic_and_separates_processes() {
        assert_eq!(mix_stable_id(501, 0xDEAD_BEEF), mix_stable_id(501, 0xDEAD_BEEF));
        // Same element hash, different app: must not collide.
        assert_ne!(mix_stable_id(501, 0xDEAD_BEEF), mix_stable_id(502, 0xDEAD_BEEF));
        // Same app, different element: must not collide.
        assert_ne!(mix_stable_id(501, 1), mix_stable_id(501, 2));
    }

    #[test]
    fn stable_id_keeps_the_pid_recoverable_in_the_high_bits() {
        // Folding the hash into the low 32 bits is what makes this true, and it
        // is the property that stops a busy app's elements from shadowing
        // another app's.
        for pid in [1i32, 501, 99_999] {
            let id = mix_stable_id(pid, u64::MAX);
            assert_eq!((id as u64) >> 32, pid as u32 as u64, "pid {pid} was clobbered");
        }
    }

    #[test]
    fn ax_roles_map_onto_the_windows_role_vocabulary() {
        assert_eq!(ghost_role("AXButton"), "button");
        assert_eq!(ghost_role("AXTextArea"), "edit");
        assert_eq!(ghost_role("AXTextField"), "edit");
        assert_eq!(ghost_role("AXStaticText"), "text");
        assert_eq!(ghost_role("AXWindow"), "window");
        assert_eq!(ghost_role("AXMenuItem"), "menuitem");
    }

    #[test]
    fn an_unknown_ax_role_degrades_instead_of_disappearing() {
        assert_eq!(ghost_role("AXFancyNewThing"), "FancyNewThing");
        assert_eq!(ghost_role("NotPrefixed"), "NotPrefixed");
    }

    #[test]
    fn pressable_elements_advertise_the_synthesizable_mouse_actions() {
        let actions = action_kinds(&[kAXPressAction.to_string()]);
        assert!(actions.contains(&ActionKind::Click));
        assert!(actions.contains(&ActionKind::DoubleClick));
        assert!(actions.contains(&ActionKind::RightClick));
        assert!(actions.contains(&ActionKind::Hover));
        // AXPress alone does not mean the value is settable.
        assert!(!actions.contains(&ActionKind::Type));
    }

    #[test]
    fn an_element_with_no_ax_actions_advertises_none() {
        assert!(action_kinds(&[]).is_empty());
        assert!(action_kinds(&["AXShowMenu".to_string()]).is_empty());
    }

    #[test]
    fn settable_elements_advertise_type() {
        let actions = action_kinds(&["AXSetValue".to_string()]);
        assert!(actions.contains(&ActionKind::Type));
    }

    #[test]
    fn name_matching_is_substring_and_case_insensitive() {
        assert!(name_matches("Save Document", "save"));
        assert!(name_matches("Save Document", "DOCUMENT"));
        assert!(name_matches("Save Document", ""));
        assert!(!name_matches("Save Document", "delete"));
        assert!(!name_matches("", "save"));
    }
}
