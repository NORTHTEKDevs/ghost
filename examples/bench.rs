//! Where does the time actually go?
//!
//!     cargo run --release -p ghost-session --example bench
//!
//! Times each primitive against a real target so optimisation work is aimed at
//! measured hot paths rather than guesses.

use ghost_browser::{Browser, LaunchMode, LaunchOptions};
use ghost_session::{By, GhostSession};
use std::time::{Duration, Instant};

const PAGE: &str = r#"<!doctype html><html><body>
<input id="f"><button id="b" onclick="document.getElementById('o').innerText='ok'">go</button>
<div id="o">-</div>
<ul>__ROWS__</ul></body></html>"#;

fn bench<T>(label: &str, runs: u32, mut f: impl FnMut() -> T) -> Duration {
    // One warm-up: first calls pay COM proxy setup, D3D device creation, and JIT of
    // the provider side, none of which is representative of steady state.
    let _ = f();
    let start = Instant::now();
    for _ in 0..runs {
        let _ = f();
    }
    let per = start.elapsed() / runs;
    println!("  {label:<34} {:>9.2?}", per);
    per
}

async fn bench_async<F, Fut, T>(label: &str, runs: u32, mut f: F) -> Duration
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let _ = f().await;
    let start = Instant::now();
    for _ in 0..runs {
        let _ = f().await;
    }
    let per = start.elapsed() / runs;
    println!("  {label:<34} {:>9.2?}", per);
    per
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let t0 = Instant::now();
    let session = GhostSession::new()?;
    println!("session init                         {:>9.2?}\n", t0.elapsed());

    // ---------------- desktop / UIA ----------------
    let stem = format!("ghost_bench_{}", std::process::id());
    let scratch = std::env::temp_dir().join(format!("{stem}.txt"));
    std::fs::write(&scratch, "bench\n")?;
    session.launch(&format!("notepad.exe {}", scratch.display())).await?;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let target = session.window(&stem)?;
    println!("UIA / window ops (target: {})", target.title);

    bench("list_windows", 20, || ghost_core::uia::tree::list_windows());
    bench("window resolve by title", 20, || session.window(&stem));
    bench_async("find_in(window, role)", 10, || async {
        session.find_in(&target.title, By::role("document")).await
    })
    .await;
    bench_async("find(role) unscoped", 5, || async {
        session.find(By::role("document")).await
    })
    .await;
    bench_async("describe_screen(window)", 5, || async {
        session.describe_screen(Some(&target.title)).await
    })
    .await;
    bench_async("describe_screen(desktop)", 3, || async {
        session.describe_screen(None).await
    })
    .await;
    bench("capture_window", 10, || target.capture(false));
    bench("type_background", 10, || target.type_text("x"));

    // ---------------- browser / CDP ----------------
    let rows: String = (0..300)
        .map(|i| format!("<li id=\"row{i}\">row {i}</li>"))
        .collect();
    let page = std::env::temp_dir().join("ghost_bench.html");
    std::fs::write(&page, PAGE.replace("__ROWS__", &rows))?;
    let url = format!("file:///{}", page.display().to_string().replace('\\', "/"));

    let t = Instant::now();
    let browser = Browser::launch(&LaunchOptions { mode: LaunchMode::Headless, ..Default::default() }).await?;
    println!("\nbrowser launch                       {:>9.2?}", t.elapsed());

    let t = Instant::now();
    let tab = browser.new_tab(&url).await?;
    tab.wait_for_load(20_000).await?;
    println!("tab open + first load                {:>9.2?}\n", t.elapsed());
    println!("CDP ops");

    bench_async("eval (trivial)", 20, || async { tab.eval("1+1").await }).await;
    bench_async("wait_for_selector (present)", 20, || async {
        tab.wait_for_selector("#b", 5_000).await
    })
    .await;
    bench_async("element_center", 20, || async { tab.element_center("#b").await }).await;
    bench_async("click", 20, || async { tab.click("#b", 5_000).await }).await;
    bench_async("type_text", 20, || async {
        tab.type_text("#f", "abc", true).await
    })
    .await;
    bench_async("text(selector)", 20, || async { tab.text("#o").await }).await;
    bench_async("describe(120)", 10, || async { tab.describe(120).await }).await;
    bench_async("screenshot", 10, || async { tab.screenshot(false).await }).await;
    bench_async("navigate (same url)", 5, || async {
        tab.navigate(&url, 20_000).await
    })
    .await;

    browser.close().await?;
    let _ = ghost_core::process::kill(target.pid);
    let _ = std::fs::remove_file(&scratch);
    let _ = std::fs::remove_file(&page);
    println!("\ntotal wall clock {:.2?}", t0.elapsed());
    Ok(())
}
