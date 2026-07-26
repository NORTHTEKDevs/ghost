# Ghost — Quick Start

Three binaries, no toolchain. You should be driving an app inside two minutes.

- `ghost-mcp.exe` — the MCP server. This is the one Claude Code / Claude Desktop talks to.
- `ghost.exe` — the CLI, for driving apps from a terminal or a script.
- `ghost-http.exe` — a local HTTP server, if you would rather call Ghost over REST.

Requires Windows 10 (build 19041) or newer. Nothing else.

---

## 1. Unblock the download

These binaries are **not code-signed**, so Windows will show a blue
"Windows protected your PC" box the first time you run one.

> **SmartScreen:** click **More info**, then **Run anyway**.

If you downloaded the zip with a browser, Windows may also mark the extracted
files as blocked. Clear that in one command:

```powershell
Get-ChildItem -Path . -Recurse | Unblock-File
```

We would rather tell you this plainly than have you wonder. A code-signing
certificate is on the roadmap. If you would rather not trust a binary at all,
the full source is MIT-licensed at https://github.com/NORTHTEKDevs/ghost and
builds with `cargo build --release`.

## 2. Install

```powershell
powershell -ExecutionPolicy Bypass -File install.ps1
```

That copies the binaries to `%LOCALAPPDATA%\Programs\ghost`, clears the
downloaded-file block flag, adds them to PATH, writes the MCP config for Claude
Code and Claude Desktop (merging into your existing config, with a backup, never
overwriting it), and finishes by running `ghost doctor`.

It only stops Ghost processes running from the folder it is installing into, so
it will not disturb an agent or automation you already have running elsewhere.

Prefer to do it by hand? `-SkipPath` and `-SkipClaude` turn those steps off, and
section 4 below has the manual equivalents.

## 3. Check the machine

```powershell
ghost doctor
```

Every line should read PASS or WARN. A WARN on vision credentials is expected -
vision is an optional fallback and everything else works without it. If anything
reads FAIL, send us that output and we will tell you exactly what is wrong.

## 4. Wiring Claude by hand (only if you skipped it)

- **Claude Code** - `claude mcp add ghost --scope user -- "%LOCALAPPDATA%\Programs\ghost\ghost-mcp.exe"`
- **Claude Desktop** - merge the `mcpServers` block from `mcp-config.json` into
  `%APPDATA%\Claude\claude_desktop_config.json`, replacing the placeholder path.

Restart Claude and the `ghost_*` tools appear.

## 5. Prove it works

Ask Claude:

> Use ghost to open Notepad and type "hello from ghost", then screenshot it.

Or from a terminal:

```powershell
ghost launch notepad.exe
ghost focus-window "Notepad"
ghost type --role edit --text "hello from ghost"
ghost screenshot --out proof.png
```

---

## What Ghost is actually for

It drives Windows applications the way a person does — clicking, typing,
reading the screen — including apps that expose no API at all. The part worth
paying attention to is that it can do this **in the background**, using posted
window messages, so it does not steal your foreground window or your cursor
while it works. You can keep using your machine.

## Where to start

`recipes/` has two working starting points for the jobs that make up most real
automation work:

- **`01_batch_data_entry.py`** - reads a CSV, types each row into an app, reads
  it back, and halts on the first row that does not land. That verify-and-halt
  loop is the part worth copying: a run that stops on row 40 costs you ten
  minutes, a run that silently skips it costs you a reconciliation.
- **`02_extract_to_csv.py`** - dumps every element a window exposes to CSV. Use
  it to pull data out of software with no export, and to find reliable
  `name`+`role` selectors instead of clicking at coordinates.

Both run against Notepad as-is, so you can watch them work before pointing them
at anything that matters. `recipes/README.md` explains how to adapt them.

`examples/` has the lower-level demos, and `docs/` in the repo has the full verb
reference.

## Known limits, stated up front

- **Windows only.** The macOS and Linux backends are scaffolds, not working
  engines. Do not buy this for a Mac.
- **Background dispatch needs a real window handle.** Windowless UWP/WinUI and
  Chromium content have no HWND, so those fall back to normal UIA and *will*
  take focus. Ghost tells you which path it used rather than pretending.
- **Browsers are driven as a user, not at the DOM level.** For deep web
  scraping, Playwright is the better tool. Ghost is for the other 90% of your
  desktop.
- **Multi-monitor** click coordinates are normalised across the virtual
  desktop as of v0.16.1. If you are on an older build, upgrade.

## Support

Issues and questions: https://github.com/NORTHTEKDevs/ghost/issues
Email: info@northtek.io
