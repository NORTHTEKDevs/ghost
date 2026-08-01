//! Accessibility tree, mirroring `ghost_core::uia::tree`.
//!
//! Each public method is exactly one job on the accessibility thread. The walk
//! itself happens entirely inside that job: bouncing per-node across the thread
//! boundary would turn a thousand-node window into a thousand handoffs.
//!
//! Every walk is node-budgeted, matching the Windows engine's budgets. A
//! Chromium tab can expose tens of thousands of accessible nodes, and an
//! unbounded walk there would hang the serial MCP server.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use atspi::State;

use super::element::{A11yElement, ElementDescriptor, UiaElement};
use super::handles::{self, ObjAddr};
use super::ops::{self, effective_role, Snapshot};
use super::roles::is_interactive_role;
use crate::bridge::{A11yBridge, Ctx};
use crate::error::{CoreError, Result};

/// Node budgets, identical to the Windows engine's.
const SEARCH_NODE_BUDGET: usize = 3000;
const DESCRIBE_NODE_BUDGET: usize = 6000;
const TEXT_NODE_BUDGET: usize = 8000;

/// Walks touch many objects over D-Bus, so they get a longer deadline than a
/// single call. Still bounded: a wedged app must not hang the server.
const WALK_TIMEOUT: Duration = Duration::from_millis(20_000);

/// Re-exported for shared code that calls `tree::role_alias_matches`.
pub use super::roles::role_alias_matches;

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub name: String,
    pub pid: u32,
    pub focused: bool,
    /// Interned handle for this window. `0` means none.
    pub hwnd: isize,
    /// "normal" | "minimized", matching the Windows engine's vocabulary.
    pub state: &'static str,
}

pub struct A11yTree {
    bridge: Arc<A11yBridge>,
}

impl A11yTree {
    pub fn new() -> Result<Self> {
        Ok(Self { bridge: super::global_bridge()? })
    }

    pub fn bridge(&self) -> Arc<A11yBridge> {
        Arc::clone(&self.bridge)
    }

    // ---------------------------------------------------------------- windows

    pub fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        self.bridge
            .run(WALK_TIMEOUT, move |ctx| Box::pin(async move { list_windows_async(ctx).await }))
    }

    /// Handle of the active window, or 0.
    pub fn foreground_window(&self) -> isize {
        self.list_windows()
            .ok()
            .and_then(|ws| ws.into_iter().find(|w| w.focused).map(|w| w.hwnd))
            .unwrap_or(0)
    }

    pub fn window_rect(&self, window: isize) -> Option<(i32, i32, i32, i32)> {
        let addr = handles::resolve(window)?;
        self.bridge
            .run_default(move |ctx| Box::pin(async move { ops::snapshot(ctx, &addr).await }))
            .ok()
            .and_then(|s| s.rect)
            .map(|r| (r.left, r.top, r.right, r.bottom))
    }

    // ------------------------------------------------------------------ finds

    pub fn find_by_name(&self, name: &str) -> Result<Option<UiaElement>> {
        self.find(None, Some(name), None)
    }

    pub fn find_by_role(&self, role: &str) -> Result<Option<UiaElement>> {
        self.find(None, None, Some(role))
    }

    /// Fast path: search the active window only.
    pub fn find_by_name_fast(&self, name: &str) -> Result<Option<UiaElement>> {
        let fg = self.foreground_window();
        self.find(if fg == 0 { None } else { Some(fg) }, Some(name), None)
    }

    pub fn find_by_role_fast(&self, role: &str) -> Result<Option<UiaElement>> {
        let fg = self.foreground_window();
        self.find(if fg == 0 { None } else { Some(fg) }, None, Some(role))
    }

    pub fn find_by_name_in_hwnd(&self, window: isize, name: &str) -> Result<Option<UiaElement>> {
        self.find(Some(window), Some(name), None)
    }

    pub fn find_by_role_in_hwnd(&self, window: isize, role: &str) -> Result<Option<UiaElement>> {
        self.find(Some(window), None, Some(role))
    }

    pub fn find_all_in_hwnd(
        &self,
        window: isize,
        name: Option<&str>,
        role: Option<&str>,
        cap: usize,
    ) -> Result<Vec<UiaElement>> {
        let roots = self.roots_for(Some(window))?;
        let name = name.map(str::to_string);
        let role = role.map(str::to_string);
        let bridge = Arc::clone(&self.bridge);

        let hits: Vec<(ObjAddr, Snapshot, isize)> = self.bridge.run(WALK_TIMEOUT, move |ctx| {
            Box::pin(async move {
                let mut out = Vec::new();
                for (win_handle, root) in roots {
                    let mut found =
                        collect_matching(ctx, &root, name.as_deref(), role.as_deref(), cap).await?;
                    out.extend(found.drain(..).map(|(a, s)| (a, s, win_handle)));
                    if out.len() >= cap {
                        break;
                    }
                }
                out.truncate(cap);
                Ok(out)
            })
        })?;

        Ok(hits
            .into_iter()
            .map(|(addr, snap, win)| A11yElement::new(Arc::clone(&bridge), addr, snap, win))
            .collect())
    }

    fn find(
        &self,
        window: Option<isize>,
        name: Option<&str>,
        role: Option<&str>,
    ) -> Result<Option<UiaElement>> {
        let roots = self.roots_for(window)?;
        let name = name.map(str::to_string);
        let role = role.map(str::to_string);
        let bridge = Arc::clone(&self.bridge);

        let best: Option<(ObjAddr, Snapshot, isize)> = self.bridge.run(WALK_TIMEOUT, move |ctx| {
            Box::pin(async move {
                let mut best: Option<(i32, ObjAddr, Snapshot, isize)> = None;
                for (win_handle, root) in roots {
                    let hits = collect_matching(
                        ctx,
                        &root,
                        name.as_deref(),
                        role.as_deref(),
                        SEARCH_NODE_BUDGET,
                    )
                    .await?;
                    for (addr, snap) in hits {
                        let score = score_match(&snap, name.as_deref(), role.as_deref());
                        if best.as_ref().map(|(b, ..)| score > *b).unwrap_or(true) {
                            best = Some((score, addr, snap, win_handle));
                        }
                    }
                    // An exact, enabled, on-screen hit cannot be beaten; stop.
                    if best.as_ref().map(|(s, ..)| *s >= EXACT_SCORE).unwrap_or(false) {
                        break;
                    }
                }
                Ok(best.map(|(_, a, s, w)| (a, s, w)))
            })
        })?;

        Ok(best.map(|(addr, snap, win)| A11yElement::new(bridge, addr, snap, win)))
    }

    // --------------------------------------------------------------- describe

    pub fn describe_screen(&self, window_name: Option<&str>) -> Result<Vec<ElementDescriptor>> {
        let roots = self.roots_named(window_name)?;
        self.bridge.run(WALK_TIMEOUT, move |ctx| {
            Box::pin(async move {
                let mut out = Vec::new();
                for (_, root) in roots {
                    out.extend(collect_descriptors(ctx, &root, DESCRIBE_NODE_BUDGET).await?);
                }
                Ok(out)
            })
        })
    }

    pub fn describe_screen_fast(&self) -> Result<Vec<ElementDescriptor>> {
        let fg = self.foreground_window();
        let roots = self.roots_for(if fg == 0 { None } else { Some(fg) })?;
        self.bridge.run(WALK_TIMEOUT, move |ctx| {
            Box::pin(async move {
                let mut out = Vec::new();
                for (_, root) in roots {
                    out.extend(collect_descriptors(ctx, &root, DESCRIBE_NODE_BUDGET).await?);
                }
                Ok(out)
            })
        })
    }

    pub fn collect_text(&self, window_name: Option<&str>, max_chars: usize) -> Result<(String, bool)> {
        let roots = self.roots_named(window_name)?;
        self.bridge.run(WALK_TIMEOUT, move |ctx| {
            Box::pin(async move {
                let mut buf = String::new();
                for (_, root) in roots {
                    collect_text_async(ctx, &root, max_chars, &mut buf).await?;
                    if buf.len() >= max_chars {
                        break;
                    }
                }
                // Second element mirrors the Windows engine: true when the
                // walk hit its cap and the text is therefore incomplete.
                let truncated = buf.len() >= max_chars;
                buf.truncate(max_chars);
                Ok((buf, truncated))
            })
        })
    }

    pub fn element_from_point(&self, x: i32, y: i32) -> Result<Option<UiaElement>> {
        let roots = self.roots_for(None)?;
        let bridge = Arc::clone(&self.bridge);

        let hit: Option<(ObjAddr, Snapshot, isize)> = self.bridge.run(WALK_TIMEOUT, move |ctx| {
            Box::pin(async move {
                for (win, root) in roots {
                    if let Ok(Some(addr)) = ops::accessible_at_point(ctx, &root, x, y).await {
                        if let Ok(snap) = ops::snapshot(ctx, &addr).await {
                            return Ok(Some((addr, snap, win)));
                        }
                    }
                }
                Ok(None)
            })
        })?;

        Ok(hit.map(|(addr, snap, win)| A11yElement::new(bridge, addr, snap, win)))
    }

    // ---------------------------------------------------------------- helpers

    /// Resolve which subtrees to search: one window, or every window with the
    /// active one first so the common case short-circuits early.
    fn roots_for(&self, window: Option<isize>) -> Result<Vec<(isize, ObjAddr)>> {
        if let Some(w) = window {
            let addr = handles::resolve(w).ok_or(CoreError::WindowGone)?;
            return Ok(vec![(w, addr)]);
        }
        let mut wins = self.list_windows()?;
        wins.sort_by_key(|w| !w.focused); // active window first
        Ok(wins
            .into_iter()
            .filter(|w| w.hwnd != 0)
            .filter_map(|w| handles::resolve(w.hwnd).map(|a| (w.hwnd, a)))
            .collect())
    }

    fn roots_named(&self, window_name: Option<&str>) -> Result<Vec<(isize, ObjAddr)>> {
        let Some(want) = window_name else {
            let fg = self.foreground_window();
            return self.roots_for(if fg == 0 { None } else { Some(fg) });
        };
        let want_lc = want.to_ascii_lowercase();
        let wins = self.list_windows()?;
        let matched: Vec<(isize, ObjAddr)> = wins
            .into_iter()
            .filter(|w| w.name.to_ascii_lowercase().contains(&want_lc))
            .filter_map(|w| handles::resolve(w.hwnd).map(|a| (w.hwnd, a)))
            .collect();
        if matched.is_empty() {
            return Err(CoreError::NotFound(format!("no window matching {want:?}")));
        }
        Ok(matched)
    }
}

/// Perfect match: exact name, enabled, on screen.
const EXACT_SCORE: i32 = 100;

fn score_match(snap: &Snapshot, name: Option<&str>, _role: Option<&str>) -> i32 {
    let mut score = 0;
    if let Some(want) = name {
        let got = snap.name.to_ascii_lowercase();
        let want = want.to_ascii_lowercase();
        if got == want {
            score += 60;
        } else if got.starts_with(&want) {
            score += 35;
        } else if got.contains(&want) {
            score += 20;
        }
    } else {
        score += 20;
    }
    if snap.enabled {
        score += 25;
    }
    if snap.showing && snap.rect.map(|r| !r.is_empty()).unwrap_or(false) {
        score += 15;
    }
    score
}

fn name_matches(snap: &Snapshot, want: &str) -> bool {
    let got = snap.name.to_ascii_lowercase();
    let want = want.to_ascii_lowercase();
    got == want || got.contains(&want)
}

fn role_matches(snap: &Snapshot, want: &str) -> bool {
    let actual = effective_role(snap);
    if actual == want || role_alias_matches(want, actual) {
        return true;
    }
    // Last resort for `edit`: trust the interface over any role name at all.
    want == "edit" && snap.has_editable_text
}

async fn list_windows_async(ctx: &Ctx) -> Result<Vec<WindowInfo>> {
    let root = ctx
        .conn
        .root_accessible_on_registry()
        .await
        .map_err(|e| CoreError::platform(format!("registry root: {e}")))?;

    let apps = root
        .get_children()
        .await
        .map_err(|e| CoreError::platform(format!("registry children: {e}")))?;

    let mut out = Vec::new();
    for app_ref in apps.iter() {
        let Some(bus) = app_ref.name_as_str() else { continue };
        let app_addr = ObjAddr::new(bus, app_ref.path_as_str());

        let pid = ops::pid_for_bus(ctx, bus).await.unwrap_or(0);
        let app_name = match ops::accessible(ctx, &app_addr).await {
            Ok(a) => a.name().await.unwrap_or_default(),
            Err(_) => String::new(),
        };

        // An application's children are its top-level windows.
        let windows = ops::children(ctx, &app_addr).await.unwrap_or_default();
        for win in windows {
            let Ok(acc) = ops::accessible(ctx, &win).await else { continue };
            let states = match acc.get_state().await {
                Ok(s) => s,
                Err(_) => continue,
            };
            if states.contains(State::Defunct) {
                continue;
            }
            let title = acc.name().await.unwrap_or_default();
            let name = if title.is_empty() { app_name.clone() } else { title };

            out.push(WindowInfo {
                name,
                pid,
                focused: states.contains(State::Active),
                hwnd: handles::intern(&win),
                // Minimised windows are kept, matching the Windows engine, so an
                // agent does not lose a window it just interacted with.
                state: if states.contains(State::Iconified) { "minimized" } else { "normal" },
            });
        }
    }
    Ok(out)
}

/// Breadth-first match collection, node-budgeted.
async fn collect_matching(
    ctx: &Ctx,
    root: &ObjAddr,
    name: Option<&str>,
    role: Option<&str>,
    cap: usize,
) -> Result<Vec<(ObjAddr, Snapshot)>> {
    let mut queue: VecDeque<ObjAddr> = VecDeque::new();
    queue.push_back(root.clone());

    let mut visited = 0usize;
    let mut out = Vec::new();

    while let Some(addr) = queue.pop_front() {
        if visited >= SEARCH_NODE_BUDGET || out.len() >= cap {
            break;
        }
        visited += 1;

        if let Ok(snap) = ops::snapshot(ctx, &addr).await {
            let name_ok = name.map(|n| name_matches(&snap, n)).unwrap_or(true);
            let role_ok = role.map(|r| role_matches(&snap, r)).unwrap_or(true);
            if name_ok && role_ok {
                out.push((addr.clone(), snap));
            }
        }

        if let Ok(kids) = ops::children(ctx, &addr).await {
            for k in kids {
                queue.push_back(k);
            }
        }
    }
    Ok(out)
}

async fn collect_descriptors(
    ctx: &Ctx,
    root: &ObjAddr,
    budget: usize,
) -> Result<Vec<ElementDescriptor>> {
    let mut queue: VecDeque<ObjAddr> = VecDeque::new();
    queue.push_back(root.clone());

    let mut visited = 0usize;
    let mut out = Vec::new();

    while let Some(addr) = queue.pop_front() {
        if visited >= budget {
            break;
        }
        visited += 1;

        if let Ok(snap) = ops::snapshot(ctx, &addr).await {
            let role = effective_role(&snap);
            let rect = snap.rect.unwrap_or_default();
            // Only report things an agent could plausibly act on, and only when
            // they have a real on-screen rectangle. Anything editable counts as
            // actionable regardless of the role name its toolkit chose.
            let actionable = is_interactive_role(role) || snap.has_editable_text;
            if actionable && snap.showing && !rect.is_empty() {
                out.push(ElementDescriptor {
                    name: snap.name.clone(),
                    role: role.to_string(),
                    left: rect.left,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                    enabled: snap.enabled,
                });
            }
        }

        if let Ok(kids) = ops::children(ctx, &addr).await {
            for k in kids {
                queue.push_back(k);
            }
        }
    }
    Ok(out)
}

async fn collect_text_async(
    ctx: &Ctx,
    root: &ObjAddr,
    max_chars: usize,
    buf: &mut String,
) -> Result<()> {
    let mut queue: VecDeque<ObjAddr> = VecDeque::new();
    queue.push_back(root.clone());

    let mut visited = 0usize;
    while let Some(addr) = queue.pop_front() {
        if visited >= TEXT_NODE_BUDGET || buf.len() >= max_chars {
            break;
        }
        visited += 1;

        if let Ok(snap) = ops::snapshot(ctx, &addr).await {
            let role = effective_role(&snap);
            let piece = if matches!(role, "edit" | "document") {
                ops::read_text(ctx, &addr).await.unwrap_or_default()
            } else if !snap.name.is_empty() {
                snap.name.clone()
            } else {
                String::new()
            };
            if !piece.trim().is_empty() {
                buf.push_str(piece.trim());
                buf.push('\n');
            }
        }

        if let Ok(kids) = ops::children(ctx, &addr).await {
            for k in kids {
                queue.push_back(k);
            }
        }
    }
    Ok(())
}

/// Mirrors `ghost_core::uia::tree::focus_window_under_point`.
pub fn focus_window_under_point(x: i32, y: i32) -> Result<bool> {
    let tree = A11yTree::new()?;
    match tree.element_from_point(x, y)? {
        Some(el) => {
            el.set_focus()?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Mirrors `ghost_core::uia::tree::WindowState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Maximize,
    Minimize,
    Restore,
    Close,
}

impl WindowState {
    // Inherent `from_str` (not the FromStr trait) mirrors ghost-core's API
    // exactly; shared code calls it by this name on both platforms.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "maximize" => Some(Self::Maximize),
            "minimize" => Some(Self::Minimize),
            "restore" => Some(Self::Restore),
            "close" => Some(Self::Close),
            _ => None,
        }
    }
}

/// Free-function form, mirroring `ghost_core::uia::tree::list_windows`.
pub fn list_windows() -> Result<Vec<WindowInfo>> {
    A11yTree::new()?.list_windows()
}

/// Raise and focus a window by (partial, case-insensitive) name.
pub fn focus_window(name: &str) -> Result<()> {
    let tree = A11yTree::new()?;
    let want = name.to_ascii_lowercase();
    let win = tree
        .list_windows()?
        .into_iter()
        .find(|w| w.name.to_ascii_lowercase().contains(&want))
        .ok_or_else(|| CoreError::ProcessNotFound { name: name.to_string() })?;

    let addr = handles::resolve(win.hwnd).ok_or(CoreError::WindowGone)?;
    let bridge = tree.bridge();
    let ok = bridge
        .run_default(move |ctx| Box::pin(async move { ops::grab_focus(ctx, &addr).await }))?;
    if ok {
        Ok(())
    } else {
        Err(CoreError::FocusFailed { window: name.to_string() })
    }
}

/// Window state changes.
///
/// AT-SPI exposes no minimise/maximise/close action, and neither X11 nor Wayland
/// offers a portable way to do it without a window-manager protocol Ghost does
/// not speak. Rather than silently no-op -- which would make an agent believe a
/// window was minimised when it was not -- this reports the limitation. `Close`
/// is attempted through the window's own accessible action, which many toolkits
/// do expose.
pub fn set_window_state(name: &str, state: WindowState) -> Result<()> {
    if !matches!(state, WindowState::Close) {
        return Err(CoreError::Unsupported(format!(
            "window {state:?} is not available on Linux: AT-SPI exposes no such action and Ghost \
             does not speak a window-manager protocol. Use your desktop's own shortcut, or drive \
             the window's controls directly"
        )));
    }

    let tree = A11yTree::new()?;
    let want = name.to_ascii_lowercase();
    let win = tree
        .list_windows()?
        .into_iter()
        .find(|w| w.name.to_ascii_lowercase().contains(&want))
        .ok_or_else(|| CoreError::ProcessNotFound { name: name.to_string() })?;
    let addr = handles::resolve(win.hwnd).ok_or(CoreError::WindowGone)?;
    tree.bridge()
        .run_default(move |ctx| Box::pin(async move { ops::do_best_action(ctx, &addr).await }))?;
    Ok(())
}

/// Mirrors `ghost_core::uia::tree::ensure_foreground`.
///
/// Polls until the window reports `Active`, since a focus request is
/// asynchronous: the compositor and toolkit decide when it takes effect.
pub fn ensure_foreground(hwnd: isize, timeout_ms: u64) -> Result<bool> {
    let addr = handles::resolve(hwnd).ok_or(CoreError::WindowGone)?;
    let bridge = super::global_bridge()?;

    {
        let a = addr.clone();
        let _ = bridge.run_default(move |ctx| Box::pin(async move { ops::grab_focus(ctx, &a).await }));
    }

    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let a = addr.clone();
        let active = bridge
            .run_default(move |ctx| Box::pin(async move { Ok(ops::is_active(ctx, &a).await) }))
            .unwrap_or(false);
        if active {
            return Ok(true);
        }
        if std::time::Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Re-exported under the Windows engine's name.
pub type UiaTree = A11yTree;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a11y::ops::Rect;

    fn snap(name: &str, role_id: u32, enabled: bool, showing: bool) -> Snapshot {
        Snapshot {
            name: name.into(),
            role_id,
            rect: Some(Rect { left: 0, top: 0, right: 10, bottom: 10 }),
            enabled,
            showing,
            focusable: true,
            has_action: true,
            has_editable_text: false,
        }
    }

    #[test]
    fn exact_name_outranks_substring() {
        let exact = score_match(&snap("Save", 50000, true, true), Some("Save"), None);
        let partial = score_match(&snap("Save As...", 50000, true, true), Some("Save"), None);
        assert!(exact > partial);
    }

    #[test]
    fn enabled_and_visible_outrank_disabled() {
        let good = score_match(&snap("Save", 50000, true, true), Some("Save"), None);
        let greyed = score_match(&snap("Save", 50000, false, true), Some("Save"), None);
        assert!(good > greyed);
    }

    #[test]
    fn a_perfect_hit_reaches_the_short_circuit_threshold() {
        // find() stops walking further windows once a match scores this high.
        // If scoring ever drops below it, searches silently get slower.
        assert!(score_match(&snap("Save", 50000, true, true), Some("Save"), None) >= EXACT_SCORE);
    }

    #[test]
    fn name_matching_is_case_insensitive() {
        assert!(name_matches(&snap("Save As", 50000, true, true), "save as"));
        assert!(name_matches(&snap("Save As", 50000, true, true), "SAVE"));
        assert!(!name_matches(&snap("Save As", 50000, true, true), "delete"));
    }

    #[test]
    fn role_matching_honours_the_edit_document_alias() {
        // The alias that was proven load-bearing on Windows.
        let doc = snap("body", 50030, true, true);
        assert!(role_matches(&doc, "edit"));
        assert!(role_matches(&doc, "document"));
        assert!(!role_matches(&doc, "button"));
    }

    #[test]
    fn an_editable_field_reported_as_text_still_matches_edit() {
        // Measured on GTK 3: `zenity --forms` exposes its entry as AT-SPI
        // `text`, not `entry`. Matching on the role name alone missed it.
        let mut s = snap("", 50020, true, true); // 50020 = text
        s.has_editable_text = true;
        assert!(role_matches(&s, "edit"));
        assert_eq!(effective_role(&s), "edit");
    }

    #[test]
    fn a_read_only_label_is_not_an_edit() {
        // The interface is the discriminator: a plain label is also `text`, but
        // it is not editable and must not be returned for role="edit".
        let s = snap("Name:", 50020, true, true);
        assert!(!role_matches(&s, "edit"));
        assert_eq!(effective_role(&s), "text");
    }

    #[test]
    fn normalisation_does_not_rewrite_other_roles() {
        let mut s = snap("Save", 50000, true, true); // button
        s.has_editable_text = true;
        assert_eq!(effective_role(&s), "button", "only generic `text` is normalised");
    }

    #[test]
    fn role_matching_honours_tab_and_list_aliases() {
        assert!(role_matches(&snap("General", 50019, true, true), "tab"));
        assert!(role_matches(&snap("Item", 50007, true, true), "list"));
    }
}
