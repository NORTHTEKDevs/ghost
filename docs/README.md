# Ghost documentation

## Using Ghost

| Page | What it covers |
| --- | --- |
| [Install](install.md) | MCP Bundle, release archives, checksums and provenance, building from source, `ghost doctor`, uninstall |
| [Using Ghost](usage.md) | Window anchoring, what every response promises, shell, CLI, intents, HTTP API, Rust SDK, architecture |
| [Configuration](configuration.md) | The two consent settings, every environment variable, the optional vision tier, the HTTP server |
| [Security model](security-model.md) | Who can reach Ghost, what an agent may do, emergency stop, human-only resume, stop scopes, SSRF guard |
| [Background mode](background-mode.md) | How Ghost drives apps without taking your screen, hidden desktops, and what was measured |
| [Tool reference](tools.md) | Every MCP tool and parameter, generated from the server and checked by a test |
| [Linux setup](linux-fedora.md) | Accessibility bus, X11 and Wayland, verification checklist, known limits |
| [Platforms](cross-platform.md) | Capability matrix for Windows, Linux and macOS |
| [Comparison](comparison.md) | Where Ghost fits next to Playwright-MCP, cua-driver and Computer Use |
| [Benchmarks](benchmarks.md) | Task-success benchmark, reliability soak, per-call speed |
| [Antivirus and SmartScreen](antivirus.md) | Why heuristics notice Ghost, and how to report a false positive |
| [Code signing](code-signing.md) and [signing policy](signing-policy.md) | What is signed, by whom, and what you can check yourself |

## Maintainer notes

| Page | What it covers |
| --- | --- |
| [Publishing to the MCP registry](publishing-mcp.md) | How the registry entry and listings are kept current |
| [Building on macOS](macos-build.md) | The macOS scaffold and its implementation map |
| [SignPath application](signpath-application.md) | Prepared answers for the free code-signing program |

## Engineering record (internal)

These folders are the project's working record, kept public for transparency.
They are not user documentation and may describe behaviour that has since
changed; the [CHANGELOG](../CHANGELOG.md) is the authority on what shipped.

| Folder | Contents |
| --- | --- |
| [plans/](plans/README.md) | Design notes and implementation plans, by date |
| [benches/](benches/README.md) | Raw measurements and test logs behind each release |
| [audits/](audits/README.md) | Findings from code and security audits |
| [research/](research/README.md) | Background research on other tools |
