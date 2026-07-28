//! Window enumeration and focus.
//!
//! | Ghost operation | Apple API |
//! | --- | --- |
//! | list windows | `CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly)` |
//! | window title / pid / bounds | `kCGWindowName`, `kCGWindowOwnerPID`, `kCGWindowBounds` |
//! | frontmost app | `[NSWorkspace sharedWorkspace].frontmostApplication` |
//! | running apps | `[NSWorkspace sharedWorkspace].runningApplications` |
//! | focus an app | `[NSRunningApplication activateWithOptions:]` |
//! | launch an app | `[NSWorkspace launchApplication:]` |
//! | raise a window within an app | `AXUIElementPerformAction(kAXRaiseAction)` |
//!
//! # Why titles are matched rather than handles being derived
//!
//! Capture needs a `CGWindowID`; Accessibility gives out `AXUIElement`s. The
//! private `_AXUIElementGetWindow` symbol converts between them and is what most
//! tools use — but linking an undocumented symbol can fail at link time on a
//! future macOS, and it would fail on the *partner's* machine rather than in CI
//! (an rlib does not link, so CI would stay green). Ghost instead correlates the
//! two worlds through public API only: owning pid plus window title, from
//! `CGWindowListCopyWindowInfo`. Slightly weaker when one app has two
//! identically-titled windows, and it cannot vanish from under us.
//!
//! `kCGWindowName` is itself gated: without Screen Recording, macOS returns the
//! window list with titles omitted. [`list_windows`] reports what it can and
//! [`titles_available`] says whether titles were readable, so a caller can tell
//! "no match" from "titles were withheld".

use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::CFString;
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowListExcludeDesktopElements,
    kCGWindowListOptionOnScreenOnly,
};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};
use objc2_foundation::NSString;

use super::ax::{name_matches, AxElement};
use super::error::{MacError, MacResult};
use super::ffi::{as_dictionary, as_f64, as_i64, as_string};
use crate::types::{Rect, WindowRef};

/// One on-screen window, as CoreGraphics sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacWindow {
    /// `kCGWindowNumber` — the id `CGWindowListCreateImage` needs.
    pub window_id: u32,
    /// `kCGWindowOwnerPID` — the pid to build an `AXUIElement` from.
    pub pid: i32,
    /// `kCGWindowName`, empty when Screen Recording is not granted.
    pub title: String,
    /// `kCGWindowOwnerName` — the application name, readable without any grant.
    pub app_name: String,
    /// `kCGWindowBounds`, in points.
    pub bounds: Rect,
    /// True when this window belongs to the frontmost application.
    pub focused: bool,
}

impl MacWindow {
    /// The platform-neutral shape `ghost_window list` returns.
    pub fn as_window_ref(&self) -> WindowRef {
        WindowRef {
            title: if self.title.is_empty() {
                self.app_name.clone()
            } else {
                self.title.clone()
            },
            id: self.window_id as i64,
            focused: self.focused,
        }
    }
}

/// Only normal application windows live on layer 0. Menus, the Dock, the menu bar,
/// tooltips and notifications sit on higher layers; including them would fill an
/// agent's window list with things it cannot act on.
const NORMAL_WINDOW_LAYER: i64 = 0;

/// Enumerate on-screen windows — `CGWindowListCopyWindowInfo`.
///
/// Windows with zero area or on a non-zero layer are filtered out. Sorted by
/// window id so repeated calls are stable.
pub fn list_windows() -> MacResult<Vec<MacWindow>> {
    let frontmost = frontmost_pid();

    // `copy_window_info` hands back an untyped array; each element is a
    // CFDictionaryRef describing one window.
    let info = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    )
    .ok_or_else(|| MacError::CaptureFailed("CGWindowListCopyWindowInfo returned null".into()))?;

    let mut out = Vec::new();
    for entry in info.iter() {
        let raw = *entry as CFDictionaryRef;
        if raw.is_null() {
            continue;
        }
        // SAFETY: the array owns each dictionary, so `raw` is live for as long as
        // `info`; the get rule retains it for the body of this loop.
        let dict = unsafe { CFDictionary::<CFString, CFType>::wrap_under_get_rule(raw) };

        let layer = dict_i64(&dict, "kCGWindowLayer").unwrap_or(NORMAL_WINDOW_LAYER);
        if layer != NORMAL_WINDOW_LAYER {
            continue;
        }

        let Some(window_id) = dict_i64(&dict, "kCGWindowNumber") else {
            continue;
        };
        let pid = dict_i64(&dict, "kCGWindowOwnerPID").unwrap_or(0) as i32;
        let bounds = dict_bounds(&dict).unwrap_or(Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        });
        if bounds.width() == 0 || bounds.height() == 0 {
            continue;
        }

        out.push(MacWindow {
            window_id: window_id as u32,
            pid,
            title: dict_string(&dict, "kCGWindowName").unwrap_or_default(),
            app_name: dict_string(&dict, "kCGWindowOwnerName").unwrap_or_default(),
            bounds,
            focused: Some(pid) == frontmost,
        });
    }

    out.sort_by_key(|w| w.window_id);
    Ok(out)
}

/// Whether the window list came back with titles.
///
/// `kCGWindowName` requires Screen Recording. When it is missing, every title is
/// empty, and "I could not find a window called X" would be misleading — the real
/// answer is "titles were withheld".
pub fn titles_available(windows: &[MacWindow]) -> bool {
    windows.iter().any(|w| !w.title.is_empty())
}

/// Find a window by title substring, falling back to the owning application's
/// name so a lookup still works when Screen Recording has hidden the titles.
pub fn find_window(query: &str) -> MacResult<MacWindow> {
    let windows = list_windows()?;

    if let Some(found) = windows.iter().find(|w| name_matches(&w.title, query)) {
        return Ok(found.clone());
    }
    if let Some(found) = windows.iter().find(|w| name_matches(&w.app_name, query)) {
        return Ok(found.clone());
    }

    let hint = if titles_available(&windows) {
        String::new()
    } else {
        " (window titles are unavailable without Screen Recording permission, so only application names could be matched)"
    .to_string()
    };
    Err(MacError::WindowNotFound(format!("{query:?}{hint}")))
}

/// The pid of the frontmost application — `NSWorkspace.frontmostApplication`.
pub fn frontmost_pid() -> Option<i32> {
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    Some(app.processIdentifier())
}

/// The localized name of the frontmost application.
///
/// This is what `ghost doctor --mac` asserts against after a focus change, since
/// it is the OS's own opinion about what is in front.
pub fn frontmost_app_name() -> Option<String> {
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    app.localizedName().map(|s| s.to_string())
}

/// Bring an application to the foreground — `activateWithOptions:`.
///
/// `NSApplicationActivateAllWindows` matches what clicking the Dock icon does.
/// Activation is asynchronous: the call returning `true` means the request was
/// accepted, not that the app is already frontmost, so a caller that needs to be
/// sure must poll [`frontmost_pid`].
pub fn focus_app(pid: i32) -> MacResult<()> {
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .ok_or_else(|| MacError::WindowNotFound(format!("no running application with pid {pid}")))?;

    let ok = app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
    if ok {
        Ok(())
    } else {
        Err(MacError::Unsupported(format!(
            "the system refused to activate pid {pid}"
        )))
    }
}

/// Focus a window: activate its application, then raise the window within that app
/// via `kAXRaiseAction`.
///
/// Both halves are needed. Activating alone leaves the app's other window in
/// front; raising alone leaves the app itself in the background.
pub fn focus_window(window: &MacWindow) -> MacResult<()> {
    focus_app(window.pid)?;

    let app = AxElement::for_app(window.pid)?;
    for ax_window in app.windows()? {
        let title = ax_window.name()?;
        if window.title.is_empty() || name_matches(&title, &window.title) {
            // Raising is best-effort: some windows do not expose AXRaise, and
            // activating the app has already done the important part.
            let _ = ax_window.raise();
            return Ok(());
        }
    }
    Ok(())
}

/// The pid of a running application, matched on its localized name —
/// `NSWorkspace.runningApplications`.
///
/// Matching is the same substring/case-insensitive rule the rest of the backend
/// uses, so `"textedit"` finds `TextEdit`.
pub fn running_app_pid(name: &str) -> Option<i32> {
    let workspace = NSWorkspace::sharedWorkspace();
    let apps = workspace.runningApplications();
    // Indexed rather than iterated: `NSArray::iter` lives behind objc2's
    // NSEnumerator feature, and pulling that in for one loop is not worth it.
    for i in 0..apps.count() {
        let app = apps.objectAtIndex(i);
        if app
            .localizedName()
            .is_some_and(|n| name_matches(&n.to_string(), name))
        {
            return Some(app.processIdentifier());
        }
    }
    None
}

/// Launch an application by name — `NSWorkspace.launchApplication:`.
///
/// If the app is already running this activates it instead of starting a second
/// copy, which is why [`super::MacBackend`] can call it unconditionally.
///
/// # Why the deprecated call
///
/// The replacement, `openApplicationAtURL:configuration:completionHandler:`,
/// needs a resolved bundle URL and delivers its result to a block on another
/// queue. Ghost launches an app from exactly one place — `ghost doctor --mac`,
/// which then polls [`running_app_pid`] anyway — so the async API would add a
/// block, a URL lookup and a channel to reach the same place. The deprecated
/// selector still ships in macOS 15.
pub fn launch_app(name: &str) -> MacResult<()> {
    let workspace = NSWorkspace::sharedWorkspace();
    #[allow(deprecated)]
    let ok = workspace.launchApplication(&NSString::from_str(name));
    if ok {
        Ok(())
    } else {
        Err(MacError::WindowNotFound(format!(
            "no application named {name:?} could be launched"
        )))
    }
}

/// Poll until an application is running, or the deadline passes.
///
/// Returns its pid. Launching is asynchronous — `launchApplication:` returns as
/// soon as the request is accepted — so every caller that needs the app to exist
/// has to wait for it explicitly.
pub fn wait_for_app(name: &str, timeout: std::time::Duration) -> MacResult<i32> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(pid) = running_app_pid(name) {
            return Ok(pid);
        }
        if std::time::Instant::now() >= deadline {
            return Err(MacError::Timeout(format!(
                "{name:?} did not start within {:?}",
                timeout
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Poll until an application has at least `count` windows, or the deadline passes.
///
/// Used by `ghost doctor --mac` to tell "File > New worked" from "the menu item
/// was clicked and nothing happened": the window appears some frames after the
/// click returns.
pub fn wait_for_window_count(
    pid: i32,
    count: usize,
    timeout: std::time::Duration,
) -> MacResult<usize> {
    let deadline = std::time::Instant::now() + timeout;
    let mut seen = 0;
    loop {
        if let Ok(app) = AxElement::for_app(pid) {
            if let Ok(windows) = app.windows() {
                seen = windows.len();
                if seen >= count {
                    return Ok(seen);
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(MacError::Timeout(format!(
                "pid {pid} had {seen} window(s), expected at least {count}, after {timeout:?}"
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

fn dict_value(dict: &CFDictionary<CFString, CFType>, key: &str) -> Option<CFType> {
    dict.find(CFString::new(key)).map(|v| v.clone())
}

fn dict_string(dict: &CFDictionary<CFString, CFType>, key: &str) -> Option<String> {
    dict_value(dict, key).as_ref().and_then(as_string)
}

fn dict_i64(dict: &CFDictionary<CFString, CFType>, key: &str) -> Option<i64> {
    dict_value(dict, key).as_ref().and_then(as_i64)
}

/// `kCGWindowBounds` is a nested dictionary of X/Y/Width/Height, not a CGRect.
fn dict_bounds(dict: &CFDictionary<CFString, CFType>) -> Option<Rect> {
    let bounds = as_dictionary(&dict_value(dict, "kCGWindowBounds")?)?;
    let x = as_f64(&dict_value(&bounds, "X")?)?;
    let y = as_f64(&dict_value(&bounds, "Y")?)?;
    let w = as_f64(&dict_value(&bounds, "Width")?)?;
    let h = as_f64(&dict_value(&bounds, "Height")?)?;
    Some(bounds_to_rect(x, y, w, h))
}

/// Convert CoreGraphics window bounds into Ghost's [`Rect`].
///
/// Split out so the arithmetic is testable without a window server.
pub fn bounds_to_rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    let left = x.round() as i32;
    let top = y.round() as i32;
    let right = (x + width).round() as i32;
    let bottom = (y + height).round() as i32;
    Rect {
        left,
        top,
        right: right.max(left),
        bottom: bottom.max(top),
    }
}

#[cfg(all(test, feature = "mac-headless-tests"))]
mod tests {
    use super::*;

    #[test]
    fn window_bounds_convert_to_a_rect() {
        let r = bounds_to_rect(100.0, 50.0, 400.0, 300.0);
        assert_eq!(
            r,
            Rect {
                left: 100,
                top: 50,
                right: 500,
                bottom: 350
            }
        );
        assert_eq!(r.width(), 400);
        assert_eq!(r.height(), 300);
    }

    #[test]
    fn a_degenerate_bounds_never_produces_an_inverted_rect() {
        let r = bounds_to_rect(10.0, 10.0, -5.0, 0.0);
        assert!(r.right >= r.left);
        assert!(r.bottom >= r.top);
        assert_eq!(r.width(), 0);
    }

    #[test]
    fn a_window_on_a_secondary_display_keeps_negative_coordinates() {
        let r = bounds_to_rect(-1920.0, -50.0, 800.0, 600.0);
        assert_eq!(r.left, -1920);
        assert_eq!(r.top, -50);
        assert_eq!(r.right, -1120);
    }

    #[test]
    fn a_window_ref_falls_back_to_the_app_name_when_the_title_is_withheld() {
        // Without Screen Recording, kCGWindowName is empty. Reporting a blank
        // title would make the window unaddressable, so the app name stands in.
        let untitled = MacWindow {
            window_id: 7,
            pid: 501,
            title: String::new(),
            app_name: "TextEdit".into(),
            bounds: bounds_to_rect(0.0, 0.0, 100.0, 100.0),
            focused: true,
        };
        let r = untitled.as_window_ref();
        assert_eq!(r.title, "TextEdit");
        assert_eq!(r.id, 7);
        assert!(r.focused);

        let titled = MacWindow {
            title: "Untitled.txt".into(),
            ..untitled
        };
        assert_eq!(titled.as_window_ref().title, "Untitled.txt");
    }

    #[test]
    fn titles_available_distinguishes_withheld_from_merely_absent() {
        let base = MacWindow {
            window_id: 1,
            pid: 1,
            title: String::new(),
            app_name: "App".into(),
            bounds: bounds_to_rect(0.0, 0.0, 10.0, 10.0),
            focused: false,
        };
        assert!(!titles_available(std::slice::from_ref(&base)));
        assert!(!titles_available(&[]));
        assert!(titles_available(&[
            base.clone(),
            MacWindow {
                title: "Real Title".into(),
                ..base
            }
        ]));
    }

    #[test]
    fn enumerating_windows_works_on_a_headless_runner() {
        // A CI runner has a window server but no logged-in GUI session, so the
        // list is usually empty. The contract under test is that enumeration
        // succeeds and yields well-formed entries rather than erroring.
        let windows = list_windows().expect("window enumeration must not fail");
        for w in &windows {
            assert!(w.bounds.width() > 0, "{w:?} has zero width");
            assert!(w.bounds.height() > 0, "{w:?} has zero height");
        }
        // Ids are unique and sorted.
        let ids: Vec<u32> = windows.iter().map(|w| w.window_id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids, sorted, "window ids must be sorted and unique");
    }

    #[test]
    fn at_most_one_window_is_reported_focused_per_process_group() {
        let windows = list_windows().expect("enumerate");
        let focused_pids: std::collections::HashSet<i32> =
            windows.iter().filter(|w| w.focused).map(|w| w.pid).collect();
        assert!(
            focused_pids.len() <= 1,
            "more than one app reported frontmost: {focused_pids:?}"
        );
    }
}
