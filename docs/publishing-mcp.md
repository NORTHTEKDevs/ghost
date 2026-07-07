# Publishing Ghost to the MCP registry

Getting Ghost into the [Model Context Protocol registry](https://registry.modelcontextprotocol.io)
(and the client directories that mirror it) is the discovery lever — it's how an
agent or user finds Ghost without already knowing it exists.

## What's in the repo

- [`server.json`](../server.json) — the registry manifest (name, description,
  repository, version, transport, env vars).

## Steps

1. **Verify the manifest against the live schema.** The registry schema is
   versioned and evolves; before publishing, validate `server.json` with the
   official `mcp-publisher` CLI (`mcp-publisher validate`) and update the
   `$schema` / `registryType` fields if the current schema differs from what's
   pinned here. Do not publish a manifest you haven't validated.
2. **Authenticate** as the `io.github.NORTHTEKDevs/*` namespace owner (GitHub
   OAuth via `mcp-publisher login github`).
3. **Publish**: `mcp-publisher publish`.
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
