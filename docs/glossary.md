# Glossary

- grounding: the process of turning indexed code state into concise, relevant context for a question or tool action
- snapshot: a derived SQLite-backed grounding view that can be rebuilt from the primary graph tables
- projection: derived persisted data such as callable projection state or ranked grounding summaries
- trail: a focused graph walk rooted at one symbol, usually caller/callee or neighborhood oriented
- runtime: the orchestration surface that coordinates project opening, indexing, search, grounding, trail generation, and system actions
- workspace: the manifest plus filesystem discovery layer that decides which files belong to the project
- contracts: shared graph, DTO, and event types that are safe to depend on across boundaries
