//! A window that something hid outright is findable and restorable.
//!
//! On 2026-09-04 two browser windows an agent had driven ended the day neither
//! visible nor minimised. A window in that state is gone from the taskbar, from
//! Alt-Tab, and from `ghost_window op=list`, so nothing could bring it back and
//! the tabs looked lost. 0.21.1 added the listing and the restore path; this
//! proves them end to end, and measures how much noise the listing carries so
//! the answer stays usable by a human hunting one window.
//!
//! The window is hidden from OUTSIDE Ghost (PowerShell `ShowWindow`), which is
//! how the real incident happened: no Ghost code path hides a window.
//!
//! `#[ignore]` gated: needs a real Windows session and the testbed binary
//! (`cargo build -p ghost-testbed --release`). It drives its own window on the
//! user desktop and never raises or focuses it:
//!   cargo test -p ghost-session --test hidden_window_recovery -- --ignored --nocapture
#![cfg(windows)]

use ghost_session::GhostSession;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn testbed_exe() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("target");
    ["release", "debug"]
        .iter()
        .map(|p| root.join(p).join("ghost-testbed.exe"))
        .find(|p| p.exists())
}

/// Ask PowerShell whether the window is visible - the check a user would make.
fn is_visible(hwnd: isize) -> bool {
    let script = format!(
        "Add-Type -Namespace T -Name V -MemberDefinition '[DllImport(\"user32.dll\")] public static extern bool IsWindowVisible(IntPtr h);'; \
         [T.V]::IsWindowVisible([IntPtr]{hwnd})"
    );
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Hide it the way the incident did: from outside Ghost.
fn hide_from_outside(hwnd: isize) {
    let script = format!(
        "Add-Type -Namespace T -Name H -MemberDefinition '[DllImport(\"user32.dll\")] public static extern bool ShowWindow(IntPtr h, int c);'; \
         [void][T.H]::ShowWindow([IntPtr]{hwnd}, 0)"
    );
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .status();
}

#[tokio::test]
#[ignore]
async fn a_window_hidden_from_outside_is_listed_and_restorable() {
    let Some(exe) = testbed_exe() else {
        eprintln!("skipped: build the testbed first (cargo build -p ghost-testbed --release)");
        return;
    };
    let title = format!("Ghost Vanish {}", std::process::id());
    let mut child = std::process::Command::new(&exe)
        .args(["--title", &title])
        .spawn()
        .expect("spawn the testbed");
    let pid = child.id();
    let session = GhostSession::new().expect("session");

    // Find it while it is still an ordinary visible window.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut hwnd = 0isize;
    while Instant::now() < deadline && hwnd == 0 {
        if let Ok(ws) = session.list_windows().await {
            if let Some(w) = ws.iter().find(|w| w.name.contains(&title)) {
                hwnd = w.hwnd;
            }
        }
        if hwnd == 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    assert!(hwnd != 0, "the testbed window never appeared in list_windows");
    assert!(is_visible(hwnd), "precondition: the window starts visible");

    // Something outside Ghost hides it. This is the reported failure.
    hide_from_outside(hwnd);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!is_visible(hwnd), "precondition: the window is now hidden");

    // It has vanished from the ordinary listing - the symptom.
    let visible_now = session.list_windows().await.expect("list_windows");
    assert!(
        !visible_now.iter().any(|w| w.hwnd == hwnd),
        "a hidden window must not appear in the ordinary window list"
    );

    // ...but the hidden listing finds it, and says so with state=hidden.
    let hidden = session.list_hidden_windows().await.expect("list_hidden_windows");
    let row = hidden.iter().find(|w| w.hwnd == hwnd);
    assert!(
        row.is_some(),
        "the hidden window must be listed: {} hidden rows, none of them ours",
        hidden.len()
    );
    let row = row.unwrap();
    assert_eq!(row.state, "hidden", "row: {row:?}");
    assert_eq!(row.pid, pid, "the row must carry the owning pid");
    assert!(row.name.contains(&title), "the row must carry the title: {row:?}");

    // The listing has to stay usable for a human hunting one window: the whole
    // point is reading the answer, not scrolling it. This is a guard rail, not
    // a hard property of Windows - if it trips, tighten the filter rather than
    // the number.
    // The listing has to stay readable for a person hunting one window. The
    // count is machine-dependent, so this is a wide guard rail against the
    // filter regressing, not a tuned number: before the structural filter
    // (caption + system menu + minimise box, no tool windows, no sub-dialog
    // sizes) this desktop returned 118 rows of popup hosts and message sinks,
    // and 44 after.
    eprintln!("HIDDEN_ROW_COUNT={}", hidden.len());
    assert!(
        hidden.len() <= 100,
        "the hidden listing returned {} rows; the structural filter has regressed",
        hidden.len()
    );

    // And restore brings it back, without activating it.
    session.window_state(&title, "restore").await.expect("restore the hidden window");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(is_visible(hwnd), "restore must make the window visible again");
    let back = session.list_windows().await.expect("list_windows");
    assert!(
        back.iter().any(|w| w.hwnd == hwnd),
        "a restored window must be in the ordinary window list again"
    );

    let _ = ghost_core::process::kill(pid);
    // Reap it: a killed child left unwaited is a zombie handle for as long as
    // the test binary lives.
    let _ = child.wait();
}
