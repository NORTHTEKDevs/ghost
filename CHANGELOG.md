# Changelog

## [0.19.0] - Background automation, browsers, isolated desktops, concurrency

Merges the `feat/background-automation` line into the cross-platform 0.18 base.
The differentiated claims - automate without touching the user's screen, run
many agents at once with no contention - are now enforced, fast, and provable
on demand.

### Added

- **`ghost-browser` crate + 19 `ghost_browser_*`/`ghost_tab_*` tools**: drive
  individual browser tabs over CDP - Comet, Chrome, Edge, or Brave - without
  the tab being visible or the window focused. Isolated launch per id (own
  process/profile/port) or attach to the user's own browser. Includes the
  fire-and-forget mouse-move fix (a background tab never composits, so awaiting
  the move ack cost 5.01s per click; now ~1-3ms) and screenshot strategies for
  non-compositing tabs.
- **Focus policy** (`ghost-core::focus`): `background` (default) /
  `prefer_background` / `foreground`. Every `SendInput` and
  `SetForegroundWindow` path is gated and fails loudly instead of stealing the
  cursor or foreground. Enforced by `tests/focus_enforcement.rs`;
  `GHOST_FOCUS_POLICY` overrides.
- **Isolated desktops + 12 `ghost_desktop_*` tools**: launch an app onto a
  Windows desktop that is never displayed - the desktop-app equivalent of
  headless. UIA, window messages, and PrintWindow capture all work there;
  `SendInput` does not (OS boundary, documented and measured).
- **Concurrent MCP dispatch**: requests run as parallel tasks against a shared
  session. The engine moved from STA to the multithreaded COM apartment - sound
  because UIA's client objects register the "Both" threading model and this
  codebase uses no COM event callbacks (the EventBus is SetWinEventHook on its
  own thread). Measured: a fast call completes in 0.4ms while a 2s call is in
  flight; three concurrent `ghost_see` calls in 38ms. A compile-time assert
  keeps `GhostSession: Send + Sync`.
- **`ghost verify`**: the claims audit. Drives the real `ghost-mcp` server over
  stdio through ten checks with hard timing budgets - background enforcement,
  concurrent tabs without cross-talk, click/screenshot latency, a fast call
  during a slow call, a second server running its own browser alongside,
  emergency stop/resume, and an untouched foreground. Exit 1 on any failure.
- `ghost_session_state`: foreground hwnd + cursor, so "nothing was disturbed"
  is checkable before and after any batch of actions.

### Fixed

- **The emergency stop never fired.** `RegisterHotKey(None, ..)` binds the
  hotkey to the calling thread's message queue, but the pump ran on a different
  thread, so WM_HOTKEY arrived where nobody read it. Registration and pump now
  share one thread.
- **A second ghost process had no emergency stop at all** (hotkey already
  registered elsewhere). The stop is now a named kernel event shared across the
  session: one Ctrl+Alt+G stops every ghost process, `ghost_reset` resumes them
  all, and hotkey ownership migrates if the owner exits. The event handle is
  held for the process lifetime - a named kernel object dies with its last
  handle, which previously destroyed the signal as it was sent.

### Changed

- `ghost-intent`'s `OpsDispatcher` and `ghost-ground`'s `GroundingTier` are now
  `Send` futures (dispatchers/tiers must be `Sync`) so tool dispatch can be
  spawned as tasks.
- Session interior mutability moved from `RefCell` to `Mutex`
  (reflection/grounding-stats/shells) for the shared-session model.

## [0.18.1] - 2026-08-02 - the fixes a fifth review round found

Supersedes 0.18.0 for Linux users. Take this one: 0.18.0 shipped a resource
leak that can stall the whole MCP server, and a world-readable credential.

### Fixed

- **`find_text_local` leaked a thread and a process on every OCR timeout.**
  Tesseract was bounded only by a `tokio::time::timeout` wrapped around a
  `spawn_blocking` handle, which stops *awaiting* the closure but cannot
  cancel it, so the OS thread stayed parked in `wait()` and the tesseract
  process was never reaped. A caller retrying after each timeout - an ordinary
  automation pattern - starved Tokio's blocking pool until every other tool
  stalled behind it. The deadline now lives inside the blocking closure, which
  kills and then reaps the child.
- **The same call could deadlock.** The whole PNG was written to tesseract's
  stdin before anything read its stdout, so a large capture could fill the
  64KB pipe buffer with both sides waiting on the other. stdin is now fed from
  a separate thread.
- **The Wayland restore token was world-readable.** It was written with
  `std::fs::write`, which lands on disk as 0644 under the default umask. That
  token is a bearer credential: presenting it resumes remote desktop control
  with no consent prompt. It is now 0600 inside a 0700 directory, re-asserted
  on every write so a pre-existing loose file is tightened rather than trusted.
- **`ghost_shell` completion markers could be forged by command output.** The
  sentinel was a predictable counter, so a command printing
  `__GHOST_DONE_1__ 0` - from a log line, a downloaded file, a `cat` of an
  untrusted path - would make Ghost report someone else's exit code and
  desynchronise the session permanently. The nonce now carries an unguessable
  per-session secret. Both drivers echo the field verbatim, so neither changed.
- **Out-of-range capture regions returned an image of the wrong screen area.**
  X11 encodes coordinates as i16; a larger value wrapped silently and the
  server returned a real, plausible-looking image of somewhere else. Rejected
  before the cast.
- **`ghost_window list` could report a minimised window as normal.** It read
  AT-SPI's `Iconified`, which is not a state AT-SPI models reliably. Now
  cross-checked against ICCCM `WM_STATE`, and only where the window matches
  unambiguously - an unmatched window keeps the old reading rather than
  borrowing another window's state.
- **Paste guessed an insertion point.** A failed caret read defaulted to
  offset 0, pasting at the start of the field and reporting success. It now
  fails.
- **Cut could delete the wrong text.** AT-SPI offers no atomic "cut the
  selection" verb, only offsets, and a widget that reformats as you type can
  shift them between the read and the act. The selection is re-verified first.
- **`focused_descendant` had no cycle guard**, the only tree walk without one,
  so a cyclic accessibility tree reported "nothing is focused" with full
  confidence.
- **A window manager's rejection of a window operation was reported as
  success.** The ClientMessage cookie is now checked.
- **Non-ASCII titles never matched themselves** in window resolution: the
  needle was ASCII-folded against a fully-folded haystack.
- **The emergency-stop hotkey could not re-arm.** If the X11 connection
  dropped, the grab was gone and Ctrl+Alt+G was dead for the life of the
  process. The listener now clears its armed flag and logs on the way out. A
  partial grab - working plain, failing under Caps or Num Lock - is no longer
  reported as full success.
- Occluded-window capture inherited the full 20-second accessibility walk
  budget; bounded to 3 seconds.
- Application-supplied values (`n_actions`, element extents) are capped and
  saturated rather than trusted.

### Changed

- `ghost doctor` reports emergency-stop status by querying the grab rather
  than re-registering it. Re-registering to test would collide with Ghost's
  own working grab and report a healthy hotkey as held by another application.
- The Linux limitations table in `docs/linux-fedora.md` said window
  minimize/maximize/restore, the global hotkey, and local OCR were
  unsupported. All three ship on X11. Each entry is now scoped to the session
  type where the limit is real.

## [0.18.0] - 2026-08-02 - Linux parity, and the fixes four reviews found

Supersedes 0.17.0 for Linux users. 0.17.0 shipped defects that could hang the
MCP server; anyone on Linux should take this instead.

### Added — closing the Windows parity gaps

- **Window state via EWMH** (`ghost-linux/src/wm.rs`). `ghost_window op=state`
  now supports minimize, maximize, restore and close on X11 and on XWayland
  windows, using ClientMessages to the root window. AT-SPI has no window
  management at all, which is why this needed a new mechanism.
- **Background-safe edit shortcuts.** Copy/cut/paste/select-all go through
  AT-SPI `EditableText`, the direct analogue of Windows' `WM_COPY`/`WM_CUT`/
  `WM_PASTE`. The application performs the operation, so no keystroke is
  synthesised, the pointer does not move and focus does not change.
- **Occluded-window capture via XComposite.** Reads a window's backing pixmap,
  so a window that is behind another still captures its own content. Background
  mode drives windows that are by definition not in front, so act-then-verify
  was previously checking the wrong pixels.
- **Global Ctrl+Alt+G emergency hotkey** on X11, grabbed under every lock-modifier
  combination so it keeps working with Caps or Num Lock on.
- **Local OCR via Tesseract** when installed, with real per-word boxes.
- `ghost_scroll` left/right, and a working `release_all_modifiers`.

### Fixed — from the production, code, parity and test-quality reviews

- **The MCP server could hang permanently.** The Wayland Screenshot portal call
  had no timeout, and the server runs a current-thread runtime dispatching
  serially — a wedged portal froze everything, silently. Now bounded.
- **Every response ran a full AT-SPI walk.** `foreground_info` is attached to all
  tool results; on Linux it enumerated every application under a 20s budget. Now
  400ms-bounded.
- **Data loss.** `set_value_ex`'s fallback fired Ctrl+A + Delete with no guard.
  Windows gates that on `is_editable_role` precisely because on a non-editable
  target it means select-all + delete. Ported.
- **`mode=background` moved the real cursor** for non-button elements while
  reporting `cursor_preserved: true`. It now drives AT-SPI or refuses.
- **Zombie processes.** Launched applications were never reaped.
- **A declined Wayland consent dialog killed input for the whole session** —
  the error was cached in a `OnceLock`. Only success is cached now.
- **`ghost doctor` raised permission prompts on Wayland.** Diagnostics now report
  which paths would be used instead of exercising them.
- Cycle detection in the AT-SPI walks; blocking captures moved off the async
  runtime; idle waits honour the stop flag.

### Changed

- `capabilities_for(Linux)` claims 7 of 9 features. `KeyInput` is still not
  claimed: synthetic typing has no end-to-end test.
- `ghost doctor` reports Wayland limits explicitly.

### Verification

19 live tests against a real GTK application on Ubuntu and Fedora 41, plus the
CLI, the installer and an MCP stdio smoke test. The Wayland portal paths remain
unverified on hardware — CI runs X11 — and are documented as such.

## [0.17.0] - 2026-08-01 - Linux support (AT-SPI2)

Ghost runs on Linux. `ghost`, `ghost-mcp` and `ghost-http` all build and run
there, exposing the same 20 MCP verbs as on Windows.

### Added

- **`ghost-linux`** - the Linux engine. AT-SPI2 over D-Bus for the accessibility
  tree and actions, XTEST / RemoteDesktop portal / uinput for synthetic input,
  X11 `GetImage` / Screenshot portal for capture. Pure Rust throughout: no
  `-devel` packages are needed to build.
- **Shared session and MCP layers.** `ghost-session` and `ghost-mcp` are now one
  codebase across platforms. `ghost_session::engine` is a `cfg` alias resolving
  to `ghost-core` on Windows and `ghost-linux` on Linux; both expose the same
  module tree and signatures.
- **Background dispatch on Linux** via AT-SPI actions. The application performs
  the operation through its own toolkit, so nothing is raised and no pointer
  moves - a cleaner guarantee than posted window messages, and identical under
  X11 and Wayland.
- **`ghost_shell` on Linux** - bash by default (`sh`/`zsh`/`pwsh` accepted).
  Persistent sessions keep variables, cwd and env across sends using the same
  base64 + per-session-nonce framing as the PowerShell driver.
- **`ghost doctor` Linux checks** - session type, AT-SPI bus reachability,
  whether applications are exposing trees, the selected input backend, capture.
- **`scripts/install.sh`** - builds, installs to `~/.local/bin`, enables toolkit
  accessibility, registers the MCP server, and runs `ghost doctor`.
- **`docs/linux-fedora.md`** - setup, on-device checklist, limitations,
  troubleshooting.
- **Live CI on real Linux.** A synthesised desktop (Xvfb + D-Bus +
  at-spi-bus-launcher) runs 17 live tests against a real GTK application on both
  ubuntu-latest and Fedora 41: 10 against the engine, 7 driving the real
  `ghost-mcp` binary over JSON-RPC. CI also runs `ghost doctor`,
  `ghost list-windows`, `ghost screenshot`, and executes `install.sh`.

### Changed

- Window handles are `isize` across the engine API on both platforms (`0` =
  none): an `HWND` on Windows, an interned AT-SPI `(bus name, object path)` on
  Linux. `WindowInfo.hwnd`, `find_*_in_hwnd`, `BackgroundClicker::click` and
  `ensure_foreground` take `isize`.
- `ghost_shell`'s MCP schema and description are platform-aware. An agent reads
  the schema to decide what to send, so advertising `powershell|cmd` on Linux
  made every call it composed wrong before it was sent.
- `capabilities_for(Linux).functional` is `true`, listing only the six features
  the live suite verifies. `KeyInput`, `EditShortcuts`, `VisionGrounding` and the
  Wayland paths are implemented but not claimed.
- Release builds now publish Linux **and** Windows archives with SHA256 sums.

### Fixed

- **Act-then-verify missed small changes.** One changed cell of the 32x32 grid
  scores 0.00098, below the old 0.002 threshold, so a one-word edit verified as
  "nothing happened". Now thresholded on changed-cell count.
- **The MCP server refused to start** when the accessibility bus was unreachable
  - an ordinary condition on Linux. The connection is now lazy: the server
  starts, answers `tools/list`, and the first real call returns an actionable
  error naming the fix.
- **`ghost doctor` failed on an empty desktop.** Zero accessible windows is
  ambiguous (accessibility off, or nothing open) and reporting FAIL sent users to
  change a setting that was already correct. It now warns.
- Editable fields are matched by the `EditableText` interface rather than the
  role name, because toolkits disagree about whether a text entry is `entry` or
  `text`.
- `quinn-proto` bumped to 0.11.16, clearing a high-severity advisory (Ghost has
  no reachable path to it, but the lockfile carried the vulnerable version).

### Not yet verified

The Wayland paths - RemoteDesktop portal input and Screenshot portal capture -
are implemented and compile-verified but have not run on hardware; CI runs X11.
See `docs/linux-fedora.md` section 3.

## [0.16.0] - 2026-07-19 — Shell control (terminal / PowerShell / CLIs)

### Added

- **`ghost_shell`** — run terminal commands and drive persistent shells, so an
  agent can do more than click GUIs: run builds, git, and CLIs, edit files on
  machines with no file tools, and open apps (including spawning a new Claude
  Code session) from a command line.
  - `op=run` (default): one-shot. Spawns `powershell` (default), `pwsh`, or `cmd`,
    runs `cmd`, returns `{output, exit_code, timed_out, duration_ms}`. Merged
    stdout+stderr, tail-capped at 24000 chars. Kills the process on timeout.
  - `op=open` / `send` / `read` / `kill` / `list`: **persistent PowerShell
    sessions** whose variables, cwd, and env persist across commands. A timed-out
    `send` leaves the command running (`busy:true`); `read` drains the rest.
  - Injection-safe framing: each command is base64-encoded and the persistent
    driver decodes it, so arbitrary command text (quotes, newlines, `$`) is safe.
    A per-session nonce sentinel means a late reply from a timed-out command can
    never be misread as a later command's result.
  - Emergency-stop (`ghost_stop` / Ctrl+Alt+G) kills a runaway command mid-wait.
  - **Kill-switch:** set `GHOST_SHELL=off` to disable the verb entirely; every op
    returns a clear error. Ghost is public — deployments that want GUI automation
    without shell access can opt out.
  - To start a new Claude Code session:
    `ghost_shell op=run cmd='Start-Process wt -ArgumentList "pwsh","-NoExit","-Command","claude"'`,
    then drive the terminal window with `ghost_see` / `ghost_act` / `ghost_key`.
  - Live-verified end to end: one-shot exit codes captured (`exit 7` → 7),
    variable + cwd persistence across sends (`$x=41` then `$x+1` → 42, `Set-Location`
    survives), timeout→busy→read drain, kill, and `GHOST_SHELL=off` refusal.
  - 20 lean verbs now advertised (was 19). Boot latency unchanged (~20ms).

## [0.15.1] - 2026-07-05 — Snapshot knows enabled/disabled state

### Changed

- **`ghost_snapshot` now reports `enabled`** per element, and `actionable` means
  "interactable role AND currently enabled" — so a greyed-out button reads
  `actionable: false` and `actionable_only` drops it. An agent won't waste a call
  clicking a disabled control. Nearly free: `IsEnabled` was already in the UIA
  cache batch. `ElementDescriptor` gained an `enabled` field.
  - Live-verified: a fresh Calculator snapshot marked the three disabled memory
    buttons (Recall / Clear / flyout) `enabled:false, actionable:false` while
    digit buttons stayed `enabled:true` (33 of 36 actionable).

## [0.15.0] - 2026-07-05 — Structured agent-planning snapshot

### Added

- **`ghost_snapshot`** — a structured, agent-planning view of a window's UI. Each
  element comes back with a stable `id`, `name`, `role`, `rect`, `center`, an
  `actionable` flag, and the `actions` it accepts (`click` / `type`), plus
  `actionable_count`. `actionable_only=true` filters to just the interactable
  elements. This lets an agent plan over structure — far cheaper in tokens than a
  screenshot — then `ghost_act` by name/role. Read-only; no foreground change.
  - Live-verified: a Calculator snapshot returned 36 actionable elements with
    correct centers and click actions.

## [0.14.0] - 2026-07-05 — Background clipboard/edit shortcuts

Closes the most-used part of the "no modifier combos in background" limit.

### Added

- **Background Ctrl+C / Ctrl+X / Ctrl+V / Ctrl+A / Ctrl+Z** via `ghost_key
  background=true`. The common editing shortcuts are dispatched as their semantic
  window messages (`WM_COPY` / `WM_CUT` / `WM_PASTE` / `WM_UNDO`, `EM_SETSEL` for
  select-all) instead of a posted modifier+key — so they work reliably in the
  background, without the `GetKeyState` problem that makes raw combos unreliable.
  No foreground, no cursor. Other combos (Ctrl+S, custom accelerators) are still
  rejected — those genuinely can't be posted reliably.
  - Live-verified: typed text into a background charmap edit, then background
    Ctrl+A + Ctrl+C, and the clipboard read back the text (replacing a sentinel)
    while Calculator kept the foreground.

New primitive: `BackgroundClicker::edit_command` + `EditCommand`. +3 unit tests.

### Honest remaining limits (not closeable here, by design)

- Windowless UWP/WinUI/Chromium controls (no window handle) can't be driven truly
  in the background by any tool — Ghost falls back to UIA and flags it.
- Arbitrary modifier combos beyond the clipboard/undo family.
- A hosted multi-tenant Windows runner is a separate product/infra effort, not a
  library feature.

## [0.13.0] - 2026-07-05 — Full background input + model-agnostic vision

Closes the gaps left by v0.12.0's background dispatch.

### Added

- **Background `double_click` / `right_click` / `hover`** (`ghost_act
  background=true`). Posted mouse messages on windowed controls
  (`WM_LBUTTONDBLCLK`, `WM_RBUTTONDOWN/UP`, `WM_MOUSEMOVE`) at the element's
  `ScreenToClient` centre — no foreground, no cursor. double_click reuses the
  element-ROI PrintWindow verify; right_click/hover report `verified: null` with a
  note (a context menu is a separate popup; posted hover has no OS cursor).
  Windowless controls error instead of stealing focus.
- **Background keyboard** (`ghost_key background=true`). Posts a single key to the
  target window's focused control (found via `GetGUIThreadInfo`) with no
  foreground/cursor change — printable chars as `WM_CHAR`, named keys
  (Enter/Tab/F-keys/arrows) as `WM_KEYDOWN`/`UP`. Modifier combos are rejected
  (posting can't set the modifier state apps read via `GetKeyState`).
  Live-verified: a char posted to a background charmap edit read back correct
  while Calculator kept the foreground.
- **Model-agnostic vision.** Description grounding works with any OpenAI-compatible
  endpoint (NVIDIA, OpenAI, Gemini, Groq, local vLLM/Ollama/LM Studio) or
  Anthropic. Key resolution is provider-agnostic (`GHOST_VISION_API_KEY` >
  `OPENAI_API_KEY` > `NVIDIA_API_KEY`); a keyless local server needs only
  `GHOST_VISION_BASE_URL`. Verified against a capture server: the request carried
  the custom model + generic bearer key + image.

### Changed

- README: softened the "unblockable" framing to a plain description of how Ghost
  works, plus an authorized-use note.

New primitives: `BackgroundClicker::{double_click_screen, right_click_screen,
hover_screen, send_key, send_char, focused_control}`. +6 unit tests (null-hwnd
guards for every new primitive).

Adversarial review found + fixed before push: `send_key` now sets the
extended-key bit (24) for the nav cluster (arrows/Home/End/PageUp·Down/Ins/Del)
and synthesizes `WM_CHAR` for Enter/Tab/Backspace (a posted `WM_KEYDOWN` alone
won't edit text without a message pump); `ghost_key background` now reports
`focused_control: false` with a clear note when nothing held focus and the key
went to the frame; the vision key is trimmed before use.

## [0.12.0] - 2026-07-05 — Background dispatch (agent-harness mode)

Drive an app WITHOUT bringing it to the foreground or moving the cursor — so an
LLM agent can operate a Windows app while the human keeps working. This is the
capability agent harnesses (OpenClaw, Hermes/cua-driver) mount as their
computer-use layer; Ghost adds per-action verification on top.

### Added

- **`ghost_act background=true`** (requires `window` + `name`/`role`; `click`/`type`).
  Acts on a control inside a named window with NO foreground change and NO cursor
  movement, then verifies — all without the window being visible:
  - **True background via posted window messages.** Real Win32 controls (which
    expose a native window handle) are driven with `BM_CLICK` / `WM_LBUTTONDOWN/UP`
    (click) and `WM_SETTEXT` (type) — these do not activate the window, unlike UIA
    `Invoke`/`SetValue`, whose providers bring the window to the foreground.
  - **Occlusion-proof verification.** `type` is confirmed by reading
    `ValuePattern.CurrentValue` back; `click` by a `PrintWindow(PW_RENDERFULLCONTENT)`
    before/after delta that renders even an occluded/background window.
  - **Honest reporting.** The response carries `verified`, `focus_preserved`, and
    `cursor_preserved`. Windowless controls (UWP/WinUI/Chromium — no HWND) fall
    back to UIA dispatch, which activates the window; the response flags this and
    `focus_preserved` reports it truthfully. This is a real limitation of every
    tool, not just Ghost — you cannot post a message to a control that has no
    window handle. Classic Win32 line-of-business apps (the no-API automation
    target) drive cleanly in the background.
  - Verified live: charmap (Win32) click AND type while Calculator held the
    foreground — `verified=true, focus_preserved=true, cursor_preserved=true`,
    foreground never moved. cargo test --workspace 370 passed / 0 failed.

New primitives: `ghost_core::input::BackgroundClicker::{button_click, set_text}`,
`ghost_core::capture::capture_window_printwindow`,
`UiaElement::native_window_handle`, and strict UIA-only `invoke_ex`/`set_value_ex`
(no coordinate fallback).

## [0.11.0] - 2026-07-05 — Capture latency (measured, corrected), canvas vision, soak

Four evidence-driven improvements. Notably, end-to-end measurement corrected the
v0.10.0 assumption about the fast capture path.

### Changed

- **Region captures route through GDI, not DXGI (up to ~5x faster per action on
  large windows).** v0.10.0 optimized the DXGI region *convert* (~20x on that
  step). But `tests/capture_latency_probe.rs` (release, added here) shows the DXGI
  *acquire* dominates: DXGI must acquire+map a whole desktop frame regardless of
  region size and hits a **70-83ms cliff for a 1600x900 window**, while GDI BitBlt
  of just the rect is flat **~16.5ms** at any size. act-verify captures the
  foreground window ~5x per action, so large/maximized windows see up to ~5x less
  capture latency. The act-verify, screenshot, and Set-of-Marks paths all route
  region rects through GDI now; full-screen still uses DXGI (cached duplicator
  wins there). No correctness change — GDI BitBlt already backed the DXGI path as
  its universal fallback, and it works on any monitor.
  - Consequence: the originally-planned per-output DXGI duplicator for secondary
    monitors was **dropped** — evidence shows DXGI region is the *slower* path, and
    it was unverifiable on a single-monitor box anyway. GDI already handles any
    monitor.

### Added

- **GPU-free CPU element detector for canvas / no-accessibility-tree apps.** A new
  always-compiled classical-CV detector (`ghost_ground::cv_detect`) proposes
  element-like boxes from pixels alone (edge density -> connected components ->
  size/aspect filter -> OmniParser-style de-nest) — no GPU, no model, no deps.
  `build_marks` augments Set-of-Marks candidates with these regions only when the
  UIA tree is sparse (<4 elements), so custom-drawn UIs, remote-desktop surfaces,
  and game canvases become markable while normal apps are untouched. Set-of-Marks
  geometry moved to a shared `marks` module; `Tier::Cv` labels these detections.
  Verified on synthetic images (exact bbox recovery, noise filtered) and a real
  800x600 desktop capture (35 icon/button-sized regions). NOT OmniParser (that
  needs a GPU model); the CV-marks -> VLM-pick end-to-end needs a configured
  vision key and was not live-run here.
- **Reliability soak harness (`bench/soak.py`).** Drives the real ghost-mcp binary
  through many act-then-verify cycles and gates on verify-null rate, verify-false
  rate, focus-loss rate, error rate, effect-mismatch (display re-observed), and
  latency percentiles. First run (160 acts): PASS — verify-null 0.0, focus-loss
  0.0, effect-mismatch 0 (100% correct), p50 85ms / p95 117ms. Would have caught
  the v0.10.0 static-screen regression (verify-null spike). `--self-test` proves
  the harness can fail.

## [0.10.0] - 2026-07-04 — Region Capture (the measured latency win)

The one raw-performance optimization deferred across several versions — now
measured, implemented, and verified rather than claimed.

### Changed

- **Capture converts only the pixels it needs.** Every `ghost_act` captures the
  foreground-window rect up to ~5 times (a before-frame plus verification polls).
  Previously each capture converted the ENTIRE monitor's BGRA→RGBA buffer (and
  cloned it for the static-screen cache) and then software-cropped to the window.
  Now an on-primary region capture converts only the requested sub-rect.
  - Measured (`cargo bench -p ghost-core --bench convert`, 1080p): full-frame
    convert **~4.06 ms** → 400x300 region convert **~206 µs** — a **~20x** cut
    in per-capture conversion cost, which runs several times per action.
  - Also skips the full-frame RGBA clone on region captures (a ~33 MB alloc at 4K).
  - Live-verified pixel-correct end-to-end: action verification returns
    verified=true and Set-of-Marks grounding still lands exactly on target (Equals
    and Plus, 2/2). A unit test proves the region convert is byte-identical to
    full-convert-then-crop across offsets and row-pitch padding.
  - Off-primary and full-screen paths are unchanged; only the on-primary
    region path (used by act-verification and SoM capture) is optimized.

### Fixed

- **Region captures no longer silently disable act-then-verify on a static
  screen** (adversarial-review finding). Region captures don't warm the
  full-frame cache, so a DXGI `AcquireNextFrame` timeout on an unchanging screen
  (the common case for a "before" frame or a no-visible-delta action) had no
  cached frame to crop and returned an error — which the session layer swallowed
  into a null `verified`, defeating the double-action guard. The crop path now
  falls back to the GDI region capture on that timeout, always returning a real
  frame. Live-verified: two back-to-back captures of a fully static window both
  return byte-identical valid frames.
- **Stale-cache crop is re-clamped against the cached frame's own dimensions**,
  not the current monitor size, so a resolution/DPI change between the last full
  capture and a timeout can't return a partially-black region as `Ok`; a
  degenerate crop now errors and routes to a fresh GDI region capture.

## [0.9.1] - 2026-07-04 — Selection + Scroll-Until Primitives

### Added

- **Read text selection without clobbering the clipboard** — `ghost_see
  mode=selection` (name/role) reads an element's current text selection via UIA
  TextPattern. Lets an agent confirm/read what's selected before copy/delete/
  format, without a Ctrl+C round-trip that would overwrite the clipboard. Native
  edit/RichEdit/document controls; browser controls often don't expose
  TextPattern (documented). Live-verified: read 'select-this-text' from Notepad.
- **`ghost_scroll` until-mode** — pass `until_name`/`until_role` to scroll the
  foreground window repeatedly until that element becomes visible (long or
  virtualized lists), up to `max_scrolls` (capped at 100). Returns found=true/
  false. The one thing linear `ghost_run` steps couldn't express. Live-verified:
  returns fast when already visible, bounded-false when absent.

## [0.9.0] - 2026-07-04 — Optimization + Capability Batch

Found via a three-pass codebase audit (performance, capability, robustness).

### Fixed

- **Memory leak in the locator cache.** The in-memory `LocatorCache` had no size
  cap and its `clear()` was never called, so a long session against a dynamic UI
  (infinite-scroll list, SPA) grew the map without bound. Now capped at 4096
  entries (sweep expired, then evict oldest). Bounded-growth test added.
- **OCR now works on secondary monitors.** `ghost_find text=`/OCR previously
  hard-errored "virtual-desktop capture not yet supported" for any window on a
  non-primary monitor; it now routes through the same multi-monitor GDI capture
  `ghost_screenshot` already used.

### Added

- **`ghost_stats`** — grounding + cache telemetry (which tier wins, VLM
  escalation rate, cache hit/miss) is now a discoverable lean tool, not just a
  hidden alias. Call it to debug why a flow is slow or a find is flaky.
- **`ghost_wait for=value`** — wait until an element's value equals/contains/
  changes (forms, async fields, "wait until the total updates") — a common
  flow-blocker with no primitive before.
- **`ghost_drag` by element** — endpoints can be `from_name`/`to_name` (etc.),
  resolved to element centers like click, not just raw coords.
- **`ghost_see mode=marks`** — returns the Set-of-Marks annotated screenshot the
  VLM sees when grounding by description, plus the numbered label list. The
  fastest way to diagnose "why did vision grounding pick the wrong element".

### Removed / cleaned

- Deleted the unused SQLite `LocatorStore` (279 LOC, zero call sites across 3
  versions) and dropped its `rusqlite` + `tempfile` dependencies — smaller,
  faster builds. Applied `cargo clippy --fix` (mechanical warnings).

## [0.8.0] - 2026-07-03 — Set-of-Marks Vision Grounding

### Added

- **Set-of-Marks visual grounding** for description-based `ghost_find` and VLM
  escalation. Instead of asking the model to regress raw pixel coordinates
  (unreliable — a plain "coordinates of the equals button" landed ~250px off in
  testing), Ghost overlays numbered badges on the window's detected elements,
  sends the marked screenshot plus each badge's accessible-name label, and asks
  the model which *number* matches. The number maps back to that element's exact
  rect. Live-verified on Calculator: four natural-language descriptions each
  landed exactly on the correct button (vs ~250px off before).
  - New `ghost-core` mark renderer (`capture/marks.rs`) — numbered badges drawn
    with a hardcoded bitmap font, zero new dependencies.
  - Falls back to the previous coordinate-regression path when a window has no
    detectable elements to mark, so nothing regresses.

Honest scope: labels carry most of the disambiguation on well-named apps; the
visual badges carry unlabeled icons. Truly a11y-invisible elements (pure canvas)
still need a local visual detector, which is GPU-dependent and not shipped here.

## [bench] - 2026-07-03 — Benchmark self-test + broader coverage

(No binary change — `ghost-mcp` stays 0.7.7; this expands the `bench/` suite.)

- **Negative-control self-test** (`python bench/run_bench.py --self-test`): runs
  deliberately-wrong scenarios (assert the display reads 99 when it reads 42, a
  missing element scored as found, junk bytes scored as an image) and passes only
  if the harness scores every one as FAIL. Proves the benchmark actually detects
  failure — the 14/14 green run is a real signal, not a rubber stamp.
- **+2 tasks (12 → 14)**: window minimize/restore (verify the state really
  changes), and a clipboard set/get round-trip.
- **Clipboard safety**: the harness now saves the user's clipboard before the run
  and restores it after, so running the benchmark never clobbers what you'd
  copied.

## [0.7.7] - 2026-07-02 — Reproducible Benchmark + Symbol Keys

### Added

- **`bench/` — a reproducible, honest benchmark.** Drives the real `ghost-mcp`
  binary through 12 Windows desktop tasks and scores each by re-observing the
  actual result (e.g. the Calculator display really reads 42), not by trusting a
  tool call returned ok. Self-contained, runs on any Windows 10/11 box, exit 0
  iff all pass. Ships Ghost's own measured numbers only (12/12, ~2.5s/task) plus
  an honest protocol for comparing to other tools without fabricating their
  columns. See `bench/README.md`.

### Fixed (found by the benchmark on its first run)

- **`ghost_key` / `ghost_press` can now send symbol keys** (`*`, `/`, `-`, `.`,
  `=`, etc.). Previously only named keys and a few OEM symbols had a VK mapping,
  so `keys="*"` (multiply) was a silent no-op — an agent typing any operator hit
  it immediately. A single character with no VK mapping is now sent as a Unicode
  character (layout-independent, exact glyph); multi-char unknown names still error.

## [0.7.6] - 2026-07-02 — Stuck-Modifier Safety

### Fixed (found by convergence audit)

- **`hotkey` can no longer leave a modifier stuck down**: if a modifier key-down
  succeeded (e.g. Ctrl) but a later one failed (e.g. Shift in Ctrl+Shift+T), the
  early return skipped the release loop, leaving Ctrl physically held — which
  corrupts all subsequent keyboard input system-wide. Every exit path now
  releases the modifiers already pressed.
- **`ghost_stop` now releases held modifiers immediately** on arrival (in the
  stdin reader fast-path), instead of only when the queued stop later dispatches
  — so a stuck Ctrl/Shift/Alt from an in-flight or held `key_down` is cleared at
  once.

## [0.7.5] - 2026-07-02 — Paste Fallback for Rich-Text Editors

### Added

- **Clipboard-paste fallback for `type`**: when a `type` still shows no change
  after keystroke retries AND the target is an editable control, Ghost escalates
  to a real paste (save clipboard → set → Ctrl+V → restore). This is the path
  rich-text web editors (Monaco, ProseMirror, Slate) accept when they ignore
  both ValuePattern.SetValue and synthesized keystrokes. Made idempotent
  (select-all before paste = replace) so a false-negative verification can't
  double the text, gated to editable roles, and the original clipboard is always
  restored. Results carry `used_paste_fallback` when it fires.

## [0.7.4] - 2026-07-02 — Hardening + Flow Chaining

### Fixed (found by convergence audit)

- **`ghost_see`/describe can no longer hang the server**: `collect_interactive`
  gained a 6000-node budget (the other UIA walkers already had one). A wide,
  shallow accessibility tree (big list, Chromium DOM) previously walked in full,
  blocking the single-threaded server uninterruptibly.
- **`ghost_key "Ctrl++"`** (Ctrl+Plus / zoom) now parses correctly — the old
  parser rejected the exact syntax its own error message recommended. A single
  trailing `+` (e.g. `"Ctrl+"`, a truncated combo) correctly errors instead of
  silently firing Ctrl+Plus.
- **`ghost_screenshot_region` with an inverted rect** (e.g. `[50,0,-1,100]`) now
  errors instead of clamping the negative edge to the screen edge and silently
  returning a huge region.

### Added

- **`ghost_run` step chaining**: a param value of `"${steps.N.path}"` is replaced
  with a field from step N's result before dispatch (e.g. find an element in step
  0, then `ghost_click_at` at `"${steps.0.center.x}"`). Whole-string refs keep
  their type; embedded refs are stringified; unresolved refs are left verbatim.
- **`ghost_assert value-equals` / `value-contains`**: compares an element's actual
  value (ValuePattern) to expected text — the fill-then-verify check.
- **`ghost_screenshot` element/region crop**: pass name/role to capture one
  element, or rect=[l,t,r,b] for a region (VLM-in-the-loop debugging).

### Tests

- 346 passing (was 341). New coverage for key parsing, step-ref substitution,
  and inverted-rect rejection.

## [0.7.3] - 2026-07-02 — Actionability, Waits, Structured Errors

### Added

- **`ghost_wait for=element`**: wait for an element (by name/role) to appear or
  disappear WITHOUT clicking anything first — the "wait until Save exists"
  primitive agents constantly need. Event-bus-driven backoff.
- **Structured errors**: every failing tool call now carries an `error_code`
  and a `suggested_action` (e.g. element-not-found → "call ghost_see to confirm
  focus, or retry with mode=deliberate"), classified from the error. Was a bare
  opaque string with a single generic -32000 code.
- **`ghost_query` provenance + correctness**: now reads each field's VALUE
  (ValuePattern/get_text) instead of echoing the element's NAME (which returned
  labels like "Email:" instead of the actual value), and reports a per-field
  `sources` map (uia|vlm).
- **Clear-before-type**: `type` now replaces existing field content instead of
  appending, on the keyboard-fallback and coordinate paths too (UIA
  ValuePattern already replaced). Gated behind an editable-role check so a
  mis-grounded type can never fire Ctrl+A+Delete on a file list / non-text focus.
- **Retry-until-verified for `type`**: an unverified `type` re-dispatches once
  (safe — SetValue/clear-then-type are idempotent). Click/double/right-click are
  deliberately NOT auto-retried: a slow-but-successful click must never be
  double-fired (double-submit/charge/delete). Results carry an `attempts` count.
- **Occlusion diagnostic**: coordinate-dispatch actions report `hit_element`
  (what actually sits at the click point) so a mis-hit is diagnosable.

### Tests

- 338 passing (was 337). New coverage for error classification.

## [0.7.2] - 2026-07-01 — Multi-Monitor & Interaction Robustness

### Added / Fixed

- **Multi-monitor capture**: screenshots and act-verification now work on ANY
  monitor. The DXGI fast path only duplicates the primary output; rects that
  fall off-primary (or straddle a monitor boundary) now route to a GDI
  virtual-screen capture that spans the whole desktop (negative coordinates for
  monitors left of / above the primary included). Previously an action or
  screenshot on a secondary monitor silently cropped garbage from the primary.
- **Scroll-into-view before acting**: `ghost_act` detects UIA-offscreen elements
  (scrolled out of a list, collapsed, hidden tab) and calls ScrollItemPattern +
  a 60ms settle before re-reading the rect, so clicks in long/virtualized lists
  land on the real element instead of empty space.
- **Disabled-control guard on all actions**: `double_click`/`right_click`/`hover`
  now fail fast on a disabled element like `click`/`type` already did (was a
  silent no-op returning ok).

### Tests

- 337 passing (was 334). New coverage for virtual-screen / on-primary rect
  routing helpers.

## [0.7.1] - 2026-07-01 — Targeting, Reading, Preemption

### Added

- **`window` param on `ghost_find`/`ghost_act`**: focuses + confirms the target
  window (title substring) before resolution, so UIA fast paths, OCR crops, and
  cache keys all anchor to the INTENDED window in multi-window flows — closes
  the remaining wrong-window-first-match hole.
- **`index` param on `ghost_find`/`ghost_act`**: act on / return the nth match
  (0-based) when several elements share a name/role (e.g. multiple "Close Tab"
  buttons); name+role AND-combine on this path and responses carry a `matches`
  count. Out-of-range gives an actionable error with the match count.
- **`ghost_see mode=text`**: extract the readable text of a window/page
  directly from the accessibility tree (names of text-carrying roles +
  ValuePattern content of edit/document). The cheapest way to READ a page —
  no screenshot, no element dump. `limit` = char cap (default 20000).
- **Preemptible emergency stop**: stdin now runs on a dedicated reader thread;
  a `ghost_stop` request sets the stop flag the moment it ARRIVES instead of
  waiting in the serial queue. Live-verified: a queued 10s `ghost_wait` is
  interrupted in ~1ms. `ghost_wait` also polls the stop flag (100ms) and
  reports interruption as an error instead of sleeping through it.
- **Content-change WinEvent hooks** (`EVENT_OBJECT_VALUECHANGE` / `REORDER` /
  `SHOW`): find()/wait primitives now wake on text updates, list mutations,
  and elements becoming visible instead of falling back to 25-150ms polling;
  a 10ms debounce on the wake path prevents walk-thrash during event bursts.

### Tests

- 333 passing (was 330). Live e2e: stop preemption 1ms, mode=text reads typed
  content, index disambiguation with matches count.

## [0.7.0] - 2026-07-01 — Reliability & Latency Overhaul

Root-caused and fixed the three classes of field failures: actions that need
multiple calls to fire, silent wrong-window input, and perceived lag.

### Fixed — actions that "don't fire"

- **OS-foreground anchoring on every action path.** `ghost_act` now brings the
  target element's own window to the foreground (AttachThreadInput + confirm)
  before dispatching input. Previously only UIA `SetFocus()` was attempted —
  it fails silently for a background console process, so SendInput fallbacks
  (double_click, right_click, pattern-miss typing) landed in whichever window
  had focus, usually the MCP client's own terminal.
- **`ghost_key` gained a `window` param.** Keyboard SendInput routes to the OS
  focus owner; with `window` set the target is focused + confirmed first and
  the call FAILS LOUDLY if focus can't be confirmed, instead of typing into the
  wrong app and returning ok:true.
- **Honest verification.** `ghost_act` responses now carry `verified`
  (screen-delta detected), `focus_confirmed`, and a `warning` when an action
  dispatched but nothing visibly changed. Previously `ok:true` was hardcoded,
  so silent no-ops looked like success and clients re-issued the action.
- **Coordinate-tier actions (OCR/VLM) get the same verification** — that path
  previously had none at all.
- **Adaptive post-action verify window** (40→240ms early-exit polling) instead
  of a single fixed 50ms capture that false-negatived async renders
  (web/Electron).

### Fixed — windows agents lose track of

- `ghost_window op=list` now includes minimized windows (with a `state` field:
  normal|minimized) — Win11 cloaks some minimized windows (e.g. Notepad) and
  they vanished from the list entirely while still alive.
- `ghost_window op=focus` auto-restores minimized windows before focusing.
- `ghost_see mode=full window=X` with an unknown window is now an ERROR that
  lists the open windows — previously it silently walked the ENTIRE desktop and
  returned a huge dump including -32000 garbage coords from minimized windows.
- Minimized-window scope requests return an actionable error ("restore it
  first") instead of garbage coordinates.

### Fixed — latency

- **VLM timeout 30s → 8s** (configurable via `GHOST_VLM_TIMEOUT_MS`), with one
  bounded retry on connect errors/5xx (never on timeout). A silent VLM
  escalation could previously block the serial stdio loop — and every queued
  request behind it — for 30 seconds.
- `ghost_find`/`ghost_act` responses expose `escalated: true` when local tiers
  missed and a network VLM call was paid, so hidden latency is visible.
- **On-device OCR bounded at 3s** (WinRT spin-wait previously had no timeout).
- **UIA desktop walks capped at 3000 visited nodes** — a DOM-heavy Chromium
  window could previously turn one find into an unbounded COM-call storm.
- **DXGI black-frame flag is no longer permanent**: re-probes every 100 GDI
  captures so a transient event (sleep/resume, driver reset) doesn't downgrade
  every future screenshot to the 30-100ms GDI path forever.
- `ghost_screenshot full=true` returns a 1280px JPEG by default instead of a
  native-resolution lossless PNG (multi-MB base64 over stdio); pass `max_dim=0`
  for the old behavior.
- `ghost_see`/describe responses filter zero-area and off-screen elements and
  cap at 150 elements by default (`limit` param, 0 = unlimited) — element dumps
  were the top source of client-side context bloat.
- Every `tools/call` response envelope now includes `ms` (server-side latency).
- Locator cache entries expire after 30s (TTL) — bounds the window where a
  re-rendered UI could pass point-validation on a coincidentally-matching
  element and cause a wrong-target click.
- Tokio runtime pinned to `current_thread` flavor, making the COM-STA
  single-thread invariant structural instead of accidental.

### Tests

- 330 passing (was 320), including new coverage for element filtering/limits,
  act-result honesty (verified/warning), verification sensitivity (typed-text
  detection, noise tolerance), and cache behavior.

## [0.5.0] - 2026-05-07 — Local OCR + Multi-Provider Vision

### Added

- **NVIDIA Build vision provider** (default): OpenAI-compatible client in
  `ghost-session/src/vision.rs`. Uses `meta/llama-3.2-90b-vision-instruct`
  by default at `https://integrate.api.nvidia.com/v1/chat/completions`.
  Free with NVIDIA Developer signup (~zero local disk).
  Set `NVIDIA_API_KEY`. Falls back to Anthropic if key absent.
- **Multi-provider selection**: `GHOST_VISION_PROVIDER=nvidia|anthropic`
  for explicit choice. `GHOST_VISION_BASE_URL` overrides for self-hosted
  Ollama / vLLM / llama.cpp servers (any OpenAI-compat endpoint).
  `GHOST_VISION_MODEL` overrides per-provider default.
- Robust JSON response parsing: handles bare JSON, code-fenced JSON, and
  prose-wrapped JSON across providers.
- 5 new unit tests on response parsing + provider selection (97 total).
- **Windows.Media.Ocr integration** (`ghost-core/src/ocr.rs`):
  on-device, free, no API. SoftwareBitmap built from raw BGRA via
  IBufferByteAccess memcpy. OcrEngine init via TryCreateFromUserProfileLanguages
  with en-US fallback. Returns Vec<OcrWord{text, BoundingRect}> in absolute
  screen coords.
- Session methods: `find_text_local(needle, foreground)` and
  `click_text_local(needle, timeout_ms)` with event-driven backoff.
- 2 new MCP tools: `ghost_find_text_local`, `ghost_click_text_local`.
  Pair with vision fallback: try OCR first (free, ~50-200ms), then
  ghost_locate_by_description (paid API, ~1-3s) only on miss.
- Public helpers in `capture/screen.rs`: `capture_screen_full_rgba`,
  `rgba_to_bgra_in_place` for non-encoding consumers.

### Dependencies

- `windows-core = "0.58"` direct dep on ghost-core (required by
  `windows::core::implement` macro expansions).
- Workspace `windows` features added: `Foundation`, `Foundation_Collections`,
  `Globalization`, `Graphics_Imaging`, `Media_Ocr`, `Storage_Streams`,
  `Win32_System_WinRT`.

### Deferred from v0.5

- **Full IUIAutomation event handlers**: windows-rs 0.58 does not
  auto-generate `_Impl` traits for `IUIAutomationFocusChangedEventHandler`
  / `IUIAutomationStructureChangedEventHandler`. Implementing them needs
  raw VTABLE-level COM (200+ LOC unsafe, deadlock risk on UIA threadpool,
  SAFEARRAY param marshaling). `EventBus::bump()` retained as the public
  hook for when this gets wired (~5 line integration).
- **LocatorStore read path**: write path is trivial; the value comes from
  the read path which needs `IUIAutomation::ElementFromPoint` integration
  to verify a cached rect before skipping the walk. Without that,
  populating the store gains nothing.

## [0.4.0] - 2026-05-07 — Hot Path Overhaul

Targets browser-automation-grade latency for the desktop. Six-phase upgrade to
the action loop, screenshot pipeline, and locator system. 92 tests passing.

### Added

- **Scoped UIA search** (`tree.rs`): `find_by_name_fast` / `find_by_role_fast` /
  `describe_screen_fast` use `IUIAutomation::ElementFromHandle` rooted at the
  foreground HWND. Typically 10-100x faster than the prior full-desktop walk;
  falls back to desktop scope on miss.
- **System event bus** (`uia/event_bus.rs`): `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)`
  on a dedicated pump thread populates a global `EventBus` with sequence
  counter + `tokio::sync::Notify`. `wait_for_change(since_seq, timeout_ms)`
  is spurious-wakeup-safe and races to <5ms wakeup latency.
- **Region/JPEG capture** (`capture/screen.rs`): `capture_screen_region(rect, max_dim, format)`
  + `CaptureFormat::{Png, Jpeg(quality)}`. Refactored DXGI path into shared
  `capture_rgba` helper. Recommended vision payload preset:
  foreground rect + max_dim=768 + jpeg q75 = 10-50x smaller than full PNG.
- **Vision fallback** (`session/vision.rs`): `By::Description("the blue Submit button")`
  routes through Claude Messages API with model tiering (Haiku default, Opus
  via `GHOST_VISION_MODEL`). Capture → ROI → downscale → vision_locate → coord
  back-translation. New session methods: `locate_by_description`,
  `click_by_description`, `type_by_description`. Requires `ANTHROPIC_API_KEY`.
- **8 new MCP tools** (45 total):
  - `ghost_describe_screen_fast` — foreground-scoped describe.
  - `ghost_screenshot_region` — ROI + downscale + JPEG/PNG selection.
  - `ghost_event_seq` / `ghost_wait_for_event` — direct event-bus access.
  - `ghost_locate_by_description` / `ghost_click_by_description` /
    `ghost_type_by_description` — vision fallback ergonomic surface.
  - `ghost_batch_actions` — single MCP round-trip for N ops, replaces
    multiple sequential tool calls in agent flows.
- **Tracing instrumentation** on `find`, `click_and_wait_for_text`,
  `screenshot_region`, `wait_for_event`, `describe_screen_fast` with
  per-action lookup-microsecond fields. `RUST_LOG=ghost_session=debug` for
  per-call latency.

### Changed

- `find()` polling: fixed 100ms → tiered backoff (25ms warm / 75ms / 150ms),
  now driven by `EventBus::wait_for_change` for event wakeups during the
  backoff window.
- `click_and_wait_for_text` and FSM `Op::WaitForText`: replaced full
  `describe_screen` rewalks with scoped `find_by_name_fast` probes.
  ~50x cheaper per poll.
- `By` enum gained `Description` variant for vision-bound locators.
- Workspace `image` dep gains `jpeg` feature.

### Fixed

- Spurious-wakeup safety in `EventBus::wait_for_change`: re-checks `seq` after
  each `Notify` wake; only returns Ok when seq has actually advanced past
  `since_seq`.

## [0.3.0] - 2026-04-18 — Speed Overhaul

### Added

- **`ghost-cache` crate**: event-driven UIA mirror with snapshot/delta API, 8-slot
  history ring, SQLite-backed `LocatorStore` with schema v1 and cold/warm/drift
  lookup + eviction.
- **`ghost-intent` crate**: JSON intent compiler, JSONLogic subset evaluator, FSM
  executor with `abort_if` / `retry_if` + exponential backoff + deadline gate.
- **`StaPool`**: STA-threaded UIA worker pool with `catch_unwind` panic recovery,
  3-panics-in-60s circuit breaker, and per-job tokio timeout.
- **`CachedTreeWalker`**: batched `IUIAutomationCacheRequest` + `FindAllBuildCache`
  for 10 UIA properties in one round-trip.
- **`IdleDetector`**: blake3-hashed frame capture with stable-frame detection.
- **`BackgroundClicker`**: PostMessage-based `WM_LBUTTONDOWN/UP` with `IsWindow` gate.
- 10 new MCP tools: `ghost_wait_until`, `ghost_wait_for_idle`, `ghost_navigate_and_wait`,
  `ghost_click_and_wait_for_text`, `ghost_fill_form`, `ghost_execute_intent`,
  `ghost_describe_screen_delta`, `ghost_click_background`, `ghost_cache_stats`,
  `ghost_cache_invalidate` — total 37.
- sonic-rs response encoder (3-5x faster on large payloads) with serde_json fallback.
- Criterion benches (`cargo bench -p ghost-intent`) and `docs/benches/v030-baseline.md`.
- `chaos` feature flag for failure-injection tests.

### Changed

- `OpsDispatcher` trait is `?Send` to accommodate `!Send` COM handles on `GhostSession`.
- `ghost-mcp` `recursion_limit = "512"` to fit the 37-tool `json!` macro.

### See

- Design: `docs/2026-04-17-ghost-v030-speed-overhaul.md`
- Plan: `docs/plans/2026-04-18-ghost-v030-speed-overhaul.md`

## [0.2.0] - 2026-04-17

### Added

- `ghost_reset` MCP tool: resume automation after `ghost_stop`
- MCP protocol compliance: `initialize`, `initialized`, and `tools/list` methods
- `tools/list` returns full inputSchema for all 25 tools (MCP 2024-11-05 spec)
- 17 new MCP tools bringing total to 25 (full human-input parity)
- **Input:** `ghost_press`, `ghost_hotkey`, `ghost_key_down`, `ghost_key_up`
- **Mouse:** `ghost_hover`, `ghost_right_click`, `ghost_double_click`, `ghost_drag`, `ghost_scroll`
- **Clipboard:** `ghost_get_clipboard`, `ghost_set_clipboard`
- **Windows:** `ghost_list_windows`, `ghost_focus_window`, `ghost_window_state`
- **Perception:** `ghost_describe_screen`, `ghost_get_text`
- **Control:** `ghost_wait`
- `name_to_vk`: string-to-VIRTUAL_KEY mapping (Enter, Tab, Escape, F1-F12, arrows, A-Z, 0-9)
- `ElementDescriptor` and `WindowInfo` types exported from `ghost-session`
- Emergency stop (Ctrl+Alt+G) now idempotent across multiple GhostSession instances

### Fixed

- `SetClipboardData` failure now properly frees HGLOBAL handle before returning error
- `EmptyClipboard` errors are now propagated instead of silently ignored
- Clipboard null-terminator scan bounded to 10M characters (prevents runaway on malformed data)
- `SendInput` partial failures now return `Err` instead of logging a warning and returning `Ok`
- `RegisterHotKey` error now uses windows-rs error code directly (no GetLastError race)

## [0.1.0] - 2026-04-01

### Added

- Initial release: 7 MCP tools over stdio JSON-RPC
- `ghost_find`, `ghost_click`, `ghost_type`, `ghost_click_at`, `ghost_screenshot`, `ghost_launch`, `ghost_stop`
- UI Automation element tree search with `By::name` and `By::role` locators
- DXGI Desktop Duplication screen capture (PNG, base64)
- Emergency stop: Ctrl+Alt+G global hotkey, STOP_FLAG atomic
- 3-crate workspace: `ghost-core`, `ghost-session`, `ghost-mcp`
