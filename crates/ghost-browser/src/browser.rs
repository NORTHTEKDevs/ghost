//! Browser-level operations: enumerate tabs, open and close them, attach sessions.

use crate::cdp::{field_str, Cdp};
use crate::error::{BrowserError, Result};
use crate::launch::{self, LaunchOptions, LaunchedBrowser};
use crate::tab::{Tab, TabInfo};
use serde_json::json;

pub struct Browser {
    cdp: Cdp,
    info: LaunchedBrowser,
    /// True when ghost started this browser and is therefore responsible for
    /// shutting it down. Never true for an attached browser: killing the user's own
    /// browser because an automation finished would be indefensible.
    owned: bool,
}

impl Browser {
    /// Launch a private browser instance and connect to it.
    pub async fn launch(opts: &LaunchOptions) -> Result<Self> {
        let info = launch::launch(opts).await?;
        let cdp = Cdp::connect(&info.ws_url).await?;
        Ok(Self { cdp, info, owned: true })
    }

    /// Connect to a browser already running with `--remote-debugging-port=<port>`.
    pub async fn attach(port: u16) -> Result<Self> {
        let info = launch::attach(port).await?;
        let cdp = Cdp::connect(&info.ws_url).await?;
        Ok(Self { cdp, info, owned: false })
    }

    pub fn port(&self) -> u16 {
        self.info.port
    }

    pub fn pid(&self) -> u32 {
        self.info.pid
    }

    pub fn is_owned(&self) -> bool {
        self.owned
    }

    /// Every page target. Excludes service workers, extension pages, and the browser
    /// target itself, which are not things an agent means by "tab".
    pub async fn tabs(&self) -> Result<Vec<TabInfo>> {
        let r = self.cdp.call("Target.getTargets", json!({}), None).await?;
        let list = r.get("targetInfos").and_then(|t| t.as_array()).ok_or_else(|| {
            BrowserError::Protocol {
                method: "Target.getTargets".into(),
                detail: "no targetInfos array".into(),
            }
        })?;
        Ok(list
            .iter()
            .filter(|t| t.get("type").and_then(|x| x.as_str()) == Some("page"))
            .map(|t| TabInfo {
                target_id: t.get("targetId").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                title: t.get("title").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                url: t.get("url").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            })
            .collect())
    }

    /// Attach a session to an existing tab.
    ///
    /// `flatten: true` multiplexes the tab's session onto the one browser socket
    /// instead of opening a second connection per tab.
    pub async fn tab(&self, target_id: &str) -> Result<Tab> {
        let r = self
            .cdp
            .call(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                None,
            )
            .await?;
        let session_id = field_str(&r, "sessionId", "Target.attachToTarget")?;
        let tab = Tab::new(self.cdp.clone(), session_id, target_id.to_string(), self.owned);
        tab.enable().await?;
        Ok(tab)
    }

    /// Open a new tab and attach to it.
    ///
    /// `background: true` so opening a tab never yanks the user's view - which also
    /// means several ghost processes can each open tabs without fighting over which
    /// one is in front.
    pub async fn new_tab(&self, url: &str) -> Result<Tab> {
        let r = self
            .cdp
            .call(
                "Target.createTarget",
                json!({ "url": url, "background": true }),
                None,
            )
            .await?;
        let target_id = field_str(&r, "targetId", "Target.createTarget")?;
        self.tab(&target_id).await
    }

    pub async fn close_tab(&self, target_id: &str) -> Result<()> {
        self.cdp
            .call("Target.closeTarget", json!({ "targetId": target_id }), None)
            .await?;
        Ok(())
    }

    /// First tab whose URL or title contains `needle` (case-insensitive).
    pub async fn find_tab(&self, needle: &str) -> Result<TabInfo> {
        let n = needle.to_lowercase();
        self.tabs()
            .await?
            .into_iter()
            .find(|t| t.url.to_lowercase().contains(&n) || t.title.to_lowercase().contains(&n))
            .ok_or_else(|| BrowserError::TabNotFound(needle.to_string()))
    }

    /// Close a browser ghost launched. Attached browsers are left alone.
    pub async fn close(&self) -> Result<()> {
        if !self.owned {
            return Ok(());
        }
        // Ignore the result: Browser.close races with the socket dying, and a
        // transport error here just means the browser already went away.
        let _ = self.cdp.call("Browser.close", json!({}), None).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn attaching_to_a_dead_port_fails_fast() {
        let r = Browser::attach(1).await;
        assert!(r.is_err(), "attach to port 1 must not succeed");
    }
}
