# Using Ghost

How Ghost behaves once it is installed: targeting windows, what every response
promises, and the three ways to drive it (MCP, CLI, HTTP) plus the Rust SDK.
Every tool and parameter is in the [tool reference](tools.md).

## What people use it for

| Use | Tools |
| --- | --- |
| Apps with no API: installers, line-of-business software, vendor portals, WPF and Electron tools | `ghost_act`, `ghost_key`, `ghost_scroll`, `ghost_drag` |
| Window and process control: launch invisibly, focus, minimise, restore, close, recover hidden windows | `ghost_window`, `ghost_desktop_*`, `ghost_stats` |
| A terminal for the agent: builds, git, CLIs, persistent PowerShell | `ghost_shell`, `ghost_run` |
| The browser you are signed into, over its DevTools port: tabs, navigation, DOM clicks, JS, page text | `ghost_browser_*`, `ghost_tab_*` |
| Reading data out of apps with no export | `ghost_see mode=text`, `ghost_tab_text` |
| Making "did it work" a machine check | `ghost_assert`, `ghost_wait` |
| Repeatable multi-step flows with no model in the loop | intents via `ghost run`, `POST /run`, `ghost_run` |

## Targeting a window: the anchor

`ghost_see`, `ghost_find`, `ghost_act`, `ghost_key`, `ghost_scroll`,
`ghost_wait`, `ghost_assert` and `ghost_screenshot` take `window`: a title
substring, resolved across your desktop and Ghost's hidden desktops (exact
title beats prefix beats substring; a window that is not minimised wins ties).
The match becomes the session **anchor**, and later calls without `window`
target it, never the window you happen to be using. The anchor follows the
window by handle, so a title change still resolves (`title_drift` in the
response).

- `ghost_window op=focus` anchors without raising the window.
- `ghost_window op=anchor` sets, clears or reports the anchor.
- Every response carries `target {hwnd, title, surface, source}`.
- A title that matches nothing lists the open windows (error code -32007).
- If the anchored window closes, unanchored calls refuse and name it instead of
  silently retargeting.

## What every response promises

- **`verified`**: whether the change was observed (the control's value read
  back, the title changed, or a before/after pixel delta), with `verified_by`.
  Never a blind `ok: true`. Check it before re-issuing an action.
- **`focus_preserved`, `cursor_preserved`**: whether your foreground window and
  cursor were left alone.
- **Errors name the fix.** "Element not found" lists the closest names in that
  window. A refused action names the route that would work.
- **`index`** picks the nth match when several controls share a name; responses
  carry a `matches` count.
- **`ms`** on every response, and `escalated: true` when a lookup needed the
  optional vision tier.
- **Nothing is left running.** Browsers Ghost launches end when the server ends,
  however it ends, and the server sweeps browsers abandoned by earlier servers
  at start (`ghost_stats.orphan_sweep`).
- **Windows never disappear.** Minimised windows stay in `ghost_window list`;
  a window something hid shows with `include_hidden=true` and `op=state
  state=restore` brings it back without activating it.
- **Stop always works.** `ghost_stop` pre-empts the call in flight, and
  Ctrl+Alt+G is the OS-level kill switch ([security model](security-model.md)).

## Shell control

`ghost_shell op=run` is a one-shot command (`powershell`, `pwsh` or `cmd`);
`op=open` starts a persistent PowerShell whose variables and working directory
survive across `op=send` calls. Output is merged stdout and stderr, capped for
the agent's context. A timed-out command keeps running and is drained with
`op=read`. `op=kill` ends a session and every program its commands started, as
does an `op=run` timeout. With the default `powershell`, `op=run` uses a
pre-started process, so a command costs about 85 ms instead of 230 to 450 ms
(`GHOST_SHELL_WARM=off` disables that). To turn the shell off entirely, see
[configuration](configuration.md#the-two-consent-settings).

## Speed habits

Ghost's own time is small: about 75 ms for a text read, 200 ms for a verified
click, 1 ms to list windows. Agent sessions get slow around the calls:

- Name the window once, then omit `window=`.
- Wait for a condition, not a duration: `ghost_wait for=element`, `for=value`,
  `for=navigate` (sets the address bar in the background and returns when the
  title changes, 0.4 to 1.4 s where a fixed sleep costs 6).
- Batch steps with `ghost_run` so one model turn does several.

## CLI

```bash
ghost launch notepad.exe
ghost type --role edit --text "hello from ghost"
ghost press Enter
ghost hotkey --mods Ctrl --key s
ghost screenshot --out shot.png
ghost list-windows
ghost describe --window "Notepad"
ghost click --name "Save"
ghost click-at 500 300
ghost run my-flow.json
```

Everything prints JSON for `jq` or scripts. `ghost doctor` checks the machine,
`ghost verify` audits the MCP server, and `ghost reset` is the human-only way to
lift your own emergency stop.

## Intents: declarative flows

A JSON list of steps, run by a state machine with retries, timeouts and
JSONLogic conditions (`abort_if`, `retry_if`):

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

Run it with `ghost run flow.json`, `POST /run`, or over MCP.

## HTTP API

```bash
ghost-http --addr 127.0.0.1:7878
curl http://127.0.0.1:7878/list-windows
curl -X POST http://127.0.0.1:7878/click -H 'content-type: application/json' -d '{"name":"Submit"}'
curl http://127.0.0.1:7878/screenshot -o shot.png
```

```python
import requests
requests.post("http://127.0.0.1:7878/launch", json={"exe": "notepad.exe"})
requests.post("http://127.0.0.1:7878/type", json={"role": "edit", "text": "hello from python"})
```

Endpoints and the loopback-only rules: [configuration](configuration.md#http-server).

## Rust SDK

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

`By::name("Save")` matches the accessible name (substring), `By::role("edit")`
the control type.

## Architecture

```
ghost-cli     ghost-http     ghost-mcp     Rust SDK
    \            |            /   |           |
     +-----> ghost-session <------+-----------+     shared verbs, locators, act-then-verify
              /           \       |
      ghost-core      ghost-linux |                 one cfg alias picks the engine
     (UIA, posted    (AT-SPI2,    +-> ghost-browser  CDP over the DevTools port
      messages,       XTEST,
      DXGI/GDI)       portals)
```

Supporting crates: `ghost-cache` (UI snapshot and delta), `ghost-intent`
(intent executor), `ghost-ground` (the locator tier cascade), `ghost-platform`
(the per-OS capability matrix). Contributor notes: [CONTRIBUTING.md](../CONTRIBUTING.md).
