---
name: codestory-grounding
description: Use when an agent should ground a local repository with CodeStory before making source claims, planning edits, choosing tests, reviewing changes, or using packet/search evidence through the CodeStory plugin or codestory-cli.
---

# CodeStory Grounding

Agents usually rediscover a repository on every question: search files, read
snippets, chase imports, and spend context rebuilding the same map. CodeStory
indexes once and serves read-only evidence from that map so you can answer from
citations instead of repeating discovery work.

Use CodeStory to keep source claims tied to the current local repository. Prefer
the plugin MCP server when it is installed and healthy. Use the CLI for setup,
repair, explicit transcripts, or when MCP is unavailable.

**Key concepts:**

- **Repository rediscovery**: The repeated agent work of searching files, reading snippets, and rebuilding a mental map on every new question. CodeStory indexes once to reduce that cost.
- **Local navigation**: SQLite cache, graph, and DB-backed browse commands (`ground`, `report`, `files`, `trail`, `snippet`, `context --id`, etc.) are usable.
- **Agent packet/search**: Sidecars are healthy and `retrieval_mode=full`; required for trustworthy `packet`, `search`, and query-based candidate discovery.
- **Retrieval mode**: Sidecar status contract; only `full` serves agent packet/search.
- **Semantic ready**: Dense-anchor embedding state matches policy; not the same as agent packet/search readiness.

The target is always a repository workspace. The CodeStory checkout is only the
tool source unless the user is editing CodeStory itself.

## Setup Gate

1. Resolve `<target-workspace>` explicitly. Do not infer the target from the
   current shell if the user named another repo.
2. For installed plugin MCP runtime, `.mcp.json` launches `codestory-cli` from
   the agent host `PATH`. Use `CODESTORY_CLI` only for manual CLI/source
   fallback commands, not as the installed MCP launch path.
3. Resolve `<codestory-cli>` for explicit CLI commands:
   - prefer `CODESTORY_CLI` when set;
   - otherwise use `codestory-cli` on `PATH`;
   - otherwise use a nearby `target/release/codestory-cli*` from the current or
     sibling CodeStory checkout;
   - otherwise install the released binary for the host OS.
4. Resolve the latest GitHub release tag. If `codestory-cli` exists, compare
   `codestory-cli --version` with that tag and keep the binary only when it
   already matches the latest release.
5. If `codestory-cli` is missing or outdated, download and unpack only the
   matching host asset derived from the latest tag. Do this before asking the
   human to install or run manual commands unless network access, permissions,
   or a missing release asset blocks the setup. For latest tag `vX.Y.Z`, use:
   - Windows x64: `codestory-cli-vX.Y.Z-windows-x64.zip`
   - Windows arm64: `codestory-cli-vX.Y.Z-windows-arm64.zip`
   - macOS arm64: `codestory-cli-vX.Y.Z-macos-arm64.tar.gz`
   - Linux x64: `codestory-cli-vX.Y.Z-linux-x64.tar.gz`
   - Linux arm64: `codestory-cli-vX.Y.Z-linux-arm64.tar.gz`
   - macOS x64 or missing asset: Source fallback. Build from source.
6. Put the binary in a stable user bin directory, verify
   `codestory-cli --version`, and prefer checking `SHA256SUMS.txt` from the
   same release when the host has the tools. If `PATH` changed, say the plugin MCP process may need a Codex host/app restart before a new agent thread can see it.
   If a running `codestory-cli serve --stdio --refresh none` process locks the
   old binary, install the current release into a versioned directory and put
   that directory before stale entries on `PATH`; verify `codestory-cli
   --version` from `PATH` before launch.
7. Use `scripts/setup.ps1` or `scripts/setup.sh` from this skill only for the
   source-build fallback or explicit source-artifact setup.

## MCP Loop

When the plugin MCP server is available:

1. Read `codestory://status`.
2. Read `codestory://agent-guide`.
3. Read `codestory://grounding` or call `ground` for a compact repo map before planning.
4. Use `files` when you need indexed file inventory, language coverage, or
   role counts from the existing cache.
5. Use `packet` for broad repo questions only when status says
   `agent_packet_search` is ready and strict sidecar status reports
   `retrieval_mode=full`.
6. Use `search` to discover candidates only when packet/search is ready.
7. Use `symbol`, `trail`, `references`, `snippet`, and `context` after selecting
   a concrete target.

If status is degraded, fall back to direct source reads or CLI local-navigation
commands and label packet/search as blocked.

## CLI Loop

When MCP is unavailable or a transcript is needed, use the CLI directly:

1. `doctor --project <target-workspace>` for cache, index, freshness, and
   sidecar health.
2. `index --project <target-workspace> --refresh full` for a first index;
   `--refresh incremental` for normal repair.
3. `ground --project <target-workspace> --why` for compact orientation.
4. `files --project <target-workspace>` for indexed file inventory.
5. `context`, `symbol`, `trail --story --hide-speculative`, `snippet`, `files`,
   and `affected` for concrete source-backed follow-up.
6. `search --project <target-workspace> --query ... --why` for candidate
   discovery after sidecars are full.
7. `packet --project <target-workspace> --question ...` for broad answers only
   when packet/search readiness is full.

Always pass `--project <target-workspace>` explicitly.

## Evidence Rules

- Treat CodeStory output as evidence, not omniscience.
- Preserve cited file, symbol, trail, and snippet anchors in user-facing claims.
- When `packet` reports `sufficient` and has no `follow_up_commands`, answer
  from the packet and preserve its cited anchors.
- When `packet` reports `partial`, run the named follow-up commands before
  making proof claims.
- Treat repo-text hits, semantic suggestions, fallback retrieval, stale caches,
  missing sidecar manifests, and any non-`full` retrieval mode as navigation
  hints only.
- `retrieval_mode=full` means graph and lexical sidecars are complete, generated
  symbol docs/component reports are current, and dense anchors are valid for the
  selected corpus. It is infrastructure eligibility, not answer-quality proof.
  Anything weaker is not product packet/search proof.
- Do not run broad reindexing, sidecar rebuilds, benchmarks, or Cargo builds in
  parallel with another noise-sensitive lane unless the user accepts the timing
  noise.

## Command Routing

| Need | Route |
| --- | --- |
| Setup and health | `setup embeddings`, `doctor`, `ready`, `index`, `cache` |
| Agent orientation | MCP `ground` / `codestory://grounding` or CLI `ground` |
| Broad task packet | MCP/CLI `packet` |
| Candidate discovery | MCP/CLI `search --why` |
| Focused source view | `symbol`, `trail`, `snippet`, `context`, `explore` |
| Coverage and impact | MCP/CLI `files`, `affected` |
| Reusable targets | `bookmark` |
| Structured evaluation | `drill`, `drill-suite` |
| Local integration surface | `serve --stdio` |

Load the matching reference only when detailed flags, examples, or
troubleshooting rules are needed:

- [index](references/index.md)
- [cache](references/cache.md)
- [ground](references/ground.md)
- [doctor](references/doctor.md)
- [packet](references/packet.md)
- [search](references/search.md)
- [context](references/context.md)
- [symbol](references/symbol.md)
- [trail](references/trail.md)
- [snippet](references/snippet.md)
- [drill](references/drill.md)
- [drill-suite](references/drill-suite.md)
- [query](references/query.md)
- [explore](references/explore.md)
- [files](references/files.md)
- [affected](references/affected.md)
- [bookmark](references/bookmark.md)
- [setup](references/setup.md)
- [retrieval-rollout](references/retrieval-rollout.md)
- [serve](references/serve.md)
