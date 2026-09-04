//! Route window-anchored verbs through CDP when the window belongs to a
//! Chromium process that exposes a DevTools port.
//!
//! Why this exists: three weeks of transcripts showed the agent's most-driven
//! windows were the user's own Comet browser (over 1,000 anchored calls), and
//! `ghost_browser_attach` on Comet's port was attempted 13 times. Through UI
//! Automation a web page is a sparse tree (icon buttons without names, no
//! modifier-key support, actions that may activate the window). Through CDP the
//! same page has DOM names, `data-testid`s, trusted input events into a specific
//! renderer, and full key combos - and none of it can touch the user's
//! foreground. So when the window's process was started with
//! `--remote-debugging-port`, the ordinary verbs go through CDP; when it was not,
//! nothing changes and the response says how to enable it.

use crate::error::{GhostError, Result};
use crate::locator::By;
use crate::session::GhostSession;
use crate::target::WindowTarget;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
#[cfg(windows)]
use std::time::Duration;
use std::time::Instant;

/// How long a "no port" answer for a pid is trusted before re-reading its
/// command line (a browser is rarely relaunched with new flags mid-session).
#[cfg(windows)]
const PORT_CACHE_TTL: Duration = Duration::from_secs(60);

/// Per-process memo of the DevTools port lookup, plus the window -> tab
/// bindings already resolved.
#[derive(Default)]
pub struct CdpPortCache {
    entries: HashMap<u32, (Option<u16>, Instant)>,
    /// hwnd -> (browser id, target id). A window's active tab is stable across
    /// calls while its title changes constantly, and on a hidden desktop the
    /// native window title lags the page title by seconds, so a title-only match
    /// would drop the route mid-flow.
    bindings: HashMap<isize, (String, String)>,
}

/// A window that can be driven through CDP.
#[derive(Clone)]
pub struct CdpRoute {
    pub browser_id: String,
    pub port: u16,
    pub target_id: String,
    pub title: String,
    pub tab: Arc<ghost_browser::Tab>,
}

impl CdpRoute {
    pub fn to_json(&self) -> Value {
        json!({ "route": "cdp", "browser": self.browser_id, "port": self.port, "tab": self.target_id })
    }
}

/// Which page target a top-level window shows. Chromium titles its window
/// `<active tab title> - <Browser>`; the active tab is the one whose title is
/// the longest prefix of the window title. Pure, unit-tested.
pub fn tab_for_window_title<'a>(
    window_title: &str,
    tabs: &'a [ghost_browser::TabInfo],
) -> Option<&'a ghost_browser::TabInfo> {
    let wt = window_title.trim().to_lowercase();
    let mut best: Option<(&ghost_browser::TabInfo, usize)> = None;
    for t in tabs {
        let tt = t.title.trim().to_lowercase();
        if tt.is_empty() {
            continue;
        }
        let matched = if wt == tt || wt.starts_with(&format!("{tt} - ")) {
            tt.len()
        } else if wt.starts_with(&tt) {
            tt.len().saturating_sub(1)
        } else {
            continue;
        };
        match best {
            Some((_, n)) if n >= matched => {}
            _ => best = Some((t, matched)),
        }
    }
    best.map(|(t, _)| t)
}

/// One element of the accessible page view, as `describe_accessible` reports it.
#[derive(Debug, Clone)]
pub struct PageElement {
    pub name: String,
    pub role: String,
    pub selector: String,
    pub rect: (i32, i32, i32, i32),
    pub enabled: bool,
    pub value: Option<String>,
}

impl PageElement {
    fn from_json(v: &Value) -> Option<Self> {
        Some(PageElement {
            name: v["name"].as_str()?.to_string(),
            role: v["role"].as_str()?.to_string(),
            selector: v["selector"].as_str()?.to_string(),
            rect: (
                v["left"].as_i64()? as i32,
                v["top"].as_i64()? as i32,
                v["right"].as_i64()? as i32,
                v["bottom"].as_i64()? as i32,
            ),
            enabled: v["enabled"].as_bool().unwrap_or(true),
            value: v["value"].as_str().map(str::to_string),
        })
    }

    pub fn center(&self) -> (i32, i32) {
        ((self.rect.0 + self.rect.2) / 2, (self.rect.1 + self.rect.3) / 2)
    }

    pub fn to_descriptor(&self) -> crate::engine::uia::ElementDescriptor {
        crate::engine::uia::ElementDescriptor {
            name: self.name.clone(),
            role: self.role.clone(),
            left: self.rect.0,
            top: self.rect.1,
            right: self.rect.2,
            bottom: self.rect.3,
            enabled: self.enabled,
        }
    }
}

fn intent(e: impl std::fmt::Display) -> GhostError {
    GhostError::Intent(e.to_string())
}

impl GhostSession {
    /// The DevTools port of the process behind `pid`, memoised.
    #[cfg(windows)]
    fn cdp_port_for_pid(&self, pid: u32) -> Option<u16> {
        let mut cache = self.cdp_ports.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((port, at)) = cache.entries.get(&pid) {
            if port.is_some() || at.elapsed() < PORT_CACHE_TTL {
                return *port;
            }
        }
        let port = crate::engine::process::cmdline::debug_port(pid);
        cache.entries.insert(pid, (port, Instant::now()));
        port
    }

    #[cfg(not(windows))]
    fn cdp_port_for_pid(&self, _pid: u32) -> Option<u16> {
        None
    }

    /// Whether CDP routing is on (`GHOST_CDP_ROUTE=off` disables it).
    fn cdp_route_enabled() -> bool {
        !matches!(std::env::var("GHOST_CDP_ROUTE"), Ok(v) if v.trim().eq_ignore_ascii_case("off"))
    }

    /// The CDP route for a window, if its process exposes a DevTools port and
    /// one of its page targets matches the window title. Attaches (once per
    /// port, id `auto:<port>`) and enables focus emulation on the tab.
    pub async fn cdp_route_for(&self, t: &WindowTarget) -> Option<CdpRoute> {
        if !Self::cdp_route_enabled() || t.pid == 0 {
            return None;
        }
        let port = self.cdp_port_for_pid(t.pid)?;
        let browser_id = format!("auto:{port}");
        let browser = {
            let existing = self.browsers.lock().await.browsers.get(&browser_id).cloned();
            match existing {
                Some(b) => b,
                None => match ghost_browser::Browser::attach(port).await {
                    Ok(b) => {
                        let b = Arc::new(b);
                        self.browsers
                            .lock()
                            .await
                            .browsers
                            .insert(browser_id.clone(), b.clone());
                        b
                    }
                    Err(e) => {
                        tracing::debug!(pid = t.pid, port, "cdp route: attach failed: {e}");
                        // Chromium 136+ ignores the switch on its default profile,
                        // so a port in the command line is not proof it listens.
                        self.cdp_ports
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .entries
                            .insert(t.pid, (None, Instant::now()));
                        return None;
                    }
                },
            }
        };
        let tabs = browser.tabs().await.ok()?;
        // Resolution ladder: the tab whose title the window shows (the user may
        // have switched tabs since last time), else the binding remembered for
        // this window (title lag), else the only tab there is.
        let bound = self
            .cdp_ports
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .bindings
            .get(&t.hwnd)
            .cloned();
        let info = tab_for_window_title(&t.title, &tabs)
            .or_else(|| {
                bound
                    .as_ref()
                    .filter(|(b, _)| *b == browser_id)
                    .and_then(|(_, target)| tabs.iter().find(|x| x.target_id == *target))
            })
            .or_else(|| if tabs.len() == 1 { tabs.first() } else { None })?;
        let tab = self.tab(&browser_id, &info.target_id).await.ok()?;
        self.cdp_ports
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .bindings
            .insert(t.hwnd, (browser_id.clone(), info.target_id.clone()));
        // Idempotent and cheap; the page must think it is focused for typing
        // to land in editors that check document.hasFocus().
        let _ = tab.set_focus_emulation(true).await;
        Some(CdpRoute {
            browser_id,
            port,
            target_id: info.target_id.clone(),
            title: info.title.clone(),
            tab,
        })
    }

    /// The accessible page view through the route.
    pub async fn cdp_describe(
        &self,
        r: &CdpRoute,
        limit: usize,
        name: Option<&str>,
        role: Option<&str>,
    ) -> Result<Vec<PageElement>> {
        let v = r
            .tab
            .describe_accessible(limit.max(1), name, role)
            .await
            .map_err(intent)?;
        Ok(v.as_array()
            .map(|a| a.iter().filter_map(PageElement::from_json).collect())
            .unwrap_or_default())
    }

    /// Locate the `index`-th element matching `by` on the page.
    pub async fn cdp_find(&self, r: &CdpRoute, by: &By, index: Option<usize>) -> Result<(PageElement, usize)> {
        let (name, role) = match by {
            By::Name(n) => (Some(n.as_str()), None),
            By::Role(rl) => (None, Some(rl.as_str())),
            By::Description(d) => {
                return Err(GhostError::Vision(format!(
                    "description targets need vision grounding; through CDP locate by name or role (description={d})"
                )))
            }
        };
        let cap = if index.is_some() { 64 } else { 1 };
        let els = self.cdp_describe(r, cap, name, role).await?;
        let total = els.len();
        let idx = index.unwrap_or(0);
        els.into_iter()
            .nth(idx)
            .map(|e| (e, total))
            .ok_or_else(|| GhostError::ElementNotFound {
                query: format!("{by:?} (index {idx}, {total} match(es)) in page '{}'", r.title),
                screenshot: None,
            })
    }

    /// Convert screen pixels to the page's viewport coordinates.
    pub async fn cdp_screen_to_viewport(&self, r: &CdpRoute, x: i32, y: i32) -> Result<(f64, f64)> {
        let (ox, oy, dpr) = r.tab.viewport_origin().await.map_err(intent)?;
        let dpr = if dpr > 0.0 { dpr } else { 1.0 };
        Ok(((x as f64 - ox) / dpr, (y as f64 - oy) / dpr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: &str, title: &str) -> ghost_browser::TabInfo {
        ghost_browser::TabInfo {
            target_id: id.into(),
            title: title.into(),
            url: String::new(),
        }
    }

    #[test]
    fn window_title_picks_the_active_tab_by_longest_prefix() {
        let tabs = vec![
            tab("a", "Inbox"),
            tab("b", "Inbox (3) - Gmail"),
            tab("c", "Meta Business Suite"),
        ];
        assert_eq!(tab_for_window_title("Inbox (3) - Gmail - Comet", &tabs).unwrap().target_id, "b");
        assert_eq!(tab_for_window_title("Meta Business Suite - Google Chrome", &tabs).unwrap().target_id, "c");
        assert_eq!(tab_for_window_title("meta business suite", &tabs).unwrap().target_id, "c");
        assert!(tab_for_window_title("Settings - Comet", &tabs).is_none());
        assert!(tab_for_window_title("Inbox (3) - Gmail - Comet", &[]).is_none());
    }

    #[test]
    fn page_element_parses_and_centres() {
        let v = json!({ "name": "Post", "role": "button", "selector": "#p", "left": 10, "top": 20, "right": 30, "bottom": 40, "enabled": false, "value": null });
        let e = PageElement::from_json(&v).unwrap();
        assert_eq!(e.center(), (20, 30));
        assert!(!e.enabled);
        assert_eq!(e.to_descriptor().role, "button");
        assert!(PageElement::from_json(&json!({ "name": "x" })).is_none());
    }
}
