# Ghost

[![CI](https://github.com/NORTHTEKDevs/ghost/actions/workflows/ci.yml/badge.svg)](https://github.com/NORTHTEKDevs/ghost/actions/workflows/ci.yml)
[![Linux](https://github.com/NORTHTEKDevs/ghost/actions/workflows/linux.yml/badge.svg)](https://github.com/NORTHTEKDevs/ghost/actions/workflows/linux.yml)
[![Release](https://img.shields.io/github/v/release/NORTHTEKDevs/ghost)](https://github.com/NORTHTEKDevs/ghost/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**The computer-use layer for AI agents, on Windows and Linux.** Ghost lets an
agent operate any desktop app - including the ones with no API - **in the
background without taking your screen or cursor**, and it **proves every action
actually happened**.

Like Playwright, but for native desktop apps, and built for agents: an MCP server
any model can mount to see and drive the desktop.

One MCP surface, two engines: Win32 UI Automation on Windows, AT-SPI2 over D-Bus
on Linux. The verbs, the locator tiers and the act-then-verify loop are written
once and behave the same on both. [Platform support](#platforms) ·
[Linux setup](docs/linux-fedora.md)

## Why Ghost is different

- **Runs in the background, and that is enforced.** An agent can click, type, and
  use shortcuts inside an app *while you keep working in another window* - no focus
  steal, no cursor jump. It posts window messages to real controls and uses UI
  Automation patterns on windowless ones; most tools can only drive whatever is in
  the foreground. Since 0.19 this is a policy, not a preference: the focus policy
  defaults to `background`. Since 0.20 it is also *constructive*: anything Ghost
  starts (an app, a windowed browser) is born on a hidden desktop that has its own
  input queue and cannot take your foreground, and the ordinary verbs drive it there
  by window title. A call that truly has no background path fails naming the action
  instead of quietly taking the screen. Raise the policy per target with
  `ghost_set_focus_policy` when you actually want real input.
  ([how](#background-mode-agent-harness--computer-use))
- **Never your window by accident.** The session remembers the last window the
  agent named or launched and every window-scoped verb targets it by default. The
  human's foreground window is used only when nothing was ever anchored, and the
  response says so. Three weeks of real transcripts showed "element not found in the
  foreground window" as the top failure before this; it was the agent searching the
  window *you* had open.
- **Many agents at once.** Requests are dispatched in parallel, so a 15-second wait
  in one tab does not stall an instant query behind it, and a second Ghost process
  runs its own browser alongside the first without contending for the mouse.
- **Prove it on your machine.** `ghost verify` drives the real MCP server over
  stdio and audits every claim above against hard timing budgets, exiting non-zero
  if any of them does not hold on your hardware.
- **Every action is verified.** Ghost re-checks the screen (or reads the control's
  value back) after acting and returns `verified` / `focus_confirmed` - never a
  blind `ok:true`. Agents fail by acting and not knowing if it worked; Ghost closes
  that loop.
- **Drives apps with no API.** Legacy Win32, WPF, Electron, UWP, vendor portals - 
  the software that has no integration and most needs automating. No CDP, no
  browser, no app cooperation required.
- **Model-agnostic.** Vision grounding works with any OpenAI-compatible model
  (NVIDIA, OpenAI, Gemini, Groq, local vLLM/Ollama) or Anthropic. No vendor lock-in.
- **Accessibility-native and deep.** Real element discovery through the OS's own
  accessibility API - UI Automation on Windows, AT-SPI2 on Linux - not
  pixel-guessing. Elements come back with real names, roles and bounds.

See it in one script: [`examples/background_agent_demo.py`](examples/background_agent_demo.py)
drives an app in the background while the foreground stays yours.
Honest comparison vs Playwright-MCP / cua-driver / Computer Use:
[`docs/comparison.md`](docs/comparison.md).

## What is Ghost?

Ghost gives you programmatic control over any desktop application - native Win32, Electron, WPF, UWP, GTK, Qt, or otherwise.

On **Windows** it uses UI Automation for element discovery, SendInput for keyboard/mouse injection, and DXGI/GDI for screen capture. On **Linux** it uses AT-SPI2 over D-Bus for discovery and actions, XTEST (X11) or the RemoteDesktop portal / uinput (Wayland) for input, and X11 `GetImage` or the Screenshot portal for capture. The Linux engine is pure Rust - no `-devel` packages to install.

Ship it three ways:

- **`ghost` CLI** - one-shot commands, great for scripts and CI (`ghost click --name "Submit"`)
- **`ghost-http` server** - local REST API, call it from Python, Node, curl, anything (`curl http://127.0.0.1:7878/list-windows`)
- **`ghost-mcp` server** - Model Context Protocol server for Claude, Cursor, and any MCP client (54 tools on Windows)

The MCP surface is 20 desktop verbs, 19 `ghost_browser_*` / `ghost_tab_*` tools for
driving individual browser tabs in the background (Chrome, Comet, Edge, Brave), and
15 Windows-only tools: the focus policy plus `ghost_desktop_*` for explicit control of
isolated Windows desktops the user never sees. Under the default policy you rarely
need the latter: `ghost_window op=launch` already starts the app on the hidden
desktop `auto`, and `ghost_see` / `ghost_act` / `ghost_key` / `ghost_scroll` reach it
with `window=<title>` exactly as they reach a window on your own desktop
(`target.surface` in the response tells you which). UIA, window messages and capture
work fully there. Real `SendInput` does not, because Windows refuses it off the
input desktop, and typing is proven by reading the control's value back, so a target
that drops posted characters returns an error rather than a false success. The
desktop verbs and the browser tools build on Linux as well; the focus policy and
hidden desktops are Windows-only.

No Claude required. No browser required. No CDP. It drives apps through the OS's
own automation and input APIs, so it works with native apps that have no API and
no automation hooks of their own - the same reliability whether or not an app was
built to be automated.

### Platforms

| Platform | Status | Engine |
| --- | --- | --- |
| **Windows** | ✅ full and verified | `ghost-core` - Win32 UI Automation, SendInput, posted window messages, DXGI/GDI capture |
| **Linux** | ✅ functional - X11 + AT-SPI2 verified by live CI tests | `ghost-linux` - AT-SPI2 over D-Bus, XTEST / RemoteDesktop portal / uinput, X11 `GetImage` / Screenshot portal |
| **macOS** | 🚧 scaffold | Accessibility + CGEvent + ScreenCaptureKit - to be built on a Mac |

`ghost-session` and `ghost-mcp` are shared: the locator tiers, grounding cascade,
act-then-verify loop and the 20 core MCP verbs are written once and run on both
platforms. Only the engine underneath changes, behind a one-line `cfg` alias. The
browser and tab tools are engine-independent and build for both; the focus policy
and isolated desktops are Windows-only and are reported as such rather than faked.

**The wedge survives the port.** On Windows, driving an app without stealing
focus is built on posted window messages. Linux has a cleaner analogue in
AT-SPI2 actions: the application performs the operation through its own toolkit,
so there is no pointer to move and no window to raise - and it behaves the same
under X11 and Wayland. Synthetic input is only the fallback there.

This is tested, not asserted: CI stands up a real desktop (Xvfb + D-Bus +
at-spi-bus-launcher), drives a real GTK application, and requires that text
written through AT-SPI reads back from the app and that invoking a button
actually dismisses the dialog. Wayland portal input and capture are implemented
but not yet verified on hardware.

Linux setup, verification checklist and honest limitations:
[`docs/linux-fedora.md`](docs/linux-fedora.md). Capability matrix across all
three: [`docs/cross-platform.md`](docs/cross-platform.md).

Ghost is a general-purpose automation tool. Use it on systems you own or are
authorized to automate, and in line with the terms of the software you drive.

## Install

**One-click - MCP Bundle (free).** Every release ships `ghost-windows-x64.mcpb` and
`ghost-linux-x86_64.mcpb` on the [Releases page](https://github.com/NORTHTEKDevs/ghost/releases/latest).
Open one in a client that supports MCP Bundles (Claude Desktop: *Settings -> Extensions ->
Install from file*) and Ghost is registered, no PATH or config editing. Ghost is also listed in
the [MCP registry](https://registry.modelcontextprotocol.io) as `io.github.NORTHTEKDevs/ghost`,
so registry-aware clients can install it from there. The bundle holds the `ghost-mcp` server
only; the CLI and HTTP server are in the archives below.

**Option A - Prebuilt binaries (free).** Every release ships signed-by-checksum
archives for both platforms on the
[Releases page](https://github.com/NORTHTEKDevs/ghost/releases/latest):

```bash
# Linux x86_64
curl -LO https://github.com/NORTHTEKDevs/ghost/releases/latest/download/ghost-linux-x86_64.tar.gz
curl -LO https://github.com/NORTHTEKDevs/ghost/releases/latest/download/ghost-linux-x86_64.tar.gz.sha256
sha256sum -c ghost-linux-x86_64.tar.gz.sha256
tar -xzf ghost-linux-x86_64.tar.gz && ./install.sh
```

Windows: download `ghost-windows-x64.zip` from the same page. Verify the
checksum, unzip, and add the folder to your `PATH`. Then run `ghost doctor`.

**Option B - Ready-to-run Windows kit ($20, one-time).** Prebuilt Windows binaries (`ghost.exe`,
`ghost-http.exe`, `ghost-mcp.exe`) plus a quick-start, MCP config, and examples - no Rust toolchain, runs in
two minutes. Every kit is built by `scripts/package-kit.ps1`, which refuses to package unless the full live
desktop suite passes. Get it at **[northtek.io/ghost](https://northtek.io/ghost)**.

The binaries are **not code-signed yet**, so Windows SmartScreen will warn you on first run (click *More info*
→ *Run anyway*). The release pipeline signs them the moment a signing identity is configured; see
[`docs/code-signing.md`](docs/code-signing.md). The kit buys convenience, not capability - everything Ghost can do is in the free source
below, and building it yourself takes one command.

**Option C - Build from source (free, MIT).** Ghost is open source. Compile it yourself:

```bash
git clone https://github.com/NORTHTEKDevs/ghost
cd ghost
cargo build --release --bin ghost --bin ghost-http --bin ghost-mcp
# binaries in target/release/
```

Requirements: Windows 10 build 19041+, or Linux with `at-spi2-core` (and Rust
stable only if building from source).

**On Linux:**

```bash
sudo dnf install at-spi2-core xdg-desktop-portal xdg-desktop-portal-gnome
gsettings set org.gnome.desktop.interface toolkit-accessibility true
./scripts/install.sh          # build, install, register the MCP server, run doctor
```

No `-devel` packages are needed - the Linux engine is pure Rust. Full setup and
troubleshooting: [`docs/linux-fedora.md`](docs/linux-fedora.md).

**Check your machine first:**

```bash
ghost doctor
```

Reports PASS/WARN/FAIL and exits 1 if anything is FAIL. Run it before opening an
issue - it usually names the problem outright.

- **Windows:** build version, interactive desktop, UI Automation, DPI awareness,
  monitor layout, screen capture, optional vision credentials.
- **Linux:** session type (X11/Wayland), AT-SPI bus reachability, whether
  applications are actually exposing accessible trees, the selected input
  backend, and screen capture.

## Quick Start - CLI

```bash
# Launch Notepad and type into it
ghost launch notepad.exe
ghost focus-window "Notepad"
ghost type --role edit --text "hello from ghost"

# Keys and hotkeys
ghost press Enter
ghost hotkey --mods Ctrl --key s

# Screenshot
ghost screenshot --out shot.png

# Enumerate windows or UI
ghost list-windows
ghost describe --window "Notepad"

# Click at coords or by name
ghost click-at 500 300
ghost click --name "Save"

# Run a JSON intent (finite-state machine with retries, timeouts, conditions)
ghost run my-flow.json
echo '{"ops":[{"op":"launch","exe":"notepad.exe"}]}' | ghost run -
```

Everything outputs JSON for easy piping into `jq` or scripts.

## Quick Start - HTTP Server

Start the server:

```bash
ghost-http --addr 127.0.0.1:7878
```

Then from **any language**:

```bash
# Bash / curl
curl http://127.0.0.1:7878/list-windows
curl -X POST http://127.0.0.1:7878/click \
  -H 'content-type: application/json' \
  -d '{"name":"Submit"}'
curl http://127.0.0.1:7878/screenshot -o shot.png
```

```python
# Python
import requests
requests.post("http://127.0.0.1:7878/launch", json={"exe": "notepad.exe"})
requests.post("http://127.0.0.1:7878/type",
              json={"role": "edit", "text": "hello from python"})
```

```javascript
// Node
await fetch("http://127.0.0.1:7878/hotkey", {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ mods: ["Ctrl"], key: "s" }),
});
```

Endpoints: `/health`, `/tools`, `/click`, `/click-at`, `/type`, `/press`, `/hotkey`, `/screenshot`, `/launch`, `/list-windows`, `/focus-window`, `/window-state`, `/describe`, `/clipboard` (GET/POST), `/run`.

## Quick Start - Rust SDK

```toml
[dependencies]
ghost-session = { git = "https://github.com/NORTHTEKDevs/ghost" }
```

```rust
use ghost_session::{GhostSession, By, session::Region};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let session = GhostSession::new()?;
    session.launch("notepad.exe").await?;
    let edit = session.find(By::role("edit")).await?;
    edit.type_text("hello world")?;
    let png = session.screenshot(Region::full()).await?;
    std::fs::write("screen.png", png)?;
    Ok(())
}
```

## Quick Start - Claude Desktop / MCP

```bash
cargo build -p ghost-mcp --release
```

Add to Claude Desktop config (`%APPDATA%\Claude\claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "ghost": { "command": "C:/path/to/ghost-mcp.exe" }
  }
}
```

Works with any MCP client (Claude, Cursor, etc.). 54 tools on Windows (legacy names
stay dispatchable): 20 desktop verbs covering
see/snapshot/find/act/keys/scroll/drag/clipboard/screenshot/windows/shell/waits/query/run,
19 `ghost_browser_*` / `ghost_tab_*` tools, and 15 Windows-only tools for the focus
policy and isolated desktops.

Every tool runs on its own task, so a slow call does not block a fast one, and a
second Ghost process can run alongside the first. Once it is mounted, run
`ghost verify` to audit that on your own machine.

### Shell control (`ghost_shell`)

Ghost drives GUIs *and* the command line. `ghost_shell` runs terminal commands and
persistent PowerShell sessions - builds, git, CLIs, file edits on hosts without file
tools, or launching apps. `op=run` is a one-shot (`powershell`/`pwsh`/`cmd`); `op=open`
starts a persistent PowerShell whose variables and cwd survive across `op=send` calls.
Output is merged stdout+stderr, tail-capped for the agent's context window; a timed-out
command keeps running and is drained with `op=read`; `ghost_stop` kills a runaway.
`op=run` with the default `powershell` is served from a pre-spawned spare process, so
a command costs about 85 ms instead of the 230-450 ms a fresh PowerShell start takes
(the spare is single-use and replaced immediately; `GHOST_SHELL_WARM=off` disables it).

Spawn a fresh Claude Code session from the agent:
`ghost_shell op=run cmd='Start-Process wt -ArgumentList "pwsh","-NoExit","-Command","claude"'`,
then drive the new terminal window with `ghost_see` / `ghost_act` / `ghost_key`.

**Security:** shell access is powerful. Set `GHOST_SHELL=off` in the server's env to
disable the verb entirely - every op then returns a clear refusal, leaving the GUI
automation verbs fully usable.

## Reliability Model

Desktop automation driven from an MCP client has a hostile focus environment: between tool
calls, the client's own terminal usually retakes OS focus. Ghost is built for that:

- **`ghost_act` is atomic** - find → act → verify via screen delta. One call, no
  cross-call race. Under the default `background` policy it drives the control in
  place and never raises the window; raise the policy and it additionally brings the
  target's window to the foreground first (AttachThreadInput, confirmed).
- **Every action response is honest**: `verified` (did the screen actually change),
  `focus_confirmed` (was the right window foreground), and a `warning` when either is off - 
  never a blind `ok:true`. Check `verified` before re-issuing an action.
- **Nothing is left running.** Browsers Ghost launches are in a kill-on-close job
  object: when the server ends, however it ends, they end with it
  (`dies_with_server` on the launch response). At startup the server also sweeps
  browsers abandoned by earlier servers and reports them in
  `ghost_stats.orphan_sweep`.
- **Anchor to a window** - `ghost_see`, `ghost_find`, `ghost_act`, `ghost_key`,
  `ghost_click_at`, `ghost_scroll`, `ghost_wait`, `ghost_assert` and `ghost_screenshot`
  (unless `full=true`) all take `window`
  (a title substring, resolved across your desktop and Ghost's hidden desktops: exact
  title beats prefix beats substring, a window that is not minimised wins ties). The
  match becomes the session **anchor**: later calls without `window` target it, never
  the window the human happens to be using. `ghost_window op=focus` anchors without
  raising under the background policy; `op=anchor` sets, clears or reports it; every
  response carries `target {hwnd, title, surface, source}`. A title that matches
  nothing lists the open windows and returns code -32007.
- **Your own browser, through its own protocol** - when a window's process was started
  with `--remote-debugging-port` (Comet, Chrome, Edge, Brave), the same anchored verbs
  route through the DevTools protocol instead of UI Automation: DOM names and
  `aria-label`s rather than a sparse tree, selectors that survive re-renders, trusted
  input events into that renderer, full modifier combos, and focus emulation so pages
  that check `document.hasFocus()` still accept typing. Nothing about it can reach your
  foreground. The response carries `route: {browser, port, tab}` and `coords: viewport`;
  a browser without a port keeps the UI Automation path unchanged. `GHOST_CDP_ROUTE=off`
  turns it off. Start Comet or Chrome once with `--remote-debugging-port=9333` to get it.
- **Misses name the alternatives** - "element not found" is followed by the closest
  element names in that window, so an agent does not spend a round trip on `ghost_see`
  to learn what the app calls the thing.
- **Disambiguate duplicates** - `index` selects the nth match when several elements share a
  name/role (multiple "Close Tab" buttons); responses carry a `matches` count.
- **It audits itself** - an independent sampler watches the foreground window and the
  OS's last-input time; any foreground change with no real hardware input behind it is
  recorded as synthetic, with the tool calls that were in flight. `ghost_stats` reports
  the tally, so the headline claim is proven continuously, not once.
- **Read, don't screenshot** - `ghost_see mode=text` extracts a window/page's readable text
  straight from the accessibility tree: faster and ~10x cheaper in tokens than images.
- **Latency is visible**: every response carries `ms`, and `escalated: true` flags when a
  find had to pay a network VLM round trip (local tiers: cache → UIA → OCR are all on-device).
- **Windows never disappear**: minimized windows stay in `ghost_window list` (with `state`)
  and `op=focus` auto-restores them. A window that something hid outright (not visible,
  not minimized) shows up with `op=list include_hidden=true` as `state: "hidden"`, and
  `op=state state=restore` brings it back without activating it. `op=state` also
  reaches windows on Ghost's own hidden desktops, so an app started with `op=launch`
  can be closed by name.
- **Stop always works**: `ghost_stop` preempts the in-flight call the moment it arrives
  (dedicated stdin reader), and Ctrl+Alt+G remains the OS-level kill switch.

## Background mode (agent-harness / computer-use)

Agent harnesses (OpenClaw, Hermes/cua-driver, and any MCP client) mount a
computer-use tool to let an LLM operate the desktop. Ghost is that tool - and it acts
**without stealing your focus or moving your cursor**, so an agent drives an app
while you keep working in another window.

Since 0.19 this is the default and it is enforced. The process-wide focus policy
starts at `background`, and every primitive that could only work by taking the real
cursor or foreground window is gated behind it. There is no silent fallback: a call
with no background path returns an error naming the action and the policy that would
unblock it. Set `GHOST_FOCUS_POLICY` in the server env, or call
`ghost_set_focus_policy` for a target that genuinely needs real input, and set it
back afterwards. `ghost_focus_policy` reports the current setting.

```jsonc
// Drive an app while the human keeps working. No flag needed: background is the default.
ghost_window { "op": "launch", "exe": "notepad.exe" }
// -> { "surface": "hidden", "desktop": "auto", "window": { "title": "Notepad", ... }, "target": {...} }
ghost_act { "role": "edit", "action": "type", "text_input": "hello" }   // targets the anchor
// -> { "verified": true, "focus_preserved": true, "cursor_preserved": true, "mode": "hidden" }
ghost_act { "window": "Comet", "name": "Post", "action": "click" }      // a window on YOUR desktop
// -> { "verified": true, "focus_preserved": true, "cursor_preserved": true, "mode": "background" }
```

- **Launches never surface.** Measured on this repo's CI box: Edge and Chrome
  activate their first window on launch in every launch style (normal, hidden,
  minimised, from a background parent, placed at -32000,-32000). A window created on
  your desktop takes your keyboard the moment it exists. So under the background
  policy Ghost never creates one there: `ghost_window op=launch`, `ghost_run` launch
  steps and `ghost_browser_launch mode=windowed` start on a hidden desktop with its
  own input queue, and the window is anchored so the next `ghost_see` shows it. An
  independent observer sampling the foreground at 100 ms across a full
  launch-drive-close run reported zero changes.
- **True background via posted window messages.** Real Win32 controls are driven
  with `BM_CLICK` / `WM_LBUTTONDOWN·UP` (click), `WM_SETTEXT` (type) and
  `WM_MOUSEWHEEL` (scroll). These do not activate the window.
- **Windowless controls, without the screen.** UWP/WinUI/Chromium/Electron controls
  have no window handle. Ghost drives them with UI Automation patterns - `Invoke` for
  a click, `ValuePattern` for typing - which web content services without raising the
  window (measured on Chrome: a page button clicked and a page `<input>` filled, with
  input events firing, `focus_preserved: true`). Views-level chrome such as the
  address bar can still activate; `focus_preserved` reports the truth. Posted single
  keys reach the page's focused element.
- **Verified even while occluded.** `type` is confirmed by reading the control's
  value back; `click` by a `PrintWindow` before/after delta that renders a window
  that isn't visible. Every response carries `verified`, `focus_preserved`,
  `cursor_preserved` - Ghost never claims a background action it can't confirm.
- **Hidden desktops are the same vocabulary.** A window on the hidden desktop is
  driven with the same `window=<title>` calls; `target.surface: "hidden"` is the only
  difference. Chromium and Electron windows there are driven by posted messages
  rather than UIA actions (Chromium services `Invoke`/`SetValue` on a non-composited
  desktop only after a ~2 s internal wait; a posted click lands in ~100 ms), and pixel
  verification is skipped for them because a software render there costs seconds -
  confirm through `ghost_tab_eval` or `ghost_see`. Real `SendInput` still does not
  work on a hidden desktop (Windows refuses it off the input desktop), so an app that
  answers *only* real hardware input needs the `foreground` policy on your own desktop.
- **Honest about single-instance apps.** Windows 11 Notepad, Explorer, or a browser on
  its default profile hand a launch to their already-running process, whose windows
  live on your desktop. Ghost cannot prevent that; it reports `surface: "user"` with a
  warning rather than claiming the app is hidden.

Supports `click`, `type`, `double_click`, `right_click`, `hover` and scrolling, plus
`ghost_key` for single keys (Enter/Tab/F-keys/char via `WM_KEYDOWN`/`WM_CHAR`). The
`background: true` flag is accepted for compatibility; it is the default behaviour.

The clipboard and edit combos work in the background too - Ctrl+C, Ctrl+X, Ctrl+V,
Ctrl+Z and Ctrl+A are sent as the semantic messages an app actually implements
(`WM_COPY`, `WM_CUT`, `WM_PASTE`, `WM_UNDO`, `EM_SETSEL`) rather than as a posted
modifier that apps reading `GetKeyState` would ignore. Combos outside that set are
rejected rather than silently dropped, because posting cannot set the modifier state
those apps read; use the `foreground` policy for them.

## Vision is model-agnostic

Description-based grounding works with any tool-capable vision model behind an
OpenAI-compatible endpoint - NVIDIA (free default), OpenAI, Gemini, Groq, or a
local vLLM / Ollama / LM Studio server - or Anthropic. Point `GHOST_VISION_BASE_URL`
+ `GHOST_VISION_MODEL` at your endpoint and set `GHOST_VISION_API_KEY` (a keyless
local server needs only the base URL). No vendor lock-in.

## Emergency Stop

Press **Ctrl+Alt+G** at any time to immediately halt every acting call. Read-only
queries (`ghost_see`, `ghost_snapshot`, `ghost_window` list) keep answering, so you
can still inspect what happened while everything is stopped.
- All queued actions are cancelled
- Any held modifier keys (Shift, Ctrl, Alt) are released immediately
- No stuck keys, no stuck modifier states
- **Session-wide since 0.19.** The stop is a named kernel event
  (`Local\ghost-emergency-stop-event`), so one press halts every Ghost process in
  your logon session, not just the one that happened to register the hotkey.
  `ghost_reset` resumes service, again for all of them.

## Element Locators

```rust
session.find(By::name("Save")).await?          // by accessible name (substring)
session.find(By::role("edit")).await?          // by UIA control type
session.find(By::role("button")).await?
```

From the CLI: `ghost click --name "Save"` or `ghost click --role button`.

## Intents - Declarative Flows

Write reproducible multi-step flows as JSON. The FSM executor supports retries, timeouts, and JSONLogic conditions for `abort_if` / `retry_if`.

```json
{
  "ops": [
    { "op": "launch", "exe": "notepad.exe" },
    { "op": "focus_window", "name": "Notepad" },
    { "op": "type", "role": "edit", "text": "hello" },
    { "op": "hotkey", "mods": ["Ctrl"], "key": "s" }
  ]
}
```

Run with `ghost run flow.json`, `POST /run`, or `ghost_execute_intent` over MCP.

## Architecture

```
ghost-cli     ghost-http     ghost-mcp     Rust SDK
    \            |            /   |           |
     \           |           /    |           |
      +-----> ghost-session <-----|-----------+   ← safe Rust API
              /           \       |
      ghost-core      ghost-linux |               ← one cfg alias picks the engine
          |                |      |
    Win32 UIA,       AT-SPI2 over |               ← ghost-core: SendInput, DXGI/GDI
    posted msgs      D-Bus, XTEST |               ← ghost-linux: portal / uinput
          |                |      |
      Windows OS        Linux     +-> ghost-browser  ← CDP over the DevTools port,
                                                        engine-independent
```

Supporting crates: `ghost-cache` (UIA snapshot + delta), `ghost-intent` (FSM +
JSONLogic executor), `ghost-ground` (the locator tier cascade), `ghost-platform`
(the capability matrix reported per OS).

## Vision grounding (Set-of-Marks)

When you locate an element by natural-language description (`ghost_find
description="the blue submit button"`, or when a name/text lookup misses and
escalates to the VLM), Ghost does **not** ask the model to guess pixel
coordinates - models are unreliable at that (in testing, a plain "give me the
coordinates of the equals button" landed ~250px off the target). Instead it uses
**Set-of-Marks**: it overlays numbered badges on the window's detected elements,
sends that marked screenshot plus each badge's accessible-name label, and asks
the model which *number* matches. The number maps back to that element's exact
rect, so the result is a real on-element coordinate, not a regression guess.

In a live check on Calculator, four descriptions ("the equals button", "the plus
button", "the number seven key", "the multiply button") each landed exactly on
the correct button - versus ~250px off with coordinate regression.

Honest scope: when detected elements carry accessible names (most apps), the
labels do much of the disambiguation; for unlabeled icons the model leans on the
badge's visual position/appearance.

**Canvas / no-accessibility-tree apps.** When the UIA tree is sparse (custom-drawn
UIs, remote-desktop surfaces, game canvases), Ghost augments the Set-of-Marks
candidates with a built-in **CPU classical-CV detector** (`ghost_ground::cv_detect`):
edge density → connected components → size/aspect filter, no GPU and no model
download. It gives the VLM real boxes to pick from where the accessibility tree
has nothing. It is coarser than a trained detector - an optional OmniParser ONNX
tier (`--features yolo` + `GHOST_YOLO_MODEL`) plugs into the same Set-of-Marks
path when a GPU model is available. The CV-marks → VLM-pick end-to-end needs a
configured vision key.

## Benchmark - task success, not "did the call return ok"

`bench/` holds a reproducible end-to-end benchmark: it drives the real
`ghost-mcp` binary through 14 Windows desktop tasks and scores each by
**re-observing the actual result** (does the Calculator display really read 42?
is the typed value really present?), never by trusting a tool call's return.

Latest run (see [`bench/results/latest.md`](bench/results/latest.md)):

> **14/14 tasks passed (100%)**, median ~2.7 s per task (full wall-clock incl.
> app launch) - perception, click/keyboard action+verify, waits, window
> management (list/minimize/restore), text extraction, disambiguation, flow
> chaining, clipboard round-trip, structured errors, element screenshots, and
> value assertions.

And it proves it can *fail*: `--self-test` runs deliberately-wrong negative
controls (assert the display reads 99 when it reads 42, etc.) and passes only if
the harness scores every one as FAIL - so the green run above is a real signal,
not a rubber stamp.

Reproduce on any Windows 10/11 machine:

```bash
cargo build --release -p ghost-mcp
python bench/run_bench.py             # exit 0 iff every task passed
python bench/run_bench.py --self-test # exit 0 iff the harness caught every planted failure
```

### Reliability soak

`bench/soak.py` drives many act-then-verify cycles and gates on the signals unit
tests can't see: how often `verified` comes back null/false, focus-loss rate,
error rate, whether each action's real effect happened (the display is
re-observed, never trusted from the return), and latency percentiles.

> Latest (160 acts): **PASS** - verify-null 0.0, focus-loss 0.0, effect-mismatch
> 0 (100% correct), p50 85ms / p95 117ms. See
> [`bench/results/soak.md`](bench/results/soak.md).

```bash
python bench/soak.py                  # exit 0 iff every reliability threshold holds
python bench/soak.py --cycles 250     # ~1000 acts
python bench/soak.py --self-test      # exit 0 iff the harness flags a planted-wrong effect
```

We deliberately publish only Ghost's own measured numbers - never invented
columns for other tools. `bench/README.md` gives an honest protocol for
comparing against Playwright-MCP / Computer Use / UI-TARS, and explains why a
naive same-suite comparison isn't apples-to-apples (Playwright is browser-only;
vision agents need an API + VM).

### Microbenchmarks

| Operation                          | Measured  |
| ---------------------------------- | --------- |
| Region capture, GDI, any size      | ~16.5 ms  |
| Region capture, DXGI, 1600x900     | ~70-83 ms |
| BGRA→RGBA convert, 400x300 region  | ~206 µs   |
| JSONLogic eq/var                   | 32.2 ns   |
| Intent compile (3op)               | 1.49 µs   |

End-to-end capture measurement (release, `crates/ghost-core/tests/capture_latency_probe.rs`)
corrected the v0.10.0 assumption: the DXGI *acquire* dominates and hits a cliff on
large windows, so region captures (act-verify, screenshots, Set-of-Marks) route
through flat ~16.5ms GDI BitBlt in v0.11.0; full-screen still uses DXGI. Run the
convert microbench: `cargo bench -p ghost-core --bench convert`. Older baselines:
`docs/benches/v030-baseline.md`.

## Requirements

- **Windows** 10 build 19041 or later
- **Linux** with `at-spi2-core` and a desktop session (X11 or Wayland);
  `xdg-desktop-portal-gnome` additionally for Wayland input and capture
- A Chromium-family browser (Chrome, Comet, Edge, or Brave) for the
  `ghost_browser_*` / `ghost_tab_*` tools only; the desktop verbs need none
- Rust stable (only for building from source)

## License

MIT - Copyright 2026 Northtek
