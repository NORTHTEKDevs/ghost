# Ghost cross-platform implementation plan — macOS + Linux

**Date:** 2026-07-27
**Owner:** Kristian / Northtek
**Status:** proposed (Pass 2 of the honesty + cross-platform work)
**Companions:** [`docs/cross-platform.md`](../cross-platform.md) (capability
contract — still current, cited here, not duplicated),
[`docs/audits/2026-07-honesty-audit.md`](../audits/2026-07-honesty-audit.md)
(surface-wording redlines).

> **Standing rule for everything below.** Ghost runs on **Windows 10/11 only**
> today. `capabilities_for(Platform::MacOS)` and `capabilities_for(Platform::Linux)`
> return `functional: false` and stay that way until the on-device verification
> checklist in §7 passes on real hardware. No document, README line, kit file,
> registry entry, or landing page may say otherwise before then. Roadmap wording
> ("planned", "in progress", "lands in v0.17+") is the only permitted non-Windows
> language.

---

## 1. Goals and non-goals

### Goals for this pass

1. **Make the workspace honest at build time.** Today `cargo build --workspace`
   on macOS or Linux fails with a wall of Win32 FFI errors, because
   `ghost-core`, `ghost-cache`, `ghost-intent`, `ghost-session`, `ghost-cli`,
   `ghost-http` and `ghost-mcp` all pull the `windows` crate unconditionally.
   After this pass, a non-Windows host gets either a green build of the
   genuinely portable subset or a single explicit sentence explaining that the
   crate is Windows-only.
2. **Prove portability with CI, not with prose.** `macos-latest` and
   `ubuntu-latest` jobs that check the portable crates and run
   `cargo test -p ghost-platform` on their native hosts.
3. **Write down the native backend designs** for macOS (Apple Silicon primary)
   and Linux so the on-device work is transcription, not invention. The
   per-method notes already in `crates/ghost-platform/src/macos.rs` and
   `crates/ghost-platform/src/linux.rs` are the source of truth; §5 and §6
   expand them into a function-by-function map against the real Windows session
   API in `crates/ghost-session/src/session.rs`.
4. **Add a tripwire** so nobody can flip a scaffold to `functional: true` by
   accident.
5. **Fix the user-facing surfaces** that currently let a scanning reader
   conclude Ghost is a three-platform product (audit items S1-a, S1-c, S1-d,
   S2-a, S3-a, S3-b).

### Non-goals for this pass

- Writing any native macOS or Linux FFI. There is no Mac and no Linux desktop
  session in the loop yet; shipping unverifiable FFI is a guess, not an
  implementation (this is the same reasoning already recorded in
  `docs/cross-platform.md` §"Why not build the native backends here").
- Changing any Windows behaviour. The Pass-3 PR is a build-honesty and docs
  change; Windows-only source semantics are untouched.
- Flipping any capability flag.
- Shipping a non-Windows installer or kit artifact (see §10).

### Target selection

- **macOS: Apple Silicon first.** `aarch64-apple-darwin` is the primary and only
  verified Mac target for v0.17. `x86_64-apple-darwin` is expected to work
  (same frameworks) but is *unverified* and must not be claimed. macOS 14+ is
  the floor, because ScreenCaptureKit's `SCScreenshotManager` one-shot API is
  the capture path we want and older fallbacks (`CGWindowListCreateImage`) are
  deprecated.
- **Linux: `x86_64-unknown-linux-gnu`**, X11 session first, Wayland behind an
  explicit detection branch (§6.4). GNOME-on-Wayland input is a known hard
  problem and is explicitly out of scope for v0.17 (§11).

---

## 2. Current architecture reality

Nine workspace crates. Sorted by what they actually depend on:

| Crate | Kind | `windows` crate? | Portable today? | Notes |
| --- | --- | --- | --- | --- |
| `ghost-platform` | lib | no | **yes** | Pure Rust + `serde`. Defines `Platform`, `Feature`, `Capabilities`, `Backend`, `capabilities_for()`. Already compiles for all three targets. |
| `ghost-ground` | lib | no | **yes** | Grounding cascade: coordinate contract, types, parser, engine, optional YOLO/`ort` tier. Grep for `windows`/`Win32` finds only two comments (`types.rs:45`, `cv_detect.rs:2`). No FFI. |
| `ghost-core` | lib | **yes** (+ `windows-core`) | no | Win32/UIA/SendInput/DXGI/GDI/OCR primitives. The FFI floor. |
| `ghost-cache` | lib | **yes** | no | UIA mirror + locator cache; depends on `ghost-core`. |
| `ghost-intent` | lib | no direct | no | Depends on `ghost-core` + `ghost-cache`, so Windows-locked transitively. |
| `ghost-session` | lib | **yes** | no | The safe async API — `GhostSession`. Depends on core/cache/intent/ground. |
| `ghost-cli` | bin `ghost` | **yes** | no | Depends on `ghost-session`. |
| `ghost-http` | bin `ghost-http` | no direct | no | Depends on `ghost-session`; Windows-locked transitively. |
| `ghost-mcp` | bin `ghost-mcp` | **yes** | no | Depends on `ghost-session`; the MCP surface. |

Two facts worth stating plainly:

1. **The Windows lock is a dependency-graph fact, not a code-style fact.**
   Everything above `ghost-core` inherits it. Gating only the direct `windows`
   dependency in five crates is not enough: `ghost-cache` also names `windows`
   directly, and `ghost-intent` / `ghost-http` inherit the lock without naming
   it. Any gating scheme has to cover all seven.
2. **`ghost-platform` is deliberately not wired to the engine.** It declares the
   contract and the capability matrix; it does not call `ghost-session`. That is
   why it already builds everywhere, and it is the right seam to grow the native
   backends behind.

CI today (`.github/workflows/ci.yml`) is `windows-latest` only: build, test,
clippy, release build. No target currently proves "compiles on Darwin/Linux".
`RUSTFLAGS: -D warnings` is set workspace-wide and must not be weakened.

---

## 3. Proposed target-conditional refactor

### Option A — cfg-gate the Windows crates at the workspace level

Move `windows = { workspace = true }` (and `windows-core`) into
`[target.'cfg(windows)'.dependencies]` in each Windows-locked crate, add a
crate-root Windows gate to each `lib.rs` / `main.rs`, and add `default-members`
to the workspace so `cargo build` on a non-Windows host builds only the crates
that legitimately build there (`ghost-platform`, `ghost-ground`).

- **Cost:** a few hours. Seven manifest edits, seven one-line source edits, one
  workspace edit.
- **Effect:** `cargo build` / `cargo check` (no `--workspace`) is green on
  macOS/Linux. `cargo check --workspace` on macOS/Linux stops with one explicit
  sentence per gated crate instead of hundreds of unresolved-import errors.
  Windows behaviour is byte-identical.
- **Limitation:** it does not make `ghost-mcp` (the actual product surface)
  available off Windows. It makes the *build story* honest, nothing more.

### Option B — extract `ghost-session-core`

Pull the OS-agnostic half of the session API into a new `ghost-session-core`
crate: the verbs (`find`, `act`, `type`, `press`, `screenshot`, `describe`, …)
as an async trait plus the shared request/response types, with zero FFI.
`ghost-platform` grows a per-OS implementation of that trait (Windows delegates
to today's `ghost-session`; macOS/Linux get the native backends). `ghost-mcp`,
`ghost-http` and `ghost-cli` then depend on `ghost-session-core` +
`ghost-platform` and dispatch through `ghost_platform::current()`.

- **Cost:** significant. `GhostSession` has ~65 public methods
  (`crates/ghost-session/src/session.rs`), several of which return Windows types
  (`ghost_core::uia::ElementDescriptor`, `WindowInfo`, `EditCommand`) that would
  have to be re-expressed in `ghost-platform`'s vocabulary (`ElementInfo`,
  `WindowRef`, `ActionKind` already exist in `ghost-platform::types` and are the
  natural landing zone). It also touches the MCP tool dispatch, which is the
  highest-traffic, best-tested code in the repo.
- **Effect:** the real prize — one MCP binary that works on three OSes, with the
  per-OS engine selected at runtime by `current()`.
- **Risk:** doing this *before* a Mac backend exists means designing the trait
  against one implementation and guessing at the second. That is how you get an
  abstraction shaped like Win32 with a macOS adapter bolted on.

### Pick: **Option A now, Option B immediately after the Mac backend lands.**

Rationale. Option A buys the thing we actually need this month — an honest,
CI-proven build story — for a few hours of low-risk work, and it does not
foreclose Option B. Option B's trait should be extracted *with two working
implementations in hand*, so the seam is drawn where the OSes genuinely differ
(background dispatch, capture permissions, key encoding) rather than where Win32
happens to draw it. Concretely: ship A in 0.17.0-alpha, write the Mac backend
against `ghost-platform::Backend` directly, and start B once
`cargo test -p ghost-platform` passes on an Apple Silicon machine with live AX
discovery. Order matters — B before A means refactoring code that still can't be
compiled off Windows, and B before a Mac backend means designing blind.

---

## 4. Crate-by-crate deltas (Option A)

Goal: on a Windows dev box (or CI without a Mac in the loop),
`cargo check -p ghost-platform -p ghost-ground --target aarch64-apple-darwin`
and `--target x86_64-unknown-linux-gnu` both pass, and
`cargo check --workspace --target …` fails with one readable sentence per
Windows-only crate rather than an FFI avalanche.

### 4.1 Workspace `Cargo.toml`

```toml
[workspace]
members = [ …unchanged… ]
# Crates that legitimately build on every OS. `cargo build` with no --workspace
# flag builds only these, so a macOS/Linux checkout gets a green, honest build.
default-members = [
    "crates/ghost-platform",
    "crates/ghost-ground",
]
```

Windows developers who want everything keep using `cargo build --workspace`
(which overrides `default-members`); a `.cargo/config.toml` alias is added so
the intent is discoverable:

```toml
[alias]
build-all = "build --workspace"
check-all = "check --workspace"
```

### 4.2 The seven Windows-locked crates

For `ghost-core`, `ghost-cache`, `ghost-intent`, `ghost-session`, `ghost-cli`,
`ghost-http`, `ghost-mcp`:

**(a) Manifest.** Where the crate names `windows` (or `windows-core`) directly,
move it:

```toml
[target.'cfg(windows)'.dependencies]
windows = { workspace = true }
windows-core = "0.58"      # ghost-core only
```

`ghost-intent` and `ghost-http` name no Windows dependency and need no manifest
change; they are gated in source only.

**(b) Crate root.** Add `#![cfg(windows)]` as the first inner attribute of
`src/lib.rs` / `src/main.rs` (after the existing `//!` docs and, in
`ghost-mcp`, after `#![recursion_limit]`). On Windows this is a no-op; off
Windows it strips the crate rather than letting `use windows::Win32::…` explode.

**(c) Build guard.** Add a six-line `build.rs` to each:

```rust
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        eprintln!("Ghost's <crate> is Windows-only today; see docs/cross-platform.md");
        std::process::exit(1);
    }
}
```

*Why both (b) and (c)?* `#![cfg(windows)]` alone gives a clean result for
libraries (they compile to nothing) but for the three binaries it produces
`error[E0601]: main function not found`, which is terse but says nothing useful.
A `#[cfg(not(windows))] compile_error!("…")` alone gives the right sentence but
does **not** suppress the hundreds of unresolved-import errors from the rest of
the file, and gating ~60 top-level items in `ghost-mcp/src/main.rs` individually
is not a "smallest correct change". The build-script guard is the only mechanism
that produces exactly one sentence, identically for libs and bins, and it reads
the *target* OS (so cross-checking a Mac target from a Windows box errors
correctly too).

### 4.3 `ghost-platform`

No manifest change in this pass. When the native backends land:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
accessibility-sys = "0.1"
core-foundation = "0.10"
core-graphics = "0.24"
objc2 = "0.5"
objc2-app-kit = "0.2"
objc2-screen-capture-kit = "0.2"

[target.'cfg(target_os = "linux")'.dependencies]
atspi = "0.22"
zbus = "4"
x11rb = "0.13"
ashpd = "0.9"
```

New files at that point: `src/macos/{mod,ax,cgevent,capture,permissions}.rs`,
`src/linux/{mod,atspi,input_x11,input_libei,capture_x11,capture_portal,session_kind}.rs`.
`src/macos.rs` and `src/linux.rs` become `mod.rs` and keep their existing
implementation notes verbatim — those notes are the spec.

### 4.4 `ghost-ground`

No change. Verified portable: no `windows`/`winapi` dependency, and the only
`Win32` mentions in the source are two comments. It stays a `default-member`.
Note the optional `yolo` feature pulls `ort` with `download-binaries`; the
macOS/Linux CI jobs must build it **without** `--all-features` so no ONNX
Runtime download happens in CI.

### 4.5 Verification of §4 on a Windows dev box

```powershell
rustup target add aarch64-apple-darwin x86_64-unknown-linux-gnu
cargo check -p ghost-platform -p ghost-ground --target aarch64-apple-darwin
cargo check -p ghost-platform -p ghost-ground --target x86_64-unknown-linux-gnu
cargo check --workspace           # still green on Windows
```

`cargo check -p` for pure-Rust crates cross-compiles fine without an SDK; it
never links. Anything that needs to *link* (the three binaries) stays a
Windows-host job.

---

## 5. MacBackend design

Primary target `aarch64-apple-darwin`, macOS 14+. Source of truth: the
implementation map already in `crates/ghost-platform/src/macos.rs`. This section
expands it, it does not replace it.

### 5.1 Discovery — AXUIElement

- Entry points: `AXUIElementCreateSystemWide()` for global, and
  `AXUIElementCreateApplication(pid)` per target app (obtained from
  `NSWorkspace.runningApplications` / `CGWindowListCopyWindowInfo`).
- Tree walk: `AXUIElementCopyAttributeValue(el, kAXChildrenAttribute)`,
  recursing depth-first with a depth cap and a node budget (Windows UIA walks
  already use a budget; reuse the same numbers so snapshot sizes are comparable).
- Per node read: `kAXRoleAttribute` (→ `ElementInfo.role`),
  `kAXTitleAttribute` (→ name; fall back to `kAXDescriptionAttribute`, then
  `kAXValueAttribute` for static text), `kAXEnabledAttribute` (→ `enabled`),
  `kAXPositionAttribute` + `kAXSizeAttribute` (→ `Rect`; note AX returns
  `AXValue` wrappers — unwrap with `AXValueGetValue`),
  `kAXActionsAttribute` via `AXUIElementCopyActionNames` (→ available actions).
- Role mapping: `AXButton`→`button`, `AXTextField`/`AXTextArea`→`edit`,
  `AXCheckBox`→`checkbox`, `AXRadioButton`→`radio`, `AXMenuItem`→`menuitem`,
  `AXStaticText`→`text`, `AXWindow`→`window`, `AXList`/`AXTable`→`list`. The
  mapping table lives next to the Windows UIA control-type mapping so the MCP
  role vocabulary stays identical across OSes — this is what lets one agent
  prompt work on both.
- Coordinates: AX is in **top-left-origin screen points**, already matching
  Ghost's `Rect` contract; CGEvent is also top-left. `NSScreen` is
  bottom-left-origin — convert if that path is ever used. On Retina, AX points
  are logical; `ghost-ground` needs the backing-scale factor
  (`NSScreen.backingScaleFactor`) to reconcile with pixel-space screenshots.
  This is the macOS analogue of the Windows DPI-awareness check and must be a
  `ghost doctor` item.

### 5.2 Input — CGEvent

- Click: `CGEventCreateMouseEvent(NULL, kCGEventLeftMouseDown/Up, point, kCGMouseButtonLeft)`
  then `CGEventPost(kCGHIDEventTap, …)`. Right-click/double-click via the
  matching event types and `kCGMouseEventClickState`.
- Keys: `CGEventCreateKeyboardEvent(src, keycode, down)` with
  `CGEventSetFlags` for modifiers. Text entry prefers
  `CGEventKeyboardSetUnicodeString` so we are not fighting keyboard layouts —
  this is the closest analogue to Windows `SendInput` with `KEYEVENTF_UNICODE`.
- Scroll: `CGEventCreateScrollWheelEvent`.
- Drag: mouse-down, N `kCGEventLeftMouseDragged` steps, mouse-up — same shape as
  the Windows implementation.

### 5.3 Background dispatch — the honest part

Windows' wedge is **posted window messages** (`WM_SETTEXT`, `BM_CLICK`,
`WM_COPY`, …): the target processes them off its own message pump without being
raised or focused. macOS has **no exact equivalent.** The two candidates:

1. `AXUIElementSetAttributeValue(el, kAXValueAttribute, str)` for text — this is
   the true analogue of UIA `ValuePattern.SetValue` and is very likely
   focus-safe.
2. `AXUIElementPerformAction(el, kAXPressAction)` for clicks — semantically
   right, but **some apps raise/activate on press**. AppKit apps that implement
   `accessibilityPerformPress` by routing through the responder chain may
   order-front.

Therefore `Feature::BackgroundDispatch` on macOS is **unknown → measure** and
must not appear in `capabilities_for(Platform::MacOS).supported` until §7(d)
produces a number across a real app set. If the measured focus-preservation rate
is materially below the Windows number, the honest outcome is to list macOS
background dispatch as PARTIAL (or omit the feature) and say so in
`docs/cross-platform.md` — not to soften the definition of the claim.

`CGEventPostToPid(pid, event)` is worth measuring as a third path: it delivers a
synthesized event to one process without a global tap, but it still generally
targets the app's key window, so treat it as "measure", not "solution".

### 5.4 Capture — ScreenCaptureKit

- `SCShareableContent.getWithCompletionHandler` to enumerate displays/windows,
  then `SCScreenshotManager.captureImageWithFilter:configuration:` for a one-shot
  screen or single-window grab (macOS 14+).
- Window-scoped capture uses `SCContentFilter(desktopIndependentWindow:)` — this
  is the analogue of Windows `PrintWindow`, and like `PrintWindow` it can capture
  an occluded window, which is what makes act-then-verify work in background mode.
- Convert `CGImage` → RGBA → the same `image` crate encode path Ghost already
  uses, so `ghost-ground` and the MCP screenshot tool are unchanged.
- Legacy `CGWindowListCreateImage` is deprecated in macOS 14; keep it only as a
  fallback behind a version check, if at all.

### 5.5 Permissions

Two separate, user-granted, non-programmable TCC permissions:

| Permission | Needed for | Probe | Prompt |
| --- | --- | --- | --- |
| **Accessibility** (`kTCCServiceAccessibility`) | all AX read + AX actions + CGEvent posting | `AXIsProcessTrusted()` | `AXIsProcessTrustedWithOptions` with `kAXTrustedCheckOptionPrompt: true` |
| **Screen Recording** (`kTCCServiceScreenCapture`) | ScreenCaptureKit / any window-content read | `CGPreflightScreenCaptureAccess()` | `CGRequestScreenCaptureAccess()` |

Both are granted per *binary*, keyed to the code signature. An unsigned
`ghost-mcp` rebuilt with a different hash may need re-granting — which is one of
the reasons code-signing is on the list in §11. If Ghost is ever driven by
another app via Apple Events, that host needs the
`com.apple.security.automation.apple-events` entitlement plus
`NSAppleEventsUsageDescription`; Ghost itself does not use Apple Events for the
AX/CG paths, but `ghost doctor` should report it because the failure mode
(silent no-op) is otherwise unexplainable.

### 5.6 Function-by-function map

Windows column is the real implementation in `crates/ghost-session/src/session.rs`
(delegating to `ghost-core`). Every row is a v0.17 obligation unless marked.

| `GhostSession` fn | Windows primitive | macOS primitive | Linux primitive |
| --- | --- | --- | --- |
| `find` / `find_all_foreground` | UIA `FindAll` + cached tree walk | `AXUIElementCopyAttributeValue(kAXChildrenAttribute)` DFS + `kAXRole`/`kAXTitle` match | AT-SPI `Accessible.GetChildren` DFS + `GetRole`/`Name` |
| `describe_screen` / `describe_screen_fast` | UIA tree snapshot (`ghost-cache` mirror) | AX tree snapshot per-app, cached by `AXObserver` notifications | AT-SPI tree snapshot, cached by D-Bus a11y events |
| `describe_screen_delta` / `event_seq` / `wait_for_event` | UIA event mirror (`ghost-cache`) | `AXObserverCreate` + `kAXUIElementDestroyed`/`kAXValueChanged`/`kAXFocusedUIElementChanged` | AT-SPI `object:state-changed`, `object:text-changed` signals |
| `act` / `act_on_element` (click) | `InvokePattern.Invoke` / `BM_CLICK` | `AXUIElementPerformAction(kAXPressAction)` | `Action.DoAction("click")` |
| `act` (type/set value) | `ValuePattern.SetValue` / `WM_SETTEXT` | `AXUIElementSetAttributeValue(kAXValueAttribute)` | `EditableText.SetTextContents` |
| `act_background` / `click_background` / `key_background` | **posted window messages** (the wedge) | AX set-value + `kAXPressAction`; **measure focus preservation** (§5.3) | AT-SPI action/set-text; **measure raise behaviour** (§6.3) |
| `edit_command_background` | `WM_COPY`/`WM_CUT`/`WM_PASTE`/`WM_UNDO` | AX `kAXSelectedTextAttribute` + `NSPasteboard`; CGEvent ⌘C/⌘V fallback | AT-SPI text selection + clipboard (X11 selections / `wl-clipboard`) |
| `click_at` / `right_click_at` / `double_click_at` / `hover` | `SendInput` mouse | `CGEventCreateMouseEvent` + `CGEventPost` | XTest `XTestFakeButtonEvent` / libei |
| `drag` | `SendInput` down/move/up | `kCGEventLeftMouseDragged` sequence | XTest motion + button |
| `scroll` / `scroll_until` | `WM_MOUSEWHEEL` / `SendInput` | `CGEventCreateScrollWheelEvent` | XTest button 4/5 or AT-SPI scroll |
| `press` / `key_down` / `key_up` / `hotkey` | `SendInput` / `WM_KEYDOWN` | `CGEventCreateKeyboardEvent` + `CGEventSetFlags` | XTest `XTestFakeKeyEvent` / libei |
| `paste_text` | clipboard + `WM_PASTE` | `NSPasteboard.setString` + ⌘V | clipboard + AT-SPI paste action |
| `get_clipboard` / `set_clipboard` | Win32 clipboard API | `NSPasteboard.general` | X11 CLIPBOARD selection / Wayland `zwlr_data_control` |
| `screenshot` / `screenshot_region` | DXGI desktop duplication / GDI | `SCScreenshotManager.captureImage` | X11 `XGetImage` / XShm; Wayland `org.freedesktop.portal.Screenshot` |
| `foreground_window_rect` | `GetForegroundWindow` + `GetWindowRect` | `AXUIElementCopyAttributeValue(systemWide, kAXFocusedApplicationAttribute)` → `kAXFocusedWindow` → position/size | AT-SPI `Accessible.GetState(active)` + `Component.GetExtents` |
| `list_windows` | `EnumWindows` + `GetWindowText` | `CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly)` + AX titles | `wmctrl`-equivalent via EWMH `_NET_CLIENT_LIST` (X11); AT-SPI app list (Wayland) |
| `focus_window` / `ensure_window_foreground` | `SetForegroundWindow` / `AttachThreadInput` dance | `NSRunningApplication.activate` + `AXUIElementSetAttributeValue(kAXMainAttribute, true)` | EWMH `_NET_ACTIVE_WINDOW` (X11); **no portable Wayland equivalent** |
| `window_state` (min/max/restore) | `ShowWindow` | AX `kAXMinimizedAttribute` / `AXZoomButton` press | EWMH `_NET_WM_STATE` |
| `launch` | `CreateProcess` | `NSWorkspace.openApplicationAtURL` / `posix_spawn` | `posix_spawn` / `.desktop` via `gio launch` |
| `get_text` / `read_text` / `get_selected_text` | UIA `TextPattern` / `ValuePattern` | `kAXValueAttribute`, `kAXSelectedTextAttribute`, AXTextMarker ranges | AT-SPI `Text.GetText`, `Text.GetSelection` |
| `wait_for_element` / `wait_for_value` / `wait_until` / `wait_for_idle` | polling over UIA cache + events | polling over AX cache + `AXObserver` | polling over AT-SPI cache + D-Bus signals |
| per-action verify (inside `act_*`) | screen-delta + control read-back | `SCScreenshotManager` delta + AX read-back | portal/XGetImage delta + AT-SPI read-back |
| `render_marks` / `locate_by_description` / `ground` / `find_text_local` | `ghost-ground` (OS-agnostic) + OCR | same `ghost-ground`; OCR via Vision.framework `VNRecognizeTextRequest` (replaces `Windows.Media.Ocr`) | same `ghost-ground`; OCR via `tesseract` or the existing remote/vision tier |
| `execute_intent` | `ghost-intent` FSM (OS-agnostic logic, Windows verbs) | same FSM once verbs are behind `Backend` (Option B) | same |
| `shell` verbs (`ghost_shell`) | Win32 console/ConPTY | **not in scope for v0.17** (§11) | **not in scope for v0.17** (§11) |
| `stop` / emergency stop | Ctrl+Alt+G global hotkey (`RegisterHotKey`) | `CGEventTapCreate` global tap (needs Accessibility) or `NSEvent.addGlobalMonitor` | X11 `XGrabKey`; Wayland: compositor-dependent, likely unavailable |

Rows where the macOS or Linux cell is not a straight substitution —
`act_background`, `focus_window` on Wayland, `shell`, emergency stop — are the
ones that decide whether the capability list shrinks off Windows. Plan for the
list to shrink.

---

## 6. LinuxBackend design

Target `x86_64-unknown-linux-gnu`. Source of truth: the notes in
`crates/ghost-platform/src/linux.rs`.

### 6.1 Discovery and action — AT-SPI2

- `atspi` crate over `zbus`. Requires the accessibility bus to be running
  (`org.a11y.Bus`) and toolkits to expose a11y — GTK3/4 and Qt5/6 do by default;
  `GTK_MODULES=gail:atk-bridge` / `QT_ACCESSIBILITY=1` are the escape hatches,
  and Electron needs `--force-renderer-accessibility` (or responds to the
  a11y-enabled signal). Apps that expose nothing (raw X11, some games, Java
  without the a11y bridge) are simply out of reach — the same class of limitation
  as a Windows app that exposes no UIA tree.
- Roles via `Accessible.GetRole`, name via `Accessible.Name`, enabled/sensitive
  via `Accessible.GetState` (`STATE_ENABLED`, `STATE_SENSITIVE`,
  `STATE_SHOWING`), geometry via `Component.GetExtents(coord_type=Screen)`,
  children via `Accessible.GetChildren`.
- Actions via the `Action` interface: `GetActions` → `DoAction(index)`. The
  action named `click`/`press`/`activate` is the analogue of UIA `Invoke`.
- Text via `Text` / `EditableText`: `SetTextContents` is the background-safe
  value-set analogue.
- Events: subscribe to `object:state-changed`, `object:text-changed`,
  `object:children-changed` on the a11y bus to drive the same delta/`event_seq`
  machinery `ghost-cache` provides on Windows.

### 6.2 Input

- **X11:** XTest via `x11rb` — `XTestFakeKeyEvent`, `XTestFakeButtonEvent`,
  `XTestFakeMotionEvent`. Well-trodden, synchronous, no prompt.
- **Wayland:** XTest does not exist. Options, in order of preference:
  1. **libei** (`ei` / `reis` bindings) negotiated through the
     `org.freedesktop.portal.RemoteDesktop` portal — the sanctioned path, and
     the direction GNOME/KDE are converging on. Requires a user grant, and the
     grant is per-session.
  2. `uinput` (`/dev/uinput`) — works everywhere, needs group/udev permissions,
     and injects at the kernel level so it is global rather than targeted.
  3. AT-SPI actions only (no synthetic input at all) — degraded but often
     sufficient, since Ghost prefers accessibility actions over coordinates
     anyway.

### 6.3 Background dispatch

AT-SPI `DoAction` / `SetTextContents` generally do **not** require raising the
window, which makes Linux a plausibly *good* background platform. But "generally"
is not a claim. Some toolkits grab focus inside their action handler. Measure per
§7(d) before listing `Feature::BackgroundDispatch` for Linux.

### 6.4 Capture and the Wayland/X11 fork

Detection branch, evaluated once at backend construction and cached:

```text
if env XDG_SESSION_TYPE == "wayland"            -> Wayland
else if env WAYLAND_DISPLAY is set               -> Wayland
else if env DISPLAY is set                       -> X11
else                                             -> Headless  (no input, no capture)
```

Report the resolved session kind in `ghost doctor` and in the backend's
`Capabilities.status` string, because it changes what Ghost can do:

| | X11 | Wayland |
| --- | --- | --- |
| Capture | `XGetImage` / XShm, no prompt, can target a window id | `org.freedesktop.portal.Screenshot` via `ashpd`; **may prompt**, screen-only on some compositors, window-scoped capture not guaranteed |
| Input | XTest, no prompt | libei via RemoteDesktop portal (grant required) or `uinput` |
| Window list / activate | EWMH `_NET_CLIENT_LIST`, `_NET_ACTIVE_WINDOW` | no protocol for client window enumeration or activation; fall back to AT-SPI app list, and treat `focus_window` as unsupported |
| Global hotkey (emergency stop) | `XGrabKey` | `org.freedesktop.portal.GlobalShortcuts` where available, else unsupported |

Wayland's honest summary: discovery and action work (AT-SPI is display-server
agnostic), capture is portal-gated and may prompt, input needs a grant, and
window management is mostly unavailable. That is a materially smaller capability
set than X11, and `capabilities_for(Platform::Linux)` will eventually need to be
session-aware rather than a single static list.

---

## 7. On-device verification checklist (gate for `functional: true`)

No capability flag flips until **every** item passes on real hardware for that
OS. Record the run — date, machine, OS version, app set, numbers — in
`docs/benches/` next to the Windows results, and link it from
`docs/cross-platform.md`.

- **(a) Unit gate.** `cargo test -p ghost-platform` passes on the native target
  (`aarch64-apple-darwin` / `x86_64-unknown-linux-gnu`), including the new
  host-capability tripwire test. Note the tripwire asserts `!is_functional()` on
  non-Windows, so flipping a flag *deliberately* means updating that test in the
  same commit — which is the point.
- **(b) Live discovery.** Discover the full control tree of at least one built-in
  app and assert non-trivial structure: ≥1 window, ≥5 elements, correct roles,
  correct `enabled` values, plausible `Rect`s. macOS: TextEdit and Calculator.
  Linux: gedit/GNOME Text Editor and Files, on both a GTK and a Qt app.
- **(c) Act-then-verify.** Click and type both return `verified: true` from real
  read-back, not from "the call didn't error": type into a text field and read
  `kAXValue` / `Text.GetText` back; click a button and confirm the observable
  state change. 20/20 on the app set, no flakes.
- **(d) Focus preservation on background dispatch** — Ghost's headline claim.
  Across ≥5 apps × ≥20 dispatches each: measure the rate at which the target
  did **not** activate and the foreground app did **not** change (poll the
  frontmost app before/after; on macOS `NSWorkspace.frontmostApplication`, on
  Linux the EWMH active window or AT-SPI active state). Report the rate as a
  number. Below the Windows number → the feature is PARTIAL or absent off
  Windows, and the docs say so. Do not average away per-app failures; list them.
- **(e) Screenshot round-trip.** Full-screen and window-scoped capture produce a
  decodable image of the expected pixel dimensions, correct on Retina/HiDPI
  (backing-scale reconciled), and `ghost-ground` can locate a known on-screen
  element in the captured frame.

Only after (a)–(e): extend `capabilities_for()` with exactly the `Feature`s that
were verified — not `all_features()` — and update the matrix in
`docs/cross-platform.md`.

---

## 8. CI matrix additions

Add to `.github/workflows/ci.yml`, alongside the existing `windows-latest`
`test` and `release-build` jobs (which are unchanged):

```yaml
  check-macos:
    name: Check (macOS)
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo check -p ghost-platform -p ghost-ground
      - run: cargo test -p ghost-platform

  check-linux:
    name: Check (Linux)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo check -p ghost-platform -p ghost-ground
      - run: cargo test -p ghost-platform
```

Rules for these jobs:

- They run **only** the portable crates. They must never be given
  `--workspace`, because the Windows-only crates are expected to fail there by
  design and a red X on an expected failure trains people to ignore CI.
- The workspace `RUSTFLAGS: -D warnings` still applies. Do not weaken it, do not
  add `--cap-lints`, do not special-case the new jobs.
- No `--all-features` (that would drag in the `ort`/YOLO download).
- `macos-latest` is Apple Silicon, which is exactly the primary target — this
  job is also the future home of the Mac backend's compile check.
- When the native backends land, these two jobs additionally run
  `cargo clippy -p ghost-platform -- -D warnings`, and a **manually dispatched**
  self-hosted job runs the §7 on-device checks (GitHub's hosted runners have no
  interactive desktop session, no TCC grants, and no a11y bus — they can never
  verify (b)–(e)).

---

## 9. `ghost doctor` changes

### Now (v0.17.0-alpha)

`crates/ghost-cli/src/doctor.rs` already has a `#[cfg(not(windows))]`
`run_checks()` returning a single FAIL. Because `ghost-cli` does not build off
Windows at all after §4, that branch is currently unreachable — keep it, and
make its message version-accurate and actionable:

```
FAIL  platform  Ghost is Windows-only at v0.16.x; macOS/Linux backends land in
                v0.17+. See docs/cross-platform.md.
```

Exit code: the current policy is `exit_code()` → 1 when any check FAILs. The
requested behaviour for "wrong OS" is **exit code 2**, distinct from "right OS,
something is broken" (1). Implement that as an explicit early return in
`main()` for the non-Windows case rather than by changing `exit_code()`'s
FAIL→1 rule, so the existing exit-code tests
(`only_fail_is_fatal`) keep their meaning:

- `0` — all PASS/WARN
- `1` — at least one FAIL on a supported OS
- `2` — unsupported OS

### Later (Mac doctor, ships with the Mac backend)

`ghost doctor` on macOS reports:

| Check | PASS condition | Probe |
| --- | --- | --- |
| macOS version | ≥ 14.0 | `ProcessInfo.operatingSystemVersion` |
| Accessibility permission | granted | `AXIsProcessTrusted()` — FAIL with the exact System Settings path and the `x-apple.systempreferences:` deep link |
| Screen Recording permission | granted | `CGPreflightScreenCaptureAccess()` |
| Apple Events entitlement | present if a host app drives Ghost | read `com.apple.security.automation.apple-events` from the embedded entitlements; WARN (not FAIL) when absent, since the AX/CG paths don't need it |
| Active display count | ≥ 1; report each display's frame + `backingScaleFactor` | `CGGetActiveDisplayList` / `NSScreen.screens` |
| Notch / menu-bar safe zone | report `NSScreen.safeAreaInsets` and `visibleFrame` vs `frame` | WARN when a notch is present, because the unusable strip changes coordinate assumptions in vision grounding |
| Code signature | signed + notarized | WARN when unsigned: permission grants are keyed to the signature and will be lost on rebuild |

Linux doctor (later still): a11y bus reachable, session type (X11/Wayland),
XTest available, portal available, `uinput` writable, display list.

---

## 10. Kit / install changes

**No changes to `kit/*` in this pass.** `kit/install.ps1` and
`kit/mcp-config.json` are Windows artifacts and make no cross-platform claim;
the audit's verdict is "keep as-is" and that stands.

When macOS is verified (§7 green on Apple Silicon):

- Add `kit/install.sh` — POSIX shell, macOS first, with the Accessibility and
  Screen Recording grant walkthrough inline (the install is not "done" until
  both are granted, and the script should verify, not just instruct).
- Add `kit/mcp-config.mac.json` — same tool surface, macOS binary path,
  `~/Library/Application Support/Claude/claude_desktop_config.json` guidance.
- Ship as a **separate release artifact** (`ghost-kit-macos-<ver>.tar.gz`), not
  by extending the current `.ps1` flow. Do not mix a Mac binary into the Windows
  kit zip; the two have different permission stories, different signing
  requirements, and different support burdens.
- `scripts/package-kit.ps1` refuses to package unless the live desktop suite
  passes; the Mac kit needs the equivalent gate (`scripts/package-kit.sh`) with
  the §7 checks, or it does not ship.
- Only then may northtek.io/ghost mention macOS. Until then the paid page says
  Windows 10/11 — see audit §8, which flags a cross-platform claim on a paid
  page as the highest-stakes item in the whole audit.

---

## 11. Release plan

**v0.17.0-alpha — "honest build gates".** Ships §4 (cfg-gated workspace,
`default-members`, build guards), §8 (macOS/Linux CI jobs), the
`ghost-platform` tripwire test, the §9 "now" doctor message, and the audit
redlines. `MacBackend` and `LinuxBackend` remain inert and still report
`functional: false`. The single claim this release makes is: *the portable
subset of the Ghost workspace compiles and tests green on macOS and Linux in
CI.* Nothing more.

**v0.17.0 — "macOS lands".** Ships when §7(a)–(e) pass on Apple Silicon and the
results are recorded in `docs/benches/`. Capability list for macOS is exactly
what was measured. If background dispatch measures poorly, it ships as PARTIAL
or absent, and the README's wedge paragraph stays Windows-scoped.

**v0.18.0 — "Linux lands" + Option B.** Linux backend verified on X11 (Wayland
capability set stated separately), and the `ghost-session-core` extraction so
`ghost-mcp` is a single binary across OSes.

---

## 12. Explicitly NOT in this plan

Each of these needs its own design pass:

1. **Shell control (`ghost_shell`) on non-Windows.** The Windows implementation
   is ConPTY-shaped. A POSIX PTY implementation is a different design (job
   control, signal handling, TTY sizing), and the security model for a
   remote-driven shell deserves its own review.
2. **The kit installer on non-Windows.** §10 sketches the shape; the actual
   script, its verification gate, and the release plumbing are separate work.
3. **Code-signing and notarization on macOS.** Developer ID certificate,
   hardened runtime, `notarytool` submission, stapling, and the CI secrets to do
   it. This is a prerequisite for a *usable* Mac kit (unsigned binaries lose TCC
   grants on every rebuild and trip Gatekeeper), not merely a polish item.
4. **Wayland input on GNOME.** libei/RemoteDesktop-portal support varies by
   compositor and version; getting reliable synthetic input on GNOME-Wayland is
   a research task, not an implementation task. v0.17/0.18 Linux support is
   X11-first with Wayland's reduced capability set stated honestly.
5. **Windows-on-ARM** (`aarch64-pc-windows-msvc`) — untested, unclaimed.
6. **`x86_64-apple-darwin`** — expected to work, unverified, unclaimed.
7. **Vision.framework OCR** as a replacement for `Windows.Media.Ocr` — noted in
   the §5.6 table, but the OCR tier's accuracy on macOS must be re-benchmarked
   against the Windows numbers before `ghost-ground`'s tier ordering is reused.
8. **A code-level audit of the bench harness.** Out of scope for the honesty
   audit and out of scope here.
