//! Element handle, mirroring `ghost_core::uia::element`.

use std::sync::Arc;

use super::handles::{self, ObjAddr};
use super::ops::{self, Snapshot};
use crate::bridge::A11yBridge;
use crate::error::{CoreError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Flat description of one element, identical in shape to the Windows engine's
/// `ElementDescriptor` so `ghost-mcp` serialises the same JSON on both.
#[derive(Debug, Clone)]
pub struct ElementDescriptor {
    pub name: String,
    pub role: String,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub enabled: bool,
}

/// A discovered accessible object.
///
/// Holds a cached snapshot for cheap reads plus the address needed to
/// re-resolve for any live operation. Actions never reuse a cached proxy.
#[derive(Clone)]
pub struct A11yElement {
    bridge: Arc<A11yBridge>,
    addr: ObjAddr,
    snap: Snapshot,
    /// Handle of the top-level window this element belongs to.
    window: isize,
}

impl std::fmt::Debug for A11yElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("A11yElement")
            .field("name", &self.snap.name)
            .field("role", &ops::effective_role(&self.snap))
            .field("path", &self.addr.path)
            .finish()
    }
}

impl A11yElement {
    pub fn new(bridge: Arc<A11yBridge>, addr: ObjAddr, snap: Snapshot, window: isize) -> Self {
        Self { bridge, addr, snap, window }
    }

    pub fn addr(&self) -> &ObjAddr {
        &self.addr
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snap
    }

    pub fn name(&self) -> String {
        self.snap.name.clone()
    }

    pub fn control_type(&self) -> u32 {
        self.snap.role_id
    }

    /// The role Ghost reports, after normalisation -- so an element found via
    /// `role="edit"` also reports `edit`, rather than whatever name its toolkit
    /// happened to use. See [`ops::effective_role`].
    pub fn role_name(&self) -> &'static str {
        ops::effective_role(&self.snap)
    }

    pub fn bounding_rect(&self) -> Option<BoundingRect> {
        self.snap.rect.map(|r| BoundingRect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        })
    }

    pub fn center(&self) -> (i32, i32) {
        self.bounding_rect().map(|r| r.center()).unwrap_or((0, 0))
    }

    pub fn is_enabled(&self) -> bool {
        self.snap.enabled
    }

    /// Offscreen means "not currently rendered": either the toolkit says it is
    /// not showing, or it has no usable rectangle.
    pub fn is_offscreen(&self) -> bool {
        !self.snap.showing || self.snap.rect.map(|r| r.is_empty()).unwrap_or(true)
    }

    /// The handle Ghost's shared layers pass to background dispatch. On Windows
    /// this is the control's `HWND`; here it is the element's own interned
    /// handle, which background dispatch resolves back into an AT-SPI address.
    pub fn native_window_handle(&self) -> isize {
        handles::intern(&self.addr)
    }

    /// Handle of the owning top-level window.
    pub fn window_handle(&self) -> isize {
        self.window
    }

    pub fn get_text(&self) -> String {
        let addr = self.addr.clone();
        self.bridge
            .run_default(move |ctx| Box::pin(async move { ops::read_text(ctx, &addr).await }))
            .unwrap_or_default()
    }

    pub fn set_focus(&self) -> Result<()> {
        let addr = self.addr.clone();
        let ok = self
            .bridge
            .run_default(move |ctx| Box::pin(async move { ops::grab_focus(ctx, &addr).await }))?;
        if ok {
            Ok(())
        } else {
            Err(CoreError::FocusFailed { window: format!("{} refused focus", self.addr.path) })
        }
    }

    pub fn scroll_into_view(&self) -> Result<()> {
        let addr = self.addr.clone();
        self.bridge
            .run_default(move |ctx| Box::pin(async move { ops::scroll_into_view(ctx, &addr).await }))?;
        Ok(())
    }

    /// Invoke the element through its own toolkit -- Ghost's background
    /// dispatch on Linux. No pointer movement, no focus steal.
    pub fn invoke(&self) -> Result<bool> {
        let addr = self.addr.clone();
        self.bridge
            .run_default(move |ctx| Box::pin(async move { ops::do_best_action(ctx, &addr).await }))
    }

    /// Set the element's text or numeric value directly.
    pub fn set_value(&self, text: &str) -> Result<bool> {
        let addr = self.addr.clone();
        let text = text.to_string();
        self.bridge.run_default(move |ctx| {
            Box::pin(async move { ops::set_text_value(ctx, &addr, &text).await })
        })
    }

    /// Re-read this element from the bus. Use before asserting on state that may
    /// have changed since discovery.
    pub fn refresh(&mut self) -> Result<()> {
        let addr = self.addr.clone();
        self.snap = self
            .bridge
            .run_default(move |ctx| Box::pin(async move { ops::snapshot(ctx, &addr).await }))?;
        Ok(())
    }

    /// Currently selected text inside this element.
    pub fn selected_text(&self) -> Result<String> {
        let addr = self.addr.clone();
        self.bridge
            .run_default(move |ctx| Box::pin(async move { ops::read_selection(ctx, &addr).await }))
    }

    pub fn descriptor(&self) -> ElementDescriptor {
        let r = self.snap.rect.unwrap_or_default();
        ElementDescriptor {
            name: self.snap.name.clone(),
            role: ops::effective_role(&self.snap).to_string(),
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
            enabled: self.snap.enabled,
        }
    }
}

/// Re-exported under the Windows engine's name so shared code compiles as-is.
pub type UiaElement = A11yElement;

pub use super::roles::role_id_to_name;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounding_rect_centre_matches_the_windows_engine() {
        let r = BoundingRect { left: 0, top: 0, right: 100, bottom: 50 };
        assert_eq!(r.center(), (50, 25));
    }

    #[test]
    fn descriptor_uses_ghost_role_names_not_atspi_ones() {
        let d = ElementDescriptor {
            name: "Save".into(),
            role: role_id_to_name(50000).into(),
            left: 0,
            top: 0,
            right: 10,
            bottom: 10,
            enabled: true,
        };
        assert_eq!(d.role, "button");
    }
}
