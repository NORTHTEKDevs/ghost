# Publishing Ghost to the MCP registry

Getting Ghost into the [Model Context Protocol registry](https://registry.modelcontextprotocol.io)
(and the client directories that mirror it) is the discovery lever — it's how an
agent or user finds Ghost without already knowing it exists.

## What's in the repo

- [`server.json`](../server.json) - the registry manifest (name, title, description,
  repository, version, website).

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

## Also worth listing on

- Client-side directories that index MCP servers (Claude Desktop / Cursor
  community lists, Smithery, mcp.so, etc.). Most read from the official registry
  or a simple PR to a markdown list.

## Client config (copy-paste)

```json
{
  "mcpServers": {
    "ghost": { "command": "C:/path/to/ghost-mcp.exe" }
  }
}
```

Works with any MCP client. See the README for the full tool list.

## Keeping the installed binary current (auto-update)

New Claude sessions launch `~/.local/bin/ghost-mcp.exe` (a stable path outside the
build folder, so it survives `cargo clean` and repo moves). Two things keep it on
the latest build:

- **`scripts/install.ps1`** — `cargo build --release -p ghost-mcp` then install to
  the stable path. Run it to publish a new version immediately.
- **`GhostMcpAutoSync`** scheduled task (hourly) — copies the newest release build
  to the stable path automatically. Because Windows locks a running `.exe`, the
  sync renames the in-use binary aside and drops the fresh one in, so it updates
  even while sessions are live; new sessions pick it up, running sessions keep
  their copy, and the renamed leftovers self-clean once their process exits.

After any `cargo build --release -p ghost-mcp`, the installed binary refreshes
within the hour (or instantly via `scripts/install.ps1`).
