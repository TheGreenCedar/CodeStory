# Documentation Search Index

Use this page for keyword routing. [Docs home](README.md) owns reader journeys;
the linked page owns the concept.

## Architecture

| Keyword | Canonical page |
| --- | --- |
| topology, crates, dependency direction, identity scopes | [Architecture overview](architecture/overview.md) |
| plugin launcher, fail-open, provisioning, handoff, project routing | [Host integration](architecture/host-integration.md) |
| activation, tool classes, request lifecycle, bounded retry | [Runtime execution](architecture/runtime-execution-path.md) |
| core index, refresh plan, parser, resolution, snapshots | [Indexing pipeline](architecture/indexing-pipeline.md) |
| vectors.sqlite3, retrieval manifest, publication, readiness | [Retrieval design](architecture/retrieval-design.md) |
| Metal, Vulkan, embedded model, llama.cpp, engine queues | [Llama sys subsystem](architecture/subsystems/llama-sys.md) |
| lexical, semantic, SCIP, generation leases, retention | [Retrieval subsystem](architecture/subsystems/retrieval.md) |
| language tiers, parser-backed, structural source proof | [Language support](architecture/language-support.md) |
| contracts, workspace, store, indexer, runtime, CLI | [Subsystem pages](architecture/subsystems/) |

## Users and operators

| Keyword | Canonical page |
| --- | --- |
| host selection, installation | [User guides](users/README.md) |
| Codex, Cursor, Claude Code, Copilot | [Host guides](users/README.md#pick-your-host) |
| proof, hints, readiness, allowed surfaces | [Trust and readiness](users/trust-and-readiness.md) |
| preparing, stale, unavailable, recovery | [Troubleshooting](users/troubleshooting.md) |
| commands, status fields, configuration | [CLI reference](users/cli-reference.md) |
| prompt shapes | [Prompt patterns](users/prompt-patterns.md) |
| terminology | [Glossary](glossary.md) |

## Contributors and evidence

| Keyword | Canonical page |
| --- | --- |
| local setup, worktrees | [Contributor setup](contributors/getting-started.md) |
| test lane, proof tier, release gate | [Testing matrix](contributors/testing-matrix.md) |
| engine package, hardware, restart proof | [Retrieval verification](testing/retrieval-architecture.md) |
| embedding measurements | [Embedding benchmarks](testing/embedding-backend-benchmarks.md) |
| indexing and packet telemetry | [E2E stats log](testing/codestory-e2e-stats-log.md) |
| language benchmark evidence | [Language holdout stats](testing/language-expansion-holdout-stats.md) |
| retrieval diagnostics | [Retrieval operations](ops/retrieval-engine.md) |
| research comparisons | [Research handbook](research.md) |
| docs checks and ownership | [Documentation checklist](contributors/documentation-maintenance-checklist.md) |

## Common routing

- “How does one installed plugin serve several repositories?” Start with
  [host integration](architecture/host-integration.md), then
  [runtime execution](architecture/runtime-execution-path.md).
- “Why is graph navigation ready while packet is preparing?” Start with
  [indexing pipeline](architecture/indexing-pipeline.md), then
  [retrieval design](architecture/retrieval-design.md).
- “What proves Metal or Vulkan?” Start with
  [retrieval verification](testing/retrieval-architecture.md).
- “Where should this code change live?” Start with the
  [architecture overview](architecture/overview.md), then the owning subsystem.

Update this index only for a new canonical topic or page. Do not duplicate
command matrices, test instructions, or time-specific measurements here.
