# Glossary

## Readiness

- **local navigation readiness**: SQLite cache, graph, and DB-backed browse commands (`ground`, `report`, `files`, `trail`, `snippet`, `context --id`, etc.) are usable
- **agent packet/search readiness**: sidecars are healthy and `retrieval_mode=full`; required for trustworthy `packet`, `search`, and query-based candidate discovery
- **retrieval mode**: sidecar status contract; only `full` serves agent packet/search
- **semantic ready**: dense-anchor embedding state matches policy; not the same as agent packet/search readiness

## Index and graph

- **grounding**: indexed context returned for a question or command, with source ties
- **snapshot**: derived grounding view rebuilt from graph tables
- **projection**: persisted derived state such as callable projection state or ranked summaries
- **staged snapshot**: temporary DB during full refresh before publish
- **refresh baseline**: file inventory used to plan incremental refresh
- **trail**: focused graph walk from one symbol
- **symbol doc**: deterministic per-symbol search text in SQLite; not embedded by default
- **dense anchor**: symbol, component report, or doc selected for vector embedding
- **repo-text hit**: raw file-content match; diagnostic, not a substitute for graph evidence

## System

- **runtime**: orchestrates indexing, grounding, trails, packet/search flows, and system actions
- **workspace**: manifest and discovery layer for which files belong to the project
- **contracts**: shared graph types, DTOs, and events across crates
- **target context**: DB-first bundle for one concrete target (`context --id` or bookmark), not broad `packet`
- **cache root**: directory for one project cache; override with `--cache-dir`
