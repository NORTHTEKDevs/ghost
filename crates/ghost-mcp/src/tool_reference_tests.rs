//! docs/tools.md is generated from the server's own `tools/list` answer and
//! this test fails the moment the two drift. Regenerate on Windows with
//! `GHOST_WRITE_TOOL_REFERENCE=1 cargo test -p ghost-mcp tool_reference`.
//! On other platforms the Windows-only tools are compiled out, so there the
//! test only checks that every advertised tool is documented.

use crate::tools_schema;
use serde_json::Value;

const DOC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/tools.md");

fn group(name: &str) -> usize {
    if name.starts_with("ghost_browser_") || name.starts_with("ghost_tab_") {
        1
    } else if name.starts_with("ghost_desktop_") || name.contains("focus_policy") {
        2
    } else {
        0
    }
}

const GROUPS: [(&str, &str); 3] = [
    ("Desktop verbs", "Windows and Linux. Look, act, wait, assert, manage windows, run commands."),
    ("Browser and tabs", "Windows and Linux. Drive individual tabs of a Chromium-family browser over its DevTools port, in the background."),
    ("Hidden desktops and focus policy", "Windows only. Explicit control of isolated desktops the user never sees, and the focus policy."),
];

/// Whitespace runs (Rust string continuations carry indentation) become one space.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One line of markdown table text: pipes escaped.
fn cell(text: &str) -> String {
    flat(text).replace('|', "\\|")
}

fn param_type(spec: &Value) -> String {
    let base = spec["type"].as_str().unwrap_or("any").to_string();
    match spec["enum"].as_array() {
        Some(values) => {
            let names: Vec<String> = values.iter().map(|v| format!("`{}`", v.as_str().unwrap_or_default())).collect();
            format!("{base}: {}", names.join(", "))
        }
        None => match (spec["minimum"].as_i64(), spec["maximum"].as_i64()) {
            (Some(lo), Some(hi)) => format!("{base}, {lo} to {hi}"),
            (Some(lo), None) => format!("{base}, at least {lo}"),
            (None, Some(hi)) => format!("{base}, at most {hi}"),
            (None, None) => base,
        },
    }
}

/// The tool's MCP annotations as one line under its description.
fn annotation_line(a: &Value) -> String {
    let title = a["title"].as_str().unwrap_or_default();
    let mut tags = Vec::new();
    if a["readOnlyHint"] == true {
        tags.push("read-only");
    } else if a["destructiveHint"] == true {
        tags.push("may change or remove data");
    } else {
        tags.push("changes state, not destructive");
    }
    if a["idempotentHint"] == true {
        tags.push("idempotent");
    }
    if a["openWorldHint"] == true {
        tags.push("reaches the network or runs programs");
    }
    format!("\n**{title}**: {}.\n", tags.join(", "))
}

pub(crate) fn render_tool_reference(tools: &Value) -> String {
    let tools = tools.as_array().expect("tools/list is an array");
    let mut out = String::new();
    out.push_str("# Tool reference\n\n");
    out.push_str(&format!(
        "Every tool `ghost-mcp` advertises on Windows ({} tools), generated from the server's own \
         `tools/list` answer. A test in `crates/ghost-mcp/src/tool_reference_tests.rs` fails when \
         this page and the server disagree. Regenerate it on Windows with \
         `GHOST_WRITE_TOOL_REFERENCE=1 cargo test -p ghost-mcp tool_reference`.\n\n",
        tools.len()
    ));
    out.push_str("Every action returns `verified`; a call that cannot do what was asked returns an error naming why. \
                  Older tool names stay callable for existing clients but are not listed here.\n");
    for (index, (title, blurb)) in GROUPS.iter().enumerate() {
        let members: Vec<&Value> = tools.iter().filter(|t| group(t["name"].as_str().unwrap_or_default()) == index).collect();
        out.push_str(&format!("\n## {title}\n\n{blurb}\n\n"));
        for t in &members {
            let name = t["name"].as_str().unwrap_or_default();
            out.push_str(&format!("- [`{name}`](#{name})\n"));
        }
        for t in members {
            let name = t["name"].as_str().unwrap_or_default();
            out.push_str(&format!("\n### `{name}`\n\n{}\n", flat(t["description"].as_str().unwrap_or_default())));
            out.push_str(&annotation_line(&t["annotations"]));
            let schema = &t["inputSchema"];
            let Some(props) = schema["properties"].as_object().filter(|p| !p.is_empty()) else {
                out.push_str("\nNo parameters.\n");
                continue;
            };
            let required: Vec<&str> = schema["required"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            out.push_str("\n| Parameter | Type | Required | Description |\n| --- | --- | --- | --- |\n");
            for (param, spec) in props {
                out.push_str(&format!(
                    "| `{param}` | {} | {} | {} |\n",
                    cell(&param_type(spec)),
                    if required.contains(&param.as_str()) { "yes" } else { "" },
                    cell(spec["description"].as_str().unwrap_or_default())
                ));
            }
        }
    }
    out
}

#[test]
fn tool_reference_matches_tools_list() {
    let tools = tools_schema();
    if cfg!(windows) {
        let rendered = render_tool_reference(&tools);
        if std::env::var_os("GHOST_WRITE_TOOL_REFERENCE").is_some() {
            std::fs::write(DOC, &rendered).expect("write docs/tools.md");
        }
        let on_disk = std::fs::read_to_string(DOC).expect("docs/tools.md exists").replace("\r\n", "\n");
        assert!(
            on_disk == rendered,
            "docs/tools.md is out of date with tools/list; regenerate it with \
             GHOST_WRITE_TOOL_REFERENCE=1 cargo test -p ghost-mcp tool_reference"
        );
    } else {
        let on_disk = std::fs::read_to_string(DOC).expect("docs/tools.md exists");
        for t in tools.as_array().unwrap() {
            let name = t["name"].as_str().unwrap();
            assert!(on_disk.contains(&format!("### `{name}`")), "{name} is advertised but missing from docs/tools.md");
        }
    }
}
