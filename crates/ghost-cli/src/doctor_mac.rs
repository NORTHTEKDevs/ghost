//! `ghost doctor --mac` — the on-device verification the macOS backend is waiting on.
//!
//! # Why this command exists
//!
//! `crates/ghost-platform/src/macos` compiles and links against Apple's SDK in CI,
//! which proves the FFI is well-formed and proves nothing about whether TextEdit
//! actually receives a keystroke. `capabilities_for(Platform::MacOS).functional` is
//! therefore `false`. This command is what makes it possible to change that: it
//! drives every implemented capability against a real app and prints a machine-
//! readable verdict.
//!
//! The whole test protocol for a Mac owner is one command. Nobody has to read this
//! code, set up a toolchain, or interpret a stack trace — they run it once and send
//! back the JSON. See `docs/mac-testing.md`.
//!
//! # Why every step is scored rather than asserted
//!
//! A `panic!` on the first failure would report one problem per round-trip to a
//! person who may not be available for another. Each step is therefore independent,
//! timed, and recorded with what was expected and what was observed, so a single run
//! says as much as possible about a machine we cannot log into.

use std::fmt;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use ghost_platform::macos::ax::AxElement;
use ghost_platform::macos::error::MacError;
use ghost_platform::macos::{clipboard, input, perms, window, MacBackend};
use ghost_platform::types::Locator;

/// The app driven by the smoke tests.
///
/// TextEdit because it ships with every macOS install, needs no configuration, and
/// has a genuine accessibility implementation (an `AXTextArea` that reports its
/// value) rather than a custom-drawn canvas.
const TARGET_APP: &str = "TextEdit";

/// The text typed and then read back.
const PROBE_TEXT: &str = "hello ghost";

/// How long to wait for the user to grant a permission in System Settings.
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(60);
const PERMISSION_POLL: Duration = Duration::from_secs(2);

/// How long an app gets to appear after being asked to launch. Cold-starting
/// TextEdit on a busy machine is slower than the 5s a window is then given.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(15);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(5);

/// The accessibility tree of a text editor is shallow. Bounding the walk stops a
/// pathological or cyclic tree from hanging the one run we get.
const SEARCH_DEPTH: u32 = 12;

/// Menus are deeper than a document window: menu bar > menu > item.
const MENU_DEPTH: u32 = 4;

/// After a synthesized keystroke, the app processes it on its own run loop. Nothing
/// in the AX API is a barrier, so a read-back immediately after a keypress can
/// legitimately observe the old value. This is the settle time before reading.
const SETTLE: Duration = Duration::from_millis(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Pass,
    Fail,
    /// Not applicable on this platform, by design. Does not affect the exit code.
    Skip,
    /// The step ran but could not decide. Counts as a failure, because an
    /// unverified capability is exactly what this command exists to eliminate.
    Unknown,
}

impl Outcome {
    fn as_str(&self) -> &'static str {
        match self {
            Outcome::Pass => "PASS",
            Outcome::Fail => "FAIL",
            Outcome::Skip => "SKIP",
            Outcome::Unknown => "UNKNOWN",
        }
    }

    /// ANSI colour. Written by hand rather than pulling in a colour crate for one
    /// summary table.
    fn colour(&self) -> &'static str {
        match self {
            Outcome::Pass => "\x1b[32m",
            Outcome::Fail => "\x1b[31m",
            Outcome::Skip => "\x1b[90m",
            Outcome::Unknown => "\x1b[33m",
        }
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One capability, exercised once.
#[derive(Debug, Clone, serde::Serialize)]
struct Step {
    capability: &'static str,
    target_app: &'static str,
    expected: String,
    observed: String,
    result: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    ms: u128,
}

/// The whole report, as written to stdout and to `~/.ghost/`.
#[derive(Debug, Clone, serde::Serialize)]
struct Report {
    ghost_version: &'static str,
    /// Seconds since the Unix epoch. Deliberately not a formatted date: this file
    /// is read by a maintainer alongside a CI log, and adding a date-formatting
    /// dependency to learn the timezone of a machine we do not own is not worth it.
    unix_time: u64,
    arch: &'static str,
    accessibility_granted: bool,
    screen_recording_granted: bool,
    /// Kept honest on purpose: this report is the evidence for flipping the flag,
    /// so it records the value that was in force while the evidence was gathered.
    reported_functional: bool,
    steps: Vec<Step>,
    passed: usize,
    failed: usize,
    skipped: usize,
}

/// What a step's body returns: what was observed, and whether that is what we wanted.
type StepResult = Result<(Outcome, String), MacError>;

/// Run one step, timing it and converting an error into a `FAIL` row.
///
/// Taking a closure rather than letting each step build its own `Step` keeps the
/// timing and the error handling in one place, so no step can forget either.
fn step(
    steps: &mut Vec<Step>,
    capability: &'static str,
    expected: impl Into<String>,
    body: impl FnOnce() -> StepResult,
) -> Outcome {
    let expected = expected.into();
    let started = Instant::now();
    let (result, observed, error) = match body() {
        Ok((outcome, observed)) => (outcome, observed, None),
        Err(e) => (Outcome::Fail, "error".to_string(), Some(e.to_string())),
    };
    let row = Step {
        capability,
        target_app: TARGET_APP,
        expected,
        observed,
        result: result.as_str(),
        error,
        ms: started.elapsed().as_millis(),
    };
    println!(
        "  {}{:<7}\x1b[0m {:<22} {}",
        result.colour(),
        result,
        capability,
        row.observed
    );
    if let Some(e) = &row.error {
        println!("          \x1b[31m{e}\x1b[0m");
    }
    steps.push(row);
    result
}

/// Shorthand for the common "observed matches expected" case.
fn verdict(ok: bool, observed: impl Into<String>) -> StepResult {
    Ok((
        if ok { Outcome::Pass } else { Outcome::Fail },
        observed.into(),
    ))
}

pub fn run() -> ExitCode {
    println!("\x1b[1mghost doctor --mac\x1b[0m");
    println!();
    println!("Ghost's macOS backend compiles and links in CI but has never been run on a");
    println!("Mac. This command drives every implemented capability against {TARGET_APP} and");
    println!("writes a JSON report. It will open and then quit {TARGET_APP}.");
    println!();
    println!("Two one-time permissions are needed. macOS ties both to this exact binary, so");
    println!("a rebuilt `ghost` has to be granted them again.");
    println!();

    if let Err(code) = ensure_permissions() {
        return code;
    }

    println!();
    println!("\x1b[1mCapabilities\x1b[0m");
    let steps = run_smoke_tests();
    let report = build_report(steps);

    print_summary(&report);
    emit(&report)
}

/// Acquire both TCC grants, prompting and then polling.
///
/// Returns `Err(ExitCode)` when a grant never arrives, because there is nothing
/// useful left to test: every AX call and every capture would fail identically, and
/// twelve rows of the same permission error is worse than one clear message.
fn ensure_permissions() -> Result<(), ExitCode> {
    acquire(
        "Accessibility",
        "Privacy & Security > Accessibility",
        perms::accessibility_granted,
        || {
            // Returns the trust state at the moment of the call, which is ~always
            // false on a first run: the dialog it raises is asynchronous.
            let _ = perms::prompt_accessibility();
        },
    )?;

    acquire(
        "Screen Recording",
        "Privacy & Security > Screen Recording",
        perms::screen_recording_granted,
        || {
            let _ = perms::request_screen_recording();
        },
    )?;

    println!();
    println!("  \x1b[32mPASS\x1b[0m    Accessibility");
    println!("  \x1b[32mPASS\x1b[0m    Screen Recording");
    Ok(())
}

fn acquire(
    name: &str,
    pane: &str,
    granted: impl Fn() -> bool,
    request: impl FnOnce(),
) -> Result<(), ExitCode> {
    if granted() {
        return Ok(());
    }

    println!("Requesting {name}. Open System Settings > {pane} and enable this binary.");
    request();

    let deadline = Instant::now() + PERMISSION_TIMEOUT;
    while Instant::now() < deadline {
        std::thread::sleep(PERMISSION_POLL);
        if granted() {
            println!("  {name} granted.");
            return Ok(());
        }
        print!(".");
        let _ = std::io::stdout().flush();
    }

    println!();
    eprintln!(
        "ghost: {name} was not granted within {}s.\n\
         Enable it under System Settings > {pane}, then run `ghost doctor --mac` again.\n\
         If the switch is already on, macOS may be holding a stale entry for a previous\n\
         build: remove it with the minus button, re-add this binary, and retry.",
        PERMISSION_TIMEOUT.as_secs()
    );
    Err(ExitCode::from(1))
}

fn run_smoke_tests() -> Vec<Step> {
    let backend = MacBackend;
    let mut steps = Vec::new();

    // --- launch ---
    let launched = step(&mut steps, "launch app", format!("{TARGET_APP} running"), || {
        window::launch_app(TARGET_APP)?;
        let pid = window::wait_for_app(TARGET_APP, LAUNCH_TIMEOUT)?;
        verdict(true, format!("pid {pid}"))
    });
    if launched != Outcome::Pass {
        // Every remaining step targets this app. Reporting eleven identical
        // failures would bury the one that matters.
        return steps;
    }
    let pid = window::running_app_pid(TARGET_APP).unwrap_or(0);

    // --- main window ---
    step(&mut steps, "window appears", "at least 1 window", || {
        let n = window::wait_for_window_count(pid, 1, WINDOW_TIMEOUT)?;
        verdict(n >= 1, format!("{n} window(s)"))
    });

    // --- accessibility tree ---
    //
    // Recent macOS ships TextEdit configured to show an open-file panel instead of
    // a blank document, in which case there is no text area to find. Escape
    // dismisses the panel and Cmd+N asks for the document we actually need. Doing
    // this unconditionally would leave two documents open on a machine where the
    // first launch behaved, so it is a fallback rather than a preamble.
    step(&mut steps, "snapshot", "kAXRole == AXTextArea", || {
        if text_area(pid).is_err() {
            let _ = input::press_key("escape", &[]);
            let _ = input::press_key("n", &[input::Modifier::Command]);
            std::thread::sleep(SETTLE);
        }
        let area = text_area(pid)?;
        let role = area.role()?;
        verdict(role == "AXTextArea", role)
    });

    // --- type and read back ---
    step(&mut steps, "type text", format!("value == {PROBE_TEXT:?}"), || {
        window::focus_window(&window::find_window(TARGET_APP)?)?;
        std::thread::sleep(SETTLE);
        input::type_text(PROBE_TEXT)?;
        std::thread::sleep(SETTLE);

        let observed = text_area(pid)?.value_string()?.unwrap_or_default();
        // `contains` rather than equality: an app is entitled to add a trailing
        // newline or an autocorrection, and the capability under test is "the
        // keystrokes arrived", not "TextEdit left them untouched".
        verdict(observed.contains(PROBE_TEXT), observed)
    });

    // --- menu invocation ---
    let menu = step(&mut steps, "menu File > New", "a second window opens", || {
        let before = AxElement::for_app(pid)?.windows()?.len();
        let app = AxElement::for_app(pid)?;
        let bar = app
            .menu_bar()?
            .ok_or_else(|| MacError::ElementNotFound("kAXMenuBarAttribute".into()))?;
        let file = bar
            .find_child_named("File", MENU_DEPTH)?
            .ok_or_else(|| MacError::ElementNotFound("File menu".into()))?;
        let new = file
            .find_child_named("New", MENU_DEPTH)?
            .ok_or_else(|| MacError::ElementNotFound("File > New item".into()))?;
        new.press()?;

        let after = window::wait_for_window_count(pid, before + 1, WINDOW_TIMEOUT)?;
        verdict(after > before, format!("{before} -> {after} windows"))
    });
    if menu == Outcome::Pass {
        // The new document is frontmost and empty, so the capture and clipboard
        // steps below would read the wrong window. Cmd+W closes an unmodified
        // document without a save prompt, restoring the document holding
        // PROBE_TEXT. Not scored: this is housekeeping, and if it silently fails
        // the read-backs that follow report it.
        let _ = input::press_key("w", &[input::Modifier::Command]);
        std::thread::sleep(SETTLE);
    }

    // --- screenshot ---
    step(&mut steps, "screenshot window", "a decodable, non-blank PNG", || {
        let shot = backend.screenshot_window(TARGET_APP)?;
        let decoded = image::load_from_memory(&shot.png)
            .map_err(|e| MacError::CaptureUnusable(format!("not a decodable PNG: {e}")))?;
        let observed = format!(
            "{}x{} px, {}x{} pt, scale {:.1}, {} bytes{}",
            shot.pixel_width,
            shot.pixel_height,
            shot.region.width(),
            shot.region.height(),
            shot.scale(),
            shot.png.len(),
            if shot.blank { ", BLANK" } else { "" }
        );
        // A blank image is the signature of a missing Screen Recording grant:
        // CoreGraphics returns a valid, fully black picture rather than an error.
        verdict(
            !shot.blank
                && decoded.width() == shot.pixel_width
                && decoded.height() == shot.pixel_height,
            observed,
        )
    });

    // --- clipboard ---
    step(&mut steps, "clipboard", format!("pasteboard has {PROBE_TEXT:?}"), || {
        // Overwritten first so a pasteboard that already happened to hold the probe
        // text cannot make a broken Cmd+C look like a working one.
        clipboard::set_text("ghost-doctor-sentinel")?;
        window::focus_window(&window::find_window(TARGET_APP)?)?;
        std::thread::sleep(SETTLE);
        input::press_key("a", &[input::Modifier::Command])?;
        input::press_key("c", &[input::Modifier::Command])?;
        std::thread::sleep(SETTLE);

        let observed = clipboard::get_text()?.unwrap_or_default();
        verdict(observed.contains(PROBE_TEXT), observed)
    });

    // --- window enumeration ---
    step(&mut steps, "list windows", format!("{TARGET_APP} present"), || {
        let windows = backend.list_windows()?;
        // The neutral `WindowRef` carries the *document* title, which for TextEdit is
        // "Untitled", not the app name. Owning pid is the reliable test; the title
        // match is kept because it is the check an agent would actually write, and
        // knowing which of the two matched is worth a line in the report.
        let by_title = windows
            .iter()
            .any(|w| w.title.to_lowercase().contains(&TARGET_APP.to_lowercase()));
        let by_pid = window::list_windows()?.iter().any(|w| w.pid == pid);
        verdict(
            by_title || by_pid,
            format!(
                "{} window(s) listed; matched by {}",
                windows.len(),
                match (by_title, by_pid) {
                    (true, true) => "title and pid",
                    (true, false) => "title only",
                    (false, true) => "pid only",
                    (false, false) => "neither",
                }
            ),
        )
    });

    // --- focus round-trip ---
    step(&mut steps, "focus app", format!("frontmost == {TARGET_APP}"), || {
        // Away and back, because asserting on an app that was already frontmost
        // would pass without focus_app having done anything.
        backend.focus_window("Finder")?;
        std::thread::sleep(SETTLE);
        let detour = backend.frontmost_app().unwrap_or_default();

        window::focus_app(pid)?;
        std::thread::sleep(SETTLE);
        let observed = backend.frontmost_app().unwrap_or_default();
        verdict(
            observed.to_lowercase().contains(&TARGET_APP.to_lowercase()),
            format!("via {detour:?} -> {observed:?}"),
        )
    });

    // --- element location ---
    step(&mut steps, "locate by role", "an AXTextArea with geometry", || {
        let found = backend.find(TARGET_APP, &Locator::Role("edit".into()))?;
        verdict(
            found.rect.width() > 0 && found.rect.height() > 0,
            format!("{:?} at {:?}", found.role, found.rect),
        )
    });

    // --- background dispatch ---
    steps.push(Step {
        capability: "background dispatch",
        target_app: TARGET_APP,
        expected: "not implemented".into(),
        observed: "skipped".into(),
        result: Outcome::Skip.as_str(),
        error: Some(
            "no macOS equivalent of Windows posted messages; measurement out of scope for this drop."
                .into(),
        ),
        ms: 0,
    });
    println!(
        "  {}{:<7}\x1b[0m {:<22} {}",
        Outcome::Skip.colour(),
        Outcome::Skip,
        "background dispatch",
        "no macOS equivalent of Windows posted messages"
    );

    // --- quit ---
    step(&mut steps, "quit app", format!("{TARGET_APP} exits"), || {
        window::focus_app(pid)?;
        std::thread::sleep(SETTLE);
        input::press_key("q", &[input::Modifier::Command])?;
        std::thread::sleep(SETTLE);
        // TextEdit asks whether to save the text typed above. "Delete" is the
        // sheet's discard button; Cmd+Delete is its keyboard equivalent.
        let _ = input::press_key("delete", &[input::Modifier::Command]);

        let deadline = Instant::now() + WINDOW_TIMEOUT;
        while Instant::now() < deadline {
            if window::running_app_pid(TARGET_APP).is_none() {
                return verdict(true, "exited");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        // Left running is untidy but says nothing about the capability, so it is
        // UNKNOWN rather than FAIL — and UNKNOWN still fails the run, because the
        // point is that a human looks at it.
        Ok((Outcome::Unknown, "still running".into()))
    });

    steps
}

/// The document text area, from the application element down.
fn text_area(pid: i32) -> Result<AxElement, MacError> {
    AxElement::for_app(pid)?
        .find_child_with_role("AXTextArea", SEARCH_DEPTH)?
        .ok_or_else(|| MacError::ElementNotFound("AXTextArea".into()))
}

fn build_report(steps: Vec<Step>) -> Report {
    let state = perms::PermissionState::probe();
    let count = |name: &str| steps.iter().filter(|s| s.result == name).count();
    Report {
        ghost_version: env!("CARGO_PKG_VERSION"),
        unix_time: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        arch: std::env::consts::ARCH,
        accessibility_granted: state.accessibility,
        screen_recording_granted: state.screen_recording,
        reported_functional: ghost_platform::capabilities_for(ghost_platform::Platform::MacOS)
            .functional,
        passed: count("PASS"),
        failed: count("FAIL") + count("UNKNOWN"),
        skipped: count("SKIP"),
        steps,
    }
}

fn print_summary(report: &Report) {
    println!();
    if report.failed == 0 {
        println!(
            "\x1b[32m{} passed, {} skipped.\x1b[0m The macOS backend works on this machine.",
            report.passed, report.skipped
        );
    } else {
        println!(
            "\x1b[31m{} failed\x1b[0m, {} passed, {} skipped.",
            report.failed, report.passed, report.skipped
        );
    }
}

/// Write the report to stdout and to `~/.ghost/doctor-mac-<unix time>.json`.
///
/// Both, because the two readers are different: a person pipes stdout into a
/// message, and the file survives a closed terminal.
fn emit(report: &Report) -> ExitCode {
    let json = match serde_json::to_string_pretty(report) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("ghost: could not serialize the report: {e}");
            return ExitCode::from(1);
        }
    };

    match write_report(report.unix_time, &json) {
        Ok(path) => println!("\nReport written to {}", path.display()),
        // Not fatal: the JSON below is the deliverable, and the file is a
        // convenience. Failing the run because `~/.ghost` is read-only would
        // discard a result that took a person's time to produce.
        Err(e) => eprintln!("\nghost: could not write the report file ({e}); the JSON follows."),
    }

    println!("\n{json}");

    if report.failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn write_report(unix_time: u64, json: &str) -> std::io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| std::io::Error::other("HOME is not set"))?;
    let dir = PathBuf::from(home).join(".ghost");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("doctor-mac-{unix_time}.json"));
    std::fs::write(&path, json)?;
    Ok(path)
}

#[cfg(all(test, feature = "mac-headless-tests"))]
mod tests {
    use super::*;

    fn row(result: Outcome) -> Step {
        Step {
            capability: "x",
            target_app: TARGET_APP,
            expected: "e".into(),
            observed: "o".into(),
            result: result.as_str(),
            error: None,
            ms: 1,
        }
    }

    #[test]
    fn a_skip_does_not_fail_the_run() {
        let report = build_report(vec![row(Outcome::Pass), row(Outcome::Skip)]);
        assert_eq!(report.failed, 0);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.passed, 1);
    }

    #[test]
    fn an_unknown_counts_as_a_failure() {
        // An unverified capability is what this command exists to eliminate, so
        // "could not tell" must not be reported as success.
        let report = build_report(vec![row(Outcome::Pass), row(Outcome::Unknown)]);
        assert_eq!(report.failed, 1);
    }

    #[test]
    fn the_report_records_that_macos_is_still_declared_nonfunctional() {
        // This report is the evidence for flipping the flag. If it ever claims the
        // flag was already true, the evidence is circular.
        assert!(!build_report(Vec::new()).reported_functional);
    }

    #[test]
    fn a_step_body_error_becomes_a_failed_row_rather_than_a_panic() {
        let mut steps = Vec::new();
        let outcome = step(&mut steps, "cap", "something", || {
            Err(MacError::Unsupported("nope".into()))
        });
        assert_eq!(outcome, Outcome::Fail);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].result, "FAIL");
        assert_eq!(steps[0].error.as_deref(), Some("nope"));
    }

    #[test]
    fn every_step_is_serialized_with_the_agreed_keys() {
        // The Mac owner sends this JSON back and it is read by a maintainer who
        // was not there. The key names are the contract.
        let json = serde_json::to_string(&row(Outcome::Pass)).expect("serialize");
        for key in ["capability", "target_app", "expected", "observed", "result", "ms"] {
            assert!(json.contains(key), "{key} missing from {json}");
        }
    }

    #[test]
    fn error_is_omitted_when_a_step_succeeded() {
        let json = serde_json::to_string(&row(Outcome::Pass)).expect("serialize");
        assert!(!json.contains("error"), "{json}");
    }
}
