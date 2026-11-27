# Reporter's repro over real MCP, before and after 0.24.1

Date: 2026-11-26, 2:00 PM AKST. Machine: the maintainer's Windows 11 workstation (build 26300).
Server: `ghost-mcp.exe` 0.24.1 (sha256 `8157213bc415c5a1bc12c7918f88db87ea96b4c5b793def17abeec446557fb86`,
commit 757f416), spawned over stdio by a minimal JSON-RPC driver with
`GHOST_FOCUS_LOCK=on GHOST_FOCUS_POLICY=background GHOST_SHELL=off`. Foreground window titles and the
desktop window list are redacted.

## Before (v0.23.4, as reported, and v0.24.0 by code reading)

After `ghost_stop` returned ok (the shared manual-reset event `Local\ghost-emergency-stop-event`
signalled), `ghost_act click name="Advanced view" role=checkbox mode=instant_only` on the hidden-desktop
Character Map dispatched and toggled the checkbox. The session-level live test reproduced the same gap
on 0.24.0 for seven paths: see `live-before-fix.txt`.

## After (0.24.1)

| Step | Result |
|---|---|
| `ghost_window launch charmap.exe` | ok, `surface: "hidden"` (never on the user desktop) |
| `ghost_see full` | 14 elements, no Advanced-view controls (no "Character set :", "Group by :", "Search for :") |
| `ghost_stop` | ok |
| `ghost_act click "Advanced view" checkbox instant_only` | **refused**, 0 ms: `ghost_act refused: emergency stop active - Ghost is stopped and refuses actions until ghost_reset`, error_code -32004 |
| `ghost_see full` | unchanged: still no Advanced-view controls, so the checkbox did not toggle |
| Second `ghost-mcp` process started 3 s into a stop (shared event only) | `ghost_act`, `ghost_window launch`, `ghost_key Enter` all **refused** at 0-1 ms; `ghost_see` (read) still works |
| CLI `ghost launch ...`, `ghost click --name "Advanced view"` (third process) during the stop | both **refused**, exit 1: `emergency stop active (Ctrl+Alt+G or ghost_stop): Ghost is stopped and refuses actions until ghost_reset` |
| `ghost_reset` | ok |
| same `ghost_act click` | ok, `mode: "hidden"`, 135 ms |
| `ghost_see full` | Advanced-view controls now present ("Character set :", "Go to Unicode :", "Group by :", "Search for :"): the checkbox toggled |
| `ghost_window state close` | ok; Character Map process gone |
| Fresh process after reset: launch testbed hidden, `ghost_desktop_close id=auto` | both ok: the shared stop is clear |

The checkbox's toggle state is read through the controls Advanced view reveals, because `ghost_see`
does not report toggle state for this control.

## Raw transcript: main session (stop, refused click, reset, click)

```
>>> ghost_window {"op":"launch","exe":"charmap.exe"}  (557ms)
<<< {"content":[{"text":"{\"data\":{\"desktop\":\"auto\",\"note\":\"started on a hidden desktop (never on your screen) and anchored: ghost_see / ghost_act / ghost_key now target it by default. Drive it by title like any other window; ghost_desktop_close id=auto ends it.\",\"pid\":13148,\"surface\":\"hidden\",\"target\":{\"desktop\":\"auto\",\"hwnd\":165216524,\"pid\":13148,\"source\":\"anchor\",\"surface\":\"hidden\",\"title\":\"Character Map\"},\"window\":{\"hwnd\":165216524,\"pid\":13148,\"title\":\"Character Map\"}},\"foreground\":{\"hwnd\":1116950,\"title\":\"<redacted>\"},\"ms\":555,\"ok\":true}","type":"text"}]}

>>> ghost_window {"op":"list","include_hidden":true}  (2ms)
<<< [window list of the operator desktop omitted for privacy; it showed Character Map only on surface=hidden, never on the user desktop]

>>> ghost_see {"mode":"full","window":"Character Map","limit":40}  (47ms)
<<< {"content":[{"text":"{\"data\":{\"elements\":[{\"bottom\":431,\"left\":158,\"name\":\"Characters to copy :\",\"right\":323,\"role\":\"edit\",\"top\":405},{\"bottom\":428,\"left\":329,\"name\":\"Select\",\"right\":404,\"role\":\"button\",\"top\":405},{\"bottom\":428,\"left\":410,\"name\":\"Copy\",\"right\":485,\"role\":\"button\",\"top\":405},{\"bottom\":469,\"left\":34,\"name\":\"Advanced view\",\"right\":226,\"role\":\"checkbox\",\"top\":449},{\"bottom\":136,\"left\":86,\"name\":\"Font :\",\"right\":407,\"role\":\"combobox\",\"top\":106},{\"bottom\":135,\"left\":391,\"name\":\"Open\",\"right\":406,\"role\":\"button\",\"top\":107},{\"bottom\":129,\"left\":413,\"name\":\"Help\",\"right\":488,\"role\":\"hyperlink\",\"top\":106},{\"bottom\":163,\"left\":470,\"name\":\"Line up\",\"right\":491,\"role\":\"button\",\"top\":146},{\"bottom\":380,\"left\":470,\"name\":\"Page down\",\"right\":491,\"role\":\"button\",\"top\":180},{\"bottom\":397,\"left\":470,\"name\":\"Line down\",\"right\":491,\"role\":\"button\",\"top\":380},{\"bottom\":95,\"left\":23,\"name\":\"System\",\"right\":45,\"role\":\"menuitem\",\"top\":73},{\"bottom\":96,\"left\":359,\"name\":\"Minimize\",\"right\":406,\"role\":\"button\",\"top\":66},{\"bottom\":96,\"left\":406,\"name\":\"Maximize\",\"right\":452,\"role\":\"button\",\"top\":66},{\"bottom\":96,\"left\":452,\"name\":\"Close\",\"right\":499,\"role\":\"button\",\"top\":66}],\"target\":{\"desktop\":\"auto\",\"hwnd\":165216524,\"pid\":13148,\"source\":\"explicit\"...[truncated]

>>> ghost_stop {}  (18ms)
<<< {"content":[{"text":"{\"data\":{\"ok\":true},\"foreground\":{\"hwnd\":1116950,\"title\":\"<redacted>\"},\"ms\":8,\"ok\":true}","type":"text"}]}

>>> ghost_act {"action":"click","name":"Advanced view","role":"checkbox","mode":"instant_only","window":"Character Map"}  (0ms)
<<< {"content":[{"text":"{\n  \"data\": null,\n  \"error\": \"ghost_act refused: emergency stop active - Ghost is stopped and refuses actions until ghost_reset\",\n  \"error_code\": -32004,\n  \"foreground\": {\n    \"hwnd\": 1116950,\n    \"title\": \"<redacted>\"\n  },\n  \"ms\": 0,\n  \"ok\": false,\n  \"suggested_action\": \"automation was stopped - call ghost_reset before issuing more actions\"\n}","type":"text"}],"isError":true}

>>> ghost_see {"mode":"full","window":"Character Map","limit":40}  (26ms)
<<< {"content":[{"text":"{\"data\":{\"elements\":[{\"bottom\":431,\"left\":158,\"name\":\"Characters to copy :\",\"right\":323,\"role\":\"edit\",\"top\":405},{\"bottom\":428,\"left\":329,\"name\":\"Select\",\"right\":404,\"role\":\"button\",\"top\":405},{\"bottom\":428,\"left\":410,\"name\":\"Copy\",\"right\":485,\"role\":\"button\",\"top\":405},{\"bottom\":469,\"left\":34,\"name\":\"Advanced view\",\"right\":226,\"role\":\"checkbox\",\"top\":449},{\"bottom\":136,\"left\":86,\"name\":\"Font :\",\"right\":407,\"role\":\"combobox\",\"top\":106},{\"bottom\":135,\"left\":391,\"name\":\"Open\",\"right\":406,\"role\":\"button\",\"top\":107},{\"bottom\":129,\"left\":413,\"name\":\"Help\",\"right\":488,\"role\":\"hyperlink\",\"top\":106},{\"bottom\":163,\"left\":470,\"name\":\"Line up\",\"right\":491,\"role\":\"button\",\"top\":146},{\"bottom\":380,\"left\":470,\"name\":\"Page down\",\"right\":491,\"role\":\"button\",\"top\":180},{\"bottom\":397,\"left\":470,\"name\":\"Line down\",\"right\":491,\"role\":\"button\",\"top\":380},{\"bottom\":95,\"left\":23,\"name\":\"System\",\"right\":45,\"role\":\"menuitem\",\"top\":73},{\"bottom\":96,\"left\":359,\"name\":\"Minimize\",\"right\":406,\"role\":\"button\",\"top\":66},{\"bottom\":96,\"left\":406,\"name\":\"Maximize\",\"right\":452,\"role\":\"button\",\"top\":66},{\"bottom\":96,\"left\":452,\"name\":\"Close\",\"right\":499,\"role\":\"button\",\"top\":66}],\"target\":{\"desktop\":\"auto\",\"hwnd\":165216524,\"pid\":13148,\"source\":\"explicit\"...[truncated]

>>> ghost_reset {}  (1ms)
<<< {"content":[{"text":"{\"data\":{\"ok\":true},\"foreground\":{\"hwnd\":1574710,\"title\":\"<redacted>\"},\"ms\":0,\"ok\":true}","type":"text"}]}

>>> ghost_act {"action":"click","name":"Advanced view","role":"checkbox","mode":"instant_only","window":"Character Map"}  (135ms)
<<< {"content":[{"text":"{\"data\":{\"action\":\"click\",\"cursor_preserved\":true,\"focus_preserved\":true,\"mode\":\"hidden\",\"name\":\"Advanced view\",\"note\":\"click dispatched; the window surface could not be compared - read state to confirm\",\"ok\":true,\"rect\":{\"bottom\":469,\"left\":34,\"right\":226,\"top\":449},\"target\":{\"desktop\":\"auto\",\"hwnd\":165216524,\"pid\":13148,\"source\":\"explicit\",\"surface\":\"hidden\",\"title\":\"Character Map\"},\"timing_ms\":{\"action\":105,\"dispatch\":0,\"find\":28},\"verified\":null,\"window\":\"Character Map\"},\"foreground\":{\"hwnd\":1574710,\"title\":\"<redacted>\"},\"ms\":135,\"ok\":true}","type":"text"}]}

>>> ghost_see {"mode":"full","window":"Character Map","limit":40}  (51ms)
<<< {"content":[{"text":"{\"data\":{\"elements\":[{\"bottom\":431,\"left\":158,\"name\":\"Characters to copy :\",\"right\":323,\"role\":\"edit\",\"top\":405},{\"bottom\":428,\"left\":329,\"name\":\"Select\",\"right\":404,\"role\":\"button\",\"top\":405},{\"bottom\":428,\"left\":410,\"name\":\"Copy\",\"right\":485,\"role\":\"button\",\"top\":405},{\"bottom\":469,\"left\":34,\"name\":\"Advanced view\",\"right\":226,\"role\":\"checkbox\",\"top\":449},{\"bottom\":508,\"left\":158,\"name\":\"Character set :\",\"right\":361,\"role\":\"combobox\",\"top\":478},{\"bottom\":507,\"left\":345,\"name\":\"Open\",\"right\":360,\"role\":\"button\",\"top\":479},{\"bottom\":501,\"left\":446,\"name\":\"Go to Unicode :\",\"right\":484,\"role\":\"edit\",\"top\":478},{\"bottom\":549,\"left\":158,\"name\":\"Group by :\",\"right\":361,\"role\":\"combobox\",\"top\":519},{\"bottom\":548,\"left\":345,\"name\":\"Open\",\"right\":360,\"role\":\"button\",\"top\":520},{\"bottom\":582,\"left\":158,\"name\":\"Search for :\",\"right\":361,\"role\":\"document\",\"top\":559},{\"bottom\":589,\"left\":377,\"name\":\"Search\",\"right\":452,\"role\":\"button\",\"top\":566},{\"bottom\":136,\"left\":86,\"name\":\"Font :\",\"right\":407,\"role\":\"combobox\",\"top\":106},{\"bottom\":135,\"left\":391,\"name\":\"Open\",\"right\":406,\"role\":\"button\",\"top\":107},{\"bottom\":129,\"left\":413,\"name\":\"Help\",\"right\":488,\"role\":\"hyperlink\",\"top\":106},{\"bottom\":163,\"left\":470,\"name\":\"Line up\",\"right\":491,...[truncated]

>>> ghost_window {"op":"state","name":"Character Map","state":"close"}  (41ms)
<<< {"content":[{"text":"{\"data\":{\"ok\":true},\"foreground\":{\"hwnd\":65898,\"title\":\"<redacted>\"},\"ms\":40,\"ok\":true}","type":"text"}]}

>>> ghost_window {"op":"list","include_hidden":true}  (2ms)
<<< [window list of the operator desktop omitted for privacy; it showed Character Map only on surface=hidden, never on the user desktop]

```

## Raw transcript: stopper process (stop, wait 12 s, reset)

```
>>> ghost_stop {}  (25ms)
<<< {"content":[{"text":"{\"data\":{\"ok\":true},\"foreground\":{\"hwnd\":65898,\"title\":\"<redacted>\"},\"ms\":10,\"ok\":true}","type":"text"}]}

>>> ghost_reset {}  (0ms)
<<< {"content":[{"text":"{\"data\":{\"ok\":true},\"foreground\":{\"hwnd\":4655218,\"title\":\"<redacted>\"},\"ms\":0,\"ok\":true}","type":"text"}]}

```

## Raw transcript: second ghost-mcp process started during the stop

```
>>> ghost_act {"action":"click","name":"Advanced view","role":"checkbox","mode":"instant_only","window":"Character Map"}  (0ms)
<<< {"content":[{"text":"{\n  \"data\": null,\n  \"error\": \"ghost_act refused: emergency stop active - Ghost is stopped and refuses actions until ghost_reset\",\n  \"error_code\": -32004,\n  \"foreground\": {\n    \"hwnd\": 65898,\n    \"title\": \"\"\n  },\n  \"ms\": 0,\n  \"ok\": false,\n  \"suggested_action\": \"automation was stopped - call ghost_reset before issuing more actions\"\n}","type":"text"}],"isError":true}

>>> ghost_window {"op":"launch","exe":"<repo>/target/release/ghost-testbed.exe"}  (0ms)
<<< {"content":[{"text":"{\n  \"data\": null,\n  \"error\": \"ghost_window refused: emergency stop active - Ghost is stopped and refuses actions until ghost_reset\",\n  \"error_code\": -32004,\n  \"foreground\": {\n    \"hwnd\": 65898,\n    \"title\": \"\"\n  },\n  \"ms\": 0,\n  \"ok\": false,\n  \"suggested_action\": \"automation was stopped - call ghost_reset before issuing more actions\"\n}","type":"text"}],"isError":true}

>>> ghost_key {"key":"Enter"}  (1ms)
<<< {"content":[{"text":"{\n  \"data\": null,\n  \"error\": \"ghost_key refused: emergency stop active - Ghost is stopped and refuses actions until ghost_reset\",\n  \"error_code\": -32004,\n  \"foreground\": {\n    \"hwnd\": 65898,\n    \"title\": \"\"\n  },\n  \"ms\": 0,\n  \"ok\": false,\n  \"suggested_action\": \"automation was stopped - call ghost_reset before issuing more actions\"\n}","type":"text"}],"isError":true}

>>> ghost_see {"mode":"fast","limit":3}  (56ms)
<<< {"content":[{"text":"{\"data\":{\"elements\":[{\"bottom\":1440,\"left\":6,\"name\":\"Widgets 15°F Mostly clear\",\"right\":158,\"role\":\"button\",\"top\":1392},{\"bottom\":1440,\"left\":860,\"name\":\"Start\",\"right\":905,\"role\":\"button\",\"top\":1392},{\"bottom\":1432,\"left\":907,\"name\":\"Search\",\"right\":1127,\"role\":\"button\",\"top\":1400}],\"target\":{\"hwnd\":65898,\"pid\":0,\"source\":\"foreground\",\"surface\":\"user\",\"title\":\"\"},\"target_note\":\"no window anchored: this acted on the human's current foreground window. Pass window=<title substring> (or ghost_window op=anchor name=...) so the automation's own window is the target from here on.\",\"total\":23,\"truncated\":true},\"foreground\":{\"hwnd\":65898,\"title\":\"<redacted>\"},\"ms\":56,\"ok\":true}","type":"text"}]}

```

## CLI during the stop (third process)

```
=== CLI (third process, during stop) ===
ghost: emergency stop active (Ctrl+Alt+G or ghost_stop): Ghost is stopped and refuses actions until ghost_reset
cli launch exit 1
ghost: emergency stop active (Ctrl+Alt+G or ghost_stop): Ghost is stopped and refuses actions until ghost_reset
cli click exit 1
```
