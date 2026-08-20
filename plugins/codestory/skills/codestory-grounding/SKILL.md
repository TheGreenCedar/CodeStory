---
name: codestory-grounding
description: Use when an agent should ground a local repository with CodeStory before making source claims, planning edits, choosing tests, reviewing changes, or using broad retrieval evidence through the CodeStory plugin MCP.
---

# CodeStory Grounding

CodeStory keeps a local repository map and broad-search index so agents can
reach useful evidence without rediscovering the same code every turn.

The target is always the repository being grounded. Pass its exact absolute
root as `project` on every CodeStory call. Never rely on a global active
workspace.

## Direct Tool Loop

Call the tool that matches the task. Do not call `status` first.

Using this skill does not require an MCP call when the requested work is fully
local to an already named evidence surface—for example, inspecting or editing
the content of `assets/desk.svg`. Inspect that surface directly. Naming a path
does not make the evidence surface complete when the task asks about ownership,
dependencies, runtime behavior, architecture, change impact, or another claim
whose evidence extends beyond the file. For those tasks, select the narrowest
CodeStory tool that can add evidence; do not call broad `ground` as a pre-edit
ceremony.

1. Resolve the target repository root.
2. Call the intended tool with `project=<absolute-root>`.
3. If the result says `state=preparing` or `state=updating` and includes
   `retry_after_ms`, wait for that delay and retry the same tool with the same
   arguments. The delay tracks observed preparation progress, so honor the
   reported value instead of a fixed poll interval. Do not poll status or ask
   the user to set up CodeStory.
4. Preserve cited anchors in source claims. Read focused source only for the
   remaining evidence gaps.

CodeStory prepares its local repository map and shared per-user retrieval server
automatically. `status` and
the project-bound `codestory://status{?project}` resource are optional
diagnostics for a failed or unexpectedly slow request, not prerequisites for
normal grounding.

If CodeStory tools are hidden and deferred discovery is available,
search only for the intended tool, for example `codestory mcp packet`, then call
it directly. If the plugin MCP is unavailable, use ordinary source inspection
and report the visibility gap. Do not substitute CLI diagnostics for a live
plugin result unless the user explicitly asks.

## Task Router

| Situation | Route |
| --- | --- |
| Repository orientation | `ground`; use `files` for language mix or coverage gaps. |
| Exact named file, path, or static asset with file-local evidence | Inspect it directly. When adding it to a packet, use an `exact_path` tagged probe; do not run broad grounding merely to rediscover the path. If the task asks about relationships, ownership, or impact, use the corresponding narrow tool. |
| Find a symbol | `symbol`, then `definition` or `snippet`. |
| Follow a call path | `callers`, `callees`, `trace`, or `trail`. Use `neighbors`, `shortest_path`, or `query_subgraph` only for a named node. |
| Review change impact | `affected` with explicit Git-changed `paths` (or `changed_paths` / `change_records`). Never omit the path source. |
| One graph node | `get_node`, `definition`, `references`, or `symbols`. |
| Broad structural question | `packet`; stop on Supported, NotEstablished, or Unavailable. For DrillOnce, call `packet` again once with the exact original `question`, `parent_packet_id`, and the listed `option_ids`. Use `search` or `context` only for a user-named exact target, not as packet recovery. |

## Evidence Rules

- Treat CodeStory output as evidence, not omniscience.
- An irrelevant CodeStory call adds no evidence. Skipping one for a complete,
  file-local surface is a valid use of this router; do not report that as
  plugin unavailability.
- Local repository-map output is navigation evidence. Broad packet/search
  output is stronger only when the response reports full retrieval readiness.
- When `packet` reports `supported`, `not_established`, or `unavailable`, stop.
  For `supported`, answer from the compiled support units. For
  `not_established`, answer every claim those units directly establish, then
  name the material links or claims that remain unproven; do not turn a partial
  chain into a complete one. For `unavailable`, report the typed preparation
  reason. Do not search to recover.
- When `packet` reports `drill_once`, call `packet` once more with the exact
  original `question`, `parent_packet_id`, and the listed `option_ids` (and the
  pinned generation ids when present). Then answer. Do not start a free-form
  `search` / `context` / `trail` / `snippet` loop from packet.
- `affected` is planning evidence, not a guarantee that every runtime effect was
  found.
- Tagged probes select exact or additional evidence work. They do not choose
  route order or replace the packet disposition.
- Do not paste empty grounding output as context. If a repository truly has no
  supported files, fall back to ordinary inspection or resolve the intended
  root when it is ambiguous.

## Failure Handling

- `preparing`: retry the same tool after its delay.
- `updating`: the last complete repository map remains usable; retry the same
  tool when current publication evidence is required.
- `working_locally`: use local navigation while broad search prepares.
- `unavailable`: use ordinary source inspection and report that CodeStory was
  unavailable for this task.

Maintainer commands such as `doctor`, `ready`, and retrieval status are debug
transcript tools. They do not prove that the installed plugin is live in the
agent host.

`setup.ps1` and `setup.sh` under this skill are build-from-source paths for
contributors, not normal installation steps.

## References

- [Generated MCP syntax](references/generated-mcp-syntax.md) is the agent
  argument source of truth. Do not send CLI flags as MCP fields.
- [status contract](references/status-contract.md)
- [repository map](references/ground.md)
- [files](references/files.md)
- [affected](references/affected.md)
- [packet](references/packet.md)
- [search](references/search.md)
- [context](references/context.md)
- [symbols](references/symbol.md)
- [trails](references/trail.md)
- [snippets](references/snippet.md)

Maintainer CLI `--help` lives in [generated CLI syntax](references/generated-cli-syntax.md)
and is not an MCP calling convention.
