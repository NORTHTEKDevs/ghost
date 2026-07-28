//! Example: Open Notepad and type a message
//! Run: cargo run --example notepad_hello

// UIA drives these examples, so they exist only on Windows. The stub `main` below
// keeps `cargo test -p ghost-session` compiling on a Mac, where cargo still builds
// every example target.
#[cfg(not(windows))]
fn main() {
    eprintln!("this example automates Notepad and is Windows-only");
}

#[cfg(windows)]
use ghost_session::{GhostSession, By, session::Region};
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Ghost - Desktop Automation Example");
    println!("Press Ctrl+Alt+G at any time to stop automation");

    let session = GhostSession::new()?;

    println!("Launching Notepad...");
    let pid = session.launch("notepad.exe").await?;
    tokio::time::sleep(Duration::from_millis(800)).await;

    println!("Finding text area...");
    let edit = session.find(By::role("edit")).await?;
    edit.type_text("Hello from Ghost! This text was typed by an AI agent.")?;

    println!("Taking screenshot...");
    let png = session.screenshot(Region::full()).await?;
    std::fs::write("ghost_example_screenshot.png", &png)?;
    println!("Screenshot saved to ghost_example_screenshot.png ({} bytes)", png.len());

    println!("Done! Cleaning up...");
    ghost_core::process::kill(pid)?;

    Ok(())
}
