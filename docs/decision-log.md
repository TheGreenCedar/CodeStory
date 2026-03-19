# Decision Log

This file is the landing page for architecture decisions that still shape the current
CodeStory workspace.

- [ADR 0001: V2 boundary reset](adrs/0001-v2-boundaries.md)
- [ADR 0002: Workspace and store decoupling](adrs/0002-workspace-store-decoupling.md)
- [ADR 0003: Search stays behind runtime services](adrs/0003-search-placement.md)
- [ADR 0004: Snapshot lifecycle stays store-owned](adrs/0004-snapshot-lifecycle.md)
- [ADR 0005: Hybrid retrieval defaults and visible fallbacks](adrs/0005-hybrid-default-retrieval.md)

Add a new ADR when one of these changes:

- dependency direction between subsystems
- source-of-truth location for persisted or derived data
- public service or contract surface
- retrieval, staging, or fallback policy that changes how the runtime behaves


