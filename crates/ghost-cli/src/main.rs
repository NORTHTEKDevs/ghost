//! `ghost` command-line automation. Usable without Claude or any MCP client.
//!
//! Examples:
//!   ghost click --name "Submit"
//!   ghost type --role edit --text "hello"
//!   ghost screenshot --out shot.png
//!   ghost run intent.json
//!   ghost list-windows
//!   ghost describe --window "Notepad"
//!   ghost see
//!   ghost find --name "Submit"
//!   ghost act --name "Submit" --action click
//!   ghost key --keys "Ctrl+C"
//!   ghost window --op list
//!   ghost query --fields "title,status"
//!   ghost serve

use clap::{Parser, Subcommand};
use ghost_session::{By, GhostSession, LocateMode, Target};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "ghost",
    version,
    about = "Windows desktop automation - UIA + SendInput, no CDP, MCP-native",
    long_about = "Automate Windows apps via UI Automation and SendInput.\n\
                  Use subcommands for one-shot actions or `run` to execute a JSON intent."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Per-action timeout in milliseconds (default 5000).
    #[arg(long, global = true, default_value_t = 5000)]
    timeout_ms: u64,

    /// Print verbose tracing to stderr.
    #[arg(long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Command {
    // -----------------------------------------------------------------------
    // Legacy verbs (keep for backwards compat)
    // -----------------------------------------------------------------------

    /// Click a UI element by name or role.
    Click {
        #[arg(long)] name: Option<String>,
        #[arg(long)] role: Option<String>,
    },
    /// Click at absolute screen coordinates.
    ClickAt { x: i32, y: i32 },
    /// Type text into an element.
    Type {
        #[arg(long)] name: Option<String>,
        #[arg(long)] role: Option<String>,
        #[arg(long)] text: String,
    },
    /// Press a key: Enter, Tab, Escape, F5, ArrowUp, etc.
    Press { key: String },
    /// Press a hotkey combo. Example: ghost hotkey --mods Ctrl --key c
    Hotkey {
        #[arg(long, num_args = 1..)]
        mods: Vec<String>,
        #[arg(long)]
        key: String,
    },
    /// Capture primary monitor to a PNG file.
    Screenshot {
        #[arg(long, default_value = "ghost.png")]
        out: PathBuf,
    },
    /// Launch an executable. Prints the PID.
    Launch { exe: String },
    /// List visible top-level windows as JSON.
    ListWindows,
    /// Bring a window to the foreground by partial name match.
    FocusWindow { name: String },
    /// Set window state: maximize, minimize, restore, close.
    WindowState { name: String, state: String },
    /// Describe interactive elements in a window (or all windows) as JSON.
    Describe {
        #[arg(long)] window: Option<String>,
        /// Return only the delta since this sequence number.
        #[arg(long)] since_seq: Option<u64>,
    },
    /// Read the clipboard.
    GetClipboard,
    /// Write text to the clipboard.
    SetClipboard { text: String },
    /// Execute a JSON intent file or stdin (pass `-` for stdin).
    Run {
        /// Path to intent JSON, or `-` for stdin.
        path: String,
    },

    // -----------------------------------------------------------------------
    // Lean verbs (parity with MCP ghost_see / ghost_find / ghost_act / etc.)
    // -----------------------------------------------------------------------

    /// Describe the active screen's UI elements (lean alias for describe).
    /// mode=fast (default), full, delta.
    See {
        #[arg(long, default_value = "fast")]
        mode: String,
        #[arg(long)]
        window: Option<String>,
        #[arg(long)]
        since_seq: Option<u64>,
    },
    /// Ground a target via the cascade: cache→UIA→OCR→VLM. Returns center, rect, source.
    Find {
        #[arg(long)] name: Option<String>,
        #[arg(long)] role: Option<String>,
        #[arg(long)] description: Option<String>,
        #[arg(long)] text: Option<String>,
        /// Dispatch mode: instant (default), deliberate, instant_only.
        #[arg(long, default_value = "instant")]
        mode: String,
    },
    /// Atomic find→focus→action in one call. action: click|type|double_click|right_click|hover.
    Act {
        #[arg(long)] name: Option<String>,
        #[arg(long)] role: Option<String>,
        #[arg(long)] description: Option<String>,
        #[arg(long)] text: Option<String>,
        #[arg(long)]
        action: String,
        /// Text to type when action=type.
        #[arg(long)]
        text_input: Option<String>,
    },
    /// Key input. Single key: "Enter". Combo: "Ctrl+C". Hold: "down:Shift" / "up:Shift".
    Key {
        keys: String,
    },
    /// Window management. op: list|focus|state|launch.
    Window {
        #[arg(long, default_value = "list")]
        op: String,
        #[arg(long)] name: Option<String>,
        #[arg(long)] state: Option<String>,
        #[arg(long)] exe: Option<String>,
    },
    /// Extract structured data from the screen via UIA name-matching + VLM fallback.
    /// Specify field names as a comma-separated list.
    Query {
        /// Comma-separated field names to extract.
        #[arg(long)]
        fields: String,
        #[arg(long)]
        window: Option<String>,
    },
    /// Take a screenshot (lean alias). Output to --out file (PNG) or print JPEG base64 to stdout.
    Snapshot {
        #[arg(long)]
        out: Option<PathBuf>,
        /// Foreground window crop (default true).
        #[arg(long, default_value_t = true)]
        foreground: bool,
    },

    // -----------------------------------------------------------------------
    // Serve: run the MCP stdio server
    // -----------------------------------------------------------------------

    /// Start the ghost MCP stdio server. Executes the ghost-mcp binary found in PATH
    /// or adjacent to this binary. Use in stdio MCP client configurations.
    Serve {
        #[arg(long, default_value = "127.0.0.1:7878")]
        addr: String,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.verbose {
        tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    }

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ghost: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    // Serve is special — it exec/spawns ghost-mcp, no GhostSession needed.
    if let Command::Serve { .. } = &cli.command {
        return run_serve();
    }

    let session = GhostSession::new()
        .map_err(|e| e.to_string())?
        .with_timeout(cli.timeout_ms);

    match cli.command {
        // Legacy verbs ---------------------------------------------------
        Command::Click { name, role } => {
            let by = parse_by(name, role)?;
            let el = session.find(by).await.map_err(|e| e.to_string())?;
            el.click().map_err(|e| e.to_string())?;
            println!(r#"{{"ok":true}}"#);
        }
        Command::ClickAt { x, y } => {
            session.click_at(x, y).await.map_err(|e| e.to_string())?;
            println!(r#"{{"ok":true}}"#);
        }
        Command::Type { name, role, text } => {
            let by = parse_by(name, role)?;
            let el = session.find(by).await.map_err(|e| e.to_string())?;
            el.type_text(&text).map_err(|e| e.to_string())?;
            println!(r#"{{"ok":true}}"#);
        }
        Command::Press { key } => {
            session.press(&key).await.map_err(|e| e.to_string())?;
            println!(r#"{{"ok":true}}"#);
        }
        Command::Hotkey { mods, key } => {
            let mods: Vec<&str> = mods.iter().map(|s| s.as_str()).collect();
            session.hotkey(&mods, &key).await.map_err(|e| e.to_string())?;
            println!(r#"{{"ok":true}}"#);
        }
        Command::Screenshot { out } => {
            let png = session.screenshot(ghost_session::Region::full()).await
                .map_err(|e| e.to_string())?;
            std::fs::write(&out, &png).map_err(|e| e.to_string())?;
            println!(r#"{{"ok":true,"bytes":{},"path":"{}"}}"#, png.len(), out.display());
        }
        Command::Launch { exe } => {
            let pid = session.launch(&exe).await.map_err(|e| e.to_string())?;
            println!(r#"{{"pid":{pid}}}"#);
        }
        Command::ListWindows => {
            let windows = session.list_windows().await.map_err(|e| e.to_string())?;
            let list: Vec<serde_json::Value> = windows.iter().map(|w| serde_json::json!({
                "name": w.name, "pid": w.pid, "focused": w.focused,
            })).collect();
            println!("{}", serde_json::to_string_pretty(&list).unwrap());
        }
        Command::FocusWindow { name } => {
            session.focus_window(&name).await.map_err(|e| e.to_string())?;
            println!(r#"{{"ok":true}}"#);
        }
        Command::WindowState { name, state } => {
            session.window_state(&name, &state).await.map_err(|e| e.to_string())?;
            println!(r#"{{"ok":true}}"#);
        }
        Command::Describe { window, since_seq } => {
            if since_seq.is_some() {
                let delta = session.describe_screen_delta(window.as_deref(), since_seq)
                    .await.map_err(|e| e.to_string())?;
                println!("{}", serde_json::to_string_pretty(&delta).unwrap());
            } else {
                let elements = session.describe_screen(window.as_deref())
                    .await.map_err(|e| e.to_string())?;
                let list: Vec<serde_json::Value> = elements.iter().map(|e| serde_json::json!({
                    "name": e.name, "role": e.role,
                    "left": e.left, "top": e.top, "right": e.right, "bottom": e.bottom,
                })).collect();
                println!("{}", serde_json::to_string_pretty(&list).unwrap());
            }
        }
        Command::GetClipboard => {
            let text = session.get_clipboard().await.map_err(|e| e.to_string())?;
            println!("{}", serde_json::to_string(&text).unwrap());
        }
        Command::SetClipboard { text } => {
            session.set_clipboard(&text).await.map_err(|e| e.to_string())?;
            println!(r#"{{"ok":true}}"#);
        }
        Command::Run { path } => {
            let json = if path == "-" {
                let mut buf = String::new();
                use std::io::Read;
                std::io::stdin().read_to_string(&mut buf).map_err(|e| e.to_string())?;
                buf
            } else {
                std::fs::read_to_string(&path).map_err(|e| e.to_string())?
            };
            let result = session.execute_intent(&json).await.map_err(|e| e.to_string())?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
        }

        // Lean verbs -----------------------------------------------------
        Command::See { mode, window, since_seq } => {
            match mode.as_str() {
                "full" => {
                    let elements = session.describe_screen(window.as_deref())
                        .await.map_err(|e| e.to_string())?;
                    let list: Vec<serde_json::Value> = elements.iter().map(|e| serde_json::json!({
                        "name": e.name, "role": e.role,
                        "left": e.left, "top": e.top, "right": e.right, "bottom": e.bottom,
                    })).collect();
                    println!("{}", serde_json::to_string_pretty(&list).unwrap());
                }
                "delta" => {
                    let delta = session.describe_screen_delta(window.as_deref(), since_seq)
                        .await.map_err(|e| e.to_string())?;
                    println!("{}", serde_json::to_string_pretty(&delta).unwrap());
                }
                _ => {
                    let elements = session.describe_screen_fast()
                        .await.map_err(|e| e.to_string())?;
                    let list: Vec<serde_json::Value> = elements.iter().map(|e| serde_json::json!({
                        "name": e.name, "role": e.role,
                        "left": e.left, "top": e.top, "right": e.right, "bottom": e.bottom,
                    })).collect();
                    println!("{}", serde_json::to_string_pretty(&list).unwrap());
                }
            }
        }
        Command::Find { name, role, description, text, mode } => {
            let target = parse_target(name, role, description, text)?;
            let locate_mode = parse_locate_mode(&mode);
            let grounded = session.ground(target, locate_mode).await.map_err(|e| e.to_string())?;
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "center": { "x": grounded.center.0, "y": grounded.center.1 },
                "rect": { "left": grounded.rect.0, "top": grounded.rect.1, "right": grounded.rect.2, "bottom": grounded.rect.3 },
                "source": grounded.source.to_string(),
                "confidence": grounded.confidence,
                "name": grounded.name,
            })).unwrap());
        }
        Command::Act { name, role, description, text, action, text_input } => {
            let target = parse_target(name, role, description, text)?;
            let grounded = session.ground(target.clone(), LocateMode::Instant)
                .await.map_err(|e| e.to_string())?;
            let (cx, cy) = grounded.center;
            let text_ref = text_input.as_deref();
            match action.as_str() {
                "click" => { session.click_at(cx, cy).await.map_err(|e| e.to_string())?; }
                "type" => {
                    let t = text_ref.ok_or("action=type requires --text-input")?;
                    session.click_at(cx, cy).await.map_err(|e| e.to_string())?;
                    ghost_core::input::keyboard::type_text(t).map_err(|e| e.to_string())?;
                }
                "double_click" => { session.double_click_at(cx, cy).await.map_err(|e| e.to_string())?; }
                "right_click" => { session.right_click_at(cx, cy).await.map_err(|e| e.to_string())?; }
                "hover" => { session.hover(cx, cy).await.map_err(|e| e.to_string())?; }
                other => return Err(format!("unknown action '{other}'; use click|type|double_click|right_click|hover")),
            }
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "ok": true,
                "source": grounded.source.to_string(),
                "center": { "x": cx, "y": cy },
            })).unwrap());
        }
        Command::Key { keys } => {
            // Mirror ghost_key logic: "down:X"/"up:X", combo "Ctrl+C", or single key.
            if let Some(k) = keys.strip_prefix("down:") {
                session.key_down(k).await.map_err(|e| e.to_string())?;
            } else if let Some(k) = keys.strip_prefix("up:") {
                session.key_up(k).await.map_err(|e| e.to_string())?;
            } else {
                let parts: Vec<&str> = keys.split('+').collect();
                if parts.len() == 1 {
                    session.press(parts[0]).await.map_err(|e| e.to_string())?;
                } else {
                    let (modifiers, key) = parts.split_at(parts.len() - 1);
                    session.hotkey(modifiers, key[0]).await.map_err(|e| e.to_string())?;
                }
            }
            println!(r#"{{"ok":true}}"#);
        }
        Command::Window { op, name, state, exe } => {
            match op.as_str() {
                "list" => {
                    let windows = session.list_windows().await.map_err(|e| e.to_string())?;
                    let list: Vec<serde_json::Value> = windows.iter().map(|w| serde_json::json!({
                        "name": w.name, "pid": w.pid, "focused": w.focused,
                    })).collect();
                    println!("{}", serde_json::to_string_pretty(&list).unwrap());
                }
                "focus" => {
                    let n = name.ok_or("--name required for op=focus")?;
                    session.focus_window(&n).await.map_err(|e| e.to_string())?;
                    println!(r#"{{"ok":true}}"#);
                }
                "state" => {
                    let n = name.ok_or("--name required for op=state")?;
                    let s = state.ok_or("--state required for op=state")?;
                    session.window_state(&n, &s).await.map_err(|e| e.to_string())?;
                    println!(r#"{{"ok":true}}"#);
                }
                "launch" => {
                    let e = exe.or(name).ok_or("--exe or --name required for op=launch")?;
                    let pid = session.launch(&e).await.map_err(|e| e.to_string())?;
                    println!(r#"{{"pid":{pid}}}"#);
                }
                other => return Err(format!("unknown op '{other}'; use list|focus|state|launch")),
            }
        }
        Command::Query { fields, window: _ } => {
            let field_list: Vec<String> = fields.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let map = session.query_extract(&field_list, None).await.map_err(|e| e.to_string())?;
            println!("{}", serde_json::to_string_pretty(&serde_json::Value::Object(map)).unwrap());
        }
        Command::Snapshot { out, foreground: _ } => {
            let rect = session.foreground_window_rect();
            let bytes = session.screenshot_region(rect, Some(768), Some(75))
                .await.map_err(|e| e.to_string())?;
            if let Some(path) = out {
                std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
                println!(r#"{{"ok":true,"bytes":{},"path":"{}"}}"#, bytes.len(), path.display());
            } else {
                // Print base64 JPEG to stdout.
                let b64 = base64_encode(&bytes);
                println!(r#"{{"jpeg_base64":"{}","size_bytes":{}}}"#, b64, bytes.len());
            }
        }

        Command::Serve { .. } => unreachable!("handled above"),
    }
    Ok(())
}

/// Implement `ghost serve`: exec the ghost-mcp binary (stdio loop).
/// Looks for ghost-mcp adjacent to this binary first, then in PATH.
fn run_serve() -> Result<(), String> {
    // Find ghost-mcp binary path.
    let mcp_bin = locate_ghost_mcp()?;
    // Replace this process with ghost-mcp (exec on Unix; on Windows, spawn + wait).
    #[cfg(target_os = "windows")]
    {
        let status = std::process::Command::new(&mcp_bin)
            .status()
            .map_err(|e| format!("ghost serve: failed to start {}: {}", mcp_bin.display(), e))?;
        if !status.success() {
            return Err(format!("ghost-mcp exited with status {status}"));
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&mcp_bin).exec();
        Err(format!("ghost serve: exec failed: {err}"))
    }
}

/// Locate the ghost-mcp binary: adjacent to this binary first, then PATH.
fn locate_ghost_mcp() -> Result<PathBuf, String> {
    // Check adjacent to the current executable.
    if let Ok(exe) = std::env::current_exe() {
        let adjacent = exe.parent().unwrap_or(&exe).join("ghost-mcp.exe");
        if adjacent.exists() {
            return Ok(adjacent);
        }
        // Also check without .exe for cross-platform convenience.
        let adjacent_no_ext = exe.parent().unwrap_or(&exe).join("ghost-mcp");
        if adjacent_no_ext.exists() {
            return Ok(adjacent_no_ext);
        }
    }
    // Fall back to PATH resolution.
    which_ghost_mcp()
}

fn which_ghost_mcp() -> Result<PathBuf, String> {
    let name = if cfg!(target_os = "windows") { "ghost-mcp.exe" } else { "ghost-mcp" };
    std::env::var_os("PATH")
        .unwrap_or_default()
        .to_string_lossy()
        .split(if cfg!(target_os = "windows") { ';' } else { ':' })
        .map(|dir| PathBuf::from(dir).join(name))
        .find(|p| p.exists())
        .ok_or_else(|| format!(
            "ghost-mcp not found adjacent to this binary or in PATH. \
             Build it with: cargo build -p ghost-mcp --release"
        ))
}

fn parse_by(name: Option<String>, role: Option<String>) -> Result<By, String> {
    match (name, role) {
        (Some(n), _) => Ok(By::name(&n)),
        (_, Some(r)) => Ok(By::role(&r)),
        _ => Err("must provide --name or --role".into()),
    }
}

fn parse_target(
    name: Option<String>,
    role: Option<String>,
    description: Option<String>,
    text: Option<String>,
) -> Result<Target, String> {
    if let Some(n) = name { return Ok(Target::Name(n)); }
    if let Some(r) = role { return Ok(Target::Role(r)); }
    if let Some(d) = description { return Ok(Target::Description(d)); }
    if let Some(t) = text { return Ok(Target::Text(t)); }
    Err("must provide --name, --role, --description, or --text".into())
}

fn parse_locate_mode(mode: &str) -> LocateMode {
    match mode {
        "deliberate" => LocateMode::Deliberate,
        "instant_only" => LocateMode::InstantOnly,
        _ => LocateMode::Instant,
    }
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

// ---------------------------------------------------------------------------
// Tests — arg parsing, pure logic (no live session)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("ghost").chain(args.iter().copied()))
    }

    // --- Legacy verbs ---

    #[test]
    fn parse_click_name() {
        let cli = parse(&["click", "--name", "Submit"]).unwrap();
        assert!(matches!(cli.command, Command::Click { name: Some(_), .. }));
    }

    #[test]
    fn parse_click_at() {
        let cli = parse(&["click-at", "100", "200"]).unwrap();
        assert!(matches!(cli.command, Command::ClickAt { x: 100, y: 200 }));
    }

    #[test]
    fn parse_press() {
        let cli = parse(&["press", "Enter"]).unwrap();
        assert!(matches!(cli.command, Command::Press { key } if key == "Enter"));
    }

    #[test]
    fn parse_screenshot_default_out() {
        let cli = parse(&["screenshot"]).unwrap();
        assert!(matches!(cli.command, Command::Screenshot { out } if out == PathBuf::from("ghost.png")));
    }

    #[test]
    fn parse_list_windows() {
        let cli = parse(&["list-windows"]).unwrap();
        assert!(matches!(cli.command, Command::ListWindows));
    }

    // --- Lean verbs ---

    #[test]
    fn parse_see_default_mode() {
        let cli = parse(&["see"]).unwrap();
        assert!(matches!(cli.command, Command::See { mode, .. } if mode == "fast"));
    }

    #[test]
    fn parse_see_full_mode() {
        let cli = parse(&["see", "--mode", "full"]).unwrap();
        assert!(matches!(cli.command, Command::See { mode, .. } if mode == "full"));
    }

    #[test]
    fn parse_find_by_name() {
        let cli = parse(&["find", "--name", "OK"]).unwrap();
        assert!(matches!(cli.command, Command::Find { name: Some(n), .. } if n == "OK"));
    }

    #[test]
    fn parse_find_by_role() {
        let cli = parse(&["find", "--role", "button"]).unwrap();
        assert!(matches!(cli.command, Command::Find { role: Some(r), .. } if r == "button"));
    }

    #[test]
    fn parse_act_click() {
        let cli = parse(&["act", "--name", "Submit", "--action", "click"]).unwrap();
        assert!(matches!(cli.command, Command::Act { action, .. } if action == "click"));
    }

    #[test]
    fn parse_key_combo() {
        let cli = parse(&["key", "Ctrl+C"]).unwrap();
        assert!(matches!(cli.command, Command::Key { keys } if keys == "Ctrl+C"));
    }

    #[test]
    fn parse_key_single() {
        let cli = parse(&["key", "Enter"]).unwrap();
        assert!(matches!(cli.command, Command::Key { keys } if keys == "Enter"));
    }

    #[test]
    fn parse_key_down() {
        let cli = parse(&["key", "down:Shift"]).unwrap();
        assert!(matches!(cli.command, Command::Key { keys } if keys == "down:Shift"));
    }

    #[test]
    fn parse_window_list() {
        let cli = parse(&["window", "--op", "list"]).unwrap();
        assert!(matches!(cli.command, Command::Window { op, .. } if op == "list"));
    }

    #[test]
    fn parse_window_default_op_is_list() {
        let cli = parse(&["window"]).unwrap();
        assert!(matches!(cli.command, Command::Window { op, .. } if op == "list"));
    }

    #[test]
    fn parse_query_fields() {
        let cli = parse(&["query", "--fields", "title,status"]).unwrap();
        assert!(matches!(cli.command, Command::Query { fields, .. } if fields == "title,status"));
    }

    #[test]
    fn parse_snapshot_no_out() {
        let cli = parse(&["snapshot"]).unwrap();
        assert!(matches!(cli.command, Command::Snapshot { out: None, .. }));
    }

    #[test]
    fn parse_serve() {
        let cli = parse(&["serve"]).unwrap();
        assert!(matches!(cli.command, Command::Serve { .. }));
    }

    #[test]
    fn parse_serve_custom_addr() {
        let cli = parse(&["serve", "--addr", "0.0.0.0:9000"]).unwrap();
        assert!(matches!(cli.command, Command::Serve { addr } if addr == "0.0.0.0:9000"));
    }

    // --- parse_target helper ---

    #[test]
    fn target_name_wins_over_role() {
        let t = parse_target(Some("foo".into()), Some("button".into()), None, None).unwrap();
        assert!(matches!(t, Target::Name(n) if n == "foo"));
    }

    #[test]
    fn target_role_when_no_name() {
        let t = parse_target(None, Some("button".into()), None, None).unwrap();
        assert!(matches!(t, Target::Role(r) if r == "button"));
    }

    #[test]
    fn target_description_third() {
        let t = parse_target(None, None, Some("the blue submit button".into()), None).unwrap();
        assert!(matches!(t, Target::Description(d) if d.contains("blue")));
    }

    #[test]
    fn target_text_last() {
        let t = parse_target(None, None, None, Some("Submit".into())).unwrap();
        assert!(matches!(t, Target::Text(t) if t == "Submit"));
    }

    #[test]
    fn target_none_is_err() {
        assert!(parse_target(None, None, None, None).is_err());
    }

    // --- parse_locate_mode ---

    #[test]
    fn locate_mode_default_is_instant() {
        assert!(matches!(parse_locate_mode("instant"), LocateMode::Instant));
        assert!(matches!(parse_locate_mode("anything"), LocateMode::Instant));
    }

    #[test]
    fn locate_mode_deliberate() {
        assert!(matches!(parse_locate_mode("deliberate"), LocateMode::Deliberate));
    }

    #[test]
    fn locate_mode_instant_only() {
        assert!(matches!(parse_locate_mode("instant_only"), LocateMode::InstantOnly));
    }
}
