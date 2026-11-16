---
name: codestory-grounding
description: For codebase questions, use the bundled shell command first. Run cs find TERM, cs refs NAME, or cs read PATH::NAME from the repository. If cs is outside PATH, run node with the absolute path to ../../bin/cs relative to this SKILL.md. Returns numbered live source even without an index. Use MCP for graph and other indexed operations.
---

# CodeStory

Use `cs find TERM` for literal text, `cs refs NAME` for definitions and mentions, or `cs read PATH::NAME` or `cs read PATH:START-END` for numbered source.
Run `cs` from the repository. When it is outside PATH, pass the absolute path to this plugin's `bin/cs` to `node`. From the directory containing this skill, that path is `../../bin/cs`.
Use ordinary reads for named files and to fill any gaps. No setup or preliminary call is needed for the shell lookups.

## Indexed tools

For a broad question spanning several files, experimental `packet` accepts `question` and returns bounded source evidence or explicit gaps. Use targeted tools for missing evidence.

For diff reviews, `affected` can start from changed ranges instead of whole files; [scope requirements](references/affected.md#explicit-changed-ranges) include source hashes and explicit unresolved cases.

Pass absolute `project`. `search` needs `query`; `repo_text:"off"` finds one symbol pattern without embedding setup. Omit it for behavior search. Use actual bindings and [matching contracts](references/generated-mcp-syntax.md); inspect complete declarations for unfamiliar fields/operations or mismatch. Inventory names only when needed. Never invent fields or IDs.

If the actual inventory exposes only `call` and `describe` (opt-in `family-v1`), invoke `call` with `{operation:"search",arguments:{project:"/absolute/repository",query:"symbol"}}` or another canonical operation. `describe({operation,project})` returns its full contract when needed. The inner `arguments` owns `project`; retry returned canonical operation names through `call`. A `call` grant covers all operations and has no per-operation request/return typing.

Keep complete results and metadata; the full wrapper is safe. Filter mirrors only with the [guide's procedure](GUIDE.md#display-each-result-payload-once).

Use sufficient source for claims; preserve source/publication identities, freshness, ambiguity, truncation and gaps. Availability is not sufficiency. Missing edges or partial, stale, capped or missing results prove no absence or exhaustive counts. Graph relations are not runtime proof; distinguish source claims from runtime observations.

Compare the relevant declaration or contract with the branch that interprets or uses it. Follow the same value or resource through the guards and operations that determine the outcome. Contrast in-scope alternatives that change the result.

Complete, exact source bodies in ordinary tool results, with file hashes and physical ranges, count as source already read. Reuse them for covered claims; fetch missing context, changed source or specific verification as needed. Preserve freshness, gaps and limits, and perform required executable and runtime checks.

Keep a terminal result for each check, from a standalone command or an explicit per-check outcome. A combined shell exit cannot establish that an earlier command passed. Mark a check unverified when its outcome is missing. Reuse recorded results for unchanged inputs and run only the remaining checks needed within scope.

Tools own preparation; follow returned retry hints. Do not repair shared state. A failed call does not end authorized investigation.

Use [GUIDE.md](GUIDE.md) for needed unfamiliar or complex detail, not routinely for known operations.
