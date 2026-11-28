# Publishing Ghost to the MCP registry

Getting Ghost into the [Model Context Protocol registry](https://registry.modelcontextprotocol.io)
(and the client directories that mirror it) is the discovery lever - it's how an
agent or user finds Ghost without already knowing it exists.

## What's in the repo

- [`server.json`](../server.json) - the registry manifest (name, title, description,
  repository, version, website).
- [`mcpb/manifest.template.json`](../mcpb/manifest.template.json) - the MCP Bundle
  manifest; the release workflow fills in version, entry point and platform and packs
  it with the binary and `assets/icon.png`.
- [`Dockerfile`](../Dockerfile) - headless image for registries that build and
  introspect servers (Glama). Downloads the release matching the workspace version
  and verifies its sha256; not an install path for people.
- [`plugin.json`](../plugin.json) + [`mcp.json`](../mcp.json) - Agent Plugins 1.0.0
  manifests (cursor.directory and other plugin scanners).
- [`glama.json`](../glama.json) (Glama maintainers) and
  [`lhm.plugin.json`](../lhm.plugin.json) (LobeHub identity).
- [`assets/`](../assets) - `icon.png` (bundles, directories), `ghost.ico` (embedded
  in the Windows executables by `build.rs`), `social-preview.png` (GitHub's
  repository social preview; uploaded by hand in repository settings).

Version lives in four places by hand: `Cargo.toml` `[workspace.package]`,
`server.json`, `plugin.json`, `lhm.plugin.json`. Bump all four in the release commit.

The committed file is **listing-only** (name, description, repository, version);
the installable part is added at publish time. The registry's package types are
`npm`, `pypi`, `oci`, `nuget` and `mcpb` - there is no type for "binaries on a
GitHub release", and a manifest that once said `"registryType": "github"` was
rejected by `mcp-publisher validate` for exactly that reason. Ghost ships
`mcpb`: the release workflow packs `ghost-windows-x64.mcpb` and
`ghost-linux-x86_64.mcpb` (manifest from `mcpb/manifest.template.json` + the
`ghost-mcp` binary, validated with `@anthropic-ai/mcpb` before packing), and the
registry workflow reads their checksums off the published release and adds two
`mcpb` packages pinned by `fileSha256` before it publishes. A bundle's checksum
is only known after the release builds it, which is why the packages are not in
the committed file.

## How it publishes

`.github/workflows/registry.yml` publishes on every `v*` tag, after the GitHub
Release for that tag exists. It authenticates with GitHub OIDC (the registry
trusts a token minted by the workflow for the `io.github.NORTHTEKDevs/*`
namespace), so there is no human login and no stored secret. It pins
`mcp-publisher` by version and checksum, validates the manifest against the live
schema, rewrites `server.json`'s `version` from the tag so the two cannot drift,
publishes, and then queries the registry to confirm the entry is there.

To re-publish without a new tag (a fix to the manifest itself, or a tag cut
before the workflow existed): Actions -> Registry -> Run workflow, or
`gh workflow run registry.yml`. That path publishes the version currently in
`server.json` and still waits for its release to exist.

Why a workflow and not the CLI by hand: the manual path is a GitHub device-code
login that expires in minutes and had to be approved in a browser signed in as
NORTHTEKDevs. It expired unused twice on 2026-09-04, and before that the
manifest had never been valid, so Ghost had never been listed at all.

## Manual fallback

1. `mcp-publisher validate` - fix anything it names. `description` is capped
   at 100 characters; the schema version must be current.
2. `mcp-publisher login github` (device code, browser signed in as
   NORTHTEKDevs).
3. `mcp-publisher publish`, after the matching GitHub release exists.

## Directories beyond the official registry

Status as of 2026-09-05. "Automatic" means the directory reads the official
registry or GitHub and needs nothing from us; "done" means we submitted;
"needs the account owner" means the directory requires a login that grants a
third party access to an account, which an automated session must not do.

| Directory | Mechanism | Status |
| --- | --- | --- |
| Official MCP registry | `.github/workflows/registry.yml`, OIDC, every tag | listed, installable (mcpb) |
| PulseMCP | imports the official registry; own submissions paused | automatic |
| awesome-mcp-servers (punkpeye) | pull request, OS Automation section | PR #13724 open, mergeable |
| mcpservers.org | web form, no login | submitted; review within 12 hours, confirmation to info@northtek.io |
| Glama | indexes GitHub; `glama.json` (in repo) names the maintainer; claimed with the maintainer account. Name, description and categories live under Admin > Profile (Glama otherwise copies the GitHub About text). The tools list, coherence and tool-quality scores and the security scan all require a **Glama release**: Admin > Dockerfile, fill the build spec, click Build & Release. The spec that is saved there: base image `debian:trixie-slim`; build steps download `releases/latest/download/ghost-linux-x86_64.tar.gz`, untar to `/tmp/ghost`, `install -m 0755 /tmp/ghost/ghost-mcp /usr/local/bin/ghost-mcp`; CMD arguments `["ghost-mcp"]` (Glama prepends `mcp-proxy --`); placeholder parameters `{"GHOST_SHELL": "off"}`. The repo `Dockerfile` is not what Glama builds; it is the same recipe for anyone else and for verifying the build locally | listed: https://glama.ai/mcp/servers/hcu78gk1x6; Score tab: Maintenance A, MIT, README, glama.json, author verified all green; Glama releases: 0.23.4 (2026-09-08) and "0.23.5" (2026-11-27, Build & Release at commit 88464a9; Glama numbers its own releases by incrementing the last one, the binary inside is whatever `releases/latest` is, 0.24.5 that day). Admin > Repository > Sync Server refreshes the commit first. Admin > Releases "Auto-release on new GitHub release" is ON: Glama publishes each GitHub release under its tag (0.24.5 at 15:05 and 0.24.6 at 16:06 on 2026-11-27, after a Repository > Sync Server). **Do not also click Build & Release after a GitHub release**: a manual build takes the next number in Glama's own counter, which is how a mislabelled "0.24.7" (the 0.24.6 binary) became Latest on 2026-11-27; Glama has no control to delete a release. If a release does not appear, Sync Server and wait. Still open: "no recent usage" (use Try in Browser once) and "no related servers" (Related Servers tab) |
| Smithery | `smithery mcp publish ghost-windows-x64.mcpb -n info-j5od/ghost` from the bundle directory after `smithery login` (the account namespace is `info-j5od`, not NORTHTEKDevs); re-run per release. Display name, description, homepage, repository link and icon are NOT read from the bundle: set them once under Settings > General on the server page (the CLI has no flags for them) | **Smithery shut down publishing on 2026-10-31** (accounts, API keys, the CLI and its proxy); it is now a curated read-only directory run by Arcade.dev. registry.smithery.ai still returns `info-j5od/ghost` at 0.23.4, but its web page 404s and `smithery mcp publish` answers 403 "Updating servers has been removed". The only contact named is the cofounder; a drafted, unsent request is in `docs/plans/2026-11-27-leftovers.md` |
| LobeHub marketplace | first time: `npx -y @lobehub/market-cli login`, `github connect`, then `plugin publish https://github.com/NORTHTEKDevs/ghost` from the repo root; every release after that: bump `lhm.plugin.json` and run `npx -y @lobehub/market-cli plugin update` from the repo root (`publish` refuses an existing identifier) | listed: https://lobehub.com/mcp/northtekdevs-ghost (northtekdevs-ghost@0.24.6) |
| mcp.so | website submission behind Sign In; the only submit path is a paid $39 "Pay and submit automatically" | declined: paid listing only, we do not pay for directory placement |
| cursor.directory | cursor.directory/plugins/new scans the repo for Agent Plugins files (agent-plugins.org): root `plugin.json` + `mcp.json` (both in repo, both validate against the 1.0.0 schemas); no sign-in was required to submit | listed: https://cursor.directory/plugins/ghost (scan passed; name, homepage, keywords and logo set from Plugin options > Edit) |

Every directory that accepts submissions now has one. Two things the
directories mirror live on GitHub itself and were brought in line with the
0.21.7 framing: the repository description and topics (`gh repo edit`), which
Glama, mcpservers.org and LobeHub had copied verbatim while stale. The
repository social preview image (`assets/social-preview.png`) has no API and is
uploaded by hand under Settings > Social preview. The metadata files they
read (`glama.json`, `lhm.plugin.json`, `plugin.json`, `mcp.json`, the `.mcpb`
bundles) live in the repo; `plugin.json` and `lhm.plugin.json` carry their own
`version` field and are bumped by hand with the workspace version. `mcp.json`
names the bare `ghost-mcp` command (PATH lookup), so a plugin install from the
repo still needs the binary from the Releases page.

## Client config (copy-paste)

```json
{
  "mcpServers": {
    "ghost": { "command": "C:/path/to/ghost-mcp.exe" }
  }
}
```

Works with any MCP client. The server's `tools/list` response is the full tool list.

## Keeping the installed binary current (auto-update)

New Claude sessions launch `~/.local/bin/ghost-mcp.exe` (a stable path outside the
build folder, so it survives `cargo clean` and repo moves). Two things keep it on
the latest build:

- **`scripts/install.ps1`** - `cargo build --release -p ghost-mcp` then install to
  the stable path. Run it to publish a new version immediately.
- **`GhostMcpAutoSync`** scheduled task (hourly) - copies the newest release build
  to the stable path automatically. Because Windows locks a running `.exe`, the
  sync renames the in-use binary aside and drops the fresh one in, so it updates
  even while sessions are live; new sessions pick it up, running sessions keep
  their copy, and the renamed leftovers self-clean once their process exits.

After any `cargo build --release -p ghost-mcp`, the installed binary refreshes
within the hour (or instantly via `scripts/install.ps1`).
