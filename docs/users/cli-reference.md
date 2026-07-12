# CLI reference

Power-user and debug commands for `codestory-cli`. You should not need this page
for first install — start with [user guides](README.md), then
[Trust and readiness](trust-and-readiness.md) and
[Troubleshooting](troubleshooting.md) if a session is blocked.

Plain-language readiness lanes: [Trust and readiness](trust-and-readiness.md).
Runtime status field glossary (agents): [status-contract](../../plugins/codestory/skills/codestory-grounding/references/status-contract.md).

Install: release binary from GitHub assets, or build from source:

```sh
cargo build --release -p codestory-cli
```

Windows: `.\target\release\codestory-cli.exe`.

## Readiness and repair

| Situation | Command |
| --- | --- |
| Agent handoff when MCP is down | `codestory-cli agent preflight --project <repo> --format json` |
| Repair local graph | `codestory-cli ready --goal local --repair --project <repo> --format json` |
| Repair packet/search | `codestory-cli ready --goal agent --repair --project <repo> --format json` |
| Health summary | `codestory-cli doctor --project <repo>` |
| Sidecar status | `codestory-cli retrieval status --project <repo> --format json` |
| Direct stdio MCP (debug) | `codestory-cli serve --project <repo> --stdio --refresh none` |

Preflight exposes `safe_surfaces`, `blocked_surfaces`, and `repair_command`.
`ready --format json` returns `verdicts[]` with per-goal `status`, `summary`,
`minimum_next`, and `full_repair`. `retrieval status --format json` reports
`retrieval_mode` (trust packet/search only when `full`).
When MCP is live, prefer `codestory://status` instead.

## Local navigation

```sh
codestory-cli ground --project <repo> --why
codestory-cli files --project <repo> --path src --limit 80
codestory-cli symbol --project <repo> --id <node-id>
codestory-cli trail --project <repo> --id <node-id> --story --hide-speculative
codestory-cli snippet --project <repo> --id <node-id> --context 40
codestory-cli affected --project <repo> --format markdown
```

Pipe changed files for impact hints:

```sh
git diff --name-only HEAD | codestory-cli affected --project <repo> --stdin --format json
```

Impact hints are not test results.

## Packet and search

Only trust output when `retrieval status` reports `retrieval_mode: "full"`.

```sh
codestory-cli packet --project <repo> --question "<broad task question>" --budget compact
codestory-cli search --project <repo> --query "<symbol or behavior>" --why
```

Degraded retrieval is navigation help only. See [Glossary](../glossary.md#retrieval-mode).

## Stale local cache

```sh
codestory-cli ready --goal local --repair --project <repo> --format json
```

Read commands default to `--refresh none`. Use `--refresh incremental` when a
read should refresh an existing cache first.

Reserve `index --refresh full` or moving a cache aside for maintainer-directed
recovery after status/`doctor` identifies that exact cache and coordinated local
repair cannot converge. Verify the path is under the active CodeStory cache
root, preserve the old directory until the replacement is healthy, and never
clean a user cache merely to make tests pass.

## Index and ground

```sh
codestory-cli index --project <repo> --refresh auto
codestory-cli ground --project <repo> --why
```

## Output and configuration

Most commands default to Markdown. Use `--format json` for automation.

Optional project members file:

```json
{
  "members": ["backend/", "frontend/", "shared/"]
}
```

Team or user defaults: `.codestory.toml` at project root or user home. Home
file loads first; project file overrides for project-safe preferences.
Environment variables win over files.

Configuration is resolved independently for each project and retained for the
life of that project runtime. Multi-project stdio captures the user home,
project-network opt-in, cache root, and runtime environment once; it neither
rewrites nor re-reads them when requests switch repositories. Trusted project files
may also set `embedding_query_prefix` and `embedding_document_prefix` as part of
their per-project embedding contract.

Project `.codestory.toml` cannot choose cache roots or network egress settings.
Put `cache_dir` in user home `.codestory.toml` or pass `--cache-dir`.

## Command by situation

| Stuck situation | First command | Use next |
| --- | --- | --- |
| Orientation | `ground --project <repo> --why` | `files` for language mix or coverage gaps |
| Where to edit | `symbol --project <repo> --query "<feature>"` | `callers`, `callees`, `trail` after picking a node |
| Change impact | `affected` with `--stdin` from `git diff` | Pick focused tests; not a test run |
| Readiness | `agent preflight --format json` | `codestory://status` when MCP is live |
| Broad evidence | `retrieval status --format json` | `packet` or `search` only after `full` mode |

## Sidecar setup

Product sidecar setup: [Retrieval sidecars ops](../ops/retrieval-sidecars.md).

## Environment overrides

| Variable | Purpose |
| --- | --- |
| `CODESTORY_CLI` | Local-dev override for MCP adapter binary path |
| `CODESTORY_IDE_COMMAND` | Optional shell command template for definition-open actions. Supports `{file}`, `{line}`, and `{col}`; set only trusted local templates because the template runs through your shell. |
| `CODESTORY_NO_TUI` | Disable TUI for `explore` in CI or scripts |
| `CODESTORY_SUMMARY_ENDPOINT` | Trusted summary endpoint |
| `CODESTORY_EMBED_LLAMACPP_URL` | Trusted embedding endpoint |

## Further reading

- [Troubleshooting](troubleshooting.md)
- [Contributor debugging](../contributors/debugging.md)
- [Glossary](../glossary.md)
