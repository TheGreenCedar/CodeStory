# `serve` - Local Agent Integration Surface

Serves the indexed project over either a small HTTP JSON API or an MCP-style JSON-lines stdio protocol. It is for local browser/editor integrations after the cache is ready. MCP runtime fields and surface gating: [status-contract.md](status-contract.md).

For direct MCP-style clients:

```text
codestory-cli serve --stdio --multi-project --refresh none
```

The installed plugin starts `scripts/codestory-mcp.cjs`, provisions the managed
CLI when needed, and keeps diagnostic MCP available if startup fails. Once MCP
is live, call the intended repository tool directly and pass the same absolute
`project` path to every call.

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
| `--allow-non-loopback` | off | Required before HTTP serve can bind or answer wildcard/remote-facing addresses. Use only behind an intentional network boundary; HTTP routes do not require request authentication. |
| `--stdio` | off | Use JSON-lines stdio instead of HTTP. |
| `--multi-project` | off | In stdio mode, require each tool request to carry its own `project` instead of binding the server to `--project`. |
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
| Failure path | If a tool reports `preparing`, retry that same tool after `retry_after_ms`. Read status only if the managed path stops converging. In CLI/debug transcripts, use `fix --project <target-workspace> --format json` or the specific command surfaced by `doctor`; use explicit `index --refresh full` only when the health output calls for a rebuild. If bind fails, choose a free `--addr`. | Distinguishes managed preparation from cache or port failures. |
| Integration edge | Use `serve --stdio --multi-project` for MCP-style clients; it exposes project-scoped `status`, `ground`, `files`, `affected`, `packet`, `search`, `symbol`, and graph/source tools. | One server safely routes interleaved requests from different repositories without mutable workspace state. |

Stdio MCP status fields and allowed-surface rules: [status-contract.md](status-contract.md).

## Notes

- `serve` is local by default on `127.0.0.1`; non-loopback HTTP binds and non-loopback `Host`/`Origin` headers fail unless `--allow-non-loopback` is set. Do not bind wider unless the user explicitly needs remote access and the network boundary is intentional.
- HTTP only accepts GET requests for the documented routes.
- Start it after a successful index or with an intentional refresh mode.
- In one `serve --stdio` process, identical successful `packet` and search-fragment requests are cached with small LRUs keyed by request arguments, the current SQLite/WAL fingerprint, and a mandatory sidecar-readiness fingerprint. The sidecar fingerprint includes the active embedding backend, sidecar state-file metadata, strict retrieval mode, degraded reason, manifest generation/input hash/backend/dimension, and status errors. This is for repeated agent calls only; changed index files, sidecar state drift, and strict stale/unavailable readiness bypass the cache.
