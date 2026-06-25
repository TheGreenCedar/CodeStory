---
name: codestory-grounding
description: Use when an agent should ground a local repository with CodeStory before making source claims, planning edits, choosing tests, reviewing changes, or using packet/search evidence through the CodeStory plugin or codestory-cli.
---

# CodeStory Grounding

CodeStory indexes a repository once and serves read-only local evidence so an
agent can stop rediscovering the same files, symbols, and call paths every turn.

The target is always the repository workspace being grounded. The CodeStory
checkout is only tool source unless the user is editing CodeStory itself.

## Ambient Scope Guard

Lifecycle hooks keep CodeStory ambient in hosts that support them. On session
start, resume, clear, and compact they inject CodeStory-first grounding rules
and attempt a strict `ground` snapshot from the session cwd. On user prompts,
they attempt a tiny `packet` using the actual prompt text. Hook output is
best-effort and must fail open: missing `node`, missing `codestory-cli`, missing
MCP, degraded sidecars, timeouts, non-repo folders, and empty output never block
the host session.

Hook output is a starting packet, not final proof. Before opening source files,
read `codestory://status` when MCP is live and use the allowed CodeStory
surface that fits the task. If the hook could not provide useful grounding,
follow its repair or packet/search/context next step first.

Before calling CodeStory surfaces, confirm the target is a repository workspace.
In huge or mixed folders, use repo/workspace cues first. If `status`, `ready`,
or `ground` reports no repo, no supported files, or zero indexed files, stop the
CodeStory path for that turn. Do not inject, summarize, or paste empty ground
output as context. Fall back to ordinary source/file inspection or ask for the
intended repo path when the target is genuinely ambiguous.

## Runtime Truth

When plugin MCP is live, `codestory://status` is the first and canonical runtime
truth. Read it before `ground`, `files`, `packet`, or `search`.

Use status fields this way:

| Status field | Meaning | Agent action |
| --- | --- | --- |
| `server_version` | Version of the active MCP server binary. | Use as active runtime evidence once MCP is live. |
| `cli_version` | Version reported by the active CLI runtime. | Use as the active CLI version, not the source checkout version. |
| `server_executable` | Executable path serving this MCP session. | Use as active runtime evidence; do not guess from source paths. |
| `server_executable_sha256` | Checksum of the active MCP server binary when available. | Use to confirm the exact runtime binary after install, repair, or reload. |
| `sidecar_contract_version` | Sidecar schema contract compiled into the active CLI. | Use to diagnose sidecar/runtime contract drift. |
| `plugin_runtime` | Plugin launch source and managed CLI metadata, including `plugin_runtime.plugin_root`, `plugin_cache_version`, `build_source`, and `repo_ref` when provisioned. | Treat `managed` as installed plugin runtime, `local_dev_override` as source/dev override, and `path_fallback` as degraded launch evidence. |
| `runtime_truth` | Grouped runtime source, plugin root, managed CLI path, launcher source, sidecar policy/status, and readiness lanes. | Use as the concise bounded runtime summary; fall back to the source fields when a nested value needs detail. |
| `sidecar_setup` | Plugin sidecar setup policy and last repair state. | Ask before first automatic sidecar setup; respect `enabled` and `disabled`. |
| `allowed_surfaces.<surface>.allowed` | A concrete MCP surface is allowed. | Use local graph entries such as `ground`, `files`, `symbol`, `definition`, `callers`, `callees`, `trail`, `trace`, `references`, `snippet`, `affected`, `symbols`, `get_node`, `neighbors`, `shortest_path`, and `query_subgraph` only when their surface is allowed. |
| `allowed_surfaces.packet.allowed` / `allowed_surfaces.search.allowed` / `allowed_surfaces.context.allowed` | Sidecar-backed agent surfaces are allowed. | Use `packet`, `search`, and `context` confidently when their own allowed bit is true and `retrieval_mode=full`. |

Use `where.exe codestory-cli`, `codestory-cli --version`, release install, or
source-build checks only when MCP is missing, the plugin needs repair, status
shows `path_fallback`, or the user asks for a CLI transcript. `CODESTORY_CLI`
is an explicit local-dev override; installed `.mcp.json` launches the managed
adapter first, provisions from `github_release` when needed, and records the
launch source in `plugin_runtime`. If the resolved runtime cannot spawn, or if
only a missing, unversioned, or stale `PATH` fallback is available, the adapter
stays up with `repair_setup` diagnostics instead of closing transport.

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
6. Use `files`, `symbol`, `definition`, `callers`, `callees`, `trail`, `trace`, `references`, `snippet`,
   `affected`, `symbols`, `get_node`, `neighbors`, `shortest_path`, and
   `query_subgraph` only when each corresponding surface is allowed.
7. Use `packet` only when `allowed_surfaces.packet.allowed` is true and
   `retrieval_mode=full`; use `search` only when
   `allowed_surfaces.search.allowed` is true and `retrieval_mode=full`; use
   `context` only when `allowed_surfaces.context.allowed` is true and
   `retrieval_mode=full`. Once sidecars are installed and status reports full
   readiness, prefer these surfaces for broad repo questions instead of
   avoiding sidecar evidence.

If the skill is visible but no `mcp__codestory` tools or `codestory://status`
resource are exposed, call it a plugin MCP registration failure. Use CLI only
as a degraded fallback and report that MCP was not live.

## CLI Loop

When MCP is unavailable, repair is needed, or a transcript is requested, use the
CLI directly:

1. `ready --goal local --repair --project <target-workspace> --format json`
   before local navigation or delegation when the index is missing or stale.
   This setup path is incremental by default; request a full rebuild only for
   explicit stale-cache, corruption, schema-mismatch, moved-root, or user-chosen
   reset cases.
2. `ready --goal agent --repair --project <target-workspace> --format json`
   before packet/search claims when sidecars are missing or stale.
3. `doctor --project <target-workspace>` for a read-only health transcript.
4. `ground --project <target-workspace> --why` for compact orientation.
5. `files --project <target-workspace>` for indexed file inventory.
6. `symbol`, `trail --story --hide-speculative`, `snippet`, `files`, `symbols`,
   `get_node`, `callers`, `callees`, `neighbors`, `shortest_path`, `query_subgraph`, `trace`, and `affected`
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

Route by the situation where the agent is stuck. Do not run a generic command
inventory when one status field or one graph target will answer the question.

| Stuck situation | Route |
| --- | --- |
| Orientation: "What is in this checkout?" | MCP `ground` / `codestory://grounding` or CLI `ground`; use `files` for language mix or incomplete coverage. |
| Implementation start: "Where do I edit?" | `symbol` for a concrete feature/type, then `callers`, `callees`, `trace`, or `trail --story --hide-speculative` after a node is selected. |
| Symbol impact: "What might this change touch?" | `affected` with changed files from git; treat output as review/test planning, not proof. |
| Test choice: "Which verification is smallest?" | `affected`, nearby repo docs, and touched test names before broader test lanes. |
| Source snippet: "Show me the relevant code." | `snippet --id <node-id> --function-body --lines <n>`; use `callers`, `callees`, or `trace` when relationships matter; follow truncation guidance or read source directly if capped. |
| Readiness: "Can I trust CodeStory now?" | `codestory://status` when MCP is live; CLI `agent preflight --project <target-workspace> --format json` when MCP is unavailable. |
| Repair: "A surface is blocked." | `ready --goal local --repair` for local graph; `ready --goal agent --repair` for `packet`, `search`, or `context`; use `doctor` and `retrieval status` as proof after repair. |
| Broad evidence: "I need a packet/search answer." | `packet`, `search`, or `context` only when that surface is allowed and `retrieval_mode=full`. |
| Reusable target or structured evaluation | `bookmark` for repeated targets; `drill` / `drill-suite` for evaluation lanes. |
| Local integration surface | `serve --stdio`. |

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
