//! Live tests against the Ghost Testbed window (crates/ghost-testbed), on a
//! hidden desktop, through the background paths only.
//!
//! These replace the Notepad tests. Windows 11 Notepad is a single-instance
//! Store app that restores the user's own tabs into whatever instance starts -
//! a test that typed into "the first document" typed into the user's unsaved
//! file. The testbed has no session and no singleton, and driving it on a
//! hidden desktop means the suite never touches the screen, the keyboard focus,
//! or an app the user has open.
//!
//! `#[ignore]` gated: needs a real Windows session and the testbed binary
//! (`cargo build -p ghost-testbed --release`). Run through
//! `scripts/live-on-hidden-desktop.ps1`, or directly:
//!   cargo test -p ghost-session --test testbed -- --ignored --nocapture

#![cfg(windows)]

use ghost_core::desktop::DesktopSession;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The built testbed, whichever profile built it last.
fn testbed_exe() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("target");
    ["release", "debug"]
        .iter()
        .map(|p| root.join(p).join("ghost-testbed.exe"))
        .find(|p| p.exists())
}

struct Bed {
    desktop: DesktopSession,
    hwnd: isize,
    pid: u32,
}

impl Drop for Bed {
    /// A testbed left running on a desktop nobody can see would hold its own
    /// binary open and make the next `cargo build` fail - which is exactly what
    /// happened while writing these tests.
    fn drop(&mut self) {
        let _ = ghost_core::process::kill(self.pid);
    }
}

/// Start a testbed on a fresh hidden desktop and wait for its window.
fn testbed(label: &str) -> Option<Bed> {
    let exe = match testbed_exe() {
        Some(e) => e,
        None => {
            eprintln!("skipped: build the testbed first (cargo build -p ghost-testbed --release)");
            return None;
        }
    };
    let desktop = DesktopSession::create(label).expect("hidden desktop");
    let title = format!("Ghost Testbed {label}");
    desktop
        .launch(&format!("\"{}\" --title \"{title}\"", exe.display()))
        .expect("launch testbed");
    let w = desktop
        .wait_for_window(&title, 10_000)
        .expect("testbed window on the hidden desktop");
    Some(Bed { desktop, hwnd: w.hwnd, pid: w.pid })
}

fn title_of(bed: &Bed) -> String {
    bed.desktop
        .windows()
        .unwrap_or_default()
        .into_iter()
        .find(|w| w.hwnd == bed.hwnd)
        .map(|w| w.title)
        .unwrap_or_default()
}

fn wait_until(mut f: impl FnMut() -> bool, ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(ms);
    loop {
        if f() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Prints what the targeting resolves to, and guards the two defects this file
/// found: `read_text` returned only the FIRST CHARACTER (the message result was
/// discarded), and a name search could return the window FRAME when the title
/// contained the control's name.
#[test]
#[ignore]
fn diagnose_targets() {
    // Label chosen so the window TITLE contains the searched control name
    // ("Quit"), which is exactly the case that used to return the frame.
    let Some(bed) = testbed("quit-diag") else { return };
    let hwnd = bed.hwnd;
    let info = bed
        .desktop
        .exec(move || {
            use ghost_core::input::{window_class, BackgroundClicker};
            let focused = BackgroundClicker::focused_control(hwnd);
            let text = BackgroundClicker::text_target(hwnd);
            let first = BackgroundClicker::first_text_control(hwnd);
            let set = text.map(|t| BackgroundClicker::set_text(t, "diag-value").is_ok());
            let read = text.and_then(BackgroundClicker::read_text);
            format!(
                "frame={hwnd} ({}) focused={focused} ({}) text_target={text:?} ({}) first_text={first:?} set_text_ok={set:?} read={read:?}",
                window_class(hwnd),
                window_class(focused),
                text.map(window_class).unwrap_or_default(),
            )
        })
        .expect("worker");
    eprintln!("TARGETS: {info}");
    assert!(
        info.contains(r#"read=Some("diag-value")"#),
        "the read-back must return the WHOLE value, not its first character: {info}"
    );
    let quit = bed
        .desktop
        .with_uia(move |tree| {
            let el = tree.find_by_name_in_hwnd(hwnd, "Quit").ok().flatten();
            match el {
                Some(e) => format!(
                    "found name={:?} role={} hwnd={} class={}",
                    e.name(),
                    ghost_core::uia::element::role_id_to_name(e.control_type()),
                    e.native_window_handle(),
                    ghost_core::input::window_class(e.native_window_handle())
                ),
                None => "no match for 'Quit'".into(),
            }
        })
        .expect("uia");
    eprintln!("QUIT LOOKUP: {quit}");
    assert!(
        quit.contains("role=button") && quit.contains("class=button"),
        "a name search inside a window must find the CONTROL, never the frame \
         (this window's own title contains the searched name): {quit}"
    );
    let ctrl = bed
        .desktop
        .with_uia(move |tree| {
            tree.find_by_name_in_hwnd(hwnd, "Quit")
                .ok()
                .flatten()
                .map(|e| e.native_window_handle())
                .unwrap_or(0)
        })
        .expect("uia");
    let clicked = bed
        .desktop
        .exec(move || format!("{:?}", ghost_core::input::BackgroundClicker::button_click(ctrl)))
        .expect("worker");
    std::thread::sleep(Duration::from_millis(800));
    eprintln!(
        "QUIT CLICK: bm_click={clicked} windows_after={:?}",
        bed.desktop.windows().map(|w| w.into_iter().map(|x| x.title).collect::<Vec<_>>())
    );
}

/// Typing must never report success without the text reading back, and on a
/// classic EDIT control it must read back.
#[test]
#[ignore]
fn typing_reads_back_from_the_edit_control() {
    let Some(bed) = testbed("type") else { return };
    bed.desktop.type_text(bed.hwnd, "hello testbed").expect("type");
    let seen = bed.desktop.read_text(bed.hwnd).expect("read back");
    assert!(seen.contains("hello testbed"), "read back {seen:?}");
}

/// A background click on a real button is observable from outside: the
/// testbed rewrites its title with the click count.
#[test]
#[ignore]
fn clicking_the_button_updates_the_title() {
    let Some(bed) = testbed("click") else { return };
    let hwnd = bed.hwnd;
    let (x, y) = bed
        .desktop
        .with_uia(move |tree| {
            let el = tree
                .find_by_name_in_hwnd(hwnd, "Increment")
                .expect("walk")
                .expect("Increment button");
            let r = el.bounding_rect().expect("rect");
            r.center()
        })
        .expect("uia on the hidden desktop");
    bed.desktop.click(hwnd, x, y).ok();
    // A posted client-area click at a screen point is not what a Win32 button
    // needs; drive it the way the product does - BM_CLICK on the control itself.
    let ctrl = bed
        .desktop
        .with_uia(move |tree| {
            tree.find_by_name_in_hwnd(hwnd, "Increment")
                .expect("walk")
                .expect("button")
                .native_window_handle()
        })
        .expect("uia");
    assert_ne!(ctrl, 0, "a Win32 BUTTON must expose its own handle");
    bed.desktop
        .exec(move || ghost_core::input::BackgroundClicker::button_click(ctrl))
        .expect("worker")
        .expect("BM_CLICK");
    assert!(
        wait_until(|| title_of(&bed).contains("[clicks="), 3_000),
        "title did not record the click: {:?}",
        title_of(&bed)
    );
}

/// The edit control and the two buttons are what the accessibility tree shows,
/// with the label-derived name on the edit.
#[test]
#[ignore]
fn describe_lists_the_controls_by_name_and_role() {
    let Some(bed) = testbed("describe") else { return };
    let hwnd = bed.hwnd;
    let els = bed
        .desktop
        .with_uia(move |tree| tree.describe_hwnd(hwnd).expect("describe"))
        .expect("uia");
    let has = |name: &str, role: &str| els.iter().any(|e| e.name == name && e.role == role);
    assert!(has("Increment", "button"), "{els:?}");
    assert!(has("Quit", "button"), "{els:?}");
    assert!(has("Field", "edit"), "the EDIT must be named after its label: {els:?}");
}

/// The suite must not leave processes behind on a desktop nobody can see.
#[test]
#[ignore]
fn quit_ends_the_process_and_the_window_goes_away() {
    let Some(bed) = testbed("quit") else { return };
    let hwnd = bed.hwnd;
    let ctrl = bed
        .desktop
        .with_uia(move |tree| {
            tree.find_by_name_in_hwnd(hwnd, "Quit")
                .expect("walk")
                .expect("Quit")
                .native_window_handle()
        })
        .expect("uia");
    assert_ne!(ctrl, 0, "the Quit BUTTON must expose its own handle");
    bed.desktop
        .exec(move || ghost_core::input::BackgroundClicker::button_click(ctrl))
        .expect("worker")
        .expect("BM_CLICK");
    assert!(
        wait_until(|| title_of(&bed).is_empty(), 5_000),
        "window still present after Quit (title {:?}, windows {:?})",
        title_of(&bed),
        bed.desktop.windows().map(|w| w.into_iter().map(|x| x.title).collect::<Vec<_>>())
    );
}
