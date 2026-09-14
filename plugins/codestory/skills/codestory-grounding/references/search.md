# `search` - Discover source candidates

Search locates candidates for source inspection and relationship navigation.
It is useful for exact names, literals and behavior descriptions; broad questions
may use several searches and source reads without a preliminary packet.

## Requests

See [generated MCP syntax](generated-mcp-syntax.md). Every call names an absolute
`project`. Supply `query`; optional `limit` is 1..50 and `repo_text` is
`auto`, `on` or `off`.

`repo_text: "off"` reads the complete existing core symbol index without
embedding preparation or a writable retrieval catalog. A cold project with no
published core returns `project_unavailable` without creating a cache. Other
modes require full, current retrieval and may prepare it automatically.

In this core-only mode, `query` is one fuzzy pattern matched independently
against each symbol name. Qualified owner/member text can form one pattern;
several independent names in the same query are not a union of lookups. Preserve
an explicit user-supplied query, then use bounded targeted follow-ups for
separate names when useful. An empty composed query does not prove each name is
absent. Behavior-oriented discovery can instead use the ordinary `auto` mode
when its full-retrieval readiness and possible managed preparation are
warranted.

Behavior descriptions can locate candidates without a known name.
Field-qualified queries support `kind:`, `path:`, `name:` and `lang:` filters;
they narrow candidates rather than prove a relationship or an exhaustive result
set.

## Reading results

V3 JSON contains `identity`, `publication`, `status`, `evidence`, `gaps`,
`retrieval`, `continuation` and `diagnostics`. Each evidence row includes its own
identity, path, optional symbol ID, optional line bounds and optional excerpt.
The older top-level `hits` and `query_assessment` are not the public v3 shape.
Evidence rows retain the search engine's relevance order; their opaque
identities are not sort keys.

Use a returned non-null `symbol_id` with `context.id`, `snippet` or a relation
operation. Inspect paths and source when several candidates are plausible;
nullable IDs are not followable symbols. Source reads and native repository
search remain available when CodeStory misses the required evidence.

Treat repo text and semantic similarity as leads. Verify the claimed behavior
in source; a missing result, excerpt or optional diagnostic proves neither
absence nor a complete coverage gap. Preserve the tool's reported readiness and
source boundaries when independent inspection supplies additional evidence.
