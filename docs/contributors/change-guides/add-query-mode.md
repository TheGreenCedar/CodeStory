# Add A Query Mode

```mermaid
flowchart LR
    runtime["Add runtime orchestration flow"] --> contracts["Add or update API DTOs"]
    contracts --> read_model{"Need a new read model?"}
    read_model -->|"Yes"| store["Add store support"]
    read_model -->|"No"| cli["Add the CLI surface"]
    store --> cli
    cli --> parity["Update cli-parity.md if behavior changed"]
```

1. Add the orchestration flow in `codestory-runtime`.
2. Add or update DTOs through `codestory-contracts::api`.
3. Add storage support through `codestory-store` only if a new read model is needed.
4. Add the CLI surface after the runtime flow exists.
5. Update `docs/reference/cli-parity.md` if user-visible behavior changes.

