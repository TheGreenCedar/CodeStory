# Documentation Visuals

CodeStory docs use Mermaid diagrams embedded in markdown. GitHub renders them
natively; no build step or static image pipeline is required.

## Diagram Types

| Type | Use when |
| --- | --- |
| `flowchart` | Workflows, decision trees, readiness lanes, crate boundaries, proof ladders |
| `sequenceDiagram` | Command paths with multiple actors (CLI, runtime, store, sidecars) |
| `stateDiagram-v2` | Mode transitions (`retrieval_mode`, refresh modes) |

Prefer `flowchart` for entry docs. Reserve `sequenceDiagram` for architecture pages
that already show multi-crate call paths.

## Canonical Diagrams

Link to the canonical page instead of copying large diagrams:

| Concept | Canonical location |
| --- | --- |
| Evidence loop | [how-codestory-works.md](../concepts/how-codestory-works.md#the-loop) |
| Two readiness tracks | [usage.md](../usage.md#readiness-tracks) |
| Pipeline (repo to agent) | [README.md](../../README.md#what-it-builds) |
| Crate/layer model | [overview.md](../architecture/overview.md) |
| Indexing lifecycle | [indexing-pipeline.md](../architecture/indexing-pipeline.md) |
| Sidecar mode matrix | [retrieval-design.md](../architecture/retrieval-design.md) |
| Proof tiers | [retrieval-architecture.md](../testing/retrieval-architecture.md) |

**Link, don't copy:** README and usage may show compact summaries; full crate graphs
and sequence diagrams live in architecture pages.

## Mermaid Syntax Checklist

- Use camelCase or PascalCase node IDs without spaces (for example `localNav`, not `local nav`).
- Quote node labels that contain special characters: `["retrieval_mode=full"]`.
- Do not use HTML tags or entities in labels.
- Do not set custom colors or `style` fills; theme handles rendering.
- Avoid reserved keywords as bare node IDs (`end`, `graph`, `subgraph`).
- For subgraphs, use explicit IDs: `subgraph localLane [Local navigation]`.

## Content Rules

- Put diagrams **above** dense prose or tables they summarize.
- Keep tables for lookup; use diagrams for scan paths and mental models.
- **No benchmark numbers inside diagrams.** Token counts, timings, and overhead
  ratios belong in append-only stats logs and ledger tables, not in Mermaid blocks.
- Conceptual labels only in testing docs (for example "exploratory evidence", not
  `114510 tokens`).

## Runtime-Generated Diagrams

For symbol-level flow evidence from an indexed workspace, use:

```sh
codestory-cli trail --project <target-workspace> --id <node-id> --mermaid
```

See [.agents/skills/codestory-grounding/references/trail.md](../../.agents/skills/codestory-grounding/references/trail.md).

## Adding A New Diagram

1. Check whether a canonical diagram already covers the concept.
2. Pick the lowest tier page that owns the concept (entry, architecture, testing).
3. Add a one-line lead-in before the fenced `mermaid` block.
4. Preview in GitHub or VS Code Mermaid preview before opening a PR.
