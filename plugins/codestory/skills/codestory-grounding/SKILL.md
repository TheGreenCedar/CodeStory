---
name: codestory-grounding
description: Use when an agent should ground a local repository with CodeStory before making source claims, planning edits, choosing tests, reviewing changes, or using packet/search evidence through the CodeStory plugin or codestory-cli.
---

# CodeStory Grounding

CodeStory indexes a repository once and serves read-only local evidence so an
agent can stop rediscovering the same files, symbols, and call paths every turn.

The target is always the repository workspace being grounded. The CodeStory
checkout is only tool source unless the user is editing CodeStory itself.

## Runtime Truth

When plugin MCP is live, `codestory://status` is the first and canonical runtime
truth. Read it before `ground`, `files`, `packet`, or `search`.

Use status fields this way:

| Status field | Meaning | Agent action |
| --- | --- | --- |
| `server_version` | Version of the active MCP server binary. | Use as active runtime evidence once MCP is live. |
| `server_executable` | Executable path serving this MCP session. | Use as active runtime evidence; do not guess from source paths. |
| `allowed_surfaces.<surface>.allowed` | A concrete MCP surface is allowed. | Use local graph entries such as `ground`, `files`, `symbol`, `definition`, `trail`, `references`, `snippet`, `affected`, `symbols`, `get_node`, `neighbors`, `shortest_path`, and `query_subgraph` only when their surface is allowed. |
| `allowed_surfaces.packet.allowed` / `allowed_surfaces.search.allowed` / `allowed_surfaces.context.allowed` | Sidecar-backed agent surfaces are allowed. | Use `packet`, `search`, and `context` only when their own allowed bit is true and `retrieval_mode=full`. |

Use `where.exe codestory-cli`, `codestory-cli --version`, release install, or
source-build checks only when MCP is missing, the plugin needs repair, or the
user asks for a CLI transcript. `CODESTORY_CLI` is for manual CLI/source
fallback commands; `.mcp.json` launches `codestory-cli` from the agent host
`PATH`.

If `codestory://status` reports `repair_setup` because the active
`server_version` is older than the latest release, repair the CLI before local
navigation, packet, search, or context. Run the installer command from
`recommended_next_calls`; do not ask the human to install the binary unless
network, permissions, or release assets block the repair.

## MCP Loop

When the plugin MCP server is available:

1. Resolve `<target-workspace>` explicitly.
2. Read `codestory://status`.
3. Obey `allowed_surfaces`.
4. Read `codestory://agent-guide` when you need the runtime's recommended next
   calls.
5. Read `codestory://grounding` or call `ground` when
   `allowed_surfaces.ground.allowed` is true.
6. Use `files`, `symbol`, `definition`, `trail`, `references`, `snippet`,
   `affected`, `symbols`, `get_node`, `neighbors`, `shortest_path`, and
   `query_subgraph` only when each corresponding surface is allowed.
7. Use `packet` only when `allowed_surfaces.packet.allowed` is true and
   `retrieval_mode=full`; use `search` only when
   `allowed_surfaces.search.allowed` is true and `retrieval_mode=full`; use
   `context` only when `allowed_surfaces.context.allowed` is true and
   `retrieval_mode=full`.

If the skill is visible but no `mcp__codestory` tools or `codestory://status`
resource are exposed, call it a plugin MCP registration failure. Use CLI only
as a degraded fallback and report that MCP was not live.

## CLI Loop

When MCP is unavailable, repair is needed, or a transcript is requested, use the
CLI directly:

1. `ready --goal local --repair --project <target-workspace> --format json`
   before local navigation or delegation when the index is missing or stale.
2. `ready --goal agent --repair --project <target-workspace> --format json`
   before packet/search claims when sidecars are missing or stale.
3. `doctor --project <target-workspace>` for a read-only health transcript.
4. `ground --project <target-workspace> --why` for compact orientation.
5. `files --project <target-workspace>` for indexed file inventory.
6. `symbol`, `trail --story --hide-speculative`, `snippet`, `files`, `symbols`,
   `get_node`, `neighbors`, `shortest_path`, `query_subgraph`, and `affected`
   for concrete local graph follow-up.
7. `search --project <target-workspace> --query ... --why` for candidate
   discovery after sidecars are full.
8. `packet`, `search`, and `context` for broad or evidence-packet answers only
   when agent packet/search readiness is full.

Always pass `--project <target-workspace>` explicitly.

For binary repair details, use [serve](references/serve.md) for MCP/PATH
behavior and [doctor](references/doctor.md) for health and repair evidence.

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
| Focused source view | `symbol`, `trail`, `snippet`, `symbols`, `get_node`, `neighbors`, `shortest_path`, `query_subgraph`, `explore` |
| Sidecar-backed evidence packet | `packet`, `search`, `context` |
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
