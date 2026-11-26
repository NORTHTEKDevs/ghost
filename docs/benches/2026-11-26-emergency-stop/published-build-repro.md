# Reporter's repro against the published v0.24.1 release

Date: 2026-11-26, 2:28 PM AKST. Machine: the maintainer's Windows 11 workstation (build 26300).
The binaries were the **published** ones, not a local build: `ghost-windows-x64.zip` was downloaded
from the v0.24.1 release (`gh release download v0.24.1 -p ghost-windows-x64.zip`) into a fresh
directory and extracted.

| | |
|---|---|
| zip sha256 (computed) | `4ec6203cbbf1890618b88d1da34d20fd8af2ad48a79f4cfe4bef9b471647335f` |
| `ghost-windows-x64.zip.sha256` asset | same value: match |
| extracted `ghost-mcp.exe` sha256 | `4f7f5d78c53c4061826c4ee44bb2be41babbf2bfd6674efacb21c81bc368cbbd` |
| extracted `ghost.exe` sha256 | `6580b22ac79287f82e28dc84f231343cf94fc00f855e88c0106ae9a3fed40281` |
| `ghost.exe --version` | `ghost 0.24.1` |

Every MCP server was the extracted `ghost-mcp.exe`, spawned over stdio by the same minimal JSON-RPC
driver as `reporter-repro.md`, with `GHOST_FOCUS_LOCK=on GHOST_FOCUS_POLICY=background GHOST_SHELL=off`.
The CLI was the extracted `ghost.exe`. The testbed (not in the release zip) was
`target/release/ghost-testbed.exe`. Foreground window titles, the desktop window list, and reads of
the human's foreground window are redacted.

The shared stop event `Local\ghost-emergency-stop-event` was read directly by a separate PowerShell
process: `OpenEventW(SYNCHRONIZE)` + `WaitForSingleObject(h, 0)`, read-only (0 = signalled,
258 = clear). It never sets or resets the event.

## Result

| Time | Step (process) | Result |
|---|---|---|
| 14:28:13.899 | event read, before A | clear (258) |
| 14:28:14 | A: `ghost_window launch charmap.exe` | ok, 449 ms, `surface: "hidden"` |
| | A: `ghost_see full` | Advanced view ON (the "Character set :", "Group by :", "Search for :" controls present) |
| | A: `ghost_stop` | ok, 1 ms |
| | A: `ghost_act click "Advanced view" checkbox instant_only` | **refused**, 0 ms, error_code -32004: `ghost_act refused: emergency stop active - Ghost is stopped and refuses actions until ghost_reset` |
| | A: `ghost_see full` | unchanged: Advanced view still ON, so the checkbox did not toggle |
| 14:28:21.250 | event read, during the stop | **SIGNALLED (0)** |
| 14:28:21.379 | CLI (third process): `ghost launch ghost-testbed.exe` | **refused**, exit 1: `ghost: emergency stop active (Ctrl+Alt+G or ghost_stop): Ghost is stopped and refuses actions until ghost_reset` |
| 14:28:21.438 | CLI: `ghost click --name "Advanced view"` | **refused**, exit 1, same message |
| 14:28:21.497 | B (second `ghost-mcp`, started 7 s into the stop): `ghost_act click "Advanced view"` | **refused**, 1 ms, -32004: `ghost_act refused: emergency stop active - Ghost is stopped and refuses actions until ghost_reset` |
| | B: `ghost_window launch ghost-testbed.exe` | **refused**, 0 ms, -32004: `ghost_window refused: emergency stop active - Ghost is stopped and refuses actions until ghost_reset` |
| | B: `ghost_key Enter` | **refused**, 0 ms, -32004: `ghost_key refused: emergency stop active - Ghost is stopped and refuses actions until ghost_reset` |
| | B: `ghost_see fast` (a read) | ok, 17 ms: reads still work while stopped |
| 14:28:24.400 | event read, after the CLI and B | **SIGNALLED (0)** |
| | A: `ghost_reset` (after a 14 s sleep) | ok, 1 ms |
| | A: same `ghost_act click` | ok, 190 ms, `mode: "hidden"` |
| | A: `ghost_see full` | Advanced view now OFF: the checkbox toggled |
| | A: `ghost_window state close` | ok, 42 ms; Character Map gone |
| 14:28:36.728 | event read, after `ghost_reset` | clear (258) |
| | `ghost verify` (extracted `ghost.exe`) | **10/10 passed** in 5.3 s, exit 0, including "emergency stop blocks; reset resumes" |
| 14:29:14.401 | event read, after verify | clear (258) |
| | fresh `ghost-mcp`: launch testbed hidden, `ghost_desktop_close id=auto` | ok 123 ms (`surface: "hidden"`), ok 1 ms (`terminated_pids` = the testbed): the stop is clear |
| 14:29:20.936 | event read, final | clear (258) |

Final state: no `charmap.exe` and no `ghost-testbed.exe` running, and the stop event is clear.

Charmap remembers Advanced view across runs (`HKCU\Software\Microsoft\CharMap\Advanced`). This run
started with it ON, left by the earlier repro, so the reset click turned it OFF. That is the opposite
direction from `reporter-repro.md`, with the same proof: no change while stopped, a toggle after reset.

## Two earlier attempts, kept for honesty

- **Attempt 1.** A ran completely, with the same refusal and toggle-after-reset. The second process,
  the CLI and the event reads did not run: a shell wrapper dropped the helper functions, so `node`
  could not find the driver. Nothing acted while stopped.
- **Attempt 2.** Everything ran, but B's last step (`ghost_see` with no window, which reads whatever
  is in the human's foreground) hit a Chromium window and took 30 s. That pushed the CLI past A's
  reset, so the CLI ran while **not** stopped:
  - `ghost launch` started a testbed on the user desktop (pid 40144). It was closed with WM_CLOSE
    (`CloseMainWindow`), not killed.
  - `ghost click` was refused by the background focus policy, not by the stop.

  The event reads in that attempt matched: SIGNALLED during the stop, clear after it. The final run
  above moves the CLI before B, so all of it falls inside the stop.

## Raw transcript: A, the main session (redacted)

```
>>> ghost_window {"op":"launch","exe":"charmap.exe"}  (449ms)
<<< {"content":[{"text":"{\"data\":{\"desktop\":\"auto\",\"note\":\"started on a hidden desktop (never on your screen) and anchored: ghost_see / ghost_act / ghost_key now target it by default. Drive it by title like any other window; ghost_desktop_close id=auto ends it.\",\"pid\":23980,\"surface\":\"hidden\",\"target\":{\"desktop\":\"auto\",\"hwnd\":494078774,\"pid\":23980,\"source\":\"anchor\",\"surface\":\"hidden\",\"title\":\"Character Map\"},\"window\":{\"hwnd\":494078774,\"pid\":23980,\"title\":\"Character Map\"}},\"foreground\":{\"hwnd\":132208,\"title\":\"<redacted>\"},\"ms\":448,\"ok\":true}","type":"text"}]}

>>> ghost_window {"op":"list","include_hidden":true}  (2ms)
<<< [window list of the operator desktop omitted for privacy; Character Map appears only on surface=hidden, never on the user desktop]

>>> ghost_see {"mode":"full","window":"Character Map","limit":40}  (83ms)
<<< {"content":[{"text":"{\"data\":{\"elements\":[{\"bottom\":431,\"left\":158,\"name\":\"Characters to copy :\",\"right\":323,\"role\":\"edit\",\"top\":405},{\"bottom\":428,\"left\":329,\"name\":\"Select\",\"right\":404,\"role\":\"button\",\"top\":405},{\"bottom\":428,\"left\":410,\"name\":\"Copy\",\"right\":485,\"role\":\"button\",\"top\":405},{\"bottom\":469,\"left\":34,\"name\":\"Advanced view\",\"right\":226,\"role\":\"checkbox\",\"top\":449},{\"bottom\":508,\"left\":158,\"name\":\"Character set :\",\"right\":361,\"role\":\"combobox\",\"top\":478},{\"bottom\":507,\"left\":345,\"name\":\"Open\",\"right\":360,\"role\":\"button\",\"top\":479},{\"bottom\":501,\"left\":446,\"name\":\"Go to Unicode :\",\"right\":484,\"role\":\"edit\",\"top\":478},{\"bottom\":549,\"left\":158,\"name\":\"Group by :\",\"right\":361,\"role\":\"combobox\",\"top\":519},{\"bottom\":548,\"left\":345,\"name\":\"Open\",\"right\":360,\"role\":\"button\",\"top\":520},{\"bottom\":582,\"left\":158,\"name\":\"Search for :\",\"right\":361,\"role\":\"document\",\"top\":559},{\"bottom\":589,\"left\":377,\"name\":\"Search\",\"right\":452,\"role\":\"button\",\"top\":566},{\"bottom\":136,\"left\":86,\"name\":\"Font :\",\"right\":407,\"role\":\"combobox\",\"top\":106},{\"bottom\":135,\"left\":391,\"name\":\"Open\",\"right\":406,\"role\":\"button\",\"top\":107},{\"bottom\":129,\"left\":413,\"name\":\"Help\",\"right\":488,\"role\":\"hyperlink\",\"top\":106},{\"bottom\":163,\"left\":470,\"name\":\"Line up\",\"right\":491,...[truncated]

>>> ghost_stop {}  (1ms)
<<< {"content":[{"text":"{\"data\":{\"ok\":true},\"foreground\":{\"hwnd\":132208,\"title\":\"<redacted>\"},\"ms\":0,\"ok\":true}","type":"text"}]}

>>> ghost_act {"action":"click","name":"Advanced view","role":"checkbox","mode":"instant_only","window":"Character Map"}  (0ms)
<<< {"content":[{"text":"{\n  \"data\": null,\n  \"error\": \"ghost_act refused: emergency stop active - Ghost is stopped and refuses actions until ghost_reset\",\n  \"error_code\": -32004,\n  \"foreground\": {\n    \"hwnd\": 132208,\n    \"title\":\"<redacted>\"\n  },\n  \"ms\": 0,\n  \"ok\": false,\n  \"suggested_action\": \"automation was stopped - call ghost_reset before issuing more actions\"\n}","type":"text"}],"isError":true}

>>> ghost_see {"mode":"full","window":"Character Map","limit":40}  (76ms)
<<< {"content":[{"text":"{\"data\":{\"elements\":[{\"bottom\":431,\"left\":158,\"name\":\"Characters to copy :\",\"right\":323,\"role\":\"edit\",\"top\":405},{\"bottom\":428,\"left\":329,\"name\":\"Select\",\"right\":404,\"role\":\"button\",\"top\":405},{\"bottom\":428,\"left\":410,\"name\":\"Copy\",\"right\":485,\"role\":\"button\",\"top\":405},{\"bottom\":469,\"left\":34,\"name\":\"Advanced view\",\"right\":226,\"role\":\"checkbox\",\"top\":449},{\"bottom\":508,\"left\":158,\"name\":\"Character set :\",\"right\":361,\"role\":\"combobox\",\"top\":478},{\"bottom\":507,\"left\":345,\"name\":\"Open\",\"right\":360,\"role\":\"button\",\"top\":479},{\"bottom\":501,\"left\":446,\"name\":\"Go to Unicode :\",\"right\":484,\"role\":\"edit\",\"top\":478},{\"bottom\":549,\"left\":158,\"name\":\"Group by :\",\"right\":361,\"role\":\"combobox\",\"top\":519},{\"bottom\":548,\"left\":345,\"name\":\"Open\",\"right\":360,\"role\":\"button\",\"top\":520},{\"bottom\":582,\"left\":158,\"name\":\"Search for :\",\"right\":361,\"role\":\"document\",\"top\":559},{\"bottom\":589,\"left\":377,\"name\":\"Search\",\"right\":452,\"role\":\"button\",\"top\":566},{\"bottom\":136,\"left\":86,\"name\":\"Font :\",\"right\":407,\"role\":\"combobox\",\"top\":106},{\"bottom\":135,\"left\":391,\"name\":\"Open\",\"right\":406,\"role\":\"button\",\"top\":107},{\"bottom\":129,\"left\":413,\"name\":\"Help\",\"right\":488,\"role\":\"hyperlink\",\"top\":106},{\"bottom\":163,\"left\":470,\"name\":\"Line up\",\"right\":491,...[truncated]

>>> ghost_reset {}  (1ms)
<<< {"content":[{"text":"{\"data\":{\"ok\":true},\"foreground\":{\"hwnd\":132208,\"title\":\"<redacted>\"},\"ms\":0,\"ok\":true}","type":"text"}]}

>>> ghost_act {"action":"click","name":"Advanced view","role":"checkbox","mode":"instant_only","window":"Character Map"}  (190ms)
<<< {"content":[{"text":"{\"data\":{\"action\":\"click\",\"cursor_preserved\":true,\"focus_preserved\":true,\"mode\":\"hidden\",\"name\":\"Advanced view\",\"note\":\"click dispatched; the window surface could not be compared - read state to confirm\",\"ok\":true,\"rect\":{\"bottom\":469,\"left\":34,\"right\":226,\"top\":449},\"target\":{\"desktop\":\"auto\",\"hwnd\":494078774,\"pid\":23980,\"source\":\"explicit\",\"surface\":\"hidden\",\"title\":\"Character Map\"},\"timing_ms\":{\"action\":113,\"dispatch\":0,\"find\":74},\"verified\":null,\"window\":\"Character Map\"},\"foreground\":{\"hwnd\":132208,\"title\":\"<redacted>\"},\"ms\":189,\"ok\":true}","type":"text"}]}

>>> ghost_see {"mode":"full","window":"Character Map","limit":40}  (43ms)
<<< {"content":[{"text":"{\"data\":{\"elements\":[{\"bottom\":431,\"left\":158,\"name\":\"Characters to copy :\",\"right\":323,\"role\":\"edit\",\"top\":405},{\"bottom\":428,\"left\":329,\"name\":\"Select\",\"right\":404,\"role\":\"button\",\"top\":405},{\"bottom\":428,\"left\":410,\"name\":\"Copy\",\"right\":485,\"role\":\"button\",\"top\":405},{\"bottom\":469,\"left\":34,\"name\":\"Advanced view\",\"right\":226,\"role\":\"checkbox\",\"top\":449},{\"bottom\":136,\"left\":86,\"name\":\"Font :\",\"right\":407,\"role\":\"combobox\",\"top\":106},{\"bottom\":135,\"left\":391,\"name\":\"Open\",\"right\":406,\"role\":\"button\",\"top\":107},{\"bottom\":129,\"left\":413,\"name\":\"Help\",\"right\":488,\"role\":\"hyperlink\",\"top\":106},{\"bottom\":163,\"left\":470,\"name\":\"Line up\",\"right\":491,\"role\":\"button\",\"top\":146},{\"bottom\":380,\"left\":470,\"name\":\"Page down\",\"right\":491,\"role\":\"button\",\"top\":180},{\"bottom\":397,\"left\":470,\"name\":\"Line down\",\"right\":491,\"role\":\"button\",\"top\":380},{\"bottom\":95,\"left\":23,\"name\":\"System\",\"right\":45,\"role\":\"menuitem\",\"top\":73},{\"bottom\":96,\"left\":359,\"name\":\"Minimize\",\"right\":406,\"role\":\"button\",\"top\":66},{\"bottom\":96,\"left\":406,\"name\":\"Maximize\",\"right\":452,\"role\":\"button\",\"top\":66},{\"bottom\":96,\"left\":452,\"name\":\"Close\",\"right\":499,\"role\":\"button\",\"top\":66}],\"target\":{\"desktop\":\"auto\",\"hwnd\":494078774,\"pid\":23980,\"source\":\"explicit\"...[truncated]

>>> ghost_window {"op":"state","name":"Character Map","state":"close"}  (42ms)
<<< {"content":[{"text":"{\"data\":{\"ok\":true},\"foreground\":{\"hwnd\":132208,\"title\":\"<redacted>\"},\"ms\":41,\"ok\":true}","type":"text"}]}

>>> ghost_window {"op":"list","include_hidden":true}  (2ms)
<<< [window list of the operator desktop omitted for privacy; Character Map appears only on surface=hidden, never on the user desktop]

```

## Raw transcript: B, a second ghost-mcp started during the stop (redacted)

```
>>> ghost_act {"action":"click","name":"Advanced view","role":"checkbox","mode":"instant_only","window":"Character Map"}  (1ms)
<<< {"content":[{"text":"{\n  \"data\": null,\n  \"error\": \"ghost_act refused: emergency stop active - Ghost is stopped and refuses actions until ghost_reset\",\n  \"error_code\": -32004,\n  \"foreground\": {\n    \"hwnd\": 132208,\n    \"title\":\"<redacted>\"\n  },\n  \"ms\": 0,\n  \"ok\": false,\n  \"suggested_action\": \"automation was stopped - call ghost_reset before issuing more actions\"\n}","type":"text"}],"isError":true}

>>> ghost_window {"op":"launch","exe":"C:/Users/Krist/projects/active/ghost/target/release/ghost-testbed.exe"}  (0ms)
<<< {"content":[{"text":"{\n  \"data\": null,\n  \"error\": \"ghost_window refused: emergency stop active - Ghost is stopped and refuses actions until ghost_reset\",\n  \"error_code\": -32004,\n  \"foreground\": {\n    \"hwnd\": 132208,\n    \"title\":\"<redacted>\"\n  },\n  \"ms\": 0,\n  \"ok\": false,\n  \"suggested_action\": \"automation was stopped - call ghost_reset before issuing more actions\"\n}","type":"text"}],"isError":true}

>>> ghost_key {"key":"Enter"}  (0ms)
<<< {"content":[{"text":"{\n  \"data\": null,\n  \"error\": \"ghost_key refused: emergency stop active - Ghost is stopped and refuses actions until ghost_reset\",\n  \"error_code\": -32004,\n  \"foreground\": {\n    \"hwnd\": 132208,\n    \"title\":\"<redacted>\"\n  },\n  \"ms\": 0,\n  \"ok\": false,\n  \"suggested_action\": \"automation was stopped - call ghost_reset before issuing more actions\"\n}","type":"text"}],"isError":true}

>>> ghost_see {"mode":"fast","limit":3}  (17ms)
<<< [ok: a read of the human's foreground window (no window anchored in this process); contents omitted for privacy]

```

## CLI during the stop (third process)

```
14:28:21.379 CLI launch (third process)
ghost: emergency stop active (Ctrl+Alt+G or ghost_stop): Ghost is stopped and refuses actions until ghost_reset
cli launch exit 1
14:28:21.438 CLI click
ghost: emergency stop active (Ctrl+Alt+G or ghost_stop): Ghost is stopped and refuses actions until ghost_reset
cli click exit 1
```

## Event readings

```
14:28:13.899 [before A] Local\ghost-emergency-stop-event : clear (WaitForSingleObject(h,0) = 258)
14:28:14.037 A start (main ghost-mcp)
14:28:21.250 [during stop, before CLI and second process] Local\ghost-emergency-stop-event : SIGNALLED (stop in force) (WaitForSingleObject(h,0) = 0)
14:28:21.497 B start (second ghost-mcp)
14:28:23.199 B done
14:28:24.400 [after CLI and second process] Local\ghost-emergency-stop-event : SIGNALLED (stop in force) (WaitForSingleObject(h,0) = 0)
14:28:35.709 A done
14:28:36.728 [after ghost_reset] Local\ghost-emergency-stop-event : clear (WaitForSingleObject(h,0) = 258)
14:29:14.401 [after verify] Local\ghost-emergency-stop-event : clear (WaitForSingleObject(h,0) = 258)
14:29:20.936 [final] Local\ghost-emergency-stop-event : clear (WaitForSingleObject(h,0) = 258)
```

## ghost verify (extracted ghost.exe)

```
ghost verify - proving the product claims on this machine

  [PASS] server starts and answers initialize           24ms
  [PASS] default focus policy is 'background'           Some("background")
  [PASS] isolated browser launches (budget 10s)         396ms
  [PASS] 3 tabs typed+clicked, no cross-talk (5s)       405ms for all three
  [PASS] background click latency (150ms each)          1.6ms/click
  [PASS] background tab screenshot (2s)                 33ms
  [PASS] fast call completes while slow call in flight (500ms) 0.5ms with a 2s call running
  [PASS] second ghost server ran its own browser (20s)  0.5s alongside the first
  [PASS] emergency stop blocks; reset resumes           stop observed, reset restored service
  [PASS] foreground never taken by Ghost                hwnd 132208 -> 132208

ALL CLAIMS HOLD: 10 checks passed in 5.3s. Ghost operates as advertised on this machine.
verify exit=0
```

## After: fresh process launches and closes the testbed hidden (redacted)

```
>>> ghost_window {"op":"launch","exe":"C:/Users/Krist/projects/active/ghost/target/release/ghost-testbed.exe"}  (123ms)
<<< {"content":[{"text":"{\"data\":{\"desktop\":\"auto\",\"note\":\"started on a hidden desktop (never on your screen) and anchored: ghost_see / ghost_act / ghost_key now target it by default. Drive it by title like any other window; ghost_desktop_close id=auto ends it.\",\"pid\":31988,\"surface\":\"hidden\",\"target\":{\"desktop\":\"auto\",\"hwnd\":108530050,\"pid\":31988,\"source\":\"anchor\",\"surface\":\"hidden\",\"title\":\"Ghost Testbed\"},\"window\":{\"hwnd\":108530050,\"pid\":31988,\"title\":\"Ghost Testbed\"}},\"foreground\":{\"hwnd\":132208,\"title\":\"<redacted>\"},\"ms\":122,\"ok\":true}","type":"text"}]}

>>> ghost_desktop_close {"id":"auto"}  (1ms)
<<< {"content":[{"text":"{\"data\":{\"ok\":true,\"terminated_pids\":[31988]},\"foreground\":{\"hwnd\":132208,\"title\":\"<redacted>\"},\"ms\":0,\"ok\":true}","type":"text"}]}

```
