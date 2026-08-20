//! Which screenshot strategy is actually fastest on a background tab?
use ghost_browser::{Browser, LaunchMode, LaunchOptions};
use serde_json::json;
use std::time::Instant;

async fn time_shots(tab: &ghost_browser::Tab, label: &str, runs: u32) {
    let _ = tab.raw("Page.captureScreenshot", json!({"format":"png"})).await;
    let t = Instant::now();
    for _ in 0..runs {
        let _ = tab.raw("Page.captureScreenshot", json!({"format":"png"})).await;
    }
    println!("  {label:<40} {:>9.2?}", t.elapsed() / runs);
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rows: String = (0..300).map(|i| format!("<li>row {i}</li>")).collect();
    let page = std::env::temp_dir().join("ghost_shot_probe.html");
    std::fs::write(&page, format!("<!doctype html><body><ul>{rows}</ul></body>"))?;
    let url = format!("file:///{}", page.display().to_string().replace('\\', "/"));

    let browser = Browser::launch(&LaunchOptions { mode: LaunchMode::Headless, ..Default::default() }).await?;
    let tab = browser.new_tab(&url).await?;
    tab.wait_for_load(20_000).await?;

    // "no override" is skipped: on a background tab captureScreenshot never returns
    // and every run costs the full 30s call timeout. That is the bug the override fixes.

    tab.raw("Emulation.setDeviceMetricsOverride",
        json!({"width":1280,"height":900,"deviceScaleFactor":1,"mobile":false})).await?;
    time_shots(&tab, "override left in place", 4).await;

    tab.raw("Emulation.clearDeviceMetricsOverride", json!({})).await?;
    time_shots(&tab, "after clearing override", 4).await;

    // set + capture + clear, as one operation
    let t = Instant::now();
    for _ in 0..4 {
        let _ = tab.raw("Emulation.setDeviceMetricsOverride",
            json!({"width":1280,"height":900,"deviceScaleFactor":1,"mobile":false})).await;
        let _ = tab.raw("Page.captureScreenshot", json!({"format":"png"})).await;
        let _ = tab.raw("Emulation.clearDeviceMetricsOverride", json!({})).await;
    }
    println!("  {:<40} {:>9.2?}", "set + capture + clear each time", t.elapsed() / 4);

    // bringToFront once, then plain captures
    tab.raw("Page.bringToFront", json!({})).await?;
    time_shots(&tab, "after Page.bringToFront, no override", 4).await;

    browser.close().await?;
    let _ = std::fs::remove_file(&page);
    Ok(())
}
