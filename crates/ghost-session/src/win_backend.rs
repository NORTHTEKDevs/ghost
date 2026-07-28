//! Windows implementation of [`SessionBackend`], adapting the existing
//! [`GhostSession`] engine to the platform-neutral vocabulary.
//!
//! Not a rewrite. `GhostSession` is unchanged and remains the richer API; this
//! module only restates the subset of it that macOS can also express, in
//! [`ghost_platform`]'s types instead of `ghost-core`'s. Every method here is a
//! delegation.
//!
//! # Why a window name is turned into a focus call
//!
//! The neutral trait scopes `snapshot`, `find`, `click` and `read_value` to a named
//! window, because that is how the Accessibility API works: an `AXUIElement` is
//! obtained per-application. UIA instead searches from the desktop root and its
//! fast paths are scoped to the *foreground* window. Bringing the requested window
//! forward first is what makes the two agree, and it is also what a caller wants —
//! a click needs the window in front regardless.

use async_trait::async_trait;
use ghost_platform::{
    ActionKind, Capabilities, ElementInfo, Locator, Platform, Point, Rect, WindowRef,
};

use crate::backend::SessionBackend;
use crate::error::{GhostError, Result};
use crate::locator::By;
use crate::session::{GhostSession, Region};

/// The Windows session engine.
pub struct WinBackend {
    inner: GhostSession,
}

impl WinBackend {
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: GhostSession::new()?,
        })
    }

    /// The full Windows engine, for the Windows-only surface the neutral trait
    /// deliberately omits.
    pub fn session(&self) -> &GhostSession {
        &self.inner
    }

    /// Focus a window, then resolve a locator inside it.
    async fn locate(&self, window: &str, locator: &Locator) -> Result<crate::GhostElement> {
        self.inner.ensure_window_foreground(window).await?;
        self.inner.find(locator_to_by(locator)).await
    }
}

/// Translate a neutral locator into UIA's.
///
/// Total, because the two enums carry the same three cases. `Description` is passed
/// through rather than rejected here: `GhostSession::find` refuses it with a message
/// naming the vision entry points to use instead, which is more useful than anything
/// this layer could say.
fn locator_to_by(locator: &Locator) -> By {
    match locator {
        Locator::Name(n) => By::Name(n.clone()),
        Locator::Role(r) => By::Role(r.clone()),
        Locator::Description(d) => By::Description(d.clone()),
    }
}

/// Derive a stable id for an element that has no native one.
///
/// UIA has no per-element identity Ghost can hand out — `UiaElement` is a COM pointer
/// whose address says nothing about the element it names. The name, role and position
/// together are what an agent would use to recognise the same control again, so they
/// are what the id is built from. It is stable across calls for an element that has
/// not moved, and deliberately not stable across a layout change: an element that
/// moved is, for planning purposes, a different one.
fn stable_id(name: &str, role: &str, rect: Rect) -> usize {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    role.hash(&mut hasher);
    rect.left.hash(&mut hasher);
    rect.top.hash(&mut hasher);
    rect.right.hash(&mut hasher);
    rect.bottom.hash(&mut hasher);
    hasher.finish() as usize
}

/// Which actions a role accepts, in the neutral vocabulary.
fn actions_for(role: &str, enabled: bool) -> Vec<ActionKind> {
    if !enabled {
        return Vec::new();
    }
    let mut actions = vec![ActionKind::Click];
    if matches!(role, "edit" | "document" | "combobox") {
        actions.push(ActionKind::Type);
    }
    actions
}

fn descriptor_to_info(d: &ghost_core::uia::ElementDescriptor) -> ElementInfo {
    let rect = Rect {
        left: d.left,
        top: d.top,
        right: d.right,
        bottom: d.bottom,
    };
    ElementInfo {
        id: stable_id(&d.name, &d.role, rect),
        name: d.name.clone(),
        role: d.role.clone(),
        rect,
        enabled: d.enabled,
        actionable: d.enabled && rect.width() > 0 && rect.height() > 0,
        actions: actions_for(&d.role, d.enabled),
    }
}

fn element_to_info(el: &crate::GhostElement) -> ElementInfo {
    let rect = el
        .bounding_rect()
        .map(|(left, top, right, bottom)| Rect {
            left,
            top,
            right,
            bottom,
        })
        .unwrap_or(Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        });
    let name = el.name();
    let role = ghost_core::uia::element::role_id_to_name(el.control_type()).to_string();
    let enabled = el.is_enabled();
    ElementInfo {
        id: stable_id(&name, &role, rect),
        name,
        role: role.clone(),
        rect,
        enabled,
        actionable: enabled && !el.is_offscreen(),
        actions: actions_for(&role, enabled),
    }
}

#[async_trait(?Send)]
impl SessionBackend for WinBackend {
    fn platform(&self) -> Platform {
        Platform::Windows
    }

    fn capabilities(&self) -> Capabilities {
        ghost_platform::capabilities_for(Platform::Windows)
    }

    async fn list_windows(&self) -> Result<Vec<WindowRef>> {
        Ok(self
            .inner
            .list_windows()
            .await?
            .into_iter()
            .map(|w| WindowRef {
                title: w.name,
                id: w.hwnd as i64,
                focused: w.focused,
            })
            .collect())
    }

    async fn focus_window(&self, query: &str) -> Result<()> {
        self.inner.focus_window(query).await
    }

    async fn snapshot(&self, window: &str) -> Result<Vec<ElementInfo>> {
        Ok(self
            .inner
            .describe_screen(Some(window))
            .await?
            .iter()
            .map(descriptor_to_info)
            .collect())
    }

    async fn find(&self, window: &str, locator: &Locator) -> Result<ElementInfo> {
        let el = self.locate(window, locator).await?;
        Ok(element_to_info(&el))
    }

    async fn click(&self, window: &str, locator: &Locator) -> Result<ElementInfo> {
        let el = self.locate(window, locator).await?;
        let info = element_to_info(&el);
        el.click()?;
        Ok(info)
    }

    async fn click_at(&self, point: Point) -> Result<()> {
        self.inner.click_at(point.x, point.y).await
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        ghost_core::input::keyboard::type_text(text).map_err(GhostError::Core)
    }

    async fn press_key(&self, modifiers: &[String], key: &str) -> Result<()> {
        let mods: Vec<&str> = modifiers.iter().map(String::as_str).collect();
        self.inner.hotkey(&mods, key).await
    }

    async fn read_value(&self, window: &str, locator: &Locator) -> Result<Option<String>> {
        let el = self.locate(window, locator).await?;
        Ok(Some(el.get_text()))
    }

    async fn screenshot_window(&self, window: &str) -> Result<Vec<u8>> {
        self.inner.ensure_window_foreground(window).await?;
        // `None` would capture the whole screen, which is a different picture than
        // the one that was asked for, so a missing rect is an error rather than a
        // silent widening.
        let rect = self.inner.foreground_window_rect().ok_or_else(|| {
            GhostError::ProcessNotFound {
                name: format!("{window} (no foreground window rect after focusing it)"),
            }
        })?;
        self.inner.screenshot_region(Some(rect), None, None).await
    }

    async fn screenshot(&self) -> Result<Vec<u8>> {
        self.inner.screenshot(Region::full()).await
    }

    async fn get_clipboard(&self) -> Result<Option<String>> {
        // Win32 reports "no text on the clipboard" as an empty string; the neutral
        // API distinguishes that from a clipboard holding an empty string, which is
        // not a state Win32 can represent, so empty maps to `None`.
        let text = self.inner.get_clipboard().await?;
        Ok(if text.is_empty() { None } else { Some(text) })
    }

    async fn set_clipboard(&self, text: &str) -> Result<()> {
        self.inner.set_clipboard(text).await
    }

    async fn frontmost_app(&self) -> Option<String> {
        self.inner
            .list_windows()
            .await
            .ok()?
            .into_iter()
            .find(|w| w.focused)
            .map(|w| w.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locator_survives_the_round_trip_into_uia_terms() {
        assert!(matches!(
            locator_to_by(&Locator::Name("Save".into())),
            By::Name(n) if n == "Save"
        ));
        assert!(matches!(
            locator_to_by(&Locator::Role("button".into())),
            By::Role(r) if r == "button"
        ));
        assert!(matches!(
            locator_to_by(&Locator::Description("the blue one".into())),
            By::Description(d) if d == "the blue one"
        ));
    }

    #[test]
    fn the_same_element_hashes_to_the_same_id() {
        let rect = Rect {
            left: 10,
            top: 20,
            right: 110,
            bottom: 60,
        };
        assert_eq!(
            stable_id("Save", "button", rect),
            stable_id("Save", "button", rect)
        );
    }

    #[test]
    fn a_moved_element_gets_a_new_id() {
        // Intentional: position is part of identity, because an agent's plan is
        // invalidated by a control moving just as much as by it being replaced.
        let here = Rect {
            left: 10,
            top: 20,
            right: 110,
            bottom: 60,
        };
        let there = Rect { top: 21, ..here };
        assert_ne!(
            stable_id("Save", "button", here),
            stable_id("Save", "button", there)
        );
    }

    #[test]
    fn a_disabled_element_offers_no_actions() {
        assert!(actions_for("button", false).is_empty());
        assert_eq!(actions_for("button", true), vec![ActionKind::Click]);
    }

    #[test]
    fn a_text_field_offers_typing_as_well_as_clicking() {
        let actions = actions_for("edit", true);
        assert!(actions.contains(&ActionKind::Click));
        assert!(actions.contains(&ActionKind::Type));
    }

    #[test]
    fn a_descriptor_becomes_an_element_info_with_a_real_rect() {
        let d = ghost_core::uia::ElementDescriptor {
            name: "Save".into(),
            role: "button".into(),
            left: 10,
            top: 20,
            right: 110,
            bottom: 60,
            enabled: true,
        };
        let info = descriptor_to_info(&d);
        assert_eq!(info.name, "Save");
        assert_eq!(info.rect.width(), 100);
        assert!(info.actionable);
    }

    #[test]
    fn a_zero_sized_descriptor_is_not_actionable() {
        // UIA reports collapsed and scrolled-away controls with an empty rect;
        // clicking their centre would click whatever is behind them.
        let d = ghost_core::uia::ElementDescriptor {
            name: "Hidden".into(),
            role: "button".into(),
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
            enabled: true,
        };
        assert!(!descriptor_to_info(&d).actionable);
    }
}
