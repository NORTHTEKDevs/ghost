# Background mode

Ghost acts **without stealing your focus or moving your cursor**, so an agent
drives one app while you keep working in another. Since 0.19 that is the
default, since 0.22 an agent cannot turn it off, and since 0.23 a window that
grabs the foreground on its own is handed straight back. This page explains how,
and what was measured.

## The policy

The process-wide focus policy starts at `background` and every primitive that
could only work by taking the real cursor or foreground window is gated behind
it. There is no silent fallback: a call with no background path returns an
error naming the action and the route that needs no policy change (launch the
app on a hidden desktop, or drive the browser over CDP).

The policy is **locked**. A default is only a promise if the agent cannot flip
it, so `ghost_set_focus_policy foreground` is refused with an error saying so.
The key is the operator's: `GHOST_FOCUS_LOCK=off` in the server's environment
([configuration](configuration.md)).

## How each kind of control is driven

- **Win32 controls** get posted window messages: `BM_CLICK` /
  `WM_LBUTTONDOWN`/`UP` for a click, `WM_SETTEXT` for typing, `WM_MOUSEWHEEL`
  for scrolling. None of these activates the window.
- **Windowless controls** (UWP, WinUI, Chromium, Electron) get UI Automation
  patterns: `Invoke` for a click, `ValuePattern` for typing.
- **Edit combos** are sent as the messages apps implement: Ctrl+C, Ctrl+X,
  Ctrl+V, Ctrl+Z and Ctrl+A become `WM_COPY`, `WM_CUT`, `WM_PASTE`, `WM_UNDO`
  and `EM_SETSEL`. Other combos are refused rather than posted, because posting
  cannot set the modifier state apps read.
- **Your own browser**, when its process was started with
  `--remote-debugging-port` (Chrome, Comet, Edge, Brave): the anchored verbs
  route through the DevTools protocol, with DOM names and `aria-label`s,
  selectors that survive re-renders, trusted input events and focus emulation.
  The response carries `route: {browser, port, tab}`.
- **Linux**: AT-SPI2 actions ask the application to perform the operation
  through its own toolkit, so there is no pointer to move and no window to
  raise. Synthetic input (XTEST, portal, uinput) is only the fallback.

## Hidden desktops

Anything Ghost starts (`ghost_window op=launch`, `ghost_run` launch steps,
`ghost_browser_launch mode=windowed`) is born on a hidden Windows desktop with
its own input queue, so it cannot take your foreground at all. The ordinary
verbs reach it with `window=<title>`; `target.surface: "hidden"` is the only
difference in the response.

Measured on the CI box: Edge and Chrome activate their first window on launch in
every launch style tried (normal, hidden, minimised, from a background parent,
placed off-screen). A window created on your desktop takes your keyboard the
moment it exists, which is why Ghost never creates one there. An independent
observer sampling the foreground every 100 ms across a full launch, drive and
close run reported zero changes.

Limits, stated rather than hidden:

- Real `SendInput` does not work on a hidden desktop (Windows refuses it off the
  input desktop). An app that answers only real hardware input needs the
  `foreground` policy on your own desktop.
- Chromium and Electron windows there are driven by posted messages; pixel
  verification is skipped for them because a software render costs seconds.
  Confirm with `ghost_tab_eval` or `ghost_see`.
- Single-instance apps (Windows 11 Notepad, Explorer, a browser on its default
  profile) hand a launch to their running process on your desktop. Ghost reports
  `surface: "user"` with a warning instead of claiming it is hidden.

## When an app takes the foreground itself

An application can activate its own window, and Chromium does exactly that when
an agent types into a page or clicks a button through the accessibility API.
Nothing outside the browser can prevent that call, so Ghost undoes it: an
event hook notices the foreground change in about a millisecond and hands it
back, unless you chose that window yourself (by clicking or alt-tabbing, which
it tells apart from typing because it watches for real input).
`ghost_session_state` reports how often that happened as
`foreground_handed_back`.

Measured with an independent observer while a person typed into another window
for the whole run (`scripts/background-desktop-probe.mjs`,
`scripts/interference-watch.ps1`):

| | before 0.23 | now |
| --- | --- | --- |
| keystrokes delivered to the wrong window | 48 | 1 in six runs of about 493 |
| how soon a stolen foreground is noticed | never | about 1 ms |

Six runs gave 0, 0, 0, 1, 0 and 0 stray keystrokes. Undoing an activation can
never be a guarantee; for zero activation, let Ghost start the app on a hidden
desktop.

## Proving it on your machine

- Every action response carries `verified`, `focus_preserved` and
  `cursor_preserved`.
- `ghost_stats.interference_audit` records every foreground change with no real
  hardware input behind it, with the tool calls in flight.
- `ghost verify` audits these claims against your hardware and exits non-zero if
  one does not hold.
