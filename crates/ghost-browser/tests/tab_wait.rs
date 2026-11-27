//! Tab waits against a real headless browser, on every platform.
//!
//! Needs a Chromium-family browser: `GHOST_BROWSER_PATH`, an installed
//! Chrome/Edge/Brave/Comet on Windows, or `google-chrome`/`chromium` on PATH.
//! With none it prints why and passes, unless `GHOST_REQUIRE_BROWSER=1` (set
//! in CI) turns a missing browser into a failure. The browser is headless and
//! launched by the test, so it never opens a window.

use ghost_browser::launch::find_browser;
use ghost_browser::{Browser, BrowserError, LaunchMode, LaunchOptions};
use std::path::PathBuf;

fn browser_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"]
        .iter()
        .flat_map(|name| std::env::split_paths(&path).map(move |dir| dir.join(name)))
        .find(|p| p.is_file())
}

fn browser() -> Option<PathBuf> {
    let found = find_browser().ok().or_else(browser_on_path);
    if found.is_none() {
        assert!(
            std::env::var("GHOST_REQUIRE_BROWSER").as_deref() != Ok("1"),
            "GHOST_REQUIRE_BROWSER=1 but no Chromium-family browser was found"
        );
        eprintln!("skipped: no Chromium-family browser (set GHOST_BROWSER_PATH)");
    }
    found
}

// A page whose element appears 300 ms after load, so the wait has to poll.
const LATE_PAGE: &str = "data:text/html,<title>late</title><script>\
setTimeout(()=>{const d=document.createElement('div');d.id='late';document.body.appendChild(d)},300)\
</script><body></body>";

#[tokio::test]
async fn tab_waits_resolve_and_time_out_in_a_headless_browser() {
    let Some(exe) = browser() else { return };
    let profile = std::env::temp_dir().join(format!("ghost-tab-wait-{}", std::process::id()));
    let opts = LaunchOptions {
        mode: LaunchMode::Headless,
        user_data_dir: profile.clone(),
        executable: Some(exe),
        ..LaunchOptions::default()
    };
    let browser = Browser::launch(&opts).await.expect("launch headless browser");

    let outcome = async {
        let tab = browser.new_tab(LATE_PAGE).await?;
        tab.wait_for_load(10_000).await?;
        tab.wait_for_selector("#late", 10_000).await?;
        let started = std::time::Instant::now();
        let missing = tab.wait_for_selector("#never", 300).await;
        Ok::<_, BrowserError>((missing, started.elapsed()))
    }
    .await;

    let _ = browser.close().await;
    // The browser holds its profile files for a moment after Browser.close.
    for _ in 0..20 {
        if std::fs::remove_dir_all(&profile).is_ok() || !profile.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    let (missing, waited) = outcome.expect("load and the late element resolve");
    assert!(
        matches!(missing, Err(BrowserError::SelectorNotFound { ms: 300, .. })),
        "a selector that never matches times out: {missing:?}"
    );
    assert!(waited.as_millis() >= 300, "it waited the full timeout: {waited:?}");
}
