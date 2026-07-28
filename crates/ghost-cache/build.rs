// Build guard: Ghost's engine is Windows-only today. Failing here gives a single
// readable sentence on macOS/Linux instead of hundreds of Win32 FFI errors.
// See docs/cross-platform.md and docs/plans/2026-07-cross-platform-plan.md.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        eprintln!("Ghost's ghost-cache is Windows-only today; see docs/cross-platform.md");
        std::process::exit(1);
    }
}
