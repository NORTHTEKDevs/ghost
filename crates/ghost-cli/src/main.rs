//! `ghost` command-line automation. Usable without Claude or any MCP client.
//!
//! Examples:
//!   ghost click --name "Submit"
//!   ghost type --role edit --text "hello"
//!   ghost screenshot --out shot.png
//!   ghost run intent.json
//!   ghost list-windows
//!   ghost describe --window "Notepad"

use clap::{Parser, Subcommand};
use ghost_session::{By, GhostSession};
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
    /// Start the HTTP server (same as `ghost-http` binary).
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
    let session = GhostSession::new()
        .map_err(|e| e.to_string())?
        .with_timeout(cli.timeout_ms);

    match cli.command {
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
        Command::Serve { addr } => {
            eprintln!("ghost: the built-in serve command is a stub; run the ghost-http binary instead:");
            eprintln!("  cargo run -p ghost-http -- --addr {addr}");
            return Err("use ghost-http binary".into());
        }
    }
    Ok(())
}

fn parse_by(name: Option<String>, role: Option<String>) -> Result<By, String> {
    match (name, role) {
        (Some(n), _) => Ok(By::name(&n)),
        (_, Some(r)) => Ok(By::role(&r)),
        _ => Err("must provide --name or --role".into()),
    }
}
