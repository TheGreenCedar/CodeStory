---
name: codestory-grounding
description: Select the next useful evidence operation before CodeStory setup or tool discovery. Ordinary source reads need no CodeStory discovery. Use CodeStory MCP to locate repository code, inspect evidence, trace relationships or assess change impact; also for retrieval/plugin validation. Skip bounded named-file work needing none of these. If a CodeStory inventory is needed in Codex `ALL_TOOLS`, filter `tool.name` by the `mcp__codestory__` prefix, not generic `search` in descriptions. Emit matching names only, not full tool objects. Keep needed selected declarations complete.
---

# CodeStory

Stay within user scope; choose CodeStory or ordinary tools. No preliminary call is required. Keep ordinary file discovery within the selected project root; locate declared dependency sources and other user-authorized targets directly when needed.

For diff reviews, `affected` can start from changed ranges instead of whole files; [scope requirements](references/affected.md#explicit-changed-ranges) include source hashes and explicit unresolved cases.

Pass absolute `project`. `search` needs `query`; `repo_text:"off"` finds one symbol pattern without embedding setup. Omit it for behavior search. Use actual bindings and [matching contracts](references/generated-mcp-syntax.md); inspect complete declarations for unfamiliar fields/operations or mismatch. Inventory names only when needed. Never invent fields or IDs.

If the actual inventory exposes only `call` and `describe` (opt-in `family-v1`), invoke `call` with `{operation:"search",arguments:{project:"/absolute/repository",query:"symbol"}}` or another canonical operation. `describe({operation,project})` returns its full contract when needed. The inner `arguments` owns `project`; retry returned canonical operation names through `call`. A `call` grant covers all operations and has no per-operation request/return typing.

Keep complete results and metadata; the full wrapper is safe. Filter mirrors only with the [guide's procedure](GUIDE.md#display-each-result-payload-once).

Use sufficient source for claims; preserve source/publication identities, freshness, ambiguity, truncation and gaps. Availability is not sufficiency. Missing edges or partial, stale, capped or missing results prove no absence or exhaustive counts. Graph relations are not runtime proof; distinguish source claims from runtime observations.

Complete, exact source bodies in ordinary tool results, with file hashes and physical ranges, count as source already read. Reuse them for covered claims; fetch missing context, changed source or specific verification as needed. Preserve freshness, gaps and limits, and perform required executable and runtime checks.

Tools own preparation; follow returned retry hints. Do not repair shared state. A failed call does not end authorized investigation.

Use [GUIDE.md](GUIDE.md) for needed unfamiliar or complex detail, not routinely for known operations.
