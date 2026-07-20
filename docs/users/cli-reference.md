# CLI reference

Maintainer and automation commands for `codestory-cli`. You should not need this page
for first install — start with [user guides](README.md), then
[Trust and readiness](trust-and-readiness.md) and
[Troubleshooting](troubleshooting.md) if a session is blocked.

Plain-language readiness lanes: [Trust and readiness](trust-and-readiness.md).
Runtime status field glossary (agents): [status-contract](../../plugins/codestory/skills/codestory-grounding/references/status-contract.md).

Install: release binary from GitHub assets, or build from source:

```sh
export CODESTORY_EMBED_MODEL_SOURCE="$(node scripts/prepare-embedded-model.mjs)"
cargo build --release --locked -p codestory-cli
```

In PowerShell, prepare with
`$env:CODESTORY_EMBED_MODEL_SOURCE = node scripts/prepare-embedded-model.mjs`.
Windows binary: `.\target\release\codestory-cli.exe`.

Generated `codestory-cli --help` and subcommand help are the source of truth for
flags. This page groups stable workflows and trust boundaries rather than
copying every option.

## Readiness and retrieval

| Situation | Command |
| --- | --- |
| Agent handoff when MCP is down | `codestory-cli agent preflight --project <repo> --format json` |
| Refresh local graph | `codestory-cli index --project <repo> --refresh auto --format json` |
| Build packet/search retrieval | `codestory-cli retrieval index --project <repo> --refresh full --format json` |
| Health summary | `codestory-cli doctor --project <repo>` |
| Managed search status | `codestory-cli retrieval status --project <repo> --format json` |
| Direct single-project stdio MCP (debug) | `codestory-cli serve --project <repo> --stdio --refresh none` |

Preflight exposes `safe_surfaces`, `blocked_surfaces`, and the next normal retrieval action.
`ready --format json` returns `verdicts[]` with per-goal `status`, `summary`,
and `minimum_next`. `retrieval status --format json` reports
`retrieval_mode` (trust packet/search only when `full`).
When MCP is live, prefer the project-bound `codestory://status{?project}`
resource instead.

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
codestory-cli index --project <repo> --refresh auto --format json
```

Read commands default to `--refresh none`. Use `--refresh incremental` when a
read should refresh an existing cache first.

Reserve `index --refresh full` or moving a cache aside for maintainer-directed
recovery after status or `doctor` identifies that exact cache and coordinated
refresh cannot converge. Verify the path is under the active CodeStory cache
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

Project `.codestory.toml` cannot choose cache roots. It also cannot choose
network egress settings by default. A trusted operator may set
`CODESTORY_ALLOW_PROJECT_NETWORK_CONFIG=1` for the whole process to allow every
project opened by that process to configure summary endpoints. That opt-in can
redirect source text, so never enable it while opening untrusted repositories.
Embedding never uses a network endpoint. Put `cache_dir` in user home `.codestory.toml` or pass
`--cache-dir`.

## Command by situation

| Stuck situation | First command | Use next |
| --- | --- | --- |
| Orientation | `ground --project <repo> --why` | `files` for language mix or coverage gaps |
| Where to edit | `symbol --project <repo> --query "<feature>"` | `callers`, `callees`, `trail` after picking a node |
| Change impact | `affected` with `--stdin` from `git diff` | Pick focused tests; not a test run |
| Readiness | `agent preflight --format json` | `codestory://status{?project}` when MCP is live |
| Broad evidence | `retrieval status --format json` | `packet` or `search` only after `full` mode |

## Managed search internals

Maintainer-only engine details: [retrieval operations](../ops/retrieval-engine.md).

## Environment overrides

| Variable | Purpose |
| --- | --- |
| `CODESTORY_CLI` | Local-dev override for MCP adapter binary path |
| `CODESTORY_IDE_COMMAND` | Optional shell command template for definition-open actions. Supports `{file}`, `{line}`, and `{col}`; set only trusted local templates because the template runs through your shell. |
| `CODESTORY_NO_TUI` | Disable TUI for `explore` in CI or scripts |
| `CODESTORY_SUMMARY_ENDPOINT` | Trusted summary endpoint |
| `CODESTORY_EMBED_ALLOW_CPU` | Explicitly allow CPU embeddings. Intended for hosted CI and maintainer diagnostics; production does not fall back silently. |
| `CODESTORY_ALLOW_PROJECT_NETWORK_CONFIG` | Process-wide opt-in allowing trusted project files to configure summary endpoints |

## Further reading

- [Troubleshooting](troubleshooting.md)
- [Contributor debugging](../contributors/debugging.md)
- [Glossary](../glossary.md)
