# `serve` - Local Agent Integration Surface

Serves a project over either a small HTTP JSON API or an MCP-style JSON-lines
stdio protocol. MCP product tools own project activation and managed
preparation; the server does not require a separate doctor or manual retrieval
index step first. MCP runtime fields and surface gating:
[status-contract.md](status-contract.md).

For direct MCP-style clients:

```text
codestory-cli serve --stdio --multi-project --refresh none
```

The installed plugin starts `scripts/codestory-mcp.cjs`, provisions the managed
CLI when needed, and keeps diagnostic MCP available if startup fails. Once MCP
is live, call the intended repository tool directly and pass the same absolute
`project` path to every call.

## Syntax

See [generated CLI syntax](generated-cli-syntax.md) for the current command usage.
Use `<codestory-cli> <command> --help` for the complete option set.

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
| Failure path | If a tool reports `preparing`, retry that same tool after `retry_after_ms`. If automatic preparation stops converging, continue with local navigation or ordinary inspection and record the visibility gap. Maintainers can use `doctor` and the specific index command it surfaces. If the optional HTTP adapter cannot bind, choose a free `--addr`. | Distinguishes automatic retrieval preparation from an HTTP-adapter bind failure. |
| Integration edge | Use `serve --stdio --multi-project` for MCP-style clients; it exposes project-scoped `status`, `ground`, `files`, `affected`, `packet`, `search`, `symbol`, and graph/source tools. | One server safely routes interleaved requests from different repositories without mutable workspace state. |

Stdio MCP status fields and allowed-surface rules: [status-contract.md](status-contract.md).

## Notes

- `serve` is local by default on `127.0.0.1`; non-loopback HTTP binds and non-loopback `Host`/`Origin` headers fail unless `--allow-non-loopback` is set. Do not bind wider unless the user explicitly needs remote access and the network boundary is intentional.
- HTTP only accepts GET requests for the documented routes.
- HTTP callers may start from an existing core publication or use an intentional
  refresh mode. MCP callers should call the intended project-scoped tool and
  follow its reported retry contract.
- In one `serve --stdio` process, identical successful `packet` and
  search-fragment requests use small LRUs keyed by request arguments plus the
  exact pinned core and retrieval publication identities. Cache hits therefore
  stay inside one evidence boundary; publication changes, engine replacement,
  or unavailable readiness bypass the cached response.
