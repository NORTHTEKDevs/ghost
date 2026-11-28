//! Which inspection tool to pick. Six tools answer "what is on screen" in
//! different shapes; each description ends with one sentence that says when
//! to use it and which sibling to use instead, so an agent can choose by
//! reading one description rather than all six.

use serde_json::Value;

/// (tool, sentence appended to its description).
pub(crate) const SIBLING_HINTS: &[(&str, &str)] = &[
    ("ghost_see", "Use it to read what a window contains; use ghost_snapshot for stable element ids to plan several actions, ghost_find to locate one element, and ghost_screenshot only when pixels matter."),
    ("ghost_snapshot", "Use it instead of ghost_see when planning several actions against stable ids; for a quick read of a window, ghost_see is enough."),
    ("ghost_find", "Use it to see where an element is or how many match before acting; ghost_act already finds and acts in one call."),
    ("ghost_query", "Use it to pull a few named fields' values into one object; use ghost_see to read a whole window."),
    ("ghost_screenshot", "Use it when pixels matter (charts, canvas, images); for controls and text, ghost_see is cheaper."),
    ("ghost_assert", "Use it to turn 'did it work' into a pass or fail after acting; use ghost_wait when the condition may take time to appear."),
];

/// Appends each tool's hint to its description.
pub(crate) fn add(tools: &mut Value) {
    let Some(arr) = tools.as_array_mut() else { return };
    for tool in arr {
        let Some((_, hint)) = SIBLING_HINTS.iter().find(|(n, _)| tool["name"] == *n) else { continue };
        if let Some(desc) = tool["description"].as_str() {
            tool["description"] = Value::String(format!("{} {hint}", desc.trim_end()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SIBLING_HINTS;
    use crate::tools_schema;

    const INSPECTION: [&str; 6] = ["ghost_see", "ghost_snapshot", "ghost_find", "ghost_query", "ghost_screenshot", "ghost_assert"];

    /// Each overlapping inspection tool names at least one sibling to use
    /// instead, and every sibling it names is a tool the server advertises.
    #[test]
    fn every_inspection_tool_points_to_its_siblings() {
        let tools = tools_schema();
        let names: Vec<&str> = tools.as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        for tool in INSPECTION {
            let desc = tools.as_array().unwrap().iter().find(|t| t["name"] == tool).unwrap()["description"]
                .as_str().unwrap().to_string();
            let named: Vec<&str> = names.iter().copied().filter(|n| *n != tool && desc.contains(n)).collect();
            assert!(!named.is_empty(), "{tool} names no sibling to use instead");
        }
        for (tool, hint) in SIBLING_HINTS {
            assert!(names.contains(tool), "{tool} is not advertised");
            for word in hint.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                if word.starts_with("ghost_") {
                    assert!(names.contains(&word), "{tool}'s hint names {word}, which is not advertised");
                }
            }
        }
    }
}
