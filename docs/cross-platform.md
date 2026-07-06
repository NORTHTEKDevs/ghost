# Ghost on three platforms

Ghost ships as three versions that share one contract (`crates/ghost-platform`):

| Platform | Status | Engine |
| --- | --- | --- |
| **Windows** | ✅ full, verified | `ghost-core` / `ghost-session` — Win32 UI Automation, SendInput + posted window messages, DXGI/GDI capture |
| **macOS** | 🚧 scaffold (not functional) | native backend on Accessibility + CGEvent + ScreenCaptureKit — to be built on a Mac |
| **Linux** | 🚧 scaffold (not functional) | native backend on AT-SPI (D-Bus) + XTest/libei + X11/portal capture — to be built on Linux |

Windows is intentionally the most capable and is the only one verified today. The
macOS and Linux backends are real, compiling scaffolds (`ghost-platform` builds for
all three targets) with a precise implementation map — but their native engines
must be written and **verified on those machines**. Nothing here claims to work on
an OS it hasn't been tested on.

## The contract

`ghost-platform` defines the shared vocabulary (`Rect`, `Locator`, `ActionKind`,
`WindowRef`, `ElementInfo`), the `Feature`/`Capabilities` model (the honest
per-OS status), and the `Backend` trait each OS implements. `capabilities_for(os)`
is the single source of truth for what Ghost can do where. A backend flips
`functional: true` only after its native code is built and tested on-device.

## Capability-to-API map

| Capability | Windows (done) | macOS | Linux |
| --- | --- | --- | --- |
| Element discovery / roles / **enabled** | UI Automation (UIA) | Accessibility `AXUIElement` (kAXRole/kAXEnabled) | AT-SPI `Accessible.GetRole`/`GetState` |
| Act (click/press) | InvokePattern | `AXUIElementPerformAction(kAXPress)` | AT-SPI `Action.DoAction` |
| Type | ValuePattern / SendInput | `AXUIElementSetAttributeValue(kAXValue)` / CGEvent | `EditableText.SetTextContents` / XTest |
| **Background (no focus steal)** | **posted window messages** (unique) | AX value-set + press — *measure if it activates* | AT-SPI actions — *measure if it raises* |
| Per-action verify | screen-delta + read-back | same idea (CGWindow capture + AX read) | same idea (capture + AT-SPI read) |
| Screenshot | DXGI/GDI | ScreenCaptureKit / `CGWindowListCreateImage` | X11 `XGetImage` / Wayland portal |
| Key input | SendInput / WM_KEYDOWN | `CGEventCreateKeyboardEvent` | XTest / libei |
| Edit shortcuts (Ctrl+C/V/…) | WM_COPY/CUT/PASTE/UNDO | AX + `NSPasteboard` / CGEvent | AT-SPI + clipboard (X11/Wayland) |
| Vision grounding | `ghost-ground` (OS-agnostic) | reuse `ghost-ground` | reuse `ghost-ground` |

**The honest caveat on the wedge:** Ghost's standout — background control without
stealing focus — is built on Windows posted window messages, which have no exact
equivalent on macOS/Linux. The AX/AT-SPI action APIs are the closest analogue but
may activate/raise the target on some apps. So `BackgroundDispatch` should be
treated as **unknown → measure** on macOS/Linux, and only claimed once tested. It
may end up PARTIAL there. This is the main reason Windows stays the flagship.

## How to finish a platform (on that OS)

1. Add native deps under the target section of `crates/ghost-platform/Cargo.toml`
   (macOS: `accessibility-sys`, `core-graphics`, `objc2*`; Linux: `atspi`,
   `x11rb`, `ashpd`).
2. Implement the operations in `macos.rs` / `linux.rs` per the map above and the
   per-method notes already in those files.
3. Extend `capabilities_for(os)` to list the `Feature`s you've actually verified.
4. **Verify on-device**: build for the native target (`aarch64-apple-darwin` /
   `x86_64-unknown-linux-gnu`) and run the same live checks the Windows engine
   passes — element discovery, act-then-verify, and (measured) background dispatch.
5. Flip `functional: true` only when those checks pass on a real machine.

Wayland vs X11 is the biggest Linux fork (input + capture differ sharply); design
the Linux backend to detect the session type and pick XTest/XGetImage vs
libei/portals accordingly.

## Why not build the native backends here

They were written where they can't be verified — this repo's development machine is
Windows-only, with no macOS SDK and no Linux desktop session. Shipping native
FFI that can't be compiled or run would be a guess, not an implementation. The
scaffold + this map is the honest maximum until a Mac and a Linux box are in the
loop.
