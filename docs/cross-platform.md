# Ghost on three platforms

Ghost ships as three versions that share one contract (`crates/ghost-platform`):

| Platform | Status | Engine |
| --- | --- | --- |
| **Windows** | ✅ full, verified | `ghost-core` / `ghost-session` — Win32 UI Automation, SendInput + posted window messages, DXGI/GDI capture |
| **macOS** | 🚧 scaffold (not functional) | native backend on Accessibility + CGEvent + ScreenCaptureKit — to be built on a Mac |
| **Linux** | ✅ functional; X11 + AT-SPI2 verified by live CI tests, Wayland paths unverified | `ghost-linux` — AT-SPI2 over D-Bus, XTEST / RemoteDesktop portal / uinput, X11 `GetImage` / Screenshot portal. See [linux-fedora.md](linux-fedora.md) |

Windows remains the most capable. Linux is functional: its engine (`ghost-linux`)
is implemented and its X11 + AT-SPI2 paths are verified by a live CI suite that
drives a real GTK application on a synthesised desktop — the Wayland portal paths
are implemented but not yet verified on hardware, and are not claimed. macOS is
still a compiling scaffold with a precise implementation map, and must be built
and **verified on a Mac**. Nothing here claims to work on an OS it hasn't been
tested on.

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

**The wedge, measured:** background control without stealing focus is built on
posted window messages on Windows, which have no exact equivalent elsewhere. On
Linux the AT-SPI action APIs turned out to be a *cleaner* analogue, not a weaker
one — the application performs the operation through its own toolkit, so there is
nothing to raise and no pointer to move. `BackgroundDispatch` is therefore
claimed on Linux, on the strength of live tests that write text through
`EditableText` and invoke a button through `Action.DoAction` with observable
effects. On macOS it remains **unknown → measure**.

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

This applied to Linux until the engine was built, and still applies to macOS: the
development machine is Windows-only, with no macOS SDK.

Linux got past it without a Linux box because the whole engine is **pure Rust**
(`atspi`/`zbus`, `x11rb`, `ashpd`, `evdev`), so it cross-compiles and type-checks
from Windows, and because CI can synthesise a real desktop (Xvfb + D-Bus +
`at-spi-bus-launcher`) and run live tests against a real GTK application. macOS
has no equivalent escape hatch — its APIs are Objective-C FFI and its runners are
not free to synthesise — so it stays a scaffold until a Mac is in the loop.
