# Linux CI clippy fix (2026-11-26)

CI run 45887039606 (cf18cd2) failed at "Clippy (workspace)" on Linux. The fixes:

- `Cover`, `cover_allowed`, `invoke_fallback_due` and `cover_refusal`
  (`crates/ghost-session/src/session.rs`) are used only by the Windows click_at
  path, and their unit tests run everywhere. They get
  `#[cfg_attr(not(windows), allow(dead_code))]` rather than `cfg(windows)`,
  which would remove them from the Linux tests.
- `changed_cells` (`crates/ghost-session/src/click_verify.rs`) iterates
  `as_chunks::<4>()` instead of `chunks_exact(4)`. Behaviour is identical: both
  ignore a trailing partial pixel, and the existing click_verify tests pass.

## What was checked here (mirroring `.github/workflows/linux.yml`)

Run on Windows with `rustup target add x86_64-unknown-linux-gnu` and
`--target x86_64-unknown-linux-gnu`. `ring`'s build script compiles C, and this
machine has no Linux gcc, so `CC_x86_64_unknown_linux_gnu` /
`AR_x86_64_unknown_linux_gnu` pointed at a local wrapper:
`zig cc -target x86_64-linux-gnu`, dropping the `--target=` flag `cc` adds,
which zig does not parse. Nothing in the repo depends on the wrapper.

| Command (with `--target x86_64-unknown-linux-gnu`) | Result |
|---|---|
| `cargo clippy -p ghost-linux --all-targets -- -D warnings` | exit 0 (`clippy-ghost-linux.txt`) |
| `cargo clippy --workspace -- -D warnings`, before the fix | exit 101: exactly CI's four dead-code errors at session.rs:74, 93, 103, 181 (`clippy-workspace-before-fix.txt`) |
| `cargo clippy --workspace -- -D warnings`, after the fix | exit 0 (`clippy-workspace.txt`) |

Not reproducible here: CI's two `chunks_exact` errors. Local clippy is 1.95
(2026-04-14), which does not have that lint, and the installed nightly has no
clippy component. CI's `dtolnay/rust-toolchain@stable` is newer. The change is
the `as_chunks` replacement the lint asks for; CI will confirm it.

Windows: `cargo test --workspace` 637 / 0 / 28 (`windows-test.txt`), clippy
`--all-targets -D warnings` exit 0 (`windows-clippy.txt`), lint 0.
