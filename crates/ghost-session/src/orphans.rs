//! Startup hygiene: end the browsers left behind by Ghost servers that are
//! gone. Detection lives in `ghost_core::process::orphans`; the marker is the
//! profile root every Ghost-launched browser carries on its command line.

use serde_json::{json, Value};

/// Browser image names Ghost launches (see ghost-browser's KNOWN_BROWSERS).
const BROWSER_IMAGES: &[&str] = &["chrome.exe", "msedge.exe", "comet.exe", "brave.exe"];

/// Kill orphaned Ghost-launched browsers and describe each one ended.
#[cfg(windows)]
pub fn sweep_orphaned_browsers() -> Vec<Value> {
    let root = ghost_browser::launch::profiles_root();
    let marker = root.to_string_lossy().to_string();
    ghost_core::process::orphans::kill_orphans(&marker, BROWSER_IMAGES)
        .into_iter()
        .map(|o| json!({ "pid": o.pid, "name": o.name, "parent_pid": o.parent_pid }))
        .collect()
}

#[cfg(not(windows))]
pub fn sweep_orphaned_browsers() -> Vec<Value> {
    let _ = BROWSER_IMAGES;
    Vec::new()
}
