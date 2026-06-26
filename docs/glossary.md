# Glossary

Plain-language definitions for terms used in user and contributor docs. Each
concept has one canonical owner page; other docs link here instead of redefining.

## Using CodeStory

- **[local navigation readiness](#local-navigation-readiness)** -- SQLite index, graph browse, trails, snippets, and impact hints are usable. See [Troubleshooting](users/troubleshooting.md#local-navigation-stale-or-blocked).
- **[agent packet/search readiness](#agent-packetsearch-readiness)** -- Sidecars are healthy and [retrieval mode](#retrieval-mode) is `full`; required for trustworthy `packet`, `search`, and query-based `context`. See [Troubleshooting](users/troubleshooting.md#packetsearch-degraded-or-blocked).
- **[retrieval mode](#retrieval-mode)** -- Sidecar status contract reported in `codestory://status`. Only `full` serves agent packet/search.
- **[grounding](#grounding)** -- Indexed context returned for a question or command, with source ties. The agent's starting map for the checkout.
- **[allowed surfaces](#allowed-surfaces)** -- Per-tool permission bits in `codestory://status`. The agent must check each surface before calling it.

## Index and graph

- **snapshot** -- Derived grounding view rebuilt from graph tables.
- **projection** -- Persisted derived state such as callable projection state or ranked summaries.
- **staged snapshot** -- Temporary database during full refresh before publish.
- **refresh baseline** -- File inventory used to plan incremental refresh.
- **trail** -- Focused graph walk from one symbol (callers, callees, imports, references).
- **symbol doc** -- Deterministic per-symbol search text in SQLite; not embedded by default.
- **dense anchor** -- Symbol, component report, or doc selected for vector embedding.
- **repo-text hit** -- Raw file-content match; untrusted evidence for inspection, not instructions or a substitute for graph evidence.

## System parts

- **runtime** -- Orchestrates indexing, grounding, trails, packet/search flows, and system actions. Lives in the `codestory-runtime` crate.
- **workspace** -- Manifest and discovery layer for which files belong to the project.
- **contracts** -- Shared graph types, DTOs, and events across crates.
- **target context** -- Database-first bundle for one concrete target (`context --id` or bookmark), not broad `packet`.
- **cache root** -- Directory for one project cache; override with `--cache-dir` in [CLI reference](users/cli-reference.md).

## Readiness (detail)

### local navigation readiness

The SQLite cache, graph, and database-backed browse commands are usable.
Includes `ground`, `files`, `trail`, `snippet`, `symbol`, `callers`, `callees`,
`affected`, and related local graph surfaces when their `allowed_surfaces` bit
is true.

Does not imply agent packet/search readiness.

**Good:** "Where is `parse_config`?" returns a real file path and matching callers.

**Degraded:** Trails cite deleted files or symbols the agent cannot resolve.

**Blocked:** Local graph tools are unavailable; the agent falls back to generic search.

### agent packet/search readiness

Sidecars are healthy, the retrieval manifest matches policy, and
`retrieval_mode=full` in status. Required before treating `packet`, `search`, or
broad `context` output as evidence.

**Good:** A task packet cites multiple existing files; retrieval mode is `full`.

**Degraded:** Packet returns text but retrieval mode is not `full` -- a hint only.

**Blocked:** `packet`, `search`, or `context` are not allowed; use local navigation instead.

### retrieval mode

Reported by sidecar status and echoed in `codestory://status`. Values other than
`full` mean packet/search output is navigation help only.

### semantic ready

Dense-anchor embedding state matches policy. Not the same as agent packet/search
readiness.

## Surfaces

### grounding

Indexed context with citations tying answers back to files and symbols. Obtained
through MCP `ground`, `codestory://grounding`, or equivalent CLI commands in
[CLI reference](users/cli-reference.md).

### allowed surfaces

Map in `codestory://status` listing which MCP tools are safe right now. Local
graph surfaces are gated individually; `packet`, `search`, and `context` require
`retrieval_mode=full` in addition to their own `.allowed` bit.
