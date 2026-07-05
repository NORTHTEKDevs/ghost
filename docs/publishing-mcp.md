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
