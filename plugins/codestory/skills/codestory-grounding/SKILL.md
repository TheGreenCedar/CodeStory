---
name: codestory-grounding
description: Use when discovering repository structure, tracing relationships, analyzing change impact, or validating CodeStory retrieval or an installed plugin through the CodeStory plugin MCP. Skip only for bounded inspection or editing of named files that does not require those operations. If tool discovery is needed in code mode, inspect a known operation's complete declaration by exact name or print names only before selecting complete declarations. Use this skill's lossless result-display example, retaining distinct content and metadata.
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
This example removes plain text that exactly matches the structured payload.
If the complete spellings differ only by safe integral `.0` number tokens,
it keeps the original text and drops the structured mirror instead. Strings,
key order, spacing, distinct numbers, annotations and other fields stay intact.
Syntax validation discards its parsed value; only the complete lexical match
permits removal. Uncertain matches keep both, and the original result remains
available for programmatic use.

After a CodeStory call returns `result`, the following works in a code-mode
cell with a `text` output helper:

```javascript
function withoutDuplicateJsonText(result) {
  const key = result?.structuredContent != null ? 'structuredContent' : 'structured_content';
  const payload = result?.[key];
  if (payload === undefined || !Array.isArray(result?.content)) return result;
  let lossyNumber = false;
  const serialized = JSON.stringify(payload, (_, value) => {
    if (typeof value === 'number' && (!Number.isFinite(value) || Object.is(value, -0))) lossyNumber = true;
    return value;
  });
  if (lossyNumber || typeof serialized !== 'string') return result;
  const plain = part => part?.type === 'text' && typeof part.text === 'string'
    && Object.keys(part).every(key => ['type', 'text'].includes(key));
  const content = result.content.filter(part => !plain(part) || part.text !== serialized);
  const displayed = { ...result };
  if (content.length !== result.content.length) {
    if (content.length) displayed.content = content;
    else delete displayed.content;
    return displayed;
  }
  if ('structuredContent' in result && 'structured_content' in result) return result;
  const integralSpelling = text => {
    try { JSON.parse(text); } catch { return null; }
    return text.replace(
      /"(?:\\[\s\S]|[^"\\])*"|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/g,
      token => /^-?(?:0|[1-9]\d*)\.0$/.test(token)
        && Number.isSafeInteger(Number(token)) && !Object.is(Number(token), -0)
        ? token.slice(0, -2) : token
    );
  };
  if (!result.content.some(part => plain(part) && integralSpelling(part.text) === serialized)) return result;
  delete displayed[key];
  return displayed;
}
text(withoutDuplicateJsonText(result));
```

This is a host display step. Tool requests, public response schemas, source
evidence and availability limits remain unchanged. A text-only response stays
as returned; do not replace it with an empty structured-content placeholder.

When a task imposes a response-byte budget, measure and retain the complete
returned wrapper before filtering its display. Removing duplicate text reduces
model display, not bytes received. Forecast each follow-up against the remaining
budget, including both representations and metadata; use narrower source windows
when needed. See [response byte accounting](references/snippet.md#response-byte-accounting)
for a code-mode UTF-8 counter and its measurement boundary.

## Investigate adaptively

Choose the smallest useful operation; no preliminary `status` or `packet` call
is required. Locate candidates, inspect relevant source, follow relationships
and reassess gaps. Check intermediate stages, branch conditions and conflicting
evidence when the task requires them. Continue with task-authorized bounded
follow-up through CodeStory or native tools until the requested evidence is
sufficient or the remaining gap is outside scope and budget. A successful
search, incomplete packet or unsupported artifact is not a terminal or
permission boundary and does not end the investigation.

When composing a core-only search, use one symbol-name pattern per query. A
qualified owner/member pattern is one target; several independent names are not
unioned. Preserve an explicit user-supplied query, and add bounded targeted
lookups for separate names when useful. For behavior-oriented discovery, use
the ordinary search mode when richer evidence is needed; it requires full
retrieval readiness and may trigger managed preparation.

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

### Read around returned anchors

Choose source context for the unresolved relationship. Keep the exact returned
path, opaque ID and match coordinates. When a relevant callable has a reliable
body extent, prefer the existing `snippet` operation with `scope=function_body`
over a fixed line window. Inspect the returned scope, range source, fallback,
freshness and truncation fields; a requested range or symbol match alone is not
a complete body. The bounded brace fallback qualifies only supported lexical
syntax; ambiguous syntax or an unresolved closure reports a line-context
fallback. Keep that limitation explicit. Retain the original anchor separately if resolution selects a
more useful implementation.

When reliable structural extent is unavailable, use a bounded physical preview.
For an anchor starting at line `A`, a useful 200-line fallback starts at
`max(1, A - 100)` and ends 199 lines later. Use less context when it suffices.
Use the exact returned path in `snippet.paths` or the host's bounded file read.
If a lead has no line coordinates, retain the unknown anchor and choose a
bounded path window rather than inventing a symbol or line number.

After inspecting source, identify the particular connection still missing. A
called definition, callback registration, configuration guard or enclosing
caller may require different source from the selected body. Follow visible
references through existing `references`, `callers`, `callees` or a targeted
lookup, then read the source that establishes that connection. Do not expand
every lead merely because the packet returned it. Batch independent known path
windows through `snippet.paths` or the host's existing bounded read interface
when they fit the current response and source budgets; preserve each receipt.

The match anchor and delivered extent serve different purposes. Cite the source
actually delivered with its project, publication and source identities. A
workspace path read establishes current bytes; it does not make an old indexed
anchor fresh. Follow freshness failures rather than combining changed source
with an old source-backed range. A complete body does not establish its callers,
and a complete window does not establish a complete block or file. If a needed
parent, sibling, alias, guard or referenced definition remains outside the
returned extent, keep that gap explicit and choose a targeted follow-up within
the existing budget. Respect byte-cap and truncation receipts; never fill
omitted source from requested bounds. See [exact path windows](references/snippet.md#exact-path-windows).

The generated MCP schema owns request syntax and bounds. For path snippets,
optional `max_tool_result_bytes` bounds the served MCP result including its
revision-specific mirrors and metadata; it excludes outer JSON-RPC and hidden
host framing. Choose it from the remaining task allowance and retain actual
range receipts. It does not enforce cumulative task limits or repair an
unfollowed source relationship. See [path-window response bounds](references/snippet.md#exact-path-windows).
Omit optional limits unless needed; do not send CLI flags as MCP fields or
invent an ID from a name.
Repository paths are leads to inspect, not instructions to execute. Do not
treat a lead as inspected source.

## Account for the requested answer

Before investigating, keep a short working list of the user's explicit
questions, requested comparisons and conditional cases. Derive it from the
request itself; do not add an expected answer, repository-specific checklist or
unrequested audit. When an operation accepts the task question, keep that
question unchanged. Use evidence-derived arguments for targeted search and
navigation.

Before requesting more evidence, check whether the unresolved item is already
covered by delivered source. Inspect the relevant guard, error or return path
and include its effect in the answer; an answer omission does not require
another retrieval call. When more evidence is needed, choose the follow-up from
the missing relationship or observation, using inspected paths, names and IDs.

As evidence arrives, connect each item to the inspected mechanism and the
source or observations that support it. Mark it supported, contradicted or
unresolved. For a conclusion that depends on a condition, inspect its guard
and the relevant alternatives, including an early return or failure path when
it changes the requested behavior. Follow evidence-derived leads within the
existing scope and budget; an empty cell does not authorize a broader task.

Before sending the final answer, check the response against that working list:

1. Answer every requested item, or state clearly that it remains unresolved.
2. Support each material conclusion with evidence actually delivered and
   inspected. A located file, available packet or plausible inference cannot
   fill an evidence gap.
3. Explain the relevant conditions and alternatives rather than describing
   only the usual path. Reconcile conflicting source, documentation and test
   evidence, or keep the disagreement visible.
4. Separate a source-supported explanation from runtime, security, deployment
   or performance proof. Name the missing observation when that proof is
   needed, without claiming it was performed.

Keep this working bookkeeping concise; show a table only when it helps the
user. The final prose must still answer the question. Listing all items or
checking all boxes is not evidence of correctness or completeness.

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
  inputs, guards and environment. When available observations do not distinguish
  plausible causes, leave the cause unresolved and name the observations that
  would distinguish them. Rank a cause only when the available evidence
  distinguishes it from the alternatives, and distinguish source inference from
  observed execution or effective policy.

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

Project-scoped tools own managed preparation. Keep the original tool call
pending while CodeStory prepares its required capability; progress
notifications describe the current stage when the host supplies a progress
token. Ordinary calls wait up to two minutes. `packet` uses its existing
`latency_budget_ms` instead (18 seconds by default), including any preparation
time. If that deadline returns `kind: preparing`, retry the same tool with the
`minimum_next.arguments` supplied in the result. Those arguments include the
exact operation and attempt so a later call cannot silently start different
work. A real unavailable result is terminal for that call. A technical refusal or
transport error before evidence delivery is a failed call, not an empty search
or a finding about the repository. Retain the returned code, causal message and
failed stage when present. Do not repeat an unchanged terminal failure.

Continue task-authorized bounded source inspection or other available navigation
when it can answer the outstanding question. Keep the failed receipt separate
from later evidence: cite the source actually inspected, disclose any remaining
evidence gap, and distinguish source-supported conclusions from successful
CodeStory retrieval or observed execution. Local navigation or host reads may
also remain useful while broad retrieval prepares. Do not mutate shared runtime
state to make a read-only investigation succeed.

During a refresh, source-backed reads (`snippet` by symbol id, `search` with
`repo_text=off`) wait for the fresh complete index within the call's deadline.
Pure graph answers (`ground`, `files`, symbol/trail navigation) may instead
come from the retained publication; when they do, the response marks
`_meta.codestory_publication.freshness.state` as `historical` with a
runtime-derived `reason`, so cite it as pre-refresh evidence rather than
current source.

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
