# How CodeStory Works

CodeStory is a local evidence layer for codebases. It does not replace judgment,
tests, or source reading. It makes the first pass more structured.

An agent usually fails on a large repo by over-weighting the first few files it
opens. CodeStory gives that agent an indexed map before it explains behavior or
plans a change.

## The Loop

`doctor` → `index` → `ground` or `search` → inspect with `symbol` / `trail` / `snippet` / `explore` → `context` or `packet`.

For the two-lane system map (SQLite cache vs sidecars), see [README — What It Builds](../README.md#what-it-builds).

- `doctor` checks whether the cache, index, retrieval mode, and local embedding
  setup are usable.
- `index` builds or refreshes local graph, search, snapshot, graph-native
  symbol-doc, component-report, and selected dense-anchor state for one target
  repository.
- `ground` gives broad orientation and reports limited coverage or gaps.
- `search` finds candidate files, symbols, routes, literals, modules, or behavior
  terms.
- `symbol`, `trail`, `snippet`, and `explore` inspect one selected target.
- `context` bundles deeper evidence around that concrete target.
- `packet` handles broad task questions and reports citations, gaps, and next
  commands.

The workflow is a repeatable evidence loop.

## What Gets Stored

CodeStory writes per-project state under the user cache, keyed by the target
workspace path. The cache can include:

- discovered files and refresh metadata
- graph nodes for files, symbols, and related code elements
- graph edges such as calls, imports, overrides, and references
- source snippets and occurrence locations
- search projection rows and local search indexes
- grounding snapshots rebuilt from the graph
- graph-native symbol docs, which are deterministic searchable summaries for
  durable AST symbols
- selected dense anchors, which are the only generated docs embedded as vectors
  under the active semantic policy

Repository data stays local. Managed setup may fetch tool or model assets, but
the indexed project evidence lives in the local cache.

## Key Terms

- Grounding is source-backed context: the files, symbols, and summaries a command
  returns so an answer can be tied back to repository evidence.
- A symbol doc is deterministic generated text for a symbol, stored so lexical
  and graph retrieval can find relevant code even when the query words are not
  exact.
- A dense anchor is a policy-selected symbol, component report, or unstructured
  doc that receives a vector embedding. Code symbols do not need dense vectors
  to be product-searchable.
- A snapshot is a cached read model rebuilt from the local graph. If a snapshot
  is stale, the tool should say so.
- A trail is a focused graph walk around one symbol: callers, callees,
  references, or neighborhood context.
- A packet is a bounded evidence bundle for a broad task. It should include
  citations, gaps, and follow-up commands.

## What Good Looks Like

A good CodeStory-backed answer does three things:

1. It names the files, symbols, or snippets it used.
2. It says when evidence is stale, partial, ambiguous, or missing.
3. It gives the next concrete command when the current evidence is not enough.

The goal is not a more confident answer. The goal is confidence constrained by
source evidence.

## Where To Go Next

- Use [../usage.md](../usage.md) for command flows.
- Use [../architecture/overview.md](../architecture/overview.md) for the system
  boundary and crate model.
- Use [../contributors/debugging.md](../contributors/debugging.md) when output
  looks wrong.
