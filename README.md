# Ghost

Windows desktop automation for humans, scripts, and AI agents. Like Playwright, but for native apps.

Any application. Any input. Any agent. Any language.

## What is Ghost?

Ghost gives you programmatic control over any Windows application — native Win32, Electron, WPF, UWP, or otherwise. It uses the Windows UI Automation API for element discovery, Win32 SendInput for keyboard/mouse injection, and DXGI/GDI for screen capture.

Ship it three ways:

- **`ghost` CLI** — one-shot commands, great for scripts and CI (`ghost click --name "Submit"`)
- **`ghost-http` server** — local REST API, call it from Python, Node, curl, anything (`curl http://127.0.0.1:7878/list-windows`)
- **`ghost-mcp` server** — Model Context Protocol server for Claude, Cursor, and any MCP client (37 tools)

No Claude required. No browser required. No CDP. Unblockable because it drives the OS like a user.

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

Works with any MCP client (Claude, Cursor, etc.) — 17 lean verbs advertised (legacy names stay dispatchable) covering see/find/act/keys/scroll/drag/clipboard/screenshot/windows/waits/query/run.

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

We deliberately publish only Ghost's own measured numbers — never invented
columns for other tools. `bench/README.md` gives an honest protocol for
comparing against Playwright-MCP / Computer Use / UI-TARS, and explains why a
naive same-suite comparison isn't apples-to-apples (Playwright is browser-only;
vision agents need an API + VM).

### Microbenchmarks (v0.3.0, Windows 11, Ryzen)

| Operation            | Measured  | Budget   |
| -------------------- | --------- | -------- |
| JSONLogic eq/var     | 32.2 ns   | 1 µs     |
| Intent compile (3op) | 1.49 µs   | 50 µs    |

See `docs/benches/v030-baseline.md`.

## Requirements

- Windows 10 or later
- Rust stable (only for building from source)

## License

MIT - Copyright 2026 Northtek
