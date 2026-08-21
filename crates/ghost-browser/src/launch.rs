//! Browser discovery, launch, and DevTools endpoint resolution.
//!
//! Two ways to get a driveable browser:
//!
//! - **launch** a private instance with its own profile directory and its own
//!   DevTools port. This is what makes "several ghost processes at once" safe: each
//!   gets a separate browser process, separate cookies, separate ports. Nothing they
//!   do can touch the user's own browser session.
//! - **attach** to a browser the user already started with `--remote-debugging-port`.
//!   Useful when the automation needs the user's real logins.
//!
//! `Headless` is the default surface because it is the only mode with a hard
//! guarantee of never appearing on screen. `Windowed` exists for sites that behave
//! differently headless; ghost moves that window off the visible desktop after launch
//! so it still never covers the user's work.

use crate::error::{BrowserError, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    /// No window is ever created. Guaranteed invisible.
    Headless,
    /// A real browser window, moved off the visible desktop immediately after launch.
    Windowed,
}

impl LaunchMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "headless" | "hidden" => Some(LaunchMode::Headless),
            "windowed" | "window" | "visible" => Some(LaunchMode::Windowed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub mode: LaunchMode,
    /// Profile directory. A distinct path per ghost process is what isolates
    /// concurrent automations from each other and from the user's own browser.
    pub user_data_dir: PathBuf,
    pub width: u32,
    pub height: u32,
    /// Explicit browser executable; otherwise auto-discovered.
    pub executable: Option<PathBuf>,
    pub startup_timeout_ms: u64,
    /// Extra command-line switches appended verbatim.
    pub extra_args: Vec<String>,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            mode: LaunchMode::Headless,
            user_data_dir: default_profile_dir(),
            width: 1280,
            height: 900,
            executable: None,
            startup_timeout_ms: 20_000,
            extra_args: Vec::new(),
        }
    }
}

/// A profile directory unique to this process, so two ghost processes started at the
/// same moment cannot collide on one Chrome profile lock.
pub fn default_profile_dir() -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push("ghost-browser-profiles");
    base.push(format!("p{}", std::process::id()));
    base
}

/// Chromium-family browsers ghost knows how to find, in default preference order.
///
/// All of them speak CDP identically, so "which browser" is purely a user choice:
/// Comet when the automation should look like the user's own browsing environment,
/// Chrome for the most-tested target, Edge because it ships on every install.
const KNOWN_BROWSERS: &[(&str, &str)] = &[
    ("chrome", r"Google\Chrome\Application\chrome.exe"),
    ("comet", r"Perplexity\Comet\Application\comet.exe"),
    ("edge", r"Microsoft\Edge\Application\msedge.exe"),
    ("brave", r"BraveSoftware\Brave-Browser\Application\brave.exe"),
];

fn install_roots() -> Vec<PathBuf> {
    ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"]
        .iter()
        .filter_map(|var| std::env::var(var).ok().map(PathBuf::from))
        .collect()
}

/// Locate a specific browser by its short name ("chrome", "comet", "edge", "brave").
pub fn find_named_browser(name: &str) -> Result<PathBuf> {
    let want = name.trim().to_lowercase();
    let rel = KNOWN_BROWSERS
        .iter()
        .find(|(n, _)| *n == want)
        .map(|(_, rel)| *rel)
        .ok_or_else(|| {
            BrowserError::Launch(format!(
                "unknown browser '{name}'; known: {}",
                KNOWN_BROWSERS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
            ))
        })?;
    for root in install_roots() {
        let candidate = root.join(rel);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(BrowserError::Launch(format!("'{name}' is not installed on this machine")))
}

/// Every known browser that is actually installed, as (name, path).
pub fn installed_browsers() -> Vec<(String, PathBuf)> {
    let roots = install_roots();
    KNOWN_BROWSERS
        .iter()
        .filter_map(|(name, rel)| {
            roots
                .iter()
                .map(|r| r.join(rel))
                .find(|c| c.exists())
                .map(|p| (name.to_string(), p))
        })
        .collect()
}

/// Locate a Chromium-family browser. `GHOST_BROWSER_PATH` wins if set.
pub fn find_browser() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("GHOST_BROWSER_PATH") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Ok(path);
        }
        return Err(BrowserError::Launch(format!(
            "GHOST_BROWSER_PATH points at {}, which does not exist",
            path.display()
        )));
    }
    installed_browsers()
        .into_iter()
        .next()
        .map(|(_, p)| p)
        .ok_or(BrowserError::BrowserNotFound)
}

/// Command-line switches used for every launched instance.
pub fn base_args(opts: &LaunchOptions) -> Vec<String> {
    let mut args = vec![
        // Port 0 makes Chrome pick a free port and write it to DevToolsActivePort.
        // Choosing a port ourselves would race with every other ghost process.
        "--remote-debugging-port=0".to_string(),
        format!("--user-data-dir={}", opts.user_data_dir.display()),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-session-crashed-bubble".to_string(),
        "--disable-popup-blocking".to_string(),
        // Without these three, Chrome throttles timers and freezes rendering in any
        // tab that is not in front. Every background tab automation stalls, which is
        // exactly the workload ghost exists for.
        "--disable-background-timer-throttling".to_string(),
        "--disable-backgrounding-occluded-windows".to_string(),
        "--disable-renderer-backgrounding".to_string(),
        format!("--window-size={},{}", opts.width, opts.height),
    ];
    if opts.mode == LaunchMode::Headless {
        // The "new" headless is the real browser rendering path, not the old
        // stripped-down shell; pages behave the same as windowed.
        args.push("--headless=new".to_string());
    } else {
        // Start far off the visible desktop so the window never flashes in front of
        // the user, even for the moment before ghost repositions it.
        args.push("--window-position=-32000,-32000".to_string());
    }
    args.extend(opts.extra_args.iter().cloned());
    args.push("about:blank".to_string());
    args
}

/// A launched browser process plus the DevTools endpoint to talk to it.
pub struct LaunchedBrowser {
    pub pid: u32,
    pub port: u16,
    pub ws_url: String,
    pub user_data_dir: PathBuf,
    pub mode: LaunchMode,
}

/// Launch an isolated browser and wait for its DevTools endpoint to come up.
pub async fn launch(opts: &LaunchOptions) -> Result<LaunchedBrowser> {
    let exe = match &opts.executable {
        Some(p) => p.clone(),
        None => find_browser()?,
    };
    std::fs::create_dir_all(&opts.user_data_dir)?;
    // A stale port file from a previous run in the same profile would be read as if
    // it were this run's, pointing us at a dead or unrelated browser.
    let port_file = opts.user_data_dir.join("DevToolsActivePort");
    let _ = std::fs::remove_file(&port_file);

    let child = std::process::Command::new(&exe)
        .args(base_args(opts))
        // Detach the browser's stdio. Two reasons, both load-bearing:
        // 1. ghost-mcp speaks JSON-RPC over stdout. An inherited stdout lets Chrome
        //    write "DevTools listening on ..." straight into the protocol stream.
        // 2. An inherited pipe keeps that pipe open for as long as the browser lives,
        //    so a parent doing wait_with_output() on a ghost process hangs until
        //    every Chrome subprocess exits, not until ghost exits.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| BrowserError::Launch(format!("{}: {e}", exe.display())))?;
    let pid = child.id();

    let port = wait_for_port(&port_file, opts.startup_timeout_ms).await?;
    let ws_url = browser_ws_url(port).await?;

    if opts.mode == LaunchMode::Windowed {
        // Belt and braces: Chrome clamps --window-position to the nearest monitor on
        // some configurations, so move the window off-desktop directly as well.
        move_browser_offscreen(pid);
    }

    Ok(LaunchedBrowser {
        pid,
        port,
        ws_url,
        user_data_dir: opts.user_data_dir.clone(),
        mode: opts.mode,
    })
}

/// Read the port Chrome chose. The file's first line is the port; the second is the
/// browser target path.
pub fn parse_port_file(contents: &str) -> Option<u16> {
    contents.lines().next()?.trim().parse::<u16>().ok()
}

async fn wait_for_port(port_file: &Path, timeout_ms: u64) -> Result<u16> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if let Ok(s) = std::fs::read_to_string(port_file) {
            if let Some(p) = parse_port_file(&s) {
                if p != 0 {
                    return Ok(p);
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(BrowserError::DevToolsTimeout { ms: timeout_ms });
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Resolve the browser-level WebSocket URL from the HTTP DevTools endpoint.
pub async fn browser_ws_url(port: u16) -> Result<String> {
    let url = format!("http://127.0.0.1:{port}/json/version");
    let body: serde_json::Value = reqwest::get(&url)
        .await
        .map_err(|e| BrowserError::Transport(format!("GET {url}: {e}")))?
        .json()
        .await
        .map_err(|e| BrowserError::Transport(format!("parse {url}: {e}")))?;
    crate::cdp::field_str(&body, "webSocketDebuggerUrl", "/json/version")
}

/// Attach to a browser already listening on `port`.
pub async fn attach(port: u16) -> Result<LaunchedBrowser> {
    let ws_url = browser_ws_url(port).await?;
    Ok(LaunchedBrowser {
        pid: 0,
        port,
        ws_url,
        user_data_dir: PathBuf::new(),
        mode: LaunchMode::Windowed,
    })
}

/// Move every top-level window owned by `pid` off the visible desktop.
///
/// Uses `SWP_NOACTIVATE` and `HWND_BOTTOM` so repositioning cannot steal focus or
/// raise the window above what the user is looking at.
#[cfg(windows)]
fn move_browser_offscreen(pid: u32) {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible, SetWindowPos, HWND_BOTTOM,
        SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };
    // Self-contained enumeration so this crate stays platform-neutral apart from
    // this one cfg'd nicety - CDP itself is pure protocol and runs anywhere.
    unsafe extern "system" fn per_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let want_pid = lparam.0 as u32;
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == want_pid && IsWindowVisible(hwnd).as_bool() {
            let _ = SetWindowPos(
                hwnd,
                HWND_BOTTOM,
                -32000,
                -32000,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
            );
        }
        TRUE
    }
    unsafe {
        let _ = EnumWindows(Some(per_window), LPARAM(pid as isize));
    }
}

#[cfg(not(windows))]
fn move_browser_offscreen(_pid: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_file_first_line_is_the_port() {
        assert_eq!(parse_port_file("54321\n/devtools/browser/abc\n"), Some(54321));
        assert_eq!(parse_port_file("  9222  \n"), Some(9222));
    }

    #[test]
    fn malformed_port_file_yields_none_rather_than_a_bogus_port() {
        assert_eq!(parse_port_file(""), None);
        assert_eq!(parse_port_file("not-a-port\n/devtools/x"), None);
        assert_eq!(parse_port_file("99999999\n"), None, "out of u16 range");
    }

    #[test]
    fn headless_mode_never_asks_for_a_window_position() {
        let opts = LaunchOptions { mode: LaunchMode::Headless, ..Default::default() };
        let args = base_args(&opts);
        assert!(args.iter().any(|a| a == "--headless=new"));
        assert!(!args.iter().any(|a| a.starts_with("--window-position")));
    }

    #[test]
    fn windowed_mode_starts_off_the_visible_desktop() {
        let opts = LaunchOptions { mode: LaunchMode::Windowed, ..Default::default() };
        let args = base_args(&opts);
        assert!(!args.iter().any(|a| a == "--headless=new"));
        assert!(args.iter().any(|a| a == "--window-position=-32000,-32000"));
    }

    #[test]
    fn background_throttling_is_always_disabled() {
        // If any of these regress, background-tab automation silently stalls, so
        // pin them explicitly rather than trusting the arg list to stay correct.
        let args = base_args(&LaunchOptions::default());
        for required in [
            "--disable-background-timer-throttling",
            "--disable-backgrounding-occluded-windows",
            "--disable-renderer-backgrounding",
        ] {
            assert!(args.iter().any(|a| a == required), "missing {required}");
        }
    }

    #[test]
    fn port_is_always_delegated_to_chrome_to_avoid_cross_process_races() {
        let args = base_args(&LaunchOptions::default());
        assert!(args.iter().any(|a| a == "--remote-debugging-port=0"));
    }

    #[test]
    fn profile_dir_is_per_process() {
        let dir = default_profile_dir();
        assert!(dir.to_string_lossy().contains(&format!("p{}", std::process::id())));
    }

    #[test]
    fn launch_mode_parses_common_spellings() {
        assert_eq!(LaunchMode::from_str("headless"), Some(LaunchMode::Headless));
        assert_eq!(LaunchMode::from_str("Windowed"), Some(LaunchMode::Windowed));
        assert_eq!(LaunchMode::from_str("nope"), None);
    }
}
