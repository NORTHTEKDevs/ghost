//! Typed VLM response parser.
//!
//! Handles all coordinate/action formats produced by known VLMs:
//!
//! - Bare JSON: `{"x": 123, "y": 456}`
//! - Code-fenced JSON: ` ```json\n{"x":123,"y":456}\n``` `
//! - Prose-wrapped JSON: `The element is at {"x": 123, "y": 456} on screen.`
//! - UI-TARS / Qwen style: `click(start_box='(250, 375)')` with 0-1000 normalised ints
//! - Bare tuple: `(250, 375)`
//! - `{"found": false}` / model says not found
//!
//! All coordinate outputs are in the 0..=1000 normalised space understood by
//! [`crate::types::CoordNorm`]. Callers must convert to pixels via
//! [`crate::types::norm_to_px`] or use the image-pixel path when the model
//! was prompted in pixel space (prompt must declare that; the parser then
//! stores them in the `pixels` variant of [`ParsedCoord`]).

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Public result types
// ---------------------------------------------------------------------------

/// The kind of coordinate the parser extracted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordSpace {
    /// 0-1000 normalised space (UI-TARS / Qwen models).
    Norm,
    /// Absolute pixel space (older prompts / JSON models).
    Pixels,
}

/// Extracted coordinates with their space annotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCoord {
    pub x: i32,
    pub y: i32,
    pub space: CoordSpace,
}

/// Optional action verb parsed alongside coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedAction {
    Click,
    DoubleClick,
    RightClick,
    Type(String),
    Scroll { direction: String, amount: i32 },
    Hover,
    Other(String),
}

/// Full parsed VLM response.
#[derive(Debug, Clone)]
pub struct ParsedVlmResponse {
    /// Extracted coordinates, if the model reported a position.
    pub coord: Option<ParsedCoord>,
    /// Action verb, if the model reported one.
    pub action: Option<ParsedAction>,
    /// True when the model explicitly reported the element is not found.
    pub not_found: bool,
}

// ---------------------------------------------------------------------------
// Internal serde helpers
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct RawCoord {
    #[serde(alias = "x")]
    x: Option<i32>,
    #[serde(alias = "y")]
    y: Option<i32>,
    found: Option<bool>,
    // UI-TARS emits "start_box" inside JSON for some formats
    start_box: Option<String>,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Parse a raw VLM response string into a [`ParsedVlmResponse`].
///
/// Returns `None` only on a completely unparseable input that is not a
/// "not found" signal — callers should treat `None` the same as not-found.
pub fn parse_vlm_response(text: &str) -> Option<ParsedVlmResponse> {
    let text = text.trim();

    // 1. Try UI-TARS / Qwen action-style: `action(start_box='(x,y)')` or `action(point='(x,y)')`
    if let Some(r) = try_parse_action_call(text) {
        return Some(r);
    }

    // 2. Try bare / fenced / prose JSON
    if let Some(r) = try_parse_json(text) {
        return Some(r);
    }

    // 3. Try bare tuple `(x, y)`
    if let Some(r) = try_parse_bare_tuple(text) {
        return Some(r);
    }

    // 4. Try plain "not found" text markers
    let lower = text.to_lowercase();
    if lower.contains("not found")
        || lower.contains("not visible")
        || lower.contains("cannot find")
        || lower.contains("element not")
        || lower.contains("unable to find")
    {
        return Some(ParsedVlmResponse { coord: None, action: None, not_found: true });
    }

    None
}

// ---------------------------------------------------------------------------
// Format parsers
// ---------------------------------------------------------------------------

/// Try to parse UI-TARS / Qwen style calls:
/// - `click(start_box='(250, 375)')`
/// - `double_click(start_box='(250, 375)')`
/// - `type(start_box='(250,375)', content='hello')`
/// - `scroll(start_box='(250,375)', direction='down', amount=3)`
fn try_parse_action_call(text: &str) -> Option<ParsedVlmResponse> {
    // Find action verb and the argument list.
    let paren = text.find('(')?;
    let verb = text[..paren].trim().to_lowercase();

    // Accepted verbs.
    let verb_known = matches!(
        verb.as_str(),
        "click" | "double_click" | "right_click" | "type" | "scroll" | "hover"
            | "tap" | "press"
    );
    if !verb_known {
        return None;
    }

    let inner = text[paren + 1..].trim_end_matches(')').trim();

    // Extract coordinates from `start_box='(x,y)'` or `point='(x,y)'`.
    let coord = extract_box_coord(inner, "start_box")
        .or_else(|| extract_box_coord(inner, "point"))
        .or_else(|| extract_box_coord(inner, "end_box"));

    // Extract content for type action.
    let type_content = if verb == "type" {
        extract_quoted_value(inner, "content").or_else(|| extract_quoted_value(inner, "text"))
    } else {
        None
    };

    // Scroll direction/amount.
    let (scroll_dir, scroll_amt) = if verb == "scroll" {
        let dir = extract_quoted_value(inner, "direction").unwrap_or_else(|| "down".into());
        let amt = extract_int_value(inner, "amount").unwrap_or(3);
        (Some(dir), Some(amt))
    } else {
        (None, None)
    };

    let action = match verb.as_str() {
        "click" | "tap" | "press" => Some(ParsedAction::Click),
        "double_click" => Some(ParsedAction::DoubleClick),
        "right_click" => Some(ParsedAction::RightClick),
        "type" => Some(ParsedAction::Type(type_content.unwrap_or_default())),
        "scroll" => Some(ParsedAction::Scroll {
            direction: scroll_dir.unwrap_or_else(|| "down".into()),
            amount: scroll_amt.unwrap_or(3),
        }),
        "hover" => Some(ParsedAction::Hover),
        other => Some(ParsedAction::Other(other.into())),
    };

    Some(ParsedVlmResponse { coord, action, not_found: false })
}

/// Extract a `key='(x,y)'` pair from the action call inner args.
fn extract_box_coord(inner: &str, key: &str) -> Option<ParsedCoord> {
    // Look for `key='(x,y)'` or `key="(x,y)"`.
    let search = format!("{key}=");
    let pos = inner.find(search.as_str())?;
    let after = &inner[pos + search.len()..];
    // Strip leading quote.
    let after = after.trim_start_matches(['\'', '"']);
    parse_tuple_str(after).map(|(x, y)| ParsedCoord { x, y, space: CoordSpace::Norm })
}

/// Extract a `key='value'` string.
fn extract_quoted_value(inner: &str, key: &str) -> Option<String> {
    let search = format!("{key}=");
    let pos = inner.find(search.as_str())?;
    let after = &inner[pos + search.len()..];
    let after = after.trim_start_matches(['\'', '"']);
    let end = after.find(['\'', '"']).unwrap_or(after.len());
    Some(after[..end].to_string())
}

/// Extract a `key=<integer>` value.
fn extract_int_value(inner: &str, key: &str) -> Option<i32> {
    let search = format!("{key}=");
    let pos = inner.find(search.as_str())?;
    let after = &inner[pos + search.len()..].trim_start_matches(['\'', '"']);
    let num: String = after.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
    num.parse().ok()
}

/// Parse a `(x, y)` tuple from the start of a string.
/// Accepts `(250, 375)`, `(250,375)`, `250 375`, etc.
fn parse_tuple_str(s: &str) -> Option<(i32, i32)> {
    let s = s.trim_start_matches('(');
    let s = if let Some(end) = s.find(')') { &s[..end] } else { s };
    let mut parts = s.split([',', ' ', '\t']).filter(|p| !p.is_empty());
    let x: i32 = parts.next()?.trim().parse().ok()?;
    let y: i32 = parts.next()?.trim().parse().ok()?;
    Some((x, y))
}

/// Try to parse a JSON object, including code-fenced and prose-wrapped variants.
fn try_parse_json(text: &str) -> Option<ParsedVlmResponse> {
    // Strip code fence.
    let text = text
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    // If the cleaned text doesn't contain '{', skip.
    if !text.contains('{') {
        return None;
    }

    // Extract the first {...} block.
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end < start {
        return None;
    }
    let json_str = &text[start..=end];

    let raw: RawCoord = serde_json::from_str(json_str).ok()?;

    // Explicit "not found".
    if raw.found == Some(false) {
        return Some(ParsedVlmResponse { coord: None, action: None, not_found: true });
    }

    // start_box inside JSON (some UI-TARS variants).
    if let Some(ref sb) = raw.start_box {
        if let Some((x, y)) = parse_tuple_str(sb) {
            return Some(ParsedVlmResponse {
                coord: Some(ParsedCoord { x, y, space: CoordSpace::Norm }),
                action: None,
                not_found: false,
            });
        }
    }

    // x/y fields: decide space by magnitude.
    // Normalised coords are in [0, 1000]; pixel coords can exceed 1000.
    // We treat them as Norm if both fit in [0, 1000].
    match (raw.x, raw.y) {
        (Some(x), Some(y)) => {
            let space = if x >= 0 && x <= 1000 && y >= 0 && y <= 1000 {
                CoordSpace::Norm
            } else {
                CoordSpace::Pixels
            };
            Some(ParsedVlmResponse {
                coord: Some(ParsedCoord { x, y, space }),
                action: None,
                not_found: false,
            })
        }
        _ => {
            // JSON with neither coords nor found:false — treat as not-found.
            Some(ParsedVlmResponse { coord: None, action: None, not_found: true })
        }
    }
}

/// Try to parse a bare `(x, y)` tuple anywhere in the text.
fn try_parse_bare_tuple(text: &str) -> Option<ParsedVlmResponse> {
    // Find the first `(` followed by two numbers and `)`.
    let start = text.find('(')?;
    let (x, y) = parse_tuple_str(&text[start..])?;
    Some(ParsedVlmResponse {
        coord: Some(ParsedCoord { x, y, space: CoordSpace::Norm }),
        action: None,
        not_found: false,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper.
    fn coord(text: &str) -> Option<ParsedCoord> {
        parse_vlm_response(text)?.coord
    }

    fn not_found(text: &str) -> bool {
        parse_vlm_response(text).map_or(false, |r| r.not_found)
    }

    // --- JSON formats ---

    #[test]
    fn bare_json_xy() {
        let c = coord(r#"{"x": 300, "y": 450}"#).unwrap();
        assert_eq!(c.x, 300);
        assert_eq!(c.y, 450);
        assert_eq!(c.space, CoordSpace::Norm);
    }

    #[test]
    fn bare_json_xy_pixel_space() {
        // coords > 1000 => Pixels space
        let c = coord(r#"{"x": 1280, "y": 720}"#).unwrap();
        assert_eq!(c.x, 1280);
        assert_eq!(c.y, 720);
        assert_eq!(c.space, CoordSpace::Pixels);
    }

    #[test]
    fn code_fenced_json() {
        let c = coord("```json\n{\"x\": 100, \"y\": 200}\n```").unwrap();
        assert_eq!(c.x, 100);
        assert_eq!(c.y, 200);
    }

    #[test]
    fn code_fenced_plain() {
        let c = coord("```\n{\"x\": 100, \"y\": 200}\n```").unwrap();
        assert_eq!(c.x, 100);
        assert_eq!(c.y, 200);
    }

    #[test]
    fn prose_wrapped_json() {
        let c = coord("The element is at {\"x\": 250, \"y\": 375} on screen.").unwrap();
        assert_eq!(c.x, 250);
        assert_eq!(c.y, 375);
    }

    #[test]
    fn json_not_found_explicit() {
        assert!(not_found(r#"{"found": false}"#));
    }

    #[test]
    fn json_found_false_no_coord() {
        let r = parse_vlm_response(r#"{"found": false}"#).unwrap();
        assert!(r.not_found);
        assert!(r.coord.is_none());
    }

    // --- UI-TARS / Qwen action format ---

    #[test]
    fn uitars_click_start_box() {
        let r = parse_vlm_response("click(start_box='(250, 375)')").unwrap();
        let c = r.coord.unwrap();
        assert_eq!(c.x, 250);
        assert_eq!(c.y, 375);
        assert_eq!(c.space, CoordSpace::Norm);
        assert_eq!(r.action, Some(ParsedAction::Click));
    }

    #[test]
    fn uitars_double_click() {
        let r = parse_vlm_response("double_click(start_box='(100, 200)')").unwrap();
        assert_eq!(r.action, Some(ParsedAction::DoubleClick));
        let c = r.coord.unwrap();
        assert_eq!(c.x, 100);
        assert_eq!(c.y, 200);
    }

    #[test]
    fn uitars_type_with_content() {
        let r = parse_vlm_response("type(start_box='(300,400)', content='hello world')").unwrap();
        assert_eq!(r.action, Some(ParsedAction::Type("hello world".into())));
        let c = r.coord.unwrap();
        assert_eq!(c.x, 300);
        assert_eq!(c.y, 400);
    }

    #[test]
    fn uitars_scroll() {
        let r =
            parse_vlm_response("scroll(start_box='(500,500)', direction='down', amount=3)").unwrap();
        assert!(matches!(r.action, Some(ParsedAction::Scroll { .. })));
    }

    #[test]
    fn uitars_no_space_in_coords() {
        let c = coord("click(start_box='(999,1)')").unwrap();
        assert_eq!(c.x, 999);
        assert_eq!(c.y, 1);
    }

    // --- Bare tuple ---

    #[test]
    fn bare_tuple() {
        let c = coord("(640, 480)").unwrap();
        assert_eq!(c.x, 640);
        assert_eq!(c.y, 480);
        assert_eq!(c.space, CoordSpace::Norm);
    }

    // --- Not found markers ---

    #[test]
    fn prose_not_found() {
        assert!(not_found("I cannot find the element on the screen."));
    }

    #[test]
    fn prose_element_not_visible() {
        assert!(not_found("The button is not visible in the image."));
    }

    // --- Malformed / edge cases ---

    #[test]
    fn empty_string_returns_none() {
        assert!(parse_vlm_response("").is_none());
    }

    #[test]
    fn random_prose_returns_none() {
        assert!(parse_vlm_response("Here is some random text with no coords.").is_none());
    }

    #[test]
    fn malformed_json_returns_none_or_not_found() {
        // Incomplete JSON object - no coordinates
        let r = parse_vlm_response("{not valid json}");
        // Either None or a not_found response is acceptable.
        if let Some(r) = r {
            assert!(r.coord.is_none());
        }
    }

    #[test]
    fn action_call_with_double_quoted_args() {
        let r = parse_vlm_response(r#"click(start_box="(100, 200)")"#).unwrap();
        let c = r.coord.unwrap();
        assert_eq!((c.x, c.y), (100, 200));
    }

    #[test]
    fn edge_coords_zero() {
        let c = coord("click(start_box='(0, 0)')").unwrap();
        assert_eq!((c.x, c.y), (0, 0));
    }

    #[test]
    fn edge_coords_max_norm() {
        let c = coord("click(start_box='(1000, 1000)')").unwrap();
        assert_eq!((c.x, c.y), (1000, 1000));
    }
}
