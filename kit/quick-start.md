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

## 2. Put the binaries somewhere permanent

Anywhere is fine, but **not** your Downloads folder — the MCP config below
points at this path, and moving the files later will break it.

```powershell
mkdir "$env:LOCALAPPDATA\Programs\ghost"
Copy-Item .\*.exe "$env:LOCALAPPDATA\Programs\ghost\"
```

## 3. Check your machine

```powershell
& "$env:LOCALAPPDATA\Programs\ghost\ghost.exe" doctor
```

Every line should read PASS or WARN. A WARN on vision credentials is expected
and harmless — vision is an optional fallback, and everything else works
without it.

## 4. Wire it into Claude

Open `mcp-config.json` from this kit, replace `REPLACE_WITH_YOUR_PATH` with the
folder from step 2, and merge it into your Claude config:

- **Claude Code** — `claude mcp add ghost --scope user -- "%LOCALAPPDATA%\Programs\ghost\ghost-mcp.exe"`
- **Claude Desktop** — paste the `mcpServers` block into
  `%APPDATA%\Claude\claude_desktop_config.json`

Restart Claude. You should see the `ghost_*` tools available.

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

Read `examples/` for working scripts, and `docs/` in the repo for the full
verb reference.

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
