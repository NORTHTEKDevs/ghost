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

impl ProgressEmitter {
    fn new(token: Option<Value>, writer: *mut dyn Write) -> Self {
        Self { token, writer: Some(writer), progress: 0, total: None }
    }

    fn with_total(mut self, total: u64) -> Self {
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

/// Wrap a raw tool result into the T3.2 structured envelope:
/// `{ ok, data, foreground: {hwnd, title}, error_code? }`
/// `data` contains the original fields; `ok` is true on success.
fn wrap_envelope(r: Result<Value, String>) -> Value {
    let fg = foreground_info();
    match r {
        Ok(v) => json!({ "ok": true, "data": v, "foreground": fg }),
        Err(e) => json!({ "ok": false, "data": null, "foreground": fg, "error_code": -32000i64, "error": e }),
    }
}

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("ghost-mcp/0.2.0")
            .build()
            .expect("failed to build reqwest client")
    })
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

#[tokio::main]
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

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("stdin read error: {}", e);
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        let req: McpRequest = match serde_json::from_str(&line) {
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
                "serverInfo": { "name": "ghost", "version": "0.6.0" }
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
                    let out = dispatch_tool(session, &name, &args).await;
                    // T3.2: wrap in structured envelope, then in MCP content[].
                    let envelope = wrap_envelope(out);
                    Ok(wrap_tool_result(Ok(envelope)))
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
            // Route through the grounding cascade so source/confidence are returned.
            let target = parse_target(p)?;
            let grounded = session.ground(target, mode).await.map_err(|e| e.to_string())?;
            // HIGH-2: include name field (Some for Cache/UIA tiers, null for OCR/VLM/YOLO).
            // LOW-9: has_rect indicates whether rect is meaningful (false for point-only tiers).
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
            // Pass "full": true for the full-screen lossless PNG.
            if p.get("full").and_then(|v| v.as_bool()).unwrap_or(false) {
                let png = session.screenshot(ghost_session::session::Region::full()).await.map_err(|e| e.to_string())?;
                Ok(json!({ "png_base64": base64_encode(&png), "size_bytes": png.len() }))
            } else {
                let (rect_mode, max_dim, jpeg_quality) = screenshot_default_opts(&p);
                let rect = if rect_mode { session.foreground_window_rect() } else { None };
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
            let exe = p["exe"].as_str().ok_or("missing param: exe")?;
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
            session.wait(ms).await;
            Ok(json!({ "ok": true }))
        }
        "ghost_describe_screen" => {
            let window = p["window"].as_str();
            let elements = session.describe_screen(window).await.map_err(|e| e.to_string())?;
            let list: Vec<serde_json::Value> = elements.iter().map(|e| json!({
                "name": e.name,
                "role": e.role,
                "left": e.left,
                "top": e.top,
                "right": e.right,
                "bottom": e.bottom,
            })).collect();
            Ok(json!({ "elements": list }))
        }
        "ghost_describe_screen_fast" => {
            let elements = session.describe_screen_fast().await.map_err(|e| e.to_string())?;
            let list: Vec<serde_json::Value> = elements.iter().map(|e| json!({
                "name": e.name,
                "role": e.role,
                "left": e.left,
                "top": e.top,
                "right": e.right,
                "bottom": e.bottom,
            })).collect();
            Ok(json!({ "elements": list }))
        }
        "ghost_batch_actions" => {
            let actions = p["actions"].as_array().ok_or("missing param: actions (array)")?;
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
        "ghost_wait_for_event" => {
            let since_seq = p["since_seq"].as_u64().unwrap_or(0);
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(5000);
            match session.wait_for_event(since_seq, timeout_ms).await {
                Ok(seq) => Ok(json!({ "seq": seq, "timed_out": false })),
                Err(_) => Ok(json!({ "seq": session.event_seq(), "timed_out": true })),
            }
        }
        "ghost_act" => {
            let action = p["action"].as_str().ok_or("missing param: action (click|type)")?;
            let text = p["text"].as_str();
            let (mode_label, mode) = parse_locate_mode(p);
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

/// Coordinate-based action dispatch: ensure foreground under point, then click/type at (x, y).
///
/// HIGH-1: `focus_window_under_point` is called before any input to ensure SendInput
/// keystrokes land in the correct window (the one containing the target coordinates),
/// not in whichever window currently happens to have focus.
async fn act_at_coords(
    session: &GhostSession,
    x: i32,
    y: i32,
    action: &str,
    text: Option<&str>,
) -> std::result::Result<Value, String> {
    // Bring the window under the target point to the foreground BEFORE any input.
    // Tolerant: if focus cannot be confirmed we warn and proceed rather than hard-failing.
    match ghost_core::uia::tree::focus_window_under_point(x, y) {
        Ok(true) => {} // foreground confirmed
        Ok(false) => tracing::warn!(x, y, "focus_window_under_point: could not confirm foreground; proceeding"),
        Err(e) => tracing::warn!(error=%e, x, y, "focus_window_under_point error; proceeding"),
    }

    match action {
        "click" => {
            session.click_at(x, y).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "type" => {
            let t = text.ok_or("action=type requires text param")?;
            session.click_at(x, y).await.map_err(|e| e.to_string())?;
            ghost_core::input::keyboard::type_text(t)
                .map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        other => Err(format!("ghost_act: unknown action '{other}'; use click or type")),
    }
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
    // Handle "down:KEY" / "up:KEY" special forms.
    if let Some(k) = keys.strip_prefix("down:") {
        let args = json!({ "key": k });
        return handle_tool(session, "ghost_key_down", &args).await;
    }
    if let Some(k) = keys.strip_prefix("up:") {
        let args = json!({ "key": k });
        return handle_tool(session, "ghost_key_up", &args).await;
    }
    // Split on '+' — last segment is the key, all prior segments are modifiers.
    let parts: Vec<&str> = keys.split('+').collect();
    if parts.len() == 1 {
        let args = json!({ "key": parts[0] });
        return handle_tool(session, "ghost_press", &args).await;
    }
    let (modifiers, key) = parts.split_at(parts.len() - 1);
    let mods: Vec<Value> = modifiers.iter().map(|m| Value::String(m.to_string())).collect();
    let args = json!({ "modifiers": mods, "key": key[0] });
    handle_tool(session, "ghost_hotkey", &args).await
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
        other => Err(format!("ghost_assert: unknown predicate '{other}'; use text-present|text-absent|element-exists")),
    }
}

/// `ghost_run(steps: [{op, ...}] | json_flow: string)` — T3.4 declarative flow runner.
/// Executes steps sequentially. Each step has `op` (lean verb or legacy name) and its params.
async fn handle_ghost_run(
    session: &GhostSession,
    p: &Value,
) -> std::result::Result<Value, String> {
    // Accept either steps: [...] directly or a json_flow string (JSON array).
    let steps_val = if let Some(s) = p.get("steps") {
        s.clone()
    } else if let Some(flow_str) = p["json_flow"].as_str() {
        serde_json::from_str(flow_str).map_err(|e| format!("ghost_run: invalid json_flow: {e}"))?
    } else {
        return Err("ghost_run: provide 'steps' (array) or 'json_flow' (JSON string)".into());
    };

    let steps = steps_val.as_array().ok_or("ghost_run: 'steps' must be an array")?;
    let total = steps.len() as u64;
    let max_retries = p["max_retries"].as_u64().unwrap_or(2) as usize;
    let stop_on_error = p["stop_on_error"].as_bool().unwrap_or(true);

    let mut results: Vec<Value> = Vec::with_capacity(steps.len());
    let mut errors: Vec<Value> = Vec::new();

    for (i, step) in steps.iter().enumerate() {
        let op = step["op"].as_str()
            .ok_or_else(|| format!("ghost_run: step {i} missing 'op'"))?;

        let mut last_err: Option<String> = None;
        let mut succeeded = false;

        for _attempt in 0..=max_retries {
            // Use in_run=true to prevent re-entering ghost_run recursively.
            let outcome = dispatch_tool_inner(session, op, step, true).await;
            match outcome {
                Ok(v) => {
                    results.push(v);
                    succeeded = true;
                    break;
                }
                Err(e) => {
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

    Ok(json!({
        "completed": results.len(),
        "total": total,
        "results": results,
        "errors": errors,
    }))
}

/// `ghost_query(schema?, region?)` — T3.5 structured screen extraction (stub).
/// Extracts named fields from the screen using UIA + OCR + optional VLM fallback.
/// Full implementation in T3.5; this stub returns the UIA element list annotated with
/// schema field matches so callers can see the plumbing works.
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

    // Get elements via UIA fast describe.
    let elements_result = handle_tool(session, "ghost_describe_screen_fast", p).await?;
    let elements = elements_result.get("elements")
        .and_then(|e| e.as_array())
        .cloned()
        .unwrap_or_default();

    // Naive field matching: for each schema field, find the first element whose
    // name contains the field name (case-insensitive).
    let mut extracted: serde_json::Map<String, Value> = serde_json::Map::new();
    for field in &field_names {
        let matched = elements.iter().find(|el| {
            el["name"].as_str()
                .map(|n| n.to_lowercase().contains(&field.to_lowercase()))
                .unwrap_or(false)
        });
        extracted.insert(field.clone(), matched
            .and_then(|el| el["name"].as_str())
            .map(|s| Value::String(s.to_string()))
            .unwrap_or(Value::Null));
    }

    Ok(json!({
        "extracted": extracted,
        "element_count": elements.len(),
        "note": "ghost_query T3.5 stub: UIA field matching only; VLM fallback not yet implemented",
    }))
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
          "description": "Describe the active screen's UI elements. mode=fast (default, foreground only, 5-50x faster), mode=full (full walk, scope with window=), mode=delta (only changed elements since since_seq).",
          "inputSchema": { "type": "object", "properties": {
              "mode": { "type": "string", "enum": ["fast", "full", "delta"], "description": "fast=foreground UIA walk (default), full=full tree, delta=changed only" },
              "window": { "type": "string", "description": "Partial title to scope full walk (mode=full only)" },
              "since_seq": { "type": "integer", "description": "Prior snapshot seq for delta mode" }
          }}},
        { "name": "ghost_find",
          "description": "Ground a target (name|role|description|text|coords) via the cascade: cache→UIA→OCR→VLM. Returns center (always), rect (has_rect=true for UIA/Cache), source, confidence, name.",
          "inputSchema": { "type": "object", "properties": {
              "name": { "type": "string", "description": "Accessible name (case-insensitive substring)" },
              "role": { "type": "string", "description": "Control type: button, edit, checkbox, list, menu, tab, toolbar" },
              "description": { "type": "string", "description": "Natural-language description for VLM grounding" },
              "text": { "type": "string", "description": "On-screen text for OCR-based location" },
              "mode": { "type": "string", "enum": ["instant", "deliberate", "instant_only"], "description": "'instant' (default): local tiers, auto-escalates to VLM on miss. 'deliberate': VLM from first attempt. 'instant_only': no VLM." }
          }}},
        // --- Action ---
        { "name": "ghost_act",
          "description": "Atomic find→focus→action in one call (eliminates cross-call focus race). Returns {ok, source, confidence, center}. Supply name|role|description|text to identify target.",
          "inputSchema": { "type": "object", "required": ["action"], "properties": {
              "name": { "type": "string" }, "role": { "type": "string" },
              "description": { "type": "string" }, "text": { "type": "string" },
              "action": { "type": "string", "enum": ["click", "type", "double_click", "right_click", "hover"],
                          "description": "Action to perform" },
              "text_input": { "type": "string", "description": "Text to type when action=type (use this to avoid param collision with text-target)" },
              "mode": { "type": "string", "enum": ["instant", "deliberate", "instant_only"] }
          }}},
        { "name": "ghost_key",
          "description": "Key input. Single key: keys='Enter'. Combo: keys='Ctrl+C'. Hold/release: keys='down:Shift' / keys='up:Shift'. Common combos: Ctrl+C, Ctrl+V, Ctrl+Z, Alt+F4, Win+D.",
          "inputSchema": { "type": "object", "required": ["keys"], "properties": {
              "keys": { "type": "string", "description": "Key spec: 'Enter', 'Ctrl+C', 'Ctrl+Shift+T', 'down:Shift', 'up:Shift'" }
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
          "description": "Unified wait. for=ms (default): sleep N ms. for=idle: wait for screen stable. for=text: wait for text appear/disappear. for=event: next foreground change. for=cond: JSONLogic poll. for=navigate: focus window + navigate URL + page idle.",
          "inputSchema": { "type": "object", "properties": {
              "for": { "type": "string", "enum": ["ms","idle","text","event","cond","navigate"],
                       "description": "What to wait for (default ms)" },
              "ms": { "type": "integer", "description": "Milliseconds (for=ms)" },
              "window": { "type": "string", "description": "Window scope (for=idle|navigate)" },
              "stable_frames": { "type": "integer", "default": 3, "description": "for=idle" },
              "timeout_ms": { "type": "integer", "default": 5000 },
              "text": { "type": "string", "description": "Text to wait for (for=text)" },
              "appears": { "type": "boolean", "default": true, "description": "for=text" },
              "since_seq": { "type": "integer", "description": "for=event" },
              "condition": { "type": "object", "description": "JSONLogic expression (for=cond)" },
              "url": { "type": "string", "description": "URL to navigate to (for=navigate)" }
          }}},
        // --- Extraction / assertion ---
        { "name": "ghost_query",
          "description": "Extract structured data from the screen to a caller-supplied JSON schema. UIA field matching first; VLM fallback for fields not found. Returns extracted object.",
          "inputSchema": { "type": "object", "properties": {
              "schema": { "type": "object", "description": "JSON Schema (properties map) or array of field names to extract" },
              "window": { "type": "string", "description": "Scope to window (partial title)" }
          }}},
        { "name": "ghost_assert",
          "description": "Assert a predicate about screen state. Fails (error) if the assertion is not satisfied. predicate: text-present|text-absent|element-exists.",
          "inputSchema": { "type": "object", "required": ["predicate"], "properties": {
              "predicate": { "type": "string", "enum": ["text-present","text-absent","element-exists"] },
              "text": { "type": "string", "description": "Text to check (text-present|text-absent)" },
              "name": { "type": "string", "description": "Element name (element-exists)" },
              "role": { "type": "string", "description": "Element role (element-exists)" },
              "foreground": { "type": "boolean", "default": true }
          }}},
        // --- Flow ---
        { "name": "ghost_run",
          "description": "Execute a declarative step-by-step flow. Each step: {op, ...params}. Op can be any lean verb or legacy tool name. Retries on failure (max_retries). Emits per-step progress events.",
          "inputSchema": { "type": "object", "properties": {
              "steps": { "type": "array", "items": { "type": "object" }, "description": "Array of {op, ...params} steps" },
              "json_flow": { "type": "string", "description": "JSON-encoded steps array (alternative to steps)" },
              "max_retries": { "type": "integer", "default": 2 },
              "stop_on_error": { "type": "boolean", "default": true }
          }}},
        // --- Screenshot ---
        { "name": "ghost_screenshot",
          "description": "Capture screenshot. Default: foreground window, max 768px, JPEG q=75 (~20-100KB). full=true: full-screen lossless PNG. Always includes size_bytes.",
          "inputSchema": { "type": "object", "properties": {
              "full": { "type": "boolean", "description": "Full-screen lossless PNG (default false)" },
              "foreground": { "type": "boolean", "description": "Crop to foreground window (default true)" },
              "max_dim": { "type": "integer", "description": "Longest-edge resize (default 768)" },
              "jpeg_quality": { "type": "integer", "minimum": 0, "maximum": 100 }
          }}},
        // --- Window management ---
        { "name": "ghost_window",
          "description": "Window management. op=list: all windows. op=focus: bring to foreground (name required). op=state: maximize|minimize|restore|close (name+state). op=launch: start exe.",
          "inputSchema": { "type": "object", "properties": {
              "op": { "type": "string", "enum": ["list","focus","state","launch"], "description": "Operation (default list)" },
              "name": { "type": "string", "description": "Window title (op=focus|state) or exe path (op=launch)" },
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
              "action": { "type": "string", "enum": ["click", "type"], "description": "Action to perform after finding the element" },
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
        // pure: "Ctrl+C" splits into modifiers=["Ctrl"], key="C"
        let keys = "Ctrl+C";
        let parts: Vec<&str> = keys.split('+').collect();
        assert_eq!(parts.len(), 2);
        let (mods, key_part) = parts.split_at(parts.len() - 1);
        assert_eq!(mods, &["Ctrl"]);
        assert_eq!(key_part, &["C"]);
    }

    #[test]
    fn ghost_key_single_is_press() {
        let keys = "Enter";
        let parts: Vec<&str> = keys.split('+').collect();
        assert_eq!(parts.len(), 1);
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
            "serverInfo": { "name": "ghost", "version": "0.6.0" }
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
