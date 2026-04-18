# Ghost

Windows desktop automation framework. Like Playwright, but for native apps.

Any application. Any input. Any agent.

## What is Ghost?

Ghost gives AI agents and developers programmatic control over any Windows application — native Win32, Electron, WPF, or otherwise. It uses the Windows UI Automation API for element discovery, Win32 SendInput for keyboard/mouse injection, and DXGI for screen capture.

## Quick Start

```toml
# Cargo.toml
[dependencies]
ghost-session = { git = "https://github.com/FrostbyteDevTeam/ghost" }
```

```rust
use ghost_session::{GhostSession, By, session::Region};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let session = GhostSession::new()?;

    // Launch and find
    session.launch("notepad.exe").await?;
    let edit = session.find(By::role("edit")).await?;
    edit.type_text("hello world")?;

    // Screenshot
    let png = session.screenshot(Region::full()).await?;
    std::fs::write("screen.png", png)?;

    Ok(())
}
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

### Available Tools (24)

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
