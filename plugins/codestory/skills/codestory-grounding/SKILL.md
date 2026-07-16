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

1. Resolve the target repository root.
2. Call the intended tool with `project=<absolute-root>`.
3. If the result says `state=preparing`, wait for `retry_after_ms` and retry the
   same tool with the same arguments. Do not poll status or ask the user to set
   up CodeStory.
4. Preserve cited anchors in source claims. Read focused source only for the
   remaining evidence gaps.

CodeStory prepares its local repository map and in-process retrieval runtime
automatically. `status` and
`codestory://status` are optional diagnostics for a failed or unexpectedly slow
request, not prerequisites for normal grounding.

If CodeStory tools are hidden and deferred discovery is available,
search only for the intended tool, for example `codestory mcp packet`, then call
it directly. If the plugin MCP is unavailable, use ordinary source inspection
and report the visibility gap. Do not substitute CLI diagnostics for a live
plugin result unless the user explicitly asks.

## Task Router

| Situation | Route |
| --- | --- |
| Repository orientation | `ground`; use `files` for language mix or coverage gaps. |
| Find a symbol | `symbol`, then `definition` or `snippet`. |
| Follow a call path | `callers`, `callees`, `trace`, or `trail`. |
| Review change impact | `affected` with explicit Git-changed paths, then focused symbol or trace evidence. |
| Broad structural question | `packet`; use `search` or `context` for bounded follow-up. |

## Evidence Rules

- Treat CodeStory output as evidence, not omniscience.
- Local repository-map output is navigation evidence. Broad packet/search
  output is stronger only when the response reports full retrieval readiness.
- When `packet` reports `sufficient`, answer from the packet and cited anchors.
  When it reports `partial`, run the named follow-up before making proof claims.
- `affected` is planning evidence, not a guarantee that every runtime effect was
  found.
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

- [Generated CLI syntax](references/generated-cli-syntax.md) is produced from
  Clap `--help`; use it instead of maintaining option matrices by hand.

- [status contract](references/status-contract.md)
- [repository map](references/ground.md)
- [packet](references/packet.md)
- [search](references/search.md)
- [context](references/context.md)
- [symbols](references/symbol.md)
- [trails](references/trail.md)
- [snippets](references/snippet.md)
