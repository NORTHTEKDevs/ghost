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
  authentication**. `--addr` will happily bind anything you tell it to, so do
  not point it at `0.0.0.0` or expose it to a network. It is a local
  convenience for scripts, not a service.
- `ghost_shell` runs arbitrary commands. `GHOST_SHELL=off` disables that verb
  entirely if you are handing a Ghost session to something you do not fully
  trust.

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

## Supported versions

The latest release only. Ghost is pre-1.0 and moves quickly; fixes ship forward
rather than being backported.
