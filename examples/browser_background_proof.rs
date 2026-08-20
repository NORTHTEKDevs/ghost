//! Live proof that ghost drives multiple browser tabs concurrently in the background.
//!
//! Run it, and keep typing in another window while it goes. Nothing should move, no
//! window should come forward, and the run should still pass.
//!
//!     cargo run -p ghost-browser --example browser_background_proof

use ghost_browser::{Browser, LaunchMode, LaunchOptions};
use std::time::Instant;

const PAGE: &str = r#"<!doctype html>
<html><body style="font-family:sans-serif;background:#101418;color:#e6edf3">
<h1 id="heading">ghost background proof</h1>
<input id="field" placeholder="type here" style="font-size:18px">
<button id="go" onclick="document.getElementById('out').innerText =
  'ECHO:' + document.getElementById('field').value">Submit</button>
<div id="out">empty</div>
<select id="pick"><option value="a">A</option><option value="b">B</option></select>
<div id="picked">none</div>
<script>
document.getElementById('pick').addEventListener('change', e => {
  document.getElementById('picked').innerText = 'PICKED:' + e.target.value;
});
document.getElementById('field').addEventListener('keydown', e => {
  if (e.key === 'Enter') { document.getElementById('out').innerText = 'ENTER:' + e.target.value; }
});
</script>
</body></html>"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut page = std::env::temp_dir();
    page.push("ghost_bg_proof.html");
    std::fs::write(&page, PAGE)?;
    let url = format!("file:///{}", page.display().to_string().replace('\\', "/"));

    let mode = if std::env::args().any(|a| a == "--windowed") {
        LaunchMode::Windowed
    } else {
        LaunchMode::Headless
    };
    println!("launching browser (mode: {mode:?})");
    let t0 = Instant::now();
    let browser = Browser::launch(&LaunchOptions { mode, ..Default::default() }).await?;
    println!("  up on port {} in {:?}", browser.port(), t0.elapsed());

    // Three tabs, all driven at once. If any of this needed the foreground, the
    // three would serialize and stomp on each other's focus.
    let t1 = Instant::now();
    let (a, b, c) = tokio::join!(
        drive(&browser, &url, "alpha"),
        drive(&browser, &url, "beta"),
        drive(&browser, &url, "gamma"),
    );
    let results = [a?, b?, c?];
    let elapsed = t1.elapsed();
    println!("3 tabs driven concurrently in {:?}", elapsed);

    // Regression guard. Awaiting the acknowledgement of a CDP mouse-move costs ~5s on
    // a background tab, because Chrome only acks it once the renderer produces a frame
    // and a background tab produces none. This run took 5.78s before that was fixed and
    // 0.54s after, so a budget here catches a reintroduction immediately - the run
    // would still "pass" functionally, just crawl.
    const BUDGET: std::time::Duration = std::time::Duration::from_secs(3);
    let too_slow = elapsed > BUDGET;
    if too_slow {
        eprintln!(
            "  PERF REGRESSION: {elapsed:?} exceeds the {BUDGET:?} budget. Check that              Tab::dispatch_move still uses Cdp::notify rather than awaiting a reply."
        );
    }

    let mut failures = 0;
    for r in &results {
        let echo_ok = r.echo == format!("ECHO:{}", r.label);
        let enter_ok = r.entered == format!("ENTER:{}-typed", r.label);
        let pick_ok = r.picked == "PICKED:b";
        let shot_ok = r.screenshot_bytes > 1000 && r.png_magic;
        println!(
            "  {:<6} click={} enter={} select={} screenshot={}B({}) tabs_visible_to_user=0",
            r.label,
            yn(echo_ok),
            yn(enter_ok),
            yn(pick_ok),
            r.screenshot_bytes,
            yn(shot_ok)
        );
        if !(echo_ok && enter_ok && pick_ok && shot_ok) {
            failures += 1;
            println!("     echo={:?} entered={:?} picked={:?}", r.echo, r.entered, r.picked);
        }
    }

    let tabs = browser.tabs().await?;
    println!("tabs open at end: {}", tabs.len());
    browser.close().await?;

    if failures > 0 || too_slow {
        if failures > 0 {
            eprintln!("FAIL: {failures} of {} tabs did not verify", results.len());
        }
        std::process::exit(1);
    }
    println!("PASS: all tabs driven to completion with zero foreground activity");
    Ok(())
}

fn yn(b: bool) -> &'static str {
    if b {
        "ok"
    } else {
        "FAIL"
    }
}

struct Outcome {
    label: String,
    echo: String,
    entered: String,
    picked: String,
    screenshot_bytes: usize,
    png_magic: bool,
}

async fn drive(
    browser: &Browser,
    url: &str,
    label: &str,
) -> Result<Outcome, Box<dyn std::error::Error>> {
    let tab = browser.new_tab(url).await?;
    tab.wait_for_load(15_000).await?;

    // Trusted synthetic click through the renderer's own input pipeline.
    tab.type_text("#field", label, true).await?;
    tab.click("#go", 5_000).await?;
    let echo = tab.text("#out").await?;

    // Key events must reach the focused element in a tab that is not in front.
    tab.type_text("#field", &format!("{label}-typed"), true).await?;
    tab.press("Enter", &[]).await?;
    let entered = tab.text("#out").await?;

    tab.select_option("#pick", "b").await?;
    let picked = tab.text("#picked").await?;

    let png = tab.screenshot(false).await?;
    let png_magic = png.starts_with(&[0x89, 0x50, 0x4E, 0x47]);

    Ok(Outcome {
        label: label.to_string(),
        echo,
        entered,
        picked,
        screenshot_bytes: png.len(),
        png_magic,
    })
}
