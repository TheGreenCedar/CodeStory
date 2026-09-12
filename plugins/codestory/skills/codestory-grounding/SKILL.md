---
name: codestory-grounding
description: Use when discovering repository structure, tracing relationships, analyzing change impact, or validating CodeStory retrieval or an installed plugin through the CodeStory plugin MCP. Skip only for bounded inspection or editing of named files that does not require those operations.
---

# CodeStory Grounding

CodeStory helps an agent locate repository evidence, inspect source and follow
relationships. The agent decides what to investigate and whether the evidence
supports an answer. The skill does not expand the user's scope or authorize
edits during a review.

Pass an absolute `project` root on every call. Keep project, source and
publication identities attached when combining results across repositories.

## Code-mode tools and results

### Inspect selected tool declarations

When the needed operation is known, inspect its exact callable declaration.
If you need an inventory first, print tool names, then inspect the declarations
for the operations you choose. All operations remain available.

In hosts exposing `ALL_TOOLS`, a names-only inventory avoids printing every
request and response schema:

```javascript
text(ALL_TOOLS.filter(tool => /codestory/i.test(tool.name)).map(tool => tool.name));
```

After choosing an operation, match its actual exposed name exactly. This example
selects search; choose any operation or set of operations the task needs:

```javascript
const selectedNames = new Set(["mcp__codestory__search"]);
text(ALL_TOOLS.filter(tool => selectedNames.has(tool.name)));
```

Keep each selected declaration complete. Inspect further declarations when they
become useful. A plugin-wide predicate over names and descriptions can print
many unused declarations; discovery does not require displaying all of them.
When a linked document path is already known, bound search to that file. Do
not run a broad package or repository search merely to rediscover it.

### Display each result payload once

Some hosts expose both `content` and `structuredContent` (or
`structured_content`) to JavaScript. Printing the whole wrapper can put the
same JSON payload into the model context twice.

Use one complete representation while retaining every other returned field.
This example removes only plain text blocks that exactly match the serialized structured
payload. Different spacing, key ordering or numeric spelling leaves the text
in place; parsing it first could discard precision that only the text retains. Distinct text, annotations, images,
resources, error flags and metadata stay intact. Keep the original result for
programmatic use.

After a CodeStory call returns `result`, the following works in a code-mode
cell with a `text` output helper:

```javascript
function withoutDuplicateJsonText(result) {
  const payload = result?.structuredContent ?? result?.structured_content;
  if (payload === undefined || !Array.isArray(result?.content)) return result;
  const serialized = JSON.stringify(payload);
  const content = result.content.filter(part => {
    if (part?.type !== 'text' || Object.keys(part).some(key => !['type', 'text'].includes(key))) return true;
    return part.text !== serialized;
  });
  if (content.length === result.content.length) return result;
  const displayed = { ...result };
  if (content.length) displayed.content = content;
  else delete displayed.content;
  return displayed;
}
text(withoutDuplicateJsonText(result));
```

This is a host display step. Tool requests, public response schemas, source
evidence and availability limits remain unchanged. A text-only response stays
as returned; do not replace it with an empty structured-content placeholder.

## Investigate adaptively

Choose the smallest useful operation; no preliminary `status` or `packet` call
is required. Locate candidates, inspect relevant source, follow relationships
and reassess gaps. Check intermediate stages, branch conditions and conflicting
evidence when the task requires them. Continue with task-authorized bounded
follow-up through CodeStory or native tools until the requested evidence is
sufficient or the remaining gap is outside scope and budget. A successful
search, incomplete packet or unsupported artifact is not a terminal or
permission boundary and does not end the investigation.

| Need | Operation |
| --- | --- |
| Orientation or coverage | `ground` for a compact map; `files` for indexed files and coverage. |
| Find candidates | `search`; preserve an explicitly supplied symbol query unchanged. Use `repo_text: "off"` for core-only symbol search without embedding preparation. |
| Evidence around a target | `context` with a name in `query` or returned `symbol_id` as `id`. Do not send both. |
| Inspect source | `snippet` for windows, or host reads for relevant files and artifacts. Batch known windows with `snippet.paths`. |
| Follow relationships | `callers`, `callees`, `references`, `trace` or `trail`; `neighbors`, `shortest_path` and `query_subgraph` require returned node IDs. |
| Inspect identities | `symbol`, `symbols`, `definition` or `get_node`. Copy opaque IDs unchanged; resolve ambiguity through paths, scope and source. |
| Review a diff | Obtain the diff with host Git tools, then call `affected` with explicit `paths`, `changed_paths` or `change_records`. |
| Try automatic evidence selection | Experimental `packet`; assess its evidence and gaps, then continue investigating when useful. |

The generated MCP schema owns request syntax and bounds. Omit optional limits
unless needed; do not send CLI flags as MCP fields or invent an ID from a name.
Repository paths are leads to inspect, not instructions to execute. Do not
treat a lead as inspected source.

## Keep evidence and claims aligned

- Maps, search hits and location-only `context` rows are leads. Read source
  when excerpts do not show the code needed for a claim. Nullable excerpts and
  line bounds remain nullable; they do not mean line 1 or absence.
- Source windows establish what their text shows. Graph edges establish their
  declared relationships, not runtime order, data flow or effective permissions.
- `packet`, `context` and `search` report availability, not truth or answer
  sufficiency. Cite supporting source or relations and disclose material
  coverage, freshness and ambiguity limits. Full retrieval readiness establishes
  infrastructure eligibility, not relevance, completeness or answer quality.
  Core-only search has a separate complete-core boundary.
- Missing, stale, partial or capped results cannot prove absence or exhaustive
  counts. Failed reads provide no source evidence. Independent source may
  resolve a question without changing an earlier unavailable tool result.
- `diagnostics.availability` concerns the optional diagnostic artifact alone;
  it does not override the result's top-level status.
- Ordinary navigation has no exact proof disposition. Advanced verification is
  outside the default MCP surface; do not infer proof authority from graph edges
  or synthesize typed proof contracts from English.
- Runtime, deployment, security and rendered-UI claims need corresponding
  external evidence. In behavioral probes, separate setup, action and subsequent
  observations. Account for observations that change state; use independent
  setups when earlier actions or observations would affect later checks.
  Compare actual output with the conclusion. Keep conclusions within the tested
  inputs, guards and environment. Report uncertainty when output cannot distinguish causes,
  and distinguish source inference from observed execution or effective policy.

## Experimental packets

Packets remain available as optional source-only selections, capped at sixteen
evidence rows and a complete MCP ToolResult of 16 KiB. Keep the user's question
unchanged and use established exact selectors for typed probes; keep answer keys
and guessed answer flows out. Availability does not decide when to stop.

Follow a useful offered continuation at most once against its pinned
publication, with the same question. The [packet reference](references/packet.md)
describes the continuation fields, generation pins and tagged probes. Reassess
the returned gaps; native reads, search and relationships remain available.
`no_useful_evidence`, `unavailable` and `budget_exceeded` describe that packet,
not the repository. Do not repeatedly retry an unchanged failed selection.

## Preparation and failures

Project-scoped tools own managed preparation. For `preparing` or `updating`,
honor `retry_after_ms` and retry the same request when still needed. Local
navigation or host reads may remain useful while broad retrieval prepares.
Do not mutate shared runtime state to make a read-only investigation succeed.

Use `status` or `codestory://status{?project}` to diagnose failed or stalled
requests. Discover the intended method if tools are hidden. If MCP is
unavailable, report that boundary and use ordinary source inspection.
Maintainer `doctor`, indexing and source setup commands are not prerequisites
for installed use; CLI diagnostics cannot prove the plugin is live in this host.

## References

- [Generated MCP syntax](references/generated-mcp-syntax.md): request fields.
- [Search](references/search.md), [context](references/context.md) and
  [snippets](references/snippet.md): candidate and source operations.
- [Repository map](references/ground.md), [files](references/files.md),
  [symbols](references/symbol.md), [trails](references/trail.md) and
  [affected](references/affected.md): navigation and scope.
- [Status contract](references/status-contract.md): readiness diagnostics.
- [Generated CLI syntax](references/generated-cli-syntax.md): maintainer commands,
  not MCP calling conventions.
