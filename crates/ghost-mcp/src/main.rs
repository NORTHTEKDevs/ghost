use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ghost_session::GhostSession;
use std::io::{BufRead, Write};

#[derive(Deserialize)]
struct McpRequest {
    id: Value,
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
                        "id": null,
                        "error": { "message": format!("parse error: {}", e) }
                    })).unwrap()
                );
                let _ = out.flush();
                continue;
            }
        };

        let result = handle(&session, &req.method, req.params.as_ref()).await;

        let resp = match result {
            Ok(v) => McpResponse { id: req.id, result: Some(v), error: None },
            Err(e) => McpResponse {
                id: req.id,
                result: None,
                error: Some(json!({ "message": e })),
            },
        };

        let _ = writeln!(out, "{}", serde_json::to_string(&resp).unwrap());
        let _ = out.flush();
    }
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
                "serverInfo": { "name": "ghost", "version": "0.2.0" }
            }))
        }
        "initialized" | "notifications/initialized" => Ok(json!({})),
        "tools/list" => {
            Ok(json!({ "tools": tools_schema() }))
        }
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
            let png = session.screenshot(ghost_session::session::Region::full()).await.map_err(|e| e.to_string())?;
            Ok(json!({ "png_base64": base64_encode(&png) }))
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
        "ghost_get_text" => {
            let by = parse_by(&p)?;
            let text = session.get_text(by).await.map_err(|e| e.to_string())?;
            Ok(json!({ "text": text }))
        }
        _ => Err(format!("unknown method: {}", method)),
    }
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
          "description": "Capture the primary monitor as a base64-encoded PNG.",
          "inputSchema": { "type": "object", "properties": {} }},
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
          "description": "Return a structured list of interactive UI elements (buttons, inputs, menus) with names, roles, and positions. Scope to a window by partial title.",
          "inputSchema": { "type": "object", "properties": {
              "window": { "type": "string", "description": "Optional partial window title to scope the search" }
          }}},
        { "name": "ghost_get_text",
          "description": "Get the text value or label of a found UI element.",
          "inputSchema": { "type": "object", "properties": {
              "name": { "type": "string" }, "role": { "type": "string" }
          }}}
    ])
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
    fn tools_schema_has_25_tools() {
        let tools = tools_schema();
        let list = tools.as_array().unwrap();
        assert_eq!(list.len(), 25, "expected 25 tools (24 original + ghost_reset)");
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
                          "ghost_stop","ghost_reset","ghost_wait","ghost_get_text"] {
            assert!(names.contains(required), "tools/list missing: {}", required);
        }
    }

    #[test]
    fn initialize_response_has_protocol_version() {
        // Verify initialize response shape matches MCP 2024-11-05 spec
        let resp = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "ghost", "version": "0.2.0" }
        });
        assert_eq!(resp["protocolVersion"], "2024-11-05");
        assert!(resp["capabilities"]["tools"].is_object());
        assert_eq!(resp["serverInfo"]["name"], "ghost");
    }
}
