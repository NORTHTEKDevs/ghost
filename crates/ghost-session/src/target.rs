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
        }
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
        let t = c.title.to_lowercase();
        let rank = if t == q {
            0
        } else if t.starts_with(&q) {
            1
        } else if t.contains(&q) {
            2
        } else {
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

    /// Remember `t` as the implicit target of later unanchored verbs.
    pub fn set_anchor(&self, t: &WindowTarget) {
        let mut stored = t.clone();
        stored.source = TargetSource::Anchor;
        *self.anchor.lock().unwrap_or_else(|p| p.into_inner()) = Some(stored);
    }

    pub fn clear_anchor(&self) {
        *self.anchor.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// The anchor if its window still exists, with its title refreshed. A dead
    /// anchor is cleared so it cannot keep steering verbs at a closed window.
    pub async fn live_anchor(&self) -> Option<WindowTarget> {
        let a = self.anchor()?;
        let cands = self.candidates().await.ok()?;
        match cands
            .iter()
            .find(|c| c.hwnd == a.hwnd && c.surface == a.surface)
        {
            Some(c) => Some(c.to_target(TargetSource::Anchor)),
            None => {
                self.clear_anchor();
                None
            }
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
            },
        })
    }

    /// Resolve the window a verb should act on.
    ///
    /// `Some(title)`: `"foreground"` is the human's foreground window (never
    /// anchored); any other title is matched across the user desktop and every
    /// hidden desktop, retried briefly to absorb a launch race, and the hit
    /// becomes the session anchor.
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
                    if let Some(c) = pick(&cands, w) {
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
            _ => match self.live_anchor().await {
                Some(a) => Ok(a),
                None => Self::foreground_target(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let v = t.to_json();
        assert_eq!(v["surface"], "hidden");
        assert_eq!(v["desktop"], "auto");
        assert_eq!(v["source"], "anchor");
        assert!(v.get("minimized").is_none());
        let u = WindowTarget { surface: Surface::User, minimized: true, ..t };
        let v = u.to_json();
        assert_eq!(v["surface"], "user");
        assert!(v.get("desktop").is_none());
        assert_eq!(v["minimized"], true);
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
