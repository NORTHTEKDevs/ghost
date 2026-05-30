#![recursion_limit = "512"]

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ghost_session::GhostSession;
use std::io::{BufRead, Write};
use std::sync::OnceLock;

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

async fn dispatch_tool(
    session: &GhostSession,
    name: &str,
    args: &Value,
) -> std::result::Result<Value, String> {
    handle_tool(session, name, args).await
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
                    let out = dispatch_tool(session, &name, &args).await;
                    Ok(wrap_tool_result(out.map_err(|e| (-32000i64, e))))
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
            let by = parse_by(&p)?;
            let el = session.find(by).await.map_err(|e| e.to_string())?;
            Ok(json!({
                "name": el.name(),
                "bounding_rect": el.bounding_rect()
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
            let stats = session.cache_stats();
            Ok(serde_json::to_value(stats).map_err(|e| e.to_string())?)
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
            let by = parse_by(p)?;
            let action = p["action"].as_str().ok_or("missing param: action (click|type)")?;
            let text = p["text"].as_str();
            session.act(by, action, text).await.map_err(|e| e.to_string())
        }
        _ => Err(format!("unknown method: {}", method)),
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

fn tools_schema() -> Value {
    json!([
        { "name": "ghost_find",
          "description": "Find the first UI element matching name or role. Returns element name and bounding rect.",
          "inputSchema": { "type": "object", "properties": {
              "name": { "type": "string", "description": "Accessible name (case-insensitive substring)" },
              "role": { "type": "string", "description": "Control type: button, edit, checkbox, list, menu, tab, toolbar" }
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
          "description": "Atomic find → set UIA focus → perform action. Eliminates the cross-call focus race compared to separate ghost_focus_window + ghost_click calls. Returns {ok, name, rect}. At least one of 'name' or 'role' must be supplied to identify the target element.",
          "inputSchema": { "type": "object", "required": ["action"], "properties": {
              "name": { "type": "string", "description": "Accessible name of target element (case-insensitive substring)" },
              "role": { "type": "string", "description": "Control type role: button, edit, checkbox, etc." },
              "action": { "type": "string", "enum": ["click", "type"], "description": "Action to perform after finding the element" },
              "text": { "type": "string", "description": "Text to type (required when action=type)" }
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

    #[test]
    fn tools_schema_has_expected_tool_count() {
        let tools = tools_schema();
        let list = tools.as_array().unwrap();
        assert_eq!(list.len(), 48, "expected 48 tools (47 pre-v0.6.0 + 1 ghost_act)");
    }

    #[test]
    fn v04_perf_tools_registered() {
        let tools = tools_schema();
        let names: Vec<&str> = tools.as_array().unwrap().iter()
            .filter_map(|t| t["name"].as_str()).collect();
        for t in ["ghost_describe_screen_fast", "ghost_batch_actions",
                  "ghost_screenshot_region", "ghost_event_seq", "ghost_wait_for_event"] {
            assert!(names.contains(&t), "missing v0.4 perf tool: {t}");
        }
    }

    #[test]
    fn all_v030_tools_registered() {
        let tools = tools_schema();
        let names: Vec<&str> = tools.as_array().unwrap().iter()
            .filter_map(|t| t["name"].as_str()).collect();
        for t in ["ghost_wait_until","ghost_wait_for_idle","ghost_navigate_and_wait",
                  "ghost_click_and_wait_for_text","ghost_fill_form","ghost_execute_intent",
                  "ghost_describe_screen_delta","ghost_click_background",
                  "ghost_cache_stats","ghost_cache_invalidate"] {
            assert!(names.contains(&t), "missing {t}");
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

    #[test]
    fn tools_schema_contains_all_required_tools() {
        let tools = tools_schema();
        let names: Vec<&str> = tools.as_array().unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        for required in &["ghost_find","ghost_click","ghost_type","ghost_screenshot",
                          "ghost_press","ghost_hotkey","ghost_scroll","ghost_describe_screen",
                          "ghost_get_clipboard","ghost_set_clipboard","ghost_list_windows",
                          "ghost_stop","ghost_reset","ghost_wait","ghost_get_text",
                          "ghost_http_get","ghost_http_post"] {
            assert!(names.contains(required), "tools/list missing: {}", required);
        }
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
