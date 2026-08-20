# Ghost

Windows desktop automation framework. Like Playwright, but for native apps.

Any application. Any input. Any agent.

## What is Ghost?

Ghost gives AI agents and developers programmatic control over any Windows application - native Win32, Electron, WPF, or otherwise - and over individual browser tabs.

It runs **in the background by default**: your cursor does not move, your window keeps focus, and nothing comes to the front. You can keep working while it automates, and several ghost processes can run at once on one machine. Apps that expose no automation surface can be launched onto an isolated desktop you never see. Drive it from an agent over MCP, from Rust, or from the `ghost` command line.

## Quick Start

```toml
# Cargo.toml
[dependencies]
ghost-session = { git = "https://github.com/FrostbyteDevTeam/ghost" }
```

```rust
use ghost_session::{GhostSession, By};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let session = GhostSession::new()?;

    // Launch, then find inside that window. Scoping matters: an unscoped search
    // walks every open window and can match somebody else's app.
    session.launch("notepad.exe").await?;
    let edit = session.find_in("Notepad", By::role("document")).await?;
    edit.type_text("hello world")?;   // UIA ValuePattern - no cursor, no focus change

    // Screenshot that one window, without raising it
    let png = session.capture_window("Notepad", false).await?;
    std::fs::write("window.png", png)?;

    Ok(())
}
```

## Background automation (works while you work)

Ghost runs without taking over your screen. Your cursor does not move, the window you
are typing in does not lose focus, and nothing comes to the front. Several ghost
processes can automate different apps and different browser tabs on one machine at
the same time, alongside you.

This is enforced, not aspirational. The default **focus policy** is `background`, and
every primitive that would touch the shared cursor/keyboard/foreground (`SendInput`,
`SetForegroundWindow`) is gated behind it and fails loudly instead of grabbing the
screen. `crates/ghost-core/tests/focus_enforcement.rs` asserts that for the whole
primitive surface.

```json
{ "policy": "background" }          // default: never touch the user's screen
{ "policy": "prefer_background" }   // fall back to real input when nothing else works
{ "policy": "foreground" }          // legacy: drive the real cursor and keyboard
```

### How each surface is driven

| Surface | Mechanism | Background? |
|---|---|---|
| Native / WinForms / WPF / Electron controls | UIA control patterns (Invoke, Value, Toggle, SelectionItem, ExpandCollapse, Scroll, RangeValue, LegacyIAccessible) | yes |
| Controls with no usable pattern | Window messages to the specific child HWND | yes |
| Browsers, per tab | Chrome DevTools Protocol into that tab's renderer | yes |
| Screenshots of one window | `PrintWindow` with `PW_RENDERFULLCONTENT` | yes, even when occluded |
| Screenshots of one tab | `Page.captureScreenshot` with a metrics override | yes, even when not the active tab |
| Apps with no automation surface | launched onto an isolated desktop the user never sees | yes, window messages + UIA + capture |
| Editing shortcuts (undo/copy/paste/select-all) | standard control messages (`WM_UNDO`, `EM_SETSEL`, ...) | yes |

Element actions report which route they took, so you can see when something fell back:

```json
{ "ok": true, "route": "uia:invoke", "background": true }
{ "ok": true, "route": "foreground:sendinput", "background": false }
```

### Browsers and tabs

```
ghost_browser_launch { "id": "work", "mode": "headless" }
ghost_tab_open       { "browser": "work", "url": "https://example.com" }
ghost_tab_describe   { "browser": "work", "tab": "<id>" }
ghost_tab_click      { "browser": "work", "tab": "<id>", "selector": "#submit" }
ghost_tab_screenshot { "browser": "work", "tab": "<id>" }
```

Each `id` gets its own browser process, profile directory, and DevTools port, so
concurrent ghost processes never share cookies or collide on a port. Use
`ghost_browser_attach` with `--remote-debugging-port` when the automation needs your
real logins; ghost will never close a browser it did not start.

### Scope your searches

`ghost_find` and friends take an optional `window`. Without it the search walks every
open window on the machine and can match another app - or another ghost process's
target. Always pass `window` for background work.

### Apps with no automation surface

Some apps expose no UI Automation provider and no child windows: custom-drawn
controls, canvas UIs, kiosk software. For those, launch them onto an **isolated
desktop** - a second Windows desktop that is never displayed. The app's windows never
appear on your screen at all, not even for the instant between launching and being
moved aside. It is the desktop-app equivalent of running a browser headless.

```
ghost run - <<'EOF'
desktop-create --id d1
desktop-launch --desktop d1 --command "someapp.exe"
desktop-wait-for-window --desktop d1 --title "Some App" --save win
desktop-describe --desktop d1
desktop-click --desktop d1 --hwnd $win.hwnd --x 220 --y 140
desktop-capture --desktop d1 --hwnd $win.hwnd -o app.png
desktop-close --id d1
EOF
```

What works there, measured rather than assumed
(`cargo run -p ghost-core --example desktop_input_probe`):

| Mechanism | Works on a non-displayed desktop? |
|---|---|
| UI Automation patterns | yes |
| Window messages (click, type, scroll) | yes |
| `PrintWindow` capture | yes |
| `SendInput` real input | **no** - `ERROR_ACCESS_DENIED` |

An app must be *launched* onto a desktop; Windows cannot move an existing window
there.

### What genuinely cannot run in the background

Stated plainly, because silently faking these is worse than failing:

- **Apps that read raw input state directly** - `GetAsyncKeyState`, DirectInput, most
  games. These need real hardware-level input, and Windows refuses `SendInput` from
  any thread whose desktop is not the one on screen. That is an OS boundary, not a
  missing feature: no user-space technique reaches those apps without taking over the
  real screen. Set the focus policy to `foreground` deliberately, and ghost will
  serialize that access behind a cross-process lease.
- **Keyboard shortcuts with no message equivalent** (Ctrl+S, app-specific chords).
  Posted key messages cannot set modifier key state, so a faked Ctrl+Z types a literal
  `z`. Ghost tries the standard control message, then the command that advertises the
  accelerator in the app's automation tree, then errors. It never fakes a modifier.
  Undo, cut, copy, paste, clear, and select-all all have message equivalents and work
  everywhere.
- **Hardware-overlay video surfaces** render nothing into a `PrintWindow` DC. Ghost
  detects the empty buffer, recovers by cropping a screen capture when it can verify
  the window is genuinely visible and unoccluded, and otherwise errors rather than
  returning a black image.

### Multiple Claude sessions at once

Every Claude session gets its own ghost MCP server process, and they compose without
coordination: separate browsers, separate ports, separate profiles, separate isolated
desktops. Background input is per-window and per-tab, so nothing contends for the
mouse, keyboard, or foreground - you keep typing while all of them work.

Within one session, parallel tool calls execute in parallel: each request runs as its
own task, and a slow wait in one tab never delays a click in another. Responses are
correlated by JSON-RPC id, so out-of-order completion is safe.

**Emergency stop is machine-wide**: Ctrl+Alt+G (or `ghost_stop` from any session)
halts every ghost process at once via a shared kernel event; `ghost_reset` resumes
them all.

### Choosing a browser

`ghost_browser_launch` accepts `"browser": "comet" | "chrome" | "edge" | "brave"`;
`ghost_browser_list_installed` shows what is available. All are Chromium-family and
driven identically over CDP. Each launch is an isolated instance with its own profile
- to drive your *own* logged-in browser instead, start it with
`--remote-debugging-port=9222` and use `ghost_browser_attach`.

### Performance

Typical steady-state cost per operation (`cargo run --release -p ghost-session --example bench`):

| Operation | Cost |
|---|---:|
| Browser click / type / read text | 0.3 - 1.5ms |
| Browser screenshot | ~64ms |
| `find_in(window, role)` | ~6ms |
| `describe_screen(window)` | ~36ms |
| `capture_window` | ~20ms |
| Background type into a window | ~10µs |
| Browser launch (cold) | ~350ms |

Two things dominate if you are not careful, both avoidable:

- **Scope your searches.** `describe_screen(None)` walks every window on the machine
  (~127ms); scoped to one window it is ~36ms. Same for `find` vs `find_in`.
- **Prefer text over pixels.** `ghost_tab_describe` and `ghost_document_text` are
  sub-millisecond and give an agent more to work with than a screenshot costing 64ms.

### Verify it yourself

The claims above are falsifiable, so the product ships its own audit:

```bash
ghost verify
```

Eleven checks, each with a hard timing budget: background policy enforced,
screen-stealing calls refused, three tabs driven concurrently with no cross-talk,
click and screenshot latency, a fast call completing while a slow call is in
flight, two extra ghost processes running their own browsers alongside, emergency
stop and resume, and - measured across the whole run - the foreground window
untouched. Exits nonzero if any claim does not hold on that machine. Run it in
every new environment before trusting ghost there.

Development proofs (require the repo):

```bash
cargo run -p ghost-browser --example browser_background_proof   # 3 tabs at once
cargo run -p ghost-session --example desktop_background_proof   # app + real undo, untouched screen
cargo run -p ghost-session --example concurrency_proof -- 4     # 4 ghost processes at once
cargo run -p ghost-core    --example isolated_desktop_proof     # app on an invisible desktop
cargo run -p ghost-core    --example desktop_input_probe        # what works on a hidden desktop
```

Each measures the foreground window and cursor before and after, and fails if either
moved. Keep typing in another window while they run.

## Command line

`ghost` exposes the same tools as the MCP server, sharing one dispatch layer so the
two can never drift apart.

```bash
cargo build -p ghost-cli --release      # produces target/release/ghost.exe

ghost tools                              # every command
ghost tools browser                      # filtered
ghost help shortcut-background           # parameters for one command

ghost list-windows
ghost describe-screen --window Notepad
ghost type-background --window Notepad --text "hello"
ghost shortcut-background --window Notepad --shortcut undo
ghost capture-window --window Notepad -o shot.png
ghost desktop-state                      # did anything touch my screen?
```

The `ghost_` prefix is optional and `-` works in place of `_`, so `list-windows`,
`list_windows`, and `ghost_list_windows` are the same command. Values parse as JSON
when unambiguous (`--x 40`, `--clear false`, `--modifiers '["Ctrl"]'`) and as strings
otherwise. `-o FILE` writes a returned PNG to disk instead of printing base64.

### Sessions

Isolated desktops and launched browsers live only as long as the process that created
them, so a one-shot `ghost desktop-create` would create a desktop and immediately
destroy it. `ghost run` executes a script in a single process, with `--save` binding a
result for later `$name.field` references:

```
ghost run - <<'EOF'
browser-launch --id b1 --mode headless
tab-open --browser b1 --url https://example.com --save t
tab-describe --browser b1 --tab $t.tab
tab-screenshot --browser b1 --tab $t.tab -o page.png
browser-close --id b1
EOF
```

## Emergency Stop

Press **Ctrl+Alt+G** at any time to immediately halt all automation.
- All queued actions are cancelled
- Any held modifier keys (Shift, Ctrl, Alt) are released immediately
- No stuck keys, no stuck modifier states

## MCP Server (for AI agents)

Build and add to Claude Code as an MCP server:

```bash
cargo build -p ghost-mcp --release
```

Add to Claude Code settings:
```json
{
  "mcpServers": {
    "ghost": {
      "command": "path/to/ghost-mcp.exe"
    }
  }
}
```

### Available Tools

Under the default `background` policy the legacy foreground tools (`ghost_click_at`,
`ghost_press`, `ghost_hotkey`, `ghost_hover`, `ghost_right_click`,
`ghost_double_click`, `ghost_drag`, `ghost_scroll`, `ghost_focus_window`,
`ghost_navigate_and_wait`) return `NoBackgroundPath`. Reach for the `*_background` and
`ghost_tab_*` equivalents instead; they are listed first.

#### Background (default policy)

**Focus policy and self-check**
| Tool | Parameters | Description |
|------|-----------|-------------|
| `ghost_focus_policy` | - | Report the current policy |
| `ghost_set_focus_policy` | `policy` | background / prefer_background / foreground |
| `ghost_desktop_state` | - | Foreground window and cursor, to verify nothing moved |

**Element actions (no cursor, no focus change)**
| Tool | Parameters | Description |
|------|-----------|-------------|
| `ghost_element_actions` | `window?`, `name`/`role` | Which background actions this element supports |
| `ghost_toggle` | `window?`, `name`/`role` | Toggle a checkbox |
| `ghost_select` | `window?`, `name`/`role` | Select a tab / list item / radio |
| `ghost_expand` | `window?`, `name`/`role`, `expand?` | Open or close a combo box or tree item |
| `ghost_scroll_element` | `window?`, `name`/`role`, `direction`, `amount?` | Scroll a container |
| `ghost_scroll_into_view` | `window?`, `name`/`role` | Bring an element into view |
| `ghost_set_range_value` | `window?`, `name`/`role`, `value` | Set a slider or spinner |
| `ghost_document_text` | `window?`, `name`/`role`, `max_chars?` | Full document text via TextPattern |

**Window-scoped input**
| Tool | Parameters | Description |
|------|-----------|-------------|
| `ghost_type_background` | `window`, `text` | Type into a background window |
| `ghost_press_background` | `window`, `key` | Send a key to a background window |
| `ghost_hotkey_background` | `window`, `modifiers[]`, `key` | Invoke the command owning that accelerator |
| `ghost_shortcut_background` | `window`, `shortcut` | undo/cut/copy/paste/clear/select_all via control messages |
| `ghost_set_text_background` | `window`, `text` | Replace a control's text in one message |
| `ghost_click_background` | `window`, `x`, `y` | Click a client-area point |
| `ghost_right_click_background` | `window`, `x`, `y` | Right-click |
| `ghost_double_click_background` | `window`, `x`, `y` | Double-click |
| `ghost_hover_background` | `window`, `x`, `y` | Hover |
| `ghost_scroll_background` | `window`, `x`, `y`, `direction`, `amount?` | Wheel-scroll |
| `ghost_click_element_background` | `window`, `name`/`role` | Find by locator, click via window messages |
| `ghost_capture_window` | `window`, `client_only?` | PNG of one window, even if occluded |

**Isolated desktops (apps with no automation surface)**
| Tool | Parameters | Description |
|------|-----------|-------------|
| `ghost_desktop_create` | `id?` | Create a desktop the user never sees |
| `ghost_desktop_close` | `id?` | Destroy it, terminating anything still running on it |
| `ghost_desktop_launch` | `desktop?`, `command` | Launch a program onto it |
| `ghost_desktop_windows` | `desktop?` | Visible windows, with handles |
| `ghost_desktop_wait_for_window` | `desktop?`, `title`, `timeout_ms?` | Wait for a window to appear |
| `ghost_desktop_click` | `desktop?`, `hwnd`, `x`, `y`, `button?` | Click a point |
| `ghost_desktop_scroll` | `desktop?`, `hwnd`, `x?`, `y?`, `direction?`, `amount?` | Scroll |
| `ghost_desktop_type` | `desktop?`, `hwnd`, `text` | Type |
| `ghost_desktop_press` | `desktop?`, `hwnd`, `key` | Send a key |
| `ghost_desktop_shortcut` | `desktop?`, `hwnd`, `shortcut` | Editing shortcut |
| `ghost_desktop_capture` | `desktop?`, `hwnd`, `client_only?` | See what the invisible app is doing |
| `ghost_desktop_describe` | `desktop?`, `window?` | Interactive elements via UIA |
| `ghost_desktop_click_element` | `desktop?`, `window?`, `name`/`role` | Activate an element by name or role |

**Browsers and tabs**
| Tool | Parameters | Description |
|------|-----------|-------------|
| `ghost_browser_launch` | `id?`, `mode?` | Isolated browser: own process, profile, port |
| `ghost_browser_attach` | `id?`, `port` | Attach to your running browser |
| `ghost_browser_close` | `id?` | Close a browser ghost launched |
| `ghost_browser_tabs` | `id?` | List tabs with ids, titles, URLs |
| `ghost_tab_open` | `browser?`, `url?` | Open a background tab |
| `ghost_tab_close` | `browser?`, `tab` | Close a tab |
| `ghost_tab_find` | `browser?`, `query` | Find a tab by URL or title |
| `ghost_tab_navigate` | `browser?`, `tab`, `url`, `timeout_ms?` | Navigate and wait for load |
| `ghost_tab_click` | `browser?`, `tab`, `selector` | Trusted click inside the renderer |
| `ghost_tab_type` | `browser?`, `tab`, `selector`, `text`, `clear?` | Focus and type |
| `ghost_tab_press` | `browser?`, `tab`, `key`, `modifiers[]?` | Send a key |
| `ghost_tab_text` | `browser?`, `tab`, `selector?` | Visible text |
| `ghost_tab_eval` | `browser?`, `tab`, `expression` | Evaluate JS, awaits promises |
| `ghost_tab_screenshot` | `browser?`, `tab`, `full_page?` | PNG of a background tab |
| `ghost_tab_describe` | `browser?`, `tab`, `limit?` | Interactive elements with selectors |
| `ghost_tab_scroll` | `browser?`, `tab`, `selector?`, `dx?`, `dy?` | Scroll |
| `ghost_tab_select_option` | `browser?`, `tab`, `selector`, `value` | Choose a `<select>` option |
| `ghost_tab_wait_for` | `browser?`, `tab`, `selector`, `timeout_ms?` | Wait for a selector |
| `ghost_tab_info` | `browser?`, `tab` | URL and title |

#### Legacy foreground tools

**Element interaction**
| Tool | Parameters | Description |
|------|-----------|-------------|
| `ghost_find` | `name` or `role` | Find element by accessible name or control type |
| `ghost_click` | `name` or `role` | Find and click an element |
| `ghost_type` | `name`/`role`, `text` | Find element and type text into it |
| `ghost_click_at` | `x`, `y` | Left-click at absolute screen coordinates |
| `ghost_get_text` | `name` or `role` | Read text value from a found element |

**Keyboard**
| Tool | Parameters | Description |
|------|-----------|-------------|
| `ghost_press` | `key` | Press a named key: Enter, Tab, Escape, F1-F12, ArrowUp, etc. |
| `ghost_hotkey` | `modifiers[]`, `key` | Modifier combo: Ctrl+C, Alt+F4, Win+D |
| `ghost_key_down` | `key` | Hold key down (for Ctrl+drag, Shift+click) |
| `ghost_key_up` | `key` | Release held key |

**Mouse**
| Tool | Parameters | Description |
|------|-----------|-------------|
| `ghost_hover` | `x`, `y` | Move mouse without clicking (triggers dropdowns) |
| `ghost_right_click` | `x`, `y` | Right-click at coordinates |
| `ghost_double_click` | `x`, `y` | Double-click at coordinates |
| `ghost_drag` | `from_x`, `from_y`, `to_x`, `to_y` | Click-hold, drag, release |
| `ghost_scroll` | `x`, `y`, `direction`, `amount?` | Scroll up/down/left/right |

**Clipboard**
| Tool | Parameters | Description |
|------|-----------|-------------|
| `ghost_get_clipboard` | — | Read current clipboard text |
| `ghost_set_clipboard` | `text` | Write text to clipboard |

**Screen & perception**
| Tool | Parameters | Description |
|------|-----------|-------------|
| `ghost_screenshot` | — | Capture screen as base64 PNG |
| `ghost_describe_screen` | `window?` | List interactive elements with names, roles, positions |

**Window management**
| Tool | Parameters | Description |
|------|-----------|-------------|
| `ghost_list_windows` | — | All visible top-level windows with name, pid, focused |
| `ghost_focus_window` | `name` | Bring window to foreground by partial name |
| `ghost_window_state` | `name`, `state` | maximize / minimize / restore / close |

**Process & control**
| Tool | Parameters | Description |
|------|-----------|-------------|
| `ghost_launch` | `exe` | Launch process, returns pid |
| `ghost_wait` | `ms` | Wait N milliseconds |
| `ghost_stop` | — | Emergency stop: halt all automation |
| `ghost_reset` | — | Resume after stop |

## Element Locators

```rust
// By accessible name (case-insensitive substring)
session.find(By::name("Save")).await?

// By control type role
session.find(By::role("edit")).await?    // text input
session.find(By::role("button")).await?  // button
session.find(By::role("checkbox")).await?
session.find(By::role("list")).await?
```

## Architecture

```
ghost-session  ← developer/agent API (safe Rust)
     │
ghost-core     ← Win32 FFI: UIA, SendInput, DXGI (unsafe Rust)
     │
Windows OS     ← UIA COM, user32.dll, DXGI
```

## Requirements

- Windows 10 or later
- Rust stable

## License

MIT - Copyright 2026 Frostbyte Digital
