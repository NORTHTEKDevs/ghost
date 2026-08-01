//! Windows-only example: drives Notepad through the Windows engine.
//! `ghost-core` compiles to nothing off Windows, so the body is gated and a
//! stub `main` keeps the target buildable everywhere.

#[cfg(windows)]
mod win {
//! Example: Open Notepad and type a message
//! Run: cargo run --example notepad_hello

use ghost_session::{GhostSession, By, session::Region};
use std::time::Duration;

#[tokio::main]
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
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

}

fn main() {
    #[cfg(windows)]
    win::run();
}
