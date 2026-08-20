//! Which part of a CDP click stalls?
use ghost_browser::{Browser, LaunchMode, LaunchOptions};
use serde_json::json;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let page = std::env::temp_dir().join("ghost_click_probe.html");
    std::fs::write(&page, "<!doctype html><body><button id=b>go</button></body>")?;
    let url = format!("file:///{}", page.display().to_string().replace('\\', "/"));

    let browser = Browser::launch(&LaunchOptions { mode: LaunchMode::Headless, ..Default::default() }).await?;
    let tab = browser.new_tab(&url).await?;
    tab.wait_for_load(20_000).await?;
    let (x, y) = tab.element_center("#b").await?;

    for label in ["mouseMoved", "mousePressed", "mouseReleased"] {
        let mut ev = json!({"x": x, "y": y, "type": label, "clickCount": 1});
        ev["button"] = json!(if label == "mouseMoved" { "none" } else { "left" });
        ev["buttons"] = json!(if label == "mousePressed" { 1 } else { 0 });
        for i in 0..3 {
            let t = Instant::now();
            tab.raw("Input.dispatchMouseEvent", ev.clone()).await?;
            println!("{label:<15} run{i}  {:>9.2?}", t.elapsed());
        }
    }

    println!("\n--- fire-and-forget move, awaited press (ordering preserved?) ---");
    for i in 0..5 {
        let t = Instant::now();
        tab.raw_notify(
            "Input.dispatchMouseEvent",
            json!({"x": x, "y": y, "type": "mouseMoved", "button": "none", "buttons": 0}),
        )?;
        tab.raw(
            "Input.dispatchMouseEvent",
            json!({"x": x, "y": y, "type": "mousePressed", "button": "left", "buttons": 1, "clickCount": 1}),
        )
        .await?;
        tab.raw(
            "Input.dispatchMouseEvent",
            json!({"x": x, "y": y, "type": "mouseReleased", "button": "left", "buttons": 0, "clickCount": 1}),
        )
        .await?;
        println!("full click (move not awaited) run{i}  {:>9.2?}", t.elapsed());
    }

    println!("\n--- now with a device metrics override in place ---");
    tab.raw("Emulation.setDeviceMetricsOverride",
            json!({"width": 1280, "height": 900, "deviceScaleFactor": 1, "mobile": false})).await?;
    for label in ["mouseMoved", "mousePressed", "mouseReleased"] {
        let mut ev = json!({"x": x, "y": y, "type": label, "clickCount": 1});
        ev["button"] = json!(if label == "mouseMoved" { "none" } else { "left" });
        ev["buttons"] = json!(if label == "mousePressed" { 1 } else { 0 });
        for i in 0..3 {
            let t = Instant::now();
            tab.raw("Input.dispatchMouseEvent", ev.clone()).await?;
            println!("{label:<15} run{i}  {:>9.2?}", t.elapsed());
        }
    }

    browser.close().await?;
    let _ = std::fs::remove_file(&page);
    Ok(())
}
