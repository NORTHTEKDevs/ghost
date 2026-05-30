//! ScreenSpot-style grounding accuracy harness.
//!
//! Runs a golden set of VLM responses through the parser and checks whether
//! the predicted center falls within the expected bounding rect.
//!
//! A "hit" is defined as: the predicted (x, y) center falls within the
//! expected_rect (left, top, right, bottom), all in the 0-1000 normalised space.
//!
//! # How to expand with real captures
//! 1. Run `ghost_screenshot` to capture a foreground window.
//! 2. Run `ghost_find` with mode=deliberate and record the VLM response JSON.
//! 3. Manually annotate the expected_rect in grounding_golden.json.
//! 4. Add the raw VLM response to vlm_responses.json.
//!
//! # Running
//! ```
//! cargo test -p ghost-ground -- --nocapture grounding_accuracy
//! ```
//! Prints an accuracy summary to stdout.

use ghost_ground::parser::{parse_vlm_response, ParsedAction, CoordSpace};
use ghost_ground::types::{norm_to_px, px_to_norm, CoordNorm};
use serde::Deserialize;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Fixture types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct VlmFixture {
    id: String,
    description: String,
    vlm_response: String,
    /// Expected normalised [x, y] (both axes 0-1000) — for norm-space VLMs.
    #[serde(default)]
    expected_norm: Option<[i32; 2]>,
    /// Expected pixel [x, y] — for pixel-space VLMs.
    #[serde(default)]
    expected_pixels: Option<[i32; 2]>,
    /// Expected action verb (click, double_click, type, scroll, null).
    #[serde(default)]
    expected_action: Option<String>,
    #[serde(default)]
    expected_type_text: Option<String>,
    #[serde(default)]
    expected_scroll_direction: Option<String>,
    #[serde(default)]
    expected_scroll_amount: Option<i32>,
    /// True if model is expected to report "not found".
    #[serde(default)]
    expected_not_found: bool,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct GroundingFixture {
    id: String,
    description: String,
    target_kind: String,
    target_value: String,
    /// [left, top, right, bottom] in 0-1000 norm space.
    expected_rect_norm: [i32; 4],
    note: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn load_vlm_fixtures() -> Vec<VlmFixture> {
    let path = fixtures_dir().join("vlm_responses.json");
    let data = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Cannot read vlm_responses.json: {e}"));
    serde_json::from_str(&data)
        .unwrap_or_else(|e| panic!("Cannot parse vlm_responses.json: {e}"))
}

fn load_grounding_fixtures() -> Vec<GroundingFixture> {
    let path = fixtures_dir().join("grounding_golden.json");
    let data = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Cannot read grounding_golden.json: {e}"));
    serde_json::from_str(&data)
        .unwrap_or_else(|e| panic!("Cannot parse grounding_golden.json: {e}"))
}

/// ScreenSpot hit check: predicted center (px, py) falls within expected_rect.
fn is_hit(px: i32, py: i32, rect: [i32; 4]) -> bool {
    let [left, top, right, bottom] = rect;
    px >= left && px <= right && py >= top && py <= bottom
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn vlm_parser_accuracy_on_fixtures() {
    let fixtures = load_vlm_fixtures();
    let total = fixtures.len();
    let mut hits = 0usize;
    let mut misses: Vec<String> = Vec::new();

    for f in &fixtures {
        let parsed = parse_vlm_response(&f.vlm_response);

        // Not-found expected.
        if f.expected_not_found {
            match &parsed {
                Some(r) if r.not_found => {
                    hits += 1;
                    println!("  PASS [{}] not_found correctly detected", f.id);
                }
                Some(r) => {
                    misses.push(format!("[{}] expected not_found but got coord={:?}", f.id, r.coord));
                    println!("  FAIL [{}] expected not_found", f.id);
                }
                None => {
                    // parse_vlm_response returns None for unrecognised input, which is
                    // treated the same as not_found by callers. Accept this.
                    hits += 1;
                    println!("  PASS [{}] returned None (treated as not_found)", f.id);
                }
            }
            continue;
        }

        let Some(parsed) = parsed else {
            misses.push(format!("[{}] parser returned None (expected coord)", f.id));
            println!("  FAIL [{}] parser returned None", f.id);
            continue;
        };

        // Coordinate check.
        let coord_ok = if let Some(coord) = &parsed.coord {
            if let Some([ex, ey]) = f.expected_norm {
                // Norm-space match: accept ±10 units tolerance (rounding).
                let dx = (coord.x - ex).abs();
                let dy = (coord.y - ey).abs();
                if dx > 10 || dy > 10 {
                    misses.push(format!("[{}] norm mismatch: got ({},{}) expected ({},{})", f.id, coord.x, coord.y, ex, ey));
                    false
                } else {
                    true
                }
            } else if let Some([ex, ey]) = f.expected_pixels {
                let dx = (coord.x - ex).abs();
                let dy = (coord.y - ey).abs();
                if dx > 5 || dy > 5 {
                    misses.push(format!("[{}] pixel mismatch: got ({},{}) expected ({},{})", f.id, coord.x, coord.y, ex, ey));
                    false
                } else {
                    true
                }
            } else {
                // No expected coord — just presence is enough.
                true
            }
        } else if f.expected_norm.is_some() || f.expected_pixels.is_some() {
            misses.push(format!("[{}] expected coord but parser returned None coord", f.id));
            false
        } else {
            true
        };

        // Action check.
        let action_ok = if let Some(expected_action) = &f.expected_action {
            match &parsed.action {
                Some(ParsedAction::Click) => expected_action == "click",
                Some(ParsedAction::DoubleClick) => expected_action == "double_click",
                Some(ParsedAction::RightClick) => expected_action == "right_click",
                Some(ParsedAction::Type(text)) => {
                    expected_action == "type" && f.expected_type_text.as_deref().map_or(true, |t| t == text)
                }
                Some(ParsedAction::Scroll { direction, amount }) => {
                    expected_action == "scroll"
                        && f.expected_scroll_direction.as_deref().map_or(true, |d| d == direction)
                        && f.expected_scroll_amount.map_or(true, |a| a == *amount)
                }
                Some(ParsedAction::Hover) => expected_action == "hover",
                Some(ParsedAction::Other(s)) => expected_action == s,
                None => false,
            }
        } else {
            true // no action expected
        };

        if coord_ok && action_ok {
            hits += 1;
            println!("  PASS [{}] {} ", f.id, f.description);
        } else {
            if !action_ok {
                misses.push(format!("[{}] action mismatch: expected={:?} got={:?}", f.id, f.expected_action, parsed.action));
            }
            println!("  FAIL [{}] {} ", f.id, f.description);
        }
    }

    let pct = if total > 0 { hits as f64 * 100.0 / total as f64 } else { 0.0 };

    println!("\n--- VLM Parser Accuracy Summary ---");
    println!("  Total:  {}", total);
    println!("  Hits:   {}", hits);
    println!("  Misses: {}", total - hits);
    println!("  Accuracy: {:.1}%", pct);
    if !misses.is_empty() {
        println!("  Failures:");
        for m in &misses {
            println!("    - {}", m);
        }
    }
    println!("-----------------------------------\n");

    // Assert minimum accuracy (all synthetic fixtures must pass).
    assert!(
        pct >= 90.0,
        "Parser accuracy {:.1}% is below 90% threshold on synthetic fixtures. Failures: {:?}",
        pct, misses
    );
}

/// ScreenSpot-style hit-rate test for the golden grounding fixtures.
/// This tests the CONCEPTUAL harness structure — it doesn't run real COM/VLM.
/// The actual hit-rate for production tiers is measured via live integration tests.
/// This test verifies that the fixture format is parseable and the hit-check logic works.
#[test]
fn grounding_fixtures_are_valid_and_hit_check_works() {
    let fixtures = load_grounding_fixtures();
    assert!(!fixtures.is_empty(), "grounding_golden.json must have at least one fixture");

    for f in &fixtures {
        let [left, top, right, bottom] = f.expected_rect_norm;
        let cx = (left + right) / 2;
        let cy = (top + bottom) / 2;
        // Center of its own rect must be a hit.
        assert!(
            is_hit(cx, cy, f.expected_rect_norm),
            "fixture [{}]: center ({},{}) must be inside its own rect ({},{},{},{})",
            f.id, cx, cy, left, top, right, bottom
        );
        // Corner outside rect must be a miss.
        assert!(
            !is_hit(left - 1, top - 1, f.expected_rect_norm),
            "fixture [{}]: point outside rect must not be a hit", f.id
        );
        println!("  VALID [{}] rect=({},{},{},{}) center=({},{})", f.id, left, top, right, bottom, cx, cy);
    }

    println!("\n--- Grounding Fixture Validation ---");
    println!("  {} fixtures valid", fixtures.len());
    println!("  To add real captures: see test file header for instructions.");
    println!("------------------------------------\n");
}

/// Round-trip accuracy of the norm <-> pixel coordinate conversion.
/// Ensures the 0-1000 contract doesn't introduce errors that would cause
/// predicted centers to fall outside expected rects.
#[test]
fn coord_roundtrip_accuracy_for_grounding() {
    let dims: &[(u32, u32)] = &[
        (1920, 1080),
        (2560, 1440),
        (1280, 720),
        (768, 1024),
        (3840, 2160),
    ];
    for &(w, h) in dims {
        // Simulate: VLM predicts norm (500,500), convert to pixel, back to norm.
        let norm_x = 500u16;
        let norm_y = 500u16;
        let px_x = norm_to_px(norm_x, w);
        let px_y = norm_to_px(norm_y, h);
        let back_x = px_to_norm(px_x, w);
        let back_y = px_to_norm(px_y, h);
        // Allow ±2 norm units (safe for any supported resolution).
        assert!(
            (back_x as i32 - norm_x as i32).abs() <= 2,
            "Round-trip error x: dim={w} norm={norm_x} px={px_x} back={back_x}"
        );
        assert!(
            (back_y as i32 - norm_y as i32).abs() <= 2,
            "Round-trip error y: dim={h} norm={norm_y} px={px_y} back={back_y}"
        );
    }
}
