# Emergency-stop audit - every action entry point (Stage 1, 2026-11-26)

HEAD `9cbaf38`. Audit only: no code changed. Plan:
`docs/plans/2026-11-26-emergency-stop-gate.md`. Line numbers are at this HEAD.
Not reproduced live in this stage; the Stage 3 live test reproduces the report
before the fix lands.

Legend. **State** = changes something outside Ghost (input to a window,
foreground or window state, a launched process, the clipboard, a page, an
outbound request). **Gated** = refuses with `GhostError::Stopped` (or a stop
error) while the stop is active, and where the check is. "partial" = some of
what the path does is checked and some is not.

## 0. The stop itself

| Item | Where | Finding |
|---|---|---|
| Flag | `ghost-core/src/input/hotkey.rs:31` `STOP_FLAG`, `:39` `is_stopped()` | A **process-local** `AtomicBool`. `is_stopped()` never reads the shared event. |
| Shared event | `hotkey.rs:35` `Local\ghost-emergency-stop-event` (manual reset), `:43` `trigger_stop`, `:56` `reset_stop` | `trigger_stop` sets the flag and the event; `reset_stop` clears both. |
| Watcher | `hotkey.rs:102-127` (`register_emergency_stop`) | Copies the event into the local flag, waiting up to **1 s** per poll. Started only by `GhostSession::new` (`ghost-session/src/session.rs:426`). A process that starts while the event is signalled acts with `is_stopped() == false` until its watcher thread first runs. CLI and HTTP processes are exposed to this; a fresh `ghost-mcp` is too, until its first poll. |
| MCP stop | `ghost-mcp/src/main.rs:439-444` | The stdin reader trips the stop the moment a `ghost_stop` line arrives, before it is queued. Good: this is the "before queue" signal. |
| MCP reset | `main.rs:1734` `ghost_reset` → `session.rs:1483` `reset_stop` | |
| HTTP | `ghost-http/src/main.rs:300-315` | No stop or reset route; only the Ctrl+Alt+G hotkey or another process's event reaches it. |

Stage 2 consequence: the central gate must read the event directly
(`WaitForSingleObject(ev, 0)`) as well as the flag, or the cross-process race
above stays open.

## 1. MCP choke points (`crates/ghost-mcp/src/main.rs`)

| Entry | Line | State | Gated |
|---|---|---|---|
| stdin reader → channel | 439-450 | - | trips the stop; does not gate other lines |
| `tools/call` → `dispatch_tool` | 1501 | - | **no** |
| legacy raw-name call → `dispatch_tool` | 1523 | - | **no** |
| `dispatch_tool` / `dispatch_tool_inner` | 703 / 711 | the single choke point for MCP | **no**. Order inside: `ext::dispatch` (721), target resolution (725), CDP route (733), hidden route (750), user-desktop verbs (774-786). A gate at the top of `dispatch_tool_inner` covers all five. |
| `ghost_run` steps | 3044, step at 3088 via `dispatch_tool_inner` | yes | **no** (a gate in `dispatch_tool_inner` would cover each step) |
| `ghost_batch_actions` | 1976, ops via `run_batch_op` 3815 → `handle_tool` 3843/3865 | yes | **no**. Bypasses `dispatch_tool_inner`: a gate there blocks the batch at entry, but a stop that arrives mid-batch is caught only by the low-level checks below. Needs its own per-op check. |
| `ghost_navigate_and_wait` | 2061, re-dispatches `ghost_act` 2090 and `ghost_key` 2103 via `dispatch_tool` | yes | **no** (covered per step once `dispatch_tool_inner` is gated) |

## 2. MCP tools - user desktop, legacy dispatch (`handle_tool`, main.rs:1527)

| Tool | Line | State | Gated (where) |
|---|---|---|---|
| ghost_find | 1533 | no | n/a (`find` checks at session.rs:602 anyway) |
| ghost_click | 1587 | yes | yes, `GhostElement::click` element.rs:58 |
| ghost_type | 1593 | yes | yes, `GhostElement::type_text` element.rs:73 |
| ghost_click_at | 1600 | yes | yes: `click_at` session.rs:1226; background `click_at_background` session.rs:1257 |
| ghost_screenshot / _region | 1616 / 1692 | no | n/a |
| ghost_launch (and `ghost_window op=launch`) | 1713 | yes | **no**: `session.launch` session.rs:1472, and `launch_hidden` hidden.rs:327 under the background policy |
| ghost_stop / ghost_reset | 1730 / 1734 | control | must stay ungated |
| ghost_press / ghost_hotkey | 1738 / 1743 | yes | yes, session.rs:1493 / 1507 (and `keyboard::press_key` keyboard.rs:107) |
| ghost_key_down / _up | 1754 / 1759 | yes | yes, session.rs:1546 / 1555 |
| ghost_hover / right_click / double_click | 1764 / 1770 / 1776 | yes | yes, session.rs:1564 / 1570 / 1576 |
| ghost_drag | 1782 | yes | yes, session.rs:1582 |
| ghost_scroll | 1804 | yes | **partial**: foreground `scroll` session.rs:1588 and `scroll_until` 1919 are checked; the background-policy path `scroll_background` hidden.rs:773 → `desktop::scroll_window` desktop.rs:599 is **not**. Its `find_background` pre-check fails with Stopped, and the scroll is sent anyway. |
| ghost_get_selection / get_clipboard | 1848 / 1853 | no | n/a |
| ghost_set_clipboard (and `ghost_clipboard op=set`) | 1857 | yes | **no**: session.rs:1598 → `system/clipboard.rs:48` |
| ghost_list_windows | 1862 | no | n/a |
| ghost_focus_window (and `ghost_window op=focus` off the background policy) | 1907 | yes (raises a window) | **no**: session.rs:1755 → `uia/tree.rs:1058` |
| ghost_window_state (and `ghost_window op=state`) | 1912 | yes (min/max/restore/close) | **no**: session.rs:1760 → `uia/tree.rs:1170` |
| ghost_wait | 1918 | no | checked anyway (session.rs:1791) |
| ghost_describe_screen / _fast / read_text / get_text | 1930 / 1950 / 1954 / 2003 | no | n/a |
| ghost_http_get | 2008 | no (outbound read) | n/a |
| ghost_http_post | 2025 | yes (outbound side effects) | **no** |
| ghost_wait_until / wait_for_idle | 2047 / 2054 | no | checked anyway (session.rs:521 / 551) |
| ghost_click_and_wait_for_text (`ghost_wait for=text`) | 2166 | yes | yes, session.rs:2107 |
| ghost_fill_form | 2174 | yes | **partial**: session.rs:2207 has no check of its own; each field's `el.click()` (element.rs:58) and `keyboard::type_text` (keyboard.rs:54) are checked |
| ghost_execute_intent (and CLI/HTTP `run`) | 2187 | yes | **partial**: session.rs:3168 → FSM ops; press/hotkey/click checked, but `focus_window` (session.rs:3245) and `navigate_and_wait` (3241) raise windows unchecked |
| ghost_describe_screen_delta | 2192 | no | n/a |
| ghost_click_background | 2201 | yes | yes, session.rs:2229 |
| ghost_cache_stats / stats / render_marks / cache_invalidate / event_seq | 2208-2235 | no | n/a (`cache_invalidate` changes Ghost's own cache only) |
| ghost_locate_by_description / find_text_local | 2238 / 2259 | no | n/a |
| ghost_click_by_description | 2247 | yes | **partial**: session.rs:998/1002 has no check; the final click is checked (`click_at`) |
| ghost_type_by_description | 2253 | yes | **partial**: session.rs:1037; `click_at` and `keyboard::type_text` checked |
| ghost_click_text_local | 2268 | yes | yes, session.rs:1206 |
| ghost_wait_for_element / value / event | 2274 / 2281 / 2289 | no | checked anyway (session.rs:2140 / 2177) |
| ghost_act (user desktop) | 2297 | yes | **partial**. Background: `act_background` session.rs:2540 checked. Foreground: `act` 2249 → `act_on_element` 2304 raises the window (`focus_window_under_point`, tree.rs:978) and calls `el.set_focus()` **before** the checked `el.click`/`type_text`, so the foreground still moves while stopped. `act_at` 3036 likewise raises before checked input. |
| ghost_key (lean) | 2642 | yes | yes on the user desktop: `key_background` session.rs:2879, `edit_command_background` 2933, foreground `press`/`hotkey` |
| ghost_see / snapshot / query / assert | 2624 / 2594 / 3216 / 2906 | no | n/a |
| ghost_window | 2816 | `list`/`anchor` no; `focus`/`state`/`launch` yes | see focus_window / window_state / launch rows |
| ghost_shell | 2882 | yes | yes, `shell_run` shell.rs:364 (and output loops 445, 590, 674, 701) |
| ghost_clipboard | 2890 | `get` no / `set` yes | see set_clipboard |

## 3. MCP - hidden-desktop route (`dispatch_hidden`, main.rs:1151) - the reported bypass

No function in `crates/ghost-session/src/hidden.rs` checks the stop.

| Verb | main.rs | Session fn | State | Gated |
|---|---|---|---|---|
| ghost_see / snapshot / find | 1159 / 1175 / 1179 | `hidden_describe` 416, `hidden_read_text` 425, `hidden_find` 433 | no | n/a |
| **ghost_act** | 1184 | `hidden_act` hidden.rs:463 → `with_uia` worker → `BackgroundClicker::click_screen` / `button_click` / `send_char` / `set_text` / `double_click_screen` / `right_click_screen` / `hover_screen`, `patterns::invoke_ex` / `set_value_ex` (hidden.rs:529-622) | yes | **no** (the reported defect) |
| ghost_key | 1198 | `hidden_key` hidden.rs:659 → worker `send_char`, `d.shortcut`, `d.press` (hidden.rs:666-686) | yes | **no** |
| ghost_click_at | 1202 | `hidden_click_at` hidden.rs:703 → worker `click_screen` (718) | yes | **no** |
| ghost_scroll | 1207 | `hidden_scroll` hidden.rs:764 → `DesktopSession::scroll` | yes | **no** |
| ghost_assert / screenshot / wait | 1229 / 1273 / 1294 | find/value/capture | no | n/a |
| launch onto a hidden desktop | `launch_hidden` hidden.rs:327 (from ghost_launch / ghost_window op=launch under the background policy) | `DesktopSession::launch` desktop.rs:170 | yes | **no** |

## 4. Hidden-desktop worker (`crates/ghost-core/src/desktop.rs`)

| Fn | Line | State | Gated |
|---|---|---|---|
| `DesktopSession::exec` (the worker job queue; every hidden op goes through it) | 150 | carries jobs | **no**. This is the "immediately before dispatch on the desktop worker" point. |
| `with_uia` | 408 | carries jobs | **no** |
| `launch` | 170 | yes | **no** |
| `click` / `right_click` / `double_click` / `hover` | 228 / 232 / 236 / 240 | yes | **no** |
| `scroll` | 244 | yes | **no** |
| `type_text` / `press` / `shortcut` | 281 / 362 / 378 | yes | **no** |
| `focus_window` (on its own desktop) | 439 | yes | **no** |
| `scroll_window` (free fn, used by user-desktop background scroll) | 599 | yes | **no** |
| `windows` / `all_windows` / `wait_for_window` / `read_text` / `capture` | 177-259, 428 | no | n/a |

## 5. Explicit desktop tools (`crates/ghost-mcp/src/ext.rs` → `ghost-session/src/registries.rs`)

`ext::dispatch` (ext.rs:33) runs before everything else in `dispatch_tool_inner`. Nothing in ext.rs or registries.rs checks the stop.

| Tool | ext.rs | Session fn (registries.rs) | State | Gated |
|---|---|---|---|---|
| ghost_desktop_create | 204 | `desktop_create` 286 | yes (creates a desktop) | **no** |
| ghost_desktop_close | 209 | `desktop_close` 303 (kills every process on it) | yes | **no** |
| ghost_desktop_launch | 214 | `desktop_launch` 321 | yes | **no** |
| ghost_desktop_click | 237 | `desktop_click` 357 | yes | **no** |
| ghost_desktop_scroll | 249 | `desktop_scroll` 375 | yes | **no** |
| ghost_desktop_type | 260 | `desktop_type` 396 | yes | **no** |
| ghost_desktop_press | 267 | `desktop_press` 404 | yes | **no** |
| ghost_desktop_shortcut | 274 | `desktop_shortcut` 412 | yes | **no** |
| ghost_desktop_windows / wait_for_window / capture / describe | 221 / 227 / 284 / 290 | 329 / 342 / 420 / 431 | no | n/a |
| ghost_focus_policy / session_state | 41 / 53 | - | no | n/a |
| ghost_set_focus_policy | 47 | `set_focus_policy` 85 | config (Ghost's own policy) | **no**. Recommend gated: not observation-only. |

## 6. Browser / CDP (`ext.rs` tab tools; `dispatch_cdp` main.rs:864; `crates/ghost-browser`)

Nothing in ghost-browser, registries.rs or `dispatch_cdp` checks the stop.

| Entry | Where | State | Gated |
|---|---|---|---|
| ghost_browser_launch / attach / close | ext.rs:68 / 77 / 82 → registries.rs:102 / 170 / 193 → launch.rs:330 / 530, browser.rs:126 | yes (launch/close); attach no | **no** |
| ghost_browser_tabs / list_installed | ext.rs:87 / 92 | no | n/a |
| ghost_tab_open / close | ext.rs:101 / 107 → registries.rs:216 / 248 (browser.rs:108) | yes | **no** |
| ghost_tab_navigate | ext.rs:118 → tab.rs:135 | yes | **no** |
| ghost_tab_click | ext.rs:126 → tab.rs:241 | yes | **no** |
| ghost_tab_type | ext.rs:134 → tab.rs:473 | yes | **no** |
| ghost_tab_press | ext.rs:143 → tab.rs:505 | yes | **no** |
| ghost_tab_eval | ext.rs:157 → tab.rs:104 (arbitrary JS) | yes | **no** |
| ghost_tab_scroll | ext.rs:176 → tab.rs:560 | yes | **no** |
| ghost_tab_select_option | ext.rs:185 → tab.rs:580 | yes | **no** |
| ghost_tab_find / text / screenshot / describe / wait_for | ext.rs:112 / 151 / 163 / 170 / 193 | no | n/a |
| CDP route of window verbs: ghost_act | main.rs:926 (tab.click 955, click_at_ex 960/965, hover 969, type_text 975) | yes | **no** |
| CDP route: ghost_key | main.rs:994 (tab.press 997) | yes | **no** |
| CDP route: ghost_click_at | main.rs:1004 (tab.click_at 1013) | yes | **no** |
| CDP route: ghost_scroll | main.rs:1020 (tab.scroll 1042/1048) | yes | **no** |
| CDP route: see / snapshot / find / assert / wait / screenshot | main.rs:878 / 905 / 912 / 1051 / 1085 / 1137 | no | n/a |
| Lower-level CDP input | tab.rs:277 `click_at`, 283 `click_at_ex`, 460 `hover` | yes | **no** |

## 7. CLI (`crates/ghost-cli/src/main.rs`, `run` 240)

Each invocation is a new process: it inherits the 1 s watcher race above (it may act
before learning of a stop already in force).

| Subcommand | Line | State | Gated |
|---|---|---|---|
| click | 252 | yes | yes (element.rs:58) |
| click-at | 258 | yes | yes (session.rs:1226) |
| type | 262 | yes | yes (element.rs:73) |
| press / hotkey | 268 / 272 | yes | yes (session.rs:1493 / 1507) |
| screenshot / list-windows / describe / see / find / query / snapshot / get-clipboard | 277 / 287 / 302 / 339 / 366 / 447 / 468 / 317 | no | n/a |
| launch | 283 | yes | **no** |
| focus-window | 294 | yes | **no** |
| window-state | 298 | yes | **no** |
| set-clipboard | 321 | yes | **no** |
| run (intent) | 325 | yes | **partial** (as ghost_execute_intent) |
| act | 378 | yes | yes (click_at / double / right / hover, session.rs:1226-1576) |
| key | 402 | yes | yes (key_down/up/press/hotkey) |
| window (op list / focus / state / launch) | 419 | focus/state/launch yes | **no** for focus (430), state (436), launch (441) |
| serve / doctor / verify | 488 / 192 / 198 | serve execs ghost-mcp; verify drives a testbed through it | inherit ghost-mcp |

## 8. HTTP (`crates/ghost-http/src/main.rs`, actor 77-170, routes 300-315)

| Route | Handler | State | Gated |
|---|---|---|---|
| POST /click | 84 | yes | yes (element.rs:58) |
| POST /click-at | 92 | yes | yes (session.rs:1226) |
| POST /type | 96 | yes | yes (element.rs:73) |
| POST /press, /hotkey | 104 / 108 | yes | yes |
| GET /screenshot, /list-windows, /health, /, /tools; POST /describe; GET /clipboard | 113 / 121 / 138 / 154 | no | n/a |
| POST /launch | 117 | yes | **no** |
| POST /focus-window | 130 | yes | **no** |
| POST /window-state | 134 | yes | **no** |
| POST /clipboard | 158 | yes | **no** |
| POST /run | 162 | yes | **partial** |

## 9. Low-level primitives (`crates/ghost-core`)

| Fn | Line | State | Gated |
|---|---|---|---|
| SendInput: `mouse::click`, `move_to`, `hover`, `right_click`, `double_click`, `drag`, `scroll` | mouse.rs:82 / 98 / 139 / 155 / 171 / 191 / 212 | yes | yes (each) |
| SendInput: `keyboard::type_text`, `clear_focused_field`, `press_key`, `key_down`, `key_up` | keyboard.rs:51 / 71 / 106 / 122 / 138 | yes | yes (each) |
| `hotkey::release_all_modifiers` | hotkey.rs:181 | key-ups only | must stay ungated (part of stopping) |
| PostMessage: `BackgroundClicker::click` | postmessage.rs:81 | yes | **no** |
| `click_screen` | postmessage.rs:103 | yes | **no** |
| `double_click_screen` / `right_click_screen` / `hover_screen` | 129 / 146 / 163 | yes | **no** |
| `button_click` | 176 | yes | **no** |
| `edit_command` | 200 | yes | **no** |
| `send_key` / `send_char` / `set_text` | 374 / 411 / 430 | yes | **no** |
| `focused_control` / `text_target` / `read_text` | 229 / 309 / 322 | no | n/a |
| UIA `patterns::invoke` / `invoke_ex` | patterns.rs:60 / 68 | yes | **no** |
| UIA `patterns::set_value` / `set_value_ex` | 136 / 143 | yes | **no** |
| UIA `UiaElement::scroll_into_view` / `set_focus` | uia/element.rs:79 / 94 | yes (scroll / keyboard focus) | **no** |
| Foreground: `tree::ensure_foreground`, `focus_window_under_point`, `focus_window`, `set_window_state` | uia/tree.rs:905 / 978 / 1058 / 1170 | yes | **no** (note: `foreground_guard_end_within` uses `ensure_foreground` to hand the foreground back to the human; that hand-back should stay allowed) |
| `restore_if_hidden` | tree.rs:1014 | yes | **no** (same: protective) |
| `process::launch` / `kill` | process/manager.rs:141 (launch), 193 (kill) | yes | **no** (`kill` is also used by cleanup and should stay allowed) |
| `system::clipboard::set_clipboard` | system/clipboard.rs:48 | yes | **no** |

Session-layer wrappers that already check (and so cover their user-desktop
callers): `GhostElement::click` / `type_text` / `click_background` /
`type_text_background` (element.rs:58 / 73 / 116 / 130), and session.rs:1206,
1226, 1257, 1493-1608, 1657, 2107, 2229, 2540, 2879, 2933.

## 10. Summary

- **Confirmed by reading:** the hidden-desktop route (`dispatch_hidden` → hidden.rs
  `hidden_act` / `hidden_key` / `hidden_click_at` / `hidden_scroll` / `launch_hidden`),
  the desktop worker (`DesktopSession::exec`), and the PostMessage and UIA
  primitives have no stop check. That matches the report exactly.
- **Wider than reported:**
  - Every explicit `ghost_desktop_*` action, all browser/tab/CDP input, and
    launch on every surface are ungated.
  - So are focus and window-state changes, clipboard set, and `ghost_http_post`.
  - The user-desktop background scroll is ungated.
  - The foreground `act` / `act_at` paths raise the window before their input
    check.
  - `ghost_batch_actions` bypasses the MCP choke point between ops.
- **Not a gating gap but a correctness gap in the flag itself:** `is_stopped()`
  is process-local, and a new process learns of a stop up to 1 s late. The
  central gate should read the shared event too.
- **Must stay allowed while stopped:**
  - `ghost_stop` and `ghost_reset`;
  - `release_all_modifiers`;
  - the foreground hand-back (`foreground_guard_end_within`, `restore_if_hidden`);
  - cleanup `kill`;
  - every observation-only tool listed "no" above.
- **Single MCP choke point:** the top of `dispatch_tool_inner` (main.rs:711), ahead
  of `ext::dispatch`. It also needs a per-op check inside `run_batch_op` (3815),
  plus the worker-side check in `DesktopSession::exec` / `with_uia`.
