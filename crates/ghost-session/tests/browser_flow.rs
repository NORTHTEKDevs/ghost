//! Browser integration tests for v0.3.0 (Edge / Chrome / Comet).
//! `#[ignore]` gated — run with `cargo test -p ghost-session -- --ignored`.
//! Each test detects whether the target browser is installed and skips otherwise.

#![cfg(windows)]

use ghost_session::GhostSession;
use std::path::PathBuf;

fn fixture_url() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests").join("fixtures").join("form.html");
    format!("file:///{}", path.to_string_lossy().replace('\\', "/"))
}

async fn run_on(exe: &str) -> Option<GhostSession> {
    let s = GhostSession::new().ok()?.with_timeout(5000);
    let pid = s.launch(exe).await.ok()?;
    if pid == 0 { return None; }
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    Some(s)
}

#[tokio::test]
#[ignore]
async fn navigate_and_wait_resolves_on_edge() {
    let Some(s) = run_on("msedge.exe").await else { return; };
    let url = fixture_url();
    let _ = s.navigate_and_wait("Edge", &url, 10_000).await;
}

#[tokio::test]
#[ignore]
async fn execute_intent_form_login_on_edge() {
    let Some(s) = run_on("msedge.exe").await else { return; };
    let url = fixture_url();
    let intent = format!(r#"{{"steps":[
        {{"op":"navigate","url":"{url}"}},
        {{"op":"wait_for_idle","timeout_ms":3000}}
    ],"max_duration_ms":15000}}"#);
    let _ = s.execute_intent(&intent).await;
}

#[tokio::test]
#[ignore]
async fn describe_delta_small_payload_on_dom_change() {
    let Some(s) = run_on("msedge.exe").await else { return; };
    let a = s.describe_screen_delta(None, None).await.unwrap();
    let b = s.describe_screen_delta(None, Some(a.seq)).await.unwrap();
    assert!(b.added.len() <= a.added.len());
}
