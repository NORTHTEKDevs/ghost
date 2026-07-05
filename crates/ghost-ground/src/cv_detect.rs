//! CPU classical-CV element detector — a GPU-free, dependency-free fallback that
//! proposes interactable-region bounding boxes for windows with no useful
//! accessibility tree (canvas apps, custom-drawn UIs, remote-desktop surfaces).
//!
//! # Why this exists
//! Set-of-Marks grounding (see [`crate::yolo`]) needs a list of candidate regions
//! to overlay numbered marks on. The UIA tree provides those for normal apps; the
//! optional ONNX OmniParser tier ([`crate::yolo::YoloDetector`]) provides them when
//! a GPU model is available. This module is the always-available middle ground: it
//! finds element-like boxes from pixels alone using edge density + connected
//! components — no model download, no GPU, no external crates. It is less precise
//! than OmniParser but turns "no candidates at all" into "usable candidates" for
//! the canvas case.
//!
//! # Algorithm (all integer, single pass per stage)
//! 1. Luma (grayscale) from RGBA.
//! 2. Gradient magnitude per pixel (|dx| + |dy| of luma) → edge mask.
//! 3. Coarse cell grid (`cell_px`): a cell is "content" if it holds enough edge
//!    pixels. This denoises single-pixel edges and shrinks the union-find.
//! 4. 4-connected components over content cells (union-find).
//! 5. Per component: pixel bounding box, filtered by min/max area and aspect,
//!    scored by edge density, capped to `max_regions`, then de-nested via
//!    [`crate::marks::remove_overlapping`].
//!
//! Coordinates returned are LOCAL to the input image (0,0 = top-left of `rgba`);
//! the caller adds the capture rect's origin to get absolute screen pixels.

use crate::marks::{remove_overlapping, Region};

/// Tunable detector parameters. [`Opts::default`] is calibrated for typical
/// 1x-DPI desktop UI at native capture resolution.
#[derive(Debug, Clone)]
pub struct Opts {
    /// Coarse grid cell size in pixels. Larger = fewer, blobbier regions.
    pub cell_px: usize,
    /// Per-pixel gradient-magnitude threshold (0..=510) to count as an edge.
    pub edge_threshold: u16,
    /// A grid cell is "content" if it contains at least this many edge pixels.
    pub min_edge_pixels_per_cell: usize,
    /// Discard regions smaller than this many pixels of area.
    pub min_area_px: i64,
    /// Discard regions larger than this fraction of the whole image (the
    /// whole-window blob is not a useful click target).
    pub max_area_frac: f32,
    /// Discard regions whose width/height aspect ratio is outside
    /// `[1/max_aspect, max_aspect]` (drops thin rules/borders).
    pub max_aspect: f32,
    /// Keep at most this many regions (highest edge-density first).
    pub max_regions: usize,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            cell_px: 6,
            edge_threshold: 40,
            min_edge_pixels_per_cell: 2,
            // 16x16: real clickable targets are ~>=16px; also filters coarse-grid
            // snap inflation of sub-cell specks (a 3x3 dot snaps to ~12x12).
            min_area_px: 16 * 16,
            max_area_frac: 0.6,
            max_aspect: 12.0,
            max_regions: 64,
        }
    }
}

#[inline]
fn luma(r: u8, g: u8, b: u8) -> i32 {
    // Rec.601-ish integer luma, 0..255.
    (77 * r as i32 + 150 * g as i32 + 29 * b as i32) >> 8
}

/// Detect interactable-region bounding boxes in a tightly-packed RGBA image.
///
/// Returns regions in image-local coordinates (add the capture origin for
/// absolute screen pixels). Empty when the image is too small or featureless.
pub fn detect_regions(rgba: &[u8], width: usize, height: usize, opts: &Opts) -> Vec<Region> {
    if width < 2 || height < 2 || rgba.len() < width * height * 4 || opts.cell_px == 0 {
        return Vec::new();
    }

    // --- Stage 1+2: luma → gradient edge mask (1 byte/pixel: 1 = edge). ---
    let mut lum = vec![0i32; width * height];
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            lum[y * width + x] = luma(rgba[i], rgba[i + 1], rgba[i + 2]);
        }
    }
    let mut edge = vec![0u8; width * height];
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let c = y * width + x;
            let dx = (lum[c + 1] - lum[c - 1]).unsigned_abs();
            let dy = (lum[c + width] - lum[c - width]).unsigned_abs();
            if (dx + dy) as u16 >= opts.edge_threshold {
                edge[c] = 1;
            }
        }
    }

    // --- Stage 3: coarse content-cell grid. ---
    let cell = opts.cell_px;
    let gw = width.div_ceil(cell);
    let gh = height.div_ceil(cell);
    let mut content = vec![false; gw * gh];
    for gy in 0..gh {
        for gx in 0..gw {
            let mut count = 0usize;
            let y0 = gy * cell;
            let x0 = gx * cell;
            for y in y0..(y0 + cell).min(height) {
                let row = y * width;
                for x in x0..(x0 + cell).min(width) {
                    count += edge[row + x] as usize;
                }
            }
            if count >= opts.min_edge_pixels_per_cell {
                content[gy * gw + gx] = true;
            }
        }
    }

    // --- Stage 4: 4-connected components over content cells (union-find). ---
    let mut parent: Vec<usize> = (0..gw * gh).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]]; // path halving
            i = parent[i];
        }
        i
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra.max(rb)] = ra.min(rb);
        }
    }
    for gy in 0..gh {
        for gx in 0..gw {
            let idx = gy * gw + gx;
            if !content[idx] {
                continue;
            }
            if gx + 1 < gw && content[idx + 1] {
                union(&mut parent, idx, idx + 1);
            }
            if gy + 1 < gh && content[idx + gw] {
                union(&mut parent, idx, idx + gw);
            }
        }
    }

    // --- Stage 5: per-component bbox (in pixels) + edge count for density. ---
    use std::collections::HashMap;
    struct Acc {
        min_gx: usize,
        min_gy: usize,
        max_gx: usize,
        max_gy: usize,
        cells: usize,
    }
    let mut comps: HashMap<usize, Acc> = HashMap::new();
    for gy in 0..gh {
        for gx in 0..gw {
            let idx = gy * gw + gx;
            if !content[idx] {
                continue;
            }
            let root = find(&mut parent, idx);
            let a = comps.entry(root).or_insert(Acc {
                min_gx: gx,
                min_gy: gy,
                max_gx: gx,
                max_gy: gy,
                cells: 0,
            });
            a.min_gx = a.min_gx.min(gx);
            a.min_gy = a.min_gy.min(gy);
            a.max_gx = a.max_gx.max(gx);
            a.max_gy = a.max_gy.max(gy);
            a.cells += 1;
        }
    }

    let img_area = (width * height) as f32;
    let mut regions: Vec<Region> = Vec::new();
    for a in comps.values() {
        let l = (a.min_gx * cell) as i32;
        let t = (a.min_gy * cell) as i32;
        let r = (((a.max_gx + 1) * cell).min(width)) as i32;
        let b = (((a.max_gy + 1) * cell).min(height)) as i32;
        let reg = Region { rect: (l, t, r, b), confidence: 0.0 };
        let area = reg.area();
        if area < opts.min_area_px {
            continue;
        }
        if area as f32 > img_area * opts.max_area_frac {
            continue;
        }
        let w = (r - l) as f32;
        let h = (b - t) as f32;
        let aspect = (w / h.max(1.0)).max(h / w.max(1.0));
        if aspect > opts.max_aspect {
            continue;
        }
        // Confidence: fraction of the bbox's cells that are content (0..1).
        let bbox_cells = ((a.max_gx - a.min_gx + 1) * (a.max_gy - a.min_gy + 1)) as f32;
        let conf = (a.cells as f32 / bbox_cells.max(1.0)).clamp(0.05, 1.0);
        regions.push(Region { rect: (l, t, r, b), confidence: conf });
    }

    // Highest-confidence first, cap, then de-nest overlapping boxes.
    regions.sort_by(|x, y| y.confidence.partial_cmp(&x.confidence).unwrap_or(std::cmp::Ordering::Equal));
    regions.truncate(opts.max_regions);
    remove_overlapping(regions, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a solid-color RGBA image, returning a mutable buffer for drawing.
    fn canvas(w: usize, h: usize, bg: (u8, u8, u8)) -> Vec<u8> {
        let mut v = vec![0u8; w * h * 4];
        for px in v.chunks_exact_mut(4) {
            px[0] = bg.0;
            px[1] = bg.1;
            px[2] = bg.2;
            px[3] = 255;
        }
        v
    }

    /// Fill a rectangle [l,r)×[t,b) with a color.
    fn fill(buf: &mut [u8], w: usize, l: usize, t: usize, r: usize, b: usize, c: (u8, u8, u8)) {
        for y in t..b {
            for x in l..r {
                let i = (y * w + x) * 4;
                buf[i] = c.0;
                buf[i + 1] = c.1;
                buf[i + 2] = c.2;
                buf[i + 3] = 255;
            }
        }
    }

    /// Does any detected region's center fall inside the given rect?
    fn covered(regions: &[Region], l: i32, t: i32, r: i32, b: i32) -> bool {
        regions.iter().any(|reg| {
            let (cx, cy) = reg.center();
            cx >= l && cx < r && cy >= t && cy < b
        })
    }

    #[test]
    fn blank_image_yields_no_regions() {
        let img = canvas(120, 120, (30, 30, 30));
        let regions = detect_regions(&img, 120, 120, &Opts::default());
        assert!(regions.is_empty(), "blank image should have no edges, got {}", regions.len());
    }

    #[test]
    fn detects_two_separated_buttons() {
        let (w, h) = (240usize, 160usize);
        let mut img = canvas(w, h, (20, 20, 20));
        // Two well-separated light rectangles ("buttons").
        fill(&mut img, w, 20, 20, 90, 60, (220, 220, 220));   // button A
        fill(&mut img, w, 150, 100, 220, 140, (200, 200, 200)); // button B
        let regions = detect_regions(&img, w, h, &Opts::default());
        assert!(covered(&regions, 20, 20, 90, 60), "button A not detected; regions={regions:?}");
        assert!(covered(&regions, 150, 100, 220, 140), "button B not detected; regions={regions:?}");
    }

    #[test]
    fn button_bbox_approximates_the_shape() {
        let (w, h) = (200usize, 120usize);
        let mut img = canvas(w, h, (15, 15, 15));
        fill(&mut img, w, 40, 30, 140, 90, (240, 240, 240)); // rect (40,30)-(140,90)
        let regions = detect_regions(&img, w, h, &Opts::default());
        // Exactly one dominant region whose bbox brackets the drawn rect (within a cell).
        let hit = regions.iter().find(|reg| covered(&[(*reg).clone()], 40, 30, 140, 90));
        let reg = hit.expect("no region over the drawn rect");
        let (l, t, r, b) = reg.rect;
        let cell = Opts::default().cell_px as i32;
        assert!((l - 40).abs() <= cell && (t - 30).abs() <= cell, "top-left off: {reg:?}");
        assert!((r - 140).abs() <= cell && (b - 90).abs() <= cell, "bottom-right off: {reg:?}");
    }

    #[test]
    fn regions_map_to_som_ids() {
        // Detected regions must be consumable by the existing Set-of-Marks mapping.
        let (w, h) = (240usize, 160usize);
        let mut img = canvas(w, h, (20, 20, 20));
        fill(&mut img, w, 20, 20, 90, 60, (220, 220, 220));
        fill(&mut img, w, 150, 100, 220, 140, (200, 200, 200));
        let regions = detect_regions(&img, w, h, &Opts::default());
        assert!(!regions.is_empty());
        let g = crate::marks::som_id_to_grounded(&regions, 1, crate::types::Tier::Cv)
            .expect("id 1 should map");
        assert_eq!(g.source, crate::types::Tier::Cv);
    }

    #[test]
    fn tiny_speck_is_filtered() {
        let (w, h) = (120usize, 120usize);
        let mut img = canvas(w, h, (20, 20, 20));
        fill(&mut img, w, 10, 10, 13, 13, (255, 255, 255)); // 3x3 speck << min_area
        let regions = detect_regions(&img, w, h, &Opts::default());
        assert!(regions.is_empty(), "3x3 speck should be below min_area, got {regions:?}");
    }

    #[test]
    fn undersized_image_is_safe() {
        let img = canvas(1, 1, (0, 0, 0));
        assert!(detect_regions(&img, 1, 1, &Opts::default()).is_empty());
        // Truncated buffer must not panic.
        assert!(detect_regions(&[0u8; 4], 50, 50, &Opts::default()).is_empty());
    }
}
