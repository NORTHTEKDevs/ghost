//! `index` must select the nth match on the background routes, not just the
//! foreground one. Found 2026-09-04: `ghost_act name=Close index=2` under the
//! default policy invoked match 0 - a window's title-bar Close - and closed an
//! eight-tab browser window. The testbed has two buttons named "Increment";
//! the second records `[alt=N]` in the title instead of `[clicks=N]`, so the
//! title says which one was pressed.
//!
//! `#[ignore]` gated: needs a real Windows session and the testbed binary
//! (`cargo build -p ghost-testbed --release`). Runs on Ghost's hidden desktop,
//! so it never touches the screen:
//!   cargo test -p ghost-session --test index_disambiguation -- --ignored --nocapture
#![cfg(windows)]

use ghost_session::{By, GhostSession};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn testbed_exe() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("target");
    ["release", "debug"]
        .iter()
        .map(|p| root.join(p).join("ghost-testbed.exe"))
        .find(|p| p.exists())
}

async fn title_contains(session: &GhostSession, window: &str, needle: &str, ms: u64) -> Option<String> {
    let deadline = Instant::now() + Duration::from_millis(ms);
    let mut last = String::new();
    while Instant::now() < deadline {
        if let Ok(t) = session.resolve_target(Some(window)).await {
            last = t.title.clone();
            if last.contains(needle) {
                return Some(last);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    eprintln!("last title seen: {last:?}");
    None
}

#[tokio::test]
#[ignore]
async fn act_index_selects_the_nth_match_on_the_hidden_desktop_route() {
    let Some(exe) = testbed_exe() else {
        eprintln!("skipped: build the testbed first (cargo build -p ghost-testbed --release)");
        return;
    };
    let session = GhostSession::new().expect("session");
    let title = format!("Ghost Testbed index {}", std::process::id());
    let launched = session
        .launch_hidden(&format!("\"{}\" --title \"{title}\"", exe.display()))
        .await
        .expect("launch the testbed on the hidden desktop");
    let pid = launched["window"]["pid"].as_u64().unwrap_or(0) as u32;
    let target = session.resolve_target(Some(&title)).await.expect("resolve the testbed window");
    assert!(target.is_hidden(), "the testbed must be on the hidden desktop: {target:?}");

    // index=1 is the SECOND "Increment": the title must say [alt=1], never [clicks=.
    let out = session
        .hidden_act(&target, By::name("Increment"), "click", None, Some(1), None)
        .await
        .expect("act on index 1");
    assert_eq!(out["ok"], true, "{out}");
    let seen = title_contains(&session, &title, "[alt=1]", 3_000).await;
    assert!(seen.is_some(), "index=1 must press the second Increment (title never showed [alt=1])");

    // index=0 is the first one again: the counter is shared, so [clicks=2].
    session
        .hidden_act(&target, By::name("Increment"), "click", None, Some(0), None)
        .await
        .expect("act on index 0");
    assert!(
        title_contains(&session, &title, "[clicks=2]", 3_000).await.is_some(),
        "index=0 must press the first Increment"
    );

    // Out of range is an error that says how many there are, not a silent match 0.
    let err = session
        .hidden_act(&target, By::name("Increment"), "click", None, Some(5), None)
        .await
        .expect_err("index 5 of 2 must fail");
    let msg = err.to_string();
    assert!(msg.contains("index 5") && msg.contains("2"), "error must name the index and the count: {msg}");
    assert!(
        title_contains(&session, &title, "[clicks=2]", 500).await.is_some(),
        "an out-of-range index must not press anything"
    );

    if pid != 0 {
        let _ = ghost_core::process::kill(pid);
    }
}

/// The route that caused the damage. `ghost_act` with a `window` anchor under
/// the default `background` policy goes to `act_background`, which read the
/// first name match and ignored `index` entirely - that is what invoked a
/// browser's title-bar Close instead of the dialog's. The testbed runs on the
/// real desktop here because that is where the background route applies; it is
/// never raised or focused by the act itself.
#[tokio::test]
#[ignore]
async fn act_background_index_selects_the_nth_match() {
    let Some(exe) = testbed_exe() else {
        eprintln!("skipped: build the testbed first (cargo build -p ghost-testbed --release)");
        return;
    };
    let title = format!("Ghost Testbed bg {}", std::process::id());
    let mut child = std::process::Command::new(&exe)
        .args(["--title", &title])
        .spawn()
        .expect("spawn the testbed on the user desktop");
    let session = GhostSession::new().expect("session");

    // Wait for the window to exist before driving it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && session.resolve_target(Some(&title)).await.is_err() {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // index=1 is the SECOND "Increment" - the title must say [alt=1].
    let out = session
        .act_background(&title, By::name("Increment"), "click", None, Some(1), None, None)
        .await
        .expect("background act on index 1");
    assert_eq!(out["index"], 1, "the response must echo the index it acted on: {out}");
    assert!(
        title_contains(&session, &title, "[alt=1]", 3_000).await.is_some(),
        "background index=1 must press the second Increment, not match 0"
    );

    // index=0 presses the first one; the counter is shared, so [clicks=2].
    session
        .act_background(&title, By::name("Increment"), "click", None, Some(0), None, None)
        .await
        .expect("background act on index 0");
    assert!(
        title_contains(&session, &title, "[clicks=2]", 3_000).await.is_some(),
        "background index=0 must press the first Increment"
    );

    // Out of range must name the index and the count, and press nothing.
    let err = session
        .act_background(&title, By::name("Increment"), "click", None, Some(5), None, None)
        .await
        .expect_err("index 5 of 2 must fail");
    let msg = err.to_string();
    assert!(msg.contains("index 5"), "error must name the index: {msg}");
    assert!(
        title_contains(&session, &title, "[clicks=2]", 500).await.is_some(),
        "an out-of-range index must not press anything"
    );

    let _ = child.kill();
    let _ = child.wait();
}
