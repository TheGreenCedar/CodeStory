# Add A Language

```mermaid
flowchart LR
    parser["Add parser and rules"] --> registry["Register the language"]
    registry --> manifest{"Need new manifest inputs?"}
    manifest -->|"Yes"| workspace["Extend workspace settings"]
    manifest -->|"No"| tests["Add indexer tests and fidelity coverage"]
    workspace --> tests
    tests --> docs["Update subsystem docs if behavior changed"]
```

1. Add the parser and rules in the indexing implementation.
2. Register the language in the indexer-facing registry path.
3. Extend workspace settings only if the language needs new manifest inputs.
4. Add indexer tests and, when relevant, fidelity coverage.
5. Update the subsystem docs when the public behavior changes.
