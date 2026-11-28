//! MCP tool annotations (`title`, `readOnlyHint`, `destructiveHint`,
//! `idempotentHint`, `openWorldHint`) for every advertised tool.
//!
//! The hints are derived from the same Observe / Act / Control classes the
//! emergency stop uses (`tool_class` in main.rs), and a test holds them to it:
//! a tool is read-only only if every one of its operations is Observe. A
//! client may skip a confirmation prompt for a read-only tool, so a wrong
//! `readOnlyHint: true` would be a safety bug, not a cosmetic one.
//!
//! `destructiveHint` errs toward true: anything that types, clicks, closes,
//! runs commands or sends requests may change or remove the user's data.
//! `openWorldHint` is true for tools that reach the network (HTTP, browser
//! tabs) or run arbitrary programs (shell, flows).

use serde_json::{json, Value};

/// title, read-only, destructive, idempotent, open-world.
type Hints = (&'static str, bool, bool, bool, bool);

/// Every advertised tool, in the stop classes' order.
pub(crate) const TABLE: &[(&str, Hints)] = &[
    // Observe: reads only, answers even while stopped.
    ("ghost_see", ("See a window's controls or text", true, false, true, false)),
    ("ghost_find", ("Find an element", true, false, true, false)),
    ("ghost_snapshot", ("Snapshot a window with element ids", true, false, true, false)),
    ("ghost_query", ("Query a window's elements", true, false, true, false)),
    ("ghost_assert", ("Assert a condition", true, false, true, false)),
    ("ghost_screenshot", ("Take a screenshot", true, false, true, false)),
    ("ghost_stats", ("Server statistics", true, false, true, false)),
    ("ghost_session_state", ("Session state and focus audit", true, false, true, false)),
    ("ghost_focus_policy", ("Read the focus policy", true, false, true, false)),
    ("ghost_http_get", ("HTTP GET", true, false, true, true)),
    ("ghost_browser_tabs", ("List a browser's tabs", true, false, true, true)),
    ("ghost_browser_list_installed", ("List installed browsers", true, false, true, false)),
    ("ghost_tab_text", ("Read a tab's text", true, false, true, true)),
    ("ghost_tab_screenshot", ("Screenshot a tab", true, false, true, true)),
    ("ghost_tab_describe", ("Describe a tab's page", true, false, true, true)),
    ("ghost_tab_find", ("Find an element in a tab", true, false, true, true)),
    ("ghost_tab_wait_for", ("Wait for a tab condition", true, false, true, true)),
    ("ghost_desktop_windows", ("List a hidden desktop's windows", true, false, true, false)),
    ("ghost_desktop_wait_for_window", ("Wait for a window on a hidden desktop", true, false, true, false)),
    ("ghost_desktop_capture", ("Capture a hidden-desktop window", true, false, true, false)),
    ("ghost_desktop_describe", ("Describe a hidden-desktop window", true, false, true, false)),
    // Read or act depending on op / for / policy.
    ("ghost_wait", ("Wait for a condition", false, true, false, false)),
    ("ghost_window", ("List, launch and manage windows", false, true, false, false)),
    ("ghost_clipboard", ("Read or set the clipboard", false, true, true, false)),
    ("ghost_shell", ("Run shell commands", false, true, false, true)),
    ("ghost_set_focus_policy", ("Set the focus policy", false, false, true, false)),
    // Control: part of stopping safely.
    ("ghost_stop", ("Emergency stop", false, true, true, false)),
    ("ghost_reset", ("Lift a stop an agent raised", false, false, true, false)),
    ("ghost_browser_close", ("Close a browser", false, true, true, true)),
    ("ghost_tab_close", ("Close a tab", false, true, true, true)),
    ("ghost_desktop_close", ("Close a hidden desktop", false, true, true, false)),
    // Act: changes something.
    ("ghost_act", ("Click or type on an element", false, true, false, false)),
    ("ghost_key", ("Press keys", false, true, false, false)),
    ("ghost_scroll", ("Scroll", false, false, false, false)),
    ("ghost_drag", ("Drag", false, true, false, false)),
    ("ghost_run", ("Run a multi-step flow", false, true, false, true)),
    ("ghost_http_post", ("HTTP POST", false, true, false, true)),
    ("ghost_browser_launch", ("Launch a browser", false, false, false, true)),
    ("ghost_browser_attach", ("Attach to a running browser", false, false, false, true)),
    ("ghost_tab_open", ("Open a tab", false, false, false, true)),
    ("ghost_tab_navigate", ("Navigate a tab", false, true, true, true)),
    ("ghost_tab_click", ("Click in a tab", false, true, false, true)),
    ("ghost_tab_type", ("Type in a tab", false, true, false, true)),
    ("ghost_tab_press", ("Press a key in a tab", false, true, false, true)),
    ("ghost_tab_eval", ("Run JavaScript in a tab", false, true, false, true)),
    ("ghost_tab_scroll", ("Scroll a tab", false, false, false, true)),
    ("ghost_tab_select_option", ("Choose a select option in a tab", false, true, true, true)),
    ("ghost_desktop_create", ("Create a hidden desktop", false, false, false, false)),
    ("ghost_desktop_launch", ("Launch a program on a hidden desktop", false, false, false, false)),
    ("ghost_desktop_click", ("Click on a hidden desktop", false, true, false, false)),
    ("ghost_desktop_scroll", ("Scroll on a hidden desktop", false, false, false, false)),
    ("ghost_desktop_type", ("Type on a hidden desktop", false, true, false, false)),
    ("ghost_desktop_press", ("Press a key on a hidden desktop", false, true, false, false)),
    ("ghost_desktop_shortcut", ("Send a shortcut on a hidden desktop", false, true, false, false)),
];

pub(crate) fn hints(name: &str) -> Option<Hints> {
    TABLE.iter().find(|(n, _)| *n == name).map(|(_, h)| *h)
}

/// Adds `title` and `annotations` to each tool in a tools/list array. A tool
/// missing from the table is left as it was (a test fails on it instead).
pub(crate) fn annotate(tools: &mut Value) {
    let Some(arr) = tools.as_array_mut() else { return };
    for tool in arr {
        let Some((title, read_only, destructive, idempotent, open_world)) =
            tool["name"].as_str().and_then(hints)
        else {
            continue;
        };
        tool["title"] = json!(title);
        tool["annotations"] = json!({
            "title": title,
            "readOnlyHint": read_only,
            "destructiveHint": destructive,
            "idempotentHint": idempotent,
            "openWorldHint": open_world,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::hints;
    use crate::{tool_class, tools_schema, ToolClass};
    use serde_json::{json, Value};

    /// Every argument combination of a tool that changes its stop class.
    fn variants(tool: &Value) -> Vec<Value> {
        let mut out = vec![json!({})];
        for key in ["op", "for", "policy"] {
            if let Some(values) = tool["inputSchema"]["properties"][key]["enum"].as_array() {
                for v in values {
                    out.push(json!({ key: v }));
                }
            }
        }
        out
    }

    #[test]
    fn every_advertised_tool_carries_a_title_and_all_four_hints() {
        let tools = tools_schema();
        for t in tools.as_array().unwrap() {
            let name = t["name"].as_str().unwrap();
            let a = &t["annotations"];
            assert!(t["title"].as_str().is_some_and(|s| !s.is_empty()), "{name}: no title");
            assert_eq!(a["title"], t["title"], "{name}: annotations.title differs from title");
            for hint in ["readOnlyHint", "destructiveHint", "idempotentHint", "openWorldHint"] {
                assert!(a[hint].is_boolean(), "{name}: {hint} missing");
            }
        }
    }

    /// readOnlyHint is true exactly when every operation of the tool is
    /// Observe in the emergency-stop classification.
    #[test]
    fn read_only_matches_the_stop_classification_for_every_operation() {
        for t in tools_schema().as_array().unwrap() {
            let name = t["name"].as_str().unwrap();
            let all_observe = variants(t)
                .iter()
                .all(|args| tool_class(name, args) == Some(ToolClass::Observe));
            assert_eq!(t["annotations"]["readOnlyHint"], json!(all_observe), "{name}");
        }
    }

    #[test]
    fn read_only_tools_are_never_destructive_and_closers_always_are() {
        for t in tools_schema().as_array().unwrap() {
            let name = t["name"].as_str().unwrap();
            let a = &t["annotations"];
            if a["readOnlyHint"] == json!(true) {
                assert_eq!(a["destructiveHint"], json!(false), "{name}");
            }
        }
        for name in ["ghost_stop", "ghost_shell", "ghost_window", "ghost_tab_close", "ghost_browser_close",
                     "ghost_desktop_close", "ghost_http_post", "ghost_tab_eval", "ghost_act", "ghost_key"] {
            assert_eq!(hints(name).map(|h| h.2), Some(true), "{name} must be destructive");
        }
    }

    /// The table names only tools the server dispatches, so it cannot drift
    /// into labelling a tool that no longer exists.
    #[test]
    fn every_table_entry_is_a_dispatchable_tool() {
        let advertised: Vec<String> = tools_schema().as_array().unwrap().iter()
            .map(|t| t["name"].as_str().unwrap().to_string()).collect();
        for name in &advertised {
            assert!(hints(name).is_some(), "{name} has no annotations entry");
        }
        for (name, _) in super::TABLE {
            assert!(tool_class(name, &json!({})).is_some(), "{name} is in the table but not dispatchable");
        }
    }
}
