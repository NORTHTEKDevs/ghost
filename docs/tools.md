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

Describe a window's UI as elements {name, role, rect}. Target = window= (title substring; becomes the session anchor) or the current anchor; with neither, the human's foreground window and the response says so (target.source). Never focuses or raises anything. mode=full (default whenever a window is targeted: scoped walk, works on covered windows and hidden-desktop apps), mode=fast (foreground-only quick path, used only when nothing is targeted), mode=text (READ the visible text - the cheapest way to read content), mode=delta (changed elements since since_seq), mode=marks (DEBUG Set-of-Marks image of the target window), mode=selection (selected text of a name/role element in the target window). Any other mode is an invalid-params error. Elements capped at 150 (limit); text at 20000 chars.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `limit` | integer |  | Max elements (default 150) or chars for mode=text (default 20000); 0 = unlimited elements |
| `mode` | string: `fast`, `full`, `delta`, `text`, `marks`, `selection` |  | full=scoped tree (default with a target), fast=foreground quick path, delta=changed only, text=readable text, marks=SoM debug image, selection=selected text of name/role element |
| `name` | string |  | Element name (mode=selection) |
| `role` | string |  | Element role (mode=selection) |
| `since_seq` | integer |  | Prior snapshot seq for delta mode |
| `window` | string |  | Title substring of the window to describe. Anchors it for later calls. 'foreground' = the human's current window. |

### `ghost_find`

Locate an element (name|role|description|text) and return center (always), rect (has_rect=true for UIA/cache hits), source, confidence, name, escalated (true = a network VLM call was paid). Target window = window= (anchored) or the session anchor; else the foreground. Under the default background policy nothing is focused or raised: the lookup is scoped to the window's own UIA subtree, so it works while the window is covered or lives on a hidden desktop.

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

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `action` | string: `click`, `type`, `double_click`, `right_click`, `hover` | yes | Action to perform |
| `background` | boolean |  | Compatibility flag: background dispatch is already the default. Real Win32 controls get posted messages; windowless (WinUI/Chromium/Electron) controls get UIA Invoke/ValuePattern; double_click/right_click/hover need a windowed control. focus_preserved in the response reports the truth. |
| `description` | string |  |  |
| `index` | integer |  | Act on the nth match (0-based) when several elements share the name/role. Honoured on every route (background, hidden desktop, CDP, foreground); use ghost_find with the same name/index first to see the matches count |
| `mode` | string: `instant`, `deliberate`, `instant_only` |  |  |
| `name` | string |  |  |
| `role` | string |  |  |
| `text` | string |  |  |
| `text_input` | string |  | Text to type when action=type (use this to avoid param collision with text-target) |
| `window` | string |  | Title substring of the window to act in (anchors it). Omit to use the anchor. |

### `ghost_key`

Send a key to a window WITHOUT focusing it. Target = window= (anchored) or the session anchor. Under the default background policy the key is POSTED to the window's focused control: single keys (Enter, Tab, F5, arrows, any character) and the editing shortcuts Ctrl+C/X/V/A/Z (sent as semantic messages). Other modifier combos have no background path and error naming the alternative. For text prefer ghost_act action=type (read-back verified). Hold/release keys='down:Shift' / 'up:Shift' exist only under a foreground policy.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `background` | boolean |  | Compatibility flag: background posting is already the default. |
| `keys` | string | yes | Key spec: 'Enter', 'a', 'Ctrl+C', 'down:Shift', 'up:Shift' |
| `window` | string |  | Title substring of the window to send to (anchors it). Omit to use the anchor. With no anchor at all the key goes to the human's foreground window and the response says so. |

### `ghost_snapshot`

Structured, agent-planning view of a window's UI: every element with a stable id, name, role, rect, center, enabled flag, actionable flag, and the actions it accepts (click/type). `actionable` = interactable role AND currently enabled - so a greyed-out button reads actionable:false. Far cheaper than a screenshot to reason over; plan here, then ghost_act by name/role. Params: window? (title substring; omitted = foreground), actionable_only? (only enabled interactable elements), limit?.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `actionable_only` | boolean |  | Return only interactable elements (buttons/edits/links/...) - cuts noise for planning |
| `limit` | integer |  | Max elements (default 150; 0 = unlimited) |
| `window` | string |  | Window title substring; omitted = foreground window (faster) |

### `ghost_scroll`

Scroll. direction: up/down/left/right, amount = notches (default 3). Target = window= (anchored) or the session anchor: under the default background policy the wheel is POSTED to that window (pointer never moves, window never raised; works on covered and hidden-desktop windows). 'Until' mode: until_name/until_role scrolls repeatedly until that element is present, up to max_scrolls; returns found. Coord mode (x/y at the real pointer) needs a foreground policy.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `amount` | integer |  |  |
| `direction` | string: `up`, `down`, `left`, `right` | yes |  |
| `max_scrolls` | integer |  | Max scroll steps in 'until' mode |
| `until_name` | string |  | Scroll until an element with this name is visible |
| `until_role` | string |  | Scroll until an element with this role is visible |
| `window` | string |  | Title substring of the window to scroll (anchors it). Omit to use the anchor. |
| `x` | integer |  |  |
| `y` | integer |  |  |

### `ghost_drag`

Click-hold at a start point, move to an end point, release. Each endpoint may be raw coords (from_x/from_y, to_x/to_y) OR an element (from_name/from_role, to_name/to_role) resolved to its center - e.g. drag a list row onto another, or a slider handle.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `from_name` | string |  | Start element name (instead of from_x/y) |
| `from_role` | string |  | Start element role |
| `from_x` | integer |  |  |
| `from_y` | integer |  |  |
| `to_name` | string |  | End element name (instead of to_x/y) |
| `to_role` | string |  | End element role |
| `to_x` | integer |  |  |
| `to_y` | integer |  |  |

### `ghost_wait`

Unified wait - use it instead of sleeping. for=element: an element (name/role) appears or disappears in the target window; returns the moment it does. for=value: an element's VALUE equals/contains/changes (forms, async fields, 'wait until the total updates'). for=navigate: load url in a browser window WITHOUT raising it - sets the address bar over UIA, presses Enter, returns when the window title changes (response: title_before, title_after, title_changed, ms); only a foreground focus policy takes the old focus-and-type path. for=idle: screen stable. for=text: click a target then wait for text. for=event: next foreground change. for=cond: JSONLogic poll. for=ms: a plain sleep - last resort, it costs its full length every run. Target window for element|value|idle|text|navigate = window= or the session anchor.

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
| `timeout_ms` | integer |  | Give up after this long (default 5000, max 600000; larger values are capped). Running out answers ok:false with timed_out:true for every for= |
| `url` | string |  | URL to navigate to (for=navigate) |
| `window` | string |  | Window scope (for=idle\|element\|value\|navigate); omit to use the anchor |

### `ghost_query`

Extract structured data from the screen. Strategy: UIA name-matching first, then a single batched VLM call for any fields still unmatched. Returns extracted object, unmatched list, and vlm_attempted flag.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `region` | array |  | Optional [left,top,right,bottom] region for VLM screenshot crop |
| `schema` | object |  | JSON Schema (properties map) or array of field names to extract |
| `window` | string |  | Scope to window (partial title) |

### `ghost_assert`

Assert a predicate about screen state. Fails (error) if not satisfied. text-present/text-absent: OCR text check. element-exists: element found. value-equals/value-contains: the element's actual value (ValuePattern) equals/contains 'text' - the fill-then-verify check.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `foreground` | boolean |  | Only without a window/anchor: OCR the foreground window (true) or the whole screen |
| `name` | string |  | Element name (element-exists\|value-*) |
| `predicate` | string: `text-present`, `text-absent`, `element-exists`, `value-equals`, `value-contains` | yes |  |
| `role` | string |  | Element role (element-exists\|value-*) |
| `text` | string |  | Text to check (text-present/absent) or expected value (value-equals/contains) |
| `window` | string |  | Title substring of the window to check (anchors it). Omit to use the anchor. text-present/absent then OCR THAT window by handle, never the foreground. |

### `ghost_run`

Execute a declarative step-by-step flow in one round-trip. Each step: {op, ...params}. Op is any lean verb, legacy tool name, or short name (find, click, type, press, click_at, wait...). RETRIES: a failing read-only step is retried up to max_retries; an acting step is NOT retried (its effect may have landed) unless the step sets retry:true; retry:false disables retries for a step; unknown op / missing param errors are never retried. CHAINING: a param value of "${steps.N.path}" is replaced with a field from step N's result before dispatch - e.g. {op:'find',name:'Save'} then {op:'ghost_click_at', x:'${steps.0.center.x}', y:'${steps.0.center.y}'}. A whole-string ref keeps its type (number stays number). SPEED: name the window once; later steps can omit window= because the anchor follows that window by handle through title changes (a stale title still resolves and the response carries title_drift). Wait with {op:'ghost_wait', for:'element'|'value'|'navigate'} rather than for=ms - a fixed sleep costs its full length on every run.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `json_flow` | string |  | JSON-encoded steps array string |
| `max_retries` | integer |  |  |
| `script` | string |  | YAML or JSON string of steps array (YAML tried first) |
| `steps` | array |  | Array of {op, ...params} steps (direct) |
| `stop_on_error` | boolean |  |  |

### `ghost_screenshot`

Capture a screenshot of the window the agent is working in. Target = window= or the session anchor: that window is captured BY HANDLE (works while covered, on a hidden desktop, or via CDP for a routed browser) at max 768px JPEG q=75 (~20-100KB). Only with no anchor does it fall back to the foreground window. Pass name/role to crop to ONE element inside the target, or rect=[l,t,r,b] (great for VLM-in-the-loop checks). full=true: the whole screen at max 1280px JPEG (max_dim=0 = native-res lossless PNG). Always includes size_bytes.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `foreground` | boolean |  | Only without a window/anchor: crop to the foreground window (default true) |
| `full` | boolean |  | Full-screen capture (default false) |
| `jpeg_quality` | integer |  |  |
| `max_dim` | integer |  | Longest-edge resize (default 768; 1280 with full=true; 0 = no resize, and with full=true a lossless PNG) |
| `name` | string |  | Crop to the element with this accessible name |
| `rect` | array |  | Crop to [left,top,right,bottom] region |
| `role` | string |  | Crop to the element with this role |
| `window` | string |  | Title substring of the window to capture (anchors it). Omit to use the anchor. |

### `ghost_window`

Window management across the user's desktop AND Ghost's hidden desktops. op=list: every window (name, pid, hwnd, focused, state, surface=user|hidden) plus the current anchor. op=focus: under the default background policy this ANCHORS the window and does NOT raise it (the anchored verbs drive it without focus); only an operator-unlocked prefer_background/foreground policy raises it. op=anchor: name= sets the anchor, clear=true clears it, no args reports it. op=state: maximize|minimize|restore|close (name+state); under the background policy these never activate the window, so the user's focus stays put (restore is how a minimized window becomes actionable). op=launch: start exe - under the background policy the app starts on a hidden desktop, never on your screen, and is anchored (see target.surface in the response).

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

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `op` | string: `get`, `set` |  | get=read, set=write (default get) |
| `text` | string |  | Text to write (op=set) |

### `ghost_stats`

Grounding + cache telemetry for debugging flow speed/reliability: which tier is winning (cache/UIA/OCR/VLM), VLM escalation rate, cache hit/miss counts, UIA-mirror stats. Call when a flow is slow or a find is unreliable.

No parameters.

### `ghost_reset`

Lift a stop raised with ghost_stop, for every Ghost process. A stop the human raised with Ctrl+Alt+G is refused: only the human can lift it, so wait for them.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `scope` | string: `everywhere`, `process` |  | everywhere (default): clears the stop for every Ghost process. process: only this server's own flag; a stop raised elsewhere stays. |

### `ghost_stop`

Emergency stop: halt all automation and release held modifier keys.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `scope` | string: `everywhere`, `process` |  | everywhere (default): every Ghost process in this logon session. process: only this server (used by ghost verify). |

### `ghost_http_get`

HTTP GET. Returns {status, body}.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `headers` | object |  |  |
| `url` | string | yes |  |

### `ghost_http_post`

HTTP POST. Returns {status, body}.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `body` | string |  |  |
| `content_type` | string |  |  |
| `headers` | object |  |  |
| `url` | string | yes |  |

### `ghost_session_state`

The window the user is working in (handle + title), the real cursor position, the focus policy and lock, and the interference audit (foreground changes with no human input while a tool was in flight). Call before/after a batch of actions to verify nothing was disturbed; compare foreground_hwnd, since titles change on their own.

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

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string: `chrome`, `comet`, `edge`, `brave` |  |  |
| `id` | string |  | Handle for later calls (default: 'default') |
| `mode` | string: `headless`, `windowed` |  |  |

### `ghost_browser_attach`

Attach to a browser already running with --remote-debugging-port=<port> (use for the user's own logged-in browser). Ghost never closes a browser it did not start, and drives background tabs without switching the user's active tab.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | string |  |  |
| `port` | integer | yes |  |

### `ghost_browser_close`

Close a browser ghost launched (attached browsers are only disconnected).

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | string |  |  |

### `ghost_browser_tabs`

List open tabs with target ids, titles, URLs.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | string |  |  |

### `ghost_browser_list_installed`

List Chromium-family browsers installed on this machine (chrome, comet, edge, brave).

No parameters.

### `ghost_tab_open`

Open a new background tab (never brings the tab or window to the front). Returns its id for the other ghost_tab_* tools. With an http(s) url it waits for the page to load, like ghost_tab_navigate, so the next read sees the page (loaded:true and the url it landed on; loaded:false with a note for a download or an empty reply). A page that fails to load or exceeds timeout_ms (the whole navigation) is an error, and the tab it opened is closed. wait=false answers at once, before the page loads. url must be http://, https:// or about:blank (default); file:, chrome:, data:, javascript: and the like are refused. http(s) to any host including localhost is allowed.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  |  |
| `timeout_ms` | integer |  | Load timeout when waiting, default 30000, max 600000 |
| `url` | string |  |  |
| `wait` | boolean |  | Wait for the page to load (default true; about:blank never waits) |

### `ghost_tab_navigate`

Navigate a tab and wait for load. Runs in a background tab; the user's view never changes. url must be http://, https:// or about:blank; every other scheme (file:, chrome:, data:, javascript:, ftp: ...) is refused. http(s) to any host including localhost is allowed. Returns an error (naming the net:: errorText) when the page fails to load, e.g. DNS failure or refused connection.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  |  |
| `tab` | string | yes |  |
| `timeout_ms` | integer |  | Load timeout, default 30000, max 600000 |
| `url` | string | yes |  |

### `ghost_tab_click`

Click an element by CSS selector via a trusted synthetic mouse event inside the tab's renderer. ~1-2ms; no cursor movement; works in a tab that is not in front.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  |  |
| `selector` | string | yes |  |
| `tab` | string | yes |  |
| `timeout_ms` | integer |  |  |

### `ghost_tab_type`

Focus an element by CSS selector and type into it. Layout-independent; does not touch the user's keyboard.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  |  |
| `clear` | boolean |  | Clear the field first (default true) |
| `selector` | string | yes |  |
| `tab` | string | yes |  |
| `text` | string | yes |  |

### `ghost_tab_press`

Send a key (Enter, Tab, ArrowDown, a, ...) to the tab's focused element. modifiers: Alt/Ctrl/Meta/Shift.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  |  |
| `key` | string | yes |  |
| `modifiers` | array |  |  |
| `tab` | string | yes |  |

### `ghost_tab_text`

Visible text of an element, or the whole page when selector is omitted. Prefer over screenshots for reading content.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  |  |
| `max_chars` | integer |  | Cap on returned characters (default 100000, max 2000000). When cut, the result carries truncated:true and original_length. |
| `selector` | string |  |  |
| `tab` | string | yes |  |

### `ghost_tab_eval`

Evaluate a JavaScript expression in the tab and return its value (awaits promises). The serialized result is capped at max_chars; when cut, value is the cut JSON string with truncated:true and original_length.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  |  |
| `expression` | string | yes |  |
| `max_chars` | integer |  | Cap on the serialized result in characters (default 100000, max 2000000). |
| `tab` | string | yes |  |

### `ghost_tab_screenshot`

PNG of a tab, rendered by that tab regardless of whether it is in front or focused.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  |  |
| `full_page` | boolean |  |  |
| `tab` | string | yes |  |

### `ghost_tab_describe`

Structured list of visible interactive elements with selectors and coordinates. Prefer this over screenshots for deciding what to click.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  |  |
| `limit` | integer |  |  |
| `tab` | string | yes |  |

### `ghost_tab_scroll`

Scroll a tab or a scrollable element inside it.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  |  |
| `dx` | number |  |  |
| `dy` | number |  |  |
| `selector` | string |  |  |
| `tab` | string | yes |  |

### `ghost_tab_select_option`

Choose an option in a <select>, firing the input/change events a real choice fires.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  |  |
| `selector` | string | yes |  |
| `tab` | string | yes |  |
| `value` | string | yes |  |

### `ghost_tab_wait_for`

Wait until a CSS selector matches an element in the tab.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  |  |
| `selector` | string | yes |  |
| `tab` | string | yes |  |
| `timeout_ms` | integer |  |  |

### `ghost_tab_close`

Close a tab by id.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  |  |
| `tab` | string | yes |  |

### `ghost_tab_find`

Find a tab whose URL or title contains a substring.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `browser` | string |  |  |
| `query` | string | yes |  |

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

No parameters.

### `ghost_set_focus_policy`

Set the focus policy. 'background' is always accepted and is the default. 'prefer_background' and 'foreground' let Ghost drive the user's real cursor and keyboard and are REFUSED while the lock is on (the default; see ghost_focus_policy). Do not retry a refusal: the error names the route that needs no policy (ghost_window op=launch, ghost_browser_launch). When the operator has unlocked the process, raise it only for a target with no background path and set it back afterwards.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `policy` | string: `background`, `prefer_background`, `foreground` | yes |  |

### `ghost_desktop_create`

Create an isolated Windows desktop - apps launched onto it NEVER appear on the user's screen (the desktop-app equivalent of headless). Usually unnecessary: under the default background policy ghost_window op=launch already starts apps on the hidden desktop 'auto' and the ordinary verbs (ghost_see/find/act/key/scroll with window=<title>) drive them there. UIA, window messages, and capture all work; real SendInput does not (OS boundary).

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | string |  |  |

### `ghost_desktop_close`

Destroy an isolated desktop, terminating any processes still on it (they have no visible window to close).

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | string |  |  |

### `ghost_desktop_launch`

Launch a program onto a specific isolated desktop (ghost_window op=launch does this automatically onto 'auto' under the background policy and anchors the window). Windows cannot move an existing window there - launch is the only entry.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `command` | string | yes |  |
| `desktop` | string |  |  |

### `ghost_desktop_windows`

Visible windows on an isolated desktop, with handles for the other desktop tools.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `desktop` | string |  |  |

### `ghost_desktop_wait_for_window`

Wait for a window with a matching title to appear on an isolated desktop.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `desktop` | string |  |  |
| `timeout_ms` | integer |  |  |
| `title` | string | yes |  |

### `ghost_desktop_click`

Click a client-area point in a window on an isolated desktop. button=left|right|double.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `button` | string: `left`, `right`, `double` |  |  |
| `desktop` | string |  |  |
| `hwnd` | integer | yes |  |
| `x` | integer | yes |  |
| `y` | integer | yes |  |

### `ghost_desktop_scroll`

Scroll a window on an isolated desktop.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `amount` | integer |  |  |
| `desktop` | string |  |  |
| `direction` | string: `up`, `down`, `left`, `right` |  |  |
| `hwnd` | integer | yes |  |

### `ghost_desktop_type`

Type into a window on an isolated desktop.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `desktop` | string |  |  |
| `hwnd` | integer | yes |  |
| `text` | string | yes |  |

### `ghost_desktop_press`

Send a key to a window on an isolated desktop.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `desktop` | string |  |  |
| `hwnd` | integer | yes |  |
| `key` | string | yes |  |

### `ghost_desktop_shortcut`

Editing shortcut (undo/cut/copy/paste/select_all, or 'Ctrl+Z' style) via the standard control messages - faked modifier keys would type a literal character instead.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `desktop` | string |  |  |
| `hwnd` | integer | yes |  |
| `shortcut` | string | yes |  |

### `ghost_desktop_capture`

PNG of a window on an isolated desktop - the only way to see what an invisible app is doing.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `desktop` | string |  |  |
| `hwnd` | integer | yes |  |

### `ghost_desktop_describe`

Interactive elements of an app on an isolated desktop via UIA. Prefer over capture-and-guess: UI Automation works on a non-displayed desktop.

| Parameter | Type | Required | Description |
| --- | --- | --- | --- |
| `desktop` | string |  |  |
| `window` | string |  |  |
