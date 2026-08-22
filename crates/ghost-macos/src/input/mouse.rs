//! Mouse input.
//!
//! Real synthetic mouse events need `CGEventCreateMouseEvent` + `CGEventPost`
//! (Accessibility-gated, C FFI) -- out of scope here. Every function honestly
//! refuses instead of moving nothing and claiming success.

use crate::error::CoreError;

pub fn click(_x: i32, _y: i32) -> Result<(), CoreError> {
    Err(CoreError::Unsupported { op: "click", needs: "CGEventCreateMouseEvent" })
}

pub fn hover(_x: i32, _y: i32) -> Result<(), CoreError> {
    Err(CoreError::Unsupported { op: "hover", needs: "CGEventCreateMouseEvent" })
}

pub fn right_click(_x: i32, _y: i32) -> Result<(), CoreError> {
    Err(CoreError::Unsupported { op: "right_click", needs: "CGEventCreateMouseEvent" })
}

pub fn double_click(_x: i32, _y: i32) -> Result<(), CoreError> {
    Err(CoreError::Unsupported { op: "double_click", needs: "CGEventCreateMouseEvent" })
}

pub fn drag(_from_x: i32, _from_y: i32, _to_x: i32, _to_y: i32) -> Result<(), CoreError> {
    Err(CoreError::Unsupported { op: "drag", needs: "CGEventCreateMouseEvent" })
}

pub fn scroll(_x: i32, _y: i32, _direction: &str, _amount: i32) -> Result<(), CoreError> {
    Err(CoreError::Unsupported { op: "scroll", needs: "CGEventCreateScrollWheelEvent" })
}
