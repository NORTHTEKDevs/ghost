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

impl GhostSession {
    // =======================================================================
    // Focus policy
    // =======================================================================

    /// Current focus policy: "background", "prefer_background", or "foreground".
    #[cfg(windows)]
    pub fn focus_policy(&self) -> &'static str {
        ghost_core::focus::policy().as_str()
    }

    /// Change the focus policy for this process.
    ///
    /// `background` (the default) makes every screen-stealing primitive fail
    /// rather than take over the user's cursor. Raise it only for a target that
    /// genuinely has no background path, and set it back afterwards.
    #[cfg(windows)]
    pub fn set_focus_policy(&self, policy: &str) -> Result<&'static str> {
        let p: ghost_core::focus::FocusPolicy = policy.parse().map_err(|_| {
            GhostError::Intent(format!(
                "unknown focus policy '{policy}'; use background, prefer_background, or foreground"
            ))
        })?;
        ghost_core::focus::set_policy(p);
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
            "browser": which.unwrap_or("default"),
            "mode": format!("{mode:?}").to_lowercase(),
        });
        if opts.desktop.is_some() {
            info["desktop"] = serde_json::Value::String(crate::hidden::AUTO_DESKTOP.into());
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
        let d = self.desktop_handle(id).await?;
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
        self.desktop_handle(id)
            .await?
            .type_text(hwnd, text)
            .map_err(GhostError::Core)
    }

    #[cfg(windows)]
    pub async fn desktop_press(&self, id: &str, hwnd: isize, key: &str) -> Result<()> {
        self.desktop_handle(id)
            .await?
            .press(hwnd, key)
            .map_err(GhostError::Core)
    }

    #[cfg(windows)]
    pub async fn desktop_shortcut(&self, id: &str, hwnd: isize, name: &str) -> Result<()> {
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
    fn ids_are_safe_as_directory_names() {
        assert_eq!(sanitize_id("work"), "work");
        assert_eq!(sanitize_id(r"a\b/c:d"), "a_b_c_d");
        assert_eq!(sanitize_id(""), "default");
        assert!(sanitize_id(&"x".repeat(100)).len() <= 48);
    }
}
