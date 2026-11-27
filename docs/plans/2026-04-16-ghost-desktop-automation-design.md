# Ghost - Desktop Automation Framework
**Date:** 2026-04-16  
**Status:** Approved  
**Platform:** Windows-first  
**Language:** Rust  

---

## Overview

Ghost is an open source desktop automation framework for Windows. Like Playwright for browsers, but for the entire desktop - any native app, any input, any agent. Developers use it as a Rust crate; AI agents connect via MCP.

---

## Architecture

Three-crate workspace:

```
ghost/
├── crates/
│   ├── ghost-core/        # Win32/UIA/input primitives (unsafe layer)
│   ├── ghost-session/     # High-level automation API (safe Rust)
│   └── ghost-mcp/         # MCP server binary (~200 lines)
├── examples/
└── docs/
```

**Layer model:**
```
Agent / Developer
      │
      ▼
ghost-session   ← public API: find_element(), click(), type_text(), screenshot()
      │
      ▼
ghost-core      ← Win32 UIA tree, SendInput, DXGI capture, process management
      │
      ▼
Windows OS      ← UIA COM, user32.dll, DXGI, kernel32
```

All Win32 unsafe FFI is contained in `ghost-core`. Nothing leaks into `ghost-session`. `ghost-mcp` is a thin JSON-RPC shell over `ghost-session`.

---

## Core Components

### `ghost-core`

```
input/
  keyboard.rs    # SendInput key events, hold/release, text injection
  mouse.rs       # SendInput mouse move/click/scroll, absolute + relative
  hotkey.rs      # RegisterHotKey - Ctrl+Alt+G emergency stop
uia/
  tree.rs        # Walk the UI Automation element tree
  element.rs     # Element: name, role, bounding_rect, is_enabled
  patterns.rs    # InvokePattern, ValuePattern, SelectionPattern
capture/
  screen.rs      # DXGI screen capture → raw RGBA buffer
process/
  manager.rs     # Launch, find, kill processes by name/PID
```

### `ghost-session` public API

```rust
let session = GhostSession::new()?;

let btn = session.find(By::name("Save")).await?;
btn.click().await?;

let input = session.find(By::role("edit").near("Username")).await?;
input.type_text("hello").await?;

session.click_at(420, 880).await?;

let img = session.screenshot(Region::full()).await?;

session.stop(); // also fires on Ctrl+Alt+G
```

### `ghost-mcp` MCP tool surface

| MCP Tool | Maps to |
|---|---|
| `ghost_find` | `session.find()` |
| `ghost_click` | `element.click()` |
| `ghost_type` | `element.type_text()` |
| `ghost_screenshot` | `session.screenshot()` |
| `ghost_launch` | `session.launch()` |
| `ghost_stop` | `session.stop()` |

---

## Emergency Stop

**Trigger:** `Ctrl+Alt+G` (global OS hotkey via `RegisterHotKey`)  
**Fires:** on any thread, even mid-automation in another app

**Sequence on trigger:**
1. Set `AtomicBool STOP_FLAG = true` (checked between every queued action)
2. Send key-up events for all modifier keys (Shift, Ctrl, Alt, Win)
3. Drain the action queue
4. Log: last action being executed at stop time
5. Return `Err(GhostError::Stopped)` to caller

The stop flag is checked **between** actions, not mid-keystroke. Keyboard is never left in a broken modifier state.

**Configurable:** default chord is `Ctrl+Alt+G`, overridable at session init.

---

## Data Flow

### Normal flow
```
ghost_click(By::name("Save"))
  │
  ▼
Query UIA tree → find element matching "Save"
  │
  ├─ Found → get bounding_rect → compute center → SendInput click
  │
  └─ Not found → Err(ElementNotFound { screenshot })
```

### Fallback chain
```
find(By::name("Save"))
  ├─ UIA tree search → found ✓
  └─ not found → return Err with screenshot attached
     (no silent fallback to coordinates - explicit failure)
```

---

## Error Handling

```rust
pub enum GhostError {
    ElementNotFound { query: String, screenshot: Option<Vec<u8>> },
    ElementNotInteractable { element: String, reason: String },
    Stopped,
    Timeout { action: String, ms: u64 },
    UiaUnavailable { app: String },
    Win32Error { code: u32, context: String },
    ProcessNotFound { name: String },
}
```

**Rules:**
- No panics in `ghost-session` - all failures surface as `GhostError`
- Default 5s timeout per action, configurable
- `ElementNotFound` always attaches a screenshot of current screen state
- `UiaUnavailable` is not fatal - agent decides whether to use coordinates or bail

---

## Testing Strategy

### Unit tests (`ghost-core`)
Pure logic: locator matching, tree walking, queue ordering, stop flag behavior. No Win32 calls. Run anywhere.

### Integration tests (`ghost-session`)
Always-available Windows apps: Notepad, Calculator, Paint.

```rust
#[test]
async fn test_type_in_notepad() {
    let s = GhostSession::new().unwrap();
    s.launch("notepad.exe").await.unwrap();
    s.find(By::role("edit")).await.unwrap()
     .type_text("hello ghost").await.unwrap();
}
```

### Manual QA targets (human-run)
- Claude Code terminal
- VS Code
- Chrome / Firefox
- File Explorer
- Electron apps

### CI
GitHub Actions `windows-latest` runner. Unit + integration on every PR.

---

## Distribution

- Open source (license TBD - MIT or Apache-2.0)
- Published to crates.io as `ghost-core`, `ghost-session`, `ghost-mcp`
- MCP server binary distributed via GitHub Releases
- Docs site via GitHub Pages
