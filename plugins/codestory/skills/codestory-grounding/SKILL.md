---
name: codestory-grounding
description: Use when an agent should ground a local repository with CodeStory before making source claims, planning edits, choosing tests, reviewing changes, or using packet/search evidence through the CodeStory plugin MCP.
---

# CodeStory Grounding

CodeStory indexes a repository once and serves read-only local evidence so an
agent can stop rediscovering the same files, symbols, and call paths every turn.

The target is always the repository workspace being grounded. The CodeStory
checkout is only tool source unless the user is editing CodeStory itself.

## When To Use

Use CodeStory before making source claims, planning edits, choosing tests, or
reviewing changes in a repository. Do not wait for the user to mention it by name.

Before opening source files, call the CodeStory `status` tool with
`project=<target-workspace>` when MCP is live. The MCP server is multi-project;
every tool call must carry the caller's absolute repository root and must not
rely on a mutable global or thread workspace binding. In
Codex hosts where the server-specific `mcp__codestory__*` tool namespace is not
initially visible, use host deferred discovery/tool_search with
`codestory mcp ground status packet search`, then use the loaded CodeStory MCP
tools. Treat this as MCP activation, not source fallback.

If `status`, `ready`, or `ground` reports no repo, no supported files, or zero
indexed files, stop the CodeStory path. Do not paste empty ground output as
context. Fall back to ordinary source inspection or ask for the intended repo
path when ambiguous.

## Quick Loop (MCP)

When the plugin MCP server is available:

1. Resolve `<target-workspace>` explicitly.
2. Call `status` with `project=<target-workspace>` — field meanings in [status-contract](references/status-contract.md). If `mcp__codestory__*` tools are not initially visible and tool_search is available, query `codestory mcp ground status packet search`, then use the loaded CodeStory MCP tools.
3. Reuse that status result until repository, runtime, or index state changes, or
   a tool reports stale/freshness failure.
4. Obey `allowed_surfaces` and `retrieval_mode`.
5. Call the allowed surface that fits the task; preserve cited anchors in answers.
6. Follow project-scoped `recommended_next_calls` only when the task requires a
   blocked surface or setup is otherwise necessary.

If the skill is visible but no `mcp__codestory` tools are exposed, call it a
plugin MCP visibility failure. Unscoped resources cannot identify the caller's
repository in multi-project mode. Do not use CLI as CodeStory grounding unless
the user explicitly asks; use ordinary source inspection and report that live
MCP tools were not visible.

## Task Router

| Situation | Route |
| --- | --- |
| Orientation: "What is in this checkout?" | `ground` / `codestory://grounding`; use `files` for language mix or gaps. |
| Find symbol: "Where is X defined?" | `symbol`, then `definition` or `snippet`. |
| Trace: "Who calls this?" | `callers`, `callees`, `trace`, or `trail --story --hide-speculative`. |
| Impact: "What might this change touch?" | `affected` with changed files from git; planning hints only, not proof. |
| Broad question | `packet`, `search`, or `context` only when allowed and `retrieval_mode=full`. |
| Blocked packet/search/context, local route available | Do not repair. Route by task through allowed `ground`, `files`, `symbol`, `definition`, `callers`, `callees`, `trace`, `trail`, `references`, `snippet`, `affected`, `symbols`, `get_node`, `neighbors`, `shortest_path`, or `query_subgraph`, then use focused source only for remaining evidence gaps. |
| Blocked surface required by the task | Follow `recommended_next_calls`; normally call MCP `sidecar_setup` with the same `project` and `action=repair`, then call project-scoped `status` again. |

## Evidence Rules

- Treat CodeStory output as evidence, not omniscience.
- Preserve cited file, symbol, trail, and snippet anchors in user-facing claims.
- Local graph output is navigation evidence. It does not prove full
  packet/search readiness or substitute for sidecar-backed evidence claims.
- When `packet` reports `sufficient` with no `follow_up_commands`, answer from the packet.
- When `packet` reports `partial`, run named follow-up commands before proof claims.
- `retrieval_mode=full` is infrastructure eligibility, not answer-quality proof;
  anything weaker is navigation hints only.
- On macOS arm64, expected accelerated sidecar intent is
  `launch_mode=native_spawned` with request provider `metal`. Requested provider
  and device are intent; observed device state and source are proof inputs, not
  sufficient proof. When acceleration is required, require
  `gpu_proof.proof_status=verified`, `meaningful_accelerator_work_proven=true`,
  and `embed_smoke_ok=true`; manual assertions and device inventory remain
  diagnostic. If status still reports `vulkan`/`Vulkan0` or
  `accelerator_request_unobserved` on Apple Silicon, report a stale/pre-release
  runtime or failed repair and follow the MCP repair loop.
- CPU-backed retrieval is an explicit degraded policy. It does not make
  packet/search evidence full unless refreshed status also allows those
  surfaces and reports `retrieval_mode=full`.

## Repair Loop

Supported agent repair is MCP-only:

1. Call `status` with `project=<target-workspace>`.
2. Inspect `readiness_broker` and `sidecar_setup`; status reads report state
   but do not start repairs.
3. If the task requires the blocked surface, `recommended_next_calls` says so,
   and the `mcp__codestory__sidecar_setup` tool is visible, call MCP `sidecar_setup` with
   `project=<target-workspace>, action=repair`.
4. Call `status` again with the same project.
5. Use only the surfaces allowed by the refreshed status.

If Codex exposes CodeStory resources but hides server-specific tools, report the
host tool-visibility blocker; unscoped resources are not a safe multi-project
fallback. Do not synthesize repair context or run CLI repair in the agent path.

CLI commands such as `fix`, `doctor`, `ready`, and `retrieval status` are
maintainer/debug transcript tools. They do not prove plugin MCP is live in the
agent host and are not the supported repair path for agent grounding.

Repair details: [serve](references/serve.md), [doctor](references/doctor.md).

`setup.ps1` and `setup.sh` under this skill are build-from-source paths for
contributors only, not the normal user install path.

## References (load on demand)

- [status-contract](references/status-contract.md)
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
- [retrieval-rollout](references/retrieval-rollout.md)
- [serve](references/serve.md)
