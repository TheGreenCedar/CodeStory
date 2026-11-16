# `symbol` — Inspect a Symbol's Details and Relationships

Resolves a symbol by ID or query, then returns its full metadata: kind, file location, children, incoming references, and outgoing calls.

## Syntax

See [generated MCP syntax](generated-mcp-syntax.md) for live fields. Do not send
CLI flags. Every call requires `project` (absolute repository root).

## Target Resolution

When using MCP `query`, the tool:
1. Matches indexed symbol names from a fresh complete Core publication
2. Ranks results by exact/terminal/structural match quality
3. Selects the top-ranked hit, or errors if the top two are equally ranked (ambiguous)

Name-selected symbol and graph navigation requires fresh complete Core, without
preparing or attaching a search generation or claiming a retrieval publication.
Ordinary search, packets and context retain their full-retrieval requirements.

Use `id` when you already have a stable node id. Use `choose` to pick a
1-based alternative from an ambiguity error. There is no MCP `file` field.

## Output

```
# Symbol
resolved: `AppController` -> [abc123] AppController [STRUCT] `src/lib.rs`:42
focus: [abc123] AppController [STRUCT] `src/lib.rs`:42
children: 5
- [c1] new [FUNCTION] `src/lib.rs`:100
- [c2] open_project [FUNCTION] `src/lib.rs`:150
incoming: 3
- [CALL] from main [FUNCTION] `src/main.rs`:15
outgoing: 2
- [CALL] to Storage::open [FUNCTION] `src/storage.rs`:20
```
