//! Local text location.
//!
//! The Windows engine backs `find_text_local` with the Windows.Media.Ocr
//! runtime. Fedora ships no comparable always-present OCR engine, and shipping
//! a bundled one would mean linking Tesseract and adding a large runtime
//! dependency for a fallback path.
//!
//! On Linux, text location is served by the accessibility tree instead, which is
//! both more accurate and cheaper than OCR: `ghost_find`/`ghost_see` already
//! return real text with real bounds. This module therefore reports the
//! capability as unavailable rather than silently returning nothing, so an agent
//! gets an actionable error instead of a false negative.

use crate::error::{CoreError, Result};

/// Screen-space hit for a piece of text.
#[derive(Debug, Clone)]
pub struct TextHit {
    pub text: String,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub confidence: f32,
}

pub fn find_text_local(
    _needle: &str,
    _region: Option<(i32, i32, i32, i32)>,
) -> Result<Option<(i32, i32)>> {
    Err(CoreError::Unsupported(
        "local OCR is not available on Linux; use ghost_find/ghost_see (AT-SPI reports real text \
         with real bounds), or ghost_find mode=deliberate for VLM grounding on canvas-only apps"
            .into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_unsupported_rather_than_an_empty_result() {
        // An empty Vec would read as "text not on screen", which is a false
        // negative an agent cannot distinguish from a real miss.
        match find_text_local("Save", None) {
            Err(CoreError::Unsupported(msg)) => assert!(msg.contains("ghost_find")),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
