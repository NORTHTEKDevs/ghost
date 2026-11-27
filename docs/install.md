# Install

Three ways in, all free: the MCP Bundle (one click, the server only), the
release archives (server, CLI and HTTP server), or building from source.
Then register the server with your client: the
[README quick start](../README.md#quick-start) has a copy-paste config for each
client.

Requirements: Windows 10 build 19041 or later, or Linux with `at-spi2-core` and
a desktop session (X11 or Wayland). A Chromium-family browser (Chrome, Comet,
Edge or Brave) only for the `ghost_browser_*` and `ghost_tab_*` tools.

## MCP Bundle

Every release ships `ghost-windows-x64.mcpb` and `ghost-linux-x86_64.mcpb` on
the [Releases page](https://github.com/NORTHTEKDevs/ghost/releases/latest).
Open one in a client that supports MCP Bundles (Claude Desktop: *Settings ->
Extensions -> Install from file*). The install dialog shows the two consent
settings ([configuration](configuration.md#the-two-consent-settings)) as
checkboxes. The bundle holds `ghost-mcp` only.

Ghost is also in the [MCP registry](https://registry.modelcontextprotocol.io)
as `io.github.NORTHTEKDevs/ghost` for clients that install from there.

## Windows archive

```powershell
$dir = "$env:LOCALAPPDATA\Programs\ghost"
$url = "https://github.com/NORTHTEKDevs/ghost/releases/latest/download"
Invoke-WebRequest "$url/ghost-windows-x64.zip" -OutFile ghost-windows-x64.zip
Invoke-WebRequest "$url/ghost-windows-x64.zip.sha256" -OutFile ghost-windows-x64.zip.sha256
(Get-FileHash ghost-windows-x64.zip -Algorithm SHA256).Hash.ToLower()
Get-Content ghost-windows-x64.zip.sha256     # the two hashes must match
Expand-Archive ghost-windows-x64.zip -DestinationPath $dir -Force
Get-ChildItem $dir | Unblock-File
& "$dir\ghost.exe" doctor
```

The server is then `%LOCALAPPDATA%\Programs\ghost\ghost-mcp.exe`. Add `$dir` to
your `PATH` if you want `ghost` and `ghost-http` on the command line.

The binaries are **not code-signed yet**, so SmartScreen warns on first run
(*More info -> Run anyway*). That warning is about publisher identity, which
needs a paid certificate; the provenance attestation below is the stronger
statement about where a file came from. See [code signing](code-signing.md)
and [signing policy](signing-policy.md). If an antivirus engine quarantines a
release, check the hash and see [antivirus](antivirus.md).

## Linux archive

```bash
curl -LO https://github.com/NORTHTEKDevs/ghost/releases/latest/download/ghost-linux-x86_64.tar.gz
curl -LO https://github.com/NORTHTEKDevs/ghost/releases/latest/download/ghost-linux-x86_64.tar.gz.sha256
sha256sum -c ghost-linux-x86_64.tar.gz.sha256
mkdir ghost && tar -xzf ghost-linux-x86_64.tar.gz -C ghost
mkdir -p ~/.local/bin && install -m 0755 ghost/ghost ghost/ghost-mcp ghost/ghost-http ~/.local/bin/
~/.local/bin/ghost doctor
```

The server is then `~/.local/bin/ghost-mcp`.

Ghost reads applications through the accessibility bus, which most desktops
leave off:

```bash
sudo dnf install at-spi2-core xdg-desktop-portal xdg-desktop-portal-gnome   # or apt install
gsettings set org.gnome.desktop.interface toolkit-accessibility true
```

Restart applications that were already open; they publish an accessible tree
only at startup. The Linux engine is pure Rust, so no `-devel` packages are
needed. Full setup, verification and known limits:
[Linux setup](linux-fedora.md).

## Check where a download came from

Every release artifact carries a signed build provenance attestation, proving it
came out of this repository's release workflow at a named commit:

```bash
gh attestation verify ghost-windows-x64.zip --repo NORTHTEKDevs/ghost
```

## Build from source

```bash
git clone https://github.com/NORTHTEKDevs/ghost
cd ghost
cargo build --release --bin ghost --bin ghost-mcp --bin ghost-http
# binaries in target/release/
```

Needs Rust stable. On Linux, `./scripts/install.sh` builds, installs to
`~/.local/bin`, registers the server and runs `ghost doctor`.

## Check the machine

```bash
ghost doctor
```

PASS, WARN or FAIL per check, exit code 1 on any FAIL. It usually names the
problem outright, so run it before opening an issue.

- **Windows:** build version, interactive desktop, UI Automation, DPI awareness,
  monitor layout, screen capture, optional vision settings.
- **Linux:** session type, accessibility bus, whether applications expose
  accessible trees, the input backend, screen capture.

Then `ghost verify` starts the `ghost-mcp` next to it, drives it over stdio the
way a client would, and checks the background, stop and verification claims
against your hardware.

## Uninstall

Remove the server from your client (`claude mcp remove ghost`, or delete the
`ghost` entry from the client's config, or remove the extension in Claude
Desktop), then delete the folder you installed into
(`%LOCALAPPDATA%\Programs\ghost` or the binaries in `~/.local/bin`). Ghost
installs no services, scheduled tasks or registry entries.
