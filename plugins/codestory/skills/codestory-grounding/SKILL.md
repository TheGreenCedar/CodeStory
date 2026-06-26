---
name: codestory-grounding
description: Use when an agent should ground a local repository with CodeStory before making source claims, planning edits, choosing tests, reviewing changes, or using packet/search evidence through the CodeStory plugin or codestory-cli.
---

# CodeStory Grounding

CodeStory indexes a repository once and serves read-only local evidence so an
agent can stop rediscovering the same files, symbols, and call paths every turn.

The target is always the repository workspace being grounded. The CodeStory
checkout is only tool source unless the user is editing CodeStory itself.

## When To Use

Use CodeStory before making source claims, planning edits, choosing tests, or
reviewing changes in a repository. Do not wait for the user to mention it by name.

Lifecycle hooks keep CodeStory ambient in hosts that support them. Hook output is
a starting packet, not final proof. Before opening source files, read
`codestory://status` when MCP is live.

If `status`, `ready`, or `ground` reports no repo, no supported files, or zero
indexed files, stop the CodeStory path. Do not paste empty ground output as
context. Fall back to ordinary source inspection or ask for the intended repo
path when ambiguous.

## Quick Loop (MCP)

When the plugin MCP server is available:

1. Resolve `<target-workspace>` explicitly.
2. Read `codestory://status` — field meanings in [status-contract](references/status-contract.md).
3. Obey `allowed_surfaces` and `retrieval_mode`.
4. Call the allowed surface that fits the task; preserve cited anchors in answers.
5. Read `codestory://agent-guide` when you need the runtime's recommended next calls.

If the skill is visible but no `mcp__codestory` tools or `codestory://status`
resource are exposed, call it a plugin MCP registration failure. Use CLI only as
a degraded fallback and report that MCP was not live.

## Task Router

| Situation | Route |
| --- | --- |
| Orientation: "What is in this checkout?" | `ground` / `codestory://grounding`; use `files` for language mix or gaps. |
| Find symbol: "Where is X defined?" | `symbol`, then `definition` or `snippet`. |
| Trace: "Who calls this?" | `callers`, `callees`, `trace`, or `trail --story --hide-speculative`. |
| Impact: "What might this change touch?" | `affected` with changed files from git; planning hints only, not proof. |
| Broad question | `packet`, `search`, or `context` only when allowed and `retrieval_mode=full`. |
| Blocked surface | Follow `recommended_next_calls` from status; see [doctor](references/doctor.md) and [serve](references/serve.md). |

## Evidence Rules

- Treat CodeStory output as evidence, not omniscience.
- Preserve cited file, symbol, trail, and snippet anchors in user-facing claims.
- When `packet` reports `sufficient` with no `follow_up_commands`, answer from the packet.
- When `packet` reports `partial`, run named follow-up commands before proof claims.
- `retrieval_mode=full` is infrastructure eligibility, not answer-quality proof;
  anything weaker is navigation hints only.

## CLI Fallback

Only when MCP is missing, repair is needed, or a transcript is requested:

1. `ready --goal local --repair --project <target-workspace> --format json`
2. `ready --goal agent --repair --project <target-workspace> --format json` before packet/search
3. `doctor --project <target-workspace>` for a read-only health transcript
4. Mirror allowed MCP surfaces: `ground`, `files`, `symbol`, `trail`, `snippet`, `search`, `packet`, `context`

Always pass `--project <target-workspace>` explicitly. Repair details:
[serve](references/serve.md), [doctor](references/doctor.md).

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
- [setup](references/setup.md)
- [retrieval-rollout](references/retrieval-rollout.md)
- [serve](references/serve.md)
