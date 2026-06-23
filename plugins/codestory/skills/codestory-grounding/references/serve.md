# `serve` - Local Agent Integration Surface

Serves the indexed project over either a small HTTP JSON API or an MCP-style JSON-lines stdio protocol. It is for local browser/editor integrations after the cache is ready.

For the installed plugin, `.mcp.json` starts:

```text
codestory-cli serve --stdio --refresh none
```

The process resolves `codestory-cli` from the agent host `PATH`. Once MCP is
live, `codestory://status` is the runtime truth: use `server_version`,
`server_executable`, and `allowed_surfaces` from status before any local
grounding, packet, or search call.

## Usage

```
<codestory-cli> serve [OPTIONS]
```

## Options

| Option | Default | Use |
|--------|---------|-----|
| `--project <path>` | `.` | Repository root to serve. Always pass it explicitly. |
| `--cache-dir <path>` | auto | Serve a specific cache directory. |
| `--addr <host:port>` | `127.0.0.1:3917` | HTTP bind address. |
| `--stdio` | off | Use JSON-lines stdio instead of HTTP. |
| `--refresh <auto|full|incremental|none>` | `none` | Read an existing cache unless you intentionally refresh. |

## HTTP Routes

| Route | Parameters | Use |
|-------|------------|-----|
| `/health` | none | Basic process health. |
| `/search` | `q`, optional `repo_text`, `limit` | Search indexed symbols and repo text. |
| `/symbol` | `q` | Resolve symbol details by query. |
| `/definition` | `q` or `id` | Definition metadata plus symbol context. |
| `/references` | `q` or `id`, optional `depth` | Incoming references. |
| `/symbols` | optional `parent_id`, `limit` | Root symbols or children. |
| `/trail` | `q`, optional `depth` | Neighborhood trail. |

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | `<codestory-cli> serve --project <target-workspace> --addr 127.0.0.1:3917` then `GET /health` | Local JSON service returns `{"ok": true}` and browser routes use the existing index. |
| Failure path | If serve reports missing index, run `doctor --project <target-workspace>` and `index --project <target-workspace> --refresh full`; if bind fails, choose a free `--addr`. | Distinguishes cache readiness from port conflicts. |
| Integration edge | Use `serve --stdio` for MCP-style clients; it exposes tools for `ground`, `files`, `affected`, `packet`, `search`, `symbol`, `trail`, `definition`, `references`, `symbols`, `snippet`, `context`, `get_node`, `neighbors`, `shortest_path`, and `query_subgraph`, plus project/grounding resources, warm graph primitives, and prompts. | Gives agents the same read-only packet and browser primitives without shelling each command. |

## Stdio Runtime Contract

| Status field | Use |
|--------------|-----|
| `server_version` | Active MCP server version. Prefer this over source checkout or package version once MCP is live. |
| `server_executable` | Active MCP server executable path. Use it to diagnose stale PATH or binary drift. |
| `allowed_surfaces.<surface>.allowed` | Allows that concrete MCP surface. Local graph surfaces include `ground`, `files`, `symbol`, `definition`, `trail`, `references`, `snippet`, `affected`, `symbols`, `get_node`, `neighbors`, `shortest_path`, and `query_subgraph`. |
| `allowed_surfaces.packet.allowed` / `allowed_surfaces.search.allowed` / `allowed_surfaces.context.allowed` | Allows `packet`, `search`, and `context` only when the surface bit is true and `retrieval_mode=full`. |

Use `where.exe codestory-cli` and `codestory-cli --version` only when MCP is not
registered, status is unavailable, or the status executable/version indicates a
stale binary. If PATH changes during repair, start a fresh Codex host/app session before checking status again.

## Notes

- `serve` is local by default on `127.0.0.1`; do not bind wider unless the user explicitly needs remote access.
- HTTP only accepts GET requests for the documented routes.
- Start it after a successful index or with an intentional refresh mode.
- In one `serve --stdio` process, identical successful `packet` and search-fragment requests are cached with small LRUs keyed by request arguments, the current SQLite/WAL fingerprint, and a mandatory sidecar-readiness fingerprint. The sidecar fingerprint includes the active embedding backend, sidecar state-file metadata, strict retrieval mode, degraded reason, manifest generation/input hash/backend/dimension, and status errors. This is for repeated agent calls only; changed index files, sidecar state drift, and strict stale/unavailable readiness bypass the cache.
