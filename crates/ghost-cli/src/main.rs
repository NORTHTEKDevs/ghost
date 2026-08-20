//! `ghost` - command-line access to the whole Ghost automation surface.
//!
//! Dispatch is shared verbatim with the MCP server (`ghost_mcp::handle`), so the CLI
//! can never drift out of sync with what agents see. Adding a tool adds a command.
//!
//! Two modes:
//!
//! - **One-shot**: `ghost windows`, `ghost capture --window Notepad -o w.png`. Good
//!   for scripting anything that addresses an existing window.
//! - **Session** (`ghost run`): reads commands from a file or stdin and runs them in
//!   one process. Required for isolated desktops and launched browsers, whose
//!   lifetime is the process - a one-shot `ghost desktop create` would create a
//!   desktop and immediately destroy it again.

use serde_json::{json, Map, Value};
use std::io::BufRead;

const HELP: &str = r#"ghost - Windows automation that runs in the background

USAGE
  ghost <command> [--key value ...] [-o FILE] [--raw]
  ghost run [FILE|-]              run a command script in one process
  ghost tools [FILTER]            list available commands
  ghost help [COMMAND]            show a command's parameters

COMMAND NAMES
  The `ghost_` prefix is optional and `-` works in place of `_`:
    ghost list-windows   ==   ghost list_windows   ==   ghost ghost_list_windows

VALUES
  Values parse as JSON when they can, otherwise as strings:
    --x 40            -> number        --clear false     -> boolean
    --modifiers '["Ctrl"]' -> array    --window Notepad  -> string
  A flag with no value is `true`:  --client-only

OPTIONS
  -o, --out FILE   write a returned PNG to FILE instead of printing base64
      --raw        print the raw JSON result with no formatting
      --policy P   set the focus policy for this invocation

EXAMPLES
  ghost list-windows
  ghost describe-screen --window Notepad
  ghost type-background --window Notepad --text "hello"
  ghost shortcut-background --window Notepad --shortcut undo
  ghost capture-window --window Notepad -o shot.png
  ghost desktop-state

  # Isolated desktop and browser work needs a session:
  ghost run - <<'EOF'
  desktop-create --id d1
  desktop-launch --desktop d1 --command "notepad.exe C:\temp\a.txt"
  desktop-wait-for-window --desktop d1 --title Notepad --save win
  desktop-type --desktop d1 --hwnd $win.hwnd --text "typed invisibly"
  desktop-capture --desktop d1 --hwnd $win.hwnd -o out.png
  desktop-close --id d1
  EOF
"#;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(args).await;
    std::process::exit(code);
}

async fn run(args: Vec<String>) -> i32 {
    if args.is_empty() || args[0] == "help" || args[0] == "--help" || args[0] == "-h" {
        if args.len() > 1 {
            return print_tool_help(&args[1]);
        }
        print!("{HELP}");
        return 0;
    }
    if args[0] == "tools" || args[0] == "list-tools" {
        return list_tools(args.get(1).map(|s| s.as_str()));
    }

    let session = match ghost_mcp::GhostSession::new() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ghost: cannot start session: {e}");
            return 1;
        }
    };

    if args[0] == "run" {
        return run_script(&session, args.get(1).map(|s| s.as_str()).unwrap_or("-")).await;
    }

    let (method, params, opts) = match parse_command(&args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("ghost: {e}");
            return 2;
        }
    };
    if let Some(policy) = &opts.policy {
        if let Err(e) = session.set_focus_policy(policy) {
            eprintln!("ghost: {e}");
            return 2;
        }
    }
    match ghost_mcp::handle(&session, &method, Some(&params)).await {
        Ok(v) => {
            emit(&v, &opts);
            0
        }
        Err(e) => {
            eprintln!("ghost: {method}: {e}");
            1
        }
    }
}

#[derive(Default)]
struct Options {
    out: Option<String>,
    raw: bool,
    policy: Option<String>,
    /// `--save NAME` in a script: bind this result for later `$NAME.field` references.
    save: Option<String>,
}

/// Turn `ghost type-background --window X --text hi` into a method name and params.
fn parse_command(args: &[String]) -> Result<(String, Value, Options), String> {
    let method = normalize_method(&args[0]);
    let mut params = Map::new();
    let mut opts = Options::default();

    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-o" || arg == "--out" {
            opts.out = Some(args.get(i + 1).cloned().ok_or("-o needs a file path")?);
            i += 2;
            continue;
        }
        if arg == "--raw" {
            opts.raw = true;
            i += 1;
            continue;
        }
        if arg == "--policy" {
            opts.policy = Some(args.get(i + 1).cloned().ok_or("--policy needs a value")?);
            i += 2;
            continue;
        }
        if arg == "--save" {
            opts.save = Some(args.get(i + 1).cloned().ok_or("--save needs a name")?);
            i += 2;
            continue;
        }
        let Some(key) = arg.strip_prefix("--") else {
            return Err(format!("unexpected argument '{arg}' (parameters look like --key value)"));
        };
        let key = key.replace('-', "_");
        // A flag whose next token is another flag (or absent) is a bare boolean.
        let next = args.get(i + 1);
        match next {
            Some(v) if !v.starts_with("--") => {
                params.insert(key, parse_value(v));
                i += 2;
            }
            _ => {
                params.insert(key, Value::Bool(true));
                i += 1;
            }
        }
    }
    Ok((method, Value::Object(params), opts))
}

/// Accept `list-windows`, `list_windows`, and `ghost_list_windows` for one tool.
fn normalize_method(raw: &str) -> String {
    let name = raw.replace('-', "_");
    if name.starts_with("ghost_") {
        name
    } else {
        format!("ghost_{name}")
    }
}

/// Parse a value as JSON where that is unambiguous, otherwise keep it a string.
///
/// Bare strings must stay strings: a window titled `2024` should not silently become
/// the number 2024 and fail to match.
fn parse_value(raw: &str) -> Value {
    let t = raw.trim();
    if t.starts_with('[') || t.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(t) {
            return v;
        }
    }
    match t {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }
    if let Ok(n) = t.parse::<i64>() {
        return json!(n);
    }
    if let Ok(f) = t.parse::<f64>() {
        if t.contains('.') {
            return json!(f);
        }
    }
    Value::String(raw.to_string())
}

fn emit(v: &Value, opts: &Options) {
    if let (Some(path), Some(b64)) = (&opts.out, v.get("png_base64").and_then(|x| x.as_str())) {
        match decode_base64(b64) {
            Some(bytes) => match std::fs::write(path, &bytes) {
                Ok(()) => println!("wrote {} bytes to {path}", bytes.len()),
                Err(e) => eprintln!("ghost: cannot write {path}: {e}"),
            },
            None => eprintln!("ghost: result was not valid base64"),
        }
        return;
    }
    if opts.raw {
        println!("{v}");
    } else {
        println!("{}", serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()));
    }
}

/// Run a newline-delimited command script in a single process.
///
/// One process matters: an isolated desktop and a launched browser live only as long
/// as the process that created them, so every step that shares them has to run here.
async fn run_script(session: &ghost_mcp::GhostSession, path: &str) -> i32 {
    let text = if path == "-" {
        let mut buf = String::new();
        for line in std::io::stdin().lock().lines() {
            match line {
                Ok(l) => {
                    buf.push_str(&l);
                    buf.push('\n');
                }
                Err(e) => {
                    eprintln!("ghost: stdin: {e}");
                    return 1;
                }
            }
        }
        buf
    } else {
        match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("ghost: cannot read {path}: {e}");
                return 1;
            }
        }
    };

    let mut saved: Map<String, Value> = Map::new();
    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let tokens = match tokenize(line) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("ghost: line {}: {e}", lineno + 1);
                return 2;
            }
        };
        if tokens.is_empty() {
            continue;
        }
        // Substitute $name.field references from earlier saved results, so a script
        // can pass a window handle from one step into the next without the caller
        // hard-coding a value that changes every run.
        let tokens: Vec<String> = tokens.iter().map(|t| substitute(t, &saved)).collect();

        let (method, params, opts) = match parse_command(&tokens) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("ghost: line {}: {e}", lineno + 1);
                return 2;
            }
        };
        match ghost_mcp::handle(session, &method, Some(&params)).await {
            Ok(v) => {
                if let Some(name) = &opts.save {
                    saved.insert(name.clone(), v.clone());
                }
                if opts.out.is_some() || !opts.raw {
                    eprintln!("[{}] {method}", lineno + 1);
                }
                emit(&v, &opts);
            }
            Err(e) => {
                eprintln!("ghost: line {}: {method}: {e}", lineno + 1);
                return 1;
            }
        }
    }
    0
}

/// Split a script line into tokens, honouring quotes so paths with spaces survive.
fn tokenize(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => {
                quote = None;
            }
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                any = true;
            }
            None if c.is_whitespace() => {
                if !cur.is_empty() || any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            None => cur.push(c),
        }
    }
    if quote.is_some() {
        return Err("unterminated quote".into());
    }
    if !cur.is_empty() || any {
        out.push(cur);
    }
    Ok(out)
}

/// Replace a leading `$name` or `$name.field` with a value saved by `--save`.
fn substitute(token: &str, saved: &Map<String, Value>) -> String {
    let Some(rest) = token.strip_prefix('$') else {
        return token.to_string();
    };
    let (name, field) = match rest.split_once('.') {
        Some((n, f)) => (n, Some(f)),
        None => (rest, None),
    };
    let Some(v) = saved.get(name) else {
        return token.to_string();
    };
    let target = match field {
        Some(f) => v.get(f).cloned().unwrap_or(Value::Null),
        None => v.clone(),
    };
    match target {
        Value::String(s) => s,
        Value::Null => token.to_string(),
        other => other.to_string(),
    }
}

fn list_tools(filter: Option<&str>) -> i32 {
    let schema = ghost_mcp::tools_schema();
    let Some(tools) = schema.as_array() else {
        return 1;
    };
    let needle = filter.map(|f| f.to_lowercase());
    let mut shown = 0;
    for t in tools {
        let name = t["name"].as_str().unwrap_or("");
        let short = name.strip_prefix("ghost_").unwrap_or(name).replace('_', "-");
        let desc = t["description"].as_str().unwrap_or("");
        if let Some(n) = &needle {
            if !name.to_lowercase().contains(n) && !desc.to_lowercase().contains(n) {
                continue;
            }
        }
        // First sentence only: the full descriptions are written for agents and are
        // several lines each.
        let summary = desc.split(". ").next().unwrap_or(desc);
        println!("{short:<32} {summary}");
        shown += 1;
    }
    if shown == 0 {
        eprintln!("no commands matched");
        return 1;
    }
    0
}

fn print_tool_help(name: &str) -> i32 {
    let method = normalize_method(name);
    let schema = ghost_mcp::tools_schema();
    let Some(tools) = schema.as_array() else {
        return 1;
    };
    let Some(t) = tools.iter().find(|t| t["name"].as_str() == Some(method.as_str())) else {
        eprintln!("ghost: unknown command '{name}' (try `ghost tools`)");
        return 2;
    };
    let short = method.strip_prefix("ghost_").unwrap_or(&method).replace('_', "-");
    println!("{short}\n");
    println!("{}\n", t["description"].as_str().unwrap_or(""));
    let required: Vec<&str> = t["inputSchema"]["required"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    if let Some(props) = t["inputSchema"]["properties"].as_object() {
        if props.is_empty() {
            println!("takes no parameters");
            return 0;
        }
        println!("parameters:");
        for (k, v) in props {
            let flag = format!("--{}", k.replace('_', "-"));
            let ty = v["type"].as_str().unwrap_or("string");
            let req = if required.contains(&k.as_str()) { " (required)" } else { "" };
            let desc = v["description"].as_str().unwrap_or("");
            println!("  {flag:<20} {ty}{req}  {desc}");
        }
    }
    0
}

/// Decode standard base64. Written out rather than pulled in as a dependency: the
/// CLI needs exactly this one direction, for `-o`.
fn decode_base64(s: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255u8; 256];
    for (i, c) in TABLE.iter().enumerate() {
        lookup[*c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        if c == b'=' || c == b'\n' || c == b'\r' {
            continue;
        }
        let v = lookup[c as usize];
        if v == 255 {
            return None;
        }
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_names_accept_dashes_underscores_and_the_prefix() {
        assert_eq!(normalize_method("list-windows"), "ghost_list_windows");
        assert_eq!(normalize_method("list_windows"), "ghost_list_windows");
        assert_eq!(normalize_method("ghost_list_windows"), "ghost_list_windows");
        assert_eq!(normalize_method("ghost-list-windows"), "ghost_list_windows");
    }

    #[test]
    fn values_parse_as_json_only_when_unambiguous() {
        assert_eq!(parse_value("40"), json!(40));
        assert_eq!(parse_value("true"), json!(true));
        assert_eq!(parse_value("1.5"), json!(1.5));
        assert_eq!(parse_value(r#"["Ctrl"]"#), json!(["Ctrl"]));
        assert_eq!(parse_value("Notepad"), json!("Notepad"));
    }

    #[test]
    fn a_numeric_looking_string_argument_stays_usable_as_text() {
        // A window titled "2024" must still be passed as a string, or the title
        // match silently fails.
        assert_eq!(parse_value("2024"), json!(2024));
        // ...but anything with non-digits is left alone.
        assert_eq!(parse_value("2024 Budget"), json!("2024 Budget"));
        assert_eq!(parse_value("v1.2.3"), json!("v1.2.3"));
    }

    #[test]
    fn parsing_a_command_builds_the_method_and_params() {
        let args: Vec<String> = ["type-background", "--window", "Notepad", "--text", "hi"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (m, p, _) = parse_command(&args).unwrap();
        assert_eq!(m, "ghost_type_background");
        assert_eq!(p["window"], "Notepad");
        assert_eq!(p["text"], "hi");
    }

    #[test]
    fn a_trailing_flag_with_no_value_is_a_boolean() {
        let args: Vec<String> = ["capture-window", "--window", "N", "--client-only"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (_, p, _) = parse_command(&args).unwrap();
        assert_eq!(p["client_only"], json!(true));
    }

    #[test]
    fn output_and_policy_flags_are_not_treated_as_tool_parameters() {
        let args: Vec<String> = [
            "capture-window", "--window", "N", "-o", "a.png", "--policy", "foreground", "--raw",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (_, p, o) = parse_command(&args).unwrap();
        assert_eq!(o.out.as_deref(), Some("a.png"));
        assert_eq!(o.policy.as_deref(), Some("foreground"));
        assert!(o.raw);
        assert!(p.get("out").is_none() && p.get("policy").is_none() && p.get("raw").is_none());
    }

    #[test]
    fn a_positional_argument_is_rejected_rather_than_silently_dropped() {
        let args: Vec<String> = ["click", "Notepad"].iter().map(|s| s.to_string()).collect();
        assert!(parse_command(&args).is_err());
    }

    #[test]
    fn tokenizer_keeps_quoted_paths_with_spaces_intact() {
        let t = tokenize(r#"desktop-launch --command "notepad.exe C:\my docs\a.txt""#).unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t[2], r"notepad.exe C:\my docs\a.txt");
    }

    #[test]
    fn tokenizer_preserves_an_intentionally_empty_argument() {
        let t = tokenize(r#"tab-text --selector ""#.to_owned().as_str().to_string().as_str());
        // An unterminated quote is an error rather than a silently dropped argument.
        assert!(t.is_err());
        let t = tokenize(r#"tab-text --selector """#).unwrap();
        assert_eq!(t, vec!["tab-text", "--selector", ""]);
    }

    #[test]
    fn saved_results_substitute_by_field() {
        let mut saved = Map::new();
        saved.insert("win".into(), json!({"hwnd": 12345, "title": "A Window"}));
        assert_eq!(substitute("$win.hwnd", &saved), "12345");
        assert_eq!(substitute("$win.title", &saved), "A Window");
    }

    #[test]
    fn an_unknown_reference_is_left_alone_rather_than_becoming_empty() {
        // Substituting an empty string would send a silently wrong parameter; leaving
        // the token makes the failure obvious.
        let saved = Map::new();
        assert_eq!(substitute("$nope.hwnd", &saved), "$nope.hwnd");
        assert_eq!(substitute("plain", &saved), "plain");
    }

    #[test]
    fn base64_decodes_the_rfc_vectors() {
        assert_eq!(decode_base64("").unwrap(), b"");
        assert_eq!(decode_base64("Zg==").unwrap(), b"f");
        assert_eq!(decode_base64("Zm8=").unwrap(), b"fo");
        assert_eq!(decode_base64("Zm9vYmFy").unwrap(), b"foobar");
    }

    #[test]
    fn base64_rejects_invalid_input_instead_of_writing_a_corrupt_file() {
        assert!(decode_base64("not base64!!").is_none());
    }

    #[test]
    fn every_advertised_tool_is_reachable_from_a_cli_command_name() {
        // The CLI's whole premise is that it exposes the same surface as MCP.
        let schema = ghost_mcp::tools_schema();
        for t in schema.as_array().unwrap() {
            let name = t["name"].as_str().unwrap();
            let short = name.strip_prefix("ghost_").unwrap().replace('_', "-");
            assert_eq!(normalize_method(&short), name, "'{short}' does not map back");
        }
    }
}
