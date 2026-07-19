# Ghost

**The computer-use layer for AI agents on Windows.** Ghost lets an agent operate
any Windows app — including the ones with no API — **in the background without
taking your screen or cursor**, and it **proves every action actually happened**.

Like Playwright, but for native Windows apps, and built for agents: an MCP server
any model can mount to see and drive the desktop.

## Why Ghost is different

- **Runs in the background.** An agent can click, type, and use shortcuts inside an
  app *while you keep working in another window* — no focus steal, no cursor jump.
  It posts window messages to real controls; most tools can only drive whatever is
  in the foreground. ([how](#background-mode-agent-harness--computer-use))
- **Every action is verified.** Ghost re-checks the screen (or reads the control's
  value back) after acting and returns `verified` / `focus_confirmed` — never a
  blind `ok:true`. Agents fail by acting and not knowing if it worked; Ghost closes
  that loop.
- **Drives apps with no API.** Legacy Win32, WPF, Electron, UWP, vendor portals —
  the software that has no integration and most needs automating. No CDP, no
  browser, no app cooperation required.
- **Model-agnostic.** Vision grounding works with any OpenAI-compatible model
  (NVIDIA, OpenAI, Gemini, Groq, local vLLM/Ollama) or Anthropic. No vendor lock-in.
- **Windows-native and deep.** Uses UI Automation for real element discovery, not
  pixel-guessing — the gap left by Mac/Linux-first agent tooling.

See it in one script: [`examples/background_agent_demo.py`](examples/background_agent_demo.py)
drives an app in the background while the foreground stays yours.
Honest comparison vs Playwright-MCP / cua-driver / Computer Use:
[`docs/comparison.md`](docs/comparison.md).

## What is Ghost?

Ghost gives you programmatic control over any Windows application — native Win32, Electron, WPF, UWP, or otherwise. It uses the Windows UI Automation API for element discovery, Win32 SendInput for keyboard/mouse injection, and DXGI/GDI for screen capture.

Ship it three ways:

- **`ghost` CLI** — one-shot commands, great for scripts and CI (`ghost click --name "Submit"`)
- **`ghost-http` server** — local REST API, call it from Python, Node, curl, anything (`curl http://127.0.0.1:7878/list-windows`)
- **`ghost-mcp` server** — Model Context Protocol server for Claude, Cursor, and any MCP client (37 tools)

No Claude required. No browser required. No CDP. It drives apps through the OS's
own automation and input APIs, so it works with native apps that have no API and
no automation hooks of their own — the same reliability whether or not an app was
built to be automated.

### Platforms

Ghost targets three OSes through one shared contract (`crates/ghost-platform`):

- **Windows** — full and verified. The flagship; every feature above works here.
- **macOS / Linux** — architecture in place, native backends in progress (not yet
  functional). The cross-platform crate compiles for all three; the macOS
  (Accessibility/CGEvent) and Linux (AT-SPI/XTest) engines are scaffolded with a
  precise implementation map and must be built and verified on those machines.

See [`docs/cross-platform.md`](docs/cross-platform.md) for the capability matrix
and the plan. Note: Ghost's background-without-focus-steal wedge relies on Windows
window messages, which have no exact macOS/Linux equivalent — that capability is
"measure before claiming" off Windows.

Ghost is a general-purpose automation tool. Use it on systems you own or are
authorized to automate, and in line with the terms of the software you drive.

## Install

**Option A — Ready-to-run kit ($20, one-time).** Prebuilt, verified Windows binaries (`ghost.exe`,
`ghost-http.exe`, `ghost-mcp.exe`) plus a quick-start, MCP config, and examples — no Rust toolchain, runs in
two minutes. Get it at **[northtek.io/ghost](https://northtek.io/ghost)**. (This just buys convenience; the
source below is free.)

**Option B — Build from source (free, MIT).** Ghost is open source. Compile it yourself:

```bash
git clone https://github.com/NORTHTEKDevs/ghost
cd ghost
cargo build --release --bin ghost --bin ghost-http --bin ghost-mcp
# binaries in target/release/
```

Requirements: Windows 10+ (and Rust stable only if building from source).

## Quick Start — CLI

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

## Quick Start — HTTP Server

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

## Quick Start — Rust SDK

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

## Quick Start — Claude Desktop / MCP

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

Works with any MCP client (Claude, Cursor, etc.) — 20 lean verbs advertised (legacy names stay dispatchable) covering see/snapshot/find/act/keys/scroll/drag/clipboard/screenshot/windows/shell/waits/query/run.

### Shell control (`ghost_shell`)

Ghost drives GUIs *and* the command line. `ghost_shell` runs terminal commands and
persistent PowerShell sessions — builds, git, CLIs, file edits on hosts without file
tools, or launching apps. `op=run` is a one-shot (`powershell`/`pwsh`/`cmd`); `op=open`
starts a persistent PowerShell whose variables and cwd survive across `op=send` calls.
Output is merged stdout+stderr, tail-capped for the agent's context window; a timed-out
command keeps running and is drained with `op=read`; `ghost_stop` kills a runaway.

Spawn a fresh Claude Code session from the agent:
`ghost_shell op=run cmd='Start-Process wt -ArgumentList "pwsh","-NoExit","-Command","claude"'`,
then drive the new terminal window with `ghost_see` / `ghost_act` / `ghost_key`.

**Security:** shell access is powerful. Set `GHOST_SHELL=off` in the server's env to
disable the verb entirely — every op then returns a clear refusal, leaving the GUI
automation verbs fully usable.

## Reliability Model (v0.7.x)

Desktop automation driven from an MCP client has a hostile focus environment: between tool
calls, the client's own terminal usually retakes OS focus. Ghost is built for that:

- **`ghost_act` is atomic** — find → bring the target's window to the foreground
  (AttachThreadInput, confirmed) → act → verify via screen delta. One call, no cross-call race.
- **Every action response is honest**: `verified` (did the screen actually change),
  `focus_confirmed` (was the right window foreground), and a `warning` when either is off —
  never a blind `ok:true`. Check `verified` before re-issuing an action.
- **Anchor to a window** — `ghost_key`, `ghost_find`, and `ghost_act` all take `window`;
  with it, input/resolution is guaranteed to target that window or the call errors. Use it
  for any multi-window flow.
- **Disambiguate duplicates** — `index` selects the nth match when several elements share a
  name/role (multiple "Close Tab" buttons); responses carry a `matches` count.
- **Read, don't screenshot** — `ghost_see mode=text` extracts a window/page's readable text
  straight from the accessibility tree: faster and ~10x cheaper in tokens than images.
- **Latency is visible**: every response carries `ms`, and `escalated: true` flags when a
  find had to pay a network VLM round trip (local tiers: cache → UIA → OCR are all on-device).
- **Windows never disappear**: minimized windows stay in `ghost_window list` (with `state`)
  and `op=focus` auto-restores them.
- **Stop always works**: `ghost_stop` preempts the in-flight call the moment it arrives
  (dedicated stdin reader), and Ctrl+Alt+G remains the OS-level kill switch.

## Background mode (agent-harness / computer-use)

Agent harnesses (OpenClaw, Hermes/cua-driver, and any MCP client) mount a
computer-use tool to let an LLM operate the desktop. Ghost is that tool for
Windows — and it can act **without stealing your focus or moving your cursor**, so
an agent drives an app while you keep working in another window.

```jsonc
// Drive Calculator-in-the-background style call:
ghost_act { "background": true, "window": "Character Map",
            "action": "click", "name": "Select", "role": "button" }
// -> { "verified": true, "focus_preserved": true, "cursor_preserved": true, "mode": "background" }
```

- **True background via posted window messages.** Real Win32 controls are driven
  with `BM_CLICK` / `WM_LBUTTONDOWN·UP` (click) and `WM_SETTEXT` (type). These do
  not activate the window — unlike UIA `Invoke`/`SetValue`, whose providers pull
  the window to the foreground.
- **Verified even while occluded.** `type` is confirmed by reading the control's
  value back; `click` by a `PrintWindow` before/after delta that renders a window
  that isn't visible. Every response carries `verified`, `focus_preserved`,
  `cursor_preserved` — Ghost never claims a background action it can't confirm.
- **Honest about the edge.** Windowless controls (UWP/WinUI/Chromium — no window
  handle) can't be message-posted by *any* tool; there Ghost falls back to UIA
  dispatch (which activates the window) and says so in the response. Classic Win32
  line-of-business apps — the software that has no API and most needs automating —
  drive cleanly in the background.

Supports `click`, `type`, `double_click`, `right_click`, and `hover` (posted mouse
messages), plus `ghost_key background=true` for single keys (Enter/Tab/F-keys/char
via `WM_KEYDOWN`/`WM_CHAR`). Modifier combos (Ctrl+C) are rejected in background —
posting can't set the modifier state apps read, so a combo would silently break;
use foreground for those.

## Vision is model-agnostic

Description-based grounding works with any tool-capable vision model behind an
OpenAI-compatible endpoint — NVIDIA (free default), OpenAI, Gemini, Groq, or a
local vLLM / Ollama / LM Studio server — or Anthropic. Point `GHOST_VISION_BASE_URL`
+ `GHOST_VISION_MODEL` at your endpoint and set `GHOST_VISION_API_KEY` (a keyless
local server needs only the base URL). No vendor lock-in.

## Emergency Stop

Press **Ctrl+Alt+G** at any time to immediately halt all automation.
- All queued actions are cancelled
- Any held modifier keys (Shift, Ctrl, Alt) are released immediately
- No stuck keys, no stuck modifier states

## Element Locators

```rust
session.find(By::name("Save")).await?          // by accessible name (substring)
session.find(By::role("edit")).await?          // by UIA control type
session.find(By::role("button")).await?
```

From the CLI: `ghost click --name "Save"` or `ghost click --role button`.

## Intents — Declarative Flows

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
    \            |              /             |
     \           |             /              |
      +-----> ghost-session  <----------------+   ← safe Rust API
                   |
              ghost-core                         ← Win32 FFI: UIA, SendInput, DXGI
                   |
              Windows OS
```

Supporting crates: `ghost-cache` (UIA snapshot + delta), `ghost-intent` (FSM + JSONLogic executor).

## Vision grounding (Set-of-Marks)

When you locate an element by natural-language description (`ghost_find
description="the blue submit button"`, or when a name/text lookup misses and
escalates to the VLM), Ghost does **not** ask the model to guess pixel
coordinates — models are unreliable at that (in testing, a plain "give me the
coordinates of the equals button" landed ~250px off the target). Instead it uses
**Set-of-Marks**: it overlays numbered badges on the window's detected elements,
sends that marked screenshot plus each badge's accessible-name label, and asks
the model which *number* matches. The number maps back to that element's exact
rect, so the result is a real on-element coordinate, not a regression guess.

In a live check on Calculator, four descriptions ("the equals button", "the plus
button", "the number seven key", "the multiply button") each landed exactly on
the correct button — versus ~250px off with coordinate regression.

Honest scope: when detected elements carry accessible names (most apps), the
labels do much of the disambiguation; for unlabeled icons the model leans on the
badge's visual position/appearance.

**Canvas / no-accessibility-tree apps.** When the UIA tree is sparse (custom-drawn
UIs, remote-desktop surfaces, game canvases), Ghost augments the Set-of-Marks
candidates with a built-in **CPU classical-CV detector** (`ghost_ground::cv_detect`):
edge density → connected components → size/aspect filter, no GPU and no model
download. It gives the VLM real boxes to pick from where the accessibility tree
has nothing. It is coarser than a trained detector — an optional OmniParser ONNX
tier (`--features yolo` + `GHOST_YOLO_MODEL`) plugs into the same Set-of-Marks
path when a GPU model is available. The CV-marks → VLM-pick end-to-end needs a
configured vision key.

## Benchmark — task success, not "did the call return ok"

`bench/` holds a reproducible end-to-end benchmark: it drives the real
`ghost-mcp` binary through 14 Windows desktop tasks and scores each by
**re-observing the actual result** (does the Calculator display really read 42?
is the typed value really present?), never by trusting a tool call's return.

Latest run (see [`bench/results/latest.md`](bench/results/latest.md)):

> **14/14 tasks passed (100%)**, median ~2.7 s per task (full wall-clock incl.
> app launch) — perception, click/keyboard action+verify, waits, window
> management (list/minimize/restore), text extraction, disambiguation, flow
> chaining, clipboard round-trip, structured errors, element screenshots, and
> value assertions.

And it proves it can *fail*: `--self-test` runs deliberately-wrong negative
controls (assert the display reads 99 when it reads 42, etc.) and passes only if
the harness scores every one as FAIL — so the green run above is a real signal,
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

> Latest (160 acts): **PASS** — verify-null 0.0, focus-loss 0.0, effect-mismatch
> 0 (100% correct), p50 85ms / p95 117ms. See
> [`bench/results/soak.md`](bench/results/soak.md).

```bash
python bench/soak.py                  # exit 0 iff every reliability threshold holds
python bench/soak.py --cycles 250     # ~1000 acts
python bench/soak.py --self-test      # exit 0 iff the harness flags a planted-wrong effect
```

We deliberately publish only Ghost's own measured numbers — never invented
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

End-to-end capture measurement (release, `tests/capture_latency_probe.rs`)
corrected the v0.10.0 assumption: the DXGI *acquire* dominates and hits a cliff on
large windows, so region captures (act-verify, screenshots, Set-of-Marks) route
through flat ~16.5ms GDI BitBlt in v0.11.0; full-screen still uses DXGI. Run the
convert microbench: `cargo bench -p ghost-core --bench convert`. Older baselines:
`docs/benches/v030-baseline.md`.

## Requirements

- Windows 10 or later
- Rust stable (only for building from source)

## License

MIT - Copyright 2026 Northtek
