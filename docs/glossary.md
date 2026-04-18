# Glossary

- grounding: the process of turning indexed code state into concise, relevant context for a question or tool action
- snapshot: a derived SQLite-backed grounding view that can be rebuilt from the primary graph tables
- projection: derived persisted data such as callable projection state or ranked grounding summaries
- staged snapshot: the temporary SQLite database built during full refresh before publish replaces the live cache
- refresh baseline: the persisted file inventory and metadata used to decide what an incremental refresh must index or remove
- trail: a focused graph walk rooted at one symbol, usually caller/callee or neighborhood oriented
- runtime: the orchestration surface that coordinates project opening, indexing, search, grounding, trail generation, and system actions
- workspace: the manifest plus filesystem discovery layer that decides which files belong to the project
- contracts: shared graph, DTO, and event types that are safe to depend on across boundaries
- repo-text hit: a direct file-content match surfaced alongside indexed-symbol search results
- retrieval mode: whether runtime search is operating in symbolic or hybrid mode for the current command
- cache root: the directory that owns one project cache; by default this is under the user cache directory, but `--cache-dir` can override it
