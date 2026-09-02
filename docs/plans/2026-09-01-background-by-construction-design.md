# Ghost 0.20 - Background by construction

Date: 2026-09-01. Status: approved for implementation (autonomous session; decisions
recorded here with the evidence that drove them).

## The problem, from evidence

Three weeks of Claude Code transcripts (1,409 session files, 10,323 Ghost calls) and
four live experiments on this machine:

1. **Agents drive windows that are already on the user's desktop.** The top anchored
   targets were Comet browser windows (over 1,000 calls), Discord, VS Code and file
   dialogs. Headless browsers were launched about 20 times. Isolated desktops were
   used 4 times. Any design that only makes *Ghost-launched* surfaces invisible
   misses where the calls go.
2. **The single largest failure class is target ambiguity.** About 140 failures were
   "element not found ... in the foreground window": with no `window=` the verbs act
   on whatever window the human currently has focused. Agents then reached for
   `ghost_window op=focus` (91 calls) and `ghost_set_focus_policy foreground`
   (13 calls), which is exactly the screen-stealing the policy exists to prevent.
3. **Every browser launch on the user's desktop steals the foreground.** Measured:
   Edge and Chrome activate their first window on launch in normal, hidden and
   minimized launch styles, from a background parent, even with the window placed
   at -32000,-32000. `ghost_browser_launch mode=windowed` therefore moved keyboard
   focus into an invisible window; while it was up, the human's keystrokes went
   there. Headless mode does not have a window and did not steal focus.
4. **Chromium pages can be driven without focus.** On a Chromium window UIA Invoke
   clicked a page button, UIA ValuePattern set a page `<input>` (with input events
   firing), a posted coordinate click landed, and posted `WM_CHAR`/`VK_RETURN`
   reached the page. (The keystroke paths were measured while the window happened
   to be foreground; the UIA paths are focus-independent by construction.)
5. **The tool descriptions describe pre-0.19 behaviour.** `ghost_act` still says it
   "anchors OS foreground to the target's window"; `ghost_key` says the target "is
   focused+confirmed first". Agents read these and conclude they must steal focus.
6. **Boot is not the bottleneck.** Cold boot to first MCP response: 19-86 ms;
   tools/list 27.7 KB. `ghost_shell op=run shell=powershell` (60% of all calls,
   6,161 in three weeks) costs 232-447 ms each, all of it PowerShell process start.
   UIA walks have no time deadline (a known 1.8% stall rate under load).

## Goals and acceptance criteria

- **A1 - Launches never surface.** Under the default `background` policy, nothing
  Ghost starts (`ghost_window op=launch`, `ghost_run` launch steps, windowed
  browsers) ever changes the user's foreground window. Proof: an independent
  observer sampling `GetForegroundWindow` during a launch-and-drive script sees no
  change not attributable to real hardware input.
- **A2 - One vocabulary.** `ghost_see/find/act/key/click_at/scroll/wait/assert/
  screenshot` accept `window=<title>` for windows on Ghost's hidden desktops
  exactly as for the user's desktop. Proof: launch Notepad under the default policy,
  then see, type, press Enter and read the text back by title only.
- **A3 - Never the human's window by accident.** Once a target has been anchored
  (explicit `window=`, a launch, `op=focus`), unanchored verbs use the anchor.
  Every response carries `target: {hwnd, title, surface, source}`. Proof: unit
  tests on the resolver plus an MCP-level check with a different foreground window.
- **A4 - `op=focus` is safe.** Under `background` it anchors instead of raising and
  says so; it does not error.
- **A5 - Honest descriptions.** Descriptions state the enforced background
  behaviour; every `NoBackgroundPath` error names the concrete alternative.
- **A6 - UIA calls have a hard deadline.** `IUIAutomation2::SetConnectionTimeout`
  and `SetTransactionTimeout` are set on every automation object Ghost creates.
- **A7 - Warm shell.** `ghost_shell op=run shell=powershell` served from a
  pre-spawned spare process; target under 40 ms warm (from 232 ms). Measured.
- **A8 - A dead tool is an error, not a hang.** A panicking or over-deadline tool
  call returns `isError` instead of leaving the client waiting 1,800 s; panics are
  appended to a crash log so the next unexplained death is diagnosable.
- **A9 - Gate.** Workspace tests, `cargo build --all-targets`, `ghost verify`, the
  live suite, install to `~/.local/bin`, README/docs updated, commit and push.

## Approaches considered

- **A. Keep refusing.** Leave the policy as a gate and improve the error text.
  Rejected: a refusal is not a capability; the transcripts show agents route
  around refusals by stealing focus.
- **B. Per-substrate tool families.** Keep `ghost_desktop_*` and `ghost_tab_*` as
  separate vocabularies and teach agents to pick. Rejected on evidence: 4 uses in
  three weeks; agents anchor by window title and expect the ordinary verbs to work.
- **C. Background by construction (chosen).** Every Ghost-initiated launch lands
  on a hidden desktop; the ordinary window-anchored verbs resolve titles across
  the user's desktop and the hidden ones and dispatch to the owning surface; the
  session remembers its anchor so nothing falls back to the human's window
  silently. Windows already on the user's desktop keep the proven UIA-pattern and
  posted-message paths.

## Architecture

### Target resolution (`ghost-session/src/target.rs`)

```
Surface      = User | Hidden { desktop }
TargetSource = Explicit | Anchor | Foreground
Target       = { hwnd, title, pid, surface, source }
```

`GhostSession::resolve_target(window: Option<&str>)`:

1. `Some(title)`: `"foreground"` means the user's foreground window. Otherwise a
   case-insensitive substring match over the user desktop (non-minimised first)
   and then every registered hidden desktop, polled for up to 2 s to absorb launch
   races. A hit becomes the session anchor and returns `Explicit`.
2. `None`: a live anchor returns `Anchor`; otherwise the user's foreground window
   returns `Foreground`. Under the `background` policy the response notes that no
   anchor was set.

`ghost_window op=anchor name=<title>` and `op=anchor clear=true` manage it
explicitly. `op=list` lists hidden-desktop windows too, tagged with `surface`.

### Dispatch by surface (`ghost-session/src/hidden.rs`)

For `Hidden`, each verb runs on that desktop's bound worker via
`DesktopSession::exec` / `with_uia`, using the primitives that already exist
(`describe_screen`, `find_by_name_in_hwnd`, `BackgroundClicker`, `patterns`,
`type_text`, `press`, `shortcut`, `scroll`, `capture`). The JSON shapes match the
user-desktop background verbs so the agent sees no difference. For `User`, the
existing background machinery is unchanged.

### Launch routing

Under `background`: `ghost_launch` (and so `ghost_window op=launch` and the
`launch` step of `ghost_run`) starts the process on an auto-created hidden desktop
named `auto`, waits up to 5 s for its first window, anchors it, and returns
`{pid, surface: "hidden", desktop: "auto", window}`. Under `prefer_background` or
`foreground` the user desktop is used as before.

`ghost_browser_launch mode=windowed` launches the browser process on the same
hidden desktop (`STARTUPINFO.lpDesktop`); CDP is unaffected because it is a local
TCP port. The off-screen window-position hack is dropped for that path.

### Reliability and speed

- `UiaTree::new()` and the STA pool create `IUIAutomation2` and set
  `ConnectionTimeout` / `TransactionTimeout` (env-overridable), then cast to
  `IUIAutomation`. One code path, so hidden-desktop trees inherit it.
- Each request task is wrapped: a panic becomes an error response, and an outer
  deadline (`GHOST_TOOL_DEADLINE_MS`, default 180 s, lifted for tools that carry a
  larger explicit timeout) returns an error while the work finishes in the
  background. A panic hook appends to `%LOCALAPPDATA%\ghost\crash.log`.
- `ghost_shell op=run shell=powershell` takes a pre-spawned spare PowerShell
  running the existing sentinel driver, sends the one command, closes stdin, and
  respawns the spare in the background. Lazy: the spare is created after the first
  run; `GHOST_SHELL_WARM=off` disables it.

## Out of scope (named, not forgotten)

- Attaching to the user's own Comet/Chrome via CDP automatically. The recipe
  (`--remote-debugging-port` + `ghost_browser_attach`) is documented; auto-routing
  window-anchored verbs to CDP is the next step after this one.
- Cloning a logged-in profile into a headless browser (security-sensitive).
- macOS/Linux parity for hidden desktops (Windows desktop objects have no
  analogue; Linux keeps its AT-SPI path).

## Implementation units (each verified before the next)

1. Resilience: request wrapper, crash log, UIA deadlines.
2. Target resolver, anchor, envelope `target`, `op=focus`/`op=anchor`, descriptions.
3. Hidden-desktop dispatch for the anchored verbs; launch routing.
4. Windowed browsers on the hidden desktop.
5. Warm PowerShell spare.
6. Docs, version 0.20.0, full gate, install, commit, push.
