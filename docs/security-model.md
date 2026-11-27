# Security model

Ghost drives the desktop of the machine it runs on: it types, clicks, reads
windows, captures the screen and runs shell commands. That is the product.
**Anything that can reach a Ghost server can control that desktop as the user
running it**, so the questions that matter are who can reach it, what an agent
is allowed to do through it, and how a human takes control back.

To report a vulnerability, see [SECURITY.md](../SECURITY.md).

## Who can reach Ghost

| Surface | Transport | Who can call it |
| --- | --- | --- |
| `ghost-mcp` | JSON-RPC over **stdio** only. No network listener. | Only the process that started it, which is already trusted with your session. |
| `ghost-http` | HTTP on **127.0.0.1:7878** by default. No authentication. | Any local process. It refuses to start on a non-loopback `--addr`, answers 403 to a request whose Host is not its own loopback name and port (DNS rebinding) or that carries an `Origin` header (a web page), and `/run` only accepts `application/json`. |
| `ghost` CLI | Runs once and exits. | Whoever runs it. |

Nothing leaves the machine unless you configure the optional vision tier
([configuration](configuration.md#optional-vision-tier)), which sends
screenshots to the endpoint you name. Ambient `OPENAI_API_KEY`,
`ANTHROPIC_API_KEY` and `NVIDIA_API_KEY` are ignored for that reason.

## What an agent may do

### Your mouse and keyboard stay yours

The focus policy starts at `background` and is **locked** there. Every verb
drives windows through UI Automation patterns, posted window messages or the
browser's DevTools protocol, so it never needs your cursor or foreground
window. A call with no background route fails with an error naming the action;
it never quietly takes the screen. An agent cannot raise the policy: that call
is refused. Only the operator can unlock it, by setting `GHOST_FOCUS_LOCK=off`
in the server's environment. How this works, and how well, is in
[background mode](background-mode.md).

### Shell access is on by default, and you can turn it off

`ghost_shell` runs programs with your account's rights. That is why most people
install Ghost, so it ships enabled and says so. To remove it, set
`GHOST_SHELL=off`:

- every `ghost_shell` call returns a refusal;
- the launch verbs (`ghost_launch`, `ghost_window op=launch`,
  `ghost_desktop_launch`) refuse command interpreters, terminals, script hosts and
  programs that start other programs (cmd, PowerShell, wscript, python, wmic,
  explorer, xdg-open, env, ...), judged by the program's resolved long name or
  symlink target, so 8.3 short names do not slip through;
- that filter is a denylist and cannot be complete. For a strict rule, set
  `GHOST_LAUNCH_ALLOW` to a comma-separated list of basenames or full paths
  (`notepad,C:\Tools\viewer.exe`): with the shell off, only those launch.

Neither is a sandbox. Typing and clicking can still start programs, so this
does not contain an agent you do not trust.

### Web requests cannot reach your network

`ghost_http_get` and `ghost_http_post` resolve the host first and refuse
loopback, private, link-local and reserved addresses (including 100.64/10,
0/8, 198.18/15, multicast, fec0::/10, IPv4-compatible, NAT64, 6to4 and
Teredo forms), refuse a caller-supplied `Host` header, and ignore
`HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` because a proxy would resolve the name
itself. `GHOST_HTTP_ALLOW_PRIVATE=1` lifts the address check for local
development.

Browser tabs are different: `ghost_tab_open` and `ghost_tab_navigate` drive a
real browser, so http and https to any host, localhost included, are allowed
(local dev servers are a core use). Every other scheme (`file:`, `chrome:`,
`data:`, `javascript:`, `ftp:` ...) is refused.

## How a human takes control back

### Emergency stop: Ctrl+Alt+G

Press **Ctrl+Alt+G** at any time. Every acting call in every Ghost process in
your logon session halts, queued actions are cancelled and held modifier keys
are released. Read-only calls (`ghost_see`, `ghost_snapshot`, `ghost_window
op=list`) keep answering so you can inspect what happened.

- Programs started by `ghost_shell` end with the stop, including whatever a
  command launched. Your stop also reaches programs started by a Ghost server
  that has since restarted, for up to 24 hours within the same boot.
- While stopped, Ghost may still close what it created itself (tabs and
  browsers it launched, its own hidden desktops). It never closes your windows.
- A Ghost process that cannot read the shared stop fails closed: it refuses
  every action and says why.

On Windows the stop is the named event `Local\ghost-emergency-stop-event`. On
Linux it is a marker file in `$XDG_RUNTIME_DIR/ghost/` (or
`/tmp/ghost-stop-<uid>/` when that is unset); the directory must be yours and
mode 0700, or every Ghost process counts as stopped. Wayland does not let
clients grab keys globally, so there the stop is the `ghost_stop` verb.

### Only you can resume

A stop you raised is marked as yours. An agent's `ghost_reset` cannot lift it;
the agent is told you stopped Ghost and to wait. You resume with either:

- **Ctrl+Alt+Shift+G** pressed on the keyboard (an injected key is ignored), or
- **`ghost reset`** in a terminal you type into: it shows a short word and you
  type it back (a pipe or a script is refused).

A stop an agent raised with `ghost_stop` is lifted by its own `ghost_reset`.

### Stop scopes

`ghost_stop` and `ghost_reset` take `scope`:

| Scope | `ghost_stop` | `ghost_reset` |
| --- | --- | --- |
| `everywhere` (default) | every Ghost process in this logon session | clears an agent-raised stop for every process; never a human stop |
| `process` | only this server | only this server's own flag; a stop raised elsewhere stays |

Any other value is invalid params. (Before 0.24.3 an unknown value meant
"everywhere".)

## Limits worth knowing

- A program running as your user can change the named stop event directly. The
  human-only resume protects you from an agent acting through Ghost, not from
  arbitrary code you let it run.
- On Linux, a multi-user host without `XDG_RUNTIME_DIR` lets another local user
  pre-create the `/tmp` marker path and hold the stop up. That is a denial of
  service, accepted by design rather than ever ignoring a stop. Run Ghost in a
  login session to avoid it.
- The Wayland portal restore token is a bearer credential (presenting it resumes
  remote-desktop control with no prompt). It is stored `0600` in a `0700`
  directory.
- `ghost_shell` frames commands as base64 and ends them with a per-session
  secret sentinel, so a driven command's output cannot forge a completion.

## What you can verify yourself

- `ghost verify` drives the real MCP server and fails if the background, stop
  or verification claims above do not hold on your machine.
- `ghost_stats.interference_audit` records every foreground change with no real
  hardware input behind it, and the tool calls in flight at the time.
- Release artifacts carry signed build provenance:
  `gh attestation verify <file> --repo NORTHTEKDevs/ghost`. See
  [signing policy](signing-policy.md).
