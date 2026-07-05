//! Set-of-Marks geometry — detector-agnostic region type, overlap fusion, and
//! ID→region mapping. Always compiled (no feature gate, no external deps) so both
//! the CPU detector ([`crate::cv_detect`]) and the optional ONNX detector
//! ([`crate::yolo`], feature `yolo`) share one representation.

use crate::types::{Grounded, Tier};

/// A detected region (bounding box in image-local or absolute screen pixels).
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    /// Bounding box: (left, top, right, bottom).
    pub rect: (i32, i32, i32, i32),
    /// Detector confidence, 0.0..=1.0.
    pub confidence: f32,
}

impl Region {
    pub fn center(&self) -> (i32, i32) {
        let cx = (self.rect.0 + self.rect.2) / 2;
        let cy = (self.rect.1 + self.rect.3) / 2;
        (cx, cy)
    }

    pub fn area(&self) -> i64 {
        let w = (self.rect.2 - self.rect.0).max(0) as i64;
        let h = (self.rect.3 - self.rect.1).max(0) as i64;
        w * h
    }

    /// Intersection area with another region.
    pub fn intersection_area(&self, other: &Region) -> i64 {
        let left = self.rect.0.max(other.rect.0);
        let top = self.rect.1.max(other.rect.1);
        let right = self.rect.2.min(other.rect.2);
        let bottom = self.rect.3.min(other.rect.3);
        let w = (right - left).max(0) as i64;
        let h = (bottom - top).max(0) as i64;
        w * h
    }

    /// Returns the fraction of `self`'s area that overlaps with `other`.
    pub fn overlap_fraction(&self, other: &Region) -> f32 {
        let self_area = self.area();
        if self_area == 0 {
            return 0.0;
        }
        self.intersection_area(other) as f32 / self_area as f32
    }
}

// ---------------------------------------------------------------------------
// Set-of-Marks ID mapping
// ---------------------------------------------------------------------------

/// Map a 1-based Set-of-Marks ID to the corresponding region.
///
/// Returns `None` if `id` is out of range.
pub fn som_id_to_region(regions: &[Region], id: usize) -> Option<&Region> {
    if id == 0 || id > regions.len() {
        return None;
    }
    Some(&regions[id - 1])
}

/// Convert a Set-of-Marks ID to a [`Grounded`] result using the region center.
/// `source` labels which detector produced the region (`Tier::Yolo` / `Tier::Cv`).
pub fn som_id_to_grounded(regions: &[Region], id: usize, source: Tier) -> Option<Grounded> {
    let r = som_id_to_region(regions, id)?;
    Some(Grounded::from_rect(r.rect, r.confidence, source))
}

// ---------------------------------------------------------------------------
// Overlap removal — OmniParser asymmetric containment
// ---------------------------------------------------------------------------

/// Overlap-removal threshold. A region is removed if this fraction of its
/// area is covered by a larger region (asymmetric — the larger region is kept).
const CONTAINMENT_THRESHOLD: f32 = 0.90;

/// OmniParser-style asymmetric containment overlap removal.
///
/// Removes a region from `candidates` if ≥ [`CONTAINMENT_THRESHOLD`] (90%) of its
/// area is covered by a region in `dominators`. Also removes from `candidates`
/// any pair where one candidate is largely contained in another.
///
/// Used to fuse detector boxes with OCR text boxes (or de-nest a detector's own
/// output): a box mostly inside a larger box is redundant and is removed.
pub fn remove_overlapping(icon_regions: Vec<Region>, ocr_regions: &[Region]) -> Vec<Region> {
    // Step 1: remove any icon region that is largely contained by an OCR region.
    let filtered: Vec<Region> = icon_regions
        .into_iter()
        .filter(|icon| {
            for ocr in ocr_regions {
                if icon.overlap_fraction(ocr) >= CONTAINMENT_THRESHOLD {
                    return false; // icon is inside an OCR box — discard
                }
            }
            true
        })
        .collect();

    // Step 2: within the remaining icon regions, remove smaller ones contained
    // by larger ones.
    let n = filtered.len();
    let mut keep = vec![true; n];
    for i in 0..n {
        for j in 0..n {
            if i == j || !keep[i] {
                continue;
            }
            if filtered[i].overlap_fraction(&filtered[j]) >= CONTAINMENT_THRESHOLD
                && filtered[j].area() > filtered[i].area()
            {
                keep[i] = false;
            }
        }
    }

    filtered.into_iter().enumerate().filter(|(i, _)| keep[*i]).map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(l: i32, t: i32, r: i32, b: i32) -> Region {
        Region { rect: (l, t, r, b), confidence: 0.8 }
    }

    #[test]
    fn no_overlap_gives_zero_fraction() {
        let a = region(0, 0, 10, 10);
        let b = region(20, 20, 30, 30);
        assert_eq!(a.overlap_fraction(&b), 0.0);
    }

    #[test]
    fn full_containment_gives_one() {
        let small = region(2, 2, 8, 8);
        let big = region(0, 0, 10, 10);
        let frac = small.overlap_fraction(&big);
        assert!((frac - 1.0).abs() < 1e-5, "frac={frac}");
    }

    #[test]
    fn partial_overlap() {
        let a = region(0, 0, 10, 10);
        let b = region(5, 0, 15, 10);
        let frac = a.overlap_fraction(&b);
        assert!((frac - 0.5).abs() < 1e-5, "frac={frac}");
    }

    #[test]
    fn zero_area_region_gives_zero_fraction() {
        let zero = region(5, 5, 5, 5);
        let big = region(0, 0, 10, 10);
        assert_eq!(zero.overlap_fraction(&big), 0.0);
    }

    #[test]
    fn icon_inside_ocr_is_removed() {
        let icon = region(1, 1, 9, 9);
        let ocr = region(0, 0, 10, 10);
        let result = remove_overlapping(vec![icon], &[ocr]);
        assert!(result.is_empty(), "icon inside OCR box should be removed");
    }

    #[test]
    fn icon_outside_ocr_is_kept() {
        let icon = region(50, 50, 80, 80);
        let ocr = region(0, 0, 10, 10);
        let result = remove_overlapping(vec![icon.clone()], &[ocr]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rect, icon.rect);
    }

    #[test]
    fn smaller_icon_inside_larger_icon_is_removed() {
        let small = region(2, 2, 8, 8);
        let large = region(0, 0, 10, 10);
        let result = remove_overlapping(vec![small, large.clone()], &[]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rect, large.rect);
    }

    #[test]
    fn non_overlapping_icons_both_kept() {
        let a = region(0, 0, 10, 10);
        let b = region(20, 20, 30, 30);
        let result = remove_overlapping(vec![a.clone(), b.clone()], &[]);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn partially_overlapping_icon_kept_if_below_threshold() {
        let icon = region(0, 0, 10, 10);
        let ocr = region(6, 0, 16, 10);
        let result = remove_overlapping(vec![icon], &[ocr]);
        assert_eq!(result.len(), 1, "40% overlap should not trigger removal");
    }

    #[test]
    fn som_id_1_maps_to_first_region() {
        let regions = vec![region(0, 0, 10, 10), region(20, 20, 30, 30)];
        let r = som_id_to_region(&regions, 1).unwrap();
        assert_eq!(r.rect, (0, 0, 10, 10));
    }

    #[test]
    fn som_id_2_maps_to_second_region() {
        let regions = vec![region(0, 0, 10, 10), region(20, 20, 30, 30)];
        let r = som_id_to_region(&regions, 2).unwrap();
        assert_eq!(r.rect, (20, 20, 30, 30));
    }

    #[test]
    fn som_id_0_returns_none() {
        let regions = vec![region(0, 0, 10, 10)];
        assert!(som_id_to_region(&regions, 0).is_none());
    }

    #[test]
    fn som_id_out_of_range_returns_none() {
        let regions = vec![region(0, 0, 10, 10)];
        assert!(som_id_to_region(&regions, 5).is_none());
    }

    #[test]
    fn som_grounded_has_correct_center_and_source() {
        let regions = vec![region(0, 0, 20, 10)];
        let g = som_id_to_grounded(&regions, 1, Tier::Cv).unwrap();
        assert_eq!(g.center, (10, 5));
        assert_eq!(g.source, Tier::Cv);
    }

    #[test]
    fn region_center_correct() {
        let r = region(0, 0, 100, 50);
        assert_eq!(r.center(), (50, 25));
    }

    #[test]
    fn region_area_correct() {
        let r = region(0, 0, 10, 5);
        assert_eq!(r.area(), 50);
    }

    #[test]
    fn negative_rect_gives_zero_area() {
        let r = region(10, 0, 5, 5);
        assert_eq!(r.area(), 0);
    }
}
