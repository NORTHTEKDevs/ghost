//! Chrome DevTools Protocol transport.
//!
//! One WebSocket connection to the browser endpoint drives every tab. CDP calls it
//! "flat" session mode: `Target.attachToTarget {flatten: true}` returns a `sessionId`,
//! and every later message carries that id to address one specific tab. That is why a
//! single ghost process can drive twenty tabs concurrently without twenty sockets -
//! and why two ghost processes on two browsers never see each other at all.
//!
//! Everything here is inherently background: CDP input events are injected into a
//! renderer's own event pipeline. No OS cursor moves, no window is raised, and a tab
//! does not have to be the active tab to receive them.

use crate::error::{BrowserError, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

/// Default per-call ceiling. CDP calls that touch page script (`Runtime.evaluate` on a
/// busy page) can be slow, but a call that has not returned in 30s is wedged.
pub const DEFAULT_CALL_TIMEOUT_MS: u64 = 30_000;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<std::result::Result<Value, String>>>>>;

/// A live CDP connection. Cheap to clone; all clones share one socket.
#[derive(Clone)]
pub struct Cdp {
    outbound: mpsc::UnboundedSender<Message>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
    /// Set once the reader task observes the socket closing, so in-flight and future
    /// calls fail fast with `Closed` instead of waiting out the full timeout.
    closed: Arc<std::sync::atomic::AtomicBool>,
}

impl Cdp {
    /// Connect to a DevTools WebSocket endpoint (`ws://127.0.0.1:PORT/devtools/browser/...`).
    pub async fn connect(ws_url: &str) -> Result<Self> {
        let (stream, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .map_err(|e| BrowserError::Transport(format!("connect {ws_url}: {e}")))?;
        let (mut write, mut read) = stream.split();

        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Writer task: serializes all outbound frames onto the single socket.
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if write.send(msg).await.is_err() {
                    break;
                }
            }
        });

        // Reader task: routes replies to their waiting caller by id, drops events.
        let pending_r = pending.clone();
        let closed_r = closed.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = read.next().await {
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                    Message::Close(_) => break,
                    _ => continue,
                };
                let Ok(v) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                let Some(id) = v.get("id").and_then(|i| i.as_u64()) else {
                    // An event, not a reply. Ghost polls rather than subscribes, so
                    // events are intentionally not buffered - buffering every DOM
                    // mutation on a busy page is an unbounded memory leak.
                    continue;
                };
                let waiter = pending_r.lock().unwrap().remove(&id);
                if let Some(tx) = waiter {
                    let outcome = match v.get("error") {
                        Some(e) => Err(e
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown CDP error")
                            .to_string()),
                        None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    let _ = tx.send(outcome);
                }
            }
            // Socket is gone: fail every waiter rather than leaving them to time out
            // one by one.
            closed_r.store(true, Ordering::SeqCst);
            let mut map = pending_r.lock().unwrap();
            for (_, tx) in map.drain() {
                let _ = tx.send(Err("connection closed".into()));
            }
        });

        Ok(Self {
            outbound: tx,
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
            closed,
        })
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Issue a CDP command. `session` addresses a tab; `None` addresses the browser.
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value> {
        self.call_with_timeout(method, params, session, DEFAULT_CALL_TIMEOUT_MS)
            .await
    }

    /// Send a command without waiting for its reply.
    ///
    /// For commands whose acknowledgement is worthless but slow. CDP processes the
    /// commands of one session in the order they arrive, so a later awaited call is
    /// still ordered after this one - the ordering guarantee is what makes skipping
    /// the reply safe rather than a race.
    pub fn notify(&self, method: &str, params: Value, session: Option<&str>) -> Result<()> {
        if self.is_closed() {
            return Err(BrowserError::Closed);
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        // No entry in `pending`, so the eventual reply is simply dropped by the
        // reader rather than accumulating a waiter that nobody polls.
        self.outbound
            .send(Message::Text(msg.to_string()))
            .map_err(|_| BrowserError::Closed)
    }

    pub async fn call_with_timeout(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
        timeout_ms: u64,
    ) -> Result<Value> {
        if self.is_closed() {
            return Err(BrowserError::Closed);
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }

        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);

        if self.outbound.send(Message::Text(msg.to_string())).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(BrowserError::Closed);
        }

        match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(message))) => Err(BrowserError::Cdp {
                method: method.to_string(),
                message,
            }),
            Ok(Err(_recv_closed)) => Err(BrowserError::Closed),
            Err(_elapsed) => {
                // Drop the slot so a late reply cannot resolve a caller that has
                // already given up, and so the map does not grow without bound.
                self.pending.lock().unwrap().remove(&id);
                Err(BrowserError::CdpTimeout {
                    method: method.to_string(),
                    ms: timeout_ms,
                })
            }
        }
    }
}

/// Read a required string field out of a CDP result.
pub fn field_str(v: &Value, key: &str, method: &str) -> Result<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| BrowserError::Protocol {
            method: method.to_string(),
            detail: format!("missing string field '{key}'"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_to_a_dead_endpoint_is_a_transport_error() {
        // Port 1 is never a DevTools endpoint; this must fail fast and not hang.
        let r = Cdp::connect("ws://127.0.0.1:1/devtools/browser/none").await;
        assert!(matches!(r, Err(BrowserError::Transport(_))));
    }

    #[test]
    fn field_str_reports_the_missing_key() {
        let err = field_str(&json!({"a": 1}), "webSocketDebuggerUrl", "Target.attach");
        match err {
            Err(BrowserError::Protocol { detail, .. }) => {
                assert!(detail.contains("webSocketDebuggerUrl"), "{detail}");
            }
            other => panic!("expected Protocol error, got {other:?}"),
        }
    }

    #[test]
    fn field_str_extracts_present_values() {
        assert_eq!(
            field_str(&json!({"sessionId": "ABC"}), "sessionId", "m").unwrap(),
            "ABC"
        );
    }
}
