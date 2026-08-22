# Ghost on macOS

**Status (21 Aug 2026): the known blocker is removed; the first real build on
a Mac is the remaining test.** `crates/ghost-macos` now provides the engine
`ghost-session` was missing, and `engine.rs` has a macOS arm. Verified from
Windows: `cargo check --target aarch64-apple-darwin -p ghost-macos` and its
`--all-targets` API-surface proof both pass, and `cargo check --workspace` on
Windows is unbroken. What could NOT be verified here is `ghost-session` itself
compiling for macOS, because cross-compiling the full workspace needs a
macOS-targeting `cc` for `ring`/`blake3` that this machine does not have --
so **run `cargo build --workspace` on the Mac and report what happens.**

Expect: browser automation (CDP) and `ghost_shell` to work, because they are
architecturally platform-neutral. Expect screen automation NOT to work --
element discovery, click, type, screenshot and OCR all return an explicit
"unsupported on macOS" error rather than pretending, and that is the
multi-week AXUIElement/CGEvent work described below.

---

### Original analysis (kept because it is still the accurate map)

**Status when this was written: does not build yet.** This is the honest starting point, not a
gap to paper over. `ghost-platform` (the cross-platform contract) and the
pure-Rust support crates compile cleanly for macOS today; `ghost-session`,
`ghost-cli` (`ghost`), `ghost-mcp` and `ghost-http` do not, for one precise,
well-understood reason documented below. There is no native macOS engine
(Accessibility / CGEvent / ScreenCaptureKit) - see the implementation map in
`crates/ghost-platform/src/macos.rs` and `docs/cross-platform.md`. Building
that engine is real, multi-week Objective-C FFI work and is **not** what this
document is about. This document is about getting the *rest* of the repo - 
everything that doesn't need AXUIElement - building and reporting itself
honestly.

Everything in this file was produced from a Windows machine with no macOS
access, verified as far as that allows. Anything that needs a real Mac to
confirm is marked **UNVERIFIED**, with what would prove it.

---

## 1. Prerequisites

- **Rust toolchain**: `rustup` with a recent stable toolchain (`rustc
  --version` should be 1.75+; the workspace uses edition 2021 and
  `div_ceil`/similar recent stdlib additions). Install via
  [rustup.rs](https://rustup.rs) if you don't have it. **UNVERIFIED** exact
  minimum version - if `cargo build` fails on a syntax/stdlib-API error
  unrelated to anything below, `rustup update` first.
- **Xcode Command Line Tools**: `xcode-select --install`. Needed for the
  system linker/`cc`, and for `blake3` and `ring` (transitive, via
  `ghost-core`/`ghost-cache` and `reqwest`+`rustls`) - both have native
  build scripts that shell out to a C compiler for SIMD/asm detection. On
  Windows, cross-compiling this repo without a macOS SDK fails at exactly
  this step (`error: failed to run custom build command for 'ring'` /
  `'blake3'`, `ToolNotFound: failed to find tool "cc"`) - that's an
  environment limitation of cross-compiling from Windows, not a Mac-specific
  problem; with Xcode CLT installed, native `cc` is present and this should
  be a non-issue. **UNVERIFIED on real hardware.**
- No Android/iOS SDK, no Homebrew packages, are required for anything that
  currently compiles.

## 2. The build command

```bash
git clone https://github.com/NORTHTEKDevs/ghost
cd ghost
cargo build --workspace
```

### What this is expected to do today

**Succeeds** (verified via `cargo check --target aarch64-apple-darwin`,
cross-compiled from Windows - real compiler output, not a guess):

- `ghost-platform` - the `Feature`/`Capabilities`/`Backend` contract, plus
  `crates/ghost-platform/src/macos.rs`, an inert scaffold `Backend` impl
  that reports `functional: false` and an empty `supported` list. This is
  the crate `ghost --help`-style capability reporting will eventually read
  from once the binaries build.
- `ghost-core` - compiles to an **empty** crate on macOS by design
  (`#![cfg(windows)]` at `crates/ghost-core/src/lib.rs:6`). Its own tests,
  benches, and Win32 code simply don't exist in a macOS build.
- `ghost-linux` - same idea: its Linux-only surface is gated under
  `[target.'cfg(target_os = "linux")'.dependencies]` in
  `crates/ghost-linux/Cargo.toml`, and compiles to (nearly) nothing off
  Linux.
- `ghost-cache`, `ghost-intent`, `ghost-ground` - pure Rust, no OS-gated
  code, build unchanged.
- `ghost-browser` - CDP-based Chrome/Edge/Comet automation. Its one
  Windows-only block (default browser discovery via the registry) is
  correctly gated at `crates/ghost-browser/src/launch.rs:290`
  (`#[cfg(windows)]`); the rest - WebSocket, JSON-RPC over CDP - is
  platform-neutral pure Rust.

**Fails** - `ghost-session`, and therefore everything that depends on it
(`ghost-cli`/`ghost`, `ghost-mcp`, `ghost-http`):

```
error[E0433]: failed to resolve: could not find `system` in `engine`
  --> crates/ghost-session/src/session.rs:...
```
(and dozens more like it, plus `error[E0412]: cannot find type 'CoreError' in module 'crate::engine::error'`
from `crates/ghost-session/src/error.rs:22`)

See section 3 for exactly why, with file:line citations, and section 5 for
what finishing this looks like.

## 3. Root cause

`ghost-session` is the platform-neutral orchestration layer (locator tiers,
grounding cascade, act-then-verify) that sits on top of *an engine* - see
`crates/ghost-session/src/engine.rs`:

```rust
#[cfg(windows)]
pub use ghost_core::*;

#[cfg(target_os = "linux")]
pub use ghost_linux::*;
```

This is a clean, intentional design: `ghost-core` (Windows: Win32 UIA,
SendInput, DXGI) and `ghost-linux` (AT-SPI2, XTEST, X11) both expose the
*same* module tree - `uia`, `input`, `capture`, `system`, `process`, `ocr`,
`error` - so all of `ghost-session`'s logic is written once and just calls
`crate::engine::whatever`.

**On macOS, neither `#[cfg]` arm matches, so `engine` compiles to an empty
module** - but nothing else in the crate knows that. `ghost-session` calls
into `crate::engine::*` unconditionally, dozens of times, with no
macOS-specific branch anywhere:

- `crates/ghost-session/src/session.rs` - the bulk of `GhostSession`'s
  methods; ~70 unconditional call sites, e.g. line 226
  (`crate::engine::system::foreground_window()`), line 565
  (`crate::engine::input::keyboard::type_text`), line 1014
  (`Vec<crate::engine::uia::ElementDescriptor>` as a return type).
- `crates/ghost-session/src/registries.rs` - `focus_policy`/
  `set_focus_policy`/`desktop_*` methods call `ghost_core::focus`/
  `ghost_core::DesktopSession` directly (not even through `engine::`), lines
  47, 57, 61, 211, 232. These particular ones *are* `#[cfg(windows)]`-gated
  already, so they're not the blocker - they're listed here because they'd
  be the next thing to look at once the rest compiles.
- `crates/ghost-session/src/error.rs:22` - 
  `Core(#[from] crate::engine::error::CoreError)`. `CoreError` itself
  doesn't exist outside windows/linux, so `GhostError` - the crate's own
  error type - fails to compile on macOS.
- `crates/ghost-session/src/tiers.rs` and `element.rs` - same pattern,
  smaller surface (`UiaTree`, `role_id_to_name`, `is_editable_role`).
- `crates/ghost-session/src/shell.rs` - smallest coupling of the five: only
  two call sites, both to `crate::engine::input::hotkey::is_stopped()`
  (checking the emergency-stop flag before running a shell command). See
  section 4 - this one is a real, low-effort candidate to decouple.

Downstream, `crates/ghost-cli/src/main.rs` and `crates/ghost-mcp/src/main.rs`
each called `crate::engine::system::dpi::ensure_process_dpi_aware()`
unconditionally in `main()` (originally lines 206 and 332) - that specific
call **has been fixed** in this pass (now gated
`#[cfg(any(windows, target_os = "linux"))]`; it was a real, if minor,
compile blocker on its own). It does not by itself make either binary
compile, because both files still do `use ghost_session::{GhostSession, ...}`
unconditionally and dispatch every CLI/MCP command through a live
`GhostSession`, which doesn't exist as a working type on macOS today.

One more wrinkle worth knowing about before attempting a fix: some of
`ghost-core`'s public signatures leak actual `windows`-crate types across the
engine boundary - e.g. `crates/ghost-core/src/input/keyboard.rs`'s
`key_down`/`key_up`/`press_key` take a `VIRTUAL_KEY` (from
`windows::Win32::UI::Input::KeyboardAndMouse`), and `session.rs` calls
`vk.0` on the result of `name_to_vk()` directly. A macOS engine - even a
stub one - can't reuse that type (the `windows` crate isn't available off
Windows), so it needs its own type of the same shape. This is exactly the
kind of thing that makes "just write empty stubs" bigger than it sounds:
it's not one `cfg` gate, it's mirroring roughly a dozen structs/enums
(`ElementDescriptor`, `WindowInfo`, `Verification`, `CaptureFormat`,
`EditCommand`, `CoreError` with all ~16 of its variants, a `VIRTUAL_KEY`
lookalike, ...) and ~50 function signatures across `uia`, `input`, `capture`,
`system`, `process`, `ocr`. Doable, but it's the actual size of the
remaining work - not a one-line fix - which is why this pass documents it
precisely instead of shipping a rushed, partially-wrong version of it that
nobody could verify from Windows.

## 4. What works today with no accessibility backend at all

Investigated honestly, per the architecture as it stands **today**:

| Feature | Platform-neutral? | Why it doesn't run on macOS yet anyway |
|---|---|---|
| `ghost_browser_*` (CDP browser automation) | **Yes** - WebSocket + JSON-RPC over Chrome DevTools Protocol, `crates/ghost-browser`, no Windows/AT-SPI dependency except default-browser-path lookup (gated) | Reachable only through `GhostSession`, which fails to construct/compile (section 3) |
| `ghost_shell` (one-shot + persistent PowerShell/bash/zsh/pwsh) | **Nearly** - spawns `bash`/`pwsh`/`sh` via `tokio::process::Command`, no OS-specific API, except 2 calls to `crate::engine::input::hotkey::is_stopped()` (`crates/ghost-session/src/shell.rs:515,542`) to check the emergency-stop flag | Same as above, plus that one small coupling |
| `ghost_http_get`/`ghost_http_post` (agent-facing outbound HTTP tools) | **Yes** - plain `reqwest` calls in `ghost-mcp`, no engine dependency in the handler body itself | Every tool handler takes `&GhostSession` as a parameter, so it's still gated on `GhostSession` existing |
| Element discovery, click/type, screenshot, window management, clipboard, OCR | **No** | These are exactly what AXUIElement / CGEvent / ScreenCaptureKit are for - the excluded work |

The honest summary: **nothing works on macOS today**, but that's an artifact
of architecture (one `GhostSession` struct gates everything, including
features that have no real dependency on accessibility APIs), not of
necessity. Browser automation and shell exec are the two lowest-effort wins
if someone wants partial macOS functionality *before* a real AX/CGEvent
engine exists - they'd need `GhostSession` to be constructible on macOS
(even in a "no desktop automation" mode) and the `is_stopped()` call in
`shell.rs` to have a macOS fallback (trivially `false` - no emergency-stop
hotkey on macOS yet, same honest gap already documented for Wayland in
`docs/linux-fedora.md`).

## 5. What finishing this looks like (recipe, not done here)

Two independent tracks, either can proceed alone:

**Track A - make it compile (no new capability, but honest and buildable).**
Add a macOS arm to `ghost_session::engine` (`crates/ghost-session/src/engine.rs`)
backed by a new stub module that mirrors `ghost-core`'s public surface - 
same struct/enum shapes, every function returning
`Err(CoreError::Unsupported)` (or equivalent) rather than doing real work.
`GhostSession::new()` would then either construct successfully with a
non-functional engine (unlocking browser/shell per section 4) or fail fast
with one clear message, and every other method compiles unchanged because
`ghost-session`'s logic never needed to know *which* engine it's calling - 
that's the whole point of the existing `engine::` indirection. This is
mechanical but not small: see the type list in section 3. **The critical
enabler that makes this verifiable without a Mac**: `cargo check --target
aarch64-apple-darwin` genuinely type-checks pure-Rust code on Windows (no
linker needed) - confirmed by getting a clean pass on `ghost-platform`
during this session. It cannot get past `ghost-core`/`ghost-cache`/
`reqwest`'s native build scripts (`ring`, `blake3` need a real `cc` for the
target) without Xcode CLT, so full-workspace verification still needs a Mac
in the loop eventually - but the type-level correctness of a stub engine
can be iterated on and mostly proven from Windows first.

**Track B - the real native backend.** `crates/ghost-platform/src/macos.rs`
already has the implementation map (AXUIElement for discovery/act,
`AXUIElementSetAttributeValue`/CGEvent for type, ScreenCaptureKit for
capture, `CGEventCreateKeyboardEvent` for key input). This is the
multi-week track explicitly out of scope here.

Either way, `capabilities_for(Platform::MacOS)` in
`crates/ghost-platform/src/lib.rs` and `ghost doctor`'s macOS fallback in
`crates/ghost-cli/src/doctor.rs:394-401` are **already correct and need no
change** - they already report `functional: false` / an empty feature list /
"macOS is not implemented" rather than claiming anything. The honesty layer
is ready; it's the compile layer underneath it that's missing.

## 6. Reporting a build failure usefully

If `cargo build --workspace` fails differently than section 2 describes
(especially: if it fails on something *other* than `ghost-session`/
`ghost-cli`/`ghost-mcp`/`ghost-http`, or if the errors inside those crates
don't match section 3), that's new information - this document was written
without a Mac, so it's possible the Windows-side cross-check missed
something real macOS-only tooling would catch. Please include:

1. `rustc --version --verbose` and `cargo --version`.
2. `xcode-select -p` (confirms CLT is actually installed and where).
3. The **full** `cargo build --workspace 2>&1` output, not a snippet - 
   errors from one crate can be caused by an earlier one.
4. Which crate the first `error[...]` (not `warning`) is in.

Everything in sections 2-4 above is labeled by how it was established:
static reading of the source (file:line cited) plus a real
`cargo check --target aarch64-apple-darwin` cross-compile from Windows for
the crates that don't need a native `cc`. Nothing here was verified by
actually running a macOS binary - that step is still open.
