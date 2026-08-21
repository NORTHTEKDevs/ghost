//! Extended tool surface: browsers/tabs (CDP), isolated desktops, and the focus
//! policy. Kept in one module so the main dispatcher needs exactly two hooks -
//! `dispatch` and `schemas` - and the core file stays reviewable.
//!
//! Everything here is background by construction: CDP injects input into a
//! specific renderer (a tab need not be visible or focused), and an isolated
//! desktop is never displayed at all. Multiple ghost processes can each drive
//! their own browsers and desktops concurrently - ids, ports, and profiles are
//! per-process.

use ghost_session::GhostSession;
use serde_json::{json, Value};

/// True when `name` belongs to this module's surface. Used by the router to
/// decide whether to try `dispatch` before the legacy tool table.
pub fn owns(name: &str) -> bool {
    name.starts_with("ghost_browser_")
        || name.starts_with("ghost_tab_")
        || name.starts_with("ghost_desktop_")
        || matches!(name, "ghost_focus_policy" | "ghost_set_focus_policy" | "ghost_session_state")
}

pub async fn dispatch(
    session: &GhostSession,
    method: &str,
    p: &Value,
) -> std::result::Result<Value, String> {
    match method {
        // ---- focus policy and self-check -----------------------------------
        #[cfg(windows)]
        "ghost_focus_policy" => Ok(json!({ "policy": session.focus_policy() })),
        #[cfg(windows)]
        "ghost_set_focus_policy" => {
            let policy = req_str(p, "policy")?;
            let applied = session.set_focus_policy(&policy).map_err(|e| e.to_string())?;
            Ok(json!({ "policy": applied }))
        }
        #[cfg(windows)]
        "ghost_session_state" => {
            let snap = ghost_session::engine::system::DesktopSnapshot::capture();
            Ok(json!({
                // The handle, not just the title: a window that merely retitles
                // itself (terminals do constantly) is not a stolen foreground.
                "foreground_hwnd": snap.foreground_hwnd,
                "foreground_window": snap.foreground_title,
                "cursor": [snap.cursor.0, snap.cursor.1],
                "policy": session.focus_policy(),
            }))
        }

        // ---- browsers -------------------------------------------------------
        "ghost_browser_launch" => {
            let id = p["id"].as_str().unwrap_or("default").to_string();
            let mode = p["mode"].as_str().unwrap_or("headless");
            let which = p["browser"].as_str();
            session
                .browser_launch_with(&id, mode, which)
                .await
                .map_err(|e| e.to_string())
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
        "ghost_browser_list_installed" => {
            let found: Vec<Value> = ghost_browser::installed_browsers()
                .into_iter()
                .map(|(name, path)| json!({ "name": name, "path": path.display().to_string() }))
                .collect();
            Ok(json!({ "browsers": found }))
        }

        // ---- tabs -----------------------------------------------------------
        "ghost_tab_open" => {
            let (browser, _) = browser_ref(p);
            let url = p["url"].as_str().unwrap_or("about:blank");
            let tab = session.tab_open(&browser, url).await.map_err(|e| e.to_string())?;
            Ok(json!({ "tab": tab, "background": true }))
        }
        "ghost_tab_close" => {
            let (browser, tab) = browser_tab(p)?;
            session.tab_close(&browser, &tab).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_tab_find" => {
            let (browser, _) = browser_ref(p);
            let query = req_str(p, "query")?;
            let info = session.tab_find(&browser, &query).await.map_err(|e| e.to_string())?;
            serde_json::to_value(info).map_err(|e| e.to_string())
        }
        "ghost_tab_navigate" => {
            let (browser, tab_id) = browser_tab(p)?;
            let url = req_str(p, "url")?;
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(30_000);
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            tab.navigate(&url, timeout_ms).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true, "background": true, "url": tab.url().await.unwrap_or_default() }))
        }
        "ghost_tab_click" => {
            let (browser, tab_id) = browser_tab(p)?;
            let selector = req_str(p, "selector")?;
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(10_000);
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            tab.click(&selector, timeout_ms).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_tab_type" => {
            let (browser, tab_id) = browser_tab(p)?;
            let selector = req_str(p, "selector")?;
            let text = req_str(p, "text")?;
            let clear = p["clear"].as_bool().unwrap_or(true);
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            tab.type_text(&selector, &text, clear).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_tab_press" => {
            let (browser, tab_id) = browser_tab(p)?;
            let key = req_str(p, "key")?;
            let modifiers = str_array(p, "modifiers");
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            tab.press(&key, &modifiers).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_tab_text" => {
            let (browser, tab_id) = browser_tab(p)?;
            let selector = p["selector"].as_str().unwrap_or("");
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            Ok(json!({ "text": tab.text(selector).await.map_err(|e| e.to_string())? }))
        }
        "ghost_tab_eval" => {
            let (browser, tab_id) = browser_tab(p)?;
            let expression = req_str(p, "expression")?;
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            Ok(json!({ "value": tab.eval(&expression).await.map_err(|e| e.to_string())? }))
        }
        "ghost_tab_screenshot" => {
            let (browser, tab_id) = browser_tab(p)?;
            let full_page = p["full_page"].as_bool().unwrap_or(false);
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            let png = tab.screenshot(full_page).await.map_err(|e| e.to_string())?;
            Ok(json!({ "png_base64": base64_encode(&png), "background": true }))
        }
        "ghost_tab_describe" => {
            let (browser, tab_id) = browser_tab(p)?;
            let limit = p["limit"].as_u64().unwrap_or(120) as usize;
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            Ok(json!({ "elements": tab.describe(limit).await.map_err(|e| e.to_string())? }))
        }
        "ghost_tab_scroll" => {
            let (browser, tab_id) = browser_tab(p)?;
            let selector = p["selector"].as_str().unwrap_or("");
            let dx = p["dx"].as_f64().unwrap_or(0.0);
            let dy = p["dy"].as_f64().unwrap_or(400.0);
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            tab.scroll(selector, dx, dy).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_tab_select_option" => {
            let (browser, tab_id) = browser_tab(p)?;
            let selector = req_str(p, "selector")?;
            let value = req_str(p, "value")?;
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            tab.select_option(&selector, &value).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        "ghost_tab_wait_for" => {
            let (browser, tab_id) = browser_tab(p)?;
            let selector = req_str(p, "selector")?;
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(15_000);
            let tab = session.tab(&browser, &tab_id).await.map_err(|e| e.to_string())?;
            tab.wait_for_selector(&selector, timeout_ms).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }

        // ---- isolated desktops (Windows) ------------------------------------
        #[cfg(windows)]
        "ghost_desktop_create" => {
            let id = p["id"].as_str().unwrap_or("default").to_string();
            session.desktop_create(&id).await.map_err(|e| e.to_string())
        }
        #[cfg(windows)]
        "ghost_desktop_close" => {
            let id = p["id"].as_str().unwrap_or("default").to_string();
            session.desktop_close(&id).await.map_err(|e| e.to_string())
        }
        #[cfg(windows)]
        "ghost_desktop_launch" => {
            let (id, _) = desktop_ref(p);
            let command = req_str(p, "command")?;
            let pid = session.desktop_launch(&id, &command).await.map_err(|e| e.to_string())?;
            Ok(json!({ "pid": pid, "invisible": true }))
        }
        #[cfg(windows)]
        "ghost_desktop_windows" => {
            let (id, _) = desktop_ref(p);
            let windows = session.desktop_windows(&id).await.map_err(|e| e.to_string())?;
            Ok(json!({ "windows": windows }))
        }
        #[cfg(windows)]
        "ghost_desktop_wait_for_window" => {
            let (id, _) = desktop_ref(p);
            let title = req_str(p, "title")?;
            let timeout_ms = p["timeout_ms"].as_u64().unwrap_or(20_000);
            session
                .desktop_wait_for_window(&id, &title, timeout_ms)
                .await
                .map_err(|e| e.to_string())
        }
        #[cfg(windows)]
        "ghost_desktop_click" => {
            let (id, hwnd) = desktop_window(p)?;
            let x = p["x"].as_i64().ok_or("missing param: x")? as i32;
            let y = p["y"].as_i64().ok_or("missing param: y")? as i32;
            let button = p["button"].as_str().unwrap_or("left");
            session
                .desktop_click(&id, hwnd, x, y, button)
                .await
                .map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        #[cfg(windows)]
        "ghost_desktop_scroll" => {
            let (id, hwnd) = desktop_window(p)?;
            let direction = p["direction"].as_str().unwrap_or("down");
            let amount = p["amount"].as_i64().unwrap_or(3) as i32;
            session
                .desktop_scroll(&id, hwnd, direction, amount)
                .await
                .map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        #[cfg(windows)]
        "ghost_desktop_type" => {
            let (id, hwnd) = desktop_window(p)?;
            let text = req_str(p, "text")?;
            session.desktop_type(&id, hwnd, &text).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        #[cfg(windows)]
        "ghost_desktop_press" => {
            let (id, hwnd) = desktop_window(p)?;
            let key = req_str(p, "key")?;
            session.desktop_press(&id, hwnd, &key).await.map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        #[cfg(windows)]
        "ghost_desktop_shortcut" => {
            let (id, hwnd) = desktop_window(p)?;
            let shortcut = req_str(p, "shortcut")?;
            session
                .desktop_shortcut(&id, hwnd, &shortcut)
                .await
                .map_err(|e| e.to_string())?;
            Ok(bg_ok())
        }
        #[cfg(windows)]
        "ghost_desktop_capture" => {
            let (id, hwnd) = desktop_window(p)?;
            let png = session.desktop_capture(&id, hwnd).await.map_err(|e| e.to_string())?;
            Ok(json!({ "png_base64": base64_encode(&png), "background": true }))
        }
        #[cfg(windows)]
        "ghost_desktop_describe" => {
            let (id, _) = desktop_ref(p);
            let window = p["window"].as_str();
            let els = session.desktop_describe(&id, window).await.map_err(|e| e.to_string())?;
            Ok(json!({ "elements": els }))
        }

        other => Err(format!("unknown extended tool: {other}")),
    }
}

/// Schemas for the extended surface, appended to the lean tools/list.
pub fn schemas() -> Vec<Value> {
    let mut tools = vec![
        json!({ "name": "ghost_browser_launch",
          "description": "Launch an isolated browser for background automation. Each id gets its own process, profile, and DevTools port, so concurrent ghost processes never collide. browser= chrome | comet | edge | brave (default: first installed). mode=headless (invisible) | windowed (kept off the visible desktop). Tabs are driven individually via ghost_tab_* without focusing the window or moving the cursor.",
          "inputSchema": { "type": "object", "properties": {
              "id": { "type": "string", "description": "Handle for later calls (default: 'default')" },
              "mode": { "type": "string", "enum": ["headless", "windowed"] },
              "browser": { "type": "string", "enum": ["chrome", "comet", "edge", "brave"] }
          }}}),
        json!({ "name": "ghost_browser_attach",
          "description": "Attach to a browser already running with --remote-debugging-port=<port> (use for the user's own logged-in browser). Ghost never closes a browser it did not start, and drives background tabs without switching the user's active tab.",
          "inputSchema": { "type": "object", "required": ["port"], "properties": {
              "id": { "type": "string" }, "port": { "type": "integer" }
          }}}),
        json!({ "name": "ghost_browser_close",
          "description": "Close a browser ghost launched (attached browsers are only disconnected).",
          "inputSchema": { "type": "object", "properties": { "id": { "type": "string" }}}}),
        json!({ "name": "ghost_browser_tabs",
          "description": "List open tabs with target ids, titles, URLs.",
          "inputSchema": { "type": "object", "properties": { "id": { "type": "string" }}}}),
        json!({ "name": "ghost_browser_list_installed",
          "description": "List Chromium-family browsers installed on this machine (chrome, comet, edge, brave).",
          "inputSchema": { "type": "object", "properties": {}}}),
        json!({ "name": "ghost_tab_open",
          "description": "Open a new background tab (never brings the tab or window to the front). Returns its id for the other ghost_tab_* tools.",
          "inputSchema": { "type": "object", "properties": {
              "browser": { "type": "string" }, "url": { "type": "string" }
          }}}),
        json!({ "name": "ghost_tab_navigate",
          "description": "Navigate a tab and wait for load. Runs in a background tab; the user's view never changes.",
          "inputSchema": { "type": "object", "required": ["tab", "url"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "url": { "type": "string" }, "timeout_ms": { "type": "integer" }
          }}}),
        json!({ "name": "ghost_tab_click",
          "description": "Click an element by CSS selector via a trusted synthetic mouse event inside the tab's renderer. ~1-2ms; no cursor movement; works in a tab that is not in front.",
          "inputSchema": { "type": "object", "required": ["tab", "selector"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "selector": { "type": "string" }, "timeout_ms": { "type": "integer" }
          }}}),
        json!({ "name": "ghost_tab_type",
          "description": "Focus an element by CSS selector and type into it. Layout-independent; does not touch the user's keyboard.",
          "inputSchema": { "type": "object", "required": ["tab", "selector", "text"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "selector": { "type": "string" }, "text": { "type": "string" },
              "clear": { "type": "boolean", "description": "Clear the field first (default true)" }
          }}}),
        json!({ "name": "ghost_tab_press",
          "description": "Send a key (Enter, Tab, ArrowDown, a, ...) to the tab's focused element. modifiers: Alt/Ctrl/Meta/Shift.",
          "inputSchema": { "type": "object", "required": ["tab", "key"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "key": { "type": "string" },
              "modifiers": { "type": "array", "items": { "type": "string" }}
          }}}),
        json!({ "name": "ghost_tab_text",
          "description": "Visible text of an element, or the whole page when selector is omitted. Prefer over screenshots for reading content.",
          "inputSchema": { "type": "object", "required": ["tab"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" }, "selector": { "type": "string" }
          }}}),
        json!({ "name": "ghost_tab_eval",
          "description": "Evaluate a JavaScript expression in the tab and return its value (awaits promises).",
          "inputSchema": { "type": "object", "required": ["tab", "expression"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" }, "expression": { "type": "string" }
          }}}),
        json!({ "name": "ghost_tab_screenshot",
          "description": "PNG of a tab, rendered by that tab regardless of whether it is in front or focused.",
          "inputSchema": { "type": "object", "required": ["tab"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "full_page": { "type": "boolean" }
          }}}),
        json!({ "name": "ghost_tab_describe",
          "description": "Structured list of visible interactive elements with selectors and coordinates. Prefer this over screenshots for deciding what to click.",
          "inputSchema": { "type": "object", "required": ["tab"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" }, "limit": { "type": "integer" }
          }}}),
        json!({ "name": "ghost_tab_scroll",
          "description": "Scroll a tab or a scrollable element inside it.",
          "inputSchema": { "type": "object", "required": ["tab"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "selector": { "type": "string" }, "dx": { "type": "number" }, "dy": { "type": "number" }
          }}}),
        json!({ "name": "ghost_tab_select_option",
          "description": "Choose an option in a <select>, firing the input/change events a real choice fires.",
          "inputSchema": { "type": "object", "required": ["tab", "selector", "value"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "selector": { "type": "string" }, "value": { "type": "string" }
          }}}),
        json!({ "name": "ghost_tab_wait_for",
          "description": "Wait until a CSS selector matches an element in the tab.",
          "inputSchema": { "type": "object", "required": ["tab", "selector"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" },
              "selector": { "type": "string" }, "timeout_ms": { "type": "integer" }
          }}}),
        json!({ "name": "ghost_tab_close",
          "description": "Close a tab by id.",
          "inputSchema": { "type": "object", "required": ["tab"], "properties": {
              "browser": { "type": "string" }, "tab": { "type": "string" }
          }}}),
        json!({ "name": "ghost_tab_find",
          "description": "Find a tab whose URL or title contains a substring.",
          "inputSchema": { "type": "object", "required": ["query"], "properties": {
              "browser": { "type": "string" }, "query": { "type": "string" }
          }}}),
    ];
    #[cfg(windows)]
    {
        tools.extend([
            json!({ "name": "ghost_focus_policy",
              "description": "Report the current focus policy: background (default, never touches the user's screen), prefer_background, or foreground.",
              "inputSchema": { "type": "object", "properties": {}}}),
            json!({ "name": "ghost_set_focus_policy",
              "description": "Set the focus policy. 'background' (the default) makes any cursor/keyboard/foreground-stealing call fail instead of taking over the machine. Raise to 'prefer_background' or 'foreground' ONLY for a target with no background path, and set it back afterwards.",
              "inputSchema": { "type": "object", "required": ["policy"], "properties": {
                  "policy": { "type": "string", "enum": ["background", "prefer_background", "foreground"] }
              }}}),
            json!({ "name": "ghost_session_state",
              "description": "The window the user is working in (handle + title) and the real cursor position. Call before/after a batch of actions to verify nothing was disturbed; compare foreground_hwnd, since titles change on their own.",
              "inputSchema": { "type": "object", "properties": {}}}),
            json!({ "name": "ghost_desktop_create",
              "description": "Create an isolated Windows desktop - apps launched onto it NEVER appear on the user's screen (the desktop-app equivalent of headless). UIA, window messages, and capture all work there; real SendInput does not (OS boundary).",
              "inputSchema": { "type": "object", "properties": { "id": { "type": "string" }}}}),
            json!({ "name": "ghost_desktop_close",
              "description": "Destroy an isolated desktop, terminating any processes still on it (they have no visible window to close).",
              "inputSchema": { "type": "object", "properties": { "id": { "type": "string" }}}}),
            json!({ "name": "ghost_desktop_launch",
              "description": "Launch a program onto an isolated desktop. Windows cannot move an existing window there - launch is the only entry.",
              "inputSchema": { "type": "object", "required": ["command"], "properties": {
                  "desktop": { "type": "string" }, "command": { "type": "string" }
              }}}),
            json!({ "name": "ghost_desktop_windows",
              "description": "Visible windows on an isolated desktop, with handles for the other desktop tools.",
              "inputSchema": { "type": "object", "properties": { "desktop": { "type": "string" }}}}),
            json!({ "name": "ghost_desktop_wait_for_window",
              "description": "Wait for a window with a matching title to appear on an isolated desktop.",
              "inputSchema": { "type": "object", "required": ["title"], "properties": {
                  "desktop": { "type": "string" }, "title": { "type": "string" },
                  "timeout_ms": { "type": "integer" }
              }}}),
            json!({ "name": "ghost_desktop_click",
              "description": "Click a client-area point in a window on an isolated desktop. button=left|right|double.",
              "inputSchema": { "type": "object", "required": ["hwnd", "x", "y"], "properties": {
                  "desktop": { "type": "string" }, "hwnd": { "type": "integer" },
                  "x": { "type": "integer" }, "y": { "type": "integer" },
                  "button": { "type": "string", "enum": ["left", "right", "double"] }
              }}}),
            json!({ "name": "ghost_desktop_scroll",
              "description": "Scroll a window on an isolated desktop.",
              "inputSchema": { "type": "object", "required": ["hwnd"], "properties": {
                  "desktop": { "type": "string" }, "hwnd": { "type": "integer" },
                  "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                  "amount": { "type": "integer" }
              }}}),
            json!({ "name": "ghost_desktop_type",
              "description": "Type into a window on an isolated desktop.",
              "inputSchema": { "type": "object", "required": ["hwnd", "text"], "properties": {
                  "desktop": { "type": "string" }, "hwnd": { "type": "integer" },
                  "text": { "type": "string" }
              }}}),
            json!({ "name": "ghost_desktop_press",
              "description": "Send a key to a window on an isolated desktop.",
              "inputSchema": { "type": "object", "required": ["hwnd", "key"], "properties": {
                  "desktop": { "type": "string" }, "hwnd": { "type": "integer" },
                  "key": { "type": "string" }
              }}}),
            json!({ "name": "ghost_desktop_shortcut",
              "description": "Editing shortcut (undo/cut/copy/paste/select_all, or 'Ctrl+Z' style) via the standard control messages - faked modifier keys would type a literal character instead.",
              "inputSchema": { "type": "object", "required": ["hwnd", "shortcut"], "properties": {
                  "desktop": { "type": "string" }, "hwnd": { "type": "integer" },
                  "shortcut": { "type": "string" }
              }}}),
            json!({ "name": "ghost_desktop_capture",
              "description": "PNG of a window on an isolated desktop - the only way to see what an invisible app is doing.",
              "inputSchema": { "type": "object", "required": ["hwnd"], "properties": {
                  "desktop": { "type": "string" }, "hwnd": { "type": "integer" }
              }}}),
            json!({ "name": "ghost_desktop_describe",
              "description": "Interactive elements of an app on an isolated desktop via UIA. Prefer over capture-and-guess: UI Automation works on a non-displayed desktop.",
              "inputSchema": { "type": "object", "properties": {
                  "desktop": { "type": "string" }, "window": { "type": "string" }
              }}}),
        ]);
    }
    tools
}

// ---------------------------------------------------------------------------

fn req_str(p: &Value, key: &str) -> std::result::Result<String, String> {
    p[key]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("missing param: {key}"))
}

fn browser_ref(p: &Value) -> (String, String) {
    (
        p["browser"].as_str().unwrap_or("default").to_string(),
        p["tab"].as_str().unwrap_or_default().to_string(),
    )
}

fn browser_tab(p: &Value) -> std::result::Result<(String, String), String> {
    let (browser, tab) = browser_ref(p);
    if tab.is_empty() {
        return Err("missing param: tab (get one from ghost_tab_open or ghost_browser_tabs)".into());
    }
    Ok((browser, tab))
}

#[cfg(windows)]
fn desktop_ref(p: &Value) -> (String, isize) {
    (
        p["desktop"].as_str().unwrap_or("default").to_string(),
        p["hwnd"].as_i64().unwrap_or(0) as isize,
    )
}

#[cfg(windows)]
fn desktop_window(p: &Value) -> std::result::Result<(String, isize), String> {
    let (id, hwnd) = desktop_ref(p);
    if hwnd == 0 {
        return Err("missing param: hwnd (get one from ghost_desktop_windows or \
                    ghost_desktop_wait_for_window)"
            .into());
    }
    Ok((id, hwnd))
}

/// Optional string array parameter, tolerant of a bare string.
fn str_array(p: &Value, key: &str) -> Vec<String> {
    match &p[key] {
        Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        Value::String(s) => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
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

fn bg_ok() -> Value {
    json!({ "ok": true, "background": true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_covers_every_advertised_tool() {
        // A tool advertised in schemas() but not claimed by owns() would be
        // routed to the legacy table and answered "unknown method".
        for t in schemas() {
            let name = t["name"].as_str().unwrap();
            assert!(owns(name), "'{name}' is advertised but not owned by ext");
        }
    }

    #[test]
    fn every_schema_has_name_and_input() {
        for t in schemas() {
            assert!(t["name"].as_str().is_some());
            assert!(t["inputSchema"]["type"] == "object");
        }
    }

    #[test]
    fn base64_matches_rfc_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }
}
