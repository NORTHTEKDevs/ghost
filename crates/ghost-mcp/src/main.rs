//! Ghost MCP server: JSON-RPC over stdio, with concurrent request execution.
//!
//! Tool dispatch lives in the library so the `ghost` CLI shares it verbatim.
//!
//! Requests are NOT handled one at a time. The original loop awaited each `handle`
//! before reading the next line, so a single slow call - a 15s `ghost_tab_wait_for`,
//! a browser launch - blocked every other request behind it, including instant ones
//! like `ghost_desktop_state`. That serialization was the "ghost stalls between
//! tasks" experience: Claude issues parallel tool calls, and they queued.
//!
//! Now each request runs as its own tokio task against a shared session, and
//! responses are written whenever they finish. Out-of-order completion is legal:
//! JSON-RPC clients correlate responses by `id`, not by arrival order. A dedicated
//! writer task owns stdout so two responses can never interleave bytes.

use ghost_mcp::{handle, GhostSession};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::sync::Arc;

#[derive(Deserialize)]
struct McpRequest {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Serialize)]
struct McpResponse {
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

/// Compile-time proof that the session may be shared across request tasks. If a
/// future change reintroduces a !Send field, this fails to build instead of the
/// server quietly losing its concurrency.
fn _assert_session_shareable() {
    fn check<T: Send + Sync>() {}
    check::<GhostSession>();
}

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    // Worker threads make COM calls (UIA runs directly on request tasks), so every
    // one of them joins the multithreaded apartment explicitly rather than relying
    // on implicit-MTA behavior.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .on_thread_start(|| {
            let _ = ghost_mcp::init_com_for_thread();
        })
        .build()
        .expect("build tokio runtime");

    runtime.block_on(async_main());
}

async fn async_main() {
    let session = match GhostSession::new() {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("Fatal: failed to init GhostSession: {}", e);
            std::process::exit(1);
        }
    };

    // Single writer task: responses arrive from any request task, bytes never mix.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let writer = tokio::task::spawn_blocking(move || {
        let stdout = std::io::stdout();
        let mut out = std::io::BufWriter::new(stdout.lock());
        while let Some(bytes) = out_rx.blocking_recv() {
            let _ = out.write_all(&bytes);
            let _ = out.write_all(b"\n");
            // Flush per message: the client is waiting on this response to decide
            // its next call, so buffering across messages only adds latency.
            let _ = out.flush();
        }
    });

    // Reader on a plain thread: blocking stdin reads must not occupy the runtime.
    let (in_tx, mut in_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    if in_tx.send(l).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!("stdin read error: {}", e);
                    break;
                }
            }
        }
        // Dropping in_tx closes the channel and lets async_main wind down.
    });

    while let Some(line) = in_rx.recv().await {
        if line.trim().is_empty() {
            continue;
        }
        let req: McpRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let msg = serde_json::to_vec(&json!({
                    "id": null,
                    "error": { "message": format!("parse error: {}", e) }
                }))
                .unwrap_or_else(|_| b"{}".to_vec());
                let _ = out_tx.send(msg);
                continue;
            }
        };

        let session = session.clone();
        let out_tx = out_tx.clone();
        // Every request gets its own task. A slow wait in one cannot delay another;
        // three tabs and a desktop app can all be mid-operation at once.
        tokio::spawn(async move {
            let result = handle(&session, &req.method, req.params.as_ref()).await;
            // Notifications have no id and get no response.
            let Some(id) = req.id else { return };
            let resp = match result {
                Ok(v) => McpResponse { id, result: Some(v), error: None },
                Err(e) => McpResponse { id, result: None, error: Some(json!({ "message": e })) },
            };
            let _ = out_tx.send(encode_response(&resp));
        });
    }

    // stdin closed: drop our sender and let in-flight responses drain.
    drop(out_tx);
    let _ = writer.await;
}

/// Encode an MCP response. Uses sonic-rs for large payloads (~3-5x faster on
/// 75KB responses like describe_screen), falls back to serde_json on encode error.
fn encode_response<T: Serialize>(value: &T) -> Vec<u8> {
    match sonic_rs::to_vec(value) {
        Ok(v) => v,
        Err(_) => serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_response_ok_omits_error_field() {
        let resp = McpResponse { id: json!(1), result: Some(json!({"ok": true})), error: None };
        let s = serde_json::to_string(&resp).unwrap();
        assert!(!s.contains("error"));
    }

    #[test]
    fn mcp_response_err_omits_result_field() {
        let resp = McpResponse { id: json!(1), result: None, error: Some(json!({"message": "fail"})) };
        let s = serde_json::to_string(&resp).unwrap();
        assert!(!s.contains("result"));
    }

    #[test]
    fn encode_response_produces_parseable_json() {
        // sonic-rs is used for speed on large payloads with a serde_json fallback;
        // both must yield something the client can actually parse.
        let resp = McpResponse { id: json!(7), result: Some(json!({"a": [1, 2, 3]})), error: None };
        let bytes = encode_response(&resp);
        let back: Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert_eq!(back["id"], 7);
        assert_eq!(back["result"]["a"][2], 3);
    }
}
