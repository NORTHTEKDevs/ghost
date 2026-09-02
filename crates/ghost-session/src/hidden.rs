//! Driving windows on Ghost's hidden desktops with the ordinary verbs.
//!
//! Under the default background policy every Ghost-initiated launch goes to a
//! hidden desktop: a Windows desktop object that is never displayed and has its
//! own input queue, so nothing on it can take the human's foreground or
//! keyboard (measured: Edge and Chrome activate their first window on launch in
//! every launch style, so this is the only deterministic way to keep them off
//! the screen). The verbs here let `window=<title>` reach those windows exactly
//! as it reaches windows on the user's desktop, so an agent never has to learn
//! a second vocabulary.
//!
//! Everything runs on the desktop's bound worker (`DesktopSession::exec` /
//! `with_uia`): desktop binding is a thread property, and a UIA client only sees
//! providers on its own thread's desktop. COM objects never leave the worker;
//! the closures return plain data.

#![cfg(windows)]

use crate::engine::capture::{capture_window_printwindow, compute_verification};
use crate::engine::input::BackgroundClicker;
use crate::engine::uia::{patterns, tree::UiaTree, ElementDescriptor, UiaElement};
use crate::error::{GhostError, Result};
use crate::locator::By;
use crate::session::GhostSession;
use crate::target::{Surface, TargetSource, WindowTarget};
use ghost_core::DesktopSession;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The desktop Ghost-initiated launches go to under the background policy.
pub const AUTO_DESKTOP: &str = "auto";

/// How long a launch waits for the new process to show a window before
/// answering without an anchor.
const LAUNCH_WINDOW_WAIT: Duration = Duration::from_millis(5_000);

/// UIA control type id for Button.
const UIA_BUTTON: u32 = 50000;

fn core<T>(r: std::result::Result<T, crate::engine::error::CoreError>) -> Result<T> {
    r.map_err(GhostError::Core)
}

/// Plain-data view of a located element (COM objects cannot leave the worker).
#[derive(Debug, Clone)]
struct Located {
    name: String,
    rect: Option<(i32, i32, i32, i32)>,
    ctrl: isize,
    enabled: bool,
    control_type: u32,
}

impl Located {
    fn of(el: &UiaElement) -> Self {
        Located {
            name: el.name(),
            rect: el
                .bounding_rect()
                .map(|r| (r.left, r.top, r.right, r.bottom)),
            ctrl: el.native_window_handle(),
            enabled: el.is_enabled(),
            control_type: el.control_type(),
        }
    }

    fn center(&self) -> (i32, i32) {
        self.rect
            .map(|(l, t, r, b)| ((l + r) / 2, (t + b) / 2))
            .unwrap_or((0, 0))
    }

    fn rect_json(&self) -> Value {
        match self.rect {
            Some((l, t, r, b)) => json!({ "left": l, "top": t, "right": r, "bottom": b }),
            None => Value::Null,
        }
    }
}

/// Chromium and Electron windows report the `chrome_widgetwin_*` class.
pub(crate) fn is_chromium_window(hwnd: isize) -> bool {
    ghost_core::input::window_class(hwnd).starts_with("chrome_")
}

/// Locate the `index`-th element matching `by` inside `hwnd`'s subtree. Returns
/// the element and how many matched (bounded by the search cap).
///
/// Chromium builds its accessibility tree lazily - the first UIA query on a
/// fresh window is what switches it on, and that query sees almost nothing. A
/// miss on a Chromium window is therefore retried once after a short pause.
fn find_in(
    tree: &UiaTree,
    hwnd: isize,
    by: &By,
    index: Option<usize>,
) -> Result<(UiaElement, usize)> {
    match find_in_once(tree, hwnd, by, index) {
        Err(GhostError::ElementNotFound { .. }) if is_chromium_window(hwnd) => {
            std::thread::sleep(Duration::from_millis(450));
            find_in_once(tree, hwnd, by, index)
        }
        other => other,
    }
}

/// For `action=type` by NAME: the label next to a field carries the same name
/// as the field (the Win32 proxy names an EDIT after its STATIC), and the label
/// comes first in the tree. Typing must land in the editable one, so among the
/// name matches the first editable role wins; otherwise the first match.
fn find_for_typing(tree: &UiaTree, hwnd: isize, by: &By) -> Result<(UiaElement, usize)> {
    let By::Name(n) = by else { return find_in(tree, hwnd, by, None) };
    let all = core(tree.find_all_in_hwnd(hwnd, Some(n), None, 16))?;
    if all.is_empty() {
        return find_in(tree, hwnd, by, None);
    }
    let total = all.len();
    let pos = all
        .iter()
        .position(|el| patterns::is_editable_role(el.control_type()))
        .unwrap_or(0);
    all.into_iter()
        .nth(pos)
        .map(|el| (el, total))
        .ok_or_else(|| GhostError::ElementNotFound {
            query: format!("{by:?} in the hidden-desktop window"),
            screenshot: None,
        })
}

fn find_in_once(
    tree: &UiaTree,
    hwnd: isize,
    by: &By,
    index: Option<usize>,
) -> Result<(UiaElement, usize)> {
    let (name, role) = match by {
        By::Name(n) => (Some(n.as_str()), None),
        By::Role(r) => (None, Some(r.as_str())),
        By::Description(d) => {
            return Err(GhostError::Vision(format!(
                "description targets need vision grounding, which has no hidden-desktop path; \
                 locate by name or role instead (description={d})"
            )))
        }
    };
    // A plain lookup takes the first hit and stops walking (measured: the
    // collecting walk kept visiting a Chromium tree to its node budget, ~2 s,
    // where the first-hit search returns in ~100 ms). Index disambiguation is
    // the only caller that needs the whole match list.
    let Some(idx) = index else {
        let hit = match (name, role) {
            (Some(n), _) => core(tree.find_by_name_in_hwnd(hwnd, n))?,
            (None, Some(r)) => core(tree.find_by_role_in_hwnd(hwnd, r))?,
            (None, None) => None,
        };
        return hit.map(|el| (el, 1)).ok_or_else(|| GhostError::ElementNotFound {
            query: format!("{by:?} in the hidden-desktop window"),
            screenshot: None,
        });
    };
    let all = core(tree.find_all_in_hwnd(hwnd, name, role, 64))?;
    let total = all.len();
    all.into_iter()
        .nth(idx)
        .map(|el| (el, total))
        .ok_or_else(|| GhostError::ElementNotFound {
            query: format!("{by:?} (index {idx}, {total} match(es)) in the hidden-desktop window"),
            screenshot: None,
        })
}

/// An element's current text: the UIA value, or `WM_GETTEXT` on the control's
/// own handle when the value is empty. The Win32 proxy reports an empty value
/// for a classic EDIT on a non-displayed desktop while the control's text is
/// readable directly; the text is also what the proxy uses as the NAME after
/// typing, so name-based lookups of edits are not stable there - use role.
fn element_value(el: &UiaElement) -> String {
    let v = el.get_text();
    if !v.is_empty() {
        return v;
    }
    let ctrl = el.native_window_handle();
    if ctrl != 0 {
        if let Some(t) = BackgroundClicker::read_text(ctrl) {
            return t;
        }
    }
    v
}

/// PrintWindow before/after delta of the whole window. `None` when it cannot
/// be judged (capture failed, size changed, blank surface).
fn pixels_changed(hwnd: isize, before: Option<(Vec<u8>, usize, usize)>) -> Option<bool> {
    let (b, bw, bh) = before?;
    let (a, aw, ah) = capture_window_printwindow(hwnd).ok()?;
    if bw != aw || bh != ah || b.is_empty() {
        return None;
    }
    if a.iter().all(|&p| p == 0) || b.iter().all(|&p| p == 0) {
        return None;
    }
    Some(compute_verification(&b, &a, bw, bh, true).changed)
}

/// Make `hwnd` the active window OF THE HIDDEN DESKTOP. Runs on the desktop's
/// bound worker, so this touches only that desktop's (undisplayed) input queue;
/// the human's foreground cannot change. Some providers (Chromium) service
/// accessibility actions on an inactive window only after an internal timeout.
fn activate_here(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Input::KeyboardAndMouse::SetActiveWindow;
    use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
    unsafe {
        let h = HWND(hwnd as *mut core::ffi::c_void);
        let _ = SetForegroundWindow(h);
        let _ = SetActiveWindow(h);
    }
}

/// Wheel notches for a direction word: positive = up / right, per WM_MOUSEWHEEL.
fn wheel(direction: &str, amount: i32) -> Result<(i32, bool)> {
    let n = amount.max(1);
    Ok(match direction.trim().to_lowercase().as_str() {
        "up" => (n, false),
        "down" => (-n, false),
        "left" => (-n, true),
        "right" => (n, true),
        other => {
            return Err(GhostError::Config(format!(
                "ghost_scroll: unknown direction '{other}'; use up|down|left|right"
            )))
        }
    })
}

impl GhostSession {
    /// The desktop a hidden-surface target lives on.
    async fn desktop_for(&self, t: &WindowTarget) -> Result<Arc<DesktopSession>> {
        let id = t.desktop().ok_or_else(|| {
            GhostError::Intent(format!(
                "window '{}' is on the user's desktop, not a hidden one",
                t.title
            ))
        })?;
        self.desktops.lock().await.get(id).cloned().ok_or_else(|| {
            GhostError::Intent(format!(
                "hidden desktop '{id}' is no longer registered (closed?); ghost_window op=list \
                 shows what is still open"
            ))
        })
    }

    /// The desktop launches go to, created on first use.
    pub async fn auto_desktop(&self) -> Result<Arc<DesktopSession>> {
        let mut desktops = self.desktops.lock().await;
        if let Some(d) = desktops.get(AUTO_DESKTOP) {
            return Ok(d.clone());
        }
        let d = Arc::new(core(DesktopSession::create(AUTO_DESKTOP))?);
        desktops.insert(AUTO_DESKTOP.to_string(), d.clone());
        Ok(d)
    }

    /// Start `command` on the auto hidden desktop, wait for its window, anchor it.
    ///
    /// The new window is found by diffing the desktop's window list rather than
    /// by pid: Store apps (Win11 Notepad) hand off to a broker, so the window's
    /// pid is not the launched pid.
    pub async fn launch_hidden(&self, command: &str) -> Result<Value> {
        let d = self.auto_desktop().await?;
        let before: std::collections::HashSet<isize> =
            core(d.windows())?.into_iter().map(|w| w.hwnd).collect();
        // Single-instance apps (Win11 Notepad, Explorer, a browser on its default
        // profile) hand the launch to their already-running process, whose
        // windows live on the USER's desktop. That cannot be prevented from
        // here, but it must never be reported as hidden - so remember what the
        // user's desktop looked like too.
        let user_before: std::collections::HashSet<isize> = Self::user_candidates()
            .map(|c| c.into_iter().map(|c| c.hwnd).collect())
            .unwrap_or_default();
        let resolved = crate::engine::process::manager::resolve_command_line(command);
        let pid = core(d.launch(&resolved))?;
        let deadline = Instant::now() + LAUNCH_WINDOW_WAIT;
        loop {
            if let Ok(windows) = d.windows() {
                if let Some(w) = windows.into_iter().find(|w| !before.contains(&w.hwnd)) {
                    let target = WindowTarget {
                        hwnd: w.hwnd,
                        title: w.title.clone(),
                        pid: w.pid,
                        minimized: false,
                        surface: Surface::Hidden { desktop: AUTO_DESKTOP.into() },
                        source: TargetSource::Explicit,
                    };
                    self.set_anchor(&target);
                    let mut anchored = target.clone();
                    anchored.source = TargetSource::Anchor;
                    return Ok(json!({
                        "pid": pid,
                        "surface": "hidden",
                        "desktop": AUTO_DESKTOP,
                        "window": { "hwnd": w.hwnd, "title": w.title, "pid": w.pid },
                        "target": anchored.to_json(),
                        "note": "started on a hidden desktop (never on your screen) and anchored: \
                                 ghost_see / ghost_act / ghost_key now target it by default. Drive \
                                 it by title like any other window; ghost_desktop_close id=auto \
                                 ends it.",
                    }));
                }
            }
            if Instant::now() >= deadline {
                // Did the launch surface on the user's desktop instead?
                let surfaced = Self::user_candidates()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|c| !user_before.contains(&c.hwnd) && !c.minimized);
                if let Some(c) = surfaced {
                    let target = WindowTarget {
                        hwnd: c.hwnd,
                        title: c.title.clone(),
                        pid: c.pid,
                        minimized: false,
                        surface: Surface::User,
                        source: TargetSource::Explicit,
                    };
                    self.set_anchor(&target);
                    return Ok(json!({
                        "pid": pid,
                        "surface": "user",
                        "desktop": Value::Null,
                        "window": { "hwnd": c.hwnd, "title": c.title, "pid": c.pid },
                        "target": target.to_json(),
                        "warning": "the app reused an already-running instance, so its new window \
                                    opened on YOUR desktop, not the hidden one (single-instance apps \
                                    such as Win11 Notepad, Explorer, or a browser on its default \
                                    profile do this). It is anchored and will be driven in the \
                                    background from here, but it is visible. Close the app's other \
                                    windows first if it must stay invisible.",
                    }));
                }
                return Ok(json!({
                    "pid": pid,
                    "surface": "hidden",
                    "desktop": AUTO_DESKTOP,
                    "window": Value::Null,
                    "note": "started on a hidden desktop but showed no window within 5s. \
                             ghost_window op=list shows it (surface=hidden) once it appears; \
                             pass its title as window= to drive it.",
                }));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// `ghost_see` (elements) on a hidden-desktop window.
    pub async fn hidden_describe(&self, t: &WindowTarget) -> Result<Vec<ElementDescriptor>> {
        let d = self.desktop_for(t).await?;
        let hwnd = t.hwnd;
        // Rooted by handle: a title lookup among the desktop's children returned
        // an empty walk for a Chromium window that owns same-titled helpers.
        core(core(d.with_uia(move |tree| tree.describe_hwnd(hwnd)))?)
    }

    /// `ghost_see mode=text` on a hidden-desktop window.
    pub async fn hidden_read_text(&self, t: &WindowTarget, max_chars: usize) -> Result<(String, bool)> {
        let d = self.desktop_for(t).await?;
        let hwnd = t.hwnd;
        core(core(d.with_uia(move |tree| tree.collect_text_in_hwnd(hwnd, max_chars)))?)
    }

    /// `ghost_find` on a hidden-desktop window: same shape as the user-desktop
    /// background find.
    pub async fn hidden_find(&self, t: &WindowTarget, by: By, index: Option<usize>) -> Result<Value> {
        let d = self.desktop_for(t).await?;
        let hwnd = t.hwnd;
        let found: Result<(Located, usize)> = core(d.with_uia(move |tree| {
            find_in(tree, hwnd, &by, index).map(|(el, n)| (Located::of(&el), n))
        }))?;
        let (info, matches) = found?;
        let (cx, cy) = info.center();
        Ok(json!({
            "ok": true,
            "name": info.name,
            "center": { "x": cx, "y": cy },
            "rect": info.rect_json(),
            "has_rect": info.rect.is_some(),
            "hwnd": t.hwnd,
            "window": t.title,
            "source": "uia",
            "confidence": 1.0,
            "dispatch_mode": "hidden",
            "index": index,
            "matches": matches,
            "escalated": false,
        }))
    }

    /// `ghost_act` on a hidden-desktop window. Same dispatch ladder as the
    /// user-desktop background act (posted messages on real Win32 controls, UIA
    /// patterns on windowless ones), with one difference: activation is harmless
    /// here, so a windowless control with no InvokePattern gets a posted click at
    /// its centre instead of a refusal.
    pub async fn hidden_act(
        &self,
        t: &WindowTarget,
        by: By,
        action: &str,
        text: Option<&str>,
    ) -> Result<Value> {
        let d = self.desktop_for(t).await?;
        let hwnd = t.hwnd;
        let title = t.title.clone();
        let action = action.to_string();
        let text = text.map(str::to_string);
        let out: Result<Value> = core(d.with_uia(move |tree| -> Result<Value> {
            let t_start = Instant::now();
            let (el, _) = if action == "type" {
                find_for_typing(tree, hwnd, &by)?
            } else {
                find_in(tree, hwnd, &by, None)?
            };
            let info = Located::of(&el);
            let t_find = t_start.elapsed().as_millis() as u64;
            let t_action_start = Instant::now();
            if !info.enabled {
                return Err(GhostError::ElementNotInteractable {
                    element: info.name,
                    reason: "element is disabled".into(),
                });
            }
            let (cx, cy) = info.center();
            // Pixel verification is a PrintWindow before/after diff. On a
            // non-displayed desktop DWM does not compose the window, so
            // GPU-composited apps (Chromium, Electron) fall back to a software
            // WM_PRINT render that costs a second or more per capture. Those
            // windows are skipped - CDP and read-back are the right checks there.
            let composited = ghost_core::input::window_class(hwnd).starts_with("chrome_");
            let snapshot = |needed: bool| {
                if needed && !composited {
                    capture_window_printwindow(hwnd).ok()
                } else {
                    None
                }
            };
            let skipped_note = "click dispatched; pixel verification is skipped for Chromium/Electron windows on a hidden desktop (a software render would cost seconds) - confirm through ghost_tab_eval or ghost_see";
            if composited {
                activate_here(hwnd);
            }
            let mut t_dispatch: u64 = 0;
            let (verified, note): (Option<bool>, Option<&str>) = match action.as_str() {
                "click" => {
                    let before = snapshot(true);
                    let t0 = Instant::now();
                    if composited {
                        // Measured: UIA Invoke/SetValue against Chromium on a
                        // hidden desktop returns only after a ~2 s internal wait
                        // (activation does not help), while a posted click at
                        // the element's centre lands in milliseconds.
                        core(BackgroundClicker::click_screen(hwnd, cx, cy))?;
                    } else if info.ctrl != 0 && info.control_type == UIA_BUTTON {
                        core(BackgroundClicker::button_click(info.ctrl))?;
                    } else if info.ctrl != 0 {
                        core(BackgroundClicker::click_screen(info.ctrl, cx, cy))?;
                    } else if patterns::invoke_ex(&el, false).is_err() {
                        core(BackgroundClicker::click_screen(hwnd, cx, cy))?;
                    }
                    t_dispatch = t0.elapsed().as_millis() as u64;
                    std::thread::sleep(Duration::from_millis(80));
                    if composited {
                        (None, Some(skipped_note))
                    } else {
                        let changed = pixels_changed(hwnd, before);
                        let note = match changed {
                            None => Some("click dispatched; the window surface could not be compared - read state to confirm"),
                            Some(false) => Some("click dispatched; the window's pixels did not change - a click's effect is often elsewhere, read state to confirm"),
                            Some(true) => None,
                        };
                        (changed, note)
                    }
                }
                "type" => {
                    let value = text.ok_or_else(|| {
                        GhostError::Vision("ghost_act: action=type requires text_input".into())
                    })?;
                    let t0 = Instant::now();
                    if composited {
                        // Same ~2 s wait for SetValue. The window is active on
                        // its own (undisplayed) desktop, so a posted click gives
                        // the field DOM focus and posted characters type into it
                        // exactly as real keystrokes would.
                        core(BackgroundClicker::click_screen(hwnd, cx, cy))?;
                        std::thread::sleep(Duration::from_millis(40));
                        let target = BackgroundClicker::focused_control(hwnd);
                        for ch in value.chars() {
                            core(BackgroundClicker::send_char(target, ch))?;
                        }
                    } else if info.ctrl != 0 {
                        core(BackgroundClicker::set_text(info.ctrl, &value))?;
                    } else {
                        core(patterns::set_value_ex(&el, &value, false))?;
                    }
                    t_dispatch = t0.elapsed().as_millis() as u64;
                    // Providers update their accessible value asynchronously
                    // (Chromium's AX tree in particular), so poll the read-back
                    // briefly instead of judging on the first stale read.
                    let fold = |s: &str| s.replace("\r\n", "\n");
                    let deadline = Instant::now() + Duration::from_millis(600);
                    let mut ok = false;
                    loop {
                        let got = element_value(&el);
                        if got.trim() == value.trim() || fold(&got).contains(&fold(&value)) {
                            ok = true;
                            break;
                        }
                        if Instant::now() >= deadline {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    (Some(ok), if ok { None } else { Some("the control did not read back the text within 600ms - it may not accept a set value; try ghost_key per character, or confirm by other means") })
                }
                "double_click" => {
                    let before = snapshot(true);
                    let target = if info.ctrl != 0 { info.ctrl } else { hwnd };
                    core(BackgroundClicker::double_click_screen(target, cx, cy))?;
                    std::thread::sleep(Duration::from_millis(80));
                    if composited {
                        (None, Some(skipped_note))
                    } else {
                        (pixels_changed(hwnd, before), None)
                    }
                }
                "right_click" => {
                    let target = if info.ctrl != 0 { info.ctrl } else { hwnd };
                    core(BackgroundClicker::right_click_screen(target, cx, cy))?;
                    (None, Some("right-click posted; a context menu is a separate popup - ghost_see to confirm"))
                }
                "hover" => {
                    let target = if info.ctrl != 0 { info.ctrl } else { hwnd };
                    core(BackgroundClicker::hover_screen(target, cx, cy))?;
                    (None, Some("WM_MOUSEMOVE posted; no pointer exists on a hidden desktop, so hover visuals may not render"))
                }
                other => {
                    return Err(GhostError::Vision(format!(
                        "ghost_act: unknown action '{other}' (use click|type|double_click|right_click|hover)"
                    )))
                }
            };
            let mut out = json!({
                "ok": true,
                "mode": "hidden",
                "action": action,
                "name": info.name,
                "rect": info.rect_json(),
                "window": title,
                "verified": verified,
                "focus_preserved": true,
                "cursor_preserved": true,
                // Where the time went, so a slow call is diagnosable from the
                // response alone (find = UIA walk, action = dispatch + verify).
                "timing_ms": { "find": t_find, "dispatch": t_dispatch, "action": t_action_start.elapsed().as_millis() as u64 },
            });
            if let Some(n) = note {
                out["note"] = Value::String(n.into());
            }
            Ok(out)
        }))?;
        out
    }

    /// `ghost_key` on a hidden-desktop window: a character goes as WM_CHAR to
    /// the window's text control, a named key as WM_KEYDOWN/UP, and the editing
    /// shortcuts Ctrl+C/X/V/A/Z as semantic edit messages.
    pub async fn hidden_key(&self, t: &WindowTarget, keys: &str) -> Result<Value> {
        let d = self.desktop_for(t).await?;
        let hwnd = t.hwnd;
        let spec = keys.trim().to_string();
        let lower = spec.to_lowercase();
        let how: &'static str = if spec.chars().count() == 1 {
            let ch = spec.chars().next().unwrap();
            core(d.exec(move || {
                let target = BackgroundClicker::text_target(hwnd)
                    .unwrap_or_else(|| BackgroundClicker::focused_control(hwnd));
                BackgroundClicker::send_char(target, ch)
            }))??;
            "WM_CHAR posted to the window's text control"
        } else if let Some(rest) = lower.strip_prefix("ctrl+") {
            core(d.shortcut(hwnd, &format!("ctrl+{rest}"))).map_err(|_| {
                GhostError::Config(format!(
                    "ghost_key: combo '{spec}' has no background path (only Ctrl+C/X/V/A/Z are \
                     dispatchable as edit messages); use single keys or ghost_act action=type"
                ))
            })?;
            "semantic edit message (WM_COPY/CUT/PASTE/UNDO/EM_SETSEL)"
        } else if lower.contains('+') {
            return Err(GhostError::Config(format!(
                "ghost_key: modifier combo '{spec}' cannot be posted (apps read real modifier \
                 state); use a single key, Ctrl+C/X/V/A/Z, or ghost_act action=type"
            )));
        } else {
            core(d.press(hwnd, &spec))?;
            "WM_KEYDOWN/WM_KEYUP posted to the window's text control"
        };
        Ok(json!({
            "ok": true,
            "mode": "hidden",
            "key": spec,
            "window": t.title,
            "focus_preserved": true,
            "cursor_preserved": true,
            "verified": Value::Null,
            "note": format!("{how}; read state (ghost_see mode=text) to confirm the effect"),
        }))
    }

    /// `ghost_click_at` (screen coordinates on the hidden desktop, as returned by
    /// `ghost_find`/`ghost_see` there).
    pub async fn hidden_click_at(&self, t: &WindowTarget, x: i32, y: i32) -> Result<Value> {
        let d = self.desktop_for(t).await?;
        let hwnd = t.hwnd;
        let changed: Option<bool> = core(d.exec(move || -> Result<Option<bool>> {
            let composited = ghost_core::input::window_class(hwnd).starts_with("chrome_");
            let before = if composited { None } else { capture_window_printwindow(hwnd).ok() };
            core(BackgroundClicker::click_screen(hwnd, x, y))?;
            std::thread::sleep(Duration::from_millis(80));
            Ok(if composited { None } else { pixels_changed(hwnd, before) })
        }))??;
        Ok(json!({
            "ok": true,
            "mode": "hidden",
            "action": "click_at",
            "x": x,
            "y": y,
            "window": t.title,
            "verified": changed,
            "focus_preserved": true,
            "cursor_preserved": true,
        }))
    }

    /// `ghost_scroll` on a hidden-desktop window (posted wheel messages).
    pub async fn hidden_scroll(&self, t: &WindowTarget, direction: &str, amount: i32) -> Result<Value> {
        let d = self.desktop_for(t).await?;
        let (notches, horizontal) = wheel(direction, amount)?;
        core(d.scroll(t.hwnd, 0, 0, notches, horizontal))?;
        Ok(json!({ "ok": true, "mode": "hidden", "window": t.title, "direction": direction, "amount": amount }))
    }

    /// `ghost_scroll` on a user-desktop window under the background policy: the
    /// wheel message is posted to the window, the real pointer never moves.
    pub async fn scroll_background(&self, t: &WindowTarget, direction: &str, amount: i32) -> Result<Value> {
        let (notches, horizontal) = wheel(direction, amount)?;
        core(ghost_core::desktop::scroll_window(t.hwnd, notches, horizontal))?;
        Ok(json!({ "ok": true, "mode": "background", "window": t.title, "direction": direction, "amount": amount }))
    }

    /// PNG of a hidden-desktop window.
    pub async fn hidden_capture(&self, t: &WindowTarget) -> Result<Vec<u8>> {
        let d = self.desktop_for(t).await?;
        core(d.capture(t.hwnd, false))
    }

    /// The current value (ValuePattern / text) of an element on a hidden window.
    pub async fn hidden_value(&self, t: &WindowTarget, by: By) -> Result<String> {
        let d = self.desktop_for(t).await?;
        let hwnd = t.hwnd;
        let v: Result<String> = core(d.with_uia(move |tree| {
            find_in(tree, hwnd, &by, None).map(|(el, _)| element_value(&el))
        }))?;
        v
    }

    /// Poll for an element to appear (or disappear) on a hidden window.
    pub async fn hidden_wait_for_element(
        &self,
        t: &WindowTarget,
        by: By,
        appears: bool,
        timeout_ms: u64,
    ) -> Result<()> {
        let start = Instant::now();
        loop {
            let present = self.hidden_find(t, by.clone(), None).await.is_ok();
            if present == appears {
                return Ok(());
            }
            if start.elapsed() >= Duration::from_millis(timeout_ms) {
                return Err(GhostError::Timeout {
                    action: format!("wait_for_element:{by:?}:appears={appears} (hidden desktop)"),
                    ms: timeout_ms,
                });
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_maps_directions_to_signed_notches() {
        assert_eq!(wheel("up", 3).unwrap(), (3, false));
        assert_eq!(wheel("down", 3).unwrap(), (-3, false));
        assert_eq!(wheel("left", 2).unwrap(), (-2, true));
        assert_eq!(wheel("Right", 2).unwrap(), (2, true));
        assert_eq!(wheel("down", 0).unwrap(), (-1, false), "amount floors at one notch");
        assert!(wheel("sideways", 1).is_err());
    }

    #[test]
    fn located_centre_and_rect_json() {
        let l = Located { name: "x".into(), rect: Some((10, 20, 30, 40)), ctrl: 0, enabled: true, control_type: 0 };
        assert_eq!(l.center(), (20, 30));
        assert_eq!(l.rect_json()["right"], 30);
        let none = Located { rect: None, ..l };
        assert_eq!(none.center(), (0, 0));
        assert!(none.rect_json().is_null());
    }
}
