# Ghost on Fedora Linux

Ghost's Linux engine is `crates/ghost-linux`. It is a real implementation, not a
scaffold: AT-SPI2 for the accessibility tree and actions, XTEST / RemoteDesktop
portal / uinput for synthetic input, and X11 `GetImage` / the Screenshot portal
for capture.

It is **fully integrated**: `ghost-mcp`, `ghost` (CLI) and `ghost-http` all build
for `x86_64-unknown-linux-gnu`, so the MCP server runs on Fedora with the same 20
verbs it exposes on Windows. `ghost-session` and `ghost-mcp` are shared code --
the locator tiers, grounding cascade, act-then-verify loop and MCP protocol are
the same on both platforms; only the engine underneath changes.

**Status: verified on X11 + AT-SPI2 by automated live tests, on Ubuntu and on
Fedora 41.**

CI stands up a real desktop (Xvfb + D-Bus + `at-spi-bus-launcher`) and runs
**17 live tests against a real GTK application**, on both distros:

- `crates/ghost-linux/tests/live_atspi.rs` (10) — the engine. Both halves of the
  wedge: text written through `EditableText` and read back from the application,
  and `Action.DoAction` dismissing a dialog with an *observable* effect rather
  than merely returning `Ok`. Plus window enumeration with real PIDs, role and
  name lookup, `describe_screen` with real rectangles, capture, XTEST.
- `crates/ghost-mcp/tests/live_mcp_linux.rs` (7) — the product. Spawns the real
  `ghost-mcp` binary and speaks JSON-RPC to it exactly as Claude Code does:
  `tools/list`, `ghost_window`, `ghost_see` returning real roles, `ghost_shell`
  running a real command, and a persistent shell keeping a variable across sends.

CI also runs what a user runs first — `ghost doctor` (exit status enforced),
`ghost list-windows`, `ghost screenshot` — and executes `scripts/install.sh`
end to end. The Fedora job installs exactly the packages listed below, so these
instructions are themselves under test.

`capabilities_for(Linux).functional` is therefore `true`, and lists exactly the
features that suite exercises. **Not** claimed, because nothing has verified them
end to end: `KeyInput`, `EditShortcuts`, `VisionGrounding`, and everything on the
**Wayland** path (RemoteDesktop portal input, Screenshot portal capture) - CI
runs X11. Those are implemented and compile-verified, and the checklist in
[section 3](#3-verify-on-device) is what signs them off on real hardware.

### How the platforms split

```
ghost-mcp      (20 MCP verbs)          shared
ghost-session  (locators, verify, …)   shared
      |
   engine  ──┬── Windows → ghost-core   Win32 UIA, SendInput, posted messages
             └── Linux   → ghost-linux  AT-SPI2, XTEST/portal/uinput, X11/portal
```

`ghost_session::engine` is a one-line `cfg` alias. Both engines expose the same
module tree (`uia`, `input`, `capture`, `system`, `process`, `ocr`, `error`) and
the same signatures, and window handles are `isize` on both (`0` = none) -- an
`HWND` on Windows, an interned AT-SPI `(bus name, object path)` on Linux.

---

## 1. Why the architecture differs from Windows

On Windows, Ghost's differentiator is *background dispatch* — driving an
application without stealing focus or moving the cursor — implemented with
posted window messages.

Linux has a cleaner analogue: **AT-SPI2 accessibility actions**. `Action.DoAction`,
`EditableText.SetTextContents`, `Value.SetCurrentValue` and `Component.GrabFocus`
make the application perform the operation through its own toolkit. No synthetic
input, no pointer movement, no window raise, no consent prompt — and identical
behaviour on X11 and Wayland.

So the priority is inverted relative to Windows:

1. **AT-SPI action** — the primary path.
2. **Synthetic input** — fallback, only for elements exposing no usable action
   (custom canvases, some Chromium/Electron surfaces).

| Session | Input fallback | Capture |
|---|---|---|
| X11 | XTEST (`x11rb`) | `GetImage` on the root window |
| Wayland | RemoteDesktop portal (`ashpd`) | Screenshot portal (one-shot) |
| Any, `GHOST_INPUT=uinput` | `/dev/uinput` virtual device | unchanged |

Everything is **pure Rust**. No `at-spi2-core-devel`, `libX11-devel` or
`pipewire-devel` is needed to build. The Wayland capture path deliberately uses
the Screenshot portal rather than ScreenCast + PipeWire: Ghost captures stills
for verification and vision grounding, never video, so PipeWire would have added
C linkage and DMA-BUF handling for nothing.

---

## 2. Install

### Runtime packages

```bash
sudo dnf install at-spi2-core at-spi2-atk \
                 xdg-desktop-portal xdg-desktop-portal-gnome
```

`at-spi2-core` provides the accessibility bus. `at-spi2-atk` is the ATK bridge
for GTK3-era applications (GTK4 speaks AT-SPI directly and does not need it).
The portal packages are only required for a Wayland session.

No `*-devel` packages are required — see above.

### Enable accessibility

This is the single most common reason for "Ghost sees nothing":

```bash
gsettings set org.gnome.desktop.interface toolkit-accessibility true
```

Merely connecting to the AT-SPI registry does **not** set
`org.a11y.Status.IsEnabled`, and applications watch that property to decide
whether to expose a tree. **Restart any application that was already running**
after changing this.

Per-toolkit, for deterministic trees:

| Toolkit | Requirement |
|---|---|
| GTK4 | none (speaks AT-SPI natively) |
| GTK3 | `at-spi2-atk` installed + the gsetting above |
| Qt 5/6 | launch with `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1` |
| Electron | launch with `--force-renderer-accessibility` |
| Firefox | none, once the gsetting is true |

### Build

```bash
cargo build --release -p ghost-mcp     # the MCP server
cargo build --release -p ghost-cli     # the `ghost` CLI (includes `ghost doctor`)
```

No `*-devel` packages are needed: the whole Linux engine is pure Rust.

### Register with Claude Code

```bash
claude mcp add ghost --scope user -- /path/to/target/release/ghost-mcp
```

### Shell verb

`ghost_shell` uses **bash** on Linux (`sh` and `zsh` are also accepted, and
`pwsh` if you have it installed). Persistent sessions work exactly as on
Windows -- variables, `cwd` and env survive across `send` calls -- using the
same base64 + per-session-nonce framing, so command text is never exposed to
shell quoting.

---

## 3. Verify on-device

Run these in order. Do not report Ghost as working on Linux until all pass.

```bash
# 1. Unit tests (pure logic: roles, handles, keysyms, verification, crop)
cargo test -p ghost-linux

# 2. Is the accessibility bus reachable at all?
#    Should list your running applications. If it errors, step 2 above was skipped.
busctl --user list | grep a11y

# 3. Which session am I on? Decides the input and capture backends.
echo "$XDG_SESSION_TYPE"

# 4. Ghost's own diagnostics
cargo run --release -p ghost-cli -- doctor
```

Then the real behavioural checks, against a GTK application (`gedit`,
`gnome-text-editor`, `nautilus`):

| Check | Expected |
|---|---|
| `ghost_window op=state state=minimize` then `restore` | the window minimises and comes back. Implemented via EWMH and its plumbing is CI-verified, but the *effect* is unverified - CI's container window manager will not iconify a dialog |
| `ghost_window op=state state=close` | the window closes |
| `ghost_window` (list) | your open windows, with real titles and PIDs |
| `ghost_see` | elements with real names, roles and on-screen rectangles |
| `ghost_act` on a button by name | the button activates, **cursor does not move** |
| `ghost_act` typing into a text field | text appears without keystrokes being synthesised |
| `ghost_screenshot` | a real PNG of the screen |
| `ghost_scroll direction=up` **on Wayland** | the page scrolls **up**. The portal follows the Wayland axis convention (positive = down), the opposite of X11 and uinput, so Ghost inverts the sign for it. That inversion is inferred from the spec and unconfirmed on hardware — if scrolling goes the wrong way on Wayland, this is why |

The third and fourth rows are the ones that prove the wedge survived the port:
if the pointer visibly moves, the AT-SPI action path was not taken and something
fell through to synthetic input.

---

## 4. Wayland specifics

**Fedora Workstation defaults to Wayland, so this is probably your session.**
What that means in practice:

| Works on Wayland | Needs X11 (or an XWayland app) |
|---|---|
| Element discovery, `ghost_see`, `ghost_find` | `ghost_window op=state` minimize/maximize/restore (EWMH) |
| `ghost_act` click and type (AT-SPI — never prompts) | Occluded-window capture (XComposite) |
| Clipboard / edit shortcuts (AT-SPI) | Global Ctrl+Alt+G hotkey (XGrabKey) |
| `ghost_screenshot`, `ghost_shell`, `ghost_window` list/focus | |

The X11-only features return a clear `Unsupported` error on Wayland rather than
failing silently, and `ghost doctor` lists them up front under `wayland limits`.

`ghost doctor` never triggers a permission prompt: on Wayland it reports which
input and capture paths *would* be used instead of exercising them. The consent
dialog appears on your first real synthetic input or screenshot, where you
expect it.

**If you decline the consent dialog, or miss it, just try again.** The failure is
not cached — the next call asks again.

### Consent

The first synthetic-input fallback on Wayland raises a GNOME permission dialog.
Ghost requests `PersistMode::ExplicitlyRevoked` and saves the returned restore
token to:

```
${XDG_STATE_HOME:-~/.local/state}/ghost/remote-desktop.token
```

Subsequent runs restore silently. **Tokens are single-use**: Ghost rotates the
saved token on every successful session. If you see a prompt every run, that
file is unwritable or the permission was revoked.

Note that AT-SPI actions — the primary path — never prompt at all. A session
that only ever targets controls by name/role will never see this dialog.

### Unattended machines

```bash
sudo modprobe uinput
# Narrow udev rule — do NOT add a desktop user to a broad input group
echo 'KERNEL=="uinput", GROUP="ghost-input", MODE="0660"' \
  | sudo tee /etc/udev/rules.d/70-ghost-uinput.rules
sudo udevadm control --reload && sudo udevadm trigger
sudo groupadd -f ghost-input && sudo usermod -aG ghost-input <service-account>
```

Then run Ghost with `GHOST_INPUT=uinput`. This bypasses portals entirely — no
dialogs, no session lifetime.

Understand the trade: `/dev/uinput` write access is a machine-wide privilege.
Whoever holds it can inject input into any user's session, including a lock
screen. Grant it to a dedicated service account only.

The uinput backend has two honest limitations, both reported as errors rather
than silently doing the wrong thing:

- **No absolute pointer positioning.** A relative device cannot be commanded to
  screen coordinates. Target controls by name/role instead — AT-SPI drives them
  directly and does not need coordinates.
- **No keysym typing.** Translating the full keysym space to evdev codes needs
  the active keymap. Use AT-SPI text entry.

---

## 5. Known limitations

These are reported as `Unsupported` errors, never faked:

| Limitation | Why |
|---|---|
| Per-window capture on native Wayland | no compositor API exposes it to an ordinary client |
| Root-window capture under XWayland | rootless XWayland has no screen-sized root backing store; `GetImage` returns `BadMatch` |
| XTEST against native Wayland windows | XTEST reaches XWayland clients only — which is why the portal backend is selected on Wayland |
| Local OCR (`find_text_local`) | no always-present Fedora OCR engine; AT-SPI already returns real text with real bounds |
| Global Ctrl+Alt+G emergency hotkey | no X11/Wayland equivalent without grabbing keys globally; `ghost_stop` over MCP is the supported stop |
| `ghost_window` minimize / maximize / restore | AT-SPI exposes no such action and Ghost speaks no window-manager protocol. `close` works (via the window's own accessible action); the others return `Unsupported` rather than silently doing nothing |
| Pointer position under Wayland | neither the portal nor uinput reports it, so `cursor_preserved` is reported from the dispatch path rather than measured. AT-SPI actions never touch the pointer, so this is accurate for the path that matters |
| Applications with no accessibility tree | AT-SPI cannot reconstruct semantics from pixels; vision grounding (`ghost-ground`) is the fallback, exactly as on Windows |

---

## 6. Troubleshooting

| Symptom | Cause |
|---|---|
| "cannot reach the AT-SPI accessibility bus" | `toolkit-accessibility` is false, or no desktop session is running |
| Windows list is empty | accessibility enabled *after* the apps started — restart them |
| A Qt/Electron app shows one empty node | needs `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1` / `--force-renderer-accessibility` |
| Permission dialog on every run | restore-token file unwritable, or permission revoked |
| "absolute pointer motion needs a ScreenCast stream" | the screen-share prompt was declined; accept it, or target by name/role |
| Coordinates land in the wrong place under fractional scaling | portal buffers can be in device pixels while AT-SPI reports logical pixels; prefer name/role targeting |
