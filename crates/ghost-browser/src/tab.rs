//! A single browser tab, driven entirely in the background.
//!
//! Every operation here addresses one CDP session id. Two `Tab` handles in the same
//! process, or in two different ghost processes, can run at full speed against two
//! tabs of the same window and never interfere: input goes into each renderer's own
//! event queue, not through the OS. The tab does not need to be the frontmost tab,
//! the window does not need to be focused, and the user's cursor never moves.

use crate::cdp::Cdp;
use crate::error::{BrowserError, Result};
use crate::keys;
use base64::Engine;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// Poll schedule for the wait helpers.
///
/// A flat 100ms interval cost up to 100ms on every single wait, and almost every wait
/// in practice succeeds on the first check. Starting at 5ms and backing off keeps the
/// common case instant while still not spinning the renderer during a genuinely long
/// wait.
const POLL_START_MS: u64 = 5;
const POLL_MAX_MS: u64 = 100;

/// Next poll interval: double, capped.
fn next_poll(current: u64) -> u64 {
    (current * 2).min(POLL_MAX_MS)
}

/// Screenshots get a shorter budget than a general CDP call. A tab that is not
/// compositing never answers at all, so waiting the full 30s just delays a fallback
/// that was always going to be needed.
const SCREENSHOT_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, serde::Serialize)]
pub struct TabInfo {
    pub target_id: String,
    pub title: String,
    pub url: String,
}

pub struct Tab {
    cdp: Cdp,
    session_id: String,
    target_id: String,
    /// True when ghost launched this browser. Gates actions that are invisible in a
    /// ghost-owned browser but disruptive in one the user is looking at.
    owned_browser: bool,
}

impl Tab {
    pub(crate) fn new(cdp: Cdp, session_id: String, target_id: String, owned_browser: bool) -> Self {
        Self {
            cdp,
            session_id,
            target_id,
            owned_browser,
        }
    }

    /// Whether ghost launched the browser this tab belongs to.
    pub fn is_owned(&self) -> bool {
        self.owned_browser
    }

    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.cdp.call(method, params, Some(&self.session_id)).await
    }

    /// Escape hatch for a raw CDP command against this tab. Used by diagnostics.
    pub async fn raw(&self, method: &str, params: Value) -> Result<Value> {
        self.call(method, params).await
    }

    /// Raw fire-and-forget command against this tab.
    pub fn raw_notify(&self, method: &str, params: Value) -> Result<()> {
        self.cdp.notify(method, params, Some(&self.session_id))
    }

    /// Turn on the domains every other method depends on. Safe to call repeatedly.
    pub async fn enable(&self) -> Result<()> {
        self.call("Page.enable", json!({})).await?;
        self.call("Runtime.enable", json!({})).await?;
        self.call("DOM.enable", json!({})).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Script evaluation
    // -----------------------------------------------------------------------

    /// Evaluate a JS expression in the page and return its value.
    ///
    /// `awaitPromise` is on so callers can evaluate `fetch(...).then(r => r.text())`
    /// and get the resolved value rather than a `Promise` handle.
    pub async fn eval(&self, expression: &str) -> Result<Value> {
        let r = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        // A thrown exception comes back as a successful CDP call with an
        // exceptionDetails payload; surfacing it as Ok would hide real page errors.
        if let Some(ex) = r.get("exceptionDetails") {
            let msg = ex
                .get("exception")
                .and_then(|e| e.get("description"))
                .and_then(|d| d.as_str())
                .or_else(|| ex.get("text").and_then(|t| t.as_str()))
                .unwrap_or("script error")
                .to_string();
            return Err(BrowserError::Cdp { method: "Runtime.evaluate".into(), message: msg });
        }
        Ok(r.get("result").and_then(|x| x.get("value")).cloned().unwrap_or(Value::Null))
    }

    // -----------------------------------------------------------------------
    // Navigation
    // -----------------------------------------------------------------------

    /// Navigate and wait for the document to finish loading.
    pub async fn navigate(&self, url: &str, timeout_ms: u64) -> Result<()> {
        self.call("Page.navigate", json!({ "url": url })).await?;
        self.wait_for_load(timeout_ms).await
    }

    /// Poll `document.readyState` until the page is complete.
    ///
    /// Polling rather than waiting on `Page.loadEventFired`: if the page finished
    /// loading before we started listening, the event is already gone and an
    /// event-based wait would hang for the full timeout.
    pub async fn wait_for_load(&self, timeout_ms: u64) -> Result<()> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut poll = POLL_START_MS;
        loop {
            if let Ok(v) = self.eval("document.readyState").await {
                if v.as_str() == Some("complete") {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                return Err(BrowserError::CdpTimeout {
                    method: "wait_for_load".into(),
                    ms: timeout_ms,
                });
            }
            tokio::time::sleep(Duration::from_millis(poll)).await;
            poll = next_poll(poll);
        }
    }

    pub async fn url(&self) -> Result<String> {
        Ok(self.eval("location.href").await?.as_str().unwrap_or_default().to_string())
    }

    pub async fn title(&self) -> Result<String> {
        Ok(self.eval("document.title").await?.as_str().unwrap_or_default().to_string())
    }

    // -----------------------------------------------------------------------
    // Elements
    // -----------------------------------------------------------------------

    /// Wait until `selector` matches an element.
    pub async fn wait_for_selector(&self, selector: &str, timeout_ms: u64) -> Result<()> {
        let expr = format!("document.querySelector({}) !== null", js_string(selector));
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut poll = POLL_START_MS;
        loop {
            if self.eval(&expr).await.map(|v| v == json!(true)).unwrap_or(false) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(BrowserError::SelectorNotFound {
                    selector: selector.to_string(),
                    ms: timeout_ms,
                });
            }
            tokio::time::sleep(Duration::from_millis(poll)).await;
            poll = next_poll(poll);
        }
    }

    /// Viewport-relative centre of the first element matching `selector`, after
    /// scrolling it into view. `Input.dispatchMouseEvent` takes CSS viewport
    /// coordinates, which is exactly what `getBoundingClientRect` returns.
    pub async fn element_center(&self, selector: &str) -> Result<(f64, f64)> {
        let expr = format!(
            r#"(() => {{
                const el = document.querySelector({sel});
                if (!el) return null;
                el.scrollIntoView({{block: 'center', inline: 'center'}});
                const r = el.getBoundingClientRect();
                if (r.width === 0 || r.height === 0) return null;
                return {{x: r.left + r.width / 2, y: r.top + r.height / 2}};
            }})()"#,
            sel = js_string(selector)
        );
        let v = self.eval(&expr).await?;
        if v.is_null() {
            // Distinguish "not there" from "there but unclickable" - they need
            // different fixes from whoever is driving.
            let exists = self
                .eval(&format!("document.querySelector({}) !== null", js_string(selector)))
                .await
                .unwrap_or(json!(false));
            return if exists == json!(true) {
                Err(BrowserError::NotVisible { selector: selector.to_string() })
            } else {
                Err(BrowserError::SelectorNotFound { selector: selector.to_string(), ms: 0 })
            };
        }
        let x = v.get("x").and_then(|n| n.as_f64());
        let y = v.get("y").and_then(|n| n.as_f64());
        match (x, y) {
            (Some(x), Some(y)) => Ok((x, y)),
            _ => Err(BrowserError::Protocol {
                method: "element_center".into(),
                detail: format!("unexpected rect payload: {v}"),
            }),
        }
    }

    /// Click an element with a trusted synthetic mouse event in the renderer.
    ///
    /// Not `element.click()`: that produces an untrusted event with no preceding
    /// move/press, which many sites and most drag/hover UIs ignore.
    pub async fn click(&self, selector: &str, timeout_ms: u64) -> Result<()> {
        // `element_center` already fails cleanly when the selector matches nothing, so
        // trying it first turns the common case (element already present) into one
        // round trip instead of a wait-then-locate pair. Only fall back to waiting
        // when it is genuinely not there yet.
        let (x, y) = match self.element_center(selector).await {
            Ok(c) => c,
            Err(BrowserError::SelectorNotFound { .. }) if timeout_ms > 0 => {
                self.wait_for_selector(selector, timeout_ms).await?;
                self.element_center(selector).await?
            }
            Err(e) => return Err(e),
        };
        self.click_at(x, y).await
    }

    /// Dispatch a mouse move without waiting for the acknowledgement.
    ///
    /// This is the single biggest performance fix in the browser backend. Chrome
    /// coalesces mouse-move events and only acknowledges them once the renderer
    /// produces a compositor frame - and a background or headless tab produces none,
    /// so the reply arrives after an internal ~5 second timeout. Awaiting it made
    /// every click take 5.01s.
    ///
    /// Skipping the reply is safe rather than a race: CDP processes one session's
    /// commands in arrival order, so the button-press dispatched immediately after is
    /// still handled after this move. Measured: 5.01s -> 0.75ms per click.
    fn dispatch_move(&self, x: f64, y: f64) -> Result<()> {
        self.cdp.notify(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseMoved", "x": x, "y": y, "button": "none", "buttons": 0 }),
            Some(&self.session_id),
        )
    }

    /// Click at viewport coordinates.
    pub async fn click_at(&self, x: f64, y: f64) -> Result<()> {
        self.click_at_ex(x, y, "left", 1).await
    }

    /// Click at viewport coordinates with a button (`left`|`right`|`middle`) and a
    /// click count (2 = double-click).
    pub async fn click_at_ex(&self, x: f64, y: f64, button: &str, count: u32) -> Result<()> {
        // The move arms hover state for menus and hover-reveal controls; the press
        // and release are awaited so the caller knows the click was really handled.
        self.dispatch_move(x, y)?;
        let buttons_mask = match button {
            "right" => 2,
            "middle" => 4,
            _ => 1,
        };
        for n in 1..=count.max(1) {
            let base = json!({ "x": x, "y": y, "button": button, "clickCount": n });
            let mut down = base.clone();
            down["type"] = json!("mousePressed");
            down["buttons"] = json!(buttons_mask);
            self.call("Input.dispatchMouseEvent", down).await?;

            let mut up = base;
            up["type"] = json!("mouseReleased");
            up["buttons"] = json!(0);
            self.call("Input.dispatchMouseEvent", up).await?;
        }
        Ok(())
    }

    /// Make the page believe it has focus even though its window is not the
    /// active one on any desktop. Without this, sites that check
    /// `document.hasFocus()` (editors, some login forms) ignore typed input.
    pub async fn set_focus_emulation(&self, enabled: bool) -> Result<()> {
        self.call(
            "Emulation.setFocusEmulationEnabled",
            json!({ "enabled": enabled }),
        )
        .await?;
        Ok(())
    }

    /// Where the viewport sits on the screen: `(origin_x, origin_y, dpr)` in
    /// physical screen pixels, derived from `window.screenX/Y` plus the frame
    /// chrome height. Best-effort - Chromium reports these in CSS pixels of the
    /// primary display's scale, so multi-monitor mixed-DPI layouts can be off.
    pub async fn viewport_origin(&self) -> Result<(f64, f64, f64)> {
        let v = self
            .eval(
                "(() => { const dpr = window.devicePixelRatio || 1; \
                 const ox = window.screenX + (window.outerWidth - window.innerWidth) / 2; \
                 const oy = window.screenY + (window.outerHeight - window.innerHeight); \
                 return { x: ox * dpr, y: oy * dpr, dpr }; })()",
            )
            .await?;
        let f = |k: &str| v.get(k).and_then(|n| n.as_f64());
        match (f("x"), f("y"), f("dpr")) {
            (Some(x), Some(y), Some(d)) => Ok((x, y, d)),
            _ => Err(BrowserError::Protocol {
                method: "viewport_origin".into(),
                detail: format!("unexpected payload: {v}"),
            }),
        }
    }

    /// The accessible view of the page - the CDP counterpart of a UIA walk.
    ///
    /// Every visible interactive element (plus headings and labelled text) with
    /// an accessible NAME (aria-label, text, value, placeholder, alt, title), a
    /// ROLE (explicit or derived from the tag), its viewport rect, and a selector
    /// that survives re-renders: `#id` when the element has one, otherwise a
    /// `data-ghost-id` attribute stamped on the element. Optional filters narrow
    /// the walk by name substring (case-insensitive) and role.
    pub async fn describe_accessible(
        &self,
        limit: usize,
        name: Option<&str>,
        role: Option<&str>,
    ) -> Result<Value> {
        let expr = format!(
            r#"(() => {{
                const wantName = {name};
                const wantRole = {role};
                const limit = {limit};
                const roleOf = (el) => {{
                    const r = el.getAttribute('role');
                    if (r) return r.toLowerCase();
                    const t = el.tagName.toLowerCase();
                    const ty = (el.getAttribute('type') || '').toLowerCase();
                    if (t === 'a') return 'hyperlink';
                    if (t === 'button') return 'button';
                    if (t === 'select') return 'combobox';
                    if (t === 'textarea') return 'edit';
                    if (t === 'input') {{
                        if (ty === 'checkbox') return 'checkbox';
                        if (ty === 'radio') return 'radiobutton';
                        if (ty === 'submit' || ty === 'button' || ty === 'reset' || ty === 'image') return 'button';
                        return 'edit';
                    }}
                    if (/^h[1-6]$/.test(t)) return 'heading';
                    if (t === 'img') return 'image';
                    if (t === 'li') return 'listitem';
                    if (t === 'option') return 'listitem';
                    if (el.isContentEditable) return 'edit';
                    return 'text';
                }};
                const nameOf = (el) => {{
                    const lab = el.getAttribute('aria-label');
                    if (lab) return lab;
                    const by = el.getAttribute('aria-labelledby');
                    if (by) {{ const s = by.split(/\s+/).map(id => (document.getElementById(id) || {{}}).innerText || '').join(' ').trim(); if (s) return s; }}
                    if (el.id) {{ const l = document.querySelector('label[for="' + CSS.escape(el.id) + '"]'); if (l && l.innerText.trim()) return l.innerText.trim(); }}
                    const t = el.tagName.toLowerCase();
                    if (t === 'input' || t === 'textarea' || t === 'select') {{
                        return (el.placeholder || el.value || el.getAttribute('name') || el.title || '').toString();
                    }}
                    if (t === 'img') return el.alt || el.title || '';
                    const txt = (el.innerText || el.textContent || '').trim().replace(/\s+/g, ' ');
                    return txt || el.title || el.value || '';
                }};
                const sel = 'a,button,input,select,textarea,[role],[onclick],[contenteditable=""],[contenteditable="true"],h1,h2,h3,h4,h5,h6,label,summary';
                const out = [];
                let stamp = 0;
                const vw = window.innerWidth, vh = window.innerHeight;
                for (const el of document.querySelectorAll(sel)) {{
                    if (out.length >= limit) break;
                    const r = el.getBoundingClientRect();
                    if (r.width === 0 || r.height === 0) continue;
                    if (r.right < 0 || r.bottom < 0 || r.left > vw || r.top > vh) continue;
                    const style = window.getComputedStyle(el);
                    if (style.visibility === 'hidden' || style.display === 'none') continue;
                    const role = roleOf(el);
                    const name = nameOf(el).slice(0, 160);
                    if (wantRole && role !== wantRole) continue;
                    if (wantName && !name.toLowerCase().includes(wantName)) continue;
                    let selector;
                    if (el.id) {{ selector = '#' + CSS.escape(el.id); }}
                    else {{
                        if (!el.dataset.ghostId) {{ el.dataset.ghostId = String(Date.now() % 100000) + '-' + (stamp++); }}
                        selector = '[data-ghost-id="' + el.dataset.ghostId + '"]';
                    }}
                    out.push({{
                        name, role, selector,
                        left: Math.round(r.left), top: Math.round(r.top),
                        right: Math.round(r.right), bottom: Math.round(r.bottom),
                        enabled: !(el.disabled || el.getAttribute('aria-disabled') === 'true'),
                        value: ('value' in el && typeof el.value === 'string') ? el.value.slice(0, 200) : (el.isContentEditable ? (el.innerText || '').slice(0, 200) : null)
                    }});
                }}
                return out;
            }})()"#,
            name = name
                .map(|n| js_string(&n.to_lowercase()))
                .unwrap_or_else(|| "null".into()),
            role = role
                .map(|r| js_string(&r.to_lowercase()))
                .unwrap_or_else(|| "null".into()),
            limit = limit,
        );
        self.eval(&expr).await
    }

    /// Current value / text of the element behind `selector` (the read-back that
    /// verifies a type).
    pub async fn value_of(&self, selector: &str) -> Result<String> {
        let expr = format!(
            "(() => {{ const e = document.querySelector({}); if (!e) return null; \
             return ('value' in e && typeof e.value === 'string') ? e.value : (e.innerText || e.textContent || ''); }})()",
            js_string(selector)
        );
        let v = self.eval(&expr).await?;
        if v.is_null() {
            return Err(BrowserError::SelectorNotFound { selector: selector.to_string(), ms: 0 });
        }
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// Move the pointer over an element to trigger hover states.
    ///
    /// The move itself is fire-and-forget for the reason above, followed by a trivial
    /// awaited call as a barrier: without it this would return before the renderer had
    /// processed the move, and a caller reading the hover result immediately would see
    /// the old state.
    pub async fn hover(&self, selector: &str, timeout_ms: u64) -> Result<()> {
        self.wait_for_selector(selector, timeout_ms).await?;
        let (x, y) = self.element_center(selector).await?;
        self.dispatch_move(x, y)?;
        self.eval("0").await?;
        Ok(())
    }

    /// Focus an element and type into it.
    ///
    /// `Input.insertText` rather than per-character key events: it is an order of
    /// magnitude faster, and it is layout-independent, so the text is identical no
    /// matter which keyboard layout the user has active.
    pub async fn type_text(&self, selector: &str, text: &str, clear: bool) -> Result<()> {
        self.wait_for_selector(selector, 10_000).await?;
        let focus = format!(
            r#"(() => {{
                const el = document.querySelector({sel});
                if (!el) return false;
                el.focus();
                {clear}
                return true;
            }})()"#,
            sel = js_string(selector),
            clear = if clear {
                // Fire input+change so framework-controlled inputs (React, Vue) see
                // the reset instead of silently restoring their old state.
                "if ('value' in el) { el.value = ''; \
                 el.dispatchEvent(new Event('input', {bubbles: true})); \
                 el.dispatchEvent(new Event('change', {bubbles: true})); }"
            } else {
                ""
            }
        );
        if self.eval(&focus).await? != json!(true) {
            return Err(BrowserError::SelectorNotFound {
                selector: selector.to_string(),
                ms: 0,
            });
        }
        self.call("Input.insertText", json!({ "text": text })).await?;
        Ok(())
    }

    /// Send a key to whatever is focused in the page.
    pub async fn press(&self, key: &str, modifiers: &[String]) -> Result<()> {
        let d = keys::describe(key).ok_or_else(|| BrowserError::Cdp {
            method: "Input.dispatchKeyEvent".into(),
            message: format!("unknown key name '{key}'"),
        })?;
        let mask = keys::modifier_mask(modifiers);
        let mut down = json!({
            "type": if d.text.is_empty() { "rawKeyDown" } else { "keyDown" },
            "key": d.key,
            "code": d.code,
            "windowsVirtualKeyCode": d.windows_virtual_key_code,
            "nativeVirtualKeyCode": d.windows_virtual_key_code,
            "modifiers": mask,
        });
        if !d.text.is_empty() {
            down["text"] = json!(d.text);
            down["unmodifiedText"] = json!(d.text);
        }
        self.call("Input.dispatchKeyEvent", down).await?;
        self.call(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp",
                "key": d.key,
                "code": d.code,
                "windowsVirtualKeyCode": d.windows_virtual_key_code,
                "nativeVirtualKeyCode": d.windows_virtual_key_code,
                "modifiers": mask,
            }),
        )
        .await?;
        Ok(())
    }

    /// Visible text of an element (or of the whole body when `selector` is empty).
    pub async fn text(&self, selector: &str) -> Result<String> {
        let expr = if selector.is_empty() {
            "document.body ? document.body.innerText : ''".to_string()
        } else {
            format!(
                "(() => {{ const e = document.querySelector({}); return e ? e.innerText : null; }})()",
                js_string(selector)
            )
        };
        let v = self.eval(&expr).await?;
        if v.is_null() {
            return Err(BrowserError::SelectorNotFound {
                selector: selector.to_string(),
                ms: 0,
            });
        }
        Ok(v.as_str().unwrap_or_default().to_string())
    }

    /// Scroll the page (or a scrollable element) without touching the wheel.
    pub async fn scroll(&self, selector: &str, dx: f64, dy: f64) -> Result<()> {
        let expr = if selector.is_empty() {
            format!("window.scrollBy({dx}, {dy}); true")
        } else {
            format!(
                "(() => {{ const e = document.querySelector({}); if (!e) return false; \
                 e.scrollBy({dx}, {dy}); return true; }})()",
                js_string(selector)
            )
        };
        if self.eval(&expr).await? != json!(true) {
            return Err(BrowserError::SelectorNotFound {
                selector: selector.to_string(),
                ms: 0,
            });
        }
        Ok(())
    }

    /// Choose an option in a `<select>` and fire the events a real choice fires.
    pub async fn select_option(&self, selector: &str, value: &str) -> Result<()> {
        let expr = format!(
            r#"(() => {{
                const el = document.querySelector({sel});
                if (!el) return false;
                el.value = {val};
                el.dispatchEvent(new Event('input', {{bubbles: true}}));
                el.dispatchEvent(new Event('change', {{bubbles: true}}));
                return el.value === {val};
            }})()"#,
            sel = js_string(selector),
            val = js_string(value)
        );
        if self.eval(&expr).await? != json!(true) {
            return Err(BrowserError::SelectorNotFound {
                selector: format!("{selector} (option '{value}')"),
                ms: 0,
            });
        }
        Ok(())
    }

    /// Bring this tab to the front *within its own browser window*.
    ///
    /// In a headless or off-desktop browser this is invisible to the user - there is
    /// no window on screen to raise. Ghost only calls it on browsers it launched
    /// itself; doing it to a browser the user is using would switch their tab out
    /// from under them.
    pub async fn bring_to_front(&self) -> Result<()> {
        self.call("Page.bringToFront", json!({})).await?;
        Ok(())
    }

    /// PNG screenshot of this tab.
    ///
    /// A tab that is not compositing produces no frame, and `Page.captureScreenshot`
    /// simply never returns while it waits for one - the single worst stall in the
    /// browser backend. Two ways to force a frame, measured against each other on a
    /// background tab (`examples/shot_probe.rs`):
    ///
    /// | strategy | per capture |
    /// |---|---|
    /// | nothing | never returns |
    /// | `Page.bringToFront`, then capture | **58ms** |
    /// | set metrics override, capture, clear | 83ms |
    /// | leave an override permanently in place | 8.4s |
    ///
    /// That last row is the trap: it is the *transition* of setting the override that
    /// forces a repaint, so an override left in place goes stale and the stall returns.
    ///
    /// Which strategy is legitimate depends on ownership. `bringToFront` changes which
    /// tab is active in its window - invisible and harmless in a browser ghost
    /// launched, unacceptable in the user's own browser where it would switch their
    /// tab. So owned browsers take the fast path and attached ones take the override.
    pub async fn screenshot(&self, full_page: bool) -> Result<Vec<u8>> {
        if self.owned_browser {
            // Cheap (~1ms) and idempotent, so it is re-asserted per capture rather
            // than cached: with several tabs being driven at once, whichever captured
            // last left a different tab in front.
            let _ = self.bring_to_front().await;
            match self.raw_screenshot(full_page).await {
                Ok(png) => return Ok(png),
                // Fall through to the override path rather than failing: some pages
                // still refuse to paint after bringToFront.
                Err(BrowserError::CdpTimeout { .. }) => {}
                Err(e) => return Err(e),
            }
        }
        self.screenshot_via_override(full_page).await
    }

    /// Force a repaint by toggling a device-metrics override around the capture.
    ///
    /// The override is always cleared, including on failure, or the tab would be left
    /// at a synthetic viewport size and every later coordinate would be wrong.
    async fn screenshot_via_override(&self, full_page: bool) -> Result<Vec<u8>> {
        let (w, h) = self.viewport().await.unwrap_or((1280, 900));
        let applied = self
            .call(
                "Emulation.setDeviceMetricsOverride",
                json!({ "width": w, "height": h, "deviceScaleFactor": 1, "mobile": false }),
            )
            .await
            .is_ok();
        let shot = self.raw_screenshot(full_page).await;
        if applied {
            let _ = self.call("Emulation.clearDeviceMetricsOverride", json!({})).await;
        }
        match shot {
            Ok(png) => Ok(png),
            Err(BrowserError::CdpTimeout { ms, .. }) => Err(BrowserError::Cdp {
                method: "Page.captureScreenshot".into(),
                message: format!(
                    "no frame produced in {ms}ms. This tab is not rendering - it is not                      the active tab in an attached browser. Use ghost_tab_text or                      ghost_tab_describe instead, or drive a ghost-owned browser."
                ),
            }),
            Err(e) => Err(e),
        }
    }

    async fn raw_screenshot(&self, full_page: bool) -> Result<Vec<u8>> {
        let r = self
            .cdp
            .call_with_timeout(
                "Page.captureScreenshot",
                json!({ "format": "png", "captureBeyondViewport": full_page }),
                Some(&self.session_id),
                SCREENSHOT_TIMEOUT_MS,
            )
            .await?;
        let data = r
            .get("data")
            .and_then(|d| d.as_str())
            .ok_or_else(|| BrowserError::Protocol {
                method: "Page.captureScreenshot".into(),
                detail: "no data field".into(),
            })?;
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| BrowserError::Protocol {
                method: "Page.captureScreenshot".into(),
                detail: format!("base64: {e}"),
            })
    }

    /// CSS viewport size, used to pin the override to the tab's real size.
    async fn viewport(&self) -> Result<(u64, u64)> {
        let v = self
            .eval("({w: window.innerWidth, h: window.innerHeight})")
            .await?;
        let w = v.get("w").and_then(|x| x.as_u64()).unwrap_or(0);
        let h = v.get("h").and_then(|x| x.as_u64()).unwrap_or(0);
        // A zero dimension would make the override produce an empty image.
        if w == 0 || h == 0 {
            return Err(BrowserError::Protocol {
                method: "viewport".into(),
                detail: format!("implausible viewport {w}x{h}"),
            });
        }
        Ok((w, h))
    }

    /// Structured list of interactive elements, the browser analogue of
    /// `describe_screen`. Keeps an agent off pixel coordinates entirely.
    pub async fn describe(&self, limit: usize) -> Result<Value> {
        let expr = format!(
            r#"(() => {{
                const sel = 'a,button,input,select,textarea,[role=button],[role=link],[role=tab],[role=checkbox],[onclick]';
                const out = [];
                for (const el of document.querySelectorAll(sel)) {{
                    if (out.length >= {limit}) break;
                    const r = el.getBoundingClientRect();
                    if (r.width === 0 || r.height === 0) continue;
                    const style = window.getComputedStyle(el);
                    if (style.visibility === 'hidden' || style.display === 'none') continue;
                    out.push({{
                        tag: el.tagName.toLowerCase(),
                        type: el.getAttribute('type') || '',
                        role: el.getAttribute('role') || '',
                        id: el.id || '',
                        name: el.getAttribute('name') || '',
                        text: (el.innerText || el.value || el.getAttribute('aria-label') || '').trim().slice(0, 120),
                        selector: el.id ? '#' + CSS.escape(el.id) : '',
                        x: Math.round(r.left + r.width / 2),
                        y: Math.round(r.top + r.height / 2),
                        disabled: !!el.disabled
                    }});
                }}
                return out;
            }})()"#
        );
        self.eval(&expr).await
    }
}

/// Encode a Rust string as a JS string literal.
///
/// JSON string syntax is a subset of JS string syntax, so this is a correct and
/// injection-safe way to embed a caller-supplied selector into an expression - which
/// matters, because selectors routinely contain quotes (`input[name="q"]`).
pub fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_string_escapes_embedded_quotes() {
        assert_eq!(js_string(r#"input[name="q"]"#), r#""input[name=\"q\"]""#);
    }

    #[test]
    fn js_string_neutralizes_expression_injection() {
        // A selector is untrusted input; it must not be able to close the literal
        // and run its own code.
        let hostile = r#"a"); fetch("http://evil"); ("#;
        let encoded = js_string(hostile);
        assert!(encoded.starts_with('"') && encoded.ends_with('"'));

        // Every interior quote must be backslash-escaped, so the literal cannot be
        // terminated early and the payload can never become executable code.
        let body = &encoded[1..encoded.len() - 1];
        let mut prev_backslash = false;
        for c in body.chars() {
            if c == '"' {
                assert!(prev_backslash, "unescaped quote in {encoded}");
            }
            prev_backslash = c == '\\' && !prev_backslash;
        }

        // And it must still decode back to exactly what the caller asked for.
        assert_eq!(serde_json::from_str::<String>(&encoded).unwrap(), hostile);
    }

    #[test]
    fn js_string_escapes_newlines_and_backslashes() {
        assert_eq!(js_string("a\nb"), r#""a\nb""#);
        assert_eq!(js_string(r"a\b"), r#""a\\b""#);
    }

    #[test]
    fn tab_info_serializes_for_the_mcp_layer() {
        let info = TabInfo {
            target_id: "T1".into(),
            title: "Example".into(),
            url: "https://example.com/".into(),
        };
        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v["target_id"], "T1");
        assert_eq!(v["url"], "https://example.com/");
    }
}
