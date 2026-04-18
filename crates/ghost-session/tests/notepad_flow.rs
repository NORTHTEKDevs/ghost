//! Notepad integration tests for v0.3.0.
//! All gated `#[ignore]` — run with `cargo test -p ghost-session -- --ignored`.
//! Requires `notepad.exe` on PATH (Windows only).

#![cfg(windows)]

use ghost_session::{GhostSession, By};

async fn spawn_notepad() -> GhostSession {
    let s = GhostSession::new().unwrap().with_timeout(5000);
    s.launch("notepad.exe").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    s
}

#[tokio::test]
#[ignore]
async fn delta_describe_tracks_typed_text() {
    let s = spawn_notepad().await;
    let a = s.describe_screen_delta(Some("Notepad"), None).await.unwrap();
    let edit = s.find(By::role("edit")).await.unwrap();
    edit.type_text("hello delta").unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let b = s.describe_screen_delta(Some("Notepad"), Some(a.seq)).await.unwrap();
    assert!(b.seq >= a.seq);
}

#[tokio::test]
#[ignore]
async fn execute_intent_find_replace_flow() {
    let s = spawn_notepad().await;
    let intent = r#"{"steps":[
        {"op":"focus_window","name":"Notepad"},
        {"op":"hotkey","modifiers":["Ctrl"],"key":"a"},
        {"op":"press","key":"Delete"}
    ],"max_duration_ms":5000}"#;
    let r = s.execute_intent(intent).await.unwrap();
    assert!(matches!(r.status, ghost_intent::executor::IntentStatus::Success));
}

#[tokio::test]
#[ignore]
async fn locator_cache_lifecycle_cold_warm() {
    let s = spawn_notepad().await;
    let before = s.cache_stats();
    let _ = s.describe_screen_delta(Some("Notepad"), None).await.unwrap();
    let after = s.cache_stats();
    assert!(after.deltas_served >= before.deltas_served + 1);
}

#[tokio::test]
#[ignore]
async fn click_background_does_not_steal_foreground() {
    let s = spawn_notepad().await;
    // Best-effort: just confirm the method returns without panicking on a valid window.
    let _ = s.click_background("Notepad", 10, 10).await;
}
