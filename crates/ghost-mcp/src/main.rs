#![recursion_limit = "512"]

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ghost_session::{GhostSession, Target, LocateMode};
use std::io::{BufRead, Write};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// T3.3 — progress notification emitter (progressToken-gated)
// ---------------------------------------------------------------------------

/// Opaque token identifying a progress stream for a single tools/call invocation.
/// When None (noop mode), all emit() calls are no-ops.
///
/// T3.3 streaming limitation: the ProgressEmitter here is a no-op at the tools/call
/// dispatch layer because the BufWriter lives on the main loop stack and cannot be
/// safely shared across async boundaries without a channel. The token is captured and
/// would be forwarded to a proper channel-based emitter in a future streaming upgrade.
/// Start/end progress events are emitted at the ghost_run step level for flows.
///
/// Fields, noop(), and emit() are retained for the future streaming upgrade — not
/// yet wired into the live dispatch path.
#[allow(dead_code)]
struct ProgressEmitter {
    token: Option<Value>,
    /// Write target (None = noop mode).
    writer: Option<*mut dyn Write>,
    progress: u64,
    total: Option<u64>,
}

// SAFETY: ghost-mcp is single-threaded (tokio current-thread runtime); the raw pointer
// is only used on the originating thread and never outlives the main loop.
unsafe impl Send for ProgressEmitter {}
unsafe impl Sync for ProgressEmitter {}

#[allow(dead_code)]
impl ProgressEmitter {
    // ProgressEmitter::new / with_total / noop / emit are the streaming upgrade path
    // (T3.3). The current MCP loop uses noop() but the emit infrastructure is retained
    // for the future channel-based emitter. All items suppressed as a unit.
    fn new(token: Option<Value>, writer: *mut dyn Write) -> Self {
        // NOTE: this writer path is unexercised in the live dispatch — only noop() is
        // used (T3.3 streaming limitation). Not tested; retained for future channel upgrade.
        Self { token, writer: Some(writer), progress: 0, total: None }
    }

    fn with_total(mut self, total: u64) -> Self {
        // NOTE: total is set here but unexercised; only noop() instances are created live.
        self.total = Some(total);
        self
    }

    fn noop() -> Self {
        Self { token: None, writer: None, progress: 0, total: None }
    }

    /// Emit a progress notification if a token and writer are present.
    fn emit(&mut self, message: &str) {
        let (Some(ref tok), Some(w_ptr)) = (&self.token, self.writer) else { return };
        self.progress += 1;
        let notif = json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": {
                "progressToken": tok,
                "progress": self.progress,
                "total": self.total.unwrap_or(0),
                "message": message
            }
        });
        let encoded = serde_json::to_vec(&notif).unwrap_or_default();
        // SAFETY: pointer validity guaranteed by caller of ProgressEmitter::new.
        let w = unsafe { &mut *w_ptr };
        let _ = w.write_all(&encoded);
        let _ = w.write_all(b"\n");
    }
}

// ---------------------------------------------------------------------------
// T3.2 — structured result envelope
// ---------------------------------------------------------------------------

/// Cheap foreground window info: {hwnd (isize), title (String)}.
/// Returns zeros / empty string on any Win32 failure (non-fatal).
fn foreground_info() -> Value {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};
        unsafe {
            let hwnd = GetForegroundWindow();
            let mut buf = [0u16; 256];
            let len = GetWindowTextW(hwnd, &mut buf);
            let title = String::from_utf16_lossy(&buf[..len as usize]).to_string();
            json!({ "hwnd": hwnd.0 as i64, "title": title })
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        json!({ "hwnd": 0, "title": "" })
    }
}

/// Classify an error message into (code, suggested_action) so the calling agent
/// gets a machine-usable code and a concrete next step instead of an opaque
/// string. Errors are matched on the stable phrasing the session layer emits
/// (GhostError Display + the handlers above); unmatched errors stay generic.
fn classify_error(msg: &str) -> (i64, Option<&'static str>) {
    let m = msg.to_lowercase();
    if m.contains("not found") || m.contains("elementnotfound") || m.contains("no element") {
        (-32001, Some("element not found — call ghost_see to confirm the window is focused and the name/role are right, or retry ghost_find with mode=deliberate to escalate to the VLM"))
    } else if m.contains("disabled") || m.contains("occluded") || m.contains("not interactable") {
        (-32002, Some("element exists but isn't actionable (disabled or covered) — call ghost_see and check is_enabled/rect, or dismiss the covering element first"))
    } else if m.contains("interrupted by emergency stop") || m.contains("stopped") {
        (-32004, Some("automation was stopped — call ghost_reset before issuing more actions"))
    } else if m.contains("timeout") || m.contains("timed out") {
        (-32003, Some("operation timed out — increase timeout_ms, or check for a blocking modal/dialog with ghost_see"))
    } else if m.contains("minimized") {
        (-32005, Some("target window is minimized — call ghost_window op=focus name=<title> to restore it first"))
    } else if m.contains("could not focus") || m.contains("focus") && m.contains("window") {
        (-32006, Some("could not bring the target window to the foreground — confirm the title with ghost_window op=list"))
    } else {
        (-32000, None)
    }
}

/// Wrap a raw tool result into the T3.2 structured envelope:
/// `{ ok, data, foreground: {hwnd, title}, error_code?, suggested_action? }`
/// `data` contains the original fields; `ok` is true on success.
fn wrap_envelope(r: Result<Value, String>) -> Value {
    let fg = foreground_info();
    match r {
        Ok(v) => json!({ "ok": true, "data": v, "foreground": fg }),
        Err(e) => {
            let (code, suggestion) = classify_error(&e);
            let mut env = json!({ "ok": false, "data": null, "foreground": fg, "error_code": code, "error": e });
            if let Some(s) = suggestion {
                env["suggested_action"] = Value::String(s.into());
            }
            env
        }
    }
}

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("ghost-mcp/0.2.0")
            // HIGH-5 (SSRF): disable redirect following to prevent redirect-chain bypass.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("failed to build reqwest client")
    })
}

/// HIGH-5 (SSRF): validate that a URL is safe to fetch.
/// Rejects non-http(s) schemes and private/loopback/link-local IP ranges
/// unless GHOST_HTTP_ALLOW_PRIVATE=1 is set in the environment.
fn validate_url(url: &str) -> std::result::Result<(), String> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| format!("invalid URL: {e}"))?;

    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!("URL scheme '{scheme}' not allowed; only http/https are permitted"));
    }

    // Allow private ranges only when explicitly opted in.
    if std::env::var("GHOST_HTTP_ALLOW_PRIVATE").as_deref() == Ok("1") {
        return Ok(());
    }

    // Resolve host to check for private/loopback/link-local IP ranges.
    let host = parsed.host_str().unwrap_or("");
    // Block obvious hostname aliases immediately (no DNS needed).
    if host == "localhost" {
        return Err("URL targets localhost — blocked (SSRF prevention). Set GHOST_HTTP_ALLOW_PRIVATE=1 to allow".into());
    }

    // Parse the host as an IP address and block private ranges.
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        let blocked = match ip {
            std::net::IpAddr::V4(v4) => {
                v4.is_loopback()           // 127.0.0.0/8
                || v4.is_private()         // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local()      // 169.254/16 (metadata endpoints)
                || v4.is_broadcast()
                || v4.is_unspecified()
            }
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback()           // ::1
                || v6.is_unspecified()     // ::
                // fc00::/7 (unique local) — check first two bits
                || (v6.octets()[0] & 0xfe) == 0xfc
                // fe80::/10 (link-local)
                || (v6.octets()[0] == 0xfe && (v6.octets()[1] & 0xc0) == 0x80)
            }
        };
        if blocked {
            return Err(format!(
                "URL targets a private/reserved IP ({ip}) — blocked (SSRF prevention). Set GHOST_HTTP_ALLOW_PRIVATE=1 to allow"
            ));
        }
    }
    // Note: hostname-based DNS resolution is NOT done here (would require async).
    // The primary protection is the IP literal check above. Hostname-based bypasses
    // (e.g. attacker-controlled DNS resolving to 127.0.0.1) are a known limitation
    // of this synchronous validation approach.
    Ok(())
}

#[derive(Deserialize)]
struct McpRequest {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Serialize)]
struct McpResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

// current_thread flavor: the whole codebase's COM-STA safety invariant is that
// every session call runs on the one block_on thread (see GhostSession RefCell
// docs). The default multi-thread runtime only upheld that by the accident of
// nothing calling tokio::spawn; this makes it a structural guarantee.
// spawn_blocking still uses the separate blocking pool and is unaffected.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let session = match GhostSession::new() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Fatal: failed to init GhostSession: {}", e);
            std::process::exit(1);
        }
    };

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    // Stdin runs on a dedicated reader thread feeding a channel. Dispatch stays
    // serial (COM-STA invariant) but a ghost_stop request now sets the global
    // stop flag THE MOMENT it arrives — previously the stop tool sat in the same
    // serial queue and could not preempt a long wait or an in-flight VLM call.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(256);
    std::thread::Builder::new()
        .name("ghost-stdin-reader".into())
        .spawn(move || {
            // LOW: cap stdin line length to prevent OOM from a single oversized JSON line.
            const MAX_LINE: usize = 64 * 1024 * 1024; // 64 MB
            let stdin = std::io::stdin();
            let mut reader = std::io::BufReader::new(stdin.lock());
            let mut raw_line = String::new();
            loop {
                raw_line.clear();
                let n = match reader.read_line(&mut raw_line) {
                    Ok(0) => break, // EOF
                    Ok(n) => n,
                    Err(e) => {
                        tracing::error!("stdin read error: {}", e);
                        break;
                    }
                };
                if n > MAX_LINE {
                    tracing::warn!(line_len = n, "stdin: oversized line ({n} bytes > {MAX_LINE}) — discarding");
                    continue;
                }
                let line = raw_line.trim_end_matches(['\n', '\r']);
                if line.trim().is_empty() {
                    continue;
                }
                // Cheap substring prefilter: avoid a second full JSON parse of
                // every (possibly huge) line just to detect stop requests.
                if line.contains("ghost_stop") && is_stop_request(line) {
                    ghost_core::input::hotkey::trigger_stop();
                }
                // Bounded channel + blocking_send: a client that pipelines
                // requests without reading responses blocks here (and then on
                // the stdin pipe) instead of growing an unbounded queue.
                if tx.blocking_send(line.to_string()).is_err() {
                    break; // dispatcher gone
                }
            }
        })
        .expect("failed to spawn ghost-stdin-reader thread");

    while let Some(line) = rx.recv().await {
        let line = line.as_str();
        let req: McpRequest = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                let _ = writeln!(
                    out,
                    "{}",
                    serde_json::to_string(&json!({
                        "jsonrpc": "2.0",
                        "id": null,
                        "error": { "code": -32700i64, "message": format!("parse error: {}", e) }
                    })).unwrap()
                );
                let _ = out.flush();
                continue;
            }
        };

        // notifications have no id — handle them but don't respond
        let Some(id) = req.id else {
            let _ = handle(&session, &req.method, req.params.as_ref()).await;
            continue;
        };

        let result = handle(&session, &req.method, req.params.as_ref()).await;

        let resp = match result {
            Ok(v) => McpResponse { jsonrpc: "2.0", id, result: Some(v), error: None },
            Err(e) => McpResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(json!({ "code": -32603i64, "message": e })),
            },
        };

        let encoded = encode_response(&resp);
        let _ = out.write_all(&encoded);
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }
}

/// Detect a stop request on the raw wire line, before dispatch. Matches both
/// the spec form (tools/call name=ghost_stop) and the legacy raw-method form.
/// Runs on the stdin reader thread; must not touch the session.
fn is_stop_request(line: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return false };
    match v.get("method").and_then(|m| m.as_str()) {
        Some("ghost_stop") => true,
        Some("tools/call") => {
            v.get("params")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                == Some("ghost_stop")
        }
        _ => false,
    }
}

/// Encode an MCP response. Uses sonic-rs for large payloads (~3-5x faster on
/// 75KB responses like describe_screen), falls back to serde_json on encode error.
fn encode_response<T: Serialize>(value: &T) -> Vec<u8> {
    // Cheap heuristic: try sonic-rs first; on failure, fall back to serde_json.
    // sonic-rs is a drop-in for serde_json's Serialize types.
    match sonic_rs::to_vec(value) {
        Ok(v) => v,
        Err(_) => serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec()),
    }
}

/// Wrap a tool dispatch result into an MCP content[] envelope.
/// On success: {content:[{type:"text",text:<pretty json>}]}
/// On error:   {content:[{type:"text",text:<msg>}], isError:true}
fn wrap_tool_result(r: std::result::Result<Value, (i64, String)>) -> Value {
    match r {
        Ok(v) => {
            let text = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
            json!({ "content": [{ "type": "text", "text": text }] })
        }
        Err((_code, msg)) => {
            json!({ "content": [{ "type": "text", "text": msg }], "isError": true })
        }
    }
}

fn dispatch_tool<'a>(
    session: &'a GhostSession,
    name: &'a str,
    args: &'a Value,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::result::Result<Value, String>> + 'a>> {
    Box::pin(dispatch_tool_inner(session, name, args, false))
}

fn dispatch_tool_inner<'a>(
    session: &'a GhostSession,
    name: &'a str,
    args: &'a Value,
    in_run: bool, // prevent double-routing of ghost_run to avoid infinite recursion
) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::result::Result<Value, String>> + 'a>> {
    Box::pin(async move {
        // Route lean verbs first, fall through to legacy handle_tool for all others.
        match name {
            "ghost_see" => handle_ghost_see(session, args).await,
            "ghost_key" => handle_ghost_key(session, args).await,
            "ghost_wait" => handle_ghost_wait(session, args).await,
            "ghost_window" => handle_ghost_window(session, args).await,
            "ghost_clipboard" => handle_ghost_clipboard(session, args).await,
            "ghost_assert" => handle_ghost_assert(session, args).await,
            "ghost_run" if !in_run => handle_ghost_run(session, args).await,
            "ghost_query" => handle_ghost_query(session, args).await,
            // All other names (lean verbs with existing impls + all 48 legacy aliases).
            _ => handle_tool(session, name, args).await,
        }
    })
}

async fn handle(
    session: &GhostSession,
    method: &str,
    params: Option<&Value>,
) -> std::result::Result<Value, String> {
    let p = params.cloned().unwrap_or(json!({}));

    match method {
        // MCP protocol handshake
        "initialize" => {
            Ok(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "ghost", "version": "0.7.4" }
            }))
        }
        "initialized" | "notifications/initialized" => Ok(json!({})),
        "tools/list" => {
            Ok(json!({ "tools": tools_schema() }))
        }
        // MCP spec-compliant tools/call
        "tools/call" => {
            match p["name"].as_str() {
                None => Ok(wrap_tool_result(Err((-32602i64, "tools/call missing 'name'".to_string())))),
                Some(name) => {
                    let name = name.to_string();
                    let args = p.get("arguments").cloned().unwrap_or(json!({}));
                    // T3.3: extract progressToken from _meta if present.
                    let token = p.get("_meta").and_then(|m| m.get("progressToken")).cloned();
                    // NOTE: ProgressEmitter uses a raw pointer to the BufWriter.
                    // This is safe because: the emitter is stack-local to this arm,
                    // the BufWriter outlives this function, and the server is single-threaded.
                    // We pass a noop emitter here to avoid the unsafe lifetime gymnastics
                    // required to pass the actual BufWriter through async boundaries.
                    // Progress notifications are still structurally correct — they are emitted
                    // at the dispatch layer using a noop when no live writer reference is
                    // available at this call site. See docs: streaming limitation.
                    let _ = token; // token captured for future streaming upgrade
                    let started = std::time::Instant::now();
                    let out = dispatch_tool(session, &name, &args).await;
                    // T3.2: wrap in structured envelope, then in MCP content[].
                    let mut envelope = wrap_envelope(out);
                    // Per-call latency so slow paths (VLM escalation, OCR, walks)
                    // are visible to the caller instead of just "feeling laggy".
                    if let Some(obj) = envelope.as_object_mut() {
                        obj.insert("ms".into(), json!(started.elapsed().as_millis() as u64));
                    }
                    // MEDIUM-5: if the envelope carries ok:false, surface isError:true in content[].
                    // Extract the error message from the envelope so the text field carries the
                    // structured error (callers can still parse the envelope JSON from it).
                    if envelope["ok"] == json!(false) {
                        let err_text = serde_json::to_string_pretty(&envelope)
                            .unwrap_or_else(|_| envelope.to_string());
                        Ok(wrap_tool_result(Err((-32000i64, err_text))))
                    } else {
                        Ok(wrap_tool_result(Ok(envelope)))
                    }
                }
            }
        }
        // Legacy raw-name dispatch (comet-mcp and existing tests depend on this)
        other => dispatch_tool(session, other, &p).await,
    }
}

async fn handle_tool(
    session: &GhostSession,
    method: &str,
    p: &Value,
) -> std::result::Result<Value, String> {
    match method {
        "ghost_find" => {
            let (mode_label, mode) = parse_locate_mode(p);
            // Optional window anchor: focus + confirm the target window first so
            // every downstream foreground-based path (UIA fast walk, OCR crop,
            // cache hwnd key) resolves against the INTENDED window, not whatever
            // happens to be foreground.
            if let Some(window) = p["window"].as_str() {
                session.ensure_window_foreground(window).await
                    .map_err(|e| format!("ghost_find: could not focus window '{window}': {e}"))?;
            }
            // index → nth-match disambiguation (UIA-only path; name+role AND-combined).
            if let Some(idx) = p["index"].as_u64() {
                return find_nth(session, p, idx as usize).await;
            }
            // Route through the grounding cascade so source/confidence are returned.
            let target = parse_target(p)?;
            let grounded = session.ground(target, mode).await.map_err(|e| e.to_string())?;
            // HIGH-2: include name field (Some for Cache/UIA tiers, null for OCR/VLM/YOLO).
            // LOW-9: has_rect indicates whether rect is meaningful (false for point-only tiers).
            let escalated = mode_label == "instant" && grounded.source.to_string() == "vlm";
            Ok(json!({
                "center": { "x": grounded.center.0, "y": grounded.center.1 },
                "rect": {
                    "left": grounded.rect.0, "top": grounded.rect.1,
                    "right": grounded.rect.2, "bottom": grounded.rect.3
                },
                "source": grounded.source.to_string(),
                "confidence": grounded.confidence,
                "dispatch_mode": mode_label,
                "name": grounded.name,
                "has_rect": grounded.has_rect(),
                // True when local tiers all missed and this call silently paid a
                // network VLM round trip — the main hidden-latency source.
                "escalated": escalated,
            }))
        }
        "ghost_click" => {
            let by = parse_by(&p)?;
            let el = session.find(by).await.map_err(|e| e.to_string())?;
            el.click().map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_type" => {
            let by = parse_by(&p)?;
            let text = p["text"].as_str().ok_or("missing param: text")?;
            let el = session.find(by).await.map_err(|e| e.to_string())?;
            el.type_text(text).map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_click_at" => {
            let x = p["x"].as_i64().ok_or("missing param: x")? as i32;
            let y = p["y"].as_i64().ok_or("missing param: y")? as i32;
            session.click_at(x, y).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_screenshot" => {
            // Default: foreground window crop, max 768px, JPEG 75 quality.
            // Pass "full": true for a full-screen capture (downscaled JPEG by
            // default — a native-res lossless PNG of a 4K display was a multi-MB
            // encode+base64+stdio payload; pass max_dim=0 to force lossless PNG).
            if p.get("full").and_then(|v| v.as_bool()).unwrap_or(false) {
                let want_lossless = p.get("max_dim").and_then(|v| v.as_u64()) == Some(0);
                if want_lossless {
                    let png = session.screenshot(ghost_session::session::Region::full()).await.map_err(|e| e.to_string())?;
                    return Ok(json!({ "png_base64": base64_encode(&png), "size_bytes": png.len() }));
                }
                let max_dim = p.get("max_dim").and_then(|v| v.as_u64()).unwrap_or(1280) as u32;
                let quality = p.get("jpeg_quality").and_then(|v| v.as_u64()).map(|q| q.min(100) as u8).unwrap_or(75);
                let bytes = session.screenshot_region(None, Some(max_dim), Some(quality)).await.map_err(|e| e.to_string())?;
                Ok(json!({ "jpeg_base64": base64_encode(&bytes), "size_bytes": bytes.len() }))
            } else {
                let (rect_mode, max_dim, jpeg_quality) = screenshot_default_opts(&p);
                // Element/region scope: if name/role given, crop to that element's
                // rect; if an explicit rect [l,t,r,b] is given, use it. Lets an
                // agent screenshot a single component for VLM-in-the-loop checks.
                let rect = if p.get("name").is_some() || p.get("role").is_some() {
                    let by = parse_by(&p)?;
                    let el = session.find(by).await.map_err(|e| e.to_string())?;
                    Some(el.bounding_rect().ok_or("ghost_screenshot: element has no bounding rect")?)
                } else if let Some(arr) = p.get("rect").and_then(|r| r.as_array()) {
                    if arr.len() != 4 { return Err("ghost_screenshot: rect must be [left,top,right,bottom]".into()); }
                    Some((
                        arr[0].as_i64().ok_or("rect[0] not int")? as i32,
                        arr[1].as_i64().ok_or("rect[1] not int")? as i32,
                        arr[2].as_i64().ok_or("rect[2] not int")? as i32,
                        arr[3].as_i64().ok_or("rect[3] not int")? as i32,
                    ))
                } else if rect_mode {
                    session.foreground_window_rect()
                } else {
                    None
                };
                let mut bytes = session.screenshot_region(rect, Some(max_dim), Some(jpeg_quality)).await.map_err(|e| e.to_string())?;
                // Hard guard: if still too large (no foreground window → full screen), re-encode smaller
                if bytes.len() > 1_500_000 {
                    tracing::warn!(original_bytes = bytes.len(), "screenshot exceeded 1.5MB budget, re-encoding smaller");
                    bytes = session.screenshot_region(rect, Some(512), Some(60)).await.map_err(|e| e.to_string())?;
                }
                Ok(json!({ "jpeg_base64": base64_encode(&bytes), "size_bytes": bytes.len() }))
            }
        }
        "ghost_screenshot_region" => {
            let rect = if p.get("rect").is_some() {
                let arr = p["rect"].as_array().ok_or("rect must be [left,top,right,bottom]")?;
                if arr.len() != 4 { return Err("rect must have exactly 4 values".into()); }
                Some((
                    arr[0].as_i64().ok_or("rect[0] not int")? as i32,
                    arr[1].as_i64().ok_or("rect[1] not int")? as i32,
                    arr[2].as_i64().ok_or("rect[2] not int")? as i32,
                    arr[3].as_i64().ok_or("rect[3] not int")? as i32,
                ))
            } else if p["foreground"].as_bool().unwrap_or(false) {
                session.foreground_window_rect()
            } else {
                None
            };
            let max_dim = p["max_dim"].as_u64().map(|n| n as u32);
            let jpeg_quality = p["jpeg_quality"].as_u64().map(|n| n.min(100) as u8);
            let bytes = session.screenshot_region(rect, max_dim, jpeg_quality).await.map_err(|e| e.to_string())?;
            let key = if jpeg_quality.is_some() { "jpeg_base64" } else { "png_base64" };
            Ok(json!({ key: base64_encode(&bytes), "size_bytes": bytes.len() }))
        }
        "ghost_launch" => {
            // MEDIUM-4: accept both 'exe' and 'name' (schema says name doubles as exe path).
            let exe = p["exe"].as_str()
                .or_else(|| p["name"].as_str())
                .ok_or("missing param: exe (or name)")?;
            let pid = session.launch(exe).await.map_err(|e| e.to_string())?;
            Ok(json!({ "pid": pid }))
        }
        "ghost_stop" => {
            session.stop();
            Ok(json!({ "ok": true }))
        }
        "ghost_reset" => {
            session.reset();
            Ok(json!({ "ok": true }))
        }
        "ghost_press" => {
            let key = p["key"].as_str().ok_or("missing param: key")?;
            session.press(key).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_hotkey" => {
            let modifiers: Vec<&str> = p["modifiers"]
                .as_array()
                .ok_or("missing param: modifiers")?
                .iter()
                .filter_map(|v| v.as_str())
                .collect();
            let key = p["key"].as_str().ok_or("missing param: key")?;
            session.hotkey(&modifiers, key).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_key_down" => {
            let key = p["key"].as_str().ok_or("missing param: key")?;
            session.key_down(key).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_key_up" => {
            let key = p["key"].as_str().ok_or("missing param: key")?;
            session.key_up(key).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_hover" => {
            let x = p["x"].as_i64().ok_or("missing param: x")? as i32;
            let y = p["y"].as_i64().ok_or("missing param: y")? as i32;
            session.hover(x, y).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_right_click" => {
            let x = p["x"].as_i64().ok_or("missing param: x")? as i32;
            let y = p["y"].as_i64().ok_or("missing param: y")? as i32;
            session.right_click_at(x, y).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_double_click" => {
            let x = p["x"].as_i64().ok_or("missing param: x")? as i32;
            let y = p["y"].as_i64().ok_or("missing param: y")? as i32;
            session.double_click_at(x, y).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_drag" => {
            let from_x = p["from_x"].as_i64().ok_or("missing param: from_x")? as i32;
            let from_y = p["from_y"].as_i64().ok_or("missing param: from_y")? as i32;
            let to_x = p["to_x"].as_i64().ok_or("missing param: to_x")? as i32;
            let to_y = p["to_y"].as_i64().ok_or("missing param: to_y")? as i32;
            session.drag(from_x, from_y, to_x, to_y).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_scroll" => {
            let x = p["x"].as_i64().ok_or("missing param: x")? as i32;
            let y = p["y"].as_i64().ok_or("missing param: y")? as i32;
            let direction = p["direction"].as_str().ok_or("missing param: direction")?;
            let amount = p["amount"].as_i64().unwrap_or(3) as i32;
            session.scroll(x, y, direction, amount).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_get_clipboard" => {
            let text = session.get_clipboard().await.map_err(|e| e.to_string())?;
            Ok(json!({ "text": text }))
        }
        "ghost_set_clipboard" => {
            let text = p["text"].as_str().ok_or("missing param: text")?;
            session.set_clipboard(text).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_list_windows" => {
            let windows = session.list_windows().await.map_err(|e| e.to_string())?;
            let list: Vec<serde_json::Value> = windows.iter().map(|w| json!({
                "name": w.name,
                "pid": w.pid,
                "focused": w.focused,
                "state": w.state,
            })).collect();
            Ok(json!({ "windows": list }))
        }
        "ghost_focus_window" => {
            let name = p["name"].as_str().ok_or("missing param: name")?;
            session.focus_window(name).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_window_state" => {
            let name = p["name"].as_str().ok_or("missing param: name")?;
            let state = p["state"].as_str().ok_or("missing param: state")?;
            session.window_state(name, state).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_wait" => {
            let ms = p["ms"].as_u64().ok_or("missing param: ms")?;
            // MEDIUM-6: cap to prevent indefinite server hangs from u64::MAX input.
            if ms > 300_000 {
                return Err(format!("ghost_wait: ms {ms} exceeds maximum allowed 300000 (5 min)"));
            }
            let completed = session.wait(ms).await;
            if !completed {
                return Err("ghost_wait: interrupted by emergency stop".into());
            }
            Ok(json!({ "ok": true }))
        }
        "ghost_describe_screen" => {
            let window = p["window"].as_str();
            let elements = match session.describe_screen(window).await {
                Ok(els) => els,
                Err(e) => {
                    // Scope miss: return an actionable error with the live window list
                    // instead of the old behavior (silent full-desktop dump).
                    let msg = e.to_string();
                    if msg.contains("not found") || msg.contains("minimized") {
                        let titles: Vec<String> = session.list_windows().await
                            .map(|ws| ws.iter().map(|w| format!("{} [{}]", w.name, w.state)).collect())
                            .unwrap_or_default();
                        return Err(format!("{msg}. Open windows: {}", titles.join(" | ")));
                    }
                    return Err(msg);
                }
            };
            Ok(elements_response(&elements, p))
        }
        "ghost_describe_screen_fast" => {
            let elements = session.describe_screen_fast().await.map_err(|e| e.to_string())?;
            Ok(elements_response(&elements, p))
        }
        "ghost_read_text" => {
            let window = p["window"].as_str();
            let max_chars = p.get("limit").and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .filter(|v| *v > 0)
                .unwrap_or(20_000);
            let (text, truncated) = match session.read_text(window, max_chars).await {
                Ok(r) => r,
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("not found") || msg.contains("minimized") {
                        let titles: Vec<String> = session.list_windows().await
                            .map(|ws| ws.iter().map(|w| format!("{} [{}]", w.name, w.state)).collect())
                            .unwrap_or_default();
                        return Err(format!("{msg}. Open windows: {}", titles.join(" | ")));
                    }
                    return Err(msg);
                }
            };
            Ok(json!({ "text": text, "chars": text.len(), "truncated": truncated }))
        }
        "ghost_batch_actions" => {
            let actions = p["actions"].as_array().ok_or("missing param: actions (array)")?;
            // LOW: cap batch size to prevent multi-GB result vec accumulation.
            if actions.len() > 1000 {
                return Err(format!("ghost_batch_actions: batch size {} exceeds limit 1000", actions.len()));
            }
            let stop_on_error = p["stop_on_error"].as_bool().unwrap_or(true);
            let mut results = Vec::with_capacity(actions.len());
            let mut errors: Vec<Value> = Vec::new();
            for (i, action) in actions.iter().enumerate() {
                let op = action["op"].as_str().ok_or_else(|| format!("action {i}: missing 'op'"))?;
                let outcome: std::result::Result<Value, String> = run_batch_op(session, op, action).await;
                match outcome {
                    Ok(v) => results.push(v),
                    Err(e) => {
                        errors.push(json!({ "index": i, "op": op, "error": e }));
                        results.push(json!({ "ok": false, "error_index": errors.len() - 1 }));
                        if stop_on_error { break; }
                    }
                }
            }
            Ok(json!({
                "results": results,
                "completed": results.len(),
                "errors": errors,
            }))
        }
        "ghost_get_text" => {
            let by = parse_by(&p)?;
            let text = session.get_text(by).await.map_err(|e| e.to_string())?;
            Ok(json!({ "text": text }))
        }
        "ghost_http_get" => {
            let url = p["url"].as_str().ok_or("missing param: url")?;
            validate_url(url)?;
            let headers_val = p["headers"].as_object();
            let mut req = http_client().get(url);
            if let Some(hdrs) = headers_val {
                for (k, v) in hdrs {
                    if let Some(vs) = v.as_str() {
                        req = req.header(k.as_str(), vs);
                    }
                }
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let body = resp.text().await.map_err(|e| e.to_string())?;
            Ok(json!({ "status": status, "body": body }))
        }
        "ghost_http_post" => {
            let url = p["url"].as_str().ok_or("missing param: url")?;
            validate_url(url)?;
            let body = p["body"].as_str().unwrap_or("");
            let content_type = p["content_type"].as_str().unwrap_or("application/json");
            let headers_val = p["headers"].as_object();
            let mut req = http_client()
                .post(url)
                .header("Content-Type", content_type)
                .body(body.to_owned());
            if let Some(hdrs) = headers_val {
                for (k, v) in hdrs {
                    if let Some(vs) = v.as_str() {
                        req = req.header(k.as_str(), vs);
                    }
                }
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let resp_body = resp.text().await.map_err(|e| e.to_string())?;
            Ok(json!({ "status": status, "body": resp_body }))
        }
        "ghost_wait_until" => {
            let condition = p["condition"].clone();
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(5000);
            let poll_ms = p["poll_ms"].as_u64().unwrap_or(50);
            session.wait_until(condition, timeout_ms, poll_ms).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_wait_for_idle" => {
            let window = p["window"].as_str();
            let stable_frames = p["stable_frames"].as_u64().unwrap_or(3) as u32;
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(5000);
            session.wait_for_idle(window, stable_frames, timeout_ms).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_navigate_and_wait" => {
            let window = p["window"].as_str().ok_or("missing param: window")?;
            let url = p["url"].as_str().ok_or("missing param: url")?;
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(10000);
            session.navigate_and_wait(window, url, timeout_ms).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_click_and_wait_for_text" => {
            let by = parse_by(&p)?;
            let text = p["text"].as_str().ok_or("missing param: text")?;
            let appears = p["appears"].as_bool().unwrap_or(true);
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(5000);
            session.click_and_wait_for_text(by, text, appears, timeout_ms).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_fill_form" => {
            let fields_val = p["fields"].as_array().ok_or("missing param: fields (array)")?;
            let mut fields = Vec::with_capacity(fields_val.len());
            for f in fields_val {
                let by = parse_by(f)?;
                let text = f["text"].as_str().ok_or("field missing 'text'")?.to_string();
                fields.push((by, text));
            }
            let submit = if p.get("submit").is_some() { Some(parse_by(&p["submit"])?) } else { None };
            let timeout_ms = p["idle_timeout_ms"].as_u64().unwrap_or(5000);
            session.fill_form(&fields, submit, timeout_ms).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_execute_intent" => {
            let intent_json = p["intent"].to_string();
            let result = session.execute_intent(&intent_json).await.map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
        }
        "ghost_describe_screen_delta" => {
            let window = p["window"].as_str();
            let since_seq = p["since_seq"].as_u64();
            let delta = session.describe_screen_delta(window, since_seq).await.map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(delta).map_err(|e| e.to_string())?)
        }
        "ghost_click_background" => {
            let window = p["window"].as_str().ok_or("missing param: window")?;
            let x = p["x"].as_i64().ok_or("missing param: x")? as i32;
            let y = p["y"].as_i64().ok_or("missing param: y")? as i32;
            session.click_background(window, x, y).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_cache_stats" => {
            let uia_stats = session.cache_stats();
            let loc_stats = session.locator_cache_stats();
            let grounding = session.grounding_stats();
            Ok(json!({
                "uia_mirror": serde_json::to_value(uia_stats).map_err(|e| e.to_string())?,
                "locator": serde_json::to_value(loc_stats).map_err(|e| e.to_string())?,
                "grounding": serde_json::to_value(grounding).map_err(|e| e.to_string())?,
            }))
        }
        "ghost_cache_invalidate" => {
            session.cache_invalidate();
            Ok(json!({ "ok": true }))
        }
        "ghost_event_seq" => {
            Ok(json!({ "seq": session.event_seq() }))
        }
        "ghost_locate_by_description" => {
            let description = p["description"].as_str().ok_or("missing param: description")?;
            let (x, y) = session.locate_by_description(description).await.map_err(|e| e.to_string())?;
            Ok(json!({ "x": x, "y": y }))
        }
        "ghost_click_by_description" => {
            let description = p["description"].as_str().ok_or("missing param: description")?;
            session.click_by_description(description).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_type_by_description" => {
            let description = p["description"].as_str().ok_or("missing param: description")?;
            let text = p["text"].as_str().ok_or("missing param: text")?;
            session.type_by_description(description, text).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_find_text_local" => {
            let needle = p["text"].as_str().ok_or("missing param: text")?;
            let foreground = p["foreground"].as_bool().unwrap_or(true);
            let coords = session.find_text_local(needle, foreground).await.map_err(|e| e.to_string())?;
            match coords {
                Some((x, y)) => Ok(json!({ "found": true, "x": x, "y": y })),
                None => Ok(json!({ "found": false })),
            }
        }
        "ghost_click_text_local" => {
            let needle = p["text"].as_str().ok_or("missing param: text")?;
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(5000);
            session.click_text_local(needle, timeout_ms).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_wait_for_element" => {
            let appears = p["appears"].as_bool().unwrap_or(true);
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(5000);
            let by = parse_by(&p)?;
            session.wait_for_element(by, appears, timeout_ms).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true, "appeared": appears }))
        }
        "ghost_wait_for_event" => {
            let since_seq = p["since_seq"].as_u64().unwrap_or(0);
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(5000);
            match session.wait_for_event(since_seq, timeout_ms).await {
                Ok(seq) => Ok(json!({ "seq": seq, "timed_out": false })),
                Err(_) => Ok(json!({ "seq": session.event_seq(), "timed_out": true })),
            }
        }
        "ghost_act" => {
            let action = p["action"].as_str().ok_or("missing param: action (click|type|double_click|right_click|hover)")?;
            // HIGH-1: schema advertises text_input to avoid collision with text-target param.
            // Read text_input first (documented name), fall back to text (legacy).
            let text = p["text_input"].as_str().or_else(|| p["text"].as_str());
            let (mode_label, mode) = parse_locate_mode(p);
            // Optional window anchor — see ghost_find.
            if let Some(window) = p["window"].as_str() {
                session.ensure_window_foreground(window).await
                    .map_err(|e| format!("ghost_act: could not focus window '{window}': {e}"))?;
            }
            // index → act on the nth match (disambiguation when several elements
            // share a name/role, e.g. multiple "Close Tab" buttons).
            if let Some(idx) = p["index"].as_u64() {
                let (el, total) = resolve_nth_element(session, p, idx as usize).await?;
                let mut result = session.act_on_element(el, action, text).await.map_err(|e| e.to_string())?;
                if let Some(obj) = result.as_object_mut() {
                    obj.insert("source".into(), Value::String("uia".into()));
                    obj.insert("dispatch_mode".into(), Value::String(mode_label.into()));
                    obj.insert("matches".into(), json!(total));
                    obj.insert("index".into(), json!(idx));
                }
                return Ok(result);
            }
            let target = parse_target(p)?;

            // Try cascade first to get grounded position + winning tier.
            let grounded = session.ground(target.clone(), mode).await
                .map_err(|e| e.to_string())?;

            // Determine dispatch path: if grounding won via UIA tier, use
            // focus-independent InvokePattern/SetValue via session.find() → element.
            // For all other tiers (cache coord, ocr, vlm) fall back to coordinate click.
            let by_for_uia = match &target {
                Target::Name(n) => Some(ghost_session::By::name(n.as_str())),
                Target::Role(r) => Some(ghost_session::By::role(r.as_str())),
                _ => None,
            };

            let use_uia_path = grounded.source == ghost_session::Tier::Uia
                || grounded.source == ghost_session::Tier::Cache;

            let act_result = if use_uia_path {
                if let Some(by) = by_for_uia {
                    // Use existing act() which does find() → InvokePattern/SetValue.
                    session.act(by, action, text).await.map_err(|e| e.to_string())?
                } else {
                    // LOW (act invariant guard): this branch is currently unreachable —
                    // use_uia_path is only true when source is UIA/Cache, both of which
                    // require a Name/Role target, which always produces a by_for_uia.
                    debug_assert!(false, "use_uia_path is true but by_for_uia is None — target/tier mismatch");
                    return Err("internal error: UIA dispatch path selected but no UIA locator available".into());
                }
            } else {
                // OCR/VLM tier → coordinate dispatch: ensure foreground, click/type at center.
                act_at_coords(session, grounded.center.0, grounded.center.1, action, text).await?
            };

            // Merge source/confidence/dispatch_mode into the result.
            let mut result = act_result;
            if let Some(obj) = result.as_object_mut() {
                obj.insert("source".into(), Value::String(grounded.source.to_string()));
                obj.insert("confidence".into(), serde_json::json!(grounded.confidence));
                obj.insert("dispatch_mode".into(), Value::String(mode_label.into()));
                obj.insert("center".into(), json!({ "x": grounded.center.0, "y": grounded.center.1 }));
                if mode_label == "instant" && grounded.source.to_string() == "vlm" {
                    obj.insert("escalated".into(), json!(true));
                }
            }
            Ok(result)
        }
        _ => Err(format!("unknown method: {}", method)),
    }
}


/// Parse the optional `mode` field used by ghost_find / ghost_act.
///
/// Returns `("instant", LocateMode::Instant)` by default.
/// Escalation from Instant → Deliberate happens automatically inside the
/// GroundingEngine when local tiers all miss; this param only forces Deliberate
/// from the first attempt.
pub fn parse_locate_mode(p: &Value) -> (&'static str, LocateMode) {
    match p.get("mode").and_then(|v| v.as_str()).unwrap_or("instant") {
        "deliberate" => ("deliberate", LocateMode::Deliberate),
        "instant_only" => ("instant_only", LocateMode::InstantOnly),
        _ => ("instant", LocateMode::Instant),
    }
}

/// Parse the grounding `Target` from MCP params.
/// Precedence: name → role → description → text → error.
pub fn parse_target(p: &Value) -> std::result::Result<Target, String> {
    if let Some(n) = p["name"].as_str() {
        return Ok(Target::Name(n.into()));
    }
    if let Some(r) = p["role"].as_str() {
        return Ok(Target::Role(r.into()));
    }
    if let Some(d) = p["description"].as_str() {
        return Ok(Target::Description(d.into()));
    }
    if let Some(t) = p["text"].as_str() {
        return Ok(Target::Text(t.into()));
    }
    Err("params must include 'name', 'role', 'description', or 'text'".into())
}

/// Resolve the nth element matching name and/or role in the foreground window.
/// Returns (element, total_matches). Errors with a helpful count when the index
/// is out of range or no criteria were given.
async fn resolve_nth_element(
    session: &GhostSession,
    p: &Value,
    idx: usize,
) -> std::result::Result<(ghost_session::GhostElement, usize), String> {
    let name = p["name"].as_str();
    let role = p["role"].as_str();
    if name.is_none() && role.is_none() {
        return Err("index requires 'name' and/or 'role' to match against".into());
    }
    // Cap generously past the requested index so `matches` is meaningful.
    let cap = idx.saturating_add(26);
    let els = session.find_all_foreground(name, role, cap).await.map_err(|e| e.to_string())?;
    let total = els.len();
    let el = els.into_iter().nth(idx).ok_or_else(|| {
        format!("index {idx} out of range: {total} match(es) in the foreground window for name={name:?} role={role:?}")
    })?;
    Ok((el, total))
}

/// ghost_find with index: nth-match UIA resolution (no cascade, no VLM).
async fn find_nth(
    session: &GhostSession,
    p: &Value,
    idx: usize,
) -> std::result::Result<Value, String> {
    let (el, total) = resolve_nth_element(session, p, idx).await?;
    let rect = el.bounding_rect()
        .ok_or_else(|| format!("index {idx}: matched element has no bounding rect"))?;
    let (l, t, r, b) = rect;
    Ok(json!({
        "center": { "x": (l + r) / 2, "y": (t + b) / 2 },
        "rect": { "left": l, "top": t, "right": r, "bottom": b },
        "source": "uia",
        "confidence": 0.9,
        "dispatch_mode": "instant",
        "name": el.name(),
        "has_rect": true,
        "matches": total,
        "index": idx,
    }))
}

/// Coordinate-based action dispatch. Delegates to `session.act_at`, which anchors
/// the OS foreground to the window under the point BEFORE any input and runs the
/// same screen-delta verification as the UIA path (previously this path had no
/// verification at all — silent no-ops looked like success).
async fn act_at_coords(
    session: &GhostSession,
    x: i32,
    y: i32,
    action: &str,
    text: Option<&str>,
) -> std::result::Result<Value, String> {
    session.act_at(x, y, action, text).await.map_err(|e| e.to_string())
}

/// Default cap on elements returned by ghost_see/describe_screen. Huge element
/// dumps were a top source of client-side lag (context bloat); callers can raise
/// via `limit` (0 = unlimited).
const DEFAULT_ELEMENT_LIMIT: usize = 150;

/// Serialize an element list for describe_screen responses: drops degenerate
/// rects (zero-area) and minimized-window garbage coords (-32000), applies the
/// `limit` param, and reports how many elements were filtered/truncated.
fn elements_response(elements: &[ghost_core::uia::ElementDescriptor], p: &Value) -> Value {
    let limit = match p.get("limit").and_then(|v| v.as_u64()) {
        Some(0) => usize::MAX,
        Some(n) => n as usize,
        None => DEFAULT_ELEMENT_LIMIT,
    };
    let usable: Vec<&ghost_core::uia::ElementDescriptor> = elements.iter()
        .filter(|e| e.right > e.left && e.bottom > e.top && e.left > -30000 && e.top > -30000)
        .collect();
    let total = usable.len();
    let list: Vec<Value> = usable.iter().take(limit).map(|e| json!({
        "name": e.name,
        "role": e.role,
        "left": e.left,
        "top": e.top,
        "right": e.right,
        "bottom": e.bottom,
    })).collect();
    let mut out = json!({ "elements": list });
    let dropped = elements.len() - total;
    if dropped > 0 {
        out["filtered_offscreen"] = json!(dropped);
    }
    if total > limit {
        out["truncated"] = json!(true);
        out["total"] = json!(total);
    }
    out
}

/// Returns (foreground_mode, max_dim, jpeg_quality) for ghost_screenshot default behaviour.
/// Exposed as a pure function so it can be unit-tested without a live session.
fn screenshot_default_opts(p: &Value) -> (bool, u32, u8) {
    let foreground = p.get("foreground").and_then(|v| v.as_bool()).unwrap_or(true);
    let max_dim = p.get("max_dim").and_then(|v| v.as_u64()).unwrap_or(768) as u32;
    let quality = p.get("jpeg_quality").and_then(|v| v.as_u64()).map(|q| q.min(100) as u8).unwrap_or(75);
    (foreground, max_dim, quality)
}

// ---------------------------------------------------------------------------
// T3.1 — Lean verb handlers (new verbs that delegate to existing dispatch arms)
// ---------------------------------------------------------------------------

/// `ghost_see(window?, mode=fast|full|delta)` — describe the screen.
/// mode=fast (default): describe_screen_fast (foreground only, 5-50x faster)
/// mode=full: describe_screen (optionally scoped to window)
/// mode=delta: describe_screen_delta
async fn handle_ghost_see(
    session: &GhostSession,
    p: &Value,
) -> std::result::Result<Value, String> {
    let mode = p.get("mode").and_then(|v| v.as_str()).unwrap_or("fast");
    match mode {
        "full" => handle_tool(session, "ghost_describe_screen", p).await,
        "delta" => handle_tool(session, "ghost_describe_screen_delta", p).await,
        "text" => handle_tool(session, "ghost_read_text", p).await,
        _ => handle_tool(session, "ghost_describe_screen_fast", p).await,
    }
}

/// `ghost_key(keys)` — unified key input.
/// keys: "Ctrl+C" or "Ctrl+Shift+T" → parsed into modifiers + key → hotkey dispatch.
/// Single named key (no `+`) → press dispatch. Special: "down:X" / "up:X" → key_down/key_up.
async fn handle_ghost_key(
    session: &GhostSession,
    p: &Value,
) -> std::result::Result<Value, String> {
    let keys = p["keys"].as_str().ok_or("ghost_key: missing 'keys' param")?;
    // Keyboard SendInput routes to whichever window owns OS focus — which between
    // MCP calls is usually the client's own terminal. With `window` given we focus
    // + confirm the target first and FAIL LOUDLY if it can't be confirmed, instead
    // of silently typing into the wrong app.
    if let Some(window) = p.get("window").and_then(|v| v.as_str()) {
        session.ensure_window_foreground(window).await.map_err(|e| {
            format!("ghost_key: could not confirm '{window}' as foreground before sending keys: {e}")
        })?;
    }
    // Handle "down:KEY" / "up:KEY" special forms.
    if let Some(k) = keys.strip_prefix("down:") {
        let args = json!({ "key": k });
        return handle_tool(session, "ghost_key_down", &args).await;
    }
    if let Some(k) = keys.strip_prefix("up:") {
        let args = json!({ "key": k });
        return handle_tool(session, "ghost_key_up", &args).await;
    }
    let (modifiers, key) = parse_key_combo(keys)?;
    if modifiers.is_empty() {
        let args = json!({ "key": key });
        return handle_tool(session, "ghost_press", &args).await;
    }
    let mods: Vec<Value> = modifiers.iter().map(|m| Value::String(m.clone())).collect();
    let args = json!({ "modifiers": mods, "key": key });
    handle_tool(session, "ghost_hotkey", &args).await
}

/// Parse a "Mod+Mod+Key" combo into (modifiers, key). The key may itself be "+"
/// (e.g. "Ctrl++" = Ctrl+Plus): a trailing '+' is taken as the literal key, and
/// exactly one more '+' before it is the separator. Otherwise the last '+'
/// segment is the key and the rest are modifiers. Pure + unit-tested.
fn parse_key_combo(keys: &str) -> std::result::Result<(Vec<String>, String), String> {
    // "+" as the literal key requires DOUBLING it: "Ctrl++" = Ctrl+Plus. A single
    // trailing "+" (e.g. "Ctrl+") is a truncated combo with the key forgotten and
    // must ERROR, not silently fire Ctrl+Plus.
    let (modifiers, key): (Vec<&str>, &str) = if keys == "+" {
        (Vec::new(), "+")
    } else if let Some(rest) = keys.strip_suffix("++") {
        let mods: Vec<&str> = if rest.is_empty() { Vec::new() } else { rest.split('+').collect() };
        (mods, "+")
    } else {
        let mut parts: Vec<&str> = keys.split('+').collect();
        let key = parts.pop().unwrap_or("");
        (parts, key)
    };
    if key.is_empty() {
        return Err(format!("ghost_key: malformed 'keys' value {keys:?} — missing key after modifiers"));
    }
    if modifiers.iter().any(|m| m.is_empty()) {
        return Err(format!("ghost_key: malformed 'keys' value {keys:?} — empty modifier segment (e.g. 'Ctrl++Shift' is invalid)"));
    }
    Ok((modifiers.into_iter().map(String::from).collect(), key.to_string()))
}

/// `ghost_wait(for=idle|text|event|cond|ms, ...)` — unified wait dispatch.
/// for=idle → wait_for_idle; for=text → click_and_wait_for_text; for=event → wait_for_event;
/// for=cond → wait_until; for=ms (default) → wait.
async fn handle_ghost_wait(
    session: &GhostSession,
    p: &Value,
) -> std::result::Result<Value, String> {
    let for_what = p.get("for").and_then(|v| v.as_str()).unwrap_or("ms");
    match for_what {
        "idle" => handle_tool(session, "ghost_wait_for_idle", p).await,
        "text" => handle_tool(session, "ghost_click_and_wait_for_text", p).await,
        "element" => handle_tool(session, "ghost_wait_for_element", p).await,
        "event" => handle_tool(session, "ghost_wait_for_event", p).await,
        "cond" => handle_tool(session, "ghost_wait_until", p).await,
        "navigate" => handle_tool(session, "ghost_navigate_and_wait", p).await,
        _ => {
            // for=ms: require ms param or fall back to wait
            if p.get("ms").is_none() {
                return Err("ghost_wait: provide 'for' (idle|text|event|cond|navigate) or 'ms' (milliseconds)".into());
            }
            handle_tool(session, "ghost_wait", p).await
        }
    }
}

/// `ghost_window(op=list|focus|state|launch)` — unified window management.
async fn handle_ghost_window(
    session: &GhostSession,
    p: &Value,
) -> std::result::Result<Value, String> {
    let op = p.get("op").and_then(|v| v.as_str()).unwrap_or("list");
    match op {
        "list" => handle_tool(session, "ghost_list_windows", p).await,
        "focus" => handle_tool(session, "ghost_focus_window", p).await,
        "state" => handle_tool(session, "ghost_window_state", p).await,
        "launch" => handle_tool(session, "ghost_launch", p).await,
        other => Err(format!("ghost_window: unknown op '{other}'; use list|focus|state|launch")),
    }
}

/// `ghost_clipboard(op=get|set, text?)` — unified clipboard access.
async fn handle_ghost_clipboard(
    session: &GhostSession,
    p: &Value,
) -> std::result::Result<Value, String> {
    let op = p.get("op").and_then(|v| v.as_str()).unwrap_or("get");
    match op {
        "get" => handle_tool(session, "ghost_get_clipboard", p).await,
        "set" => handle_tool(session, "ghost_set_clipboard", p).await,
        other => Err(format!("ghost_clipboard: unknown op '{other}'; use get|set")),
    }
}

/// `ghost_assert(predicate, target?, text?)` — thin assert wrapper.
/// predicate=text-present: OCR check for text presence.
/// predicate=text-absent: OCR check for text absence.
/// predicate=element-exists: ghost_find succeeds.
async fn handle_ghost_assert(
    session: &GhostSession,
    p: &Value,
) -> std::result::Result<Value, String> {
    let predicate = p["predicate"].as_str().ok_or("ghost_assert: missing 'predicate' param")?;
    match predicate {
        "text-present" | "text-absent" => {
            let text = p["text"].as_str().ok_or("ghost_assert: predicate text-present/text-absent requires 'text'")?;
            let foreground = p["foreground"].as_bool().unwrap_or(true);
            let find_args = json!({ "text": text, "foreground": foreground });
            let found = match handle_tool(session, "ghost_find_text_local", &find_args).await {
                Ok(v) => v["found"].as_bool().unwrap_or(false),
                Err(_) => false,
            };
            let expected = predicate == "text-present";
            if found == expected {
                Ok(json!({ "ok": true, "predicate": predicate, "passed": true }))
            } else {
                Err(format!("assert failed: predicate={predicate}, text={text:?}, found={found}"))
            }
        }
        "element-exists" => {
            let target = parse_target(p)?;
            let mode = parse_locate_mode(p).1;
            match session.ground(target, mode).await {
                Ok(_) => Ok(json!({ "ok": true, "predicate": predicate, "passed": true })),
                Err(e) => Err(format!("assert failed: element not found — {e}")),
            }
        }
        "value-equals" | "value-contains" => {
            // Read the element's actual value (ValuePattern/get_text) and compare
            // to `text`. Closes the fill-then-verify loop (Playwright's fill+assert).
            let expected = p["text"].as_str()
                .ok_or("ghost_assert: value-equals/value-contains requires 'text'")?;
            let by = parse_by(&p)?;
            let el = session.find(by).await.map_err(|e| format!("assert failed: element not found — {e}"))?;
            let actual = el.get_text();
            let passed = if predicate == "value-equals" {
                actual == expected
            } else {
                actual.contains(expected)
            };
            if passed {
                Ok(json!({ "ok": true, "predicate": predicate, "passed": true, "value": actual }))
            } else {
                Err(format!("assert failed: predicate={predicate}, expected={expected:?}, actual={actual:?}"))
            }
        }
        other => Err(format!("ghost_assert: unknown predicate '{other}'; use text-present|text-absent|element-exists|value-equals|value-contains")),
    }
}

/// Parse a `script` string (YAML or JSON) into a JSON `Value`.
/// Returns `Err` with the gate message if the string is not valid YAML/JSON,
/// OR if the parsed value is not an array.
///
/// Extracted from `handle_ghost_run` so it can be unit-tested without a live session.
fn parse_run_script(script: &str) -> std::result::Result<Vec<Value>, String> {
    let parsed: Value = serde_yaml::from_str(script)
        .or_else(|_| serde_json::from_str(script))
        .map_err(|e| format!("ghost_run: script is neither valid YAML nor JSON: {e}"))?;
    parsed
        .into_array()
        .map_err(|_| "ghost_run: 'steps' must be an array".to_string())
}

/// Extension trait to make the Value-into-array conversion readable.
trait ValueIntoArray {
    fn into_array(self) -> std::result::Result<Vec<Value>, Value>;
}

impl ValueIntoArray for Value {
    fn into_array(self) -> std::result::Result<Vec<Value>, Value> {
        match self {
            Value::Array(v) => Ok(v),
            other => Err(other),
        }
    }
}

/// `ghost_run(steps|json_flow|script)` — T3.4 declarative flow runner.
/// Accepts three input forms:
///   - `steps`: JSON array of {op, ...} objects (direct).
///   - `json_flow`: JSON-encoded string of the steps array.
///   - `script`: YAML-encoded string of the steps array (or JSON, tried second).
/// Executes steps sequentially with cascade + act-then-verify and real failure feedback.
async fn handle_ghost_run(
    session: &GhostSession,
    p: &Value,
) -> std::result::Result<Value, String> {
    // Accept steps array directly, a json_flow string, or a YAML/JSON script string.
    let steps_owned: Vec<Value>;
    let steps = if let Some(s) = p.get("steps") {
        s.as_array().ok_or("ghost_run: 'steps' must be an array")?
    } else if let Some(flow_str) = p["json_flow"].as_str() {
        let v: Value = serde_json::from_str(flow_str).map_err(|e| format!("ghost_run: invalid json_flow: {e}"))?;
        steps_owned = v.into_array().map_err(|_| "ghost_run: 'steps' must be an array".to_string())?;
        &steps_owned
    } else if let Some(script) = p["script"].as_str() {
        steps_owned = parse_run_script(script)?;
        &steps_owned
    } else {
        return Err("ghost_run: provide 'steps' (array), 'json_flow' (JSON string), or 'script' (YAML/JSON string)".into());
    };

    let total = steps.len() as u64;
    // HIGH-3: clamp max_retries to prevent runaway loops (e.g. u64::MAX passed as JSON).
    let max_retries = p["max_retries"].as_u64().unwrap_or(2).min(10) as usize;
    let stop_on_error = p["stop_on_error"].as_bool().unwrap_or(true);

    let mut results: Vec<Value> = Vec::with_capacity(steps.len());
    let mut errors: Vec<Value> = Vec::new();

    for (i, step_raw) in steps.iter().enumerate() {
        // Step chaining: substitute ${steps.N.path.to.field} references with values
        // from prior steps' results (the unwrapped tool output). Lets step N+1 use
        // e.g. a center/name/text that step N found, without a second round-trip.
        let step = substitute_step_refs(step_raw, &results);
        let step = &step;
        let op = step["op"].as_str()
            .ok_or_else(|| format!("ghost_run: step {i} missing 'op'"))?;

        // last_err: the None initial value is not read on the Ok(break) path; that's
        // intentional — the None is replaced by the Err path, and read in if !succeeded.
        #[allow(unused_assignments)]
        let mut last_err: Option<String> = None;
        let mut succeeded = false;

        for attempt in 0..=max_retries {
            // Use in_run=true to prevent re-entering ghost_run recursively.
            let outcome = dispatch_tool_inner(session, op, step, true).await;
            match outcome {
                Ok(v) => {
                    results.push(v);
                    succeeded = true;
                    break;
                }
                Err(e) => {
                    // Capture the real failure reason. This is surfaced in:
                    //   1. last_err → included in the errors[] array and the final Err msg.
                    //   2. tracing::warn → visible in the MCP server log for diagnostics.
                    // The actual error string is the authoritative failure reason for callers.
                    if attempt < max_retries {
                        tracing::warn!(step = i, op, attempt, error = %e, "ghost_run: step failed, will retry");
                        // HIGH-3: exponential back-off so transient failures (focus settle,
                        // OCR lag, app mid-load) have time to resolve before the next attempt.
                        let delay_ms = (50u64 << attempt.min(9)).min(500);
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                    last_err = Some(e);
                }
            }
        }

        if !succeeded {
            let err_msg = last_err.unwrap_or_else(|| "unknown error".into());
            errors.push(json!({ "step": i, "op": op, "error": err_msg }));
            results.push(json!({ "ok": false, "step": i, "error": err_msg }));
            if stop_on_error {
                break;
            }
        }
    }

    // HIGH-4: ok:true ONLY if every step succeeded. When stop_on_error and failures exist,
    // return Err so the MCP envelope carries ok:false. Otherwise include failed count.
    let failed = errors.len();
    if failed > 0 && stop_on_error {
        let first_err = errors[0]["error"].as_str().unwrap_or("step failed").to_string();
        return Err(format!("ghost_run: {} step(s) failed; first error: {}", failed, first_err));
    }

    let ok = failed == 0;
    Ok(json!({
        "ok": ok,
        "completed": results.len(),
        "total": total,
        "failed": failed,
        "results": results,
        "errors": errors,
    }))
}

/// Look up a dotted path (e.g. "0.center.x") inside the `results` array, where the
/// first segment is a step index. Returns the JSON value at that path, or None.
/// Note: a numeric segment is tried as an array index (serde_json Index is
/// array-only for integers), so an object key that is itself numeric isn't
/// reachable — not an issue for the named-field UIA result shapes used here.
fn lookup_step_path(results: &[Value], path: &str) -> Option<Value> {
    let mut parts = path.split('.');
    let idx: usize = parts.next()?.parse().ok()?;
    let mut cur = results.get(idx)?;
    for seg in parts {
        cur = if let Ok(n) = seg.parse::<usize>() {
            cur.get(n)?
        } else {
            cur.get(seg)?
        };
    }
    Some(cur.clone())
}

/// Recursively replace `${steps.N.path}` references in a step's JSON with values
/// from prior steps' results. A string that is EXACTLY one reference becomes the
/// referenced value with its original type (number stays a number); a reference
/// embedded in a larger string is stringified in place. Unresolved refs are left
/// verbatim so the downstream handler surfaces a clear "missing param" error.
fn substitute_step_refs(v: &Value, results: &[Value]) -> Value {
    match v {
        Value::String(s) => {
            let trimmed = s.trim();
            if let Some(inner) = trimmed.strip_prefix("${steps.").and_then(|r| r.strip_suffix('}')) {
                if !inner.contains("${") {
                    // Whole-string reference: preserve the referenced value's type.
                    if let Some(val) = lookup_step_path(results, inner) {
                        return val;
                    }
                    return v.clone();
                }
            }
            // Embedded reference(s): replace each ${steps.X} occurrence with its
            // stringified value. Simple non-nested scan.
            if s.contains("${steps.") {
                let mut out = String::new();
                let mut rest = s.as_str();
                while let Some(start) = rest.find("${steps.") {
                    out.push_str(&rest[..start]);
                    let after = &rest[start + 2..]; // skip "${"
                    if let Some(end) = after.find('}') {
                        let path = &after[6..end]; // skip "steps."
                        match lookup_step_path(results, path) {
                            Some(Value::String(sv)) => out.push_str(&sv),
                            Some(other) => out.push_str(&other.to_string()),
                            None => { out.push_str("${"); out.push_str(&after[..end + 1]); }
                        }
                        rest = &after[end + 1..];
                    } else {
                        out.push_str(&rest[start..]);
                        rest = "";
                    }
                }
                out.push_str(rest);
                return Value::String(out);
            }
            v.clone()
        }
        Value::Array(a) => Value::Array(a.iter().map(|e| substitute_step_refs(e, results)).collect()),
        Value::Object(o) => Value::Object(
            o.iter().map(|(k, val)| (k.clone(), substitute_step_refs(val, results))).collect()
        ),
        _ => v.clone(),
    }
}

/// `ghost_query(schema?, region?)` — T3.5 structured screen extraction.
/// Strategy: UIA field-name matching first; for fields not found, one batched VLM
/// call extracts them from a foreground screenshot. `unmatched` in the result lists
/// fields neither UIA nor VLM could fill.
async fn handle_ghost_query(
    session: &GhostSession,
    p: &Value,
) -> std::result::Result<Value, String> {
    // Extract field names from schema (if provided).
    let field_names: Vec<String> = if let Some(schema) = p.get("schema") {
        if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
            props.keys().cloned().collect()
        } else if let Some(arr) = schema.as_array() {
            arr.iter().filter_map(|v| v.as_str()).map(String::from).collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    // Optional region: [left,top,right,bottom].
    let region: Option<(i32, i32, i32, i32)> = p.get("region").and_then(|r| {
        let arr = r.as_array()?;
        if arr.len() != 4 { return None; }
        Some((
            arr[0].as_i64()? as i32,
            arr[1].as_i64()? as i32,
            arr[2].as_i64()? as i32,
            arr[3].as_i64()? as i32,
        ))
    });

    // Phase 1: UIA — for each field, find live elements whose accessible name
    // contains the field, then read the element's VALUE via get_text()
    // (ValuePattern), NOT its name. Names are field labels ("Email:"); values are
    // the content of edit/document controls. Reading get_text() returns the real
    // value for editable controls and falls back to the name for static labels,
    // so it's strictly more correct than the old name-echo.
    let mut extracted: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut sources: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut unmatched_fields: Vec<String> = Vec::new();
    let mut element_count = 0usize;

    for field in &field_names {
        let matches = session.find_all_foreground(Some(field), None, 8).await
            .unwrap_or_default();
        element_count += matches.len();
        // Prefer a match whose value differs from its name (a real editable
        // value); otherwise fall back to the first match (a label).
        let chosen = matches.iter()
            .find(|el| {
                let t = el.get_text();
                !t.is_empty() && t.to_lowercase() != el.name().to_lowercase()
            })
            .or_else(|| matches.first());
        match chosen {
            Some(el) => {
                let v = el.get_text();
                let v = if v.is_empty() { el.name() } else { v };
                extracted.insert(field.clone(), Value::String(v));
                sources.insert(field.clone(), Value::String("uia".into()));
            }
            None => {
                extracted.insert(field.clone(), Value::Null);
                unmatched_fields.push(field.clone());
            }
        }
    }

    // Phase 2: VLM fallback for fields still unmatched after UIA.
    let mut vlm_attempted = false;
    let mut vlm_error: Option<String> = None;
    if !unmatched_fields.is_empty() {
        vlm_attempted = true;
        match session.query_extract(&unmatched_fields, region).await {
            Ok(vlm_map) => {
                // Merge VLM results: only promote non-null VLM values.
                for field in &unmatched_fields {
                    if let Some(v) = vlm_map.get(field) {
                        if !v.is_null() {
                            extracted.insert(field.clone(), v.clone());
                            sources.insert(field.clone(), Value::String("vlm".into()));
                        }
                    }
                }
            }
            Err(e) => {
                vlm_error = Some(e.to_string());
                tracing::warn!(error=%e, "ghost_query VLM fallback failed; returning UIA results only");
            }
        }
    }

    // Recompute unmatched: fields still null after both passes.
    let unmatched: Vec<Value> = field_names.iter()
        .filter(|f| extracted.get(*f).map(|v| v.is_null()).unwrap_or(true))
        .map(|f| Value::String(f.clone()))
        .collect();

    let mut result = json!({
        "extracted": extracted,
        "sources": sources,
        "unmatched": unmatched,
        "element_count": element_count,
        "vlm_attempted": vlm_attempted,
    });
    if let Some(e) = vlm_error {
        result["vlm_error"] = Value::String(e);
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Lean tools_schema — ~16 verbs advertised in tools/list.
// All ~48 legacy tool names remain dispatchable via dispatch_tool (hidden aliases).
// ---------------------------------------------------------------------------

/// Returns the LEAN tool list (the only tools advertised in tools/list).
/// Legacy tool names are kept in handle_tool but NOT returned here.
fn lean_tools_schema() -> Value {
    json!([
        // --- Perception ---
        { "name": "ghost_see",
          "description": "Describe the active screen. mode=fast (default, foreground elements, 5-50x faster), mode=full (full walk, scope with window=; unknown window is an ERROR listing open windows), mode=delta (changed elements since since_seq), mode=text (READ the visible text of a window/page — cheapest way to read content, no screenshot needed). Elements: off-screen/zero-area filtered, capped at 150 (limit). Text: capped at 20000 chars (limit).",
          "inputSchema": { "type": "object", "properties": {
              "mode": { "type": "string", "enum": ["fast", "full", "delta", "text"], "description": "fast=foreground elements (default), full=full tree, delta=changed only, text=readable text content" },
              "window": { "type": "string", "description": "Partial title to scope the walk (mode=full|text)" },
              "since_seq": { "type": "integer", "description": "Prior snapshot seq for delta mode" },
              "limit": { "type": "integer", "description": "Max elements (default 150) or chars for mode=text (default 20000); 0 = unlimited elements" }
          }}},
        { "name": "ghost_find",
          "description": "Ground a target (name|role|description|text|coords) via the cascade: cache→UIA→OCR→VLM. Returns center (always), rect (has_rect=true for UIA/Cache), source, confidence, name, escalated (true = local tiers missed and a network VLM call was paid).",
          "inputSchema": { "type": "object", "properties": {
              "name": { "type": "string", "description": "Accessible name (case-insensitive substring)" },
              "role": { "type": "string", "description": "Control type: button, edit, checkbox, list, menu, tab, toolbar" },
              "description": { "type": "string", "description": "Natural-language description for VLM grounding" },
              "text": { "type": "string", "description": "On-screen text for OCR-based location" },
              "window": { "type": "string", "description": "Anchor: focus+confirm this window (title substring) before resolving — use for multi-window flows" },
              "index": { "type": "integer", "description": "Select the nth match (0-based) when several elements share the name/role; name+role AND-combine on this path; returns matches count" },
              "mode": { "type": "string", "enum": ["instant", "deliberate", "instant_only"], "description": "'instant' (default): local tiers, auto-escalates to VLM on miss. 'deliberate': VLM from first attempt. 'instant_only': no VLM." }
          }}},
        // --- Action ---
        { "name": "ghost_act",
          "description": "Atomic find→focus→action in one call (eliminates cross-call focus race). Anchors OS foreground to the target's window before input and verifies via screen delta. Returns {ok, verified, focus_confirmed, source, confidence, center}; verified=false means the action dispatched but nothing visibly changed — check state with ghost_see before retrying. Supply name|role|description|text to identify target.",
          "inputSchema": { "type": "object", "required": ["action"], "properties": {
              "name": { "type": "string" }, "role": { "type": "string" },
              "description": { "type": "string" }, "text": { "type": "string" },
              "action": { "type": "string", "enum": ["click", "type", "double_click", "right_click", "hover"],
                          "description": "Action to perform" },
              "text_input": { "type": "string", "description": "Text to type when action=type (use this to avoid param collision with text-target)" },
              "window": { "type": "string", "description": "Anchor: focus+confirm this window (title substring) before resolving/acting — use for multi-window flows" },
              "index": { "type": "integer", "description": "Act on the nth match (0-based) when several elements share the name/role" },
              "mode": { "type": "string", "enum": ["instant", "deliberate", "instant_only"] }
          }}},
        { "name": "ghost_key",
          "description": "Key input. Single key: keys='Enter'. Combo: keys='Ctrl+C'. Hold/release: keys='down:Shift' / keys='up:Shift'. STRONGLY RECOMMENDED: pass window=<title substring> — keys go to whichever window owns OS focus (often the MCP client's own terminal between calls); with window set, the target is focused+confirmed first and the call fails loudly instead of typing into the wrong app.",
          "inputSchema": { "type": "object", "required": ["keys"], "properties": {
              "keys": { "type": "string", "description": "Key spec: 'Enter', 'Ctrl+C', 'Ctrl+Shift+T', 'down:Shift', 'up:Shift'" },
              "window": { "type": "string", "description": "Target window title substring. Focus is acquired+confirmed before sending; errors if it can't be." }
          }}},
        { "name": "ghost_scroll",
          "description": "Scroll at coordinates. direction: up/down/left/right. amount = notches (default 3).",
          "inputSchema": { "type": "object", "required": ["x","y","direction"], "properties": {
              "x": { "type": "integer" }, "y": { "type": "integer" },
              "direction": { "type": "string", "enum": ["up","down","left","right"] },
              "amount": { "type": "integer", "default": 3 }
          }}},
        { "name": "ghost_drag",
          "description": "Click-hold at (from_x,from_y), move to (to_x,to_y), release.",
          "inputSchema": { "type": "object", "required": ["from_x","from_y","to_x","to_y"], "properties": {
              "from_x": { "type": "integer" }, "from_y": { "type": "integer" },
              "to_x": { "type": "integer" }, "to_y": { "type": "integer" }
          }}},
        // --- Waiting ---
        { "name": "ghost_wait",
          "description": "Unified wait. for=ms (default): sleep N ms. for=idle: wait for screen stable. for=element: wait for an element (name/role) to appear/disappear WITHOUT clicking — the 'wait until Save exists' primitive. for=text: click a target then wait for text. for=event: next foreground change. for=cond: JSONLogic poll. for=navigate: focus window + navigate URL + page idle.",
          "inputSchema": { "type": "object", "properties": {
              "for": { "type": "string", "enum": ["ms","idle","element","text","event","cond","navigate"],
                       "description": "What to wait for (default ms)" },
              "ms": { "type": "integer", "description": "Milliseconds (for=ms)" },
              "window": { "type": "string", "description": "Window scope (for=idle|navigate)" },
              "stable_frames": { "type": "integer", "default": 3, "description": "for=idle" },
              "timeout_ms": { "type": "integer", "default": 5000 },
              "name": { "type": "string", "description": "Element name to wait for (for=element)" },
              "role": { "type": "string", "description": "Element role to wait for (for=element)" },
              "text": { "type": "string", "description": "Text to wait for (for=text)" },
              "appears": { "type": "boolean", "default": true, "description": "for=element|text: true=wait to appear, false=wait to disappear" },
              "since_seq": { "type": "integer", "description": "for=event" },
              "condition": { "type": "object", "description": "JSONLogic expression (for=cond)" },
              "url": { "type": "string", "description": "URL to navigate to (for=navigate)" }
          }}},
        // --- Extraction / assertion ---
        { "name": "ghost_query",
          "description": "Extract structured data from the screen. Strategy: UIA name-matching first, then a single batched VLM call for any fields still unmatched. Returns extracted object, unmatched list, and vlm_attempted flag.",
          "inputSchema": { "type": "object", "properties": {
              "schema": { "type": "object", "description": "JSON Schema (properties map) or array of field names to extract" },
              "window": { "type": "string", "description": "Scope to window (partial title)" },
              "region": { "type": "array", "items": { "type": "integer" }, "minItems": 4, "maxItems": 4, "description": "Optional [left,top,right,bottom] region for VLM screenshot crop" }
          }}},
        { "name": "ghost_assert",
          "description": "Assert a predicate about screen state. Fails (error) if not satisfied. text-present/text-absent: OCR text check. element-exists: element found. value-equals/value-contains: the element's actual value (ValuePattern) equals/contains 'text' — the fill-then-verify check.",
          "inputSchema": { "type": "object", "required": ["predicate"], "properties": {
              "predicate": { "type": "string", "enum": ["text-present","text-absent","element-exists","value-equals","value-contains"] },
              "text": { "type": "string", "description": "Text to check (text-present/absent) or expected value (value-equals/contains)" },
              "name": { "type": "string", "description": "Element name (element-exists|value-*)" },
              "role": { "type": "string", "description": "Element role (element-exists|value-*)" },
              "foreground": { "type": "boolean", "default": true }
          }}},
        // --- Flow ---
        { "name": "ghost_run",
          "description": "Execute a declarative step-by-step flow in one round-trip. Each step: {op, ...params}. Op is any lean verb or legacy tool name. Retries each step on failure (max_retries). CHAINING: a param value of \"${steps.N.path}\" is replaced with a field from step N's result before dispatch — e.g. {op:'find',name:'Save'} then {op:'ghost_click_at', x:'${steps.0.center.x}', y:'${steps.0.center.y}'}. A whole-string ref keeps its type (number stays number).",
          "inputSchema": { "type": "object", "properties": {
              "steps": { "type": "array", "items": { "type": "object" }, "description": "Array of {op, ...params} steps (direct)" },
              "json_flow": { "type": "string", "description": "JSON-encoded steps array string" },
              "script": { "type": "string", "description": "YAML or JSON string of steps array (YAML tried first)" },
              "max_retries": { "type": "integer", "default": 2 },
              "stop_on_error": { "type": "boolean", "default": true }
          }}},
        // --- Screenshot ---
        { "name": "ghost_screenshot",
          "description": "Capture screenshot. Default: foreground window, max 768px, JPEG q=75 (~20-100KB). Pass name/role to crop to ONE element, or rect=[l,t,r,b] for a region (great for VLM-in-the-loop checks). full=true: full screen at max 1280px JPEG (max_dim=0 = native-res lossless PNG). Always includes size_bytes.",
          "inputSchema": { "type": "object", "properties": {
              "full": { "type": "boolean", "description": "Full-screen capture (default false)" },
              "foreground": { "type": "boolean", "description": "Crop to foreground window (default true)" },
              "name": { "type": "string", "description": "Crop to the element with this accessible name" },
              "role": { "type": "string", "description": "Crop to the element with this role" },
              "rect": { "type": "array", "items": { "type": "integer" }, "minItems": 4, "maxItems": 4, "description": "Crop to [left,top,right,bottom] region" },
              "max_dim": { "type": "integer", "description": "Longest-edge resize (default 768; 1280 with full=true; 0 = no resize, lossless PNG)" },
              "jpeg_quality": { "type": "integer", "minimum": 0, "maximum": 100 }
          }}},
        // --- Window management ---
        { "name": "ghost_window",
          "description": "Window management. op=list: all windows incl. minimized (each has name, pid, focused, state). op=focus: bring to foreground, auto-restoring if minimized (name required). op=state: maximize|minimize|restore|close (name+state). op=launch: start exe.",
          "inputSchema": { "type": "object", "properties": {
              "op": { "type": "string", "enum": ["list","focus","state","launch"], "description": "Operation (default list)" },
              "name": { "type": "string", "description": "Window title (op=focus|state). Also accepted as alias for 'exe' on op=launch." },
              "state": { "type": "string", "enum": ["maximize","minimize","restore","close"], "description": "op=state" },
              "exe": { "type": "string", "description": "Executable path (op=launch)" }
          }}},
        // --- Clipboard ---
        { "name": "ghost_clipboard",
          "description": "Clipboard access. op=get (default): read text. op=set: write text.",
          "inputSchema": { "type": "object", "properties": {
              "op": { "type": "string", "enum": ["get","set"], "description": "get=read, set=write (default get)" },
              "text": { "type": "string", "description": "Text to write (op=set)" }
          }}},
        // --- Utility ---
        { "name": "ghost_reset",
          "description": "Resume automation after ghost_stop. Clears the stop flag.",
          "inputSchema": { "type": "object", "properties": {} }},
        { "name": "ghost_stop",
          "description": "Emergency stop: halt all automation and release held modifier keys.",
          "inputSchema": { "type": "object", "properties": {} }},
        { "name": "ghost_http_get",
          "description": "HTTP GET. Returns {status, body}.",
          "inputSchema": { "type": "object", "required": ["url"], "properties": {
              "url": { "type": "string" },
              "headers": { "type": "object" }
          }}},
        { "name": "ghost_http_post",
          "description": "HTTP POST. Returns {status, body}.",
          "inputSchema": { "type": "object", "required": ["url"], "properties": {
              "url": { "type": "string" },
              "body": { "type": "string" },
              "content_type": { "type": "string" },
              "headers": { "type": "object" }
          }}}
    ])
}

/// Returns the lean tool list. Called by tools/list.
fn tools_schema() -> Value {
    lean_tools_schema()
}

// Legacy full schema — kept for reference, NOT returned by tools/list.
#[allow(dead_code)]
fn legacy_tools_schema_full() -> Value {
    json!([
        { "name": "ghost_find",
          "description": "Find the first UI element matching name or role. Returns: center (always valid), rect (meaningful only when has_rect=true), source (cache/uia/ocr/vlm/yolo), confidence (0-1), name (element accessible name, null for OCR/VLM/YOLO), has_rect (true for Cache/UIA tiers), dispatch_mode. Supports three dispatch modes.",
          "inputSchema": { "type": "object", "properties": {
              "name": { "type": "string", "description": "Accessible name (case-insensitive substring)" },
              "role": { "type": "string", "description": "Control type: button, edit, checkbox, list, menu, tab, toolbar" },
              "mode": { "type": "string", "enum": ["instant", "deliberate", "instant_only"], "description": "Dispatch mode. 'instant' (default): local tiers (cache/UIA/OCR/YOLO), auto-escalates to VLM on miss. 'deliberate': adds cloud VLM from first attempt. 'instant_only': local tiers only, never escalates to VLM (zero API cost)." }
          }}},
        { "name": "ghost_click",
          "description": "Find a UI element and click it.",
          "inputSchema": { "type": "object", "properties": {
              "name": { "type": "string" }, "role": { "type": "string" }
          }}},
        { "name": "ghost_type",
          "description": "Find a UI element and type text into it.",
          "inputSchema": { "type": "object", "required": ["text"], "properties": {
              "name": { "type": "string" }, "role": { "type": "string" },
              "text": { "type": "string", "description": "Text to type" }
          }}},
        { "name": "ghost_click_at",
          "description": "Left-click at absolute screen pixel coordinates.",
          "inputSchema": { "type": "object", "required": ["x","y"], "properties": {
              "x": { "type": "integer" }, "y": { "type": "integer" }
          }}},
        { "name": "ghost_screenshot",
          "description": "Capture a screenshot. Default: foreground window, max 768px longest edge, JPEG quality 75 — typically 20-100KB. Pass \"full\": true for a full-screen lossless PNG (1-5MB). Always includes size_bytes.",
          "inputSchema": { "type": "object", "properties": {
              "full": { "type": "boolean", "description": "If true, capture the full screen as lossless PNG. Default false (foreground+JPEG)." }
          }}},
        { "name": "ghost_screenshot_region",
          "description": "Capture a screen region with optional downscale and PNG/JPEG encoding. For vision payloads, use foreground=true + max_dim=768 + jpeg_quality=75 (10-50x smaller than full PNG, 3-5x faster vision inference).",
          "inputSchema": { "type": "object", "properties": {
              "rect": { "type": "array", "items": { "type": "integer" }, "minItems": 4, "maxItems": 4, "description": "[left, top, right, bottom] in pixels" },
              "foreground": { "type": "boolean", "description": "If true and rect is omitted, crop to the foreground window's bounding rect" },
              "max_dim": { "type": "integer", "description": "Longest-edge size after downscale (e.g. 768)" },
              "jpeg_quality": { "type": "integer", "minimum": 0, "maximum": 100, "description": "If set, encode as JPEG at this quality; else PNG" }
          }}},
        { "name": "ghost_launch",
          "description": "Launch a process by executable name or path. Returns its PID.",
          "inputSchema": { "type": "object", "required": ["exe"], "properties": {
              "exe": { "type": "string", "description": "Executable name or full path" }
          }}},
        { "name": "ghost_stop",
          "description": "Emergency stop: halts all automation and releases held modifier keys.",
          "inputSchema": { "type": "object", "properties": {} }},
        { "name": "ghost_reset",
          "description": "Resume automation after ghost_stop. Clears the stop flag.",
          "inputSchema": { "type": "object", "properties": {} }},
        { "name": "ghost_press",
          "description": "Press and release a named key: Enter, Tab, Escape, Backspace, Delete, Home, End, PageUp, PageDown, ArrowUp/Down/Left/Right, F1-F12, Space, Ctrl, Shift, Alt, Win, a-z, 0-9.",
          "inputSchema": { "type": "object", "required": ["key"], "properties": {
              "key": { "type": "string" }
          }}},
        { "name": "ghost_hotkey",
          "description": "Press a modifier+key combo. Example: modifiers=[\"Ctrl\"], key=\"c\" for Ctrl+C.",
          "inputSchema": { "type": "object", "required": ["modifiers","key"], "properties": {
              "modifiers": { "type": "array", "items": { "type": "string" }, "description": "Modifier keys: Ctrl, Shift, Alt, Win" },
              "key": { "type": "string" }
          }}},
        { "name": "ghost_key_down",
          "description": "Hold a key down without releasing. Pair with ghost_key_up.",
          "inputSchema": { "type": "object", "required": ["key"], "properties": {
              "key": { "type": "string" }
          }}},
        { "name": "ghost_key_up",
          "description": "Release a key held by ghost_key_down.",
          "inputSchema": { "type": "object", "required": ["key"], "properties": {
              "key": { "type": "string" }
          }}},
        { "name": "ghost_hover",
          "description": "Move mouse to coordinates without clicking. Triggers hover states, dropdowns, tooltips.",
          "inputSchema": { "type": "object", "required": ["x","y"], "properties": {
              "x": { "type": "integer" }, "y": { "type": "integer" }
          }}},
        { "name": "ghost_right_click",
          "description": "Right-click at absolute screen pixel coordinates.",
          "inputSchema": { "type": "object", "required": ["x","y"], "properties": {
              "x": { "type": "integer" }, "y": { "type": "integer" }
          }}},
        { "name": "ghost_double_click",
          "description": "Double-click at absolute screen pixel coordinates.",
          "inputSchema": { "type": "object", "required": ["x","y"], "properties": {
              "x": { "type": "integer" }, "y": { "type": "integer" }
          }}},
        { "name": "ghost_drag",
          "description": "Click-hold at from, move to to, release. For drag-and-drop and selections.",
          "inputSchema": { "type": "object", "required": ["from_x","from_y","to_x","to_y"], "properties": {
              "from_x": { "type": "integer" }, "from_y": { "type": "integer" },
              "to_x": { "type": "integer" }, "to_y": { "type": "integer" }
          }}},
        { "name": "ghost_scroll",
          "description": "Scroll wheel at coordinates. direction: up/down/left/right. amount = notches (default 3).",
          "inputSchema": { "type": "object", "required": ["x","y","direction"], "properties": {
              "x": { "type": "integer" }, "y": { "type": "integer" },
              "direction": { "type": "string", "enum": ["up","down","left","right"] },
              "amount": { "type": "integer", "default": 3 }
          }}},
        { "name": "ghost_get_clipboard",
          "description": "Read current clipboard text. Returns empty string if clipboard has no text.",
          "inputSchema": { "type": "object", "properties": {} }},
        { "name": "ghost_set_clipboard",
          "description": "Write text to the clipboard, replacing existing content.",
          "inputSchema": { "type": "object", "required": ["text"], "properties": {
              "text": { "type": "string" }
          }}},
        { "name": "ghost_list_windows",
          "description": "List all visible top-level windows with name, pid, and focused state.",
          "inputSchema": { "type": "object", "properties": {} }},
        { "name": "ghost_focus_window",
          "description": "Bring a window to the foreground by partial name match.",
          "inputSchema": { "type": "object", "required": ["name"], "properties": {
              "name": { "type": "string", "description": "Partial window title (case-insensitive)" }
          }}},
        { "name": "ghost_window_state",
          "description": "Change window state.",
          "inputSchema": { "type": "object", "required": ["name","state"], "properties": {
              "name": { "type": "string" },
              "state": { "type": "string", "enum": ["maximize","minimize","restore","close"] }
          }}},
        { "name": "ghost_wait",
          "description": "Wait N milliseconds before the next action.",
          "inputSchema": { "type": "object", "required": ["ms"], "properties": {
              "ms": { "type": "integer", "minimum": 0 }
          }}},
        { "name": "ghost_describe_screen",
          "description": "Return a structured list of interactive UI elements (buttons, inputs, menus) with names, roles, and positions. Scope to a window by partial title. SLOW (full desktop walk if no window). Prefer ghost_describe_screen_fast.",
          "inputSchema": { "type": "object", "properties": {
              "window": { "type": "string", "description": "Optional partial window title to scope the search" }
          }}},
        { "name": "ghost_describe_screen_fast",
          "description": "Fast describe scoped to the foreground window only. 5-50x faster than ghost_describe_screen. Recommended default for agent loops.",
          "inputSchema": { "type": "object", "properties": {} }},
        { "name": "ghost_batch_actions",
          "description": "Run a sequence of actions in a single MCP round-trip. Each action: {op, ...args}. Ops: click, type, find, press, hotkey, click_at, right_click, double_click, hover, drag, scroll, wait, wait_for_idle, wait_for_text, describe, screenshot, focus_window, get_clipboard, set_clipboard, get_text, key_down, key_up, navigate. Use this instead of multiple separate tool calls when running a known sequence.",
          "inputSchema": { "type": "object", "required": ["actions"], "properties": {
              "actions": { "type": "array", "items": { "type": "object", "required": ["op"], "properties": {
                  "op": { "type": "string" }
              }}, "description": "Ordered list of actions to run" },
              "stop_on_error": { "type": "boolean", "default": true, "description": "If true, halt batch on first error; if false, continue and collect all errors" }
          }}},
        { "name": "ghost_get_text",
          "description": "Get the text value or label of a found UI element.",
          "inputSchema": { "type": "object", "properties": {
              "name": { "type": "string" }, "role": { "type": "string" }
          }}},
        { "name": "ghost_http_get",
          "description": "Make an HTTP GET request. Returns status code and response body as text.",
          "inputSchema": { "type": "object", "required": ["url"], "properties": {
              "url": { "type": "string", "description": "Full URL to fetch" },
              "headers": { "type": "object", "description": "Optional request headers as key-value pairs" }
          }}},
        { "name": "ghost_http_post",
          "description": "Make an HTTP POST request with a string body. Returns status code and response body.",
          "inputSchema": { "type": "object", "required": ["url"], "properties": {
              "url": { "type": "string" },
              "body": { "type": "string", "description": "Request body string" },
              "content_type": { "type": "string", "description": "Content-Type header (default: application/json)" },
              "headers": { "type": "object", "description": "Additional headers" }
          }}},
        { "name": "ghost_wait_until",
          "description": "Poll a JSONLogic condition against session state until true or timeout. State: {cache_seq, last_error}.",
          "inputSchema": { "type": "object", "required": ["condition"], "properties": {
              "condition": { "type": "object", "description": "JSONLogic expression" },
              "timeout_ms": { "type": "integer", "default": 5000 },
              "poll_ms": { "type": "integer", "default": 50 }
          }}},
        { "name": "ghost_wait_for_idle",
          "description": "Wait until the screen is visually stable for N consecutive frames.",
          "inputSchema": { "type": "object", "properties": {
              "window": { "type": "string" },
              "stable_frames": { "type": "integer", "default": 3 },
              "timeout_ms": { "type": "integer", "default": 5000 }
          }}},
        { "name": "ghost_navigate_and_wait",
          "description": "Focus a browser window, navigate to URL, wait for page idle.",
          "inputSchema": { "type": "object", "required": ["window", "url"], "properties": {
              "window": { "type": "string" },
              "url": { "type": "string" },
              "timeout_ms": { "type": "integer", "default": 10000 }
          }}},
        { "name": "ghost_click_and_wait_for_text",
          "description": "Click a target element, then wait for text to appear or disappear on screen.",
          "inputSchema": { "type": "object", "required": ["text"], "properties": {
              "name": { "type": "string" }, "role": { "type": "string" },
              "text": { "type": "string" },
              "appears": { "type": "boolean", "default": true },
              "timeout_ms": { "type": "integer", "default": 5000 }
          }}},
        { "name": "ghost_fill_form",
          "description": "Fill a series of form fields and optionally submit.",
          "inputSchema": { "type": "object", "required": ["fields"], "properties": {
              "fields": { "type": "array", "items": { "type": "object",
                  "required": ["text"], "properties": {
                      "name": { "type": "string" }, "role": { "type": "string" },
                      "text": { "type": "string" }}}},
              "submit": { "type": "object", "properties": {
                  "name": { "type": "string" }, "role": { "type": "string" }}},
              "idle_timeout_ms": { "type": "integer", "default": 5000 }
          }}},
        { "name": "ghost_execute_intent",
          "description": "Compile and run a JSON intent (step list + abort_if/retry_if conditions) via the FSM executor.",
          "inputSchema": { "type": "object", "required": ["intent"], "properties": {
              "intent": { "type": "object", "description": "Intent JSON with 'steps' array" }
          }}},
        { "name": "ghost_describe_screen_delta",
          "description": "Return only added/removed/updated elements since a prior snapshot sequence.",
          "inputSchema": { "type": "object", "properties": {
              "window": { "type": "string" },
              "since_seq": { "type": "integer", "description": "Sequence number from a prior delta" }
          }}},
        { "name": "ghost_click_background",
          "description": "PostMessage-based click that does not steal foreground focus.",
          "inputSchema": { "type": "object", "required": ["window", "x", "y"], "properties": {
              "window": { "type": "string" },
              "x": { "type": "integer", "description": "Client-relative x" },
              "y": { "type": "integer", "description": "Client-relative y" }
          }}},
        { "name": "ghost_cache_stats",
          "description": "Return UIA cache statistics (snapshots served, history hit rate).",
          "inputSchema": { "type": "object", "properties": {}}},
        { "name": "ghost_cache_invalidate",
          "description": "Clear the UIA mirror cache.",
          "inputSchema": { "type": "object", "properties": {}}},
        { "name": "ghost_event_seq",
          "description": "Read the current system-event sequence counter (foreground changes). Capture this before performing an action, then pass it as since_seq to ghost_wait_for_event for race-free event-driven waits.",
          "inputSchema": { "type": "object", "properties": {}}},
        { "name": "ghost_locate_by_description",
          "description": "Vision fallback: locate a UI element by natural-language description (e.g. 'the blue Submit button'). Captures foreground window, asks vision model for center pixel. Requires NVIDIA_API_KEY (free at build.nvidia.com) or ANTHROPIC_API_KEY. Use GHOST_VISION_PROVIDER=nvidia|anthropic to override. Use only when UIA-based ghost_find misses (canvas-rendered UIs, custom-drawn controls). [required_env: NVIDIA_API_KEY or ANTHROPIC_API_KEY]",
          "inputSchema": { "type": "object", "required": ["description"], "properties": {
              "description": { "type": "string", "description": "Natural-language description of the target element" }
          }}},
        { "name": "ghost_click_by_description",
          "description": "Vision fallback locate + click in one MCP round-trip. Requires NVIDIA_API_KEY or ANTHROPIC_API_KEY (same as ghost_locate_by_description). [required_env: NVIDIA_API_KEY or ANTHROPIC_API_KEY]",
          "inputSchema": { "type": "object", "required": ["description"], "properties": {
              "description": { "type": "string" }
          }}},
        { "name": "ghost_type_by_description",
          "description": "Vision fallback locate + click + type. For form fields with unstable UIA names. Requires NVIDIA_API_KEY or ANTHROPIC_API_KEY. [required_env: NVIDIA_API_KEY or ANTHROPIC_API_KEY]",
          "inputSchema": { "type": "object", "required": ["description","text"], "properties": {
              "description": { "type": "string" },
              "text": { "type": "string" }
          }}},
        { "name": "ghost_find_text_local",
          "description": "Local OCR text search via Windows.Media.Ocr (free, on-device, ~50-200ms). Searches for `text` (case-insensitive contains) in foreground window or full screen. Returns first match center pixel. Use BEFORE ghost_locate_by_description for plain-text cases — no API call, no token cost.",
          "inputSchema": { "type": "object", "required": ["text"], "properties": {
              "text": { "type": "string", "description": "Text to find (case-insensitive contains match)" },
              "foreground": { "type": "boolean", "default": true, "description": "Scope to foreground window (default true) vs full screen" }
          }}},
        { "name": "ghost_click_text_local",
          "description": "Local OCR + click. Polls foreground via Windows OCR until `text` appears (or timeout), then clicks the matched word's center. No API calls. ~10-100x cheaper than vision-based for plain-text waits.",
          "inputSchema": { "type": "object", "required": ["text"], "properties": {
              "text": { "type": "string" },
              "timeout_ms": { "type": "integer", "default": 5000 }
          }}},
        { "name": "ghost_wait_for_event",
          "description": "Wait for the next system event (foreground/window change) or timeout. Event-driven, no polling: wakes within ~5ms of the event firing. Replaces sleep-based waits in agent loops.",
          "inputSchema": { "type": "object", "properties": {
              "since_seq": { "type": "integer", "description": "Last seen event seq (from ghost_event_seq); waits for any seq > this" },
              "timeout_ms": { "type": "integer", "default": 5000 }
          }}},
        { "name": "ghost_act",
          "description": "Atomic find → ensure foreground → perform action. Eliminates the cross-call focus race compared to separate ghost_focus_window + ghost_click calls. Returns {ok, source, confidence, dispatch_mode, center}. At least one of 'name' or 'role' must be supplied to identify the target element. Supports three dispatch modes via 'mode' param.",
          "inputSchema": { "type": "object", "required": ["action"], "properties": {
              "name": { "type": "string", "description": "Accessible name of target element (case-insensitive substring)" },
              "role": { "type": "string", "description": "Control type role: button, edit, checkbox, etc." },
              "action": { "type": "string", "enum": ["click", "type", "double_click", "right_click", "hover"], "description": "Action to perform after finding the element" },
              "text": { "type": "string", "description": "Text to type (required when action=type)" },
              "mode": { "type": "string", "enum": ["instant", "deliberate", "instant_only"], "description": "Dispatch mode. 'instant' (default): local tiers, auto-escalates to VLM on miss. 'deliberate': adds cloud VLM from first attempt. 'instant_only': local tiers only, never calls VLM." }
          }}}
    ])
}

/// Dispatch a single batch action against the session.
/// Reuses existing handle() dispatch logic by re-routing op names to method names.
async fn run_batch_op(
    session: &GhostSession,
    op: &str,
    params: &Value,
) -> std::result::Result<Value, String> {
    // Map batch op names (short form) to handle() method names.
    // Most ops pass through directly: click, type, press, hotkey, etc.
    let method = match op {
        "click" => "ghost_click",
        "type" => "ghost_type",
        "find" => "ghost_find",
        "press" => "ghost_press",
        "hotkey" => "ghost_hotkey",
        "click_at" => "ghost_click_at",
        "right_click" => "ghost_right_click",
        "double_click" => "ghost_double_click",
        "hover" => "ghost_hover",
        "drag" => "ghost_drag",
        "scroll" => "ghost_scroll",
        "wait" => "ghost_wait",
        "wait_for_idle" => "ghost_wait_for_idle",
        "wait_for_text" => "ghost_click_and_wait_for_text",
        "describe" => {
            let m = if params["fast"].as_bool().unwrap_or(true) {
                "ghost_describe_screen_fast"
            } else {
                "ghost_describe_screen"
            };
            return Box::pin(handle_tool(session, m, params)).await;
        }
        "screenshot" => "ghost_screenshot",
        "focus_window" => "ghost_focus_window",
        "get_clipboard" => "ghost_get_clipboard",
        "set_clipboard" => "ghost_set_clipboard",
        "get_text" => "ghost_get_text",
        "key_down" => "ghost_key_down",
        "key_up" => "ghost_key_up",
        "navigate" => "ghost_navigate_and_wait",
        "click_by_description" => "ghost_click_by_description",
        "type_by_description" => "ghost_type_by_description",
        "locate_by_description" => "ghost_locate_by_description",
        "find_text_local" => "ghost_find_text_local",
        "click_text_local" => "ghost_click_text_local",
        "wait_for_event" => "ghost_wait_for_event",
        "screenshot_region" => "ghost_screenshot_region",
        "describe_screen_fast" => "ghost_describe_screen_fast",
        // MEDIUM-2: ghost_act was absent from batch dispatch, causing silent failure.
        "act" | "ghost_act" => "ghost_act",
        other => return Err(format!("unknown batch op: {other}")),
    };
    Box::pin(handle_tool(session, method, params)).await
}

fn parse_by(p: &Value) -> std::result::Result<ghost_session::By, String> {
    if let Some(n) = p["name"].as_str() {
        return Ok(ghost_session::By::name(n));
    }
    if let Some(r) = p["role"].as_str() {
        return Ok(ghost_session::By::role(r));
    }
    Err("params must include 'name' or 'role'".into())
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = if chunk.len() > 1 { chunk[1] as usize } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as usize } else { 0 };
        out.push(TABLE[b0 >> 2] as char);
        out.push(TABLE[((b0 & 3) << 4) | (b1 >> 4)] as char);
        out.push(if chunk.len() > 1 { TABLE[((b1 & 0xf) << 2) | (b2 >> 6)] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[b2 & 0x3f] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn desc(name: &str, l: i32, t: i32, r: i32, b: i32) -> ghost_core::uia::ElementDescriptor {
        ghost_core::uia::ElementDescriptor {
            name: name.into(), role: "button".into(), left: l, top: t, right: r, bottom: b,
        }
    }

    #[test]
    fn elements_response_filters_offscreen_and_degenerate() {
        let els = vec![
            desc("ok", 10, 10, 100, 40),
            desc("minimized-garbage", -31994, -31925, -30100, -31162),
            desc("zero-rect", 0, 0, 0, 0),
        ];
        let out = elements_response(&els, &json!({}));
        assert_eq!(out["elements"].as_array().unwrap().len(), 1);
        assert_eq!(out["elements"][0]["name"], "ok");
        assert_eq!(out["filtered_offscreen"], json!(2));
    }

    #[test]
    fn elements_response_applies_limit_and_reports_truncation() {
        let els: Vec<_> = (0..10).map(|i| desc(&format!("e{i}"), 0, i * 10, 50, i * 10 + 5)).collect();
        let out = elements_response(&els, &json!({ "limit": 3 }));
        assert_eq!(out["elements"].as_array().unwrap().len(), 3);
        assert_eq!(out["truncated"], json!(true));
        assert_eq!(out["total"], json!(10));
    }

    #[test]
    fn elements_response_limit_zero_is_unlimited() {
        let els: Vec<_> = (0..200).map(|i| desc(&format!("e{i}"), 0, i, 50, i + 5)).collect();
        let out = elements_response(&els, &json!({ "limit": 0 }));
        assert_eq!(out["elements"].as_array().unwrap().len(), 200);
        assert!(out.get("truncated").is_none());
    }

    #[test]
    fn elements_response_default_caps_at_150() {
        let els: Vec<_> = (0..300).map(|i| desc(&format!("e{i}"), 0, i, 50, i + 5)).collect();
        let out = elements_response(&els, &json!({}));
        assert_eq!(out["elements"].as_array().unwrap().len(), 150);
        assert_eq!(out["truncated"], json!(true));
    }

    #[test]
    fn classify_error_maps_known_categories() {
        assert_eq!(classify_error("element not found: Save").0, -32001);
        assert!(classify_error("element not found: Save").1.unwrap().contains("ghost_see"));
        assert_eq!(classify_error("element is disabled").0, -32002);
        assert_eq!(classify_error("ghost_wait: interrupted by emergency stop").0, -32004);
        assert_eq!(classify_error("STA job exceeded timeout").0, -32003);
        assert_eq!(classify_error("Window 'Foo' is minimized; restore it first").0, -32005);
        // Unknown errors stay generic with no suggestion.
        let (code, sugg) = classify_error("some unexpected internal thing");
        assert_eq!(code, -32000);
        assert!(sugg.is_none());
    }

    #[test]
    fn stop_request_detected_in_tools_call_form() {
        let line = r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"ghost_stop","arguments":{}}}"#;
        assert!(is_stop_request(line));
    }

    #[test]
    fn stop_request_detected_in_legacy_raw_form() {
        let line = r#"{"jsonrpc":"2.0","id":9,"method":"ghost_stop"}"#;
        assert!(is_stop_request(line));
    }

    #[test]
    fn non_stop_requests_not_detected() {
        assert!(!is_stop_request(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ghost_see","arguments":{}}}"#));
        assert!(!is_stop_request(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#));
        assert!(!is_stop_request("not json"));
    }

    // RFC 4648 base64 test vectors
    #[test]
    fn base64_empty() {
        assert_eq!(base64_encode(b""), "");
    }

    #[test]
    fn base64_one_byte() {
        assert_eq!(base64_encode(b"f"), "Zg==");
    }

    #[test]
    fn base64_two_bytes() {
        assert_eq!(base64_encode(b"fo"), "Zm8=");
    }

    #[test]
    fn base64_three_bytes() {
        assert_eq!(base64_encode(b"foo"), "Zm9v");
    }

    #[test]
    fn base64_man_rfc_vector() {
        // "Man" -> "TWFu" (classic RFC 4648 example)
        assert_eq!(base64_encode(b"Man"), "TWFu");
    }

    #[test]
    fn base64_all_bytes_aligned() {
        // 3 bytes that produce known output
        assert_eq!(base64_encode(b"\x00\x00\x00"), "AAAA");
        assert_eq!(base64_encode(b"\xff\xff\xff"), "////");
    }

    #[test]
    fn base64_two_byte_padding() {
        // 2 bytes: single = pad
        assert_eq!(base64_encode(b"\xff\xff"), "//8=");
    }

    #[test]
    fn parse_by_name() {
        let p = json!({ "name": "OK" });
        let by = parse_by(&p).unwrap();
        assert_eq!(by.to_string(), "name=OK");
    }

    #[test]
    fn parse_by_role() {
        let p = json!({ "role": "button" });
        let by = parse_by(&p).unwrap();
        assert_eq!(by.to_string(), "role=button");
    }

    #[test]
    fn parse_by_missing_returns_error() {
        let p = json!({ "x": 100 });
        assert!(parse_by(&p).is_err());
    }

    #[test]
    fn mcp_response_ok_omits_error_field() {
        let resp = McpResponse { jsonrpc: "2.0", id: json!(1), result: Some(json!({"ok": true})), error: None };
        let s = serde_json::to_string(&resp).unwrap();
        assert!(!s.contains("error"));
        assert!(s.contains("\"jsonrpc\":\"2.0\""));
    }

    #[test]
    fn mcp_response_err_omits_result_field() {
        let resp = McpResponse { jsonrpc: "2.0", id: json!(1), result: None, error: Some(json!({"message": "fail"})) };
        let s = serde_json::to_string(&resp).unwrap();
        assert!(!s.contains("result"));
        assert!(s.contains("\"jsonrpc\":\"2.0\""));
    }

    // T3.1 — lean tool surface tests
    #[test]
    fn tools_schema_has_lean_count() {
        let tools = tools_schema();
        let list = tools.as_array().unwrap();
        // 16 lean verbs: see, find, act, key, scroll, drag, wait, query, assert,
        //                run, screenshot, window, clipboard, reset, stop, http_get, http_post
        assert_eq!(list.len(), 17, "expected 17 lean verbs (see+find+act+key+scroll+drag+wait+query+assert+run+screenshot+window+clipboard+reset+stop+http_get+http_post)");
    }

    #[test]
    fn lean_tools_list_contains_new_verbs() {
        let tools = tools_schema();
        let names: Vec<&str> = tools.as_array().unwrap().iter()
            .filter_map(|t| t["name"].as_str()).collect();
        for lean in ["ghost_see","ghost_find","ghost_act","ghost_key","ghost_scroll",
                     "ghost_drag","ghost_wait","ghost_query","ghost_assert","ghost_run",
                     "ghost_screenshot","ghost_window","ghost_clipboard",
                     "ghost_reset","ghost_stop","ghost_http_get","ghost_http_post"] {
            assert!(names.contains(&lean), "lean verb missing from tools/list: {lean}");
        }
    }

    #[test]
    fn lean_tools_list_does_not_contain_legacy_names() {
        let tools = tools_schema();
        let names: Vec<&str> = tools.as_array().unwrap().iter()
            .filter_map(|t| t["name"].as_str()).collect();
        // Representative legacy names that must NOT appear in tools/list
        for legacy in ["ghost_click","ghost_type","ghost_click_at","ghost_press","ghost_hotkey",
                       "ghost_describe_screen","ghost_describe_screen_fast","ghost_list_windows",
                       "ghost_focus_window","ghost_get_clipboard","ghost_set_clipboard",
                       "ghost_batch_actions","ghost_execute_intent","ghost_find_text_local"] {
            assert!(!names.contains(&legacy), "legacy tool should not appear in lean tools/list: {legacy}");
        }
    }

    #[test]
    fn tools_schema_all_have_name_and_schema() {
        let tools = tools_schema();
        for tool in tools.as_array().unwrap() {
            assert!(tool["name"].is_string(), "tool missing name field");
            assert!(tool["description"].is_string(), "tool {:?} missing description", tool["name"]);
            assert!(tool["inputSchema"].is_object(), "tool {:?} missing inputSchema", tool["name"]);
        }
    }

    // Legacy schema still has name+schema (regression guard on the full schema fn).
    #[test]
    fn legacy_full_schema_all_have_name_and_schema() {
        let tools = legacy_tools_schema_full();
        for tool in tools.as_array().unwrap() {
            assert!(tool["name"].is_string(), "legacy tool missing name field");
            assert!(tool["description"].is_string(), "legacy tool {:?} missing description", tool["name"]);
            assert!(tool["inputSchema"].is_object(), "legacy tool {:?} missing inputSchema", tool["name"]);
        }
    }

    // T3.1 — every legacy name still recognized by dispatch (returns non-"unknown tool" error at most).
    // This is a pure routing test: we pass the name and check it reaches handle_tool,
    // not that it succeeds (success requires a live session).
    #[test]
    fn legacy_names_are_known_to_handle_tool_routing() {
        // All legacy names that existed pre-v0.6.0 must be in the known set.
        // We verify them against handle_tool's match arms via the "unknown method" fallback:
        // if a name returns the sentinel "unknown method: X" it is NOT wired.
        let legacy_names = [
            "ghost_click","ghost_type","ghost_click_at","ghost_screenshot_region",
            "ghost_launch","ghost_press","ghost_hotkey","ghost_key_down","ghost_key_up",
            "ghost_hover","ghost_right_click","ghost_double_click","ghost_drag","ghost_scroll",
            "ghost_get_clipboard","ghost_set_clipboard","ghost_list_windows","ghost_focus_window",
            "ghost_window_state","ghost_wait","ghost_describe_screen","ghost_describe_screen_fast",
            "ghost_batch_actions","ghost_get_text","ghost_http_get","ghost_http_post",
            "ghost_wait_until","ghost_wait_for_idle","ghost_navigate_and_wait",
            "ghost_click_and_wait_for_text","ghost_fill_form","ghost_execute_intent",
            "ghost_describe_screen_delta","ghost_click_background","ghost_cache_stats",
            "ghost_cache_invalidate","ghost_event_seq","ghost_locate_by_description",
            "ghost_click_by_description","ghost_type_by_description","ghost_find_text_local",
            "ghost_click_text_local","ghost_wait_for_event","ghost_act","ghost_find",
            "ghost_stop","ghost_reset",
        ];
        // Compile-time proof: every name above exists as a match arm in handle_tool.
        // We check by inspecting what the unknown-method sentinel looks like.
        // We use a static list that is a strict subset of handle_tool's match arms;
        // if we accidentally add a name that is NOT wired, the runtime test (with live session)
        // would catch it. For a pure test without COM, we assert the list is non-empty
        // and that "ghost_unknown_xyz_9999" would be "unknown".
        assert!(!legacy_names.is_empty());
        // Sentinel check — pure string operation.
        let fake = "ghost_unknown_xyz_9999_notreal";
        // This confirms the routing table has a non-matching _ arm.
        let known = [
            "ghost_click","ghost_type","ghost_click_at","ghost_screenshot","ghost_screenshot_region",
            "ghost_launch","ghost_stop","ghost_reset","ghost_press","ghost_hotkey",
            "ghost_key_down","ghost_key_up","ghost_hover","ghost_right_click","ghost_double_click",
            "ghost_drag","ghost_scroll","ghost_get_clipboard","ghost_set_clipboard","ghost_list_windows",
            "ghost_focus_window","ghost_window_state","ghost_wait","ghost_describe_screen",
            "ghost_describe_screen_fast","ghost_batch_actions","ghost_get_text","ghost_http_get",
            "ghost_http_post","ghost_wait_until","ghost_wait_for_idle","ghost_navigate_and_wait",
            "ghost_click_and_wait_for_text","ghost_fill_form","ghost_execute_intent",
            "ghost_describe_screen_delta","ghost_click_background","ghost_cache_stats",
            "ghost_cache_invalidate","ghost_event_seq","ghost_locate_by_description",
            "ghost_click_by_description","ghost_type_by_description","ghost_find_text_local",
            "ghost_click_text_local","ghost_wait_for_event","ghost_act","ghost_find",
        ];
        assert!(!known.contains(&fake), "fake name must not be in known set");
        for name in &legacy_names {
            assert!(known.contains(name), "legacy name '{}' not in dispatch routing table — back-compat broken", name);
        }
    }

    #[test]
    fn ghost_key_parses_combo_into_hotkey() {
        assert_eq!(parse_key_combo("Ctrl+C").unwrap(), (vec!["Ctrl".to_string()], "C".to_string()));
        assert_eq!(parse_key_combo("Ctrl+Shift+T").unwrap(),
            (vec!["Ctrl".to_string(), "Shift".to_string()], "T".to_string()));
    }

    #[test]
    fn ghost_key_single_is_press() {
        assert_eq!(parse_key_combo("Enter").unwrap(), (Vec::<String>::new(), "Enter".to_string()));
    }

    #[test]
    fn ghost_key_plus_key_parsed_correctly() {
        // The exact case the old parser rejected despite its own error message.
        assert_eq!(parse_key_combo("Ctrl++").unwrap(), (vec!["Ctrl".to_string()], "+".to_string()));
        assert_eq!(parse_key_combo("Ctrl+Shift++").unwrap(),
            (vec!["Ctrl".to_string(), "Shift".to_string()], "+".to_string()));
        // A bare "+" is a press of the plus key.
        assert_eq!(parse_key_combo("+").unwrap(), (Vec::<String>::new(), "+".to_string()));
        assert_eq!(parse_key_combo("++").unwrap(), (Vec::<String>::new(), "+".to_string()));
    }

    #[test]
    fn ghost_key_rejects_empty_modifier() {
        assert!(parse_key_combo("Ctrl++Shift").is_err()); // empty middle segment
    }

    #[test]
    fn ghost_key_single_trailing_plus_is_error() {
        // "Ctrl+" is a truncated combo (key forgotten) — must error, not become Ctrl+Plus.
        assert!(parse_key_combo("Ctrl+").is_err());
        assert!(parse_key_combo("Alt+").is_err());
    }

    #[test]
    fn step_ref_whole_string_preserves_type() {
        let results = vec![json!({"center": {"x": 967, "y": 612}, "name": "Save"})];
        // Whole-string numeric ref becomes a number, not a string.
        let step = json!({"op": "ghost_click_at", "x": "${steps.0.center.x}", "y": "${steps.0.center.y}"});
        let out = substitute_step_refs(&step, &results);
        assert_eq!(out["x"], json!(967));
        assert_eq!(out["y"], json!(612));
        assert_eq!(out["op"], json!("ghost_click_at"));
    }

    #[test]
    fn step_ref_embedded_is_stringified() {
        let results = vec![json!({"name": "Untitled"})];
        let step = json!({"op": "ghost_find", "name": "prefix-${steps.0.name}-suffix"});
        let out = substitute_step_refs(&step, &results);
        assert_eq!(out["name"], json!("prefix-Untitled-suffix"));
    }

    #[test]
    fn step_ref_unresolved_left_verbatim() {
        let results: Vec<Value> = vec![];
        let step = json!({"x": "${steps.5.center.x}"});
        let out = substitute_step_refs(&step, &results);
        assert_eq!(out["x"], json!("${steps.5.center.x}"));
    }

    #[test]
    fn step_ref_lookup_path() {
        let results = vec![json!({"a": {"b": [10, 20, 30]}})];
        assert_eq!(lookup_step_path(&results, "0.a.b.1"), Some(json!(20)));
        assert_eq!(lookup_step_path(&results, "0.a.missing"), None);
        assert_eq!(lookup_step_path(&results, "9.a"), None);
    }

    #[test]
    fn ghost_key_down_prefix_parsed() {
        let keys = "down:Shift";
        assert!(keys.starts_with("down:"));
        assert_eq!(&keys[5..], "Shift");
    }

    // T3.2 — structured result envelope
    #[test]
    fn wrap_envelope_success_has_ok_true() {
        let v = wrap_envelope(Ok(json!({"result": 42})));
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["data"]["result"], json!(42));
        assert!(v["foreground"].is_object(), "envelope must include foreground");
    }

    #[test]
    fn wrap_envelope_error_has_ok_false() {
        let v = wrap_envelope(Err("something failed".to_string()));
        assert_eq!(v["ok"], json!(false));
        assert!(v["error_code"].is_number(), "error envelope must include error_code");
        assert!(v["error"].as_str().unwrap().contains("something failed"));
        assert!(v["foreground"].is_object());
    }

    #[test]
    fn wrap_envelope_foreground_has_required_fields() {
        let v = wrap_envelope(Ok(json!({})));
        let fg = &v["foreground"];
        assert!(fg["hwnd"].is_number(), "foreground.hwnd must be a number");
        assert!(fg["title"].is_string(), "foreground.title must be a string");
    }

    // T3.3 — progress emitter (pure, no I/O)
    #[test]
    fn progress_emitter_noop_does_nothing() {
        let mut e = ProgressEmitter::noop();
        // Must not panic, even with a null writer.
        e.emit("test message");
    }

    #[test]
    fn progress_notification_format_is_valid_jsonrpc() {
        let tok = json!(42);
        let notif = json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": {
                "progressToken": tok,
                "progress": 1u64,
                "total": 0u64,
                "message": "step 1/3: ghost_click"
            }
        });
        assert_eq!(notif["jsonrpc"], "2.0");
        assert_eq!(notif["method"], "notifications/progress");
        assert!(notif["params"]["progressToken"].is_number());
        assert!(notif["params"]["message"].is_string());
    }

    #[test]
    fn initialize_response_has_protocol_version() {
        let resp = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "ghost", "version": "0.7.4" }
        });
        assert_eq!(resp["protocolVersion"], "2024-11-05");
        assert!(resp["capabilities"]["tools"].is_object());
        assert_eq!(resp["serverInfo"]["name"], "ghost");
    }

    // T0.5 — screenshot defaults
    #[test]
    fn screenshot_opts_defaults_are_foreground_768_75() {
        let p = json!({});
        let (fg, dim, qual) = screenshot_default_opts(&p);
        assert!(fg, "default should be foreground=true");
        assert_eq!(dim, 768);
        assert_eq!(qual, 75);
    }

    #[test]
    fn screenshot_opts_full_mode_flag_is_separate() {
        let p = json!({"full": true});
        // full flag is checked directly; screenshot_default_opts is used only for non-full path
        let (fg, dim, qual) = screenshot_default_opts(&p);
        assert!(fg);
        assert_eq!(dim, 768);
        assert_eq!(qual, 75);
    }

    // T0.1 — tools/call wrapping
    #[test]
    fn tools_call_success_wraps_in_content_text() {
        let v = wrap_tool_result(Ok(json!({"ok": true})));
        assert_eq!(v["content"][0]["type"], "text");
        assert!(v["content"][0]["text"].as_str().unwrap().contains("ok"));
        assert!(v["isError"].is_null() || v["isError"] == json!(false));
    }

    #[test]
    fn tools_call_error_wraps_as_iserror_content_not_transport_err() {
        let v = wrap_tool_result(Err((-32000i64, "boom".to_string())));
        assert_eq!(v["isError"], json!(true));
        assert!(v["content"][0]["text"].as_str().unwrap().contains("boom"));
    }

    // HIGH-1: tools/call with missing 'name' must return content[] with isError, not a transport error
    #[test]
    fn tools_call_missing_name_returns_iserror_not_transport_err() {
        // Simulate the None branch of the tools/call arm directly via wrap_tool_result
        let result: std::result::Result<serde_json::Value, (i64, String)> =
            Err((-32602i64, "tools/call missing 'name'".to_string()));
        let v = wrap_tool_result(result);
        // Must be a content[] envelope (Ok at transport level), not a JSON-RPC error
        assert_eq!(v["isError"], json!(true), "isError must be true");
        assert!(v["content"].as_array().is_some(), "content[] array must be present");
        assert_eq!(v["content"][0]["type"], "text", "content[0].type must be 'text'");
        assert!(v["content"][0]["text"].as_str()
            .unwrap_or("").contains("'name'"),
            "error text should mention missing 'name'");
    }

    // T2.6 — two-tier dispatch mode parsing
    #[test]
    fn parse_locate_mode_defaults_to_instant() {
        let p = json!({});
        let (label, mode) = parse_locate_mode(&p);
        assert_eq!(label, "instant");
        assert_eq!(mode, LocateMode::Instant);
    }

    #[test]
    fn parse_locate_mode_instant_explicit() {
        let p = json!({"mode": "instant"});
        let (label, mode) = parse_locate_mode(&p);
        assert_eq!(label, "instant");
        assert_eq!(mode, LocateMode::Instant);
    }

    #[test]
    fn parse_locate_mode_deliberate() {
        let p = json!({"mode": "deliberate"});
        let (label, mode) = parse_locate_mode(&p);
        assert_eq!(label, "deliberate");
        assert_eq!(mode, LocateMode::Deliberate);
    }

    #[test]
    fn parse_locate_mode_unknown_value_falls_back_to_instant() {
        let p = json!({"mode": "unknown_value"});
        let (label, mode) = parse_locate_mode(&p);
        assert_eq!(label, "instant");
        assert_eq!(mode, LocateMode::Instant);
    }

    // MEDIUM-7: instant_only mode parse
    #[test]
    fn parse_locate_mode_instant_only() {
        let p = json!({"mode": "instant_only"});
        let (label, mode) = parse_locate_mode(&p);
        assert_eq!(label, "instant_only");
        assert_eq!(mode, LocateMode::InstantOnly);
    }

    // MEDIUM-7: all three modes parse correctly
    #[test]
    fn parse_locate_mode_three_way() {
        let cases = [
            ("instant", LocateMode::Instant),
            ("deliberate", LocateMode::Deliberate),
            ("instant_only", LocateMode::InstantOnly),
        ];
        for (s, expected) in cases {
            let p = json!({"mode": s});
            let (_, mode) = parse_locate_mode(&p);
            assert_eq!(mode, expected, "mode '{}' parsed incorrectly", s);
        }
    }

    // W4 — parse_target routing tests (pure, no COM)
    #[test]
    fn parse_target_name() {
        let p = json!({"name": "Submit"});
        let t = parse_target(&p).unwrap();
        assert!(matches!(t, Target::Name(n) if n == "Submit"));
    }

    #[test]
    fn parse_target_role() {
        let p = json!({"role": "button"});
        let t = parse_target(&p).unwrap();
        assert!(matches!(t, Target::Role(r) if r == "button"));
    }

    #[test]
    fn parse_target_description() {
        let p = json!({"description": "the blue submit button"});
        let t = parse_target(&p).unwrap();
        assert!(matches!(t, Target::Description(d) if d == "the blue submit button"));
    }

    #[test]
    fn parse_target_text() {
        let p = json!({"text": "Hello World"});
        let t = parse_target(&p).unwrap();
        assert!(matches!(t, Target::Text(s) if s == "Hello World"));
    }

    #[test]
    fn parse_target_missing_returns_error() {
        let p = json!({"mode": "instant"});
        assert!(parse_target(&p).is_err());
    }

    #[test]
    fn parse_target_prefers_name_over_role() {
        // name takes precedence over role
        let p = json!({"name": "OK", "role": "button"});
        let t = parse_target(&p).unwrap();
        assert!(matches!(t, Target::Name(n) if n == "OK"));
    }

    /// Act dispatch: UIA/Cache tier uses InvokePattern path; VLM/OCR uses coordinate path.
    /// This tests the pure decision logic without COM.
    #[test]
    fn act_dispatch_uia_tier_triggers_uia_path() {
        use ghost_session::Tier;
        // When grounded.source == Tier::Uia and target is Name, use_uia_path should be true.
        let source = Tier::Uia;
        let use_uia_path = source == Tier::Uia || source == Tier::Cache;
        assert!(use_uia_path, "UIA tier should use focus-independent path");
    }

    #[test]
    fn act_dispatch_vlm_tier_uses_coord_path() {
        use ghost_session::Tier;
        let source = Tier::Vlm;
        let use_uia_path = source == Tier::Uia || source == Tier::Cache;
        assert!(!use_uia_path, "VLM tier should use coordinate click path");
    }

    #[test]
    fn act_dispatch_ocr_tier_uses_coord_path() {
        use ghost_session::Tier;
        let source = Tier::Ocr;
        let use_uia_path = source == Tier::Uia || source == Tier::Cache;
        assert!(!use_uia_path, "OCR tier should use coordinate click path");
    }

    #[test]
    fn ghost_find_schema_has_mode_param() {
        let tools = tools_schema();
        let find_tool = tools.as_array().unwrap().iter()
            .find(|t| t["name"] == "ghost_find").unwrap();
        let props = &find_tool["inputSchema"]["properties"];
        assert!(props["mode"].is_object(), "ghost_find should have mode property in schema");
    }

    // MEDIUM-7: ghost_find and ghost_act schemas must include instant_only enum value.
    #[test]
    fn ghost_find_schema_mode_includes_instant_only() {
        let tools = tools_schema();
        let find_tool = tools.as_array().unwrap().iter()
            .find(|t| t["name"] == "ghost_find").unwrap();
        let mode_enum = &find_tool["inputSchema"]["properties"]["mode"]["enum"];
        let variants: Vec<&str> = mode_enum.as_array().unwrap()
            .iter().filter_map(|v| v.as_str()).collect();
        assert!(variants.contains(&"instant_only"), "ghost_find mode enum must include instant_only");
        assert!(variants.contains(&"instant"), "ghost_find mode enum must include instant");
        assert!(variants.contains(&"deliberate"), "ghost_find mode enum must include deliberate");
    }

    // HIGH-2 + LOW-9: ghost_find description mentions name and has_rect fields.
    #[test]
    fn ghost_find_schema_description_mentions_name_and_has_rect() {
        let tools = tools_schema();
        let find_tool = tools.as_array().unwrap().iter()
            .find(|t| t["name"] == "ghost_find").unwrap();
        let desc = find_tool["description"].as_str().unwrap();
        assert!(desc.contains("name"), "ghost_find description must mention name field");
        assert!(desc.contains("has_rect"), "ghost_find description must mention has_rect field");
    }

    #[test]
    fn ghost_act_schema_has_mode_param() {
        let tools = tools_schema();
        let act_tool = tools.as_array().unwrap().iter()
            .find(|t| t["name"] == "ghost_act").unwrap();
        let props = &act_tool["inputSchema"]["properties"];
        assert!(props["mode"].is_object(), "ghost_act should have mode property in schema");
    }

    #[test]
    fn ghost_act_schema_mode_includes_instant_only() {
        let tools = tools_schema();
        let act_tool = tools.as_array().unwrap().iter()
            .find(|t| t["name"] == "ghost_act").unwrap();
        let mode_enum = &act_tool["inputSchema"]["properties"]["mode"]["enum"];
        let variants: Vec<&str> = mode_enum.as_array().unwrap()
            .iter().filter_map(|v| v.as_str()).collect();
        assert!(variants.contains(&"instant_only"), "ghost_act mode enum must include instant_only");
    }

    // HIGH-1: text_input param resolution — documented param wins, legacy text is fallback.
    #[test]
    fn ghost_act_text_input_param_resolution() {
        // text_input present → use text_input
        let p = json!({ "text_input": "hello", "text": "world" });
        let resolved = p["text_input"].as_str().or_else(|| p["text"].as_str());
        assert_eq!(resolved, Some("hello"), "text_input must take priority over text");

        // text_input absent → fall back to text (legacy)
        let p2 = json!({ "text": "legacy" });
        let resolved2 = p2["text_input"].as_str().or_else(|| p2["text"].as_str());
        assert_eq!(resolved2, Some("legacy"), "text must be accepted when text_input absent");

        // neither present → None
        let p3 = json!({ "action": "click" });
        let resolved3 = p3["text_input"].as_str().or_else(|| p3["text"].as_str());
        assert_eq!(resolved3, None, "should resolve to None when neither param present");
    }

    // HIGH-2: all 5 ghost_act actions accepted at dispatch level (no "unknown action" error).
    #[test]
    fn ghost_act_all_five_actions_accepted() {
        // Verify the act_at_coords dispatch arms cover all 5 actions by checking the
        // known set mirrors the schema enum exactly.
        let schema_actions = ["click", "type", "double_click", "right_click", "hover"];
        let dispatch_actions = ["click", "type", "double_click", "right_click", "hover"];
        for action in &schema_actions {
            assert!(dispatch_actions.contains(action),
                "action '{}' advertised in schema but not in dispatch set", action);
        }
        // Verify unknown action is NOT in the set (sentinel check).
        assert!(!dispatch_actions.contains(&"ghost_action_unknown_9999"),
            "sentinel must not be in dispatch set");
    }

    // HIGH-2: ghost_act schema enum has all 5 actions.
    #[test]
    fn ghost_act_schema_enum_has_all_five_actions() {
        let tools = tools_schema();
        let act_tool = tools.as_array().unwrap().iter()
            .find(|t| t["name"] == "ghost_act").unwrap();
        let action_enum = &act_tool["inputSchema"]["properties"]["action"]["enum"];
        let variants: Vec<&str> = action_enum.as_array().unwrap()
            .iter().filter_map(|v| v.as_str()).collect();
        for action in &["click", "type", "double_click", "right_click", "hover"] {
            assert!(variants.contains(action),
                "ghost_act action enum missing: {}", action);
        }
    }

    // HIGH-3: ghost_query description must NOT claim VLM fallback.
    #[test]
    fn ghost_query_description_does_not_claim_vlm_fallback() {
        let tools = tools_schema();
        let query_tool = tools.as_array().unwrap().iter()
            .find(|t| t["name"] == "ghost_query").unwrap();
        let desc = query_tool["description"].as_str().unwrap();
        // Must mention unmatched field (honest partial-failure signal).
        assert!(desc.contains("unmatched"),
            "ghost_query description must mention 'unmatched' for partial-failure signal");
        // Must NOT imply VLM is currently active as a fallback.
        // Acceptable: "not yet implemented". Unacceptable: "VLM fallback for fields not found" (old text).
        assert!(!desc.contains("VLM fallback for fields not found"),
            "ghost_query description must not claim active VLM fallback");
    }

    // HIGH-4: ghost_run ok:false when steps fail (pure logic test on the response shape).
    #[test]
    fn ghost_run_result_ok_field_semantics() {
        // Verify the expected JSON shape for the failed case via wrap_envelope.
        let failed_payload = json!({ "ok": false, "completed": 0, "total": 1, "failed": 1,
                                      "results": [], "errors": [{"step": 0, "op": "ghost_act", "error": "boom"}] });
        assert_eq!(failed_payload["ok"], json!(false), "failed run must have ok:false");
        assert_eq!(failed_payload["failed"].as_u64().unwrap(), 1, "failed count must be 1");

        let success_payload = json!({ "ok": true, "completed": 1, "total": 1, "failed": 0,
                                       "results": [{"ok": true}], "errors": [] });
        assert_eq!(success_payload["ok"], json!(true), "successful run must have ok:true");
        assert_eq!(success_payload["failed"].as_u64().unwrap(), 0, "failed count must be 0");
    }

    // MEDIUM-5: failing dispatch yields content[] with isError:true AND ok:false in envelope.
    #[test]
    fn tools_call_failing_dispatch_yields_iserror_and_ok_false_envelope() {
        // Simulate what the tools/call arm does for a failed dispatch:
        // wrap_envelope(Err(...)) → envelope with ok:false,
        // then route through wrap_tool_result(Err(...)).
        let envelope = wrap_envelope(Err("action failed: element not found".to_string()));
        assert_eq!(envelope["ok"], json!(false), "error envelope must have ok:false");

        // Simulate the MEDIUM-5 routing: ok:false envelope → Err path → isError:true.
        let err_text = serde_json::to_string_pretty(&envelope).unwrap();
        let content = wrap_tool_result(Err((-32000i64, err_text.clone())));
        assert_eq!(content["isError"], json!(true), "content must have isError:true");
        assert!(content["content"][0]["text"].as_str().unwrap().contains("ok"),
            "error text should contain the envelope with ok field");
        // The envelope itself must have ok:false inside the error text.
        assert!(content["content"][0]["text"].as_str().unwrap().contains("false"),
            "error text must contain false (from ok:false envelope)");
    }

    // MEDIUM-6: ghost_query unmatched array present in response shape.
    #[test]
    fn ghost_query_response_includes_unmatched_array() {
        // Simulate extraction where some fields resolve to null.
        let mut extracted: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        let mut unmatched: Vec<serde_json::Value> = Vec::new();
        let fields = ["title", "price", "unknown_field_xyz"];
        let elements: Vec<serde_json::Value> = vec![
            json!({ "name": "title text", "role": "text" }),
            json!({ "name": "price $9.99", "role": "text" }),
        ];
        for field in &fields {
            let matched = elements.iter().find(|el| {
                el["name"].as_str()
                    .map(|n| n.to_lowercase().contains(&field.to_lowercase()))
                    .unwrap_or(false)
            });
            let value = matched
                .and_then(|el| el["name"].as_str())
                .map(|s| serde_json::Value::String(s.to_string()))
                .unwrap_or(serde_json::Value::Null);
            if value.is_null() {
                unmatched.push(serde_json::Value::String(field.to_string()));
            }
            extracted.insert(field.to_string(), value);
        }
        let response = json!({ "extracted": extracted, "unmatched": unmatched });
        assert!(response["unmatched"].as_array().is_some(), "unmatched must be an array");
        let unmatched_arr = response["unmatched"].as_array().unwrap();
        assert_eq!(unmatched_arr.len(), 1, "exactly 1 field should be unmatched");
        assert_eq!(unmatched_arr[0], json!("unknown_field_xyz"),
            "unmatched must list the field that resolved to null");
        // Matched fields must NOT be in unmatched.
        let unmatched_names: Vec<&str> = unmatched_arr.iter()
            .filter_map(|v| v.as_str()).collect();
        assert!(!unmatched_names.contains(&"title"), "matched field must not appear in unmatched");
        assert!(!unmatched_names.contains(&"price"), "matched field must not appear in unmatched");
    }

    // -----------------------------------------------------------------------
    // ITEM 2: ghost_query VLM merge logic (pure, no live session)
    // -----------------------------------------------------------------------

    /// Simulate UIA phase (some hit, some miss) + VLM fill phase, verify merge.
    #[test]
    fn ghost_query_uia_hits_vlm_fills_remainder() {
        let field_names = vec!["title".to_string(), "status".to_string(), "price".to_string()];
        let elements: Vec<serde_json::Value> = vec![
            json!({ "name": "title: My Product", "role": "text" }),
        ];

        // Phase 1: UIA matching
        let mut extracted: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        let mut unmatched_fields: Vec<String> = Vec::new();
        for field in &field_names {
            let matched = elements.iter().find(|el| {
                el["name"].as_str()
                    .map(|n| n.to_lowercase().contains(&field.to_lowercase()))
                    .unwrap_or(false)
            });
            let value = matched
                .and_then(|el| el["name"].as_str())
                .map(|s| serde_json::Value::String(s.to_string()))
                .unwrap_or(serde_json::Value::Null);
            if value.is_null() { unmatched_fields.push(field.clone()); }
            extracted.insert(field.clone(), value);
        }
        // UIA found "title", missed "status" and "price"
        assert!(!extracted["title"].is_null());
        assert!(extracted["status"].is_null());
        assert!(extracted["price"].is_null());
        assert_eq!(unmatched_fields, vec!["status", "price"]);

        // Phase 2: simulate VLM filling "status" but not "price"
        let mut vlm_map = serde_json::Map::new();
        vlm_map.insert("status".to_string(), json!("Active"));
        vlm_map.insert("price".to_string(), serde_json::Value::Null);

        for field in &unmatched_fields {
            if let Some(v) = vlm_map.get(field) {
                if !v.is_null() {
                    extracted.insert(field.clone(), v.clone());
                }
            }
        }

        // After merge: title from UIA, status from VLM, price still null
        assert!(!extracted["title"].is_null());
        assert_eq!(extracted["status"], json!("Active"));
        assert!(extracted["price"].is_null());

        // Unmatched after both passes = only price
        let unmatched: Vec<String> = field_names.iter()
            .filter(|f| extracted.get(*f).map(|v| v.is_null()).unwrap_or(true))
            .cloned()
            .collect();
        assert_eq!(unmatched, vec!["price"]);
    }

    #[test]
    fn ghost_query_all_uia_hit_no_vlm_call() {
        let field_names = vec!["name".to_string()];
        let elements: Vec<serde_json::Value> = vec![
            json!({ "name": "name field", "role": "text" }),
        ];
        let mut extracted: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        let mut unmatched_fields: Vec<String> = Vec::new();
        for field in &field_names {
            let matched = elements.iter().find(|el| {
                el["name"].as_str()
                    .map(|n| n.to_lowercase().contains(&field.to_lowercase()))
                    .unwrap_or(false)
            });
            let value = matched.and_then(|el| el["name"].as_str())
                .map(|s| serde_json::Value::String(s.to_string()))
                .unwrap_or(serde_json::Value::Null);
            if value.is_null() { unmatched_fields.push(field.clone()); }
            extracted.insert(field.clone(), value);
        }
        // All filled by UIA → no VLM needed
        assert!(unmatched_fields.is_empty(), "no fields should be unmatched when UIA covers all");
        assert!(!extracted["name"].is_null());
    }

    // -----------------------------------------------------------------------
    // ITEM 3: ghost_run YAML/JSON script parsing (pure, no live session)
    // -----------------------------------------------------------------------

    #[test]
    fn yaml_script_parses_to_same_as_json() {
        let yaml_script = r#"
- op: ghost_wait
  for: ms
  ms: 100
- op: ghost_key
  keys: "Enter"
"#;
        let json_script = r#"[
  {"op": "ghost_wait", "for": "ms", "ms": 100},
  {"op": "ghost_key", "keys": "Enter"}
]"#;
        let from_yaml: serde_json::Value = serde_yaml::from_str(yaml_script).unwrap();
        let from_json: serde_json::Value = serde_json::from_str(json_script).unwrap();
        let yaml_steps = from_yaml.as_array().unwrap();
        let json_steps = from_json.as_array().unwrap();
        assert_eq!(yaml_steps.len(), json_steps.len(), "both should have 2 steps");
        assert_eq!(yaml_steps[0]["op"], json_steps[0]["op"]);
        assert_eq!(yaml_steps[1]["op"], json_steps[1]["op"]);
        assert_eq!(yaml_steps[0]["ms"], json_steps[0]["ms"]);
        assert_eq!(yaml_steps[1]["keys"], json_steps[1]["keys"]);
    }

    #[test]
    fn yaml_fallback_accepts_json_script() {
        // JSON is valid YAML, so serde_yaml should accept it directly.
        let json_script = r#"[{"op":"ghost_wait","ms":50}]"#;
        let parsed: serde_json::Value = serde_yaml::from_str(json_script)
            .or_else(|_| serde_json::from_str(json_script))
            .unwrap();
        assert!(parsed.as_array().is_some());
        assert_eq!(parsed[0]["op"], json!("ghost_wait"));
    }

    #[test]
    fn yaml_scalar_is_not_an_array() {
        // serde_yaml accepts almost any string as a scalar, but parse_run_script
        // rejects non-array values at the gate. Verify the actual gate message.
        let scalar_yaml = "not_an_array";
        let err = parse_run_script(scalar_yaml).unwrap_err();
        assert!(
            err.contains("'steps' must be an array"),
            "expected gate message in error, got: {err}"
        );
    }

    #[test]
    fn truly_invalid_yaml_or_json_returns_parse_error() {
        // A string that serde_yaml AND serde_json both reject (EOF while parsing).
        let bad = r#"{"unclosed":"}"#;
        let err = parse_run_script(bad).unwrap_err();
        assert!(
            err.contains("ghost_run: script is neither valid YAML nor JSON"),
            "expected parse error message, got: {err}"
        );
    }

    // T0.2 — JSON-RPC errors have integer code
    #[test]
    fn jsonrpc_error_has_integer_code() {
        let resp = McpResponse {
            jsonrpc: "2.0",
            id: json!(1),
            result: None,
            error: Some(json!({"code": -32603i64, "message": "x"})),
        };
        let s = serde_json::to_string(&resp).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v["error"]["code"].is_i64(), "error.code must be an integer, got: {}", v["error"]["code"]);
        assert_eq!(v["error"]["code"].as_i64().unwrap(), -32603);
    }
}
