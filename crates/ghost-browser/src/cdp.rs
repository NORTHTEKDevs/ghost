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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

/// Default per-call ceiling. CDP calls that touch page script (`Runtime.evaluate` on a
/// busy page) can be slow, but a call that has not returned in 30s is wedged.
pub const DEFAULT_CALL_TIMEOUT_MS: u64 = 30_000;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<std::result::Result<Value, String>>>>>;

/// Largest CDP message accepted. tungstenite's defaults (16 MiB frame, 64 MiB
/// message) are below a full-page screenshot of a long page, and an oversized
/// frame is a read error that used to kill the whole connection.
const MAX_MESSAGE_BYTES: usize = 256 * 1024 * 1024;

/// Ceiling on re-reading the ws URL and on opening the socket.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// One WebSocket and its reader/writer tasks.
pub(crate) struct Conn {
    outbound: mpsc::UnboundedSender<Message>,
    pending: Pending,
    /// Set once the reader task observes the socket closing, so in-flight and
    /// future calls fail fast instead of waiting out the full timeout.
    closed: Arc<AtomicBool>,
}

/// A live CDP connection. Cheap to clone; all clones share one socket.
///
/// When the browser's DevTools port is known (`connect_reconnecting`), a socket
/// that died is replaced on the next call: the ws URL is re-read from the port
/// and the connection reopened. Tab sessions belong to a socket, so a reconnect
/// bumps `generation` and each `Tab` re-attaches itself when it sees that.
#[derive(Clone)]
pub struct Cdp {
    conn: Arc<Mutex<Arc<Conn>>>,
    next_id: Arc<AtomicU64>,
    generation: Arc<AtomicU64>,
    port: Option<u16>,
    reconnecting: Arc<tokio::sync::Mutex<()>>,
}

impl Cdp {
    /// Connect to a DevTools WebSocket endpoint (`ws://127.0.0.1:PORT/devtools/browser/...`).
    /// Without a port there is nothing to reconnect to: a dead socket stays dead.
    pub async fn connect(ws_url: &str) -> Result<Self> {
        Self::with_port(ws_url, None).await
    }

    /// Connect, and reconnect through `port`'s `/json/version` if the socket dies.
    pub async fn connect_reconnecting(ws_url: &str, port: u16) -> Result<Self> {
        Self::with_port(ws_url, Some(port)).await
    }

    async fn with_port(ws_url: &str, port: Option<u16>) -> Result<Self> {
        let conn = open(ws_url).await?;
        Ok(Self {
            conn: Arc::new(Mutex::new(Arc::new(conn))),
            next_id: Arc::new(AtomicU64::new(1)),
            generation: Arc::new(AtomicU64::new(0)),
            port,
            reconnecting: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    fn current(&self) -> Arc<Conn> {
        self.conn.lock().unwrap().clone()
    }

    /// True while the current socket is dead (a reconnecting `Cdp` revives it
    /// on its next call).
    pub fn is_closed(&self) -> bool {
        self.current().closed.load(Ordering::SeqCst)
    }

    /// How many times the socket has been replaced. Tab session ids are only
    /// valid on the socket that issued them.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// The current socket, reconnecting first when it has died and a port is
    /// known. Returns the socket and its generation.
    pub(crate) async fn live(&self) -> Result<(Arc<Conn>, u64)> {
        let conn = self.current();
        if !conn.closed.load(Ordering::SeqCst) {
            return Ok((conn, self.generation()));
        }
        let Some(port) = self.port else { return Err(BrowserError::Closed) };
        let _one = self.reconnecting.lock().await;
        // Another caller may have reconnected while this one waited.
        let conn = self.current();
        if !conn.closed.load(Ordering::SeqCst) {
            return Ok((conn, self.generation()));
        }
        let ws_url = tokio::time::timeout(CONNECT_TIMEOUT, crate::launch::browser_ws_url(port))
            .await
            .map_err(|_| BrowserError::Transport(format!("reconnect: /json/version on port {port} timed out")))??;
        let fresh = Arc::new(open(&ws_url).await?);
        *self.conn.lock().unwrap() = fresh.clone();
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        tracing::info!(port, generation, "cdp: socket reconnected");
        Ok((fresh, generation))
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
    /// the reply safe rather than a race. Never reconnects (it cannot await).
    pub fn notify(&self, method: &str, params: Value, session: Option<&str>) -> Result<()> {
        let conn = self.current();
        if conn.closed.load(Ordering::SeqCst) {
            return Err(BrowserError::Closed);
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        // No entry in `pending`, so the eventual reply is simply dropped by the
        // reader rather than accumulating a waiter that nobody polls.
        conn.outbound
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
        let (conn, _) = self.live().await?;
        self.call_on(&conn, method, params, session, timeout_ms).await
    }

    /// One command on a given socket (a Tab re-attaching uses the socket it
    /// re-attached on).
    pub(crate) async fn call_on(
        &self,
        conn: &Conn,
        method: &str,
        params: Value,
        session: Option<&str>,
        timeout_ms: u64,
    ) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }

        let (tx, rx) = oneshot::channel();
        conn.pending.lock().unwrap().insert(id, tx);
        // The reader drains `pending` once when the socket dies. A waiter
        // inserted after that drain would never be answered, so look again now
        // that it is in the map.
        if conn.closed.load(Ordering::SeqCst) {
            conn.pending.lock().unwrap().remove(&id);
            return Err(BrowserError::Closed);
        }

        if conn.outbound.send(Message::Text(msg.to_string())).is_err() {
            conn.pending.lock().unwrap().remove(&id);
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
                conn.pending.lock().unwrap().remove(&id);
                Err(BrowserError::CdpTimeout {
                    method: method.to_string(),
                    ms: timeout_ms,
                })
            }
        }
    }
}

/// Open one socket and start its writer and reader tasks.
async fn open(ws_url: &str) -> Result<Conn> {
    let config = WebSocketConfig {
        max_message_size: Some(MAX_MESSAGE_BYTES),
        max_frame_size: Some(MAX_MESSAGE_BYTES),
        ..Default::default()
    };
    let (stream, _) = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio_tungstenite::connect_async_with_config(ws_url, Some(config), false),
    )
    .await
    .map_err(|_| BrowserError::Transport(format!("connect {ws_url}: timed out")))?
    .map_err(|e| BrowserError::Transport(format!("connect {ws_url}: {e}")))?;
    let (mut write, mut read) = stream.split();

    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let closed = Arc::new(AtomicBool::new(false));

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
        // one by one. `closed` first, so a caller that inserts after the drain
        // sees it (call_on re-checks after inserting).
        closed_r.store(true, Ordering::SeqCst);
        let mut map = pending_r.lock().unwrap();
        for (_, tx) in map.drain() {
            let _ = tx.send(Err("connection closed".into()));
        }
    });

    Ok(Conn { outbound: tx, pending, closed })
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

    /// A loopback CDP stand-in: answers every command with a `data` string of
    /// `reply_len` bytes. With `drop_first`, the first connection is closed
    /// right after the handshake (a socket error mid-session).
    async fn fake_cdp(reply_len: usize, drop_first: bool) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut first = true;
            while let Ok((tcp, _)) = listener.accept().await {
                let drop_it = drop_first && first;
                first = false;
                tokio::spawn(async move {
                    let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                    if drop_it {
                        return; // dropping the stream resets the socket
                    }
                    while let Some(Ok(Message::Text(t))) = ws.next().await {
                        let v: Value = serde_json::from_str(&t).unwrap();
                        let reply = json!({ "id": v["id"], "result": { "data": "a".repeat(reply_len) } });
                        if ws.send(Message::Text(reply.to_string())).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        format!("ws://127.0.0.1:{port}/devtools/browser/test")
    }

    /// A loopback `/json/version` endpoint naming `ws_url`.
    async fn fake_json_version(ws_url: String) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                let body = json!({ "webSocketDebuggerUrl": ws_url }).to_string();
                let mut buf = [0u8; 2048];
                let _ = tcp.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tcp.write_all(resp.as_bytes()).await;
            }
        });
        port
    }

    /// A full-page screenshot of a long page is a reply far over tungstenite's
    /// 16 MiB default frame cap; it must arrive, not kill the socket.
    #[tokio::test]
    async fn a_reply_over_16_mib_is_received() {
        let ws = fake_cdp(17 * 1024 * 1024, false).await;
        let cdp = Cdp::connect(&ws).await.unwrap();
        let r = cdp.call("Page.captureScreenshot", json!({}), None).await.expect("large reply");
        assert_eq!(r["data"].as_str().unwrap().len(), 17 * 1024 * 1024);
        assert!(!cdp.is_closed());
    }

    /// One socket error must not fail every later call until relaunch: the
    /// next call re-reads the ws URL from the port and reconnects.
    #[tokio::test]
    async fn a_dropped_socket_reconnects_on_the_next_call() {
        let ws = fake_cdp(4, true).await;
        let port = fake_json_version(ws.clone()).await;
        let cdp = Cdp::connect_reconnecting(&ws, port).await.unwrap();
        for _ in 0..200 {
            if cdp.is_closed() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(cdp.is_closed(), "the fake dropped the first socket");
        let gen = cdp.generation();
        let r = cdp.call("Browser.getVersion", json!({}), None).await.expect("reconnected");
        assert_eq!(r["data"], "aaaa");
        assert_eq!(cdp.generation(), gen + 1);
    }

    /// A socket opened without a port to re-read has nothing to reconnect to.
    #[tokio::test]
    async fn a_dropped_socket_without_a_port_stays_closed() {
        let ws = fake_cdp(4, true).await;
        let cdp = Cdp::connect(&ws).await.unwrap();
        for _ in 0..200 {
            if cdp.is_closed() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(matches!(cdp.call("X", json!({}), None).await, Err(BrowserError::Closed)));
    }

    #[test]
    fn the_browser_ws_url_must_be_loopback() {
        use crate::launch::check_ws_url;
        for ok in [
            "ws://127.0.0.1:9222/devtools/browser/x",
            "ws://localhost:9222/devtools/browser/x",
            "ws://[::1]:9222/devtools/browser/x",
        ] {
            assert!(check_ws_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "ws://evil.example:9222/devtools/browser/x",
            "ws://10.0.0.5:9222/devtools/browser/x",
            "ws://127.0.0.1.nip.io:9222/devtools/browser/x",
            "wss://example.com/devtools/browser/x",
            "not a url",
        ] {
            assert!(check_ws_url(bad).is_err(), "{bad}");
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
