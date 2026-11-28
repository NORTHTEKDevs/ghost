//! Descriptions for tool parameters whose schema does not describe them.
//!
//! Each entry restates what the tool's own description and handler already
//! do (defaults are the handler's); nothing here changes a parameter's name,
//! type, requiredness or behaviour. A test holds every advertised parameter
//! to having a description, so a new parameter cannot ship undocumented.

use serde_json::Value;

const BROWSER: &str = "Browser id from ghost_browser_launch or ghost_browser_attach (default 'default')";
const TAB: &str = "Tab id from ghost_tab_open or ghost_browser_tabs";
const DESKTOP: &str = "Hidden desktop id from ghost_desktop_create (default 'default')";
const HWND: &str = "Window handle from ghost_desktop_windows or ghost_desktop_wait_for_window";
const NAME: &str = "Accessible name of the target element (substring match)";
const ROLE: &str = "UI Automation role of the target element, e.g. button, edit";

/// (tool, parameter, description).
pub(crate) const PARAM_DOCS: &[(&str, &str, &str)] = &[
    ("ghost_act", "name", NAME),
    ("ghost_act", "role", ROLE),
    ("ghost_act", "text", "Match the element by its visible text instead of name/role (the text to type is text_input)"),
    ("ghost_act", "description", "Natural-language description of the element; needs vision and a window facing the screen, so prefer name/role"),
    ("ghost_act", "mode", "Dispatch mode: instant (default, local tiers, escalates if needed), deliberate, or instant_only (never escalates)"),
    ("ghost_scroll", "direction", "Scroll direction"),
    ("ghost_scroll", "amount", "Wheel notches to scroll (default 3)"),
    ("ghost_scroll", "x", "Screen x of the real pointer for coordinate mode (needs a foreground policy)"),
    ("ghost_scroll", "y", "Screen y of the real pointer for coordinate mode (needs a foreground policy)"),
    ("ghost_drag", "from_x", "Start x in screen coordinates (or use from_name/from_role)"),
    ("ghost_drag", "from_y", "Start y in screen coordinates"),
    ("ghost_drag", "to_x", "End x in screen coordinates (or use to_name/to_role)"),
    ("ghost_drag", "to_y", "End y in screen coordinates"),
    ("ghost_assert", "predicate", "What to check: text-present/text-absent (OCR), element-exists, value-equals/value-contains (element value against text)"),
    ("ghost_run", "max_retries", "Retries for a failing read-only step (default 2, max 10); acting steps retry only with retry:true"),
    ("ghost_run", "stop_on_error", "Stop at the first failed step (default true); false runs the remaining steps and reports each result"),
    ("ghost_screenshot", "jpeg_quality", "JPEG quality 0-100 (default 75)"),
    ("ghost_http_get", "url", "http(s) URL to fetch; loopback, private, link-local and reserved addresses are refused"),
    ("ghost_http_get", "headers", "Extra request headers as an object of name: value strings (Host cannot be set)"),
    ("ghost_http_post", "url", "http(s) URL to post to; loopback, private, link-local and reserved addresses are refused"),
    ("ghost_http_post", "body", "Request body, sent as given"),
    ("ghost_http_post", "content_type", "Content-Type of the body (default application/json)"),
    ("ghost_http_post", "headers", "Extra request headers as an object of name: value strings (Host cannot be set)"),
    ("ghost_browser_launch", "browser", "Which installed browser to launch (default: the first one installed)"),
    ("ghost_browser_launch", "mode", "headless (invisible, default) or windowed (a real window on a hidden desktop, never on your screen)"),
    ("ghost_browser_attach", "id", "Handle for later calls (default 'default')"),
    ("ghost_browser_attach", "port", "The --remote-debugging-port the running browser was started with"),
    ("ghost_browser_close", "id", "Browser id to close (default 'default'); an attached browser is only disconnected"),
    ("ghost_browser_tabs", "id", "Browser id (default 'default')"),
    ("ghost_tab_open", "browser", BROWSER),
    ("ghost_tab_open", "url", "http(s) URL or about:blank (default); other schemes are refused"),
    ("ghost_tab_navigate", "browser", BROWSER),
    ("ghost_tab_navigate", "tab", TAB),
    ("ghost_tab_navigate", "url", "http(s) URL or about:blank; other schemes are refused"),
    ("ghost_tab_click", "browser", BROWSER),
    ("ghost_tab_click", "tab", TAB),
    ("ghost_tab_click", "selector", "CSS selector of the element to click"),
    ("ghost_tab_click", "timeout_ms", "How long to wait for the element, 1 to 600000 (default 10000)"),
    ("ghost_tab_type", "browser", BROWSER),
    ("ghost_tab_type", "tab", TAB),
    ("ghost_tab_type", "selector", "CSS selector of the field to type into"),
    ("ghost_tab_type", "text", "Text to type"),
    ("ghost_tab_press", "browser", BROWSER),
    ("ghost_tab_press", "tab", TAB),
    ("ghost_tab_press", "key", "Key to send to the focused element: Enter, Tab, ArrowDown, a, ..."),
    ("ghost_tab_press", "modifiers", "Modifier keys held during the press: Alt, Ctrl, Meta, Shift"),
    ("ghost_tab_text", "browser", BROWSER),
    ("ghost_tab_text", "tab", TAB),
    ("ghost_tab_text", "selector", "CSS selector of the element to read (default: the whole page)"),
    ("ghost_tab_eval", "browser", BROWSER),
    ("ghost_tab_eval", "tab", TAB),
    ("ghost_tab_eval", "expression", "JavaScript expression to evaluate in the page; promises are awaited"),
    ("ghost_tab_screenshot", "browser", BROWSER),
    ("ghost_tab_screenshot", "tab", TAB),
    ("ghost_tab_screenshot", "full_page", "Capture the whole scrollable page, not just the viewport (default false)"),
    ("ghost_tab_describe", "browser", BROWSER),
    ("ghost_tab_describe", "tab", TAB),
    ("ghost_tab_describe", "limit", "Most elements to return (default 120)"),
    ("ghost_tab_scroll", "browser", BROWSER),
    ("ghost_tab_scroll", "tab", TAB),
    ("ghost_tab_scroll", "dx", "Horizontal scroll in pixels (default 0)"),
    ("ghost_tab_scroll", "dy", "Vertical scroll in pixels, positive is down (default 0)"),
    ("ghost_tab_scroll", "selector", "CSS selector of a scrollable element (default: the page)"),
    ("ghost_tab_select_option", "browser", BROWSER),
    ("ghost_tab_select_option", "tab", TAB),
    ("ghost_tab_select_option", "selector", "CSS selector of the <select> element"),
    ("ghost_tab_select_option", "value", "Value of the option to choose"),
    ("ghost_tab_wait_for", "browser", BROWSER),
    ("ghost_tab_wait_for", "tab", TAB),
    ("ghost_tab_wait_for", "selector", "CSS selector to wait for"),
    ("ghost_tab_wait_for", "timeout_ms", "How long to wait, 1 to 600000 (default 15000)"),
    ("ghost_tab_close", "browser", BROWSER),
    ("ghost_tab_close", "tab", TAB),
    ("ghost_tab_find", "browser", BROWSER),
    ("ghost_tab_find", "query", "Substring to look for in each tab's URL or title"),
    ("ghost_set_focus_policy", "policy", "background (always allowed, the default), prefer_background or foreground (refused while the focus lock is on)"),
    ("ghost_desktop_create", "id", "Id for the new hidden desktop (default 'default')"),
    ("ghost_desktop_close", "id", "Hidden desktop to close (default 'default'); processes still on it are ended"),
    ("ghost_desktop_launch", "desktop", DESKTOP),
    ("ghost_desktop_launch", "command", "Program to start on the hidden desktop, with any arguments"),
    ("ghost_desktop_windows", "desktop", DESKTOP),
    ("ghost_desktop_wait_for_window", "desktop", DESKTOP),
    ("ghost_desktop_wait_for_window", "title", "Title substring to wait for"),
    ("ghost_desktop_wait_for_window", "timeout_ms", "How long to wait, 1 to 600000 (default 20000)"),
    ("ghost_desktop_click", "desktop", DESKTOP),
    ("ghost_desktop_click", "hwnd", HWND),
    ("ghost_desktop_click", "x", "Client-area x of the point to click"),
    ("ghost_desktop_click", "y", "Client-area y of the point to click"),
    ("ghost_desktop_click", "button", "left (default), right or double"),
    ("ghost_desktop_scroll", "desktop", DESKTOP),
    ("ghost_desktop_scroll", "hwnd", HWND),
    ("ghost_desktop_scroll", "direction", "Scroll direction"),
    ("ghost_desktop_scroll", "amount", "Wheel notches to scroll (default 3)"),
    ("ghost_desktop_type", "desktop", DESKTOP),
    ("ghost_desktop_type", "hwnd", HWND),
    ("ghost_desktop_type", "text", "Text to type into the window"),
    ("ghost_desktop_press", "desktop", DESKTOP),
    ("ghost_desktop_press", "hwnd", HWND),
    ("ghost_desktop_press", "key", "Key name to send, e.g. Enter, Tab, F5"),
    ("ghost_desktop_shortcut", "desktop", DESKTOP),
    ("ghost_desktop_shortcut", "hwnd", HWND),
    ("ghost_desktop_shortcut", "shortcut", "undo, cut, copy, paste, select_all, or a 'Ctrl+Z' style combination"),
    ("ghost_desktop_capture", "desktop", DESKTOP),
    ("ghost_desktop_capture", "hwnd", HWND),
    ("ghost_desktop_describe", "desktop", DESKTOP),
    ("ghost_desktop_describe", "window", "Title substring of the window to describe"),
];

/// Fills in each listed parameter's description where its schema has none.
/// An existing description always wins.
pub(crate) fn document(tools: &mut Value) {
    let Some(arr) = tools.as_array_mut() else { return };
    for tool in arr {
        let name = tool["name"].as_str().unwrap_or_default().to_string();
        let Some(props) = tool["inputSchema"]["properties"].as_object_mut() else { continue };
        for (param, spec) in props.iter_mut() {
            if spec.get("description").and_then(Value::as_str).is_some_and(|d| !d.is_empty()) {
                continue;
            }
            if let Some((_, _, text)) = PARAM_DOCS.iter().find(|(t, p, _)| *t == name && p == param) {
                spec["description"] = Value::String((*text).to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PARAM_DOCS;
    use crate::tools_schema;

    /// Every parameter an agent sees in tools/list says what it is for.
    #[test]
    fn every_advertised_parameter_has_a_description() {
        let mut missing = Vec::new();
        for t in tools_schema().as_array().unwrap() {
            let name = t["name"].as_str().unwrap();
            if let Some(props) = t["inputSchema"]["properties"].as_object() {
                for (param, spec) in props {
                    if spec["description"].as_str().is_none_or(|d| d.trim().is_empty()) {
                        missing.push(format!("{name}.{param}"));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "{} parameters have no description: {missing:?}", missing.len());
    }

    /// The table only fills gaps: an entry for a parameter the schema already
    /// describes, or that no longer exists, is a stale entry.
    #[test]
    fn every_table_entry_fills_a_real_gap() {
        let tools = crate::raw_tools_schema();
        for (tool, param, _) in PARAM_DOCS {
            let t = tools.as_array().unwrap().iter().find(|t| t["name"] == *tool);
            let Some(t) = t else { continue }; // a Windows-only tool on another OS
            let spec = &t["inputSchema"]["properties"][*param];
            assert!(!spec.is_null(), "{tool}.{param} is not a parameter");
            assert!(spec["description"].as_str().is_none_or(|d| d.is_empty()), "{tool}.{param} is already described in its schema");
        }
    }
}
