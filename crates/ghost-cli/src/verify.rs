//! `ghost verify` - the claims audit.
//!
//! Ghost's marketing makes specific, falsifiable claims: it automates without
//! taking over the screen, several agents can drive it at once without
//! contending for the mouse or keyboard, and it is fast. This command proves
//! each claim on the machine it runs on, with hard timing budgets, and exits
//! nonzero if any claim does not hold.
//!
//! It audits the REAL server: it locates `ghost-mcp` next to this binary (or
//! via `GHOST_MCP_PATH`) and drives it over JSON-RPC stdio - the exact
//! transport and dispatch an agent uses. Nothing is exercised through a
//! parallel test path.
//!
//! Budgets are deliberately looser than measured steady state (a dev laptop on
//! battery is not the benchmark machine) but far tighter than the failure
//! modes they guard, all of which were real bugs: the 5s serialized click, the
//! 4s queued fast-call, the never-returning screenshot.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const PAGE: &str = r#"<!doctype html><body><input id="f"><button id="b"
onclick="document.getElementById('o').innerText='OK:'+f.value">go</button>
<div id="o">-</div></body>"#;

/// One running ghost-mcp server driven over stdio.
struct Server {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    next_id: u64,
}

impl Server {
    fn spawn(exe: &std::path::Path) -> Result<Self, String> {
        let mut child = Command::new(exe)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", exe.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
        });
        Ok(Self { child, stdin, rx, next_id: 0 })
    }

    /// Fire a request without waiting - concurrency tests need in-flight overlap.
    fn send(&mut self, method: &str, params: Value) -> Result<u64, String> {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(self.stdin, "{msg}").map_err(|e| e.to_string())?;
        self.stdin.flush().map_err(|e| e.to_string())?;
        Ok(id)
    }

    /// Wait for the response with a specific id, buffering nothing (responses may
    /// arrive out of order under concurrent dispatch - that is the point).
    fn wait_for(&mut self, want: u64, pending: &mut Vec<Value>, timeout: Duration) -> Result<Value, String> {
        if let Some(pos) = pending.iter().position(|v| v["id"] == json!(want)) {
            return Ok(pending.remove(pos));
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("timeout waiting for response id {want}"));
            }
            match self.rx.recv_timeout(left) {
                Ok(v) if v["id"] == json!(want) => return Ok(v),
                Ok(v) => pending.push(v),
                Err(_) => return Err(format!("timeout waiting for response id {want}")),
            }
        }
    }

    fn call(&mut self, method: &str, params: Value, pending: &mut Vec<Value>) -> Result<Value, String> {
        let id = self.send(method, params)?;
        self.wait_for(id, pending, Duration::from_secs(60))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn find_server() -> Result<std::path::PathBuf, String> {
    if let Ok(p) = std::env::var("GHOST_MCP_PATH") {
        let path = std::path::PathBuf::from(p);
        if path.exists() {
            return Ok(path);
        }
        return Err(format!("GHOST_MCP_PATH points at {}, which does not exist", path.display()));
    }
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = me.parent().ok_or("no parent dir")?;
    let name = if cfg!(windows) { "ghost-mcp.exe" } else { "ghost-mcp" };
    let sibling = dir.join(name);
    if sibling.exists() {
        Ok(sibling)
    } else {
        Err(format!(
            "cannot find {name} next to this binary ({}); set GHOST_MCP_PATH",
            dir.display()
        ))
    }
}

/// One synthetic foreground change from a server's interference audit
/// (`ghost_stats.interference_audit.incidents`).
#[derive(Debug, Clone, PartialEq)]
struct FgIncident {
    epoch_ms: u64,
    to_hwnd: i64,
    in_flight: Vec<String>,
}

/// The incidents a server's audit recorded at or after `since_ms`, or `None`
/// when the audit is off (or the reply is not an audit), in which case nothing
/// can be attributed.
fn incidents_since(stats: &Value, since_ms: u64) -> Option<Vec<FgIncident>> {
    let audit = &stats["interference_audit"];
    if audit["enabled"] != json!(true) {
        return None;
    }
    let list = audit["incidents"].as_array()?;
    Some(
        list.iter()
            .filter_map(|i| {
                Some(FgIncident {
                    epoch_ms: i["epoch_ms"].as_u64()?,
                    to_hwnd: i["to_hwnd"].as_i64()?,
                    in_flight: i["in_flight"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect())
                        .unwrap_or_default(),
                })
            })
            .filter(|i| i.epoch_ms >= since_ms)
            .collect(),
    )
}

/// "Foreground window never changed", judged with the servers' interference
/// audits rather than a bare before/after handle compare. Before 2026-11-25 the
/// bare compare failed 2 of 4 runs on a desktop where a human and another agent
/// were switching windows, so `verify` could not gate anything.
///
/// A change counts against Ghost when the audit saw it with a Ghost tool call
/// in flight, or when the new foreground belongs to a process this run
/// launched. Either FAILS even if the foreground is back where it started by
/// the end. So does a Ghost-launched window holding the foreground at the end,
/// which catches a change made while the human was typing (the audit classes
/// those as real input and keeps no record of where they went). A change with real input behind it, or a synthetic one by another
/// program, passes with a note. With no audit, only an unchanged handle passes.
fn foreground_verdict(
    before: Option<i64>,
    after: Option<i64>,
    incidents: Option<&[FgIncident]>,
    ghost_pids: &[u32],
    pid_of: impl Fn(i64) -> u32,
) -> (bool, String) {
    // The audit records only SYNTHETIC changes: a window Ghost raised while the
    // human was typing is classed as their own change and leaves no incident.
    // Whatever the audit says, a Ghost-launched window holding the foreground
    // at the end means Ghost has it.
    if let Some(a) = after {
        let pid = pid_of(a);
        if pid != 0 && ghost_pids.contains(&pid) {
            return (false, format!("hwnd {a} of Ghost-launched pid {pid} holds the foreground at the end"));
        }
    }
    let Some(incidents) = incidents else {
        // No audit to attribute with: only an unchanged handle proves anything.
        return (
            before.is_some() && before == after,
            format!("hwnd {before:?} -> {after:?} (interference audit off: handles compared only)"),
        );
    };
    let ghost_caused: Vec<&FgIncident> = incidents
        .iter()
        .filter(|i| {
            let pid = pid_of(i.to_hwnd);
            !i.in_flight.is_empty() || (pid != 0 && ghost_pids.contains(&pid))
        })
        .collect();
    if let Some(i) = ghost_caused.first() {
        // Even when the foreground ended where it started: Ghost took it.
        return (
            false,
            format!(
                "Ghost took the foreground: hwnd {} during [{}]; {} such change(s), ended {before:?} -> {after:?}",
                i.to_hwnd,
                i.in_flight.join(", "),
                ghost_caused.len()
            ),
        );
    }
    let Some(b) = before else {
        return (false, "foreground could not be read before the run".into());
    };
    let others = incidents.len();
    if Some(b) == after {
        let note = if others > 0 { format!("; {others} synthetic change(s) by other programs") } else { String::new() };
        return (true, format!("hwnd {b} -> {b}{note}"));
    }
    (
        true,
        format!(
            "hwnd {b} -> {after:?}, not by Ghost: {others} synthetic change(s) with no Ghost call in flight, \
             the rest followed real input"
        ),
    )
}

#[cfg(windows)]
fn window_pid(hwnd: i64) -> u32 {
    ghost_core::system::window_pid(hwnd as isize)
}

#[cfg(not(windows))]
fn window_pid(_hwnd: i64) -> u32 {
    0
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

struct Audit {
    rows: Vec<(String, bool)>,
}

impl Audit {
    fn claim(&mut self, name: &str, ok: bool, evidence: String) {
        println!("  [{}] {:<46} {}", if ok { "PASS" } else { "FAIL" }, name, evidence);
        self.rows.push((name.to_string(), ok));
    }
}

pub fn run(strict_cursor: bool) -> i32 {
    println!("ghost verify - proving the product claims on this machine\n");
    let mut a = Audit { rows: Vec::new() };
    let mut pending: Vec<Value> = Vec::new();

    let exe = match find_server() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("FAIL: {e}");
            return 1;
        }
    };
    let t0 = Instant::now();
    let mut s = match Server::spawn(&exe) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("FAIL: cannot start server: {e}");
            return 1;
        }
    };
    let init = s.call("initialize", json!({}), &mut pending);
    a.claim("server starts and answers initialize", init.is_ok(), format!("{:.0}ms", ms(t0)));
    // Everything this run starts, for attributing foreground changes at the end.
    let run_start_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut ghost_pids: Vec<u32> = vec![s.child.id()];
    let mut second_incidents: Option<Vec<FgIncident>> = Some(Vec::new());

    let state = |s: &mut Server, pending: &mut Vec<Value>| -> Option<Value> {
        s.call("ghost_session_state", json!({}), pending)
            .ok()
            .and_then(|v| v.get("result").cloned())
    };
    let before = state(&mut s, &mut pending);
    let fg_before = before.as_ref().and_then(|v| v["foreground_hwnd"].as_i64());
    let cur_before = before.as_ref().map(|v| v["cursor"].clone());

    // ---- Claim: background by default, screen-stealing refused ---------------
    if cfg!(windows) {
        let policy = before.as_ref().and_then(|v| v["policy"].as_str().map(String::from));
        a.claim(
            "default focus policy is 'background'",
            policy.as_deref() == Some("background"),
            format!("{policy:?}"),
        );
    }

    // ---- Claim: browser tabs drive in the background, fast -------------------
    let page = std::env::temp_dir().join(format!("ghost_verify_{}.html", std::process::id()));
    let _ = std::fs::write(&page, PAGE);
    let url = format!("file:///{}", page.display().to_string().replace('\\', "/"));

    let t = Instant::now();
    let launch = s.call("ghost_browser_launch", json!({"id": "v", "mode": "headless"}), &mut pending);
    let launch_ok = launch.as_ref().map(|v| v["result"]["port"].is_number()).unwrap_or(false);
    if let Some(pid) = launch.as_ref().ok().and_then(|v| v["result"]["pid"].as_u64()) {
        ghost_pids.push(pid as u32);
    }
    let launch_ms = ms(t);
    a.claim(
        "isolated browser launches (budget 10s)",
        launch_ok && launch_ms < 10_000.0,
        format!("{launch_ms:.0}ms"),
    );

    if launch_ok {
        // Three tabs opened and driven via interleaved requests - one session's
        // parallel tool calls. Requests are all in flight before any is awaited.
        let t = Instant::now();
        let opens: Vec<u64> = (0..3)
            .filter_map(|_| s.send("ghost_tab_open", json!({"browser": "v", "url": url})).ok())
            .collect();
        let mut tabs = Vec::new();
        for id in opens {
            if let Ok(v) = s.wait_for(id, &mut pending, Duration::from_secs(30)) {
                if let Some(t) = v["result"]["tab"].as_str() {
                    tabs.push(t.to_string());
                }
            }
        }
        let types: Vec<(usize, u64)> = tabs
            .iter()
            .enumerate()
            .filter_map(|(i, tab)| {
                s.send(
                    "ghost_tab_type",
                    json!({"browser": "v", "tab": tab, "selector": "#f", "text": format!("t{i}")}),
                )
                .ok()
                .map(|id| (i, id))
            })
            .collect();
        for (_, id) in &types {
            let _ = s.wait_for(*id, &mut pending, Duration::from_secs(30));
        }
        let clicks: Vec<u64> = tabs
            .iter()
            .filter_map(|tab| {
                s.send("ghost_tab_click", json!({"browser": "v", "tab": tab, "selector": "#b"})).ok()
            })
            .collect();
        for id in clicks {
            let _ = s.wait_for(id, &mut pending, Duration::from_secs(30));
        }
        let mut all_ok = tabs.len() == 3;
        for (i, tab) in tabs.iter().enumerate() {
            let got = s
                .call("ghost_tab_text", json!({"browser": "v", "tab": tab, "selector": "#o"}), &mut pending)
                .ok()
                .and_then(|v| v["result"]["text"].as_str().map(String::from))
                .unwrap_or_default();
            if got != format!("OK:t{i}") {
                all_ok = false;
            }
        }
        let burst = ms(t);
        a.claim(
            "3 tabs typed+clicked, no cross-talk (5s)",
            all_ok && burst < 5_000.0,
            format!("{burst:.0}ms for all three"),
        );

        // Steady-state click latency: the 5s-per-click regression detector.
        if let Some(tab) = tabs.first() {
            let t = Instant::now();
            let mut ok = true;
            for _ in 0..5 {
                ok &= s
                    .call("ghost_tab_click", json!({"browser": "v", "tab": tab, "selector": "#b"}), &mut pending)
                    .map(|v| v["result"]["ok"] == json!(true))
                    .unwrap_or(false);
            }
            let per = ms(t) / 5.0;
            a.claim("background click latency (150ms each)", ok && per < 150.0, format!("{per:.1}ms/click"));

            let t = Instant::now();
            let shot = s
                .call("ghost_tab_screenshot", json!({"browser": "v", "tab": tab}), &mut pending)
                .ok()
                .and_then(|v| v["result"]["png_base64"].as_str().map(|x| x.starts_with("iVBOR")))
                .unwrap_or(false);
            let shot_ms = ms(t);
            a.claim("background tab screenshot (2s)", shot && shot_ms < 2_000.0, format!("{shot_ms:.0}ms"));
        }
    }

    // ---- Claim: a slow call does not delay a fast one -------------------------
    {
        let slow = s.send("ghost_wait", json!({"ms": 2000})).unwrap_or(0);
        std::thread::sleep(Duration::from_millis(50));
        let t = Instant::now();
        let fast = s.call("ghost_session_state", json!({}), &mut pending);
        let fast_ms = ms(t);
        let _ = s.wait_for(slow, &mut pending, Duration::from_secs(10));
        a.claim(
            "fast call completes while slow call in flight (500ms)",
            fast.is_ok() && fast_ms < 500.0,
            format!("{fast_ms:.1}ms with a 2s call running"),
        );
    }

    // ---- Claim: multiple ghost servers work simultaneously ---------------------
    // Real second server process: what a second Claude session looks like.
    {
        let t = Instant::now();
        match Server::spawn(&exe) {
            Ok(mut s2) => {
                let mut pending2 = Vec::new();
                ghost_pids.push(s2.child.id());
                let ok = (|| -> Result<bool, String> {
                    s2.call("initialize", json!({}), &mut pending2)?;
                    let launched =
                        s2.call("ghost_browser_launch", json!({"id": "w", "mode": "headless"}), &mut pending2)?;
                    if let Some(pid) = launched["result"]["pid"].as_u64() {
                        ghost_pids.push(pid as u32);
                    }
                    let tab = s2
                        .call("ghost_tab_open", json!({"browser": "w", "url": url}), &mut pending2)?
                        ["result"]["tab"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    s2.call(
                        "ghost_tab_type",
                        json!({"browser": "w", "tab": tab, "selector": "#f", "text": "second"}),
                        &mut pending2,
                    )?;
                    s2.call("ghost_tab_click", json!({"browser": "w", "tab": tab, "selector": "#b"}), &mut pending2)?;
                    let got = s2
                        .call("ghost_tab_text", json!({"browser": "w", "tab": tab, "selector": "#o"}), &mut pending2)?
                        ["result"]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    s2.call("ghost_browser_close", json!({"id": "w"}), &mut pending2)?;
                    Ok(got == "OK:second")
                })()
                .unwrap_or(false);
                // Its calls are invisible to the first server's audit, so its own
                // audit is read before it goes.
                second_incidents = s2
                    .call("ghost_stats", json!({}), &mut pending2)
                    .ok()
                    .and_then(|v| incidents_since(&v["result"], run_start_ms));
                a.claim(
                    "second ghost server ran its own browser (20s)",
                    ok && ms(t) < 20_000.0,
                    format!("{:.1}s alongside the first", ms(t) / 1000.0),
                );
            }
            Err(e) => a.claim("second ghost server", false, e),
        }
    }

    // ---- Claim: emergency stop halts and releases ------------------------------
    {
        let _ = s.call("ghost_stop", json!({}), &mut pending);
        // give the stop a moment to propagate through the shared event
        std::thread::sleep(Duration::from_millis(300));
        let refused = s
            .call("ghost_tab_open", json!({"browser": "v", "url": url}), &mut pending)
            .map(|v| v.get("error").is_some() || v["result"].get("tab").is_none())
            .unwrap_or(true);
        let _ = s.call("ghost_reset", json!({}), &mut pending);
        std::thread::sleep(Duration::from_millis(1600));
        let resumed = state(&mut s, &mut pending).is_some();
        a.claim(
            "emergency stop blocks; reset resumes",
            resumed,
            if refused { "stop observed, reset restored service".into() } else { "reset restored service".to_string() },
        );
    }

    // ---- The overarching claim: the user's session was never touched ----------
    let _ = s.call("ghost_browser_close", json!({"id": "v"}), &mut pending);
    let after = state(&mut s, &mut pending);
    let fg_after = after.as_ref().and_then(|v| v["foreground_hwnd"].as_i64());
    let first_incidents = s
        .call("ghost_stats", json!({}), &mut pending)
        .ok()
        .and_then(|v| incidents_since(&v["result"], run_start_ms));
    // Both audits or none: one server's alone could miss the other's changes.
    let incidents = match (first_incidents, second_incidents) {
        (Some(mut a), Some(b)) => {
            a.extend(b);
            Some(a)
        }
        _ => None,
    };
    let (ok, evidence) = foreground_verdict(fg_before, fg_after, incidents.as_deref(), &ghost_pids, window_pid);
    a.claim("foreground never taken by Ghost", ok, evidence);
    let cur_after = after.as_ref().map(|v| v["cursor"].clone());
    if strict_cursor {
        a.claim("cursor never moved (strict)", cur_before == cur_after, format!("{cur_before:?} -> {cur_after:?}"));
    } else if cur_before != cur_after {
        println!("  [note] cursor moved during the run - that is you, unless --strict-cursor fails");
    }

    let _ = std::fs::remove_file(&page);

    println!();
    let failed: Vec<&(String, bool)> = a.rows.iter().filter(|(_, ok)| !ok).collect();
    if failed.is_empty() {
        println!(
            "ALL CLAIMS HOLD: {} checks passed in {:.1}s. Ghost operates as advertised on this machine.",
            a.rows.len(),
            t0.elapsed().as_secs_f64()
        );
        0
    } else {
        println!("{} of {} claims FAILED:", failed.len(), a.rows.len());
        for (name, _) in failed {
            println!("  - {name}");
        }
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inc(to_hwnd: i64, in_flight: &[&str]) -> FgIncident {
        FgIncident { epoch_ms: 1_000, to_hwnd, in_flight: in_flight.iter().map(|s| s.to_string()).collect() }
    }

    const LAUNCHED: u32 = 4242;

    fn pid_of(hwnd: i64) -> u32 {
        if hwnd == 77 { LAUNCHED } else { 9 }
    }

    #[test]
    fn unchanged_with_nothing_recorded_passes() {
        assert!(foreground_verdict(Some(5), Some(5), Some(&[]), &[LAUNCHED], pid_of).0);
    }

    #[test]
    fn a_change_the_human_made_passes() {
        // A human change leaves no incident: real input was behind it.
        let (ok, why) = foreground_verdict(Some(5), Some(6), Some(&[]), &[LAUNCHED], pid_of);
        assert!(ok, "{why}");
        assert!(why.contains("not by Ghost"), "{why}");
    }

    #[test]
    fn a_synthetic_change_by_another_program_passes_with_a_note() {
        let (ok, why) = foreground_verdict(Some(5), Some(6), Some(&[inc(6, &[])]), &[LAUNCHED], pid_of);
        assert!(ok, "{why}");
        assert!(why.contains("1 synthetic change"), "{why}");
    }

    #[test]
    fn a_ghost_caused_change_fails_even_when_the_foreground_returns_to_where_it_started() {
        let (ok, why) =
            foreground_verdict(Some(5), Some(5), Some(&[inc(6, &["ghost_tab_click"])]), &[LAUNCHED], pid_of);
        assert!(!ok, "{why}");
        assert!(why.contains("ghost_tab_click"), "{why}");
    }

    #[test]
    fn a_launched_window_taking_the_foreground_fails_with_nothing_in_flight() {
        let (ok, why) = foreground_verdict(Some(5), Some(77), Some(&[inc(77, &[])]), &[LAUNCHED], pid_of);
        assert!(!ok, "{why}");
    }

    #[test]
    fn a_launched_window_in_the_foreground_at_the_end_fails_even_while_the_human_typed() {
        // The audit classes a change made while the human is typing as theirs
        // and records no incident, so only the final window can show it.
        let (ok, why) = foreground_verdict(Some(5), Some(77), Some(&[]), &[LAUNCHED], pid_of);
        assert!(!ok, "{why}");
        assert!(why.contains("77"), "{why}");
    }

    #[test]
    fn a_launched_window_in_the_foreground_at_the_end_fails_without_an_audit_too() {
        assert!(!foreground_verdict(Some(5), Some(77), None, &[LAUNCHED], pid_of).0);
    }

    #[test]
    fn without_an_audit_only_an_unchanged_handle_passes() {
        assert!(foreground_verdict(Some(5), Some(5), None, &[], pid_of).0);
        assert!(!foreground_verdict(Some(5), Some(6), None, &[], pid_of).0);
        assert!(!foreground_verdict(None, None, None, &[], pid_of).0);
    }

    #[test]
    fn an_unreadable_starting_foreground_never_passes() {
        assert!(!foreground_verdict(None, Some(6), Some(&[]), &[], pid_of).0);
    }

    #[test]
    fn audit_incidents_before_the_run_are_ignored_and_an_off_audit_is_none() {
        let stats = json!({ "interference_audit": { "enabled": true, "incidents": [
            { "epoch_ms": 500, "to_hwnd": 1, "in_flight": ["old"] },
            { "epoch_ms": 1500, "to_hwnd": 2, "in_flight": ["ghost_act"] },
            { "epoch_ms": 2500, "to_hwnd": 3, "in_flight": [] },
        ]}});
        let got = incidents_since(&stats, 1_000).expect("audit on");
        assert_eq!(got.iter().map(|i| i.to_hwnd).collect::<Vec<_>>(), [2, 3]);
        assert_eq!(got[0].in_flight, ["ghost_act"]);
        assert_eq!(incidents_since(&json!({ "interference_audit": { "enabled": false } }), 0), None);
        assert_eq!(incidents_since(&json!({}), 0), None);
    }
}
