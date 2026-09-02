//! Manual measurement: per-node walker vs one batched `FindAllBuildCache` for a
//! window-scoped describe. Read-only (a UIA client walk), so it is safe to run
//! against whatever window is open. `#[ignore]`d: it needs a live window.
//!
//!   GHOST_BENCH_WINDOW="Comet" cargo test -p ghost-core --test describe_walk_bench -- --ignored --nocapture

#![cfg(windows)]

use ghost_core::uia::{init_com, list_windows, UiaTree};
use std::time::Instant;

#[test]
#[ignore]
fn walker_vs_cached_describe_on_a_live_window() {
    let _com = init_com().expect("com");
    let needle = std::env::var("GHOST_BENCH_WINDOW").unwrap_or_else(|_| "Comet".into());
    let win = list_windows()
        .expect("windows")
        .into_iter()
        .find(|w| w.name.to_lowercase().contains(&needle.to_lowercase()) && w.state != "minimized")
        .unwrap_or_else(|| panic!("no window matching {needle}"));
    let tree = UiaTree::new().expect("tree");
    for round in 0..3 {
        let t = Instant::now();
        let a = tree.describe_hwnd(win.hwnd).expect("walker");
        let walker_ms = t.elapsed().as_millis();
        let t = Instant::now();
        let b = tree.describe_hwnd_cached(win.hwnd).expect("cached");
        let cached_ms = t.elapsed().as_millis();
        eprintln!(
            "round {round} '{}': walker {walker_ms} ms / {} elements   cached {cached_ms} ms / {} elements",
            win.name,
            a.len(),
            b.len()
        );
    }
}
