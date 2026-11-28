# Tool reference

Every tool `ghost-mcp` advertises on Windows (54 tools), generated from the server's own `tools/list` answer. A test in `crates/ghost-mcp/src/tool_reference_tests.rs` fails when this page and the server disagree. Regenerate it on Windows with `GHOST_WRITE_TOOL_REFERENCE=1 cargo test -p ghost-mcp tool_reference`.

Every action returns `verified`; a call that cannot do what was asked returns an error naming why. Older tool names stay callable for existing clients but are not listed here.

## Desktop verbs

Windows and Linux. Look, act, wait, assert, manage windows, run commands.

- [`ghost_see`](#ghost_see)
- [`ghost_find`](#ghost_find)
- [`ghost_act`](#ghost_act)
- [`ghost_key`](#ghost_key)
- [`ghost_snapshot`](#ghost_snapshot)
- [`ghost_scroll`](#ghost_scroll)
- [`ghost_drag`](#ghost_drag)
- [`ghost_wait`](#ghost_wait)
- [`ghost_query`](#ghost_query)
- [`ghost_assert`](#ghost_assert)
- [`ghost_run`](#ghost_run)
- [`ghost_screenshot`](#ghost_screenshot)
- [`ghost_window`](#ghost_window)
- [`ghost_shell`](#ghost_shell)
- [`ghost_clipboard`](#ghost_clipboard)
- [`ghost_stats`](#ghost_stats)
- [`ghost_reset`](#ghost_reset)
- [`ghost_stop`](#ghost_stop)
- [`ghost_http_get`](#ghost_http_get)
- [`ghost_http_post`](#ghost_http_post)
- [`ghost_session_state`](#ghost_session_state)

### `ghost_see`

Describe a window's UI as elements {name, role, rect}. Target = window= (title substring; becomes the session anchor) or the current anchor; with neither, the human's foreground window and the response says so (target.source). Never focuses or raises anything. mode=full (default whenever a window is targeted: scoped walk, works on covered windows and hidden-desktop apps), mode=fast (foreground-only quick path, used only when nothing is targeted), mode=text (READ the visible text - the cheapest way to read content), mode=delta (changed elements since since_seq), mode=marks (DEBUG Set-of-Marks image of the target window), mode=selection (selected text of a name/role element in the target window). Any other mode is an invalid-params error. Elements capped at 150 (limit); text at 20000 chars. Use it to read what a window contains; use ghost_snapshot for stable element ids to plan several actions, ghost_find to locate one element, and ghost_screenshot only when pixels matter.

**See a window's controls or text**: read-only, idempotent.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `limit` | integer |  | Max elements (default 150) or chars for mode=text (default 20000); 0 = unlimited elements |
| `mode` | string: `fast`, `full`, `delta`, `text`, `marks`, `selection` |  | full=scoped tree (default with a target), fast=foreground quick path, delta=changed only, text=readable text, marks=SoM debug image, selection=selected text of name/role element |
| `name` | string |  | Element name (mode=selection) |
| `role` | string |  | Element role (mode=selection) |
| `since_seq` | integer |  | Prior snapshot seq for delta mode |
| `window` | string |  | Title substring of the window to describe. Anchors it for later calls. 'foreground' = the human's current window. |

### `ghost_find`

Locate an element (name|role|description|text) and return center (always), rect (has_rect=true for UIA/cache hits), source, confidence, name, escalated (true = a network VLM call was paid). Target window = window= (anchored) or the session anchor; else the foreground. Under the default background policy nothing is focused or raised: the lookup is scoped to the window's own UIA subtree, so it works while the window is covered or lives on a hidden desktop. Use it to see where an element is or how many match before acting; ghost_act already finds and acts in one call.

**Find an element**: read-only, idempotent.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `description` | string |  | Natural-language description for VLM grounding |
| `index` | integer |  | Select the nth match (0-based) when several elements share the name/role; name+role AND-combine on this path; returns matches count |
| `mode` | string: `instant`, `deliberate`, `instant_only` |  | 'instant' (default): local tiers, auto-escalates to VLM on miss. 'deliberate': VLM from first attempt. 'instant_only': no VLM. |
| `name` | string |  | Accessible name (case-insensitive substring) |
| `role` | string |  | Control type: button, edit, checkbox, list, menu, tab, toolbar |
| `text` | string |  | On-screen text for OCR-based location |
| `window` | string |  | Title substring of the window to search (anchors it). Omit to use the anchor. |

### `ghost_act`

Find an element and act on it in one call. Target window = window= (anchored) or the session anchor. Under the default background policy the window is NEVER raised and the cursor never moves: click = UIA Invoke or a posted click, type = ValuePattern / WM_SETTEXT with read-back, and the response reports {verified, focus_preserved, cursor_preserved}. Works on covered windows, Chromium/Electron pages and hidden-desktop apps. If a windowless control's own provider activates its window anyway (Chromium does on SetValue), the foreground is handed straight back to the window the human had and the response carries focus_guard. Identify the element by name|role (description needs vision and a screen-facing window). verified=false = dispatched but nothing visibly changed: ghost_see before retrying. background=true is accepted for compatibility; it is already the default behaviour.

**Click or type on an element**: may change or remove data.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `action` | string: `click`, `type`, `double_click`, `right_click`, `hover` | yes | Action to perform |
| `background` | boolean |  | Compatibility flag: background dispatch is already the default. Real Win32 controls get posted messages; windowless (WinUI/Chromium/Electron) controls get UIA Invoke/ValuePattern; double_click/right_click/hover need a windowed control. focus_preserved in the response reports the truth. |
| `description` | string |  | Natural-language description of the element; needs vision and a window facing the screen, so prefer name/role |
| `index` | integer |  | Act on the nth match (0-based) when several elements share the name/role. Honoured on every route (background, hidden desktop, CDP, foreground); use ghost_find with the same name/index first to see the matches count |
| `mode` | string: `instant`, `deliberate`, `instant_only` |  | Dispatch mode: instant (default, local tiers, escalates if needed), deliberate, or instant_only (never escalates) |
| `name` | string |  | Accessible name of the target element (substring match) |
| `role` | string |  | UI Automation role of the target element, e.g. button, edit |
| `text` | string |  | Match the element by its visible text instead of name/role (the text to type is text_input) |
| `text_input` | string |  | Text to type when action=type (use this to avoid param collision with text-target) |
| `window` | string |  | Title substring of the window to act in (anchors it). Omit to use the anchor. |

### `ghost_key`

Send a key to a window WITHOUT focusing it. Target = window= (anchored) or the session anchor. Under the default background policy the key is POSTED to the window's focused control: single keys (Enter, Tab, F5, arrows, any character) and the editing shortcuts Ctrl+C/X/V/A/Z (sent as semantic messages). Other modifier combos have no background path and error naming the alternative. For text prefer ghost_act action=type (read-back verified). Hold/release keys='down:Shift' / 'up:Shift' exist only under a foreground policy.

**Press keys**: may change or remove data.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `background` | boolean |  | Compatibility flag: background posting is already the default. |
| `keys` | string | yes | Key spec: 'Enter', 'a', 'Ctrl+C', 'down:Shift', 'up:Shift' |
| `window` | string |  | Title substring of the window to send to (anchors it). Omit to use the anchor. With no anchor at all the key goes to the human's foreground window and the response says so. |

### `ghost_snapshot`

Structured, agent-planning view of a window's UI: every element with a stable id, name, role, rect, center, enabled flag, actionable flag, and the actions it accepts (click/type). `actionable` = interactable role AND currently enabled - so a greyed-out button reads actionable:false. Far cheaper than a screenshot to reason over; plan here, then ghost_act by name/role. Params: window? (title substring; omitted = foreground), actionable_only? (only enabled interactable elements), limit?. Use it instead of ghost_see when planning several actions against stable ids; for a quick read of a window, ghost_see is enough.

**Snapshot a window with element ids**: read-only, idempotent.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `actionable_only` | boolean |  | Return only interactable elements (buttons/edits/links/...) - cuts noise for planning |
| `limit` | integer |  | Max elements (default 150; 0 = unlimited) |
| `window` | string |  | Window title substring; omitted = foreground window (faster) |

### `ghost_scroll`

Scroll. direction: up/down/left/right, amount = notches (default 3). Target = window= (anchored) or the session anchor: under the default background policy the wheel is POSTED to that window (pointer never moves, window never raised; works on covered and hidden-desktop windows). 'Until' mode: until_name/until_role scrolls repeatedly until that element is present, up to max_scrolls; returns found. Coord mode (x/y at the real pointer) needs a foreground policy.

**Scroll**: changes state, not destructive.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `amount` | integer |  | Wheel notches to scroll (default 3) |
| `direction` | string: `up`, `down`, `left`, `right` | yes | Scroll direction |
| `max_scrolls` | integer |  | Max scroll steps in 'until' mode |
| `until_name` | string |  | Scroll until an element with this name is visible |
| `until_role` | string |  | Scroll until an element with this role is visible |
| `window` | string |  | Title substring of the window to scroll (anchors it). Omit to use the anchor. |
| `x` | integer |  | Screen x of the real pointer for coordinate mode (needs a foreground policy) |
| `y` | integer |  | Screen y of the real pointer for coordinate mode (needs a foreground policy) |

### `ghost_drag`

Click-hold at a start point, move to an end point, release. Each endpoint may be raw coords (from_x/from_y, to_x/to_y) OR an element (from_name/from_role, to_name/to_role) resolved to its center - e.g. drag a list row onto another, or a slider handle.

**Drag**: may change or remove data.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from_name` | string |  | Start element name (instead of from_x/y) |
| `from_role` | string |  | Start element role |
| `from_x` | integer |  | Start x in screen coordinates (or use from_name/from_role) |
| `from_y` | integer |  | Start y in screen coordinates |
| `to_name` | string |  | End element name (instead of to_x/y) |
| `to_role` | string |  | End element role |
| `to_x` | integer |  | End x in screen coordinates (or use to_name/to_role) |
| `to_y` | integer |  | End y in screen coordinates |

### `ghost_wait`

Unified wait - use it instead of sleeping. for=element: an element (name/role) appears or disappears in the target window; returns the moment it does. for=value: an element's VALUE equals/contains/changes (forms, async fields, 'wait until the total updates'). for=navigate: load url in a browser window WITHOUT raising it - sets the address bar over UIA, presses Enter, returns when the window title changes (response: title_before, title_after, title_changed, ms); only a foreground focus policy takes the old focus-and-type path. for=idle: screen stable. for=text: click a target then wait for text. for=event: next foreground change. for=cond: JSONLogic poll. for=ms: a plain sleep - last resort, it costs its full length every run. Target window for element|value|idle|text|navigate = window= or the session anchor.

**Wait for a condition**: may change or remove data.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `appears` | boolean |  | for=element\|text: true=wait to appear, false=wait to disappear |
| `condition` | object |  | JSONLogic expression (for=cond) |
| `for` | string: `ms`, `idle`, `element`, `value`, `text`, `event`, `cond`, `navigate` |  | What to wait for (default ms) |
| `ms` | integer |  | Milliseconds (for=ms) |
| `name` | string |  | Element name (for=element\|value) |
| `predicate` | string: `equals`, `contains`, `changes` |  | for=value: how to compare the element's value to text (default equals) |
| `role` | string |  | Element role (for=element\|value) |
| `settle_ms` | integer |  | for=navigate: pause after the title change so the page can paint (0 = return immediately) |
| `since_seq` | integer |  | for=event |
| `stable_frames` | integer |  | for=idle |
| `text` | string |  | Text to wait for (for=text) or expected value (for=value) |
| `timeout_ms` | integer, 1 to 600000 |  | Give up after this long: an integer from 1 to 600000 (default 5000); anything else is invalid params. Running out answers ok:false with timed_out:true for every for= |
| `url` | string |  | URL to navigate to (for=navigate) |
| `window` | string |  | Window scope (for=idle\|element\|value\|navigate); omit to use the anchor |

### `ghost_query`

Extract structured data from the screen. Strategy: UIA name-matching first, then a single batched VLM call for any fields still unmatched. Returns extracted object, unmatched list, and vlm_attempted flag. Use it to pull a few named fields' values into one object; use ghost_see to read a whole window.

**Query a window's elements**: read-only, idempotent.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `region` | array |  | Optional [left,top,right,bottom] region for VLM screenshot crop |
| `schema` | object |  | JSON Schema (properties map) or array of field names to extract |
| `window` | string |  | Scope to window (partial title) |

### `ghost_assert`

Assert a predicate about screen state. Fails (error) if not satisfied. text-present/text-absent: OCR text check. element-exists: element found. value-equals/value-contains: the element's actual value (ValuePattern) equals/contains 'text' - the fill-then-verify check. Use it to turn 'did it work' into a pass or fail after acting; use ghost_wait when the condition may take time to appear.

**Assert a condition**: read-only, idempotent.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `foreground` | boolean |  | Only without a window/anchor: OCR the foreground window (true) or the whole screen |
| `name` | string |  | Element name (element-exists\|value-*) |
| `predicate` | string: `text-present`, `text-absent`, `element-exists`, `value-equals`, `value-contains` | yes | What to check: text-present/text-absent (OCR), element-exists, value-equals/value-contains (element value against text) |
| `role` | string |  | Element role (element-exists\|value-*) |
| `text` | string |  | Text to check (text-present/absent) or expected value (value-equals/contains) |
| `window` | string |  | Title substring of the window to check (anchors it). Omit to use the anchor. text-present/absent then OCR THAT window by handle, never the foreground. |

### `ghost_run`

Execute a declarative step-by-step flow in one round-trip. Each step: {op, ...params}. Op is any lean verb, legacy tool name, or short name (find, click, type, press, click_at, wait...). RETRIES: a failing read-only step is retried up to max_retries; an acting step is NOT retried (its effect may have landed) unless the step sets retry:true; retry:false disables retries for a step; unknown op / missing param errors are never retried. CHAINING: a param value of "${steps.N.path}" is replaced with a field from step N's result before dispatch - e.g. {op:'find',name:'Save'} then {op:'ghost_click_at', x:'${steps.0.center.x}', y:'${steps.0.center.y}'}. A whole-string ref keeps its type (number stays number). SPEED: name the window once; later steps can omit window= because the anchor follows that window by handle through title changes (a stale title still resolves and the response carries title_drift). Wait with {op:'ghost_wait', for:'element'|'value'|'navigate'} rather than for=ms - a fixed sleep costs its full length on every run.

**Run a multi-step flow**: may change or remove data, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `json_flow` | string |  | JSON-encoded steps array string |
| `max_retries` | integer |  | Retries for a failing read-only step (default 2, max 10); acting steps retry only with retry:true |
| `script` | string |  | YAML or JSON string of steps array (YAML tried first) |
| `steps` | array |  | Array of {op, ...params} steps (direct) |
| `stop_on_error` | boolean |  | Stop at the first failed step (default true); false runs the remaining steps and reports each result |

### `ghost_screenshot`

Capture a screenshot of the window the agent is working in. Target = window= or the session anchor: that window is captured BY HANDLE (works while covered, on a hidden desktop, or via CDP for a routed browser) at max 768px JPEG q=75 (~20-100KB). Only with no anchor does it fall back to the foreground window. Pass name/role to crop to ONE element inside the target, or rect=[l,t,r,b] (great for VLM-in-the-loop checks). full=true: the whole screen at max 1280px JPEG (max_dim=0 = native-res lossless PNG). Always includes size_bytes. Use it when pixels matter (charts, canvas, images); for controls and text, ghost_see is cheaper.

**Take a screenshot**: read-only, idempotent.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `foreground` | boolean |  | Only without a window/anchor: crop to the foreground window (default true) |
| `full` | boolean |  | Full-screen capture (default false) |
| `jpeg_quality` | integer, 0 to 100 |  | JPEG quality 0-100 (default 75) |
| `max_dim` | integer |  | Longest-edge resize (default 768; 1280 with full=true; 0 = no resize, and with full=true a lossless PNG) |
| `name` | string |  | Crop to the element with this accessible name |
| `rect` | array |  | Crop to [left,top,right,bottom] region |
| `role` | string |  | Crop to the element with this role |
| `window` | string |  | Title substring of the window to capture (anchors it). Omit to use the anchor. |

### `ghost_window`

Window management across the user's desktop AND Ghost's hidden desktops. op=list: every window (name, pid, hwnd, focused, state, surface=user|hidden) plus the current anchor. op=focus: under the default background policy this ANCHORS the window and does NOT raise it (the anchored verbs drive it without focus); only an operator-unlocked prefer_background/foreground policy raises it. op=anchor: name= sets the anchor, clear=true clears it, no args reports it. op=state: maximize|minimize|restore|close (name+state); under the background policy these never activate the window, so the user's focus stays put (restore is how a minimized window becomes actionable). op=launch: start exe - under the background policy the app starts on a hidden desktop, never on your screen, and is anchored (see target.surface in the response).

**List, launch and manage windows**: may change or remove data.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `clear` | boolean |  | op=anchor: forget the current anchor |
| `exe` | string |  | Executable path or command line (op=launch) |
| `include_hidden` | boolean |  | op=list: also list windows that have vanished (not visible, not minimised - state=hidden); op=state state=restore on one of them shows it again without activating it |
| `name` | string |  | Window title substring (op=focus\|anchor\|state). Also accepted as alias for 'exe' on op=launch. |
| `op` | string: `list`, `focus`, `anchor`, `state`, `launch` |  | Operation (default list) |
| `state` | string: `maximize`, `minimize`, `restore`, `close` |  | op=state |

### `ghost_shell`

Run terminal / PowerShell commands and drive persistent shells. THE way to run builds, git, CLIs, scripts, or open apps from a command line, and to edit files on machines with no file tools. op=run (default): one-shot - spawn shell, run 'cmd', return {output, exit_code, timed_out}. shell=powershell (default) | pwsh | cmd. op=open: start a persistent PowerShell whose variables/cwd/env survive across commands (returns an 'id'). op=send: run 'cmd' in session 'id' (state persists). op=read: drain the rest of a command that hit its timeout (busy=true). op=list: show sessions. op=kill: end a session and every program its commands started. A program a finished op=run started keeps running (until a stop). Output is merged stdout+stderr, tail-capped at 24000 chars. Emergency-stop (ghost_stop / Ctrl+Alt+G) kills a runaway command. To START A NEW CLAUDE CODE SESSION: op=run cmd='Start-Process wt -ArgumentList "pwsh","-NoExit","-Command","claude"', then drive the terminal window with ghost_see / ghost_act / ghost_key. Disabled entirely when GHOST_SHELL=off.

**Run shell commands**: may change or remove data, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `cmd` | string |  | Command text (op=run\|send) |
| `cwd` | string |  | Working directory (op=run\|open) |
| `id` | string |  | Session id (op=send\|read\|kill required; op=open optional custom name, else auto s1,s2,...) |
| `op` | string: `run`, `open`, `send`, `read`, `list`, `kill` |  | Operation (default run) |
| `shell` | string: `powershell`, `pwsh`, `cmd` |  | op=run shell (default powershell). Persistent sessions are PowerShell only. |
| `timeout_ms` | integer |  | Per-command timeout, default 30000, max 600000. On timeout op=run ends the shell and every program its command started; op=send leaves it running (drain via op=read). op=read: how long to wait for the pending command, default 2000; output already written is always collected, even with 0. |

### `ghost_clipboard`

Clipboard access. op=get (default): read text. op=set: write text.

**Read or set the clipboard**: may change or remove data, idempotent.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `op` | string: `get`, `set` |  | get=read, set=write (default get) |
| `text` | string |  | Text to write (op=set) |

### `ghost_stats`

Grounding + cache telemetry for debugging flow speed/reliability: which tier is winning (cache/UIA/OCR/VLM), VLM escalation rate, cache hit/miss counts, UIA-mirror stats. Call when a flow is slow or a find is unreliable.

**Server statistics**: read-only, idempotent.

No parameters.

### `ghost_reset`

Lift a stop raised with ghost_stop, for every Ghost process. A stop the human raised with Ctrl+Alt+G is refused: only the human can lift it, so wait for them.

**Lift a stop an agent raised**: changes state, not destructive, idempotent.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `scope` | string: `everywhere`, `process` |  | everywhere (default): clears the stop for every Ghost process. process: only this server's own flag; a stop raised elsewhere stays. |

### `ghost_stop`

Emergency stop: halt all automation and release held modifier keys.

**Emergency stop**: may change or remove data, idempotent.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `scope` | string: `everywhere`, `process` |  | everywhere (default): every Ghost process in this logon session. process: only this server (used by ghost verify). |

### `ghost_http_get`

HTTP GET. Returns {status, body}.

**HTTP GET**: read-only, idempotent, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `headers` | object |  | Extra request headers as an object of name: value strings (Host cannot be set) |
| `url` | string | yes | http(s) URL to fetch; loopback, private, link-local and reserved addresses are refused |

### `ghost_http_post`

HTTP POST. Returns {status, body}.

**HTTP POST**: may change or remove data, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `body` | string |  | Request body, sent as given |
| `content_type` | string |  | Content-Type of the body (default application/json) |
| `headers` | object |  | Extra request headers as an object of name: value strings (Host cannot be set) |
| `url` | string | yes | http(s) URL to post to; loopback, private, link-local and reserved addresses are refused |

### `ghost_session_state`

The window the user is working in (handle + title), the real cursor position, the focus policy and lock, and the interference audit (foreground changes with no human input while a tool was in flight). Call before/after a batch of actions to verify nothing was disturbed; compare foreground_hwnd, since titles change on their own.

**Session state and focus audit**: read-only, idempotent.

No parameters.

## Browser and tabs

Windows and Linux. Drive individual tabs of a Chromium-family browser over its DevTools port, in the background.

- [`ghost_browser_launch`](#ghost_browser_launch)
- [`ghost_browser_attach`](#ghost_browser_attach)
- [`ghost_browser_close`](#ghost_browser_close)
- [`ghost_browser_tabs`](#ghost_browser_tabs)
- [`ghost_browser_list_installed`](#ghost_browser_list_installed)
- [`ghost_tab_open`](#ghost_tab_open)
- [`ghost_tab_navigate`](#ghost_tab_navigate)
- [`ghost_tab_click`](#ghost_tab_click)
- [`ghost_tab_type`](#ghost_tab_type)
- [`ghost_tab_press`](#ghost_tab_press)
- [`ghost_tab_text`](#ghost_tab_text)
- [`ghost_tab_eval`](#ghost_tab_eval)
- [`ghost_tab_screenshot`](#ghost_tab_screenshot)
- [`ghost_tab_describe`](#ghost_tab_describe)
- [`ghost_tab_scroll`](#ghost_tab_scroll)
- [`ghost_tab_select_option`](#ghost_tab_select_option)
- [`ghost_tab_wait_for`](#ghost_tab_wait_for)
- [`ghost_tab_close`](#ghost_tab_close)
- [`ghost_tab_find`](#ghost_tab_find)

### `ghost_browser_launch`

Launch an isolated browser for background automation. Each id gets its own process, profile, and DevTools port, so concurrent ghost processes never collide. browser= chrome | comet | edge | brave (default: first installed). mode=headless (invisible, default) | windowed (a real browser window, started on a hidden desktop - never on your screen, never takes focus; use it only for sites that behave differently headless). Never launched on your own desktop: a new Chromium window takes the foreground on creation. Tabs are driven individually via ghost_tab_* without focusing the window or moving the cursor.

**Launch a browser**: changes state, not destructive, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string: `chrome`, `comet`, `edge`, `brave` |  | Which installed browser to launch (default: the first one installed) |
| `id` | string |  | Handle for later calls (default: 'default') |
| `mode` | string: `headless`, `windowed` |  | headless (invisible, default) or windowed (a real window on a hidden desktop, never on your screen) |

### `ghost_browser_attach`

Attach to a browser already running with --remote-debugging-port=<port> (use for the user's own logged-in browser). Ghost never closes a browser it did not start, and drives background tabs without switching the user's active tab.

**Attach to a running browser**: changes state, not destructive, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | string |  | Handle for later calls (default 'default') |
| `port` | integer | yes | The --remote-debugging-port the running browser was started with |

### `ghost_browser_close`

Close a browser ghost launched (attached browsers are only disconnected).

**Close a browser**: may change or remove data, idempotent, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | string |  | Browser id to close (default 'default'); an attached browser is only disconnected |

### `ghost_browser_tabs`

List open tabs with target ids, titles, URLs.

**List a browser's tabs**: read-only, idempotent, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | string |  | Browser id (default 'default') |

### `ghost_browser_list_installed`

List Chromium-family browsers installed on this machine (chrome, comet, edge, brave).

**List installed browsers**: read-only, idempotent.

No parameters.

### `ghost_tab_open`

Open a new background tab (never brings the tab or window to the front). Returns its id for the other ghost_tab_* tools. With an http(s) url it waits for the page to load, like ghost_tab_navigate, so the next read sees the page (loaded:true and the url it landed on; loaded:false with a note for a download or an empty reply). A page that fails to load or exceeds timeout_ms (the whole navigation) is an error, and the tab it opened is closed. wait=false answers at once, before the page loads. url must be http://, https:// or about:blank (default); file:, chrome:, data:, javascript: and the like are refused. http(s) to any host including localhost is allowed.

**Open a tab**: changes state, not destructive, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  | Browser id from ghost_browser_launch or ghost_browser_attach (default 'default') |
| `timeout_ms` | integer, 1 to 600000 |  | Load timeout when waiting, default 30000, max 600000 |
| `url` | string |  | http(s) URL or about:blank (default); other schemes are refused |
| `wait` | boolean |  | Wait for the page to load (default true; about:blank never waits) |

### `ghost_tab_navigate`

Navigate a tab and wait for load. Runs in a background tab; the user's view never changes. url must be http://, https:// or about:blank; every other scheme (file:, chrome:, data:, javascript:, ftp: ...) is refused. http(s) to any host including localhost is allowed. Returns an error (naming the net:: errorText) when the page fails to load, e.g. DNS failure or refused connection.

**Navigate a tab**: may change or remove data, idempotent, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  | Browser id from ghost_browser_launch or ghost_browser_attach (default 'default') |
| `tab` | string | yes | Tab id from ghost_tab_open or ghost_browser_tabs |
| `timeout_ms` | integer, 1 to 600000 |  | Load timeout, default 30000, max 600000 |
| `url` | string | yes | http(s) URL or about:blank; other schemes are refused |

### `ghost_tab_click`

Click an element by CSS selector via a trusted synthetic mouse event inside the tab's renderer. ~1-2ms; no cursor movement; works in a tab that is not in front.

**Click in a tab**: may change or remove data, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  | Browser id from ghost_browser_launch or ghost_browser_attach (default 'default') |
| `selector` | string | yes | CSS selector of the element to click |
| `tab` | string | yes | Tab id from ghost_tab_open or ghost_browser_tabs |
| `timeout_ms` | integer, 1 to 600000 |  | How long to wait for the element, 1 to 600000 (default 10000) |

### `ghost_tab_type`

Focus an element by CSS selector and type into it. Layout-independent; does not touch the user's keyboard.

**Type in a tab**: may change or remove data, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  | Browser id from ghost_browser_launch or ghost_browser_attach (default 'default') |
| `clear` | boolean |  | Clear the field first (default true) |
| `selector` | string | yes | CSS selector of the field to type into |
| `tab` | string | yes | Tab id from ghost_tab_open or ghost_browser_tabs |
| `text` | string | yes | Text to type |

### `ghost_tab_press`

Send a key (Enter, Tab, ArrowDown, a, ...) to the tab's focused element. modifiers: Alt/Ctrl/Meta/Shift.

**Press a key in a tab**: may change or remove data, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  | Browser id from ghost_browser_launch or ghost_browser_attach (default 'default') |
| `key` | string | yes | Key to send to the focused element: Enter, Tab, ArrowDown, a, ... |
| `modifiers` | array |  | Modifier keys held during the press: Alt, Ctrl, Meta, Shift |
| `tab` | string | yes | Tab id from ghost_tab_open or ghost_browser_tabs |

### `ghost_tab_text`

Visible text of an element, or the whole page when selector is omitted. Prefer over screenshots for reading content.

**Read a tab's text**: read-only, idempotent, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  | Browser id from ghost_browser_launch or ghost_browser_attach (default 'default') |
| `max_chars` | integer |  | Cap on returned characters (default 100000, max 2000000). When cut, the result carries truncated:true and original_length. |
| `selector` | string |  | CSS selector of the element to read (default: the whole page) |
| `tab` | string | yes | Tab id from ghost_tab_open or ghost_browser_tabs |

### `ghost_tab_eval`

Evaluate a JavaScript expression in the tab and return its value (awaits promises). The serialized result is capped at max_chars; when cut, value is the cut JSON string with truncated:true and original_length.

**Run JavaScript in a tab**: may change or remove data, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  | Browser id from ghost_browser_launch or ghost_browser_attach (default 'default') |
| `expression` | string | yes | JavaScript expression to evaluate in the page; promises are awaited |
| `max_chars` | integer |  | Cap on the serialized result in characters (default 100000, max 2000000). |
| `tab` | string | yes | Tab id from ghost_tab_open or ghost_browser_tabs |

### `ghost_tab_screenshot`

PNG of a tab, rendered by that tab regardless of whether it is in front or focused.

**Screenshot a tab**: read-only, idempotent, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  | Browser id from ghost_browser_launch or ghost_browser_attach (default 'default') |
| `full_page` | boolean |  | Capture the whole scrollable page, not just the viewport (default false) |
| `tab` | string | yes | Tab id from ghost_tab_open or ghost_browser_tabs |

### `ghost_tab_describe`

Structured list of visible interactive elements with selectors and coordinates. Prefer this over screenshots for deciding what to click.

**Describe a tab's page**: read-only, idempotent, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  | Browser id from ghost_browser_launch or ghost_browser_attach (default 'default') |
| `limit` | integer |  | Most elements to return (default 120) |
| `tab` | string | yes | Tab id from ghost_tab_open or ghost_browser_tabs |

### `ghost_tab_scroll`

Scroll a tab or a scrollable element inside it.

**Scroll a tab**: changes state, not destructive, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  | Browser id from ghost_browser_launch or ghost_browser_attach (default 'default') |
| `dx` | number |  | Horizontal scroll in pixels (default 0) |
| `dy` | number |  | Vertical scroll in pixels, positive is down (default 0) |
| `selector` | string |  | CSS selector of a scrollable element (default: the page) |
| `tab` | string | yes | Tab id from ghost_tab_open or ghost_browser_tabs |

### `ghost_tab_select_option`

Choose an option in a <select>, firing the input/change events a real choice fires.

**Choose a select option in a tab**: may change or remove data, idempotent, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  | Browser id from ghost_browser_launch or ghost_browser_attach (default 'default') |
| `selector` | string | yes | CSS selector of the <select> element |
| `tab` | string | yes | Tab id from ghost_tab_open or ghost_browser_tabs |
| `value` | string | yes | Value of the option to choose |

### `ghost_tab_wait_for`

Wait until a CSS selector matches an element in the tab.

**Wait for a tab condition**: read-only, idempotent, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  | Browser id from ghost_browser_launch or ghost_browser_attach (default 'default') |
| `selector` | string | yes | CSS selector to wait for |
| `tab` | string | yes | Tab id from ghost_tab_open or ghost_browser_tabs |
| `timeout_ms` | integer, 1 to 600000 |  | How long to wait, 1 to 600000 (default 15000) |

### `ghost_tab_close`

Close a tab by id.

**Close a tab**: may change or remove data, idempotent, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  | Browser id from ghost_browser_launch or ghost_browser_attach (default 'default') |
| `tab` | string | yes | Tab id from ghost_tab_open or ghost_browser_tabs |

### `ghost_tab_find`

Find a tab whose URL or title contains a substring.

**Find an element in a tab**: read-only, idempotent, reaches the network or runs programs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  | Browser id from ghost_browser_launch or ghost_browser_attach (default 'default') |
| `query` | string | yes | Substring to look for in each tab's URL or title |

## Hidden desktops and focus policy

Windows only. Explicit control of isolated desktops the user never sees, and the focus policy.

- [`ghost_focus_policy`](#ghost_focus_policy)
- [`ghost_set_focus_policy`](#ghost_set_focus_policy)
- [`ghost_desktop_create`](#ghost_desktop_create)
- [`ghost_desktop_close`](#ghost_desktop_close)
- [`ghost_desktop_launch`](#ghost_desktop_launch)
- [`ghost_desktop_windows`](#ghost_desktop_windows)
- [`ghost_desktop_wait_for_window`](#ghost_desktop_wait_for_window)
- [`ghost_desktop_click`](#ghost_desktop_click)
- [`ghost_desktop_scroll`](#ghost_desktop_scroll)
- [`ghost_desktop_type`](#ghost_desktop_type)
- [`ghost_desktop_press`](#ghost_desktop_press)
- [`ghost_desktop_shortcut`](#ghost_desktop_shortcut)
- [`ghost_desktop_capture`](#ghost_desktop_capture)
- [`ghost_desktop_describe`](#ghost_desktop_describe)

### `ghost_focus_policy`

Report the focus policy (background = never touches the user's mouse, keyboard or foreground; prefer_background; foreground) and whether it is locked. locked:true (the default) means this process cannot be raised above background by any tool call - only the operator can, with GHOST_FOCUS_LOCK=off in the server's environment - so do not plan on real input: launch apps with ghost_window op=launch and browsers with ghost_browser_launch and drive them anchored.

**Read the focus policy**: read-only, idempotent.

No parameters.

### `ghost_set_focus_policy`

Set the focus policy. 'background' is always accepted and is the default. 'prefer_background' and 'foreground' let Ghost drive the user's real cursor and keyboard and are REFUSED while the lock is on (the default; see ghost_focus_policy). Do not retry a refusal: the error names the route that needs no policy (ghost_window op=launch, ghost_browser_launch). When the operator has unlocked the process, raise it only for a target with no background path and set it back afterwards.

**Set the focus policy**: changes state, not destructive, idempotent.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `policy` | string: `background`, `prefer_background`, `foreground` | yes | background (always allowed, the default), prefer_background or foreground (refused while the focus lock is on) |

### `ghost_desktop_create`

Create an isolated Windows desktop - apps launched onto it NEVER appear on the user's screen (the desktop-app equivalent of headless). Usually unnecessary: under the default background policy ghost_window op=launch already starts apps on the hidden desktop 'auto' and the ordinary verbs (ghost_see/find/act/key/scroll with window=<title>) drive them there. UIA, window messages, and capture all work; real SendInput does not (OS boundary).

**Create a hidden desktop**: changes state, not destructive.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | string |  | Id for the new hidden desktop (default 'default') |

### `ghost_desktop_close`

Destroy an isolated desktop, terminating any processes still on it (they have no visible window to close).

**Close a hidden desktop**: may change or remove data, idempotent.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | string |  | Hidden desktop to close (default 'default'); processes still on it are ended |

### `ghost_desktop_launch`

Launch a program onto a specific isolated desktop (ghost_window op=launch does this automatically onto 'auto' under the background policy and anchors the window). Windows cannot move an existing window there - launch is the only entry.

**Launch a program on a hidden desktop**: changes state, not destructive.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `command` | string | yes | Program to start on the hidden desktop, with any arguments |
| `desktop` | string |  | Hidden desktop id from ghost_desktop_create (default 'default') |

### `ghost_desktop_windows`

Visible windows on an isolated desktop, with handles for the other desktop tools.

**List a hidden desktop's windows**: read-only, idempotent.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `desktop` | string |  | Hidden desktop id from ghost_desktop_create (default 'default') |

### `ghost_desktop_wait_for_window`

Wait for a window with a matching title to appear on an isolated desktop.

**Wait for a window on a hidden desktop**: read-only, idempotent.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `desktop` | string |  | Hidden desktop id from ghost_desktop_create (default 'default') |
| `timeout_ms` | integer, 1 to 600000 |  | How long to wait, 1 to 600000 (default 20000) |
| `title` | string | yes | Title substring to wait for |

### `ghost_desktop_click`

Click a client-area point in a window on an isolated desktop. button=left|right|double.

**Click on a hidden desktop**: may change or remove data.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `button` | string: `left`, `right`, `double` |  | left (default), right or double |
| `desktop` | string |  | Hidden desktop id from ghost_desktop_create (default 'default') |
| `hwnd` | integer | yes | Window handle from ghost_desktop_windows or ghost_desktop_wait_for_window |
| `x` | integer | yes | Client-area x of the point to click |
| `y` | integer | yes | Client-area y of the point to click |

### `ghost_desktop_scroll`

Scroll a window on an isolated desktop.

**Scroll on a hidden desktop**: changes state, not destructive.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `amount` | integer |  | Wheel notches to scroll (default 3) |
| `desktop` | string |  | Hidden desktop id from ghost_desktop_create (default 'default') |
| `direction` | string: `up`, `down`, `left`, `right` |  | Scroll direction |
| `hwnd` | integer | yes | Window handle from ghost_desktop_windows or ghost_desktop_wait_for_window |

### `ghost_desktop_type`

Type into a window on an isolated desktop.

**Type on a hidden desktop**: may change or remove data.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `desktop` | string |  | Hidden desktop id from ghost_desktop_create (default 'default') |
| `hwnd` | integer | yes | Window handle from ghost_desktop_windows or ghost_desktop_wait_for_window |
| `text` | string | yes | Text to type into the window |

### `ghost_desktop_press`

Send a key to a window on an isolated desktop.

**Press a key on a hidden desktop**: may change or remove data.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `desktop` | string |  | Hidden desktop id from ghost_desktop_create (default 'default') |
| `hwnd` | integer | yes | Window handle from ghost_desktop_windows or ghost_desktop_wait_for_window |
| `key` | string | yes | Key name to send, e.g. Enter, Tab, F5 |

### `ghost_desktop_shortcut`

Editing shortcut (undo/cut/copy/paste/select_all, or 'Ctrl+Z' style) via the standard control messages - faked modifier keys would type a literal character instead.

**Send a shortcut on a hidden desktop**: may change or remove data.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `desktop` | string |  | Hidden desktop id from ghost_desktop_create (default 'default') |
| `hwnd` | integer | yes | Window handle from ghost_desktop_windows or ghost_desktop_wait_for_window |
| `shortcut` | string | yes | undo, cut, copy, paste, select_all, or a 'Ctrl+Z' style combination |

### `ghost_desktop_capture`

PNG of a window on an isolated desktop - the only way to see what an invisible app is doing.

**Capture a hidden-desktop window**: read-only, idempotent.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `desktop` | string |  | Hidden desktop id from ghost_desktop_create (default 'default') |
| `hwnd` | integer | yes | Window handle from ghost_desktop_windows or ghost_desktop_wait_for_window |

### `ghost_desktop_describe`

Interactive elements of an app on an isolated desktop via UIA. Prefer over capture-and-guess: UI Automation works on a non-displayed desktop.

**Describe a hidden-desktop window**: read-only, idempotent.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `desktop` | string |  | Hidden desktop id from ghost_desktop_create (default 'default') |
| `window` | string |  | Title substring of the window to describe |
