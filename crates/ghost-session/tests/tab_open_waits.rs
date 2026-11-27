//! `tab_open_loaded` returns only once the page has loaded, the way
//! `ghost_tab_navigate` does. Before 0.24.4 `ghost_tab_open url=...` answered
//! while the tab still showed about:blank, so a read straight after it saw an
//! empty page.
//!
//! Uses a headless browser the test launches itself and local pages served
//! slowly on purpose; skipped when no browser is installed, unless
//! GHOST_REQUIRE_BROWSER=1.

use ghost_session::GhostSession;
use std::io::{BufRead, BufReader, Write};
use std::time::{Duration, Instant};

const PAGE: &str = "<!doctype html><title>Loaded</title><p>slow page</p>";

/// Serve PAGE on loopback, answering each request after `delay`.
fn slow_page(delay: Duration) -> String {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let _ = conn.set_read_timeout(Some(Duration::from_secs(2)));
                let mut reader = BufReader::new(&conn);
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 0) && !line.trim_end().is_empty() {
                    line.clear();
                }
                std::thread::sleep(delay);
                let mut out = &conn;
                let _ = write!(
                    out,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{PAGE}",
                    PAGE.len()
                );
            });
        }
    });
    format!("http://127.0.0.1:{port}/")
}

/// A port nothing listens on.
fn closed_port_url() -> String {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    format!("http://127.0.0.1:{port}/")
}

/// The browser's tab count once it settles at `want` (a closed target can
/// linger briefly after Target.closeTarget answers), or the last count seen.
async fn tabs_settle(s: &GhostSession, want: usize) -> usize {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let n = s.browser_tabs("tabwait").await.map(|t| t.len()).unwrap_or(usize::MAX);
        if n == want || Instant::now() > until {
            return n;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

struct Seen {
    title: Option<String>,
    landed: String,
    refused: Result<(), String>,
    refused_tabs: usize,
    slow: Result<(), String>,
    slow_took: Duration,
    slow_tabs: usize,
    before: usize,
}

async fn exercise(s: &GhostSession) -> Result<Seen, String> {
    let e = |e: ghost_session::GhostError| e.to_string();
    let (tab, _note, landed) = s.tab_open_loaded("tabwait", &slow_page(Duration::from_millis(800)), 15_000).await.map_err(e)?;
    let t = s.tab("tabwait", &tab).await.map_err(e)?;
    let title = t.eval("document.title").await.map_err(|e| e.to_string())?.as_str().map(String::from);

    let before = s.browser_tabs("tabwait").await.map_err(e)?.len();
    // A load that fails closes the tab it opened instead of leaving it behind.
    let refused = s.tab_open_loaded("tabwait", &closed_port_url(), 10_000).await.map(|_| ()).map_err(e);
    let refused_tabs = tabs_settle(s, before).await;
    // timeout_ms bounds the whole navigation, the wait for the reply included.
    let started = Instant::now();
    let slow = s.tab_open_loaded("tabwait", &slow_page(Duration::from_secs(4)), 800).await.map(|_| ()).map_err(e);
    let slow_took = started.elapsed();
    let slow_tabs = tabs_settle(s, before).await;
    Ok(Seen { title, landed, refused, refused_tabs, slow, slow_took, slow_tabs, before })
}

#[tokio::test]
async fn tab_open_returns_once_the_page_has_loaded() {
    let s = GhostSession::new().expect("session");
    if let Err(e) = s.browser_launch_with("tabwait", "headless", None).await {
        // CI sets GHOST_REQUIRE_BROWSER=1 so this cannot pass by skipping there.
        assert!(
            std::env::var("GHOST_REQUIRE_BROWSER").as_deref() != Ok("1"),
            "GHOST_REQUIRE_BROWSER=1 but no browser could be launched: {e}"
        );
        eprintln!("skipped: no browser could be launched ({e})");
        return;
    }
    let seen = exercise(&s).await;
    let _ = s.browser_close("tabwait").await;
    let seen = seen.expect("the open of a reachable page failed");

    assert_eq!(seen.title.as_deref(), Some("Loaded"), "read right after the open (landed on {})", seen.landed);
    assert!(seen.landed.starts_with("http://127.0.0.1:"), "{}", seen.landed);

    let e = seen.refused.expect_err("a page that cannot load must be an error");
    assert!(e.contains("the tab it opened was closed"), "{e}");
    assert_eq!(seen.refused_tabs, seen.before, "the failed open left its tab behind");

    let e = seen.slow.expect_err("a page slower than timeout_ms must be an error");
    assert!(e.contains("did not load within 800 ms"), "{e}");
    assert!(seen.slow_took < Duration::from_millis(3500), "timeout_ms 800 took {:?}", seen.slow_took);
    assert_eq!(seen.slow_tabs, seen.before, "the timed-out open left its tab behind");
}
