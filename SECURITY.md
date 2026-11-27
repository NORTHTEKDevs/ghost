# Security policy

## Reporting a vulnerability

Email **info@northtek.io** with "Ghost security" in the subject. Please do not
open a public issue for a vulnerability.

Include what you found, how to reproduce it, and what an attacker gets. You will
get an acknowledgement within 72 hours. There is no bounty program.

## What Ghost is, so you can judge the boundary

Ghost drives the desktop of the machine it runs on. It types, clicks, reads
window contents, captures the screen, and runs shell commands. That is the
product, not a flaw: **anything that can reach a Ghost server can control that
desktop as the user running it.**

So the security boundary is the transport, not the verbs:

- `ghost-mcp` speaks JSON-RPC over **stdio** only. It has no network listener.
  Whoever spawns the process is already trusted with that user's session.
- `ghost-http` binds **127.0.0.1:7878 by default** and has **no
  authentication**. It refuses to start on a non-loopback `--addr`, answers 403
  to any request whose Host is not its own loopback name and port (DNS
  rebinding) or that carries an Origin header (browser CSRF), and `/run` only
  takes `application/json`. It is a local convenience for scripts, not a
  service: any local process can still call it.
- `ghost_shell` runs arbitrary commands. `GHOST_SHELL=off` disables that verb
  entirely, and the launch verbs then refuse command interpreters, terminals,
  script hosts and proxy launchers (wmic, explorer, schtasks, xdg-open, env,
  ...), judged by the program's resolved long name / symlink target. That is a
  denylist and cannot be complete; `GHOST_LAUNCH_ALLOW` (comma-separated
  basenames or full paths) is the strict option: with the shell off, only the
  listed programs launch. Neither is a sandbox: keyboard input and clicks can
  still start programs, so it does not contain an agent you do not trust.

Reports that amount to "an MCP client can make Ghost click things" describe the
intended behaviour. Reports about **privilege boundaries being crossed** are in
scope, for example: another local user reading Ghost's credentials, a driven
application escaping into the Ghost process, output from a driven application
being interpreted as protocol, or a listener appearing where the docs promise
none.

## Things worth knowing if you are auditing

- The Wayland portal restore token is a bearer credential: presenting it
  resumes remote-desktop control with no consent prompt. It is stored `0600`
  inside a `0700` directory, re-asserted on every write.
- `ghost_shell` frames commands as base64 and terminates them with a sentinel
  carrying an unguessable per-session secret, so output from a driven command
  cannot forge a completion and desynchronise the session.
- Emergency stop: **Ctrl+Alt+G** halts automation on Windows and on X11. Wayland
  does not permit clients to grab keys globally, so there the stop path is the
  `ghost_stop` verb.
- On Linux the shared stop is marker files in `$XDG_RUNTIME_DIR/ghost/`, or
  `/tmp/ghost-stop-<uid>/` when `XDG_RUNTIME_DIR` is unset. A marker directory
  that is not this user's private directory fails closed (every Ghost process
  of that user counts as stopped). On a multi-user host without
  `XDG_RUNTIME_DIR`, another local user can therefore pre-create that `/tmp`
  path and hold the stop up: a denial of service, accepted by design rather
  than ever ignoring a stop. Run Ghost in a login session (which sets
  `XDG_RUNTIME_DIR`) to avoid it.

## Supported versions

The latest release only. Ghost is pre-1.0 and moves quickly; fixes ship forward
rather than being backported.
