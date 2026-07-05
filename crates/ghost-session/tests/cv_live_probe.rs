//! Live probe: run the CPU CV detector on a REAL captured desktop region (not a
//! synthetic image) and report how many element-like regions it finds. Proves the
//! detector produces sane output on actual UI pixels. Run:
//!   cargo test -p ghost-session --test cv_live_probe -- --ignored --nocapture

use ghost_ground::cv_detect::{detect_regions, Opts};

#[test]
#[ignore]
fn detects_regions_on_real_screen() {
    // Capture a 800x600 chunk of the primary desktop (taskbar/icons/whatever is up).
    let rect = (0, 0, 800, 600);
    let (rgba, w, h) = ghost_core::capture::capture_region_raw(Some(rect))
        .expect("capture failed");
    assert_eq!(rgba.len(), w * h * 4, "buffer size mismatch");
    let regions = detect_regions(&rgba, w, h, &Opts::default());
    println!("CV detector on real {w}x{h} desktop capture: {} regions", regions.len());
    for (i, r) in regions.iter().take(8).enumerate() {
        let (cx, cy) = r.center();
        println!("  #{}: rect={:?} center=({cx},{cy}) conf={:.2}", i + 1, r.rect, r.confidence);
    }
    // A non-blank desktop region should yield at least a few element-like boxes,
    // and never an absurd number (the cap is 64).
    assert!(!regions.is_empty(), "expected >0 regions on a real desktop");
    assert!(regions.len() <= 64, "region count exceeded cap");
}
