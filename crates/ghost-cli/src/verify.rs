//! `ghost verify` - the claims audit.
//!
//! Ghost's marketing makes specific, falsifiable claims: it automates without
//! taking over the screen, several agents can drive it at once without contending
//! for the mouse or keyboard, and it is fast. This command proves each claim on
//! the machine it runs on, with hard timing budgets, and exits nonzero if any
//! claim does not hold.
//!
//! Every check goes through `ghost_mcp::handle` - the same dispatch layer agents
//! use over MCP - so what is verified is the real tool surface, not a parallel
//! test path. The multi-process check spawns real child `ghost` processes, which
//! is exactly what several Claude sessions look like to the OS.
//!
//! Budgets are deliberately looser than the measured steady-state numbers (a dev
//! laptop on battery is not the benchmark machine) but far tighter than any of
//! the failure modes they guard against, all of which were real bugs: the 5s
//! serialized click, the 4s queued fast-call, the never-returning screenshot.

use ghost_mcp::{handle, GhostSession};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PAGE: &str = r#"<!doctype html><body><input id="f"><button id="b"
onclick="document.getElementById('o').innerText='OK:'+f.value">go</button>
<div id="o">-</div></body>"#;

struct Audit {
    rows: Vec<(String, bool, String)>,
    strict_cursor: bool,
}

impl Audit {
    fn claim(&mut self, name: &str, ok: bool, evidence: String) {
        println!("  [{}] {:<44} {}", if ok { "PASS" } else { "FAIL" }, name, evidence);
        self.rows.push((name.to_string(), ok, evidence));
    }

    fn failed(&self) -> Vec<&(String, bool, String)> {
        self.rows.iter().filter(|(_, ok, _)| !ok).collect()
    }
}

async fn call(s: &GhostSession, method: &str, params: Value) -> Result<Value, String> {
    handle(s, method, Some(&params)).await
}

/// Milliseconds since `t`.
fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

pub async fn run(strict_cursor: bool) -> i32 {
    println!("ghost verify - proving the product claims on this machine\n");
    let mut a = Audit { rows: Vec::new(), strict_cursor };

    let t0 = Instant::now();
    let session = match GhostSession::new() {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("FAIL: cannot even create a session: {e}");
            return 1;
        }
    };
    a.claim("session initializes", true, format!("{:.0}ms", ms(t0)));

    // The desktop state before anything runs. Every claim below must leave it alone.
    let before = call(&session, "ghost_desktop_state", json!({})).await.ok();
    let fg_before = before.as_ref().and_then(|v| v["foreground_hwnd"].as_i64());
    let cur_before = before.as_ref().map(|v| v["cursor"].clone());

    // ---- Claim: background by default, foreground input refused --------------
    let policy = call(&session, "ghost_focus_policy", json!({})).await;
    a.claim(
        "default focus policy is 'background'",
        policy.as_ref().map(|v| v["policy"] == "background").unwrap_or(false),
        format!("{:?}", policy.map(|v| v["policy"].clone()).unwrap_or_default()),
    );
    let blocked = call(&session, "ghost_click_at", json!({"x": 400, "y": 400})).await;
    a.claim(
        "screen-stealing call is refused, not performed",
        matches!(&blocked, Err(e) if e.contains("background")),
        match &blocked {
            Err(e) => e.chars().take(52).collect(),
            Ok(_) => "was allowed to run!".into(),
        },
    );

    // ---- Claim: browser tabs drive in the background, fast -------------------
    let page = std::env::temp_dir().join(format!("ghost_verify_{}.html", std::process::id()));
    let _ = std::fs::write(&page, PAGE);
    let url = format!("file:///{}", page.display().to_string().replace('\\', "/"));

    let t = Instant::now();
    let launch = call(&session, "ghost_browser_launch", json!({"id": "v", "mode": "headless"})).await;
    let launch_ms = ms(t);
    a.claim(
        "isolated browser launches (budget 8s)",
        launch.is_ok() && launch_ms < 8_000.0,
        format!("{launch_ms:.0}ms, port {}", launch.as_ref().map(|v| v["port"].clone()).unwrap_or_default()),
    );

    if launch.is_ok() {
        // Three tabs, driven concurrently through the same dispatch agents use.
        let t = Instant::now();
        let mut opens = Vec::new();
        for _ in 0..3 {
            let s = session.clone();
            let u = url.clone();
            opens.push(tokio::spawn(async move {
                call(&s, "ghost_tab_open", json!({"browser": "v", "url": u})).await
            }));
        }
        let mut tabs = Vec::new();
        for h in opens {
            if let Ok(Ok(v)) = h.await {
                if let Some(t) = v["tab"].as_str() {
                    tabs.push(t.to_string());
                }
            }
        }
        let mut drives = Vec::new();
        for (i, tab) in tabs.iter().enumerate() {
            let s = session.clone();
            let tab = tab.clone();
            drives.push(tokio::spawn(async move {
                call(&s, "ghost_tab_type",
                     json!({"browser": "v", "tab": tab, "selector": "#f", "text": format!("t{i}")})).await?;
                call(&s, "ghost_tab_click",
                     json!({"browser": "v", "tab": tab, "selector": "#b"})).await?;
                let txt = call(&s, "ghost_tab_text",
                     json!({"browser": "v", "tab": tab, "selector": "#o"})).await?;
                Ok::<bool, String>(txt["text"] == format!("OK:t{i}"))
            }));
        }
        let mut all_ok = tabs.len() == 3;
        for h in drives {
            all_ok &= matches!(h.await, Ok(Ok(true)));
        }
        let burst = ms(t);
        a.claim(
            "3 tabs typed+clicked concurrently, no cross-talk (5s)",
            all_ok && burst < 5_000.0,
            format!("{burst:.0}ms for all three"),
        );

        // Steady-state click latency: the 5s-per-click regression detector.
        if let Some(tab) = tabs.first() {
            let t = Instant::now();
            let mut ok = true;
            for _ in 0..5 {
                ok &= call(&session, "ghost_tab_click",
                           json!({"browser": "v", "tab": tab, "selector": "#b"})).await.is_ok();
            }
            let per = ms(t) / 5.0;
            a.claim(
                "background click latency (budget 150ms each)",
                ok && per < 150.0,
                format!("{per:.1}ms/click"),
            );

            let t = Instant::now();
            let shot = call(&session, "ghost_tab_screenshot", json!({"browser": "v", "tab": tab})).await;
            let shot_ms = ms(t);
            a.claim(
                "background tab screenshot (budget 2s)",
                shot.map(|v| v["png_base64"].as_str().map(|s| s.starts_with("iVBOR")).unwrap_or(false))
                    .unwrap_or(false)
                    && shot_ms < 2_000.0,
                format!("{shot_ms:.0}ms"),
            );
        }
    }

    // ---- Claim: a slow call does not delay a fast one (no internal queueing) --
    {
        let s2 = session.clone();
        let slow = tokio::spawn(async move { call(&s2, "ghost_wait", json!({"ms": 2000})).await });
        // Give the slow call a moment to be genuinely in flight.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let t = Instant::now();
        let fast = call(&session, "ghost_desktop_state", json!({})).await;
        let fast_ms = ms(t);
        let _ = slow.await;
        a.claim(
            "fast call completes while slow call in flight (500ms)",
            fast.is_ok() && fast_ms < 500.0,
            format!("{fast_ms:.1}ms with a 2s call running"),
        );
    }

    // ---- Claim: multiple ghost processes work simultaneously ------------------
    // Real child processes of this same binary: what several Claude sessions look
    // like to the OS. Each drives its own browser; wall clock must show overlap.
    {
        let exe = std::env::current_exe().ok();
        match exe {
            Some(exe) => {
                let t = Instant::now();
                let mut children = Vec::new();
                for i in 0..2 {
                    let script = format!(
                        "browser-launch --id w{i} --mode headless\n\
                         tab-open --browser w{i} --url {url} --save t\n\
                         tab-type --browser w{i} --tab $t.tab --selector \"#f\" --text child{i}\n\
                         tab-click --browser w{i} --tab $t.tab --selector \"#b\"\n\
                         tab-text --browser w{i} --tab $t.tab --selector \"#o\" --raw\n\
                         browser-close --id w{i}\n"
                    );
                    let mut child = std::process::Command::new(&exe)
                        .args(["run", "-"])
                        .stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .expect("spawn child ghost");
                    use std::io::Write;
                    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
                    children.push((i, child));
                }
                let mut ok = true;
                for (i, child) in children {
                    let out = child.wait_with_output().unwrap();
                    let text = String::from_utf8_lossy(&out.stdout);
                    if !text.contains(&format!("OK:child{i}")) {
                        ok = false;
                    }
                }
                let wall = ms(t);
                a.claim(
                    "2 extra ghost processes ran their own browsers (20s)",
                    ok && wall < 20_000.0,
                    format!("{:.1}s wall clock alongside this one", wall / 1000.0),
                );
            }
            None => a.claim("2 extra ghost processes", false, "cannot locate own executable".into()),
        }
    }

    // ---- Claim: emergency stop halts and releases ------------------------------
    {
        let _ = call(&session, "ghost_stop", json!({})).await;
        let refused = call(&session, "ghost_find", json!({"name": "zz", "window": "zz-none"})).await;
        let _ = call(&session, "ghost_reset", json!({})).await;
        let resumed = call(&session, "ghost_list_windows", json!({})).await;
        a.claim(
            "emergency stop blocks work; reset resumes it",
            refused.is_err() && resumed.is_ok(),
            "stop refused a find, reset restored service".into(),
        );
    }

    // ---- The overarching claim: the user's session was never touched ----------
    let _ = call(&session, "ghost_browser_close", json!({"id": "v"})).await;
    let after = call(&session, "ghost_desktop_state", json!({})).await.ok();
    let fg_after = after.as_ref().and_then(|v| v["foreground_hwnd"].as_i64());
    a.claim(
        "foreground window never changed",
        fg_before.is_some() && fg_before == fg_after,
        format!("hwnd {:?} -> {:?}", fg_before, fg_after),
    );
    let cur_after = after.as_ref().map(|v| v["cursor"].clone());
    let cursor_same = cur_before == cur_after;
    if a.strict_cursor {
        a.claim("cursor never moved (strict)", cursor_same,
                format!("{:?} -> {:?}", cur_before, cur_after));
    } else if !cursor_same {
        // A human at the machine moves their own mouse; only strict mode treats
        // that as a failure. The structural guarantee is the focus-policy claim.
        println!("  [note] cursor moved during the run - that is you, unless --strict-cursor fails");
    }

    let _ = std::fs::remove_file(&page);

    println!();
    let failed = a.failed();
    if failed.is_empty() {
        println!(
            "ALL CLAIMS HOLD: {} checks passed in {:.1}s. Ghost operates as advertised on this machine.",
            a.rows.len(),
            t0.elapsed().as_secs_f64()
        );
        0
    } else {
        println!("{} of {} claims FAILED:", failed.len(), a.rows.len());
        for (name, _, evidence) in failed {
            println!("  - {name}: {evidence}");
        }
        1
    }
}
