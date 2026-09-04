# Ghost Desktop Automation - Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a Windows desktop automation framework in Rust - a Playwright equivalent for native apps, exposed as both a Rust crate and MCP server.

**Architecture:** Three-crate Cargo workspace: `ghost-core` owns all Win32/UIA/COM FFI in unsafe Rust; `ghost-session` wraps it in a safe, ergonomic async API; `ghost-mcp` is a ~200-line MCP server binary over stdio.

**Tech Stack:** Rust, `windows` crate (Microsoft's official Win32/COM bindings), `tokio` (async), `thiserror` (errors), `tracing` (logging), `serde_json` (MCP protocol), `image` crate (screenshot PNG encoding).

---

## Task 1: Cargo workspace scaffold

**Files:**
- Create: `Cargo.toml`
- Create: `crates/ghost-core/Cargo.toml`
- Create: `crates/ghost-core/src/lib.rs`
- Create: `crates/ghost-session/Cargo.toml`
- Create: `crates/ghost-session/src/lib.rs`
- Create: `crates/ghost-mcp/Cargo.toml`
- Create: `crates/ghost-mcp/src/main.rs`
- Create: `.gitignore`
- Create: `LICENSE` (MIT)

**Step 1: Write workspace `Cargo.toml`**

```toml
[workspace]
members = [
    "crates/ghost-core",
    "crates/ghost-session",
    "crates/ghost-mcp",
]
resolver = "2"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
thiserror = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
image = { version = "0.25", default-features = false, features = ["png"] }
windows = { version = "0.58", features = [
    "Win32_Foundation",
    "Win32_System_Com",
    "Win32_System_Threading",
    "Win32_UI_Accessibility",
    "Win32_UI_Input_KeyboardAndMouse",
    "Win32_UI_WindowsAndMessaging",
    "Win32_Graphics_Dxgi",
    "Win32_Graphics_Dxgi_Common",
    "Win32_Graphics_Direct3D11",
    "Win32_Graphics_Direct3D",
] }
```

**Step 2: Write `crates/ghost-core/Cargo.toml`**

```toml
[package]
name = "ghost-core"
version = "0.1.0"
edition = "2021"

[dependencies]
windows = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
tokio = { workspace = true }
```

**Step 3: Write `crates/ghost-session/Cargo.toml`**

```toml
[package]
name = "ghost-session"
version = "0.1.0"
edition = "2021"

[dependencies]
ghost-core = { path = "../ghost-core" }
tokio = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
image = { workspace = true }
```

**Step 4: Write `crates/ghost-mcp/Cargo.toml`**

```toml
[package]
name = "ghost-mcp"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "ghost-mcp"
path = "src/main.rs"

[dependencies]
ghost-session = { path = "../ghost-session" }
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
```

**Step 5: Write skeleton `lib.rs` files**

`crates/ghost-core/src/lib.rs`:
```rust
pub mod error;
pub mod input;
pub mod uia;
pub mod capture;
pub mod process;
```

`crates/ghost-session/src/lib.rs`:
```rust
pub mod error;
pub mod locator;
pub mod element;
pub mod session;

pub use session::GhostSession;
pub use locator::By;
pub use element::GhostElement;
pub use error::GhostError;
```

`crates/ghost-mcp/src/main.rs`:
```rust
fn main() {
    println!("ghost-mcp stub");
}
```

**Step 6: Write `.gitignore`**

```
/target
Cargo.lock
```

**Step 7: Verify it compiles**

```bash
cd <repo-root>
cargo build 2>&1
```
Expected: compiles with zero errors (warnings OK).

**Step 8: Commit**

```bash
git add -A
git commit -m "chore: workspace scaffold, three-crate skeleton"
```

---

## Task 2: Error types

**Files:**
- Create: `crates/ghost-core/src/error.rs`
- Create: `crates/ghost-session/src/error.rs`
- Create: `crates/ghost-core/src/lib.rs` (add `pub use error::CoreError`)

**Step 1: Write `ghost-core` error type**

`crates/ghost-core/src/error.rs`:
```rust
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("Win32 error {code:#010x} in {context}")]
    Win32 { code: u32, context: &'static str },

    #[error("COM initialization failed: {0}")]
    ComInit(String),

    #[error("UIA not available for process: {process}")]
    UiaUnavailable { process: String },

    #[error("Process not found: {name}")]
    ProcessNotFound { name: String },
}
```

**Step 2: Write `ghost-session` error type**

`crates/ghost-session/src/error.rs`:
```rust
use crate::locator::By;

#[derive(Debug, thiserror::Error)]
pub enum GhostError {
    #[error("Element not found: {query}")]
    ElementNotFound { query: String, screenshot: Option<Vec<u8>> },

    #[error("Element not interactable: {element} - {reason}")]
    ElementNotInteractable { element: String, reason: String },

    #[error("Ghost stopped by emergency stop (Ctrl+Alt+G)")]
    Stopped,

    #[error("Timeout after {ms}ms waiting for: {action}")]
    Timeout { action: String, ms: u64 },

    #[error("UIA unavailable for app: {app}")]
    UiaUnavailable { app: String },

    #[error("Process not found: {name}")]
    ProcessNotFound { name: String },

    #[error("Core error: {0}")]
    Core(#[from] ghost_core::error::CoreError),
}

pub type Result<T> = std::result::Result<T, GhostError>;
```

**Step 3: Verify compilation**

```bash
cargo build 2>&1
```
Expected: zero errors.

**Step 4: Commit**

```bash
git add crates/
git commit -m "feat: error types for ghost-core and ghost-session"
```

---

## Task 3: Emergency stop - STOP_FLAG and hotkey registration

**Files:**
- Create: `crates/ghost-core/src/input/mod.rs`
- Create: `crates/ghost-core/src/input/hotkey.rs`

**Step 1: Write unit test for STOP_FLAG first**

`crates/ghost-core/src/input/hotkey.rs` (top of file):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_flag_starts_false() {
        // Reset first in case another test set it
        STOP_FLAG.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(!is_stopped());
    }

    #[test]
    fn stop_flag_set_and_reset() {
        STOP_FLAG.store(false, std::sync::atomic::Ordering::SeqCst);
        trigger_stop();
        assert!(is_stopped());
        reset_stop();
        assert!(!is_stopped());
    }
}
```

**Step 2: Run test - verify it fails**

```bash
cargo test -p ghost-core 2>&1
```
Expected: compile error - `STOP_FLAG`, `is_stopped`, `trigger_stop`, `reset_stop` not defined.

**Step 3: Implement `hotkey.rs`**

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use crate::error::CoreError;

pub static STOP_FLAG: AtomicBool = AtomicBool::new(false);

pub fn is_stopped() -> bool {
    STOP_FLAG.load(Ordering::SeqCst)
}

pub fn trigger_stop() {
    STOP_FLAG.store(true, Ordering::SeqCst);
}

pub fn reset_stop() {
    STOP_FLAG.store(false, Ordering::SeqCst);
}

/// Register Ctrl+Alt+G as a global hotkey (ID=1).
/// Spawns a background thread that listens for WM_HOTKEY messages.
/// On trigger: sets STOP_FLAG, releases all modifier keys.
pub fn register_emergency_stop() -> Result<(), CoreError> {
    unsafe {
        let ok = RegisterHotKey(
            None,
            1,
            MOD_CONTROL | MOD_ALT,
            b'G' as u32,
        );
        if ok.is_err() {
            let code = windows::Win32::Foundation::GetLastError().0;
            return Err(CoreError::Win32 { code, context: "RegisterHotKey" });
        }
    }

    std::thread::spawn(|| {
        let mut msg = MSG::default();
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message == WM_HOTKEY && msg.wParam.0 == 1 {
                    tracing::warn!("Emergency stop triggered (Ctrl+Alt+G)");
                    trigger_stop();
                    release_all_modifiers();
                }
            }
        }
    });

    Ok(())
}

/// Send key-up events for all modifier keys so no key stays stuck.
pub fn release_all_modifiers() {
    let modifiers = [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN];
    let inputs: Vec<INPUT> = modifiers.iter().map(|&vk| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                dwFlags: KEYEVENTF_KEYUP,
                ..Default::default()
            },
        },
    }).collect();

    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}
```

**Step 4: Write `input/mod.rs`**

```rust
pub mod hotkey;
pub mod keyboard;
pub mod mouse;

pub use hotkey::{is_stopped, register_emergency_stop, reset_stop, trigger_stop, STOP_FLAG};
```

**Step 5: Run tests - verify they pass**

```bash
cargo test -p ghost-core input 2>&1
```
Expected: 2 tests pass.

**Step 6: Commit**

```bash
git add crates/ghost-core/
git commit -m "feat: emergency stop - Ctrl+Alt+G global hotkey, STOP_FLAG"
```

---

## Task 4: Keyboard input (SendInput)

**Files:**
- Create: `crates/ghost-core/src/input/keyboard.rs`

**Step 1: Write unit tests first**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_down_event_has_correct_vk() {
        let input = key_event(VK_A, false);
        unsafe {
            assert_eq!(input.Anonymous.ki.wVk, VK_A);
            assert_eq!(input.Anonymous.ki.dwFlags, KEYBD_EVENT_FLAGS(0));
        }
    }

    #[test]
    fn key_up_event_has_keyup_flag() {
        let input = key_event(VK_A, true);
        unsafe {
            assert_eq!(input.Anonymous.ki.dwFlags, KEYEVENTF_KEYUP);
        }
    }

    #[test]
    fn text_to_inputs_produces_pairs() {
        // "ab" = key_down(A) + key_up(A) + key_down(B) + key_up(B)
        let inputs = text_to_inputs("ab");
        assert_eq!(inputs.len(), 4);
    }
}
```

**Step 2: Run - verify fails**

```bash
cargo test -p ghost-core keyboard 2>&1
```
Expected: compile error.

**Step 3: Implement `keyboard.rs`**

```rust
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use crate::error::CoreError;
use super::hotkey::is_stopped;

pub fn key_event(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                dwFlags: if key_up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                ..Default::default()
            },
        },
    }
}

pub fn text_to_inputs(text: &str) -> Vec<INPUT> {
    let mut inputs = Vec::new();
    for ch in text.chars() {
        // Use KEYEVENTF_UNICODE for arbitrary Unicode text
        let down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wScan: ch as u16,
                    dwFlags: KEYEVENTF_UNICODE,
                    ..Default::default()
                },
            },
        };
        let up = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wScan: ch as u16,
                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    ..Default::default()
                },
            },
        };
        inputs.push(down);
        inputs.push(up);
    }
    inputs
}

/// Type a string into the focused application.
/// Checks STOP_FLAG between each character.
pub fn type_text(text: &str) -> Result<(), CoreError> {
    for ch in text.chars() {
        if is_stopped() {
            return Err(CoreError::Win32 { code: 0, context: "stopped" });
        }
        let inputs = text_to_inputs(&ch.to_string());
        unsafe {
            SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }
    Ok(())
}

/// Press and release a virtual key.
pub fn press_key(vk: VIRTUAL_KEY) -> Result<(), CoreError> {
    if is_stopped() {
        return Err(CoreError::Win32 { code: 0, context: "stopped" });
    }
    let inputs = [key_event(vk, false), key_event(vk, true)];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
    Ok(())
}
```

**Step 4: Run tests - verify pass**

```bash
cargo test -p ghost-core keyboard 2>&1
```
Expected: 3 tests pass.

**Step 5: Commit**

```bash
git add crates/ghost-core/src/input/keyboard.rs
git commit -m "feat: keyboard SendInput - type_text, press_key, STOP_FLAG check"
```

---

## Task 5: Mouse input (SendInput)

**Files:**
- Create: `crates/ghost-core/src/input/mouse.rs`

**Step 1: Write unit tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_event_uses_absolute_flag() {
        let input = move_event(500, 400);
        unsafe {
            assert!(input.Anonymous.mi.dwFlags.contains(
                MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE
            ));
        }
    }

    #[test]
    fn click_event_uses_left_down_flag() {
        let input = click_event(false);
        unsafe {
            assert!(input.Anonymous.mi.dwFlags.contains(MOUSEEVENTF_LEFTDOWN));
        }
    }

    #[test]
    fn click_event_uses_left_up_flag() {
        let input = click_event(true);
        unsafe {
            assert!(input.Anonymous.mi.dwFlags.contains(MOUSEEVENTF_LEFTUP));
        }
    }
}
```

**Step 2: Run - verify fails**

```bash
cargo test -p ghost-core mouse 2>&1
```

**Step 3: Implement `mouse.rs`**

```rust
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use crate::error::CoreError;
use super::hotkey::is_stopped;

// Windows absolute coordinates are 0-65535 regardless of screen size
fn to_absolute(x: i32, y: i32) -> (i32, i32) {
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        ((x * 65535) / sw, (y * 65535) / sh)
    }
}

pub fn move_event(x: i32, y: i32) -> INPUT {
    let (ax, ay) = to_absolute(x, y);
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: ax,
                dy: ay,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
                ..Default::default()
            },
        },
    }
}

pub fn click_event(up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: if up { MOUSEEVENTF_LEFTUP } else { MOUSEEVENTF_LEFTDOWN },
                ..Default::default()
            },
        },
    }
}

/// Move mouse to (x, y) and left-click.
pub fn click(x: i32, y: i32) -> Result<(), CoreError> {
    if is_stopped() {
        return Err(CoreError::Win32 { code: 0, context: "stopped" });
    }
    let inputs = [move_event(x, y), click_event(false), click_event(true)];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
    Ok(())
}

/// Move mouse without clicking.
pub fn move_to(x: i32, y: i32) -> Result<(), CoreError> {
    if is_stopped() {
        return Err(CoreError::Win32 { code: 0, context: "stopped" });
    }
    let inputs = [move_event(x, y)];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
    Ok(())
}
```

**Step 4: Run tests - verify pass**

```bash
cargo test -p ghost-core mouse 2>&1
```
Expected: 3 tests pass.

**Step 5: Commit**

```bash
git add crates/ghost-core/src/input/mouse.rs
git commit -m "feat: mouse SendInput - move, click with absolute coordinate mapping"
```

---

## Task 6: UIA COM initialization and element struct

**Files:**
- Create: `crates/ghost-core/src/uia/mod.rs`
- Create: `crates/ghost-core/src/uia/element.rs`

**Step 1: Write unit tests for element**

```rust
// In element.rs tests - test the safe wrapper behavior, not COM itself
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_role_name_formats_correctly() {
        // We test the formatting/display logic without a real COM object
        let role = role_id_to_name(0x0000_0032); // UIA_ButtonControlTypeId = 50
        assert_eq!(role, "button");
    }
}
```

**Step 2: Run - verify fails**

```bash
cargo test -p ghost-core uia 2>&1
```

**Step 3: Write `uia/mod.rs`**

```rust
pub mod element;
pub mod tree;
pub mod patterns;

use windows::Win32::System::Com::*;
use crate::error::CoreError;

/// Initialize COM in multithreaded apartment mode.
/// Must be called once per thread that uses UIA.
pub fn init_com() -> Result<(), CoreError> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .map_err(|e| CoreError::ComInit(e.to_string()))
    }
}
```

**Step 4: Write `uia/element.rs`**

```rust
use windows::Win32::UI::Accessibility::*;
use windows::Win32::Foundation::RECT;

#[derive(Debug, Clone)]
pub struct BoundingRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl BoundingRect {
    pub fn center(&self) -> (i32, i32) {
        ((self.left + self.right) / 2, (self.top + self.bottom) / 2)
    }
}

pub struct UiaElement(pub IUIAutomationElement);

impl UiaElement {
    pub fn name(&self) -> String {
        unsafe {
            self.0.CurrentName()
                .map(|s| s.to_string())
                .unwrap_or_default()
        }
    }

    pub fn control_type(&self) -> u32 {
        unsafe { self.0.CurrentControlType().unwrap_or(0) }
    }

    pub fn bounding_rect(&self) -> Option<BoundingRect> {
        unsafe {
            self.0.CurrentBoundingRectangle().ok().map(|r| BoundingRect {
                left: r.left,
                top: r.top,
                right: r.right,
                bottom: r.bottom,
            })
        }
    }

    pub fn is_enabled(&self) -> bool {
        unsafe { self.0.CurrentIsEnabled().unwrap_or(false).as_bool() }
    }
}

pub fn role_id_to_name(id: u32) -> &'static str {
    match id {
        50 => "button",
        42 => "edit",
        50023 => "document",
        50004 => "checkbox",
        50034 => "list",
        50008 => "menu",
        50020 => "tab",
        50033 => "toolbar",
        50021 => "text",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_role_name_formats_correctly() {
        assert_eq!(role_id_to_name(50), "button");
    }

    #[test]
    fn bounding_rect_center_is_correct() {
        let r = BoundingRect { left: 100, top: 200, right: 300, bottom: 400 };
        assert_eq!(r.center(), (200, 300));
    }
}
```

**Step 5: Run tests - verify pass**

```bash
cargo test -p ghost-core uia 2>&1
```
Expected: 2 tests pass.

**Step 6: Commit**

```bash
git add crates/ghost-core/src/uia/
git commit -m "feat: UIA COM init, UiaElement wrapper with name/role/bounding_rect"
```

---

## Task 7: UIA tree walker (find elements)

**Files:**
- Create: `crates/ghost-core/src/uia/tree.rs`
- Create: `crates/ghost-core/src/uia/patterns.rs`

**Step 1: Write `tree.rs`**

```rust
use windows::Win32::UI::Accessibility::*;
use super::element::UiaElement;
use crate::error::CoreError;

pub struct UiaTree {
    automation: IUIAutomation,
}

impl UiaTree {
    pub fn new() -> Result<Self, CoreError> {
        unsafe {
            use windows::Win32::System::Com::CoCreateInstance;
            use windows::core::GUID;
            let automation: IUIAutomation = CoCreateInstance(
                &CUIAutomation as *const _,
                None,
                windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
            ).map_err(|e| CoreError::ComInit(e.to_string()))?;
            Ok(Self { automation })
        }
    }

    /// Find first element matching name (case-insensitive substring).
    pub fn find_by_name(&self, name: &str) -> Result<Option<UiaElement>, CoreError> {
        let name_lower = name.to_lowercase();
        unsafe {
            let root = self.automation.GetRootElement()
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            self.search_subtree(&root, &name_lower)
        }
    }

    /// Find first element matching control type role name.
    pub fn find_by_role(&self, role: &str) -> Result<Option<UiaElement>, CoreError> {
        unsafe {
            let root = self.automation.GetRootElement()
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            self.search_by_role(&root, role)
        }
    }

    unsafe fn search_subtree(
        &self,
        element: &IUIAutomationElement,
        name: &str,
    ) -> Result<Option<UiaElement>, CoreError> {
        let el = UiaElement(element.clone());
        if el.name().to_lowercase().contains(name) {
            return Ok(Some(el));
        }
        // Walk children
        let walker = self.automation.ControlViewWalker()
            .map_err(|e| CoreError::ComInit(e.to_string()))?;
        let mut child = walker.GetFirstChildElement(element).ok();
        while let Some(c) = child {
            if let Some(found) = self.search_subtree(&c, name)? {
                return Ok(Some(found));
            }
            child = walker.GetNextSiblingElement(&c).ok();
        }
        Ok(None)
    }

    unsafe fn search_by_role(
        &self,
        element: &IUIAutomationElement,
        role: &str,
    ) -> Result<Option<UiaElement>, CoreError> {
        let el = UiaElement(element.clone());
        if super::element::role_id_to_name(el.control_type()) == role {
            return Ok(Some(el));
        }
        let walker = self.automation.ControlViewWalker()
            .map_err(|e| CoreError::ComInit(e.to_string()))?;
        let mut child = walker.GetFirstChildElement(element).ok();
        while let Some(c) = child {
            if let Some(found) = self.search_by_role(&c, role)? {
                return Ok(Some(found));
            }
            child = walker.GetNextSiblingElement(&c).ok();
        }
        Ok(None)
    }
}
```

**Step 2: Write `patterns.rs`**

```rust
use windows::Win32::UI::Accessibility::*;
use super::element::UiaElement;
use crate::error::CoreError;

/// Click an element via InvokePattern (preferred) or fallback to coordinates.
pub fn invoke(element: &UiaElement) -> Result<(), CoreError> {
    unsafe {
        if let Ok(pattern) = element.0.GetCurrentPattern(UIA_InvokePatternId) {
            let invoke: IUIAutomationInvokePattern = pattern.cast()
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            invoke.Invoke().map_err(|e| CoreError::Win32 {
                code: e.code().0 as u32,
                context: "InvokePattern",
            })?;
            return Ok(());
        }
    }
    // Fallback: click center of bounding rect
    if let Some(rect) = element.bounding_rect() {
        let (cx, cy) = rect.center();
        crate::input::mouse::click(cx, cy)?;
    }
    Ok(())
}

/// Set value via ValuePattern (for text inputs).
pub fn set_value(element: &UiaElement, value: &str) -> Result<(), CoreError> {
    unsafe {
        if let Ok(pattern) = element.0.GetCurrentPattern(UIA_ValuePatternId) {
            let vp: IUIAutomationValuePattern = pattern.cast()
                .map_err(|e| CoreError::ComInit(e.to_string()))?;
            let bstr = windows::core::BSTR::from(value);
            vp.SetValue(&bstr).map_err(|e| CoreError::Win32 {
                code: e.code().0 as u32,
                context: "ValuePattern.SetValue",
            })?;
            return Ok(());
        }
    }
    // Fallback: focus element, type text via keyboard
    if let Some(rect) = element.bounding_rect() {
        let (cx, cy) = rect.center();
        crate::input::mouse::click(cx, cy)?;
    }
    crate::input::keyboard::type_text(value)
}
```

**Step 3: Verify compilation**

```bash
cargo build -p ghost-core 2>&1
```
Expected: zero errors.

**Step 4: Commit**

```bash
git add crates/ghost-core/
git commit -m "feat: UIA tree walker, InvokePattern, ValuePattern with keyboard fallback"
```

---

## Task 8: Screen capture (DXGI)

**Files:**
- Create: `crates/ghost-core/src/capture/mod.rs`
- Create: `crates/ghost-core/src/capture/screen.rs`

**Step 1: Write `capture/mod.rs`**

```rust
pub mod screen;
pub use screen::capture_screen;
```

**Step 2: Write `capture/screen.rs`**

```rust
use crate::error::CoreError;

/// Capture the primary monitor as a PNG-encoded byte vec.
pub fn capture_screen() -> Result<Vec<u8>, CoreError> {
    unsafe {
        use windows::Win32::Graphics::Dxgi::*;
        use windows::Win32::Graphics::Dxgi::Common::*;
        use windows::Win32::Graphics::Direct3D11::*;
        use windows::Win32::Graphics::Direct3D::*;

        // Create D3D11 device
        let mut device = None;
        let mut context = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        ).map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "D3D11CreateDevice" })?;

        let device = device.unwrap();

        // Get DXGI output and duplicate it
        let dxgi_device: IDXGIDevice = device.cast()
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "IDXGIDevice cast" })?;
        let adapter = dxgi_device.GetAdapter()
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "GetAdapter" })?;
        let output: IDXGIOutput = adapter.EnumOutputs(0)
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "EnumOutputs" })?;
        let output1: IDXGIOutput1 = output.cast()
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "IDXGIOutput1 cast" })?;
        let duplication = output1.DuplicateOutput(&device)
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "DuplicateOutput" })?;

        // Acquire a frame
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        duplication.AcquireNextFrame(500, &mut frame_info, &mut resource)
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "AcquireNextFrame" })?;

        let resource = resource.unwrap();
        let texture: ID3D11Texture2D = resource.cast()
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "texture cast" })?;

        // Get texture description for dimensions
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);

        // Create staging texture (CPU-readable)
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ,
            BindFlags: D3D11_BIND_FLAG(0),
            MiscFlags: D3D11_RESOURCE_MISC_FLAG(0),
            ..desc
        };
        let mut staging = None;
        device.CreateTexture2D(&staging_desc, None, Some(&mut staging))
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "CreateTexture2D staging" })?;
        let staging = staging.unwrap();

        // Copy frame to staging
        let ctx = context.unwrap();
        ctx.CopyResource(&staging, &texture);

        // Map and read pixels
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "Map" })?;

        let width = desc.Width as usize;
        let height = desc.Height as usize;
        let pitch = mapped.RowPitch as usize;
        let data = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * height);

        // Convert BGRA to RGBA
        let mut rgba = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let src = y * pitch + x * 4;
                let dst = (y * width + x) * 4;
                rgba[dst]     = data[src + 2]; // R
                rgba[dst + 1] = data[src + 1]; // G
                rgba[dst + 2] = data[src];     // B
                rgba[dst + 3] = 255;           // A
            }
        }

        ctx.Unmap(&staging, 0);
        duplication.ReleaseFrame()
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "ReleaseFrame" })?;

        // Encode to PNG
        let mut png_bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png_bytes)
            .encode(&rgba, width as u32, height as u32, image::ColorType::Rgba8)
            .map_err(|_| CoreError::Win32 { code: 0, context: "PNG encode" })?;

        Ok(png_bytes)
    }
}
```

**Step 3: Add `image` to `ghost-core/Cargo.toml`**

```toml
image = { workspace = true }
```

**Step 4: Verify compilation**

```bash
cargo build -p ghost-core 2>&1
```
Expected: zero errors.

**Step 5: Commit**

```bash
git add crates/ghost-core/
git commit -m "feat: DXGI screen capture to PNG bytes"
```

---

## Task 9: Process manager

**Files:**
- Create: `crates/ghost-core/src/process/mod.rs`
- Create: `crates/ghost-core/src/process/manager.rs`

**Step 1: Write unit tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonexistent_process_returns_none() {
        // "definitely_not_a_real_process_12345.exe" won't be running
        let pid = find_pid_by_name("definitely_not_a_real_process_12345.exe");
        assert!(pid.is_none());
    }
}
```

**Step 2: Run - verify fails**

```bash
cargo test -p ghost-core process 2>&1
```

**Step 3: Implement `manager.rs`**

```rust
use windows::Win32::System::Threading::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::Foundation::*;
use crate::error::CoreError;

/// Find the PID of a running process by executable name (e.g. "notepad.exe").
pub fn find_pid_by_name(name: &str) -> Option<u32> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let proc_name = String::from_utf16_lossy(
                    &entry.szExeFile[..entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(260)]
                );
                if proc_name.eq_ignore_ascii_case(name) {
                    let _ = CloseHandle(snapshot);
                    return Some(entry.th32ProcessID);
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
        None
    }
}

/// Launch a process by executable path or name.
/// Returns the PID.
pub fn launch(exe: &str) -> Result<u32, CoreError> {
    unsafe {
        let mut si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        let mut cmd: Vec<u16> = exe.encode_utf16().chain(std::iter::once(0)).collect();
        CreateProcessW(
            None,
            windows::core::PWSTR(cmd.as_mut_ptr()),
            None, None, false,
            PROCESS_CREATION_FLAGS(0),
            None, None,
            &si, &mut pi,
        ).map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "CreateProcessW" })?;
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);
        Ok(pi.dwProcessId)
    }
}

/// Kill a process by PID.
pub fn kill(pid: u32) -> Result<(), CoreError> {
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, false, pid)
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "OpenProcess" })?;
        TerminateProcess(handle, 1)
            .map_err(|e| CoreError::Win32 { code: e.code().0 as u32, context: "TerminateProcess" })?;
        let _ = CloseHandle(handle);
        Ok(())
    }
}
```

**Step 4: Write `process/mod.rs`**

```rust
pub mod manager;
pub use manager::{find_pid_by_name, launch, kill};
```

**Step 5: Run tests - verify pass**

```bash
cargo test -p ghost-core process 2>&1
```
Expected: 1 test passes.

**Step 6: Commit**

```bash
git add crates/ghost-core/src/process/
git commit -m "feat: process manager - launch, find by name, kill"
```

---

## Task 10: `ghost-session` - By locator and GhostSession

**Files:**
- Create: `crates/ghost-session/src/locator.rs`
- Create: `crates/ghost-session/src/session.rs`

**Step 1: Write locator unit tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_name_stores_name() {
        let loc = By::name("Save");
        assert!(matches!(loc, By::Name(n) if n == "Save"));
    }

    #[test]
    fn by_role_stores_role() {
        let loc = By::role("edit");
        assert!(matches!(loc, By::Role(r) if r == "edit"));
    }

    #[test]
    fn by_locator_display() {
        assert_eq!(By::name("OK").to_string(), "name=OK");
        assert_eq!(By::role("button").to_string(), "role=button");
    }
}
```

**Step 2: Run - verify fails**

```bash
cargo test -p ghost-session locator 2>&1
```

**Step 3: Implement `locator.rs`**

```rust
use std::fmt;

#[derive(Debug, Clone)]
pub enum By {
    Name(String),
    Role(String),
}

impl By {
    pub fn name(n: impl Into<String>) -> Self { By::Name(n.into()) }
    pub fn role(r: impl Into<String>) -> Self { By::Role(r.into()) }
}

impl fmt::Display for By {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            By::Name(n) => write!(f, "name={}", n),
            By::Role(r) => write!(f, "role={}", r),
        }
    }
}
```

**Step 4: Run - verify pass**

```bash
cargo test -p ghost-session locator 2>&1
```
Expected: 3 tests pass.

**Step 5: Write `session.rs`**

```rust
use std::time::Duration;
use tokio::time::timeout;
use ghost_core::{
    capture::capture_screen,
    input::hotkey::{register_emergency_stop, is_stopped, reset_stop},
    process::manager as proc,
    uia::{init_com, tree::UiaTree},
};
use crate::{
    element::GhostElement,
    error::{GhostError, Result},
    locator::By,
};

pub struct Region {
    pub full: bool,
}

impl Region {
    pub fn full() -> Self { Region { full: true } }
}

pub struct GhostSession {
    timeout_ms: u64,
    tree: UiaTree,
}

impl GhostSession {
    pub fn new() -> Result<Self> {
        init_com().map_err(GhostError::Core)?;
        register_emergency_stop().map_err(GhostError::Core)?;
        let tree = UiaTree::new().map_err(GhostError::Core)?;
        Ok(Self { timeout_ms: 5000, tree })
    }

    pub fn with_timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }

    pub async fn find(&self, by: By) -> Result<GhostElement> {
        if is_stopped() { return Err(GhostError::Stopped); }
        let action = by.to_string();
        let ms = self.timeout_ms;

        let result = timeout(Duration::from_millis(ms), async {
            // Retry loop - element may not exist yet
            loop {
                if is_stopped() { return Err(GhostError::Stopped); }
                let found = match &by {
                    By::Name(n) => self.tree.find_by_name(n).map_err(GhostError::Core)?,
                    By::Role(r) => self.tree.find_by_role(r).map_err(GhostError::Core)?,
                };
                if let Some(el) = found {
                    return Ok(GhostElement::new(el));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }).await;

        match result {
            Ok(r) => r,
            Err(_) => {
                let screenshot = capture_screen().ok();
                Err(GhostError::ElementNotFound { query: action, screenshot })
            }
        }
    }

    pub async fn click_at(&self, x: i32, y: i32) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        ghost_core::input::mouse::click(x, y).map_err(GhostError::Core)
    }

    pub async fn screenshot(&self, _region: Region) -> Result<Vec<u8>> {
        capture_screen().map_err(GhostError::Core)
    }

    pub async fn launch(&self, exe: &str) -> Result<u32> {
        proc::launch(exe).map_err(GhostError::Core)
    }

    pub fn stop(&self) {
        ghost_core::input::hotkey::trigger_stop();
        ghost_core::input::hotkey::release_all_modifiers();
    }

    pub fn reset(&self) {
        reset_stop();
    }
}
```

**Step 6: Verify compilation**

```bash
cargo build -p ghost-session 2>&1
```
Expected: zero errors.

**Step 7: Commit**

```bash
git add crates/ghost-session/
git commit -m "feat: GhostSession with find(), click_at(), screenshot(), launch(), stop()"
```

---

## Task 11: `ghost-session` GhostElement

**Files:**
- Create: `crates/ghost-session/src/element.rs`

**Step 1: Write `element.rs`**

```rust
use ghost_core::uia::{element::UiaElement, patterns};
use crate::error::{GhostError, Result};
use ghost_core::input::hotkey::is_stopped;

pub struct GhostElement {
    inner: UiaElement,
}

impl GhostElement {
    pub(crate) fn new(inner: UiaElement) -> Self {
        Self { inner }
    }

    pub fn name(&self) -> String {
        self.inner.name()
    }

    pub async fn click(&self) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        if !self.inner.is_enabled() {
            return Err(GhostError::ElementNotInteractable {
                element: self.inner.name(),
                reason: "element is disabled".into(),
            });
        }
        patterns::invoke(&self.inner).map_err(GhostError::Core)
    }

    pub async fn type_text(&self, text: &str) -> Result<()> {
        if is_stopped() { return Err(GhostError::Stopped); }
        if !self.inner.is_enabled() {
            return Err(GhostError::ElementNotInteractable {
                element: self.inner.name(),
                reason: "element is disabled".into(),
            });
        }
        patterns::set_value(&self.inner, text).map_err(GhostError::Core)
    }

    pub fn bounding_rect(&self) -> Option<(i32, i32, i32, i32)> {
        self.inner.bounding_rect().map(|r| (r.left, r.top, r.right, r.bottom))
    }
}
```

**Step 2: Verify compilation**

```bash
cargo build -p ghost-session 2>&1
```
Expected: zero errors.

**Step 3: Commit**

```bash
git add crates/ghost-session/src/element.rs
git commit -m "feat: GhostElement with click(), type_text(), disabled guard"
```

---

## Task 12: Integration tests (Notepad + Calculator)

**Files:**
- Create: `crates/ghost-session/tests/notepad.rs`
- Create: `crates/ghost-session/tests/calculator.rs`

**Step 1: Write Notepad test**

`crates/ghost-session/tests/notepad.rs`:
```rust
use ghost_session::{GhostSession, By};

// Run with: cargo test -p ghost-session --test notepad -- --nocapture
// Requires: Windows, Notepad available at notepad.exe
#[tokio::test]
#[ignore] // remove #[ignore] to run manually
async fn test_type_in_notepad() {
    let session = GhostSession::new().unwrap();
    let pid = session.launch("notepad.exe").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    let edit = session.find(By::role("edit")).await.unwrap();
    edit.type_text("hello ghost").await.unwrap();

    // Take screenshot as proof
    let png = session.screenshot(ghost_session::session::Region::full()).await.unwrap();
    assert!(!png.is_empty(), "screenshot should not be empty");

    // Cleanup
    ghost_core::process::kill(pid).ok();
}
```

**Step 2: Write Calculator test**

`crates/ghost-session/tests/calculator.rs`:
```rust
use ghost_session::{GhostSession, By};

#[tokio::test]
#[ignore]
async fn test_click_calculator_button() {
    let session = GhostSession::new().unwrap();
    let pid = session.launch("calc.exe").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

    // Click the "1" button
    let btn = session.find(By::name("One")).await.unwrap();
    btn.click().await.unwrap();

    ghost_core::process::kill(pid).ok();
}
```

**Step 3: Run unit tests (not integration - those need display)**

```bash
cargo test -p ghost-session 2>&1
```
Expected: locator unit tests pass, integration tests skipped (marked `#[ignore]`).

**Step 4: Commit**

```bash
git add crates/ghost-session/tests/
git commit -m "test: Notepad and Calculator integration tests (manual, #[ignore])"
```

---

## Task 13: MCP server

**Files:**
- Modify: `crates/ghost-mcp/src/main.rs`

**Step 1: Implement MCP server**

```rust
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ghost_session::{GhostSession, By, session::Region};
use std::io::{BufRead, Write};

#[derive(Deserialize)]
struct McpRequest {
    id: Value,
    method: String,
    params: Option<Value>,
}

#[derive(Serialize)]
struct McpResponse {
    id: Value,
    result: Option<Value>,
    error: Option<Value>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let session = match GhostSession::new() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to init GhostSession: {}", e);
            std::process::exit(1);
        }
    };

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    for line in stdin.lock().lines() {
        let line = match line { Ok(l) => l, Err(_) => break };
        if line.trim().is_empty() { continue; }

        let req: McpRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let _ = writeln!(out, "{}", json!({"error": e.to_string()}));
                continue;
            }
        };

        let result = handle(&session, &req.method, req.params.as_ref()).await;

        let resp = match result {
            Ok(v) => McpResponse { id: req.id, result: Some(v), error: None },
            Err(e) => McpResponse { id: req.id, result: None, error: Some(json!({"message": e})) },
        };
        let _ = writeln!(out, "{}", serde_json::to_string(&resp).unwrap());
        let _ = out.flush();
    }
}

async fn handle(session: &GhostSession, method: &str, params: Option<&Value>) -> Result<Value, String> {
    let p = params.cloned().unwrap_or(json!({}));

    match method {
        "ghost_find" => {
            let by = parse_by(&p)?;
            let el = session.find(by).await.map_err(|e| e.to_string())?;
            Ok(json!({ "name": el.name(), "bounding_rect": el.bounding_rect() }))
        }
        "ghost_click" => {
            let by = parse_by(&p)?;
            let el = session.find(by).await.map_err(|e| e.to_string())?;
            el.click().await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_type" => {
            let by = parse_by(&p)?;
            let text = p["text"].as_str().ok_or("missing text")?;
            let el = session.find(by).await.map_err(|e| e.to_string())?;
            el.type_text(text).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_click_at" => {
            let x = p["x"].as_i64().ok_or("missing x")? as i32;
            let y = p["y"].as_i64().ok_or("missing y")? as i32;
            session.click_at(x, y).await.map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true }))
        }
        "ghost_screenshot" => {
            let png = session.screenshot(Region::full()).await.map_err(|e| e.to_string())?;
            Ok(json!({ "png_base64": base64_encode(&png) }))
        }
        "ghost_launch" => {
            let exe = p["exe"].as_str().ok_or("missing exe")?;
            let pid = session.launch(exe).await.map_err(|e| e.to_string())?;
            Ok(json!({ "pid": pid }))
        }
        "ghost_stop" => {
            session.stop();
            Ok(json!({ "ok": true }))
        }
        _ => Err(format!("unknown method: {}", method)),
    }
}

fn parse_by(p: &Value) -> Result<By, String> {
    if let Some(n) = p["name"].as_str() { return Ok(By::name(n)); }
    if let Some(r) = p["role"].as_str() { return Ok(By::role(r)); }
    Err("params must include 'name' or 'role'".into())
}

fn base64_encode(data: &[u8]) -> String {
    use std::fmt::Write;
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = if chunk.len() > 1 { chunk[1] as usize } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as usize } else { 0 };
        let _ = write!(out, "{}{}{}{}", 
            TABLE[b0 >> 2] as char,
            TABLE[((b0 & 3) << 4) | (b1 >> 4)] as char,
            if chunk.len() > 1 { TABLE[((b1 & 0xf) << 2) | (b2 >> 6)] as char } else { '=' },
            if chunk.len() > 2 { TABLE[b2 & 0x3f] as char } else { '=' },
        );
    }
    out
}
```

**Step 2: Build MCP binary**

```bash
cargo build -p ghost-mcp --release 2>&1
```
Expected: produces `target/release/ghost-mcp.exe`, zero errors.

**Step 3: Commit**

```bash
git add crates/ghost-mcp/src/main.rs
git commit -m "feat: ghost-mcp MCP server - 7 tools over stdio JSON-RPC"
```

---

## Task 14: GitHub Actions CI

**Files:**
- Create: `.github/workflows/ci.yml`

**Step 1: Write CI workflow**

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always

jobs:
  test:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Build
        run: cargo build --all
      - name: Unit tests
        run: cargo test --all -- --skip test_type_in_notepad --skip test_click_calculator_button
      - name: Build MCP release binary
        run: cargo build -p ghost-mcp --release
```

**Step 2: Commit**

```bash
mkdir -p .github/workflows
git add .github/
git commit -m "ci: GitHub Actions - build + unit tests on windows-latest"
```

---

## Task 15: Example and README

**Files:**
- Create: `examples/notepad_hello.rs`
- Create: `README.md`

**Step 1: Write example**

`examples/notepad_hello.rs`:
```rust
use ghost_session::{GhostSession, By, session::Region};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let session = GhostSession::new()?;

    println!("Launching Notepad...");
    session.launch("notepad.exe").await?;
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    println!("Finding text area...");
    let edit = session.find(By::role("edit")).await?;
    edit.type_text("Hello from Ghost!").await?;

    println!("Taking screenshot...");
    let png = session.screenshot(Region::full()).await?;
    std::fs::write("screenshot.png", &png)?;
    println!("Saved screenshot.png ({} bytes)", png.len());

    Ok(())
}
```

**Step 2: Add to workspace Cargo.toml**

```toml
[[example]]
name = "notepad_hello"
path = "examples/notepad_hello.rs"
```

**Step 3: Write `README.md`**

```markdown
# Ghost

Windows desktop automation framework. Like Playwright, but for native apps.

## Quick Start

```rust
use ghost_session::{GhostSession, By};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let session = GhostSession::new()?;
    session.launch("notepad.exe").await?;
    let edit = session.find(By::role("edit")).await?;
    edit.type_text("hello").await?;
    Ok(())
}
```

## Emergency Stop

Press **Ctrl+Alt+G** at any time to immediately halt all automation.
All held modifier keys are released. No stuck keys.

## MCP Server

```bash
cargo build -p ghost-mcp --release
# Add target/release/ghost-mcp.exe as an MCP server in Claude Code
```

Available MCP tools: `ghost_find`, `ghost_click`, `ghost_type`, `ghost_click_at`, `ghost_screenshot`, `ghost_launch`, `ghost_stop`

## License

MIT
```

**Step 4: Final build and test**

```bash
cargo build --all 2>&1
cargo test --all -- --skip test_type_in_notepad --skip test_click_calculator_button 2>&1
```
Expected: all pass.

**Step 5: Final commit**

```bash
git add -A
git commit -m "feat: notepad example, README, MIT license"
```

---

## Manual QA Checklist (run yourself after build)

Run these in order, report any failures:

```bash
# 1. Build release binary
cargo build --release

# 2. Run Notepad integration test (needs display)
cargo test -p ghost-session --test notepad -- --ignored --nocapture

# 3. Run Calculator integration test
cargo test -p ghost-session --test calculator -- --ignored --nocapture

# 4. Run the example
cargo run --example notepad_hello
# Verify: Notepad opens, "Hello from Ghost!" appears, screenshot.png saved

# 5. Test emergency stop
# Run an automation, hit Ctrl+Alt+G, verify it halts cleanly

# 6. Test MCP server
cargo build -p ghost-mcp --release
echo '{"id":1,"method":"ghost_launch","params":{"exe":"notepad.exe"}}' | ./target/release/ghost-mcp.exe
```
