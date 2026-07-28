# Verifying Ghost's macOS backend on a Mac

Ghost's macOS backend is written. It has never been run.

That distinction is the whole reason this page exists. The code in
`crates/ghost-platform/src/macos/` is a real native engine — Accessibility for
element discovery and read-back, `CGEvent` for keyboard and mouse, CoreGraphics for
window capture, `NSPasteboard` for the clipboard — and CI compiles and links it
against Apple's SDK on every push. That proves the FFI declarations are well-formed.
It proves nothing about whether TextEdit actually receives a keystroke.

So `capabilities_for(Platform::MacOS).functional` is `false`, and
[`docs/cross-platform.md`](cross-platform.md) still calls macOS a scaffold. Both stay
that way until somebody runs the command below on real hardware.

## If you have a Mac: the entire protocol

```
cargo build -p ghost-cli --release
./target/release/ghost doctor --mac
```

Then send back the JSON it prints. That's it. You do not need to read any Ghost
source, know Rust, or interpret a stack trace.

Budget about five minutes, most of it spent on two macOS permission dialogs.

### What it will do to your machine

- Ask for **Accessibility** and **Screen Recording** permission. Both are required:
  without the first, every element lookup and keystroke fails; without the second,
  screenshots come back as valid all-black images rather than as errors.
- Open **TextEdit**, type `hello ghost` into a document, use the File > New menu,
  screenshot the window, copy the text to your clipboard, and quit TextEdit
  discarding the document.

It overwrites your clipboard. It does not touch any file of yours, install
anything, or talk to the network.

### The permission dialogs

macOS ties both grants to the *specific binary*, identified by its code signature
and path. Two consequences that surprise everyone:

- **Rebuilding `ghost` invalidates the grants.** A fresh `cargo build` produces a
  binary macOS considers different, so you will be asked again.
- **A stale entry looks like a granted one.** If System Settings shows `ghost`
  already enabled but the command still reports a permission failure, macOS is
  holding the entry for an older build. Select it, press the minus button to remove
  it, and run the command again.

The command prompts for each grant and then polls for 60 seconds. If you need longer
to find the right pane, let it time out and re-run — nothing is lost.

## Reading the result

Exit code `0` means every capability that was tested passed. Non-zero means at least
one did not, and the report says which.

The JSON goes to stdout and to `~/.ghost/doctor-mac-<unix-time>.json`. One object per
capability:

```json
{
  "capability": "type text",
  "target_app": "TextEdit",
  "expected": "value == \"hello ghost\"",
  "observed": "hello ghost",
  "result": "PASS",
  "ms": 812
}
```

| `result` | Meaning |
| --- | --- |
| `PASS` | Observed what was expected. |
| `FAIL` | Did not. `error` carries the reason when one was available. |
| `SKIP` | Not implemented on macOS by design. Does not affect the exit code. |
| `UNKNOWN` | Ran, but could not decide. Counts as a failure — an unverified capability is exactly what this command exists to eliminate. |

Every step is independent and timed. A failure in one does not stop the others, so a
single run reports as much as possible about a machine the maintainers cannot log
into. The one exception is launching TextEdit: if that fails, every later step would
report the same thing, so the run stops there.

### Capabilities exercised

Element discovery, element location by role, typing with read-back, menu invocation
by tree walk, window screenshot with a Retina scale factor, clipboard copy, window
enumeration, application focus, and quitting an app.

### Deliberately not exercised

`Feature::BackgroundDispatch` — driving an app *without* taking focus — is reported
as `SKIP`. On Windows this is posted window messages (`BM_CLICK`, `WM_SETTEXT`),
which reach a control without activating its window. macOS has no equivalent:
`CGEvent` posts to the session-wide queue and goes to whatever is focused. It is
plausible that `AXUIElementPerformAction` and `AXUIElementSetAttributeValue` do not
activate a window, but whether they do is up to each app's accessibility provider,
which makes it a measurement rather than an implementation. Ghost does not claim
capabilities it has not measured, so macOS declares every feature *except* this one.

## What happens with the report

A clean run is the evidence for flipping `functional` to `true` in
`crates/ghost-platform/src/lib.rs` and updating the macOS row in
`docs/cross-platform.md`. That change is a separate commit, made after the report
exists — not before, and not in the same pull request that added the backend.

A run with failures is more useful than no run at all: each row names the Apple API
that was called, what was expected, and what came back, which is usually enough to
fix the problem without a second Mac.

## For maintainers

- The command lives in `crates/ghost-cli/src/doctor_mac.rs`. Its unit tests run
  headless in CI under the `mac-headless-tests` feature and cover the scoring rules
  and the JSON key names, which are the contract with whoever reads the report.
- `ghost doctor --mac` on a non-Mac prints `doctor --mac requires macOS` and exits
  `2` — distinct from `1`, which plain `ghost doctor` uses for "this is the right
  host and something on it failed".
- Live tests that need a desktop are gated behind `GHOST_LIVE_MAC` and are never run
  by CI.
- The macOS backend cannot be fully type-checked from Linux. `cargo check
  --target aarch64-apple-darwin` works for `ghost-platform` alone; `ghost-session`
  and `ghost-cli` pull in `ring` (via `reqwest` → `rustls`), whose C build script
  needs a macOS SDK. The `Build (macOS)` CI job on `macos-latest` is the
  authoritative check.
