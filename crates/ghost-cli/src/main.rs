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

mod doctor;
mod verify;

use clap::{Parser, Subcommand};
use ghost_session::{By, GhostSession, LocateMode, Target};
use std::path::PathBuf;
use std::process::ExitCode;

/// The platform engine: `ghost-core` on Windows, `ghost-linux` on Linux,
/// `ghost-macos` on macOS (see docs/macos-build.md).
mod engine {
    #[cfg(windows)]
    pub use ghost_core::*;
    #[cfg(target_os = "linux")]
    pub use ghost_linux::*;
    #[cfg(target_os = "macos")]
    pub use ghost_macos::*;
}


#[derive(Parser)]
#[command(
    name = "ghost",
    version,
    about = "Desktop and browser automation for agents - Windows UIA, Linux AT-SPI, CDP",
    long_about = "Automate desktop apps (UI Automation on Windows, AT-SPI on Linux) and browsers (CDP).\n\
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
    // Serve: run the MCP stdio server (stdio only - no addr/port)
    // -----------------------------------------------------------------------

    /// Start the ghost MCP stdio server. Executes the ghost-mcp binary found in PATH
    /// or adjacent to this binary. Communicates over stdio; there is no addr/port.
    Serve,

    /// Check that this machine can run Ghost. Reports PASS/WARN/FAIL per item.
    /// Exit code 1 if anything is FAIL. Run this before reporting a problem.
    Doctor,

    /// Prove the product claims on this machine: background enforcement,
    /// concurrent tabs, latency budgets, a second server alongside, emergency
    /// stop, and an untouched foreground. Drives the real ghost-mcp server
    /// over stdio. Exit code 1 if any claim fails.
    Verify {
        /// Also fail if the cursor position changed during the run (off by
        /// default because the human at the machine moves their own mouse).
        #[arg(long)]
        strict_cursor: bool,
    },

    /// Lift the emergency stop for every Ghost process - including a stop the
    /// human raised with Ctrl+Alt+G, which an agent's ghost_reset cannot lift.
    /// This is the human's way to resume from a terminal (Ctrl+Alt+Shift+G,
    /// pressed on the keyboard, is the other). Run it yourself in an
    /// interactive terminal: it shows a short word and you type it back; a
    /// pipe or a script is refused.
    Reset,
}

#[tokio::main]
async fn main() -> ExitCode {
    // Physical-pixel coordinates. Must precede any window/DC use. Gated: on
    // macOS `engine` has no `windows`/`linux` arm yet, so this path doesn't
    // exist there (see docs/macos-build.md).
    #[cfg(any(windows, target_os = "linux"))]
    crate::engine::system::dpi::ensure_process_dpi_aware();
    let cli = Cli::parse();
    if cli.verbose {
        tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    }

    // Handled here rather than in run(): doctor owns its own exit code, and it
    // must report WHY a session cannot be created rather than failing to start
    // alongside one.
    if let Command::Doctor = &cli.command {
        let checks = doctor::run_checks();
        print!("{}", doctor::render(&checks));
        return ExitCode::from(doctor::exit_code(&checks));
    }

    if let Command::Verify { strict_cursor } = &cli.command {
        return ExitCode::from(verify::run(*strict_cursor) as u8);
    }

    // No session: lifting the stop must work even when nothing else can start.
    if let Command::Reset = &cli.command {
        return human_reset();
    }

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ghost: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Whether a subcommand changes anything (and so is refused while the
/// emergency stop is active). Reads, `serve`, `doctor` and `verify` are not:
/// `serve` and `verify` run ghost-mcp, which gates each call itself.
fn cli_acts(cmd: &Command) -> bool {
    match cmd {
        Command::Click { .. }
        | Command::ClickAt { .. }
        | Command::Type { .. }
        | Command::Press { .. }
        | Command::Hotkey { .. }
        | Command::Launch { .. }
        | Command::FocusWindow { .. }
        | Command::WindowState { .. }
        | Command::SetClipboard { .. }
        | Command::Run { .. }
        | Command::Act { .. }
        | Command::Key { .. } => true,
        Command::Window { op, .. } => op != "list",
        Command::Screenshot { .. }
        | Command::ListWindows
        | Command::Describe { .. }
        | Command::GetClipboard
        | Command::See { .. }
        | Command::Find { .. }
        | Command::Query { .. }
        | Command::Snapshot { .. }
        | Command::Serve
        | Command::Reset
        | Command::Doctor
        | Command::Verify { .. } => false,
    }
}

/// `ghost reset`: the human's resume. Says what it lifted, and fails when the
/// stop could not be lifted (a process that cannot read the shared stop event
/// stays stopped whatever happens here).
fn human_reset() -> ExitCode {
    use ghost_session::error::{human_stop_active, is_stopped, stop_event_error};
    use std::io::{BufRead, IsTerminal, Write};
    // Only a human at an interactive terminal, checked before any stop event
    // is touched: an agent's own shell runs commands on pipes, and could
    // otherwise lift the human's stop by running this.
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "ghost reset: refused - run it yourself in an interactive terminal; it shows a word for you to \
             type back (a pipe or a script cannot)"
        );
        return ExitCode::from(2);
    }
    let word = challenge_word();
    print!("Type {word} to lift the emergency stop: ");
    let _ = std::io::stdout().flush();
    let mut typed = String::new();
    let _ = std::io::stdin().lock().read_line(&mut typed);
    if !challenge_ok(&word, &typed) {
        eprintln!("ghost reset: that was not {word}; the stop stays");
        return ExitCode::from(2);
    }
    let (was_stopped, was_human) = (is_stopped(), human_stop_active());
    crate::engine::input::hotkey::reset_human_stop();
    if let Some(why) = stop_event_error() {
        eprintln!("ghost: could not lift the stop - {why}");
        return ExitCode::FAILURE;
    }
    match (was_stopped, was_human) {
        (true, true) => println!("lifted the emergency stop the human raised (Ctrl+Alt+G); every Ghost process resumes"),
        (true, false) => println!("lifted the emergency stop; every Ghost process resumes"),
        _ => println!("no emergency stop was in force"),
    }
    ExitCode::SUCCESS
}

/// A short random word for `ghost reset` to have typed back: unguessable in
/// advance, easy to read (no 0/O, 1/l/I).
fn challenge_word() -> String {
    use std::hash::{BuildHasher, Hasher};
    const LETTERS: &[u8] = b"abcdefghjkmnpqrstuvwxyz";
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.write_u32(std::process::id());
    let mut x = h.finish();
    (0..5)
        .map(|_| {
            let c = LETTERS[(x % LETTERS.len() as u64) as usize] as char;
            x /= LETTERS.len() as u64;
            c
        })
        .collect()
}

fn challenge_ok(word: &str, typed: &str) -> bool {
    typed.trim().eq_ignore_ascii_case(word)
}

async fn run(cli: Cli) -> Result<(), String> {
    // Serve is special - it exec/spawns ghost-mcp, no GhostSession needed.
    if let Command::Serve = &cli.command {
        return run_serve();
    }

    // The emergency stop, read from the shared event: this process may have
    // started after a stop was raised elsewhere.
    if cli_acts(&cli.command) {
        ghost_session::error::ensure_not_stopped().map_err(|_| ghost_session::error::stop_refusal())?;
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
                    crate::engine::input::keyboard::type_text(t).map_err(|e| e.to_string())?;
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
        Command::Query { fields, window } => {
            let field_list: Vec<String> = fields.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            // Resolve optional --window to a bounding rect by name match.
            let rect = if let Some(ref name) = window {
                let windows = session.list_windows().await.map_err(|e| e.to_string())?;
                match windows.into_iter().find(|w| w.name.contains(name.as_str())) {
                    Some(w) => match crate::engine::system::window_rect(w.hwnd) {
                        Some(r) => Some(r),
                        None => return Err(format!("could not get rect for window '{name}'")),
                    },
                    None => return Err(format!("no window matching '{name}'")),
                }
            } else {
                None
            };
            let map = session.query_extract(&field_list, rect).await.map_err(|e| e.to_string())?;
            println!("{}", serde_json::to_string_pretty(&serde_json::Value::Object(map)).unwrap());
        }
        Command::Snapshot { out, foreground } => {
            let rect = if foreground { session.foreground_window_rect() } else { None };
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

        Command::Serve | Command::Doctor | Command::Verify { .. } | Command::Reset => unreachable!("handled above"),
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
        .ok_or_else(|| "ghost-mcp not found adjacent to this binary or in PATH. \
             Build it with: cargo build -p ghost-mcp --release".to_string())
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

// ---------------------------------------------------------------------------
// Tests - arg parsing, pure logic (no live session)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encode_matches_the_rfc_vectors() {
        // RFC 4648 section 10 test vectors.
        for (raw, want) in [
            ("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(raw.as_bytes()), want, "{raw:?}");
        }
        // Every 6-bit value, including the two non-alphanumeric ones.
        assert_eq!(base64_encode(&[0xfb, 0xff, 0xbf]), "+/+/");
        assert_eq!(base64_encode(&[0, 0, 0]), "AAAA");
    }

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
        assert!(matches!(cli.command, Command::Screenshot { out } if out == std::path::Path::new("ghost.png")));
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
        assert!(matches!(cli.command, Command::Serve));
    }

    #[test]
    fn serve_rejects_addr_flag() {
        // --addr is not a valid flag on the stdio-only Serve subcommand.
        assert!(parse(&["serve", "--addr", "0.0.0.0:9000"]).is_err());
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

    #[test]
    fn action_subcommands_are_gated_by_the_emergency_stop_and_reads_are_not() {
        let acts: [&[&str]; 12] = [
            &["click", "--name", "OK"],
            &["click-at", "1", "2"],
            &["type", "--name", "F", "--text", "x"],
            &["press", "Enter"],
            &["launch", "x.exe"],
            &["focus-window", "x"],
            &["window-state", "x", "minimize"],
            &["set-clipboard", "x"],
            &["act", "--action", "click", "--name", "OK"],
            &["key", "Enter"],
            &["window", "--op", "focus", "--name", "x"],
            &["window", "--op", "launch", "--exe", "x.exe"],
        ];
        for a in acts {
            let cli = parse(a).unwrap_or_else(|e| panic!("{a:?}: {e}"));
            assert!(cli_acts(&cli.command), "{a:?} must be refused while stopped");
        }
        for a in [&["window"][..], &["list-windows"], &["see"], &["find", "--name", "OK"], &["get-clipboard"], &["doctor"], &["reset"]] {
            let cli = parse(a).unwrap_or_else(|e| panic!("{a:?}: {e}"));
            assert!(!cli_acts(&cli.command), "{a:?} only reads and must stay available");
        }
    }

    #[test]
    fn the_reset_challenge_is_random_readable_and_must_match() {
        let (a, b) = (super::challenge_word(), super::challenge_word());
        assert_eq!(a.len(), 5);
        assert!(a.chars().all(|c| c.is_ascii_lowercase() && !"ilo".contains(c)), "{a}");
        assert_ne!(a, b, "two challenges in a row should differ");
        assert!(super::challenge_ok(&a, &format!("  {}\r\n", a.to_uppercase())));
        assert!(!super::challenge_ok(&a, ""));
        assert!(!super::challenge_ok(&a, "yes"));
    }
}
