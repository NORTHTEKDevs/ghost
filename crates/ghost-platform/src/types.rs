//! Platform-neutral vocabulary shared by all three backends. Pure data, no FFI.

use serde::{Deserialize, Serialize};

/// A pixel point in screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// A rectangle in screen coordinates (left, top, right, bottom).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn center(&self) -> Point {
        Point { x: (self.left + self.right) / 2, y: (self.top + self.bottom) / 2 }
    }
    pub fn width(&self) -> i32 { (self.right - self.left).max(0) }
    pub fn height(&self) -> i32 { (self.bottom - self.top).max(0) }
}

/// How to locate an element — the same three ways on every OS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Locator {
    /// By accessible name (substring).
    Name(String),
    /// By control role/type ("button", "edit", ...).
    Role(String),
    /// By natural-language description (vision grounding).
    Description(String),
}

/// An action an agent can take on an element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionKind {
    Click,
    DoubleClick,
    RightClick,
    Hover,
    Type,
}

/// A top-level window, OS-agnostic. `id` is the native handle/identifier as an
/// integer (HWND on Windows, an AX/window id on mac, an X11/AT-SPI id on Linux).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowRef {
    pub title: String,
    pub id: i64,
    pub focused: bool,
}

/// An interactable element, OS-agnostic — the shape `snapshot()` returns and an
/// agent plans over. Mirrors the Windows `ghost_snapshot` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementInfo {
    pub id: usize,
    pub name: String,
    pub role: String,
    pub rect: Rect,
    pub enabled: bool,
    pub actionable: bool,
    pub actions: Vec<ActionKind>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_center_and_size() {
        let r = Rect { left: 0, top: 0, right: 100, bottom: 40 };
        assert_eq!(r.center(), Point { x: 50, y: 20 });
        assert_eq!(r.width(), 100);
        assert_eq!(r.height(), 40);
    }

    #[test]
    fn negative_rect_has_zero_size() {
        let r = Rect { left: 10, top: 10, right: 5, bottom: 5 };
        assert_eq!(r.width(), 0);
        assert_eq!(r.height(), 0);
    }
}
