//! Window targeting: which window a verb acts on, and the session anchor.
//!
//! Evidence (2026-09-01, three weeks of transcripts, 10,323 Ghost calls): the
//! largest failure class was "element not found ... in the foreground window".
//! With no `window=` the verbs acted on whatever window the human currently had
//! focused, which under the background policy is by definition NOT the
//! automation's window. Agents then reached for `ghost_window op=focus` and the
//! `foreground` policy - exactly the screen-stealing the policy exists to
//! prevent.
//!
//! The fix is a session ANCHOR: the last window the agent named (or launched) is
//! the implicit target of every window-scoped verb. The human's foreground is
//! used only when nothing was ever anchored, and then the response says so.
//!
//! A target also carries its SURFACE: the user's desktop, or one of Ghost's
//! hidden desktops. Titles resolve across both, so an app Ghost launched
//! invisibly is driven with the same `window=<title>` as any other.

use crate::engine::uia::tree::list_windows as core_list_windows;
use crate::error::{GhostError, Result};
use crate::session::GhostSession;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// Where a window lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Surface {
    /// The interactive desktop the human is looking at.
    User,
    /// One of Ghost's isolated desktops (see `ghost_core::DesktopSession`).
    Hidden { desktop: String },
}

impl Surface {
    pub fn as_str(&self) -> &'static str {
        match self {
            Surface::User => "user",
            Surface::Hidden { .. } => "hidden",
        }
    }
}

/// How the target was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetSource {
    /// The caller passed `window=`.
    Explicit,
    /// Nothing passed; the session anchor was used.
    Anchor,
    /// Nothing passed and nothing anchored; the human's foreground window.
    Foreground,
}

impl TargetSource {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetSource::Explicit => "explicit",
            TargetSource::Anchor => "anchor",
            TargetSource::Foreground => "foreground",
        }
    }
}

/// A resolved window target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowTarget {
    pub hwnd: isize,
    pub title: String,
    pub pid: u32,
    pub minimized: bool,
    pub surface: Surface,
    pub source: TargetSource,
    /// Set when the caller named a title this window USED to carry. Pages
    /// rewrite `document.title` on every navigation; the anchor followed the
    /// handle, and the response says so instead of failing a 2 s search.
    pub drifted_from: Option<String>,
}

impl WindowTarget {
    pub fn is_hidden(&self) -> bool {
        matches!(self.surface, Surface::Hidden { .. })
    }

    /// The hidden desktop id, when the window lives on one.
    pub fn desktop(&self) -> Option<&str> {
        match &self.surface {
            Surface::Hidden { desktop } => Some(desktop),
            Surface::User => None,
        }
    }

    /// The shape every window-scoped response carries under `target`.
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "hwnd": self.hwnd,
            "title": self.title,
            "pid": self.pid,
            "surface": self.surface.as_str(),
            "source": self.source.as_str(),
        });
        if let Some(d) = self.desktop() {
            v["desktop"] = Value::String(d.to_string());
        }
        if self.minimized {
            v["minimized"] = Value::Bool(true);
        }
        if let Some(asked) = &self.drifted_from {
            v["title_drift"] = json!({ "asked": asked, "now": self.title });
        }
        v
    }
}

/// One window as the resolver sees it, from either surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub hwnd: isize,
    pub title: String,
    pub pid: u32,
    pub minimized: bool,
    pub surface: Surface,
}

impl Candidate {
    fn to_target(&self, source: TargetSource) -> WindowTarget {
        WindowTarget {
            hwnd: self.hwnd,
            title: self.title.clone(),
            pid: self.pid,
            minimized: self.minimized,
            surface: self.surface.clone(),
            source,
            drifted_from: None,
        }
    }
}

/// How well a title answers a query: 0 exact, 1 prefix, 2 substring, all
/// case-insensitive. `query_lc` is already trimmed and lowercased.
fn title_rank(title: &str, query_lc: &str) -> Option<u8> {
    let t = title.to_lowercase();
    if t == query_lc {
        Some(0)
    } else if t.starts_with(query_lc) {
        Some(1)
    } else if t.contains(query_lc) {
        Some(2)
    } else {
        None
    }
}

/// Choose the window a title query means.
///
/// Rank: exact title (case-insensitive) beats prefix beats substring; within a
/// rank a window that is not minimised beats one that is; ties keep list order,
/// which is z-order for the user desktop (topmost first) and creation order on
/// hidden desktops. Pure so it is unit-testable without a desktop.
pub fn pick<'a>(candidates: &'a [Candidate], query: &str) -> Option<&'a Candidate> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    let mut best: Option<(u8, &Candidate)> = None;
    for c in candidates {
        let Some(rank) = title_rank(&c.title, &q) else {
            continue;
        };
        let rank = rank * 2 + u8::from(c.minimized);
        match best {
            Some((r, _)) if r <= rank => {}
            _ => best = Some((rank, c)),
        }
    }
    best.map(|(_, c)| c)
}

/// Does `query` name a title the anchored window carried earlier this session?
/// Same matching rule as `pick`, so a query that would have found the window
/// then still finds it now.
pub fn query_names_history(history: &[String], query: &str) -> bool {
    let q = query.trim().to_lowercase();
    !q.is_empty() && history.iter().any(|t| title_rank(t, &q).is_some())
}

/// The whole targeting decision, pure. A live title match wins and the caller
/// re-anchors on it. Failing that, a query that names a title the anchored
/// window used to have resolves to that window with `drifted = true`: pages
/// rewrite `document.title` on navigation, and an agent that re-targets by the
/// title it last read must land on the same handle instead of paying the
/// launch-race retry. Otherwise `None`, and the caller may keep polling for a
/// window that is still appearing.
pub fn resolve_static<'a>(
    candidates: &'a [Candidate],
    query: &str,
    anchor: Option<(isize, &Surface)>,
    history: &[String],
) -> Option<(&'a Candidate, bool)> {
    if let Some(c) = pick(candidates, query) {
        return Some((c, false));
    }
    let (hwnd, surface) = anchor?;
    let live = candidates
        .iter()
        .find(|c| c.hwnd == hwnd && &c.surface == surface)?;
    query_names_history(history, query).then_some((live, true))
}

/// How many past titles of the anchored window are kept for drift matching.
const TITLE_HISTORY: usize = 16;

/// A short, agent-readable listing for "no such window" errors.
pub fn describe_candidates(candidates: &[Candidate]) -> String {
    const MAX: usize = 12;
    let mut parts: Vec<String> = candidates
        .iter()
        .filter(|c| !c.title.trim().is_empty())
        .take(MAX)
        .map(|c| match &c.surface {
            Surface::User if c.minimized => format!("'{}' [minimized]", c.title),
            Surface::User => format!("'{}'", c.title),
            Surface::Hidden { desktop } => format!("'{}' [hidden desktop {desktop}]", c.title),
        })
        .collect();
    if candidates.len() > MAX {
        parts.push(format!("... {} more", candidates.len() - MAX));
    }
    parts.join(", ")
}

/// How long an explicit title is retried before failing. A just-launched app
/// may take a few hundred milliseconds to create its window; an anchored
/// lookup that races a launch should wait, not error.
const RESOLVE_DEADLINE: Duration = Duration::from_millis(2_000);
const RESOLVE_POLL: Duration = Duration::from_millis(100);

impl GhostSession {
    /// Every window on the user's desktop, in z-order.
    pub fn user_candidates() -> Result<Vec<Candidate>> {
        let list = core_list_windows().map_err(GhostError::Core)?;
        Ok(list
            .into_iter()
            .map(|w| Candidate {
                hwnd: w.hwnd,
                title: w.name,
                pid: w.pid,
                minimized: w.state == "minimized",
                surface: Surface::User,
            })
            .collect())
    }

    /// Every visible window on every hidden desktop this session owns.
    #[cfg(windows)]
    pub async fn hidden_candidates(&self) -> Vec<Candidate> {
        let mut out = Vec::new();
        let desktops = self.desktops.lock().await;
        for (id, d) in desktops.iter() {
            if let Ok(windows) = d.windows() {
                out.extend(windows.into_iter().map(|w| Candidate {
                    hwnd: w.hwnd,
                    title: w.title,
                    pid: w.pid,
                    minimized: false,
                    surface: Surface::Hidden { desktop: id.clone() },
                }));
            }
        }
        out
    }

    #[cfg(not(windows))]
    pub async fn hidden_candidates(&self) -> Vec<Candidate> {
        Vec::new()
    }

    /// User desktop first, then hidden desktops.
    pub async fn candidates(&self) -> Result<Vec<Candidate>> {
        let mut all = Self::user_candidates()?;
        all.extend(self.hidden_candidates().await);
        Ok(all)
    }

    /// The current anchor, if any (not checked for liveness).
    pub fn anchor(&self) -> Option<WindowTarget> {
        self.anchor.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Titles the anchored window has carried this session, oldest first.
    pub fn anchor_titles(&self) -> Vec<String> {
        self.anchor_titles
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Record a title the anchored window is showing now, so a later query by
    /// that title still finds the window after it retitles itself.
    fn remember_title(&self, title: &str) {
        let mut titles = self.anchor_titles.lock().unwrap_or_else(|p| p.into_inner());
        if title.trim().is_empty() || titles.iter().any(|t| t == title) {
            return;
        }
        if titles.len() >= TITLE_HISTORY {
            titles.remove(0);
        }
        titles.push(title.to_string());
    }

    /// Remember `t` as the implicit target of later unanchored verbs. Anchoring
    /// a different window starts its title history afresh.
    pub fn set_anchor(&self, t: &WindowTarget) {
        let same = self
            .anchor()
            .is_some_and(|a| a.hwnd == t.hwnd && a.surface == t.surface);
        if !same {
            self.anchor_titles
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clear();
        }
        self.remember_title(&t.title);
        let mut stored = t.clone();
        stored.source = TargetSource::Anchor;
        stored.drifted_from = None;
        *self.anchor.lock().unwrap_or_else(|p| p.into_inner()) = Some(stored);
    }

    pub fn clear_anchor(&self) {
        *self.anchor.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.anchor_titles
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    /// The anchor if its window still exists, with its title refreshed and
    /// remembered. A closed anchor is kept (it reads as `None` here) so that
    /// `resolve_target` keeps refusing unanchored verbs until the agent picks a
    /// window or clears the anchor; see [`anchor_status`].
    pub async fn live_anchor(&self) -> Option<WindowTarget> {
        let a = self.anchor()?;
        let cands = self.candidates().await.ok()?;
        match anchor_status(Some(&a), &cands) {
            AnchorStatus::Live(c) => {
                self.remember_title(&c.title);
                Some(c.to_target(TargetSource::Anchor))
            }
            AnchorStatus::Closed | AnchorStatus::Unset => None,
        }
    }

    /// The human's foreground window, as a target.
    pub fn foreground_target() -> Result<WindowTarget> {
        let hwnd = crate::tiers::foreground_hwnd();
        let cands = Self::user_candidates()?;
        Ok(match cands.iter().find(|c| c.hwnd == hwnd) {
            Some(c) => c.to_target(TargetSource::Foreground),
            None => WindowTarget {
                hwnd,
                title: String::new(),
                pid: 0,
                minimized: false,
                surface: Surface::User,
                source: TargetSource::Foreground,
                drifted_from: None,
            },
        })
    }

    /// Resolve the window a verb should act on.
    ///
    /// `Some(title)`: `"foreground"` is the human's foreground window (never
    /// anchored); any other title is matched across the user desktop and every
    /// hidden desktop and the hit becomes the session anchor. A title the
    /// anchored window carried earlier this session resolves to that window at
    /// once, flagged `title_drift` (see `resolve_static`). Only a title nobody
    /// has is retried briefly, to absorb a launch race.
    ///
    /// `None`: the live anchor if there is one, otherwise the foreground window
    /// (source `Foreground`, so the caller can say so).
    pub async fn resolve_target(&self, window: Option<&str>) -> Result<WindowTarget> {
        match window {
            Some(w) if w.trim().eq_ignore_ascii_case("foreground") => Self::foreground_target(),
            Some(w) if !w.trim().is_empty() => {
                let deadline = Instant::now() + RESOLVE_DEADLINE;
                loop {
                    let cands = self.candidates().await?;
                    let anchor = self.anchor();
                    let history = self.anchor_titles();
                    let anchored = anchor.as_ref().map(|a| (a.hwnd, &a.surface));
                    if let Some((c, drifted)) = resolve_static(&cands, w, anchored, &history) {
                        if drifted {
                            self.remember_title(&c.title);
                            let mut t = c.to_target(TargetSource::Anchor);
                            t.drifted_from = Some(w.to_string());
                            return Ok(t);
                        }
                        let t = c.to_target(TargetSource::Explicit);
                        self.set_anchor(&t);
                        return Ok(t);
                    }
                    if Instant::now() >= deadline {
                        return Err(GhostError::ProcessNotFound {
                            name: format!(
                                "window '{w}' (no open window matches; open windows: {})",
                                describe_candidates(&cands)
                            ),
                        });
                    }
                    tokio::time::sleep(RESOLVE_POLL).await;
                }
            }
            _ => {
                let anchor = self.anchor();
                let cands = self.candidates().await?;
                match anchor_status(anchor.as_ref(), &cands) {
                    AnchorStatus::Live(c) => {
                        self.remember_title(&c.title);
                        Ok(c.to_target(TargetSource::Anchor))
                    }
                    // Falling back to the foreground here retargeted whatever
                    // window the human was using once the anchored one closed,
                    // and `ghost_key` answered with a focus-policy error that
                    // never mentioned the closed window (C11).
                    AnchorStatus::Closed => Err(GhostError::ProcessNotFound {
                        name: format!(
                            "the anchored window '{}' (it has closed, so nothing was done; pass \
                             window=<title> to choose another window, or ghost_window op=anchor \
                             clear=true to work on the foreground window again)",
                            anchor.map(|a| a.title).unwrap_or_default()
                        ),
                    }),
                    AnchorStatus::Unset => Self::foreground_target(),
                }
            }
        }
    }
}

/// Off Windows there is no posted-wheel path yet; the background policy is not
/// enforced there either, so the user-desktop scroll never routes here at
/// runtime. The stub keeps the shared MCP dispatcher compiling on every engine.
#[cfg(not(windows))]
impl GhostSession {
    pub async fn scroll_background(
        &self,
        t: &WindowTarget,
        _direction: &str,
        _amount: i32,
    ) -> Result<Value> {
        Err(GhostError::Config(format!(
            "background scroll of '{}' is not available on this platform",
            t.title
        )))
    }
}

/// What an unanchored verb targets: the live anchor, nothing (the anchored
/// window closed - an error, never a silent switch), or, only when no window
/// was ever anchored, the foreground.
#[derive(Debug, PartialEq)]
pub(crate) enum AnchorStatus<'a> {
    Unset,
    Live(&'a Candidate),
    Closed,
}

pub(crate) fn anchor_status<'a>(anchor: Option<&WindowTarget>, cands: &'a [Candidate]) -> AnchorStatus<'a> {
    let Some(a) = anchor else { return AnchorStatus::Unset };
    match cands.iter().find(|c| c.hwnd == a.hwnd && c.surface == a.surface) {
        Some(c) => AnchorStatus::Live(c),
        None => AnchorStatus::Closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchored(hwnd: isize, title: &str) -> WindowTarget {
        WindowTarget {
            hwnd,
            title: title.into(),
            pid: 1,
            minimized: false,
            surface: Surface::User,
            source: TargetSource::Explicit,
            drifted_from: None,
        }
    }

    #[test]
    fn a_live_anchor_is_used() {
        let cands = vec![c("Bed", false, 10)];
        assert_eq!(anchor_status(Some(&anchored(10, "Bed")), &cands), AnchorStatus::Live(&cands[0]));
    }

    #[test]
    fn a_closed_anchor_is_closed_even_when_the_human_has_another_window() {
        // The anchored window (10) is gone; the human's window (20) is open and
        // must not become the target.
        let cands = vec![c("Human's editor", false, 20)];
        assert_eq!(anchor_status(Some(&anchored(10, "Bed")), &cands), AnchorStatus::Closed);
    }

    #[test]
    fn the_same_handle_on_another_desktop_is_not_the_anchor() {
        let mut other = c("Bed", false, 10);
        other.surface = Surface::Hidden { desktop: "ghost-1".into() };
        assert_eq!(anchor_status(Some(&anchored(10, "Bed")), &[other]), AnchorStatus::Closed);
    }

    #[test]
    fn never_anchored_is_unset() {
        assert_eq!(anchor_status(None, &[c("Bed", false, 10)]), AnchorStatus::Unset);
    }

    fn c(title: &str, minimized: bool, hwnd: isize) -> Candidate {
        Candidate {
            hwnd,
            title: title.into(),
            pid: 1,
            minimized,
            surface: Surface::User,
        }
    }

    #[test]
    fn pick_prefers_exact_then_prefix_then_substring() {
        let cands = vec![
            c("Untitled - Notepad", false, 1),
            c("Notepad", false, 2),
            c("My Notepad Notes - Word", false, 3),
        ];
        assert_eq!(pick(&cands, "notepad").unwrap().hwnd, 2);
        assert_eq!(pick(&cands, "untitled").unwrap().hwnd, 1);
        assert_eq!(pick(&cands, "notes").unwrap().hwnd, 3);
        assert!(pick(&cands, "calculator").is_none());
        assert!(pick(&cands, "   ").is_none());
    }

    #[test]
    fn pick_prefers_a_window_that_is_not_minimised_within_a_rank() {
        let cands = vec![
            c("Comet - Inbox", true, 1),
            c("Comet - Drafts", false, 2),
        ];
        assert_eq!(pick(&cands, "comet").unwrap().hwnd, 2);
        // ...but a minimised exact match still beats a live substring match.
        let cands = vec![c("Comet - Inbox", false, 1), c("Comet", true, 2)];
        assert_eq!(pick(&cands, "comet").unwrap().hwnd, 2);
    }

    #[test]
    fn pick_keeps_list_order_on_ties() {
        let cands = vec![c("OC | one", false, 1), c("OC | two", false, 2)];
        assert_eq!(pick(&cands, "OC |").unwrap().hwnd, 1);
    }

    #[test]
    fn target_json_carries_surface_and_source() {
        let t = WindowTarget {
            hwnd: 7,
            title: "X".into(),
            pid: 9,
            minimized: false,
            surface: Surface::Hidden { desktop: "auto".into() },
            source: TargetSource::Anchor,
            drifted_from: None,
        };
        let v = t.to_json();
        assert_eq!(v["surface"], "hidden");
        assert_eq!(v["desktop"], "auto");
        assert_eq!(v["source"], "anchor");
        assert!(v.get("minimized").is_none());
        assert!(v.get("title_drift").is_none());
        let u = WindowTarget { surface: Surface::User, minimized: true, ..t };
        let v = u.to_json();
        assert_eq!(v["surface"], "user");
        assert!(v.get("desktop").is_none());
        assert_eq!(v["minimized"], true);
    }

    #[test]
    fn target_json_reports_title_drift() {
        let t = WindowTarget {
            hwnd: 7,
            title: "Dockerfile | Ghost | Glama - Comet".into(),
            pid: 9,
            minimized: false,
            surface: Surface::User,
            source: TargetSource::Anchor,
            drifted_from: Some("Repository | Ghost | Glama".into()),
        };
        let v = t.to_json();
        assert_eq!(v["source"], "anchor");
        assert_eq!(v["title_drift"]["asked"], "Repository | Ghost | Glama");
        assert_eq!(v["title_drift"]["now"], "Dockerfile | Ghost | Glama - Comet");
    }

    #[test]
    fn a_stale_title_resolves_to_the_anchored_window_without_a_live_match() {
        // The agent anchored the window as "Repository | ...", the page
        // navigated and now says "Dockerfile | ...", and the agent asks for it
        // again by the title it last read.
        let cands = vec![
            c("Dockerfile | Ghost | Glama - Comet", false, 7),
            c("Terminal", false, 8),
        ];
        let history = vec!["Repository | Ghost | Glama - Comet".to_string()];
        let (hit, drifted) =
            resolve_static(&cands, "Repository | Ghost | Glama", Some((7, &Surface::User)), &history)
                .unwrap();
        assert_eq!(hit.hwnd, 7);
        assert!(drifted);
        // A minimised anchor still resolves: reads work on minimised windows.
        let cands = vec![c("Dockerfile | Ghost | Glama - Comet", true, 7)];
        let (hit, drifted) =
            resolve_static(&cands, "repository", Some((7, &Surface::User)), &history).unwrap();
        assert_eq!(hit.hwnd, 7);
        assert!(drifted);
    }

    #[test]
    fn a_live_title_match_beats_drift_and_an_unknown_title_still_misses() {
        let cands = vec![
            c("Dockerfile | Ghost | Glama - Comet", false, 7),
            c("Repository - Other Browser", false, 9),
        ];
        let history = vec!["Repository | Ghost | Glama - Comet".to_string()];
        let (hit, drifted) =
            resolve_static(&cands, "Repository", Some((7, &Surface::User)), &history).unwrap();
        assert_eq!(hit.hwnd, 9, "a window that has the title now wins");
        assert!(!drifted);
        assert!(resolve_static(&cands, "Calculator", Some((7, &Surface::User)), &history).is_none());
        assert!(resolve_static(&cands, "   ", Some((7, &Surface::User)), &history).is_none());
    }

    #[test]
    fn drift_needs_the_anchor_alive_on_the_same_surface() {
        let cands = vec![c("Dockerfile | Ghost | Glama - Comet", false, 7)];
        let history = vec!["Repository | Ghost | Glama - Comet".to_string()];
        assert!(resolve_static(&cands, "Repository", None, &history).is_none());
        assert!(resolve_static(&cands, "Repository", Some((99, &Surface::User)), &history).is_none());
        let hidden = Surface::Hidden { desktop: "auto".into() };
        assert!(resolve_static(&cands, "Repository", Some((7, &hidden)), &history).is_none());
        assert!(!query_names_history(&[], "Repository"));
    }

    #[test]
    fn candidate_listing_tags_surfaces_and_caps_length() {
        let mut cands: Vec<Candidate> = (0..15).map(|i| c(&format!("W{i}"), i == 1, i)).collect();
        cands[2].surface = Surface::Hidden { desktop: "auto".into() };
        let s = describe_candidates(&cands);
        assert!(s.contains("'W1' [minimized]"), "{s}");
        assert!(s.contains("'W2' [hidden desktop auto]"), "{s}");
        assert!(s.contains("... 3 more"), "{s}");
    }
}
