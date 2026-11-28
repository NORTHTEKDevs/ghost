//! `ghost_shell op=run` on a command that prints far more than the reply
//! keeps: the reply is the tail, and its truncation note counts every byte
//! that was dropped, including the bytes dropped while reading.
//!
//! Starts only `cmd /C`, on no window; touches no desktop, input or clipboard.

#![cfg(windows)]

use ghost_session::GhostSession;
use serde_json::json;

#[tokio::test]
async fn a_huge_output_is_capped_to_its_tail_with_an_exact_drop_count() {
    if !ghost_session::shell::shell_enabled() {
        eprintln!("GHOST_SHELL is off; skipping");
        return;
    }
    let s = GhostSession::new().expect("session");
    // 20000 lines of 70 characters + CRLF = 1_440_000 bytes, then END.
    let line = "x".repeat(70);
    let cmd = format!("(for /L %i in (1,1,20000) do @echo {line})& echo END");
    let r = s
        .shell(&json!({"op": "run", "shell": "cmd", "cmd": cmd, "timeout_ms": 120000}))
        .await
        .expect("run");
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["truncated"], true, "{r}");
    let out = r["output"].as_str().unwrap();
    let total = 20000 * 72 + "END\r\n".len();
    let note = format!("...[{} bytes truncated]...\n", total - 24_000);
    assert!(out.starts_with(&note), "{}", &out[..80.min(out.len())]);
    assert!(out.trim_end().ends_with("END"), "{}", &out[out.len().saturating_sub(80)..]);
    assert_eq!(out.len(), note.len() + 24_000);
}
