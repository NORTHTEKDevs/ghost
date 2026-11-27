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
//! `Headless` is the default surface: no window exists at all. `Windowed` exists
//! for sites that behave differently headless. On Windows a windowed browser is
//! started on a hidden desktop (`LaunchOptions::desktop`): measured on Edge and
//! Chrome, a new browser window takes the foreground on creation in every launch
//! style, so a window on the user's desktop would hand their keyboard to an
//! invisible window even when moved off-screen. Elsewhere the window is moved off
//! the visible desktop after launch.

use crate::error::{BrowserError, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    /// No window is ever created. Guaranteed invisible.
    Headless,
    /// A real browser window: on a hidden desktop when `LaunchOptions::desktop` is
    /// set (the default through ghost-mcp on Windows), otherwise moved off the visible
    /// desktop immediately after launch.
    Windowed,
}

impl std::str::FromStr for LaunchMode {
    type Err = ();
    fn from_str(s: &str) -> std::result::Result<Self, ()> {
        match s.trim().to_lowercase().as_str() {
            "headless" | "hidden" => Ok(LaunchMode::Headless),
            "windowed" | "window" | "visible" => Ok(LaunchMode::Windowed),
            _ => Err(()),
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
    /// Windows desktop object to start the process on (see `spawn_on_desktop`).
    /// `None` = the calling thread's desktop, i.e. the user's.
    pub desktop: Option<String>,
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
            desktop: None,
        }
    }
}

/// A profile directory unique to this process, so two ghost processes started at the
/// same moment cannot collide on one Chrome profile lock.
pub fn default_profile_dir() -> PathBuf {
    profiles_root().join(format!("p{}", std::process::id()))
}

/// Where every Ghost-launched browser keeps its profile. The path is on each
/// browser's command line, which is how the orphan sweep recognises them.
pub fn profiles_root() -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push("ghost-browser-profiles");
    base
}

/// Chromium-family browsers ghost knows how to find, in default preference order.
///
/// All of them speak CDP identically, so "which browser" is purely a user choice:
/// Comet when the automation should look like the user's own browsing environment,
/// Chrome for the most-tested target, Edge because it ships on every install.
#[cfg(windows)]
const KNOWN_BROWSERS: &[(&str, &str)] = &[
    ("chrome", r"Google\Chrome\Application\chrome.exe"),
    ("comet", r"Perplexity\Comet\Application\comet.exe"),
    ("edge", r"Microsoft\Edge\Application\msedge.exe"),
    (
        "brave",
        r"BraveSoftware\Brave-Browser\Application\brave.exe",
    ),
];

/// Outside Windows, the executable names each browser installs on PATH. The
/// names match the Windows list so the tool schema is the same everywhere;
/// Chromium answers to "chrome", and Comet has no Linux build.
#[cfg(not(windows))]
const KNOWN_BROWSERS: &[(&str, &[&str])] = &[
    ("chrome", &["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"]),
    ("comet", &[]),
    ("edge", &["microsoft-edge", "microsoft-edge-stable"]),
    ("brave", &["brave-browser", "brave"]),
];

#[cfg(windows)]
fn install_roots() -> Vec<PathBuf> {
    ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"]
        .iter()
        .filter_map(|var| std::env::var(var).ok().map(PathBuf::from))
        .collect()
}

/// The first of `names` that is a file in a directory of `path` (a PATH
/// value), trying each name across the whole PATH before the next name.
#[cfg_attr(windows, allow(dead_code))]
fn search_path(path: &std::ffi::OsStr, names: &[&str]) -> Option<PathBuf> {
    names.iter().find_map(|name| {
        std::env::split_paths(path)
            .map(|dir| dir.join(name))
            .find(|c| c.is_file())
    })
}

/// Where a known browser is installed, if it is.
fn locate(spec: &KnownBrowser) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        install_roots()
            .into_iter()
            .map(|r| r.join(spec))
            .find(|c| c.exists())
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("PATH").and_then(|p| search_path(&p, spec))
    }
}

#[cfg(windows)]
type KnownBrowser = str;
#[cfg(not(windows))]
type KnownBrowser = [&'static str];

/// Locate a specific browser by its short name ("chrome", "comet", "edge", "brave").
pub fn find_named_browser(name: &str) -> Result<PathBuf> {
    let want = name.trim().to_lowercase();
    let spec = KNOWN_BROWSERS
        .iter()
        .find(|(n, _)| *n == want)
        .map(|(_, spec)| *spec)
        .ok_or_else(|| {
            BrowserError::Launch(format!(
                "unknown browser '{name}'; known: {}",
                KNOWN_BROWSERS
                    .iter()
                    .map(|(n, _)| *n)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
    locate(spec).ok_or_else(|| {
        BrowserError::Launch(format!("'{name}' is not installed on this machine"))
    })
}

/// Every known browser that is actually installed, as (name, path).
pub fn installed_browsers() -> Vec<(String, PathBuf)> {
    KNOWN_BROWSERS
        .iter()
        .filter_map(|(name, spec)| locate(spec).map(|p| (name.to_string(), p)))
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
        // Headless has no display, and Chrome's GPU compositor then hands out a
        // new frame about once a second, so every Page.captureScreenshot waited
        // ~1 s for one (Chrome 1004 ms, Edge 614 ms median). Software
        // compositing produces the frame on demand: 31 ms / 40 ms, the page
        // pixels identical, ~3.7% of a core idle. GPU raster and WebGL are
        // untouched; plain --disable-gpu would take WebGL away.
        // Measured in docs/benches/2026-11-25-stage-3 (vsync, frame-rate,
        // occlusion and compositor-stage flags made no difference).
        args.push("--disable-gpu-compositing".to_string());
    } else if opts.desktop.is_none() {
        // Start far off the visible desktop so the window never flashes in front of
        // the user, even for the moment before ghost repositions it. On a hidden
        // desktop nothing is displayed, so the window keeps ordinary coordinates
        // (which also keeps UIA rects sane for the window-scoped verbs).
        args.push("--window-position=-32000,-32000".to_string());
    } else {
        // A hidden desktop has no DWM composition, so the compositor never gets
        // a vsync tick and every accessibility action (Invoke, SetValue) waited
        // ~2 s for a frame that never came. Free-running frames fix that.
        args.push("--disable-gpu-vsync".to_string());
        args.push("--disable-frame-rate-limit".to_string());
    }
    args.extend(opts.extra_args.iter().cloned());
    args.push("about:blank".to_string());
    args
}

/// Browsers die with the server that launched them.
///
/// A Windows job object with KILL_ON_JOB_CLOSE, created once per process;
/// every browser Ghost launches is assigned to it right after it starts. When
/// this process ends - normally, by taskkill, or by a crash - the kernel closes
/// the job's last handle and terminates every process in it. Before this, a
/// server that was killed left its browsers running invisibly (two headless
/// Chromes, 32 processes between them, found on 2026-09-01).
#[cfg(windows)]
pub mod job {
    use std::sync::OnceLock;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

    /// A kill-on-close job. Dropping it - or this process ending - ends its members.
    pub struct Job(HANDLE);
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn new() -> Result<Self, String> {
            unsafe {
                let h = CreateJobObjectW(None, windows::core::PCWSTR::null()).map_err(|e| e.to_string())?;
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if let Err(e) = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) {
                    let _ = CloseHandle(h);
                    return Err(e.to_string());
                }
                Ok(Job(h))
            }
        }

        /// A job with no kill-on-close: its members outlive this process and
        /// end only on `terminate`. For programs that are the human's once
        /// started (whatever a shell command opened), which a server restart
        /// must not take down but the emergency stop must.
        pub fn new_terminable() -> Result<Self, String> {
            unsafe {
                CreateJobObjectW(None, windows::core::PCWSTR::null())
                    .map(Job)
                    .map_err(|e| e.to_string())
            }
        }

        /// A named terminable job (`Local\<name>`), so a later Ghost process
        /// can open it by name and end it: a job lives as long as any process
        /// is in it, even after the process that created it has exited.
        pub fn new_terminable_named(name: &str) -> Result<Self, String> {
            unsafe {
                CreateJobObjectW(None, &windows::core::HSTRING::from(format!("Local\\{name}")))
                    .map(Job)
                    .map_err(|e| e.to_string())
            }
        }

        /// Open a named job to end it. Fails once every process in it has
        /// exited (the job, and its name, are gone).
        pub fn open_named(name: &str) -> Result<Self, String> {
            use windows::Win32::System::JobObjects::OpenJobObjectW;
            const JOB_OBJECT_TERMINATE: u32 = 0x0008;
            unsafe {
                OpenJobObjectW(JOB_OBJECT_TERMINATE, false, &windows::core::HSTRING::from(format!("Local\\{name}")))
                    .map(Job)
                    .map_err(|e| e.to_string())
            }
        }

        /// Put the process (and everything it starts from now on) in this job.
        pub fn adopt(&self, pid: u32) -> Result<(), String> {
            unsafe {
                let p = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid)
                    .map_err(|e| e.to_string())?;
                let r = AssignProcessToJobObject(self.0, p).map_err(|e| e.to_string());
                let _ = CloseHandle(p);
                r
            }
        }

        /// End every process in the job now. The job stays usable: processes
        /// adopted afterwards run normally.
        pub fn terminate(&self, exit_code: u32) -> Result<(), String> {
            unsafe {
                windows::Win32::System::JobObjects::TerminateJobObject(self.0, exit_code)
                    .map_err(|e| e.to_string())
            }
        }

        /// The ids of the processes in the job now (up to 1024).
        pub fn process_ids(&self) -> Vec<u32> {
            use windows::Win32::System::JobObjects::{
                JobObjectBasicProcessIdList, QueryInformationJobObject, JOBOBJECT_BASIC_PROCESS_ID_LIST,
            };
            const CAP: usize = 1024;
            // The struct holds one id inline; room for CAP after the header.
            let words = std::mem::size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>().div_ceil(8) + CAP;
            let mut buf = vec![0u64; words];
            unsafe {
                let list = buf.as_mut_ptr() as *mut JOBOBJECT_BASIC_PROCESS_ID_LIST;
                let ok = QueryInformationJobObject(
                    self.0,
                    JobObjectBasicProcessIdList,
                    list as *mut core::ffi::c_void,
                    (buf.len() * 8) as u32,
                    None,
                );
                if ok.is_err() {
                    return Vec::new();
                }
                let n = ((*list).NumberOfProcessIdsInList as usize).min(CAP);
                let ids = std::ptr::addr_of!((*list).ProcessIdList) as *const usize;
                (0..n).map(|i| *ids.add(i) as u32).collect()
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    static SERVER_JOB: OnceLock<Option<Job>> = OnceLock::new();

    /// Whether `pid` is in this server's job - a browser this server launched.
    /// False when no job was ever created or the process cannot be opened.
    pub fn is_bound_to_this_process(pid: u32) -> bool {
        use windows::Win32::System::JobObjects::IsProcessInJob;
        use windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
        let Some(Some(job)) = SERVER_JOB.get() else { return false };
        unsafe {
            let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
            let mut inside = windows::Win32::Foundation::BOOL(0);
            let ok = IsProcessInJob(p, job.0, &mut inside).is_ok();
            let _ = CloseHandle(p);
            ok && inside.as_bool()
        }
    }

    /// Bind the process to this server's lifetime. An error is reported, never
    /// fatal: the next server's startup sweep ends a browser that slips through.
    pub fn bind_to_this_process(pid: u32) -> Result<(), String> {
        match SERVER_JOB.get_or_init(|| Job::new().ok()) {
            Some(job) => job.adopt(pid),
            None => Err("the server job object could not be created".into()),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::Job;
        use std::os::windows::process::CommandExt;

        fn spawn_ping() -> std::process::Child {
            std::process::Command::new("ping")
                .args(["-n", "30", "127.0.0.1"])
                .stdout(std::process::Stdio::null())
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .spawn()
                .expect("spawn ping")
        }

        /// A terminable job ends its members only when asked: closing it (the
        /// server exiting) leaves them running, so a program a shell started -
        /// an editor, a terminal - survives a server restart.
        #[test]
        fn a_terminable_job_ends_members_on_terminate_not_on_close() {
            let mut kept = spawn_ping();
            let job = Job::new_terminable().expect("job");
            job.adopt(kept.id()).expect("adopt");
            drop(job);
            std::thread::sleep(std::time::Duration::from_millis(300));
            let survived = kept.try_wait().expect("try_wait").is_none();
            let _ = kept.kill();
            let _ = kept.wait();
            assert!(survived, "closing the job ended its member");

            let mut ended = spawn_ping();
            let job = Job::new_terminable().expect("job");
            job.adopt(ended.id()).expect("adopt");
            job.terminate(7).expect("terminate");
            let code = ended.wait().expect("wait").code();
            assert_eq!(code, Some(7), "terminate must end the member");
        }
    }
}

/// A launched browser process plus the DevTools endpoint to talk to it.
pub struct LaunchedBrowser {
    pub pid: u32,
    /// The process is in this server's kill-on-close job: it cannot outlive us.
    pub job_bound: bool,
    pub port: u16,
    pub ws_url: String,
    pub user_data_dir: PathBuf,
    pub mode: LaunchMode,
    /// The hidden desktop the process was started on, if any.
    pub desktop: Option<String>,
}

/// Ends the browser process a launch started if the launch is abandoned
/// before the browser is handed back: a tool deadline or a cancel dropping the
/// future during the port wait, or the wait itself failing. Without it Chrome
/// kept running and held the profile, so the next launch with the same id
/// handed off to the orphan and hung.
/// Both kinds of process are held by handle (a `Child`, or the handle
/// `CreateProcessW` returned), never by id: an id can be reused by an unrelated
/// process once the browser exits (Chrome exits at once when it hands off to an
/// instance already running on the profile).
struct LaunchGuard {
    child: Option<std::process::Child>,
    /// A process handle (as an integer so the launch future stays `Send`).
    #[cfg(windows)]
    process: Option<isize>,
}

impl LaunchGuard {
    fn child(child: std::process::Child) -> Self {
        LaunchGuard {
            child: Some(child),
            #[cfg(windows)]
            process: None,
        }
    }

    /// Takes ownership of `handle` (closed on drop, ended first if still armed).
    #[cfg(windows)]
    fn process(handle: isize) -> Self {
        LaunchGuard { child: None, process: Some(handle) }
    }

    fn disarm(&mut self) {
        self.child = None;
        #[cfg(windows)]
        if let Some(h) = self.process.take() {
            close_process_handle(h, false);
        }
    }
}

impl Drop for LaunchGuard {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        #[cfg(windows)]
        if let Some(h) = self.process.take() {
            close_process_handle(h, true);
        }
    }
}

#[cfg(windows)]
fn close_process_handle(h: isize, end_first: bool) {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::TerminateProcess;
    let h = HANDLE(h as _);
    unsafe {
        if end_first {
            let _ = TerminateProcess(h, 1);
        }
        let _ = CloseHandle(h);
    }
}

#[cfg(all(test, windows))]
fn pid_running(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            Ok(h) => {
                let running = WaitForSingleObject(h, 0) == WAIT_TIMEOUT;
                let _ = CloseHandle(h);
                running
            }
            Err(_) => false,
        }
    }
}

#[cfg(all(test, not(windows)))]
fn pid_running(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|s| s.success())
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

    #[allow(unused_mut)]
    let mut desktop_pid: Option<u32> = None;
    let mut guard = match opts.desktop.as_deref() {
        #[cfg(windows)]
        Some(desktop) => {
            let (pid, handle) = spawn_on_desktop(&exe, &base_args(opts), desktop)?;
            desktop_pid = Some(pid);
            LaunchGuard::process(handle)
        }
        #[cfg(not(windows))]
        Some(desktop) => {
            return Err(BrowserError::Launch(format!(
                "launching onto desktop '{desktop}' is Windows-only"
            )))
        }
        None => {
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
            LaunchGuard::child(child)
        }
    };
    let pid = guard.child.as_ref().map(|c| c.id()).or(desktop_pid).expect("a launch guard holds its process");

    #[cfg(windows)]
    let job_bound = match job::bind_to_this_process(pid) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(pid, error = %e, "browser not bound to this server's lifetime; a later server's sweep ends it if this one dies");
            false
        }
    };
    #[cfg(not(windows))]
    let job_bound = false;

    let port = wait_for_port(&port_file, opts.startup_timeout_ms).await?;
    let ws_url = browser_ws_url(port).await?;

    if opts.mode == LaunchMode::Windowed && opts.desktop.is_none() {
        // Belt and braces: Chrome clamps --window-position to the nearest monitor on
        // some configurations, so move the window off-desktop directly as well.
        // (On a hidden desktop nothing is displayed, so the window stays put.)
        move_browser_offscreen(pid);
    }

    // Handed back: the registry owns the browser's lifetime from here.
    guard.disarm();
    Ok(LaunchedBrowser {
        pid,
        job_bound,
        port,
        ws_url,
        user_data_dir: opts.user_data_dir.clone(),
        mode: opts.mode,
        desktop: opts.desktop.clone(),
    })
}

/// Quote one argument the way `CommandLineToArgvW` expects it back.
fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| c == ' ' || c == '\t' || c == '"') {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// The single command line `CreateProcessW` takes, from an executable and its
/// arguments.
pub fn windows_command_line(exe: &Path, args: &[String]) -> String {
    let mut parts = vec![quote_arg(&exe.to_string_lossy())];
    parts.extend(args.iter().map(|a| quote_arg(a)));
    parts.join(" ")
}

/// Start the browser on a named desktop. The desktop is chosen at process
/// creation through `STARTUPINFO.lpDesktop` and cannot be changed afterwards -
/// which is exactly why a windowed browser has to be born there: a window created
/// on the user's desktop takes the foreground on creation (measured for Edge and
/// Chrome in every launch style), and moving it off-screen afterwards does not
/// give the keyboard back.
#[cfg(windows)]
/// Returns the pid and the process handle (owned by the caller, who closes it).
fn spawn_on_desktop(exe: &Path, args: &[String], desktop: &str) -> Result<(u32, isize)> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        CreateProcessW, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
    };

    let cmdline = windows_command_line(exe, args);
    let mut cmd_w: Vec<u16> = OsStr::new(&cmdline)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut desk_w: Vec<u16> = OsStr::new(desktop)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desk_w.as_mut_ptr()),
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        // No handle inheritance: the browser must not hold ghost's stdio pipes
        // open (see the plain spawn above for why that matters).
        CreateProcessW(
            None,
            PWSTR(cmd_w.as_mut_ptr()),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            None,
            &si,
            &mut pi,
        )
        .map_err(|e| {
            BrowserError::Launch(format!(
                "CreateProcessW on desktop '{desktop}' ({}): {e}",
                exe.display()
            ))
        })?;
        let pid = pi.dwProcessId;
        let _ = CloseHandle(pi.hThread);
        Ok((pid, pi.hProcess.0 as isize))
    }
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

/// The browser ws URL comes from whatever answers on the port; it must point
/// back at this machine (ws:// on 127.0.0.1, localhost or ::1), or a process
/// squatting the port could hand Ghost a socket on another host.
pub fn check_ws_url(ws_url: &str) -> Result<()> {
    let refuse = |why: &str| BrowserError::Transport(format!("refusing webSocketDebuggerUrl {ws_url:?}: {why}"));
    let parsed = reqwest::Url::parse(ws_url).map_err(|e| refuse(&e.to_string()))?;
    if parsed.scheme() != "ws" {
        return Err(refuse("scheme must be ws"));
    }
    let host = parsed.host_str().unwrap_or("");
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let loopback = bare.eq_ignore_ascii_case("localhost")
        || bare.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if loopback {
        Ok(())
    } else {
        Err(refuse("host must be 127.0.0.1, localhost or [::1]"))
    }
}

/// Resolve the browser-level WebSocket URL from the HTTP DevTools endpoint.
pub async fn browser_ws_url(port: u16) -> Result<String> {
    let url = format!("http://127.0.0.1:{port}/json/version");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .build()
        .map_err(|e| BrowserError::Transport(format!("http client: {e}")))?;
    let body: serde_json::Value = client
        .get(&url)
        .send()
        .await
        .map_err(|e| BrowserError::Transport(format!("GET {url}: {e}")))?
        .json()
        .await
        .map_err(|e| BrowserError::Transport(format!("parse {url}: {e}")))?;
    let ws_url = crate::cdp::field_str(&body, "webSocketDebuggerUrl", "/json/version")?;
    check_ws_url(&ws_url)?;
    Ok(ws_url)
}

/// Attach to a browser already listening on `port`.
pub async fn attach(port: u16) -> Result<LaunchedBrowser> {
    let ws_url = browser_ws_url(port).await?;
    Ok(LaunchedBrowser {
        pid: 0,
        // Not ours: an attached browser belongs to whoever started it.
        job_bound: false,
        port,
        ws_url,
        user_data_dir: PathBuf::new(),
        mode: LaunchMode::Windowed,
        desktop: None,
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

    /// Outside Windows a browser is found on PATH. Before, only the Windows
    /// install roots were searched, so ghost_browser_launch on Linux found
    /// nothing unless GHOST_BROWSER_PATH was set.
    #[test]
    fn path_search_finds_the_first_listed_name_in_path_order() {
        let root = std::env::temp_dir().join(format!("ghost-path-search-{}", std::process::id()));
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("chromium"), b"").unwrap();
        std::fs::write(a.join("chromium"), b"").unwrap();
        std::fs::create_dir_all(a.join("google-chrome")).unwrap(); // a directory, not a browser
        let path = std::env::join_paths([&a, &b]).unwrap();

        let found = search_path(&path, &["google-chrome", "chromium"]);
        let none = search_path(&path, &["brave-browser"]);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(found, Some(a.join("chromium")));
        assert_eq!(none, None);
    }

    fn sleeper() -> std::process::Child {
        let mut c = if cfg!(windows) {
            let mut c = std::process::Command::new("ping");
            c.args(["-n", "30", "127.0.0.1"]);
            c
        } else {
            let mut c = std::process::Command::new("sleep");
            c.arg("30");
            c
        };
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut c, 0x0800_0000); // CREATE_NO_WINDOW
        c.stdout(std::process::Stdio::null()).spawn().expect("spawn sleeper")
    }

    fn alive(pid: u32) -> bool {
        pid_running(pid)
    }

    /// The hidden-desktop path guards the process by its HANDLE: a guard that
    /// fires after the process exited and its id was reused must not end the
    /// new owner, so nothing in the guard looks a process up by id.
    #[test]
    fn the_guard_never_ends_a_process_by_id() {
        let src = include_str!("launch.rs");
        let start = src.find("impl Drop for LaunchGuard").expect("Drop impl");
        let b = &src[start..start + src[start..].find("\n}\n").expect("end")];
        assert!(!b.contains("end_pid(") && !b.contains("OpenProcess("), "{b}");
        assert!(!src.contains(concat!("fn end", "_pid(")), "a pid-based kill helper is back");
    }

    #[cfg(windows)]
    #[test]
    fn a_handle_guard_ends_its_process_and_a_disarmed_one_does_not() {
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE};
        let open = |pid: u32| unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) }.expect("open") .0 as isize;

        let mut ended = sleeper();
        drop(LaunchGuard::process(open(ended.id())));
        let code = ended.wait().expect("wait").code();
        assert_eq!(code, Some(1), "the guard did not end its process");

        let mut kept = sleeper();
        let mut g = LaunchGuard::process(open(kept.id()));
        g.disarm();
        drop(g);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let survived = kept.try_wait().expect("try_wait").is_none();
        let _ = kept.kill();
        let _ = kept.wait();
        assert!(survived, "a disarmed guard ended a launched browser");
    }

    /// A launch abandoned before its browser was handed over (a deadline or a
    /// cancel during the port wait, or the wait timing out) ends the process
    /// it started instead of leaving it holding the profile.
    #[test]
    fn an_abandoned_launch_ends_the_process_it_started() {
        let child = sleeper();
        let pid = child.id();
        drop(LaunchGuard::child(child));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while alive(pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(!alive(pid), "the abandoned launch's process {pid} kept running");

        let mut kept = sleeper();
        let mut g = LaunchGuard::child(std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) { &["/c", "exit", "0"][..] } else { &[][..] })
            .spawn()
            .expect("spawn"));
        g.disarm();
        drop(g);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let survived = kept.try_wait().expect("try_wait").is_none();
        let _ = kept.kill();
        let _ = kept.wait();
        assert!(survived, "a disarmed guard ended an unrelated process");
    }

    /// `launch` arms the guard before the port wait and disarms it only once
    /// the browser is handed back.
    #[test]
    fn launch_guards_the_process_until_it_is_handed_back() {
        let src = include_str!("launch.rs");
        let start = src.find("pub async fn launch(").expect("launch");
        let b = &src[start..start + src[start..].find("\n}").expect("end")];
        let armed = b.find("LaunchGuard::").expect("launch does not guard its process");
        let wait = b.find("wait_for_port(").expect("port wait");
        let disarm = b.find(".disarm()").expect("never disarmed");
        assert!(armed < wait && wait < disarm, "{b}");
    }

    #[test]
    fn windowed_on_a_hidden_desktop_keeps_ordinary_window_coordinates() {
        let opts = LaunchOptions {
            mode: LaunchMode::Windowed,
            desktop: Some("ghost-1-auto".into()),
            ..Default::default()
        };
        let args = base_args(&opts);
        assert!(!args.iter().any(|a| a.starts_with("--window-position")));
        assert!(!args.iter().any(|a| a == "--headless=new"));
    }

    #[test]
    fn command_line_quoting_round_trips_spaces_and_quotes() {
        assert_eq!(quote_arg("--flag"), "--flag");
        assert_eq!(quote_arg(r"C:\Program Files\x.exe"), r#""C:\Program Files\x.exe""#);
        // An embedded quote is escaped; a backslash run before it is doubled.
        assert_eq!(quote_arg(r#"a"b"#), r#""a\"b""#);
        // A backslash not followed by a quote is literal, so a trailing one needs
        // no quoting on its own - but inside quotes it must be doubled.
        assert_eq!(quote_arg(r"trail\"), r"trail\");
        assert_eq!(quote_arg(r"C:\t p\"), r#""C:\t p\\""#);
        assert_eq!(quote_arg(""), r#""""#);
        let line = windows_command_line(
            Path::new(r"C:\Program Files\c.exe"),
            &["--a".into(), r"--user-data-dir=C:\t p".into()],
        );
        assert_eq!(line, r#""C:\Program Files\c.exe" --a "--user-data-dir=C:\t p""#);
    }

    #[test]
    fn port_file_first_line_is_the_port() {
        assert_eq!(
            parse_port_file("54321\n/devtools/browser/abc\n"),
            Some(54321)
        );
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
        let opts = LaunchOptions {
            mode: LaunchMode::Headless,
            ..Default::default()
        };
        let args = base_args(&opts);
        assert!(args.iter().any(|a| a == "--headless=new"));
        assert!(!args.iter().any(|a| a.starts_with("--window-position")));
    }

    #[test]
    fn headless_mode_composites_in_software_and_only_headless_does() {
        // Without it a headless capture waits ~1 s for Chrome's next GPU frame
        // (docs/benches/2026-11-25-stage-3). Windowed browsers draw for a real
        // or hidden desktop and keep their GPU compositor.
        let headless = base_args(&LaunchOptions { mode: LaunchMode::Headless, ..Default::default() });
        assert!(headless.iter().any(|a| a == "--disable-gpu-compositing"));
        assert!(!headless.iter().any(|a| a == "--disable-gpu"), "WebGL must keep its GPU");
        for desktop in [None, Some("ghost-1-auto".to_string())] {
            let windowed = base_args(&LaunchOptions { mode: LaunchMode::Windowed, desktop, ..Default::default() });
            assert!(!windowed.iter().any(|a| a == "--disable-gpu-compositing"));
        }
    }

    #[test]
    fn windowed_mode_starts_off_the_visible_desktop() {
        let opts = LaunchOptions {
            mode: LaunchMode::Windowed,
            ..Default::default()
        };
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

    /// The mechanism itself, with a throwaway process: closing the job ends it.
    #[cfg(windows)]
    #[test]
    fn a_job_member_dies_when_the_job_closes() {
        use std::os::windows::process::CommandExt;
        use std::time::{Duration, Instant};
        let job = job::Job::new().expect("job");
        let mut child = std::process::Command::new("ping.exe")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("child");
        job.adopt(child.id()).expect("adopt");
        assert!(child.try_wait().unwrap().is_none(), "alive while the job is open");
        drop(job);
        let deadline = Instant::now() + Duration::from_secs(3);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("the member outlived the job");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn profile_dir_is_per_process() {
        let dir = default_profile_dir();
        assert!(dir
            .to_string_lossy()
            .contains(&format!("p{}", std::process::id())));
    }

    #[test]
    fn launch_mode_parses_common_spellings() {
        assert_eq!("headless".parse::<LaunchMode>(), Ok(LaunchMode::Headless));
        assert_eq!("Windowed".parse::<LaunchMode>(), Ok(LaunchMode::Windowed));
        assert!("nope".parse::<LaunchMode>().is_err());
    }
}
