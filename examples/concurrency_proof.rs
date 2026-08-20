//! Live proof that several independent ghost processes run at once on one machine.
//!
//! Spawns N copies of itself as separate OS processes. Each launches its own isolated
//! browser, drives its own tabs, and captures its own screenshots - concurrently, with
//! no coordination between them and without any of them touching the desktop.
//!
//!     cargo run -p ghost-session --example concurrency_proof
//!     cargo run -p ghost-session --example concurrency_proof -- 5   # five processes

use ghost_browser::{Browser, LaunchMode, LaunchOptions};
use ghost_core::system::DesktopSnapshot;
use std::time::Instant;

const PAGE: &str = r#"<!doctype html><html><body>
<input id="f"><button id="b" onclick="document.getElementById('o').innerText='OK:'+f.value">go</button>
<div id="o">-</div></body></html>"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--worker") {
        let id = args.get(pos + 1).cloned().unwrap_or_else(|| "0".into());
        return worker(&id).await;
    }

    let n: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(3);
    let exe = std::env::current_exe()?;

    let before = DesktopSnapshot::capture();
    println!("spawning {n} independent ghost processes");
    println!("foreground before: '{}'", before.foreground_title);
    let t0 = Instant::now();

    let children: Vec<_> = (0..n)
        .map(|i| {
            std::process::Command::new(&exe)
                .arg("--worker")
                .arg(i.to_string())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("spawn worker")
        })
        .collect();

    let mut passed = 0;
    let mut ports: Vec<String> = Vec::new();
    for child in children {
        let out = child.wait_with_output()?;
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            println!("  {line}");
            if line.contains("worker-ok") {
                passed += 1;
                if let Some(p) = line.split("port=").nth(1) {
                    ports.push(p.split_whitespace().next().unwrap_or("?").to_string());
                }
            }
        }
    }

    let delta = before.delta_now();
    println!("\n{passed}/{n} processes completed in {:?}", t0.elapsed());
    println!("DevTools ports used: {ports:?}");
    println!("desktop after: {}", delta.describe());

    let distinct: std::collections::HashSet<_> = ports.iter().collect();
    let mut failures = Vec::new();
    if passed != n {
        failures.push(format!("{}/{n} workers failed", n - passed));
    }
    if distinct.len() != ports.len() {
        failures.push(format!("ports collided across processes: {ports:?}"));
    }
    if delta.foreground_changed {
        failures.push(format!("foreground was stolen: {}", delta.describe()));
    }

    if failures.is_empty() {
        println!("PASS: {n} concurrent ghost processes, isolated ports, foreground untouched");
        Ok(())
    } else {
        for f in &failures {
            eprintln!("FAIL: {f}");
        }
        std::process::exit(1);
    }
}

async fn worker(id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut page = std::env::temp_dir();
    page.push(format!("ghost_conc_{id}.html"));
    std::fs::write(&page, PAGE)?;
    let url = format!("file:///{}", page.display().to_string().replace('\\', "/"));

    // Each worker gets its own profile directory. Sharing one would make Chrome's
    // profile lock serialize the workers, or fail them outright.
    let mut profile = std::env::temp_dir();
    profile.push("ghost-browser-profiles");
    profile.push(format!("conc-{}-{}", std::process::id(), id));

    let browser = Browser::launch(&LaunchOptions {
        mode: LaunchMode::Headless,
        user_data_dir: profile,
        ..Default::default()
    })
    .await?;

    let mut ok = true;
    for t in 0..2 {
        let label = format!("w{id}t{t}");
        let tab = browser.new_tab(&url).await?;
        tab.wait_for_load(20_000).await?;
        tab.type_text("#f", &label, true).await?;
        tab.click("#b", 5_000).await?;
        let got = tab.text("#o").await?;
        // Each tab must read back exactly its own label. A mismatch would mean two
        // processes or two tabs crossed wires.
        if got != format!("OK:{label}") {
            ok = false;
            println!("worker-{id} tab {t} MISMATCH: {got:?}");
        }
        if tab.screenshot(false).await?.len() < 500 {
            ok = false;
            println!("worker-{id} tab {t} screenshot too small");
        }
    }

    let port = browser.port();
    browser.close().await?;
    if ok {
        println!("worker-ok id={id} port={port} pid={} tabs=2", std::process::id());
        Ok(())
    } else {
        eprintln!("worker-{id} failed");
        std::process::exit(1);
    }
}
