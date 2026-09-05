# Publishing Ghost to the MCP registry

Getting Ghost into the [Model Context Protocol registry](https://registry.modelcontextprotocol.io)
(and the client directories that mirror it) is the discovery lever — it's how an
agent or user finds Ghost without already knowing it exists.

## What's in the repo

- [`server.json`](../server.json) - the registry manifest (name, title, description,
  repository, version, website).

It is a **listing-only** entry on purpose. The registry's package types are
`npm`, `pypi`, `oci`, `nuget` and `mcpb`; there is no package type for "prebuilt
binaries on a GitHub release", and the manifest that carried
`"registryType": "github"` was rejected by `mcp-publisher validate` for exactly
that reason (checked 2026-09-04 against schema 2025-12-11). A listing with no
`packages` block validates and publishes, and points people at the repository,
where the README carries the download, install and env-var instructions.

To become installable from the registry itself, ship an `.mcpb` bundle
(manifest + binary, `fileSha256` pinned) as a release asset and add an `mcpb`
package entry. That is a release-pipeline change, tracked separately.

## Steps

1. **Validate against the live schema.** The schema is versioned and the
   registry rejects deprecated ones (`2025-07-09` was refused; `2025-12-11` is
   current as of 2026-09-04). Run `mcp-publisher validate` and fix anything it
   names. Do not publish a manifest you haven't validated. `description` is
   capped at 100 characters.
2. **Authenticate** as the `io.github.NORTHTEKDevs/*` namespace owner (GitHub
   OAuth via `mcp-publisher login github`).
3. **Publish**: `mcp-publisher publish`, after the matching GitHub release exists.
4. **Bump on release**: keep `version` in `server.json` in lockstep with
   `crates/ghost-mcp/Cargo.toml` and re-publish on each release.

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
