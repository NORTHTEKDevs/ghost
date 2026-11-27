//! Live: while the emergency stop is active, no action path may change
//! anything - and after `reset` the same actions work again.
//!
//! The windows are a testbed this test launches on Ghost's own hidden desktop
//! and a headless browser it launches itself; nothing else gets input. The stop
//! is raised with the process-local `STOP_FLAG` only, never `trigger_stop`: the
//! shared event would also stop every other Ghost process on this machine.
//!
//! `#[ignore]`: needs a Windows session and a built testbed
//! (`cargo build --release -p ghost-testbed`; found via GHOST_TESTBED, then
//! `$CARGO_TARGET_DIR`, then `target/`). Run:
//!   cargo test -p ghost-session --test emergency_stop_live -- --ignored --nocapture

#![cfg(windows)]

use ghost_core::input::STOP_FLAG;
use ghost_session::{By, GhostSession, WindowTarget};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

fn testbed_exe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("GHOST_TESTBED") {
        return Some(PathBuf::from(p)).filter(|p| p.exists());
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let dirs: Vec<PathBuf> = std::env::var("CARGO_TARGET_DIR")
        .map(|d| vec![PathBuf::from(d)])
        .unwrap_or_default()
        .into_iter()
        .chain([root.join("target")])
        .collect();
    dirs.iter()
        .flat_map(|d| ["release", "debug"].map(|p| d.join(p).join("ghost-testbed.exe")))
        .find(|p| p.exists())
}

fn stop() {
    STOP_FLAG.store(true, Ordering::Release);
}

fn resume() {
    STOP_FLAG.store(false, Ordering::Release);
}

/// Lowers the local stop and ends what the test started, whatever happened.
struct Cleanup<'a> {
    session: &'a GhostSession,
    pids: Vec<u32>,
    browser: bool,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        resume();
        for pid in &self.pids {
            let _ = ghost_core::process::kill(*pid);
        }
        if self.browser {
            let s = self.session;
            let _ = futures_lite_block(async { s.browser_close("stoptest").await });
        }
    }
}

/// Run a future to completion from `Drop` (no runtime handle is available there).
fn futures_lite_block<F: std::future::Future>(f: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(f))
}

#[derive(Debug)]
struct Row {
    path: &'static str,
    refused_while_stopped: bool,
    error: String,
    state_unchanged: bool,
    works_after_reset: bool,
}

impl Row {
    fn ok(&self) -> bool {
        self.refused_while_stopped && self.state_unchanged && self.works_after_reset
    }
}

async fn tab_exists(s: &GhostSession, id: &str) -> bool {
    s.browser_tabs("stoptest").await.map(|t| t.iter().any(|t| t.target_id == id)).unwrap_or(false)
}

fn is_stop_error(e: &str) -> bool {
    e.to_lowercase().contains("stop")
}

async fn count_of(s: &GhostSession, t: &WindowTarget) -> Option<u32> {
    for n in 0..50 {
        if s.hidden_find(t, By::Name(format!("Count {n}")), None).await.is_ok() {
            return Some(n);
        }
    }
    None
}

async fn edit_value(s: &GhostSession, t: &WindowTarget) -> String {
    s.hidden_value(t, By::Role("edit".into()), None).await.unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn every_action_path_refuses_while_stopped_and_works_after_reset() {
    let Some(exe) = testbed_exe() else {
        eprintln!("skipped: build the testbed (cargo build --release -p ghost-testbed)");
        return;
    };
    resume();
    let s = GhostSession::new().expect("session");
    let title = format!("Ghost Stop Bed {}", std::process::id());
    let launched = s
        .launch_hidden(&format!("\"{}\" --title \"{title}\"", exe.display()))
        .await
        .expect("launch testbed");
    assert_eq!(launched["surface"], "hidden", "the testbed must be on the hidden desktop: {launched}");
    let mut cleanup = Cleanup { session: &s, pids: vec![launched["pid"].as_u64().unwrap() as u32], browser: false };
    let t = s.resolve_target(Some(&title)).await.expect("resolve testbed");
    assert!(t.is_hidden(), "resolved target must be the hidden testbed");
    let mut rows = Vec::new();

    // act click: the Count button's caption counts its clicks.
    let before = count_of(&s, &t).await;
    stop();
    let r = s.hidden_act(&t, By::Name("Count".into()), "click", None, None, Some("button")).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    resume();
    let after_stopped = count_of(&s, &t).await;
    let again = s.hidden_act(&t, By::Name("Count".into()), "click", None, None, Some("button")).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let after_reset = count_of(&s, &t).await;
    rows.push(Row {
        path: "hidden act click",
        refused_while_stopped: r.as_ref().err().is_some_and(|e| is_stop_error(&e.to_string())),
        error: r.err().map(|e| e.to_string()).unwrap_or_default(),
        state_unchanged: after_stopped == before,
        works_after_reset: again.is_ok() && after_reset.zip(after_stopped).is_some_and(|(a, b)| a == b + 1),
    });

    // act type into the edit control.
    let before = edit_value(&s, &t).await;
    stop();
    let r = s.hidden_act(&t, By::Role("edit".into()), "type", Some("stopped"), None, None).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    resume();
    let after_stopped = edit_value(&s, &t).await;
    let again = s.hidden_act(&t, By::Role("edit".into()), "type", Some("resumed"), None, None).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let after_reset = edit_value(&s, &t).await;
    rows.push(Row {
        path: "hidden act type",
        refused_while_stopped: r.as_ref().err().is_some_and(|e| is_stop_error(&e.to_string())),
        error: r.err().map(|e| e.to_string()).unwrap_or_default(),
        state_unchanged: after_stopped == before,
        works_after_reset: again.is_ok() && after_reset.contains("resumed"),
    });

    // key: a single character goes into the edit control.
    let before = edit_value(&s, &t).await;
    stop();
    let r = s.hidden_key(&t, "q").await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    resume();
    let after_stopped = edit_value(&s, &t).await;
    let again = s.hidden_key(&t, "z").await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let after_reset = edit_value(&s, &t).await;
    rows.push(Row {
        path: "hidden key",
        refused_while_stopped: r.as_ref().err().is_some_and(|e| is_stop_error(&e.to_string())),
        error: r.err().map(|e| e.to_string()).unwrap_or_default(),
        state_unchanged: after_stopped == before,
        works_after_reset: again.is_ok() && after_reset.len() == after_stopped.len() + 1,
    });

    // click_at: the Count button's centre, in screen coordinates of the hidden desktop.
    let found = s.hidden_find(&t, By::Name("Count".into()), None).await.expect("Count button");
    let (cx, cy) = (found["center"]["x"].as_i64().unwrap() as i32, found["center"]["y"].as_i64().unwrap() as i32);
    let before = count_of(&s, &t).await;
    stop();
    let r = s.hidden_click_at(&t, cx, cy).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    resume();
    let after_stopped = count_of(&s, &t).await;
    let again = s.hidden_click_at(&t, cx, cy).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let after_reset = count_of(&s, &t).await;
    rows.push(Row {
        path: "hidden click_at",
        refused_while_stopped: r.as_ref().err().is_some_and(|e| is_stop_error(&e.to_string())),
        error: r.err().map(|e| e.to_string()).unwrap_or_default(),
        state_unchanged: after_stopped == before,
        works_after_reset: again.is_ok() && after_reset.zip(after_stopped).is_some_and(|(a, b)| a == b + 1),
    });

    // scroll: the testbed has nothing that scrolls, so only the refusal is checked.
    stop();
    let r = s.hidden_scroll(&t, "down", 1).await;
    resume();
    let again = s.hidden_scroll(&t, "down", 1).await;
    rows.push(Row {
        path: "hidden scroll",
        refused_while_stopped: r.as_ref().err().is_some_and(|e| is_stop_error(&e.to_string())),
        error: r.err().map(|e| e.to_string()).unwrap_or_default(),
        state_unchanged: true,
        works_after_reset: again.is_ok(),
    });

    // launch onto the hidden desktop: no new process may start.
    let title2 = format!("{title} launch");
    let cmd = format!("\"{}\" --title \"{title2}\"", exe.display());
    stop();
    let r = s.launch_hidden(&cmd).await;
    if let Ok(v) = &r {
        if let Some(pid) = v["pid"].as_u64() {
            cleanup.pids.push(pid as u32);
        }
    }
    resume();
    let started_while_stopped = r.as_ref().is_ok_and(|v| v["pid"].as_u64().is_some());
    let again = s.launch_hidden(&cmd).await;
    if let Some(pid) = again.as_ref().ok().and_then(|v| v["pid"].as_u64()) {
        cleanup.pids.push(pid as u32);
    }
    rows.push(Row {
        path: "launch (hidden)",
        refused_while_stopped: r.as_ref().err().is_some_and(|e| is_stop_error(&e.to_string())),
        error: r.err().map(|e| e.to_string()).unwrap_or_default(),
        state_unchanged: !started_while_stopped,
        works_after_reset: again.is_ok_and(|v| v["surface"] == "hidden"),
    });

    // CDP: a headless browser this test launches; a click changes the page title.
    match s.browser_launch_with("stoptest", "headless", None).await {
        Err(e) => eprintln!("CDP case skipped: no browser could be launched ({e})"),
        Ok(info) => {
            cleanup.browser = true;
            let page = "data:text/html,<title>0</title><button id=b onclick=\"document.title=+document.title+1\">b</button>";
            let tid = s.tab_open("stoptest", page).await.expect("tab");
            let tab = s.tab("stoptest", &tid).await.expect("tab handle");
            tokio::time::sleep(Duration::from_millis(300)).await;
            let title_now = || async { tab.title().await.unwrap_or_default() };
            let before = title_now().await;
            stop();
            let r = tab.click("#b", 2_000).await;
            tokio::time::sleep(Duration::from_millis(150)).await;
            resume();
            let after_stopped = title_now().await;
            let again = tab.click("#b", 2_000).await;
            tokio::time::sleep(Duration::from_millis(150)).await;
            let after_reset = title_now().await;
            rows.push(Row {
                path: "CDP tab click",
                refused_while_stopped: r.as_ref().err().is_some_and(|e| is_stop_error(&e.to_string())),
                error: r.err().map(|e| e.to_string()).unwrap_or_default(),
                state_unchanged: after_stopped == before,
                works_after_reset: again.is_ok() && after_reset != after_stopped,
            });

            // ghost_tab_eval: an agent's script can click, type or navigate, so
            // it is refused at the moment it would run, not only at MCP entry.
            let before = title_now().await;
            stop();
            let r = tab.run_script("document.title = 'script ran'").await;
            resume();
            let after_stopped = title_now().await;
            let again = tab.run_script("document.title = 'script ran'").await;
            let after_reset = title_now().await;
            rows.push(Row {
                path: "CDP tab script",
                refused_while_stopped: r.as_ref().err().is_some_and(|e| is_stop_error(&e.to_string())),
                error: r.err().map(|e| e.to_string()).unwrap_or_default(),
                state_unchanged: after_stopped == before,
                works_after_reset: again.is_ok() && after_reset == "script ran",
            });

            // Closing during a stop: a tab of the browser Ghost launched may be
            // closed (it is Ghost's); the same tab reached through an ATTACHED
            // entry (a browser Ghost did not launch - here the test's own browser
            // attached a second time, so no human browser is involved) may not.
            let port = info["port"].as_u64().expect("port") as u16;
            s.browser_attach("stoptest-attached", port).await.expect("attach");
            let doomed = s.tab_open("stoptest", "about:blank").await.expect("tab to close");
            stop();
            let attached = s.tab_close("stoptest-attached", &doomed).await;
            let still_open = tab_exists(&s, &doomed).await;
            let owned = s.tab_close("stoptest", &doomed).await;
            resume();
            let closed = !tab_exists(&s, &doomed).await;
            rows.push(Row {
                path: "close attached tab",
                refused_while_stopped: attached.as_ref().err().is_some_and(|e| is_stop_error(&e.to_string())),
                error: attached.err().map(|e| e.to_string()).unwrap_or_default(),
                state_unchanged: still_open,
                works_after_reset: true,
            });
            rows.push(Row {
                path: "close owned tab",
                // Inverted on purpose: Ghost's own tab closes DURING the stop.
                refused_while_stopped: owned.is_ok(),
                error: owned.err().map(|e| e.to_string()).unwrap_or_default(),
                state_unchanged: closed,
                works_after_reset: true,
            });
            let _ = s.browser_close("stoptest-attached").await;
        }
    }

    eprintln!("{:<18} {:>8} {:>10} {:>12}  error", "path", "refused", "unchanged", "after reset");
    for r in &rows {
        eprintln!(
            "{:<18} {:>8} {:>10} {:>12}  {}",
            r.path, r.refused_while_stopped, r.state_unchanged, r.works_after_reset, r.error
        );
    }
    drop(cleanup);
    let failed: Vec<_> = rows.iter().filter(|r| !r.ok()).map(|r| r.path).collect();
    assert!(failed.is_empty(), "paths not gated by the emergency stop: {failed:?}");
}
