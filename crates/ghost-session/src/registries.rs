//! Browser and isolated-desktop registries, and the session methods that drive
//! them. Split from `session.rs` because that file is orchestration for the
//! grounding/act pipeline; this is lifetime management for long-lived external
//! resources (browser processes, desktop worker threads) keyed by caller ids.
//!
//! Everything here is inherently background: CDP injects input into a specific
//! renderer, and an isolated desktop is never displayed at all. Nothing touches
//! the user's cursor, keyboard focus, or foreground window.

use crate::error::{GhostError, Result};
use crate::session::GhostSession;
use std::collections::HashMap;
use std::sync::Arc;

/// Browsers and tabs opened by this session, keyed by caller-chosen id.
///
/// Tab handles are cached rather than re-attached per call: every
/// `Target.attachToTarget` opens a new CDP session in the browser, so
/// re-attaching on each tool call would leak sessions for the browser lifetime.
#[derive(Default)]
pub struct BrowserRegistry {
    pub(crate) browsers: HashMap<String, Arc<ghost_browser::Browser>>,
    pub(crate) tabs: HashMap<String, Arc<ghost_browser::Tab>>,
    /// Ids (sanitized) of launches in flight, so two concurrent launches of one
    /// id cannot both start a browser on the same profile directory.
    /// A std mutex so a [`LaunchClaim`] can release synchronously on drop,
    /// including when the launch future is abandoned mid-await.
    launching: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

/// An id claimed for a launch in flight; released when dropped, so a launch
/// cut off by the tool deadline or a cancelled request does not keep it.
pub(crate) struct LaunchClaim {
    launching: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    key: String,
}

impl Drop for LaunchClaim {
    fn drop(&mut self) {
        self.launching.lock().unwrap_or_else(|p| p.into_inner()).remove(&self.key);
    }
}

impl BrowserRegistry {
    /// Claim `id` for a launch. Refused at once when a launch of it is in
    /// flight or a browser is registered under it (or under an id that maps to
    /// the same profile directory) - the second launch used to hang 20 s on the
    /// locked profile and then report "did not publish a DevTools port".
    pub(crate) fn reserve_launch(&self, id: &str) -> std::result::Result<LaunchClaim, String> {
        let key = sanitize_id(id);
        let mut launching = self.launching.lock().unwrap_or_else(|p| p.into_inner());
        let taken = launching.contains(&key) || self.browsers.keys().any(|k| sanitize_id(k) == key);
        if taken {
            return Err(format!(
                "a browser is already registered as '{id}' (or is launching); use a different id, or ghost_browser_close it first"
            ));
        }
        launching.insert(key.clone());
        Ok(LaunchClaim { launching: Arc::clone(&self.launching), key })
    }
}

/// Run `launch` with `id` claimed in the registry for its duration.
async fn launch_claimed<T>(
    registry: &tokio::sync::Mutex<BrowserRegistry>,
    id: &str,
    launch: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    let _claim = registry.lock().await.reserve_launch(id).map_err(GhostError::Intent)?;
    launch.await
}

/// Keep a caller-supplied id safe to use as a directory name.
fn sanitize_id(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "default".to_string()
    } else {
        cleaned.chars().take(48).collect()
    }
}

/// Whether a close may run now. While the emergency stop is active only what
/// Ghost itself created may be closed (a browser it launched and that
/// browser's tabs, its own hidden desktops): closing those only reduces what
/// Ghost can do. A browser Ghost only attached to belongs to the human, and
/// stays untouched until the stop is lifted.
fn may_close(stopped: bool, owned: bool) -> bool {
    !stopped || owned
}

fn ensure_may_close(owned: bool) -> Result<()> {
    if may_close(crate::error::is_stopped(), owned) {
        Ok(())
    } else {
        Err(GhostError::Stopped)
    }
}

impl GhostSession {
    // =======================================================================
    // Focus policy
    // =======================================================================

    /// Current focus policy: "background", "prefer_background", or "foreground".
    #[cfg(windows)]
    pub fn focus_policy(&self) -> &'static str {
        ghost_core::focus::policy().as_str()
    }

    /// Off Windows the policy is not enforced (see `target.rs`), so every
    /// caller that branches on it takes the background path.
    #[cfg(not(windows))]
    pub fn focus_policy(&self) -> &'static str {
        "background"
    }

    /// Whether the policy can be raised above `background` from inside this
    /// process. Locked unless the operator set `GHOST_FOCUS_LOCK=off` in the
    /// server's environment; an agent cannot change it.
    #[cfg(windows)]
    pub fn focus_locked(&self) -> bool {
        ghost_core::focus::locked()
    }

    /// Off Windows there is no switchable policy at all, which for a caller is
    /// the same as a lock that is never opened.
    #[cfg(not(windows))]
    pub fn focus_locked(&self) -> bool {
        true
    }

    /// Change the focus policy for this process.
    ///
    /// `background` (the default) makes every screen-stealing primitive fail
    /// rather than take over the user's cursor, and is always accepted. The
    /// other two are refused while the operator's lock is on (the default);
    /// the error says where the work can be done without a policy change.
    #[cfg(windows)]
    pub fn set_focus_policy(&self, policy: &str) -> Result<&'static str> {
        let p: ghost_core::focus::FocusPolicy = policy.parse().map_err(|_| {
            GhostError::Intent(format!(
                "unknown focus policy '{policy}'; use background, prefer_background, or foreground"
            ))
        })?;
        // Dropping to `background` only takes powers away, so it stays
        // available while stopped; anything that hands out real input does not.
        if p != ghost_core::focus::FocusPolicy::Background {
            crate::error::ensure_not_stopped()?;
        }
        ghost_core::focus::set_policy(p).map_err(GhostError::Core)?;
        Ok(p.as_str())
    }

    // =======================================================================
    // Browsers and tabs (CDP) - platform-neutral
    // =======================================================================

    /// Launch an isolated browser under `id`. `mode` is "headless" or "windowed";
    /// `which` selects an installed browser ("chrome", "comet", "edge", "brave")
    /// or the first installed when None.
    pub async fn browser_launch_with(
        &self,
        id: &str,
        mode: &str,
        which: Option<&str>,
    ) -> Result<serde_json::Value> {
        crate::error::ensure_not_stopped()?;
        launch_claimed(&self.browsers, id, self.launch_reserved(id, mode, which)).await
    }

    async fn launch_reserved(
        &self,
        id: &str,
        mode: &str,
        which: Option<&str>,
    ) -> Result<serde_json::Value> {
        let mode: ghost_browser::LaunchMode =
            mode.parse().unwrap_or(ghost_browser::LaunchMode::Headless);
        let executable = match which {
            Some(name) => Some(
                ghost_browser::find_named_browser(name)
                    .map_err(|e| GhostError::Intent(e.to_string()))?,
            ),
            None => None,
        };
        let mut dir = std::env::temp_dir();
        dir.push("ghost-browser-profiles");
        dir.push(format!("p{}-{}", std::process::id(), sanitize_id(id)));
        #[allow(unused_mut)]
        let mut opts = ghost_browser::LaunchOptions {
            mode,
            user_data_dir: dir,
            executable,
            ..Default::default()
        };
        // A windowed browser is never born on the user's desktop. Chromium
        // activates its first window on launch in every launch style (measured
        // for Edge and Chrome: normal, hidden, minimized, from a background
        // parent), which hands the human's keyboard to an invisible window.
        // The hidden desktop has its own input queue, so that cannot happen
        // there, and CDP is a local TCP port that does not care where the
        // window lives.
        #[cfg(windows)]
        if mode == ghost_browser::LaunchMode::Windowed {
            let d = self.auto_desktop().await?;
            opts.desktop = Some(d.name().to_string());
        }
        let browser = ghost_browser::Browser::launch(&opts)
            .await
            .map_err(|e| GhostError::Intent(e.to_string()))?;
        let mut info = serde_json::json!({
            "id": id,
            "port": browser.port(),
            "pid": browser.pid(),
            "dies_with_server": browser.job_bound(),
            "browser": which.unwrap_or("default"),
            "mode": format!("{mode:?}").to_lowercase(),
        });
        if let Some(desktop) = opts.desktop.as_deref() {
            let _ = desktop;
            info["desktop"] = serde_json::Value::String("auto".into());
            info["note"] = serde_json::Value::String(
                "windowed browser started on the hidden desktop 'auto': never on your screen, \
                 never takes focus. Drive tabs with ghost_tab_*; the window itself is also \
                 reachable by title through ghost_see/ghost_act (surface=hidden)."
                    .into(),
            );
        }
        self.browsers
            .lock()
            .await
            .browsers
            .insert(id.to_string(), Arc::new(browser));
        Ok(info)
    }

    /// Attach to a browser already running with `--remote-debugging-port=<port>`.
    /// Ghost never closes a browser it did not start.
    pub async fn browser_attach(&self, id: &str, port: u16) -> Result<serde_json::Value> {
        crate::error::ensure_not_stopped()?;
        let browser = ghost_browser::Browser::attach(port)
            .await
            .map_err(|e| GhostError::Intent(e.to_string()))?;
        let info = serde_json::json!({ "id": id, "port": browser.port(), "attached": true });
        self.browsers
            .lock()
            .await
            .browsers
            .insert(id.to_string(), Arc::new(browser));
        Ok(info)
    }

    async fn browser_handle(&self, id: &str) -> Result<Arc<ghost_browser::Browser>> {
        self.browsers.lock().await.browsers.get(id).cloned().ok_or_else(|| {
            GhostError::Intent(format!(
                "no browser registered as '{id}'; call ghost_browser_launch or ghost_browser_attach first"
            ))
        })
    }

    /// Close a browser ghost launched and forget its tabs. Attached browsers are
    /// disconnected but left running - they belong to the user.
    pub async fn browser_close(&self, id: &str) -> Result<()> {
        let browser = self.browser_handle(id).await?;
        ensure_may_close(browser.is_owned())?;
        browser
            .close()
            .await
            .map_err(|e| GhostError::Intent(e.to_string()))?;
        let mut reg = self.browsers.lock().await;
        reg.browsers.remove(id);
        let prefix = format!("{id}/");
        reg.tabs.retain(|k, _| !k.starts_with(&prefix));
        Ok(())
    }

    pub async fn browser_tabs(&self, id: &str) -> Result<Vec<ghost_browser::TabInfo>> {
        self.browser_handle(id)
            .await?
            .tabs()
            .await
            .map_err(|e| GhostError::Intent(e.to_string()))
    }

    /// Open a background tab and return its target id. Opening never brings the
    /// tab or its window to the front.
    pub async fn tab_open(&self, browser_id: &str, url: &str) -> Result<String> {
        crate::error::ensure_not_stopped()?;
        let browser = self.browser_handle(browser_id).await?;
        let tab = browser
            .new_tab(url)
            .await
            .map_err(|e| GhostError::Intent(e.to_string()))?;
        let target_id = tab.target_id().to_string();
        self.browsers
            .lock()
            .await
            .tabs
            .insert(format!("{browser_id}/{target_id}"), Arc::new(tab));
        Ok(target_id)
    }

    /// Open a background tab at `url` and return once the page has loaded
    /// (as `ghost_tab_navigate` does): its id, the navigation's note if any
    /// (a download, an empty 204 reply) and the URL it landed on.
    ///
    /// The tab opens at about:blank and is then navigated: a tab created at
    /// the URL reports `readyState == "complete"` for its initial blank
    /// document, so polling it would return before the page arrived.
    /// `timeout_ms` bounds the whole navigation, the wait for the server's
    /// reply included. When the load fails or times out, the tab is closed
    /// and the error says whether the close worked.
    pub async fn tab_open_loaded(
        &self,
        browser_id: &str,
        url: &str,
        timeout_ms: u64,
    ) -> Result<(String, Option<&'static str>, String)> {
        let target_id = self.tab_open(browser_id, "about:blank").await?;
        let failed = match self.tab(browser_id, &target_id).await {
            Err(e) => e.to_string(),
            Ok(tab) => {
                let limit = std::time::Duration::from_millis(timeout_ms);
                let load = async {
                    let note = tab.navigate(url, timeout_ms).await?;
                    Ok::<_, ghost_browser::BrowserError>((note, tab.url().await.unwrap_or_default()))
                };
                match tokio::time::timeout(limit, load).await {
                    Ok(Ok((note, landed))) => return Ok((target_id, note, landed)),
                    Ok(Err(e)) => e.to_string(),
                    Err(_) => format!("the page did not load within {timeout_ms} ms"),
                }
            }
        };
        let closed = match self.tab_close(browser_id, &target_id).await {
            Ok(()) => "the tab it opened was closed".to_string(),
            Err(e) => format!("the tab it opened ({target_id}) could not be closed and may remain: {e}"),
        };
        Err(GhostError::Intent(format!("{failed}; {closed}")))
    }

    /// Get a cached tab handle, attaching on first use.
    pub async fn tab(&self, browser_id: &str, target_id: &str) -> Result<Arc<ghost_browser::Tab>> {
        let key = format!("{browser_id}/{target_id}");
        if let Some(t) = self.browsers.lock().await.tabs.get(&key) {
            return Ok(t.clone());
        }
        let browser = self.browser_handle(browser_id).await?;
        let tab = Arc::new(
            browser
                .tab(target_id)
                .await
                .map_err(|e| GhostError::Intent(e.to_string()))?,
        );
        self.browsers.lock().await.tabs.insert(key, tab.clone());
        Ok(tab)
    }

    pub async fn tab_close(&self, browser_id: &str, target_id: &str) -> Result<()> {
        let browser = self.browser_handle(browser_id).await?;
        ensure_may_close(browser.is_owned())?;
        browser
            .close_tab(target_id)
            .await
            .map_err(|e| GhostError::Intent(e.to_string()))?;
        self.browsers
            .lock()
            .await
            .tabs
            .remove(&format!("{browser_id}/{target_id}"));
        Ok(())
    }

    /// First tab in `browser_id` whose URL or title contains `needle`.
    pub async fn tab_find(&self, browser_id: &str, needle: &str) -> Result<ghost_browser::TabInfo> {
        self.browser_handle(browser_id)
            .await?
            .find_tab(needle)
            .await
            .map_err(|e| GhostError::Intent(e.to_string()))
    }

    // =======================================================================
    // Isolated desktops - Windows only. Apps launched onto one never appear on
    // the user's screen at all.
    // =======================================================================

    #[cfg(windows)]
    async fn desktop_handle(&self, id: &str) -> Result<Arc<ghost_core::DesktopSession>> {
        self.desktops.lock().await.get(id).cloned().ok_or_else(|| {
            GhostError::Intent(format!(
                "no isolated desktop registered as '{id}'; call ghost_desktop_create first"
            ))
        })
    }

    #[cfg(windows)]
    pub async fn desktop_create(&self, id: &str) -> Result<serde_json::Value> {
        crate::error::ensure_not_stopped()?;
        let d = ghost_core::DesktopSession::create(id).map_err(GhostError::Core)?;
        let info = serde_json::json!({
            "id": id,
            "desktop": d.name(),
            "real_input_supported": d.real_input_supported(),
        });
        self.desktops
            .lock()
            .await
            .insert(id.to_string(), Arc::new(d));
        Ok(info)
    }

    /// Destroy an isolated desktop. Processes still running on it are terminated
    /// first: they have no visible window, so leaving them would strand them.
    #[cfg(windows)]
    pub async fn desktop_close(&self, id: &str) -> Result<serde_json::Value> {
        // Every desktop in the registry is one Ghost created: closing it is
        // allowed while stopped (it only ends what Ghost started).
        let d = self.desktop_handle(id).await?;
        ensure_may_close(true)?;
        let mut killed = Vec::new();
        if let Ok(windows) = d.all_windows() {
            let mut pids: Vec<u32> = windows.into_iter().map(|w| w.pid).collect();
            pids.sort_unstable();
            pids.dedup();
            for pid in pids {
                if ghost_core::process::kill(pid).is_ok() {
                    killed.push(pid);
                }
            }
        }
        self.desktops.lock().await.remove(id);
        Ok(serde_json::json!({ "ok": true, "terminated_pids": killed }))
    }

    #[cfg(windows)]
    pub async fn desktop_launch(&self, id: &str, command: &str) -> Result<u32> {
        crate::error::ensure_not_stopped()?;
        crate::shell::ensure_launch_allowed(command)?;
        self.desktop_handle(id)
            .await?
            .launch(command)
            .map_err(GhostError::Core)
    }

    #[cfg(windows)]
    pub async fn desktop_windows(&self, id: &str) -> Result<Vec<serde_json::Value>> {
        let windows = self
            .desktop_handle(id)
            .await?
            .windows()
            .map_err(GhostError::Core)?;
        Ok(windows
            .into_iter()
            .map(|w| serde_json::json!({ "hwnd": w.hwnd, "title": w.title, "pid": w.pid }))
            .collect())
    }

    #[cfg(windows)]
    pub async fn desktop_wait_for_window(
        &self,
        id: &str,
        needle: &str,
        timeout_ms: u64,
    ) -> Result<serde_json::Value> {
        let w = self
            .desktop_handle(id)
            .await?
            .wait_for_window(needle, timeout_ms)
            .map_err(GhostError::Core)?;
        Ok(serde_json::json!({ "hwnd": w.hwnd, "title": w.title, "pid": w.pid }))
    }

    #[cfg(windows)]
    pub async fn desktop_click(
        &self,
        id: &str,
        hwnd: isize,
        x: i32,
        y: i32,
        button: &str,
    ) -> Result<()> {
        crate::error::ensure_not_stopped()?;
        let d = self.desktop_handle(id).await?;
        match button {
            "right" => d.right_click(hwnd, x, y),
            "double" => d.double_click(hwnd, x, y),
            _ => d.click(hwnd, x, y),
        }
        .map_err(GhostError::Core)
    }

    #[cfg(windows)]
    pub async fn desktop_scroll(
        &self,
        id: &str,
        hwnd: isize,
        direction: &str,
        amount: i32,
    ) -> Result<()> {
        crate::error::ensure_not_stopped()?;
        let (notches, horizontal) = match direction {
            "up" => (amount, false),
            "down" => (-amount, false),
            "right" => (amount, true),
            "left" => (-amount, true),
            _ => return Err(GhostError::Intent("invalid scroll direction".into())),
        };
        self.desktop_handle(id)
            .await?
            .scroll(hwnd, 0, 0, notches, horizontal)
            .map_err(GhostError::Core)
    }

    #[cfg(windows)]
    pub async fn desktop_type(&self, id: &str, hwnd: isize, text: &str) -> Result<()> {
        crate::error::ensure_not_stopped()?;
        self.desktop_handle(id)
            .await?
            .type_text(hwnd, text)
            .map_err(GhostError::Core)
    }

    #[cfg(windows)]
    pub async fn desktop_press(&self, id: &str, hwnd: isize, key: &str) -> Result<()> {
        crate::error::ensure_not_stopped()?;
        self.desktop_handle(id)
            .await?
            .press(hwnd, key)
            .map_err(GhostError::Core)
    }

    #[cfg(windows)]
    pub async fn desktop_shortcut(&self, id: &str, hwnd: isize, name: &str) -> Result<()> {
        crate::error::ensure_not_stopped()?;
        self.desktop_handle(id)
            .await?
            .shortcut(hwnd, name)
            .map_err(GhostError::Core)
    }

    #[cfg(windows)]
    pub async fn desktop_capture(&self, id: &str, hwnd: isize) -> Result<Vec<u8>> {
        self.desktop_handle(id)
            .await?
            .capture(hwnd, false)
            .map_err(GhostError::Core)
    }

    /// Interactive elements of a window on an isolated desktop, via UIA. UIA works
    /// on a non-displayed desktop, so an app there is driveable by patterns rather
    /// than pixels.
    #[cfg(windows)]
    pub async fn desktop_describe(
        &self,
        id: &str,
        window: Option<&str>,
    ) -> Result<Vec<serde_json::Value>> {
        let w = window.map(|s| s.to_string());
        let d = self.desktop_handle(id).await?;
        let out = d
            .with_uia(move |tree| {
                tree.describe_screen(w.as_deref())
                    .map(|els| {
                        els.into_iter()
                            .map(|e| {
                                serde_json::json!({
                                    "name": e.name,
                                    "role": e.role,
                                    "rect": [e.left, e.top, e.right, e.bottom],
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .map_err(|e| e.to_string())
            })
            .map_err(GhostError::Core)?;
        out.map_err(GhostError::Intent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_launch_of_one_id_is_refused_at_once() {
        let reg = BrowserRegistry::default();
        let work = reg.reserve_launch("work").expect("first claim");
        let e = reg.reserve_launch("work").err().expect("in flight");
        assert!(e.contains("'work'") && e.contains("already"), "{e}");
        // Same profile directory after sanitizing.
        let _wslash = reg.reserve_launch("w/ork").expect("different dir is fine");
        assert!(reg.reserve_launch("w:ork").is_err(), "w/ork and w:ork share a profile dir");
        // A finished (or failed) launch releases the id so a retry works.
        drop(work);
        let _work = reg.reserve_launch("work").expect("released on drop");
        // Other ids are unaffected.
        assert!(reg.reserve_launch("other").is_ok());
    }

    /// A launch abandoned mid-flight (the tool deadline, a cancelled request)
    /// must not leave its id claimed forever.
    #[tokio::test]
    async fn an_aborted_launch_releases_its_id() {
        let reg = tokio::sync::Mutex::new(BrowserRegistry::default());
        let slow = async {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            Ok(())
        };
        let r = tokio::time::timeout(std::time::Duration::from_millis(5), launch_claimed(&reg, "work", slow)).await;
        assert!(r.is_err(), "the fake launch should still be running at the deadline");
        assert!(reg.lock().await.reserve_launch("work").is_ok(), "the aborted launch kept 'work' claimed");
    }

    #[test]
    fn ids_are_safe_as_directory_names() {
        assert_eq!(sanitize_id("work"), "work");
        assert_eq!(sanitize_id(r"a\b/c:d"), "a_b_c_d");
        assert_eq!(sanitize_id(""), "default");
        assert!(sanitize_id(&"x".repeat(100)).len() <= 48);
    }
}

#[cfg(test)]
mod close_tests {
    use super::may_close;

    #[test]
    fn while_stopped_only_what_ghost_owns_may_be_closed() {
        assert!(may_close(false, false), "not stopped: any close");
        assert!(may_close(false, true));
        assert!(may_close(true, true), "stopped: Ghost's own browser, tab or desktop");
        assert!(!may_close(true, false), "stopped: never a browser Ghost only attached to");
    }
}
