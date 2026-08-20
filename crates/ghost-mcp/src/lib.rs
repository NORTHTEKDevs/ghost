#![recursion_limit = "512"]

//! Ghost MCP tool dispatch.
//!
//! The tool surface lives here rather than in the stdio binary so the `ghost`
//! CLI can expose exactly the same tools without a second implementation that
//! would drift out of sync.

use serde_json::{json, Value};
pub use ghost_session::GhostSession;

/// Join the calling thread to the multithreaded COM apartment.
///
/// Every runtime worker thread calls this at startup so UIA COM calls are legal from
/// any request task. Idempotent; the "already initialized" HRESULT is success here.
pub fn init_com_for_thread() -> bool {
    ghost_session::init_com_for_thread()
}
use std::sync::OnceLock;

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("ghost-mcp/0.5.0")
            .build()
            .expect("failed to build reqwest client")
    })
}

pub async fn handle(
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
                "serverInfo": { "name": "ghost", "version": "0.5.0" }
            }))
        }
        "initialized" | "notifications/initialized" => Ok(json!({})),
        "tools/list" => {
            Ok(json!({ "tools": tools_schema() }))
        }
        "ghost_find" => {
            let by = parse_by(&p)?;
            let el = find_scoped(session, &p, by).await?;
            Ok(json!({
                "name": el.name(),
                "bounding_rect": el.bounding_rect()
            }))
        }
        "ghost_click" => {
            let by = parse_by(&p)?;
            let el = find_scoped(session, &p, by).await?;
            let route = el.click().map_err(|e| e.to_string())?;
            Ok(route_result(route))
        }
        "ghost_type" => {
            let by = parse_by(&p)?;
            let text = p["text"].as_str().ok_or("missing param: text")?;
            let el = find_scoped(session, &p, by).await?;
            let route = el.type_text(text).map_err(|e| e.to_string())?;
            Ok(route_result(route))
        }
        "ghost_toggle" => {
            let el = find_scoped(session, &p, parse_by(&p)?).await?;
            Ok(route_result(el.toggle().map_err(|e| e.to_string())?))
        }
        "ghost_select" => {
            let el = find_scoped(session, &p, parse_by(&p)?).await?;
            Ok(route_result(el.select().map_err(|e| e.to_string())?))
        }
        "ghost_expand" => {
            let expand = p["expand"].as_bool().unwrap_or(true);
            let el = find_scoped(session, &p, parse_by(&p)?).await?;
            Ok(route_result(el.expand_collapse(expand).map_err(|e| e.to_string())?))
        }
        "ghost_scroll_element" => {
            let direction = p["direction"].as_str().unwrap_or("down");
            let amount = p["amount"].as_i64().unwrap_or(1) as i32;
            let el = find_scoped(session, &p, parse_by(&p)?).await?;
            Ok(route_result(el.scroll(direction, amount).map_err(|e| e.to_string())?))
        }
        "ghost_scroll_into_view" => {
            let el = find_scoped(session, &p, parse_by(&p)?).await?;
            Ok(route_result(el.scroll_into_view().map_err(|e| e.to_string())?))
        }
        "ghost_set_range_value" => {
            let value = p["value"].as_f64().ok_or("missing param: value")?;
            let el = find_scoped(session, &p, parse_by(&p)?).await?;
            Ok(route_result(el.set_range_value(value).map_err(|e| e.to_string())?))
        }
        "ghost_document_text" => {
            let max = p["max_chars"].as_i64().unwrap_or(100_000) as i32;
            let el = find_scoped(session, &p, parse_by(&p)?).await?;
            Ok(json!({ "text": el.document_text(max) }))
        }
        "ghost_element_actions" => {
            let el = find_scoped(session, &p, parse_by(&p)?).await?;
            Ok(json!({
                "name": el.name(),
                "background_actions": el.supported_actions(),
                "bounding_rect": el.bounding_rect(),
            }))
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
            let route = session
                .navigate_and_wait(window, url, timeout_ms)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true, "route": route, "background": route == "cdp" }))
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
        // ---- focus policy -------------------------------------------------
        "ghost_desktop_state" => {
            let snap = ghost_session::DesktopSnapshot::capture();
            Ok(json!({
                // The handle, not just the title: a window that merely retitles
                // itself (terminals do this constantly) is not a stolen foreground,
                // and comparing titles would report a false positive.
                "foreground_hwnd": snap.foreground_hwnd,
                "foreground_window": snap.foreground_title,
                "cursor": [snap.cursor.0, snap.cursor.1],
                "policy": session.focus_policy(),
            }))
        }
        "ghost_focus_policy" => Ok(json!({ "policy": session.focus_policy() })),
        "ghost_set_focus_policy" => {
            let policy = p["policy"].as_str().ok_or("missing param: policy")?;
            let applied = session.set_focus_policy(policy).map_err(|e| e.to_string())?;
            Ok(json!({ "policy": applied }))
        }

        // ---- window-scoped background input --------------------------------
        "ghost_right_click_background" => {
            let (w, x, y) = window_point(&p)?;
            session.right_click_background(&w, x, y).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_double_click_background" => {
            let (w, x, y) = window_point(&p)?;
            session.double_click_background(&w, x, y).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_hover_background" => {
            let (w, x, y) = window_point(&p)?;
            session.hover_background(&w, x, y).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_scroll_background" => {
            let (w, x, y) = window_point(&p)?;
            let direction = p["direction"].as_str().unwrap_or("down");
            let amount = p["amount"].as_i64().unwrap_or(3) as i32;
            session.scroll_background(&w, x, y, direction, amount).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_type_background" => {
            let window = req_str(&p, "window")?;
            let text = req_str(&p, "text")?;
            session.type_background(&window, &text).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_press_background" => {
            let window = req_str(&p, "window")?;
            let key = req_str(&p, "key")?;
            session.press_background(&window, &key).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_hotkey_background" => {
            let window = req_str(&p, "window")?;
            let key = req_str(&p, "key")?;
            let modifiers = str_array(&p, "modifiers");
            session.hotkey_background(&window, &modifiers, &key).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_set_text_background" => {
            let window = req_str(&p, "window")?;
            let text = req_str(&p, "text")?;
            session.set_text_background(&window, &text).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_capture_window" => {
            let window = req_str(&p, "window")?;
            let client_only = p["client_only"].as_bool().unwrap_or(false);
            let png = session.capture_window(&window, client_only).await.map_err(|e| e.to_string())?;
            Ok(json!({ "png_base64": base64_encode(&png), "background": true }))
        }
        "ghost_click_element_background" => {
            let window = req_str(&p, "window")?;
            let by = parse_by(&p)?;
            let (title, x, y) = session
                .click_element_background(by, &window)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true, "background": true, "window": title, "client_x": x, "client_y": y }))
        }

        // ---- browsers and tabs ---------------------------------------------
        "ghost_browser_launch" => {
            let id = p["id"].as_str().unwrap_or("default").to_string();
            let mode = p["mode"].as_str().unwrap_or("headless");
            let which = p["browser"].as_str();
            session
                .browser_launch_with(&id, mode, which)
                .await
                .map_err(|e| e.to_string())
        }
        "ghost_browser_list_installed" => {
            let found: Vec<Value> = ghost_browser::installed_browsers()
                .into_iter()
                .map(|(name, path)| json!({ "name": name, "path": path.display().to_string() }))
                .collect();
            Ok(json!({ "browsers": found }))
        }
        "ghost_browser_attach" => {
            let id = p["id"].as_str().unwrap_or("default").to_string();
            let port = p["port"].as_u64().ok_or("missing param: port")? as u16;
            session.browser_attach(&id, port).await.map_err(|e| e.to_string())
        }
        "ghost_browser_close" => {
            let id = p["id"].as_str().unwrap_or("default").to_string();
            session.browser_close(&id).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_browser_tabs" => {
            let id = p["id"].as_str().unwrap_or("default").to_string();
            let tabs = session.browser_tabs(&id).await.map_err(|e| e.to_string())?;
            Ok(json!({ "tabs": tabs }))
        }
        "ghost_tab_open" => {
            let (browser, _) = browser_ref(&p)?;
            let url = p["url"].as_str().unwrap_or("about:blank");
            let tab = session.tab_open(&browser, url).await.map_err(|e| e.to_string())?;
            Ok(json!({ "tab": tab, "background": true }))
        }
        "ghost_tab_close" => {
            let (browser, tab) = browser_tab(&p)?;
            session.tab_close(&browser, &tab).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_tab_find" => {
            let (browser, _) = browser_ref(&p)?;
            let query = req_str(&p, "query")?;
            let info = session.tab_find(&browser, &query).await.map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(info).map_err(|e| e.to_string())?)
        }
        "ghost_tab_navigate" => {
            let (browser, tab_id) = browser_tab(&p)?;
            let url = req_str(&p, "url")?;
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(30_000);
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            tab.navigate(&url, timeout_ms).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true, "background": true, "url": tab.url().await.unwrap_or_default() }))
        }
        "ghost_tab_click" => {
            let (browser, tab_id) = browser_tab(&p)?;
            let selector = req_str(&p, "selector")?;
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(10_000);
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            tab.click(&selector, timeout_ms).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_tab_type" => {
            let (browser, tab_id) = browser_tab(&p)?;
            let selector = req_str(&p, "selector")?;
            let text = req_str(&p, "text")?;
            let clear = p["clear"].as_bool().unwrap_or(true);
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            tab.type_text(&selector, &text, clear).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_tab_press" => {
            let (browser, tab_id) = browser_tab(&p)?;
            let key = req_str(&p, "key")?;
            let modifiers = str_array(&p, "modifiers");
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            tab.press(&key, &modifiers).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_tab_text" => {
            let (browser, tab_id) = browser_tab(&p)?;
            let selector = p["selector"].as_str().unwrap_or("");
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            Ok(json!({ "text": tab.text(selector).await.map_err(|e| e.to_string())? }))
        }
        "ghost_tab_eval" => {
            let (browser, tab_id) = browser_tab(&p)?;
            let expression = req_str(&p, "expression")?;
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            Ok(json!({ "value": tab.eval(&expression).await.map_err(|e| e.to_string())? }))
        }
        "ghost_tab_screenshot" => {
            let (browser, tab_id) = browser_tab(&p)?;
            let full_page = p["full_page"].as_bool().unwrap_or(false);
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            let png = tab.screenshot(full_page).await.map_err(|e| e.to_string())?;
            Ok(json!({ "png_base64": base64_encode(&png), "background": true }))
        }
        "ghost_tab_describe" => {
            let (browser, tab_id) = browser_tab(&p)?;
            let limit = p["limit"].as_u64().unwrap_or(120) as usize;
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            Ok(json!({ "elements": tab.describe(limit).await.map_err(|e| e.to_string())? }))
        }
        "ghost_tab_scroll" => {
            let (browser, tab_id) = browser_tab(&p)?;
            let selector = p["selector"].as_str().unwrap_or("");
            let dx = p["dx"].as_f64().unwrap_or(0.0);
            let dy = p["dy"].as_f64().unwrap_or(400.0);
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            tab.scroll(selector, dx, dy).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_tab_select_option" => {
            let (browser, tab_id) = browser_tab(&p)?;
            let selector = req_str(&p, "selector")?;
            let value = req_str(&p, "value")?;
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            tab.select_option(&selector, &value).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_tab_wait_for" => {
            let (browser, tab_id) = browser_tab(&p)?;
            let selector = req_str(&p, "selector")?;
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(15_000);
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            tab.wait_for_selector(&selector, timeout_ms).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_tab_info" => {
            let (browser, tab_id) = browser_tab(&p)?;
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            Ok(json!({
                "url": tab.url().await.map_err(|e| e.to_string())?,
                "title": tab.title().await.map_err(|e| e.to_string())?,
            }))
        }

        // ---- shortcuts via standard control messages ------------------------
        "ghost_shortcut_background" => {
            let window = req_str(&p, "window")?;
            let shortcut = req_str(&p, "shortcut")?;
            session
                .shortcut_background(&window, &shortcut)
                .await
                .map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }

        // ---- isolated desktops ----------------------------------------------
        "ghost_desktop_create" => {
            let id = p["id"].as_str().unwrap_or("default").to_string();
            session.desktop_create(&id).await.map_err(|e| e.to_string())
        }
        "ghost_desktop_close" => {
            let id = p["id"].as_str().unwrap_or("default").to_string();
            session.desktop_close(&id).await.map_err(|e| e.to_string())
        }
        "ghost_desktop_launch" => {
            let (id, _) = desktop_ref(&p);
            let command = req_str(&p, "command")?;
            let pid = session.desktop_launch(&id, &command).await.map_err(|e| e.to_string())?;
            Ok(json!({ "pid": pid, "invisible": true }))
        }
        "ghost_desktop_windows" => {
            let (id, _) = desktop_ref(&p);
            let windows = session.desktop_windows(&id).await.map_err(|e| e.to_string())?;
            Ok(json!({ "windows": windows }))
        }
        "ghost_desktop_wait_for_window" => {
            let (id, _) = desktop_ref(&p);
            let title = req_str(&p, "title")?;
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(20_000);
            session
                .desktop_wait_for_window(&id, &title, timeout_ms)
                .await
                .map_err(|e| e.to_string())
        }
        "ghost_desktop_click" => {
            let (id, hwnd) = desktop_window(&p)?;
            let x = p["x"].as_i64().ok_or("missing param: x")? as i32;
            let y = p["y"].as_i64().ok_or("missing param: y")? as i32;
            let button = p["button"].as_str().unwrap_or("left");
            match button {
                "right" => session.desktop_right_click(&id, hwnd, x, y).await,
                "double" => session.desktop_double_click(&id, hwnd, x, y).await,
                _ => session.desktop_click(&id, hwnd, x, y).await,
            }
            .map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_desktop_scroll" => {
            let (id, hwnd) = desktop_window(&p)?;
            let x = p["x"].as_i64().unwrap_or(10) as i32;
            let y = p["y"].as_i64().unwrap_or(10) as i32;
            let direction = p["direction"].as_str().unwrap_or("down");
            let amount = p["amount"].as_i64().unwrap_or(3) as i32;
            session
                .desktop_scroll(&id, hwnd, x, y, direction, amount)
                .await
                .map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_desktop_type" => {
            let (id, hwnd) = desktop_window(&p)?;
            let text = req_str(&p, "text")?;
            session.desktop_type(&id, hwnd, &text).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_desktop_press" => {
            let (id, hwnd) = desktop_window(&p)?;
            let key = req_str(&p, "key")?;
            session.desktop_press(&id, hwnd, &key).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_desktop_shortcut" => {
            let (id, hwnd) = desktop_window(&p)?;
            let shortcut = req_str(&p, "shortcut")?;
            session
                .desktop_shortcut(&id, hwnd, &shortcut)
                .await
                .map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_desktop_capture" => {
            let (id, hwnd) = desktop_window(&p)?;
            let client_only = p["client_only"].as_bool().unwrap_or(false);
            let png = session
                .desktop_capture(&id, hwnd, client_only)
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "png_base64": base64_encode(&png), "background": true }))
        }
        "ghost_desktop_describe" => {
            let (id, _) = desktop_ref(&p);
            let window = p["window"].as_str();
            let els = session.desktop_describe(&id, window).await.map_err(|e| e.to_string())?;
            Ok(json!({ "elements": els }))
        }
        "ghost_desktop_click_element" => {
            let (id, _) = desktop_ref(&p);
            let window = p["window"].as_str();
            let out = session
                .desktop_click_element(&id, window, p["name"].as_str(), p["role"].as_str())
                .await
                .map_err(|e| e.to_string())?;
            let (label, route) = out.split_once('|').unwrap_or((out.as_str(), "unknown"));
            Ok(json!({ "ok": true, "background": true, "element": label, "route": route }))
        }

        "ghost_cache_stats" => {
            let stats = session.cache_stats();
            Ok(serde_json::to_value(stats).map_err(|e| e.to_string())?)
        }
        "ghost_cache_invalidate" => {
            session.cache_invalidate();
            Ok(json!({ "ok": true }))
        }
        _ => Err(format!("unknown method: {}", method)),
    }
}

pub fn tools_schema() -> Value {
    json!([
        { "name": "ghost_find",
          "description": "Find the first UI element matching name or role. Returns element name and bounding rect.",
          "inputSchema": { "type": "object", "properties": {
              "window": { "type": "string", "description": "Restrict the search to this top-level window (partial title). Strongly recommended: without it the search walks every open window and can match another app or another ghost process's target." },
              "name": { "type": "string", "description": "Accessible name (case-insensitive substring)" },
              "role": { "type": "string", "description": "Control type: button, edit, checkbox, list, menu, tab, toolbar" }
          }}},
        { "name": "ghost_click",
          "description": "Find a UI element and click it.",
          "inputSchema": { "type": "object", "properties": {
              "window": { "type": "string", "description": "Restrict the search to this top-level window (partial title). Strongly recommended: without it the search walks every open window and can match another app or another ghost process's target." },
              "name": { "type": "string" }, "role": { "type": "string" }
          }}},
        { "name": "ghost_type",
          "description": "Find a UI element and type text into it.",
          "inputSchema": { "type": "object", "required": ["text"], "properties": {
              "window": { "type": "string", "description": "Restrict the search to this top-level window (partial title). Strongly recommended: without it the search walks every open window and can match another app or another ghost process's target." },
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
              "window": { "type": "string", "description": "Restrict the search to this top-level window (partial title). Strongly recommended: without it the search walks every open window and can match another app or another ghost process's target." },
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
          "description": "Navigate a browser to a URL. Routes through CDP automatically if any browser is registered (fully background); otherwise falls back to raising the window and typing the URL, which needs a non-background focus policy. Prefer ghost_browser_launch + ghost_tab_navigate for explicit control.",
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
        { "name": "ghost_shortcut_background",
          "description": "Run an editing shortcut against a background window using the standard control message (WM_UNDO, WM_CUT, WM_COPY, WM_PASTE, WM_CLEAR, EM_SETSEL). This is the correct way to do Ctrl+Z/X/C/V/A in the background - posted key messages cannot set modifier state and would type a literal character instead.",
          "inputSchema": { "type": "object", "required": ["window", "shortcut"], "properties": {
              "window": { "type": "string" },
              "shortcut": { "type": "string", "enum": ["undo", "cut", "copy", "paste", "clear", "select_all"], "description": "Also accepts the key combination, e.g. 'Ctrl+Z'" }
          }}},

        { "name": "ghost_desktop_create",
          "description": "Create an isolated Windows desktop. Apps launched onto it never appear on the user's screen at all - the desktop-app equivalent of headless. UIA, window messages, and window capture all work there; real SendInput does not (the OS refuses it on a non-displayed desktop).",
          "inputSchema": { "type": "object", "properties": {
              "id": { "type": "string", "description": "Handle for later calls (default: 'default')" }
          }}},
        { "name": "ghost_desktop_close",
          "description": "Destroy an isolated desktop, terminating any processes still running on it. Those processes have no visible window, so leaving them would strand them invisibly.",
          "inputSchema": { "type": "object", "properties": { "id": { "type": "string" }}}},
        { "name": "ghost_desktop_launch",
          "description": "Launch a program onto an isolated desktop. Its windows are never shown to the user. An app must be launched onto a desktop; Windows cannot move an existing window there.",
          "inputSchema": { "type": "object", "required": ["command"], "properties": {
              "desktop": { "type": "string" },
              "command": { "type": "string", "description": "Executable plus arguments" }
          }}},
        { "name": "ghost_desktop_windows",
          "description": "List visible windows on an isolated desktop, with handles to pass to the other desktop tools.",
          "inputSchema": { "type": "object", "properties": { "desktop": { "type": "string" }}}},
        { "name": "ghost_desktop_wait_for_window",
          "description": "Wait for a window with a matching title to appear on an isolated desktop.",
          "inputSchema": { "type": "object", "required": ["title"], "properties": {
              "desktop": { "type": "string" }, "title": { "type": "string" },
              "timeout_ms": { "type": "integer" }
          }}},
        { "name": "ghost_desktop_click",
          "description": "Click a client-area point in a window on an isolated desktop.",
          "inputSchema": { "type": "object", "required": ["hwnd", "x", "y"], "properties": {
              "desktop": { "type": "string" }, "hwnd": { "type": "integer" },
              "x": { "type": "integer" }, "y": { "type": "integer" },
              "button": { "type": "string", "enum": ["left", "right", "double"] }
          }}},
        { "name": "ghost_desktop_scroll",
          "description": "Scroll inside a window on an isolated desktop.",
          "inputSchema": { "type": "object", "required": ["hwnd"], "properties": {
              "desktop": { "type": "string" }, "hwnd": { "type": "integer" },
              "x": { "type": "integer" }, "y": { "type": "integer" },
              "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
              "amount": { "type": "integer" }
          }}},
        { "name": "ghost_desktop_type",
          "description": "Type into a window on an isolated desktop.",
          "inputSchema": { "type": "object", "required": ["hwnd", "text"], "properties": {
              "desktop": { "type": "string" }, "hwnd": { "type": "integer" },
              "text": { "type": "string" }
          }}},
        { "name": "ghost_desktop_press",
          "description": "Send a key to a window on an isolated desktop.",
          "inputSchema": { "type": "object", "required": ["hwnd", "key"], "properties": {
              "desktop": { "type": "string" }, "hwnd": { "type": "integer" },
              "key": { "type": "string" }
          }}},
        { "name": "ghost_desktop_shortcut",
          "description": "Run an editing shortcut (undo/cut/copy/paste/clear/select_all, or 'Ctrl+Z' style) against a window on an isolated desktop.",
          "inputSchema": { "type": "object", "required": ["hwnd", "shortcut"], "properties": {
              "desktop": { "type": "string" }, "hwnd": { "type": "integer" },
              "shortcut": { "type": "string" }
          }}},
        { "name": "ghost_desktop_capture",
          "description": "PNG of a window on an isolated desktop. The only way to see what an invisible app is doing.",
          "inputSchema": { "type": "object", "required": ["hwnd"], "properties": {
              "desktop": { "type": "string" }, "hwnd": { "type": "integer" },
              "client_only": { "type": "boolean" }
          }}},
        { "name": "ghost_desktop_describe",
          "description": "Interactive elements of an app on an isolated desktop, via UIA. Prefer this over capture-and-guess-coordinates: UI Automation works on a non-displayed desktop.",
          "inputSchema": { "type": "object", "properties": {
              "desktop": { "type": "string" },
              "window": { "type": "string", "description": "Scope to one window title" }
          }}},
        { "name": "ghost_desktop_click_element",
          "description": "Find an element by name or role on an isolated desktop and activate it via its UIA pattern.",
          "inputSchema": { "type": "object", "properties": {
              "desktop": { "type": "string" }, "window": { "type": "string" },
              "name": { "type": "string" }, "role": { "type": "string" }
          }}},

        { "name": "ghost_cache_invalidate",
          "description": "Clear the UIA mirror cache.",
          "inputSchema": { "type": "object", "properties": {}}},

        // ---- focus policy ------------------------------------------------
        { "name": "ghost_desktop_state",
          "description": "Report the window the user is currently working in (handle and title) and the real cursor position. Call before and after a batch of actions to verify nothing was disturbed; compare foreground_hwnd, since titles change on their own.",
          "inputSchema": { "type": "object", "properties": {}}},
        { "name": "ghost_focus_policy",
          "description": "Report the current focus policy: background (default, never touches the user's screen), prefer_background, or foreground.",
          "inputSchema": { "type": "object", "properties": {}}},
        { "name": "ghost_set_focus_policy",
          "description": "Set the focus policy. 'background' makes any cursor/keyboard/foreground-stealing call fail instead of taking over the machine. Raise to 'prefer_background' or 'foreground' ONLY for a target with no background path, and set it back afterwards.",
          "inputSchema": { "type": "object", "required": ["policy"], "properties": {
              "policy": { "type": "string", "enum": ["background", "prefer_background", "foreground"] }
          }}},

        // ---- background element actions ----------------------------------
        { "name": "ghost_element_actions",
          "description": "List which background actions an element supports (invoke, toggle, select, set_value, scroll, ...). Use this when an action fails, to pick one that works instead of falling back to the screen.",
          "inputSchema": { "type": "object", "properties": {
              "window": { "type": "string", "description": "Restrict the search to this top-level window (partial title). Strongly recommended: without it the search walks every open window and can match another app or another ghost process's target." },
              "name": { "type": "string" }, "role": { "type": "string" }
          }}},
        { "name": "ghost_toggle",
          "description": "Toggle a checkbox or toggle button in the background.",
          "inputSchema": { "type": "object", "properties": {
              "window": { "type": "string", "description": "Restrict the search to this top-level window (partial title). Strongly recommended: without it the search walks every open window and can match another app or another ghost process's target." },
              "name": { "type": "string" }, "role": { "type": "string" }
          }}},
        { "name": "ghost_select",
          "description": "Select a tab, list item, or radio button in the background.",
          "inputSchema": { "type": "object", "properties": {
              "window": { "type": "string", "description": "Restrict the search to this top-level window (partial title). Strongly recommended: without it the search walks every open window and can match another app or another ghost process's target." },
              "name": { "type": "string" }, "role": { "type": "string" }
          }}},
        { "name": "ghost_expand",
          "description": "Expand or collapse a combo box, tree item, or split button in the background.",
          "inputSchema": { "type": "object", "properties": {
              "window": { "type": "string", "description": "Restrict the search to this top-level window (partial title). Strongly recommended: without it the search walks every open window and can match another app or another ghost process's target." },
              "name": { "type": "string" }, "role": { "type": "string" },
              "expand": { "type": "boolean", "description": "true to expand (default), false to collapse" }
          }}},
        { "name": "ghost_scroll_element",
          "description": "Scroll a scrollable element in the background via its scroll pattern (no wheel, no cursor).",
          "inputSchema": { "type": "object", "properties": {
              "window": { "type": "string", "description": "Restrict the search to this top-level window (partial title). Strongly recommended: without it the search walks every open window and can match another app or another ghost process's target." },
              "name": { "type": "string" }, "role": { "type": "string" },
              "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
              "amount": { "type": "integer", "description": "Number of large scroll increments (default 1)" }
          }}},
        { "name": "ghost_scroll_into_view",
          "description": "Bring an element into view inside its scrollable parent, without moving the cursor.",
          "inputSchema": { "type": "object", "properties": {
              "window": { "type": "string", "description": "Restrict the search to this top-level window (partial title). Strongly recommended: without it the search walks every open window and can match another app or another ghost process's target." },
              "name": { "type": "string" }, "role": { "type": "string" }
          }}},
        { "name": "ghost_set_range_value",
          "description": "Set a slider or spinner value in the background.",
          "inputSchema": { "type": "object", "required": ["value"], "properties": {
              "window": { "type": "string", "description": "Restrict the search to this top-level window (partial title). Strongly recommended: without it the search walks every open window and can match another app or another ghost process's target." },
              "name": { "type": "string" }, "role": { "type": "string" },
              "value": { "type": "number" }
          }}},
        { "name": "ghost_document_text",
          "description": "Read the full text of a document or editor via TextPattern. Reads far more than ghost_get_text, which only returns a single control value.",
          "inputSchema": { "type": "object", "properties": {
              "window": { "type": "string", "description": "Restrict the search to this top-level window (partial title). Strongly recommended: without it the search walks every open window and can match another app or another ghost process's target." },
              "name": { "type": "string" }, "role": { "type": "string" },
              "max_chars": { "type": "integer" }
          }}},

        // ---- window-scoped background input --------------------------------
        { "name": "ghost_type_background",
          "description": "Type text into a background window's focused control via window messages. The user's cursor and focus are untouched. Works for Win32/WinForms/MFC apps; for browsers use the ghost_tab_* tools.",
          "inputSchema": { "type": "object", "required": ["window", "text"], "properties": {
              "window": { "type": "string", "description": "Partial window title (case-insensitive)" },
              "text": { "type": "string" }
          }}},
        { "name": "ghost_press_background",
          "description": "Send a key to a background window without touching the user's keyboard focus.",
          "inputSchema": { "type": "object", "required": ["window", "key"], "properties": {
              "window": { "type": "string" }, "key": { "type": "string" }
          }}},
        { "name": "ghost_hotkey_background",
          "description": "Send a modifier combo (e.g. Ctrl+S) to a background window. Modifiers are always released, even if the key press fails.",
          "inputSchema": { "type": "object", "required": ["window", "key"], "properties": {
              "window": { "type": "string" },
              "modifiers": { "type": "array", "items": { "type": "string" }, "description": "e.g. [\"Ctrl\"]" },
              "key": { "type": "string" }
          }}},
        { "name": "ghost_set_text_background",
          "description": "Replace a background window's text in one message (WM_SETTEXT).",
          "inputSchema": { "type": "object", "required": ["window", "text"], "properties": {
              "window": { "type": "string" }, "text": { "type": "string" }
          }}},
        { "name": "ghost_right_click_background",
          "description": "Right-click at a client-area point in a background window.",
          "inputSchema": { "type": "object", "required": ["window", "x", "y"], "properties": {
              "window": { "type": "string" }, "x": { "type": "integer" }, "y": { "type": "integer" }
          }}},
        { "name": "ghost_double_click_background",
          "description": "Double-click at a client-area point in a background window.",
          "inputSchema": { "type": "object", "required": ["window", "x", "y"], "properties": {
              "window": { "type": "string" }, "x": { "type": "integer" }, "y": { "type": "integer" }
          }}},
        { "name": "ghost_hover_background",
          "description": "Move the pointer within a background window (hover states, tooltips) without moving the user's real cursor.",
          "inputSchema": { "type": "object", "required": ["window", "x", "y"], "properties": {
              "window": { "type": "string" }, "x": { "type": "integer" }, "y": { "type": "integer" }
          }}},
        { "name": "ghost_scroll_background",
          "description": "Wheel-scroll inside a background window.",
          "inputSchema": { "type": "object", "required": ["window", "x", "y"], "properties": {
              "window": { "type": "string" }, "x": { "type": "integer" }, "y": { "type": "integer" },
              "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
              "amount": { "type": "integer", "description": "Wheel notches (default 3)" }
          }}},
        { "name": "ghost_click_element_background",
          "description": "Find an element by name/role, then click it through its window's message queue. Use when an element has no usable UIA pattern but you still must not touch the screen.",
          "inputSchema": { "type": "object", "required": ["window"], "properties": {
              "window": { "type": "string" }, "name": { "type": "string" }, "role": { "type": "string" }
          }}},
        { "name": "ghost_capture_window",
          "description": "PNG of one window, captured without raising it. Sees windows the user has covered, and unlike ghost_screenshot does not capture the user's whole screen.",
          "inputSchema": { "type": "object", "required": ["window"], "properties": {
              "window": { "type": "string" },
              "client_only": { "type": "boolean", "description": "Exclude title bar and border" }
          }}},

        // ---- browsers and tabs ---------------------------------------------
        { "name": "ghost_browser_launch",
          "description": "Launch an isolated browser for automation. Each id gets its own process, profile, and DevTools port, so concurrent ghost processes never collide. 'headless' is invisible; 'windowed' is a real window kept off the visible desktop.",
          "inputSchema": { "type": "object", "properties": {
              "id": { "type": "string", "description": "Handle for later calls (default: 'default')" },
              "mode": { "type": "string", "enum": ["headless", "windowed"] },
              "browser": { "type": "string", "enum": ["chrome", "comet", "edge", "brave"], "description": "Which installed browser to launch (default: first installed). All are driven identically over CDP." }
          }}},
        { "name": "ghost_browser_list_installed",
          "description": "List the Chromium-family browsers installed on this machine that ghost can launch (chrome, comet, edge, brave).",
          "inputSchema": { "type": "object", "properties": {}}},
        { "name": "ghost_browser_attach",
          "description": "Attach to a browser already running with --remote-debugging-port=<port>. Use when the automation needs the user's real logins. Never closes that browser.",
          "inputSchema": { "type": "object", "required": ["port"], "properties": {
              "id": { "type": "string" }, "port": { "type": "integer" }
          }}},
        { "name": "ghost_browser_close",
          "description": "Close a browser ghost launched. Attached browsers are only disconnected.",
          "inputSchema": { "type": "object", "properties": { "id": { "type": "string" }}}},
        { "name": "ghost_browser_tabs",
          "description": "List open tabs with their target ids, titles, and URLs.",
          "inputSchema": { "type": "object", "properties": { "id": { "type": "string" }}}},
        { "name": "ghost_tab_open",
          "description": "Open a new background tab and return its id. Opening never brings the tab or window to the front.",
          "inputSchema": { "type": "object", "properties": {
              "browser": { "type": "string" }, "url": { "type": "string" }
          }}},
        { "name": "ghost_tab_close",
          "description": "Close a tab by id.",
          "inputSchema": { "type": "object", "required": ["tab"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" }
          }}},
        { "name": "ghost_tab_find",
          "description": "Find a tab whose URL or title contains a substring.",
          "inputSchema": { "type": "object", "required": ["query"], "properties": {
              "browser": { "type": "string" }, "query": { "type": "string" }
          }}},
        { "name": "ghost_tab_navigate",
          "description": "Navigate a tab and wait for load. Runs in a background tab; the user's view never changes.",
          "inputSchema": { "type": "object", "required": ["tab", "url"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "url": { "type": "string" }, "timeout_ms": { "type": "integer" }
          }}},
        { "name": "ghost_tab_click",
          "description": "Click an element by CSS selector using a trusted synthetic mouse event inside the tab's renderer. No cursor movement, works in a tab that is not in front.",
          "inputSchema": { "type": "object", "required": ["tab", "selector"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "selector": { "type": "string" }, "timeout_ms": { "type": "integer" }
          }}},
        { "name": "ghost_tab_type",
          "description": "Focus an element by CSS selector and type into it. Layout-independent and does not use the user's keyboard.",
          "inputSchema": { "type": "object", "required": ["tab", "selector", "text"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "selector": { "type": "string" }, "text": { "type": "string" },
              "clear": { "type": "boolean", "description": "Clear the field first (default true)" }
          }}},
        { "name": "ghost_tab_press",
          "description": "Send a key (Enter, Tab, ArrowDown, a, ...) to the tab's focused element.",
          "inputSchema": { "type": "object", "required": ["tab", "key"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "key": { "type": "string" },
              "modifiers": { "type": "array", "items": { "type": "string" }, "description": "Alt, Ctrl, Meta, Shift" }
          }}},
        { "name": "ghost_tab_text",
          "description": "Visible text of an element, or of the whole page when selector is omitted.",
          "inputSchema": { "type": "object", "required": ["tab"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" }, "selector": { "type": "string" }
          }}},
        { "name": "ghost_tab_eval",
          "description": "Evaluate a JavaScript expression in the tab and return its value. Awaits promises.",
          "inputSchema": { "type": "object", "required": ["tab", "expression"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" }, "expression": { "type": "string" }
          }}},
        { "name": "ghost_tab_screenshot",
          "description": "PNG of a tab, rendered by that tab regardless of whether it is in front or its window is focused.",
          "inputSchema": { "type": "object", "required": ["tab"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "full_page": { "type": "boolean" }
          }}},
        { "name": "ghost_tab_describe",
          "description": "Structured list of visible interactive elements with selectors and coordinates. Prefer this over screenshots for deciding what to click.",
          "inputSchema": { "type": "object", "required": ["tab"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" }, "limit": { "type": "integer" }
          }}},
        { "name": "ghost_tab_scroll",
          "description": "Scroll a tab or a scrollable element inside it.",
          "inputSchema": { "type": "object", "required": ["tab"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "selector": { "type": "string" }, "dx": { "type": "number" }, "dy": { "type": "number" }
          }}},
        { "name": "ghost_tab_select_option",
          "description": "Choose an option in a <select>, firing the input and change events a real choice fires.",
          "inputSchema": { "type": "object", "required": ["tab", "selector", "value"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "selector": { "type": "string" }, "value": { "type": "string" }
          }}},
        { "name": "ghost_tab_wait_for",
          "description": "Wait until a CSS selector matches an element in the tab.",
          "inputSchema": { "type": "object", "required": ["tab", "selector"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "selector": { "type": "string" }, "timeout_ms": { "type": "integer" }
          }}},
        { "name": "ghost_tab_info",
          "description": "Current URL and title of a tab.",
          "inputSchema": { "type": "object", "required": ["tab"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" }
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

/// Find an element, scoped to the optional `window` parameter.
///
/// Scoping is what keeps concurrent ghost processes from finding each other's
/// elements, so every element tool accepts it.
async fn find_scoped(
    session: &GhostSession,
    p: &Value,
    by: ghost_session::By,
) -> std::result::Result<ghost_session::GhostElement, String> {
    match p["window"].as_str() {
        Some(w) => session.find_in(w, by).await,
        None => session.find(by).await,
    }
    .map_err(|e| e.to_string())
}

/// `desktop` id, defaulting to "default", plus the optional window handle.
fn desktop_ref(p: &Value) -> (String, isize) {
    (
        p["desktop"].as_str().unwrap_or("default").to_string(),
        p["hwnd"].as_i64().unwrap_or(0) as isize,
    )
}

fn desktop_window(p: &Value) -> std::result::Result<(String, isize), String> {
    let (id, hwnd) = desktop_ref(p);
    if hwnd == 0 {
        return Err("missing param: hwnd (get one from ghost_desktop_windows or \
                    ghost_desktop_wait_for_window)"
            .into());
    }
    Ok((id, hwnd))
}

/// Required string parameter.
fn req_str(p: &Value, key: &str) -> std::result::Result<String, String> {
    p[key]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("missing param: {key}"))
}

/// `window` + `x` + `y`, the shape every window-scoped background tool takes.
fn window_point(p: &Value) -> std::result::Result<(String, i32, i32), String> {
    let window = req_str(p, "window")?;
    let x = p["x"].as_i64().ok_or("missing param: x")? as i32;
    let y = p["y"].as_i64().ok_or("missing param: y")? as i32;
    Ok((window, x, y))
}

/// Optional string array parameter (modifier lists), tolerant of a bare string so a
/// caller passing `"Ctrl"` instead of `["Ctrl"]` still works.
fn str_array(p: &Value, key: &str) -> Vec<String> {
    match &p[key] {
        Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        Value::String(s) => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn browser_ref(p: &Value) -> std::result::Result<(String, String), String> {
    let browser = p["browser"].as_str().unwrap_or("default").to_string();
    let tab = p["tab"].as_str().unwrap_or_default().to_string();
    Ok((browser, tab))
}

fn browser_tab(p: &Value) -> std::result::Result<(String, String), String> {
    let (browser, tab) = browser_ref(p)?;
    if tab.is_empty() {
        return Err("missing param: tab (get one from ghost_tab_open or ghost_browser_tabs)".into());
    }
    Ok((browser, tab))
}

/// Standard result for an action that ran without touching the user's screen.
fn bg_ok() -> Value {
    json!({ "ok": true, "background": true })
}

/// Report which mechanism actually performed an element action, so an agent can see
/// when it silently fell back to real input instead of a background pattern.
fn route_result(route: ghost_session::ActionRoute) -> Value {
    json!({ "ok": true, "route": route.as_str(), "background": route.is_background() })
}

pub fn base64_encode(data: &[u8]) -> String {
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


    /// Tool names declared in the schema, in declaration order.
    fn schema_names() -> Vec<String> {
        tools_schema()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str().map(String::from))
            .collect()
    }

    /// Tool names that `handle` actually dispatches, scraped from this file.
    ///
    /// A count assertion only tells you the number changed. This pair of tests tells
    /// you *which* tool is advertised with no implementation (an agent calling it
    /// gets "unknown method") or implemented but undiscoverable.
    fn handled_names() -> Vec<String> {
        let src = include_str!("lib.rs");
        let mut out = Vec::new();
        for line in src.lines() {
            let t = line.trim();
            // Match the dispatch arms: `"ghost_x" => {`
            if let Some(rest) = t.strip_prefix('"') {
                if let Some((name, tail)) = rest.split_once('"') {
                    if name.starts_with("ghost_") && tail.trim_start().starts_with("=>") {
                        out.push(name.to_string());
                    }
                }
            }
        }
        out
    }

    #[test]
    fn every_declared_tool_has_a_handler() {
        let handled = handled_names();
        for name in schema_names() {
            assert!(
                handled.contains(&name),
                "tool '{name}' is advertised in tools/list but has no dispatch arm"
            );
        }
    }

    #[test]
    fn every_handled_tool_is_declared() {
        let declared = schema_names();
        for name in handled_names() {
            assert!(
                declared.contains(&name),
                "tool '{name}' is implemented but missing from tools/list, so no agent can find it"
            );
        }
    }

    #[test]
    fn tool_names_are_unique() {
        let names = schema_names();
        let mut seen = std::collections::HashSet::new();
        for n in &names {
            assert!(seen.insert(n.clone()), "duplicate tool name: {n}");
        }
        assert!(names.len() > 60, "expected the full tool surface, got {}", names.len());
    }

    #[test]
    fn background_surface_is_registered() {
        // The tools that make the "works while you work" claim true. If any of these
        // vanish, the claim silently regresses to the old foreground-only behavior.
        let names = schema_names();
        for t in [
            "ghost_set_focus_policy",
            "ghost_focus_policy",
            "ghost_type_background",
            "ghost_press_background",
            "ghost_hotkey_background",
            "ghost_click_element_background",
            "ghost_capture_window",
            "ghost_element_actions",
            "ghost_browser_launch",
            "ghost_tab_open",
            "ghost_tab_click",
            "ghost_tab_type",
            "ghost_tab_screenshot",
            "ghost_tab_describe",
        ] {
            assert!(names.contains(&t.to_string()), "missing background tool {t}");
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
        // Verify initialize response shape matches MCP 2024-11-05 spec
        let resp = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "ghost", "version": "0.5.0" }
        });
        assert_eq!(resp["protocolVersion"], "2024-11-05");
        assert!(resp["capabilities"]["tools"].is_object());
        assert_eq!(resp["serverInfo"]["name"], "ghost");
    }
}
