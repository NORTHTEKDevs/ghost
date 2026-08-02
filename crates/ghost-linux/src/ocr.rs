//! Local text location.
//!
//! The Windows engine backs `find_text_local` with the always-present
//! Windows.Media.Ocr runtime. Linux ships no equivalent by default, so this uses
//! **Tesseract** when it is installed and reports honestly when it is not.
//!
//! Tesseract is invoked as a subprocess rather than linked. Linking `leptonica`
//! and `tesseract` would drag C dependencies into a crate that is otherwise pure
//! Rust, forcing every Linux user to install `-devel` packages for a fallback
//! path most of them never hit. Shelling out keeps `dnf install tesseract`
//! optional and the build unchanged.
//!
//! Text location is usually better served by the accessibility tree anyway --
//! `ghost_find`/`ghost_see` return real text with real bounds and no OCR error.
//! This exists for the same case it does on Windows: canvas-drawn or
//! remote-desktop content that exposes no accessible text at all.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::capture::screen::{capture_region_raw, encode_rgba, CaptureFormat};
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

impl TextHit {
    pub fn center(&self) -> (i32, i32) {
        ((self.left + self.right) / 2, (self.top + self.bottom) / 2)
    }
}

/// Is Tesseract available?
pub fn is_available() -> bool {
    Command::new("tesseract")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Locate `needle` on screen, returning its centre in screen coordinates.
///
/// Signature matches `ghost_core::ocr::find_text_local`.
pub fn find_text_local(
    needle: &str,
    region: Option<(i32, i32, i32, i32)>,
) -> Result<Option<(i32, i32)>> {
    let hits = find_all(needle, region)?;
    Ok(hits.first().map(|h| h.center()))
}

/// Every match for `needle`, best confidence first.
pub fn find_all(needle: &str, region: Option<(i32, i32, i32, i32)>) -> Result<Vec<TextHit>> {
    if !is_available() {
        return Err(CoreError::Unsupported(
            "local OCR needs Tesseract, which is not installed. Install it with: \
             sudo dnf install tesseract  --  or prefer ghost_find/ghost_see, which read real text \
             with real bounds from the accessibility tree and need no OCR"
                .into(),
        ));
    }

    let (rgba, w, h) = capture_region_raw(region)?;
    let png = encode_rgba(&rgba, w, h, CaptureFormat::Png)?;

    // Origin of the captured area, so hits come back in screen coordinates
    // rather than crop-relative ones.
    let (ox, oy) = region.map(|(l, t, _, _)| (l, t)).unwrap_or((0, 0));

    let tsv = run_tesseract(&png)?;
    let needle_lc = needle.to_ascii_lowercase();

    let mut hits: Vec<TextHit> = parse_tsv(&tsv, ox, oy)
        .into_iter()
        .filter(|hit| hit.text.to_ascii_lowercase().contains(&needle_lc))
        .collect();

    // Best confidence first, so `find_text_local` returns the strongest match.
    hits.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
    Ok(hits)
}

/// Run Tesseract over a PNG on stdin, asking for TSV so we get per-word boxes.
fn run_tesseract(png: &[u8]) -> Result<String> {
    let mut child = Command::new("tesseract")
        .args(["stdin", "stdout", "--psm", "11", "tsv"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| CoreError::platform(format!("running tesseract: {e}")))?;

    child
        .stdin
        .take()
        .ok_or_else(|| CoreError::platform("tesseract stdin unavailable"))?
        .write_all(png)
        .map_err(|e| CoreError::platform(format!("writing image to tesseract: {e}")))?;

    let out = child
        .wait_with_output()
        .map_err(|e| CoreError::platform(format!("waiting for tesseract: {e}")))?;

    if !out.status.success() {
        return Err(CoreError::platform("tesseract exited with an error"));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parse Tesseract's TSV into screen-space hits.
///
/// Columns are: level, page, block, par, line, word, left, top, width, height,
/// conf, text. Rows with confidence -1 are layout rows, not words.
fn parse_tsv(tsv: &str, ox: i32, oy: i32) -> Vec<TextHit> {
    let mut out = Vec::new();
    for line in tsv.lines().skip(1) {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 12 {
            continue;
        }
        let text = cols[11].trim();
        if text.is_empty() {
            continue;
        }
        let conf: f32 = cols[10].parse().unwrap_or(-1.0);
        if conf < 0.0 {
            continue;
        }
        let (Ok(l), Ok(t), Ok(w), Ok(h)) = (
            cols[6].parse::<i32>(),
            cols[7].parse::<i32>(),
            cols[8].parse::<i32>(),
            cols[9].parse::<i32>(),
        ) else {
            continue;
        };
        out.push(TextHit {
            text: text.to_string(),
            left: ox + l,
            top: oy + t,
            right: ox + l + w,
            bottom: oy + t + h,
            confidence: conf / 100.0,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
5\t1\t1\t1\t1\t1\t100\t200\t60\t20\t95.5\tSave\n\
5\t1\t1\t1\t1\t2\t170\t200\t80\t20\t88.0\tCancel\n\
4\t1\t1\t1\t1\t0\t0\t0\t500\t50\t-1\t\n";

    #[test]
    fn parses_word_boxes_and_skips_layout_rows() {
        let hits = parse_tsv(SAMPLE, 0, 0);
        assert_eq!(hits.len(), 2, "the conf=-1 layout row must be skipped");
        assert_eq!(hits[0].text, "Save");
        assert_eq!((hits[0].left, hits[0].top, hits[0].right, hits[0].bottom), (100, 200, 160, 220));
    }

    #[test]
    fn hits_are_offset_into_screen_coordinates() {
        // A region capture starts at the region's origin; without adding it back
        // every hit would be reported relative to the crop and clicks would land
        // in the wrong place.
        let hits = parse_tsv(SAMPLE, 1000, 500);
        assert_eq!((hits[0].left, hits[0].top), (1100, 700));
    }

    #[test]
    fn confidence_is_normalised_to_zero_to_one() {
        let hits = parse_tsv(SAMPLE, 0, 0);
        assert!((hits[0].confidence - 0.955).abs() < 0.001);
    }

    #[test]
    fn centre_is_the_box_midpoint() {
        let hits = parse_tsv(SAMPLE, 0, 0);
        assert_eq!(hits[0].center(), (130, 210));
    }

    #[test]
    fn malformed_rows_are_skipped_rather_than_panicking() {
        let junk = "level\ttext\nnot\tenough\tcolumns\n\t\t\t\t\t\t\t\t\t\t\t\n";
        assert!(parse_tsv(junk, 0, 0).is_empty());
    }

    #[test]
    fn missing_tesseract_names_the_install_command_and_the_better_alternative() {
        if !is_available() {
            let err = find_all("Save", None).unwrap_err();
            let m = err.to_string();
            assert!(m.contains("dnf install tesseract"), "must name the install command: {m}");
            assert!(m.contains("ghost_find"), "must point at the better path: {m}");
        }
    }
}
