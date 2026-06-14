# How CodeStory Works

Product framing lives in the [README](../../README.md). This page is the concept
layer: what gets stored, what the commands mean, and what a good run looks like.

## STAR for one change

**Situation.** You need to edit behavior that lives somewhere in the graph, not in
the file you already have open.

**Task.** Land on the owning symbol, see call paths, read the running code, ship
the change without a blind file walk.

**Action.** `index` builds the graph; `search` or `ground` picks a node;
`trail` / `snippet` / `symbol` inspect it; `context` or `packet` bundles an
answer. See [README — How it works](../../README.md#how-it-works).

**Result.** Citations point at paths in the repo. Partial or stale index state
is reported instead of implied.

Readiness lanes (cache-only vs sidecars): [usage.md](../usage.md#readiness-tracks).

## What gets stored

Per-project SQLite under your user cache, keyed by workspace path:

| Stored | Purpose |
| --- | --- |
| File inventory and refresh metadata | Incremental re-index |
| Graph nodes and edges | Calls, imports, overrides, references |
| Snippets and occurrences | Source-backed reads |
| Search projections and symbol docs | Lookup without opening every file |
| Snapshots | Cached read models rebuilt from the graph |
| Dense anchors (when policy selects them) | Sidecar vector search only |

Repo content stays local. Managed setup may fetch tool assets; indexed evidence
does not leave the cache unless you copy it.

## Terms

| Term | Meaning |
| --- | --- |
| Grounding | Context tied back to indexed files and symbols |
| Symbol doc | Generated searchable text for a symbol (lexical, not embedded by default) |
| Dense anchor | Policy-selected symbol or report that gets a vector |
| Snapshot | Derived read model; may be stale — commands should say so |
| Trail | Graph walk from one symbol: callers, callees, neighbors |
| Packet | Bounded task evidence with citations, gaps, next commands |

Full list: [glossary.md](../glossary.md).

## Where to go next

- Operator flows: [usage.md](../usage.md)
- Crates and boundaries: [architecture/overview.md](../architecture/overview.md)
- Wrong output: [contributors/debugging.md](../contributors/debugging.md)
