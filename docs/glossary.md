# Glossary

Short definitions for terms used across README, usage, and architecture docs.

## Readiness

- **local navigation readiness** — SQLite cache, graph, and DB-backed browse commands (`ground`, `trail`, `snippet`, etc.) are usable
- **agent packet/search readiness** — sidecars healthy and `retrieval_mode=full`; required for trustworthy `packet` / `search`
- **retrieval mode** — sidecar status contract; only `full` serves agent packet/search
- **semantic ready** — dense-anchor embedding state matches policy; not the same as agent packet/search readiness

## Index and graph

- **grounding** — indexed context returned for a question or command, with source ties
- **snapshot** — derived grounding view rebuilt from graph tables
- **projection** — persisted derived state (callable projection, ranked summaries)
- **staged snapshot** — temporary DB during full refresh before publish
- **refresh baseline** — file inventory used to plan incremental refresh
- **trail** — focused graph walk from one symbol
- **symbol doc** — deterministic per-symbol search text in SQLite; not embedded by default
- **dense anchor** — symbol, component report, or doc selected for vector embedding
- **repo-text hit** — raw file-content match; diagnostic, not a substitute for graph evidence

## System

- **runtime** — orchestrates index, search, grounding, trails, agent flows
- **workspace** — manifest and discovery for which files belong to the project
- **contracts** — shared graph types, DTOs, events across crates
- **target context** — DB-first bundle for one concrete target (`context`), not broad `packet`
- **cache root** — directory for one project cache; override with `--cache-dir`
