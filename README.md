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

Download prebuilt Windows binaries from the [Releases page](https://github.com/FrostbyteDevTeam/ghost/releases/latest):

- `ghost.exe` — CLI
- `ghost-http.exe` — REST server
- `ghost-mcp.exe` — MCP server

Or build from source:

```bash
git clone https://github.com/FrostbyteDevTeam/ghost
cd ghost
cargo build --release --bin ghost --bin ghost-http --bin ghost-mcp
# binaries in target/release/
```

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
ghost-session = { git = "https://github.com/FrostbyteDevTeam/ghost" }
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

Works with any MCP client (Claude, Cursor, etc.) — exposes 37 tools covering find/click/type/keys/mouse/clipboard/screenshot/windows/waits/intents/cache.

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

## Benchmarks (v0.3.0, Windows 11, Ryzen)

| Operation            | Measured  | Budget   |
| -------------------- | --------- | -------- |
| JSONLogic eq/var     | 32.2 ns   | 1 µs     |
| Intent compile (3op) | 1.49 µs   | 50 µs    |

See `docs/benches/v030-baseline.md`.

## Requirements

- Windows 10 or later
- Rust stable (only for building from source)

## License

MIT - Copyright 2026 Frostbyte Digital
