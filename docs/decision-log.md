# Decision Log

This file is the index for architecture decisions that matter during the V2 cutover.

- [ADR 0001: V2 boundary reset](adrs/0001-v2-boundaries.md)
- [ADR 0002: Workspace and store decoupling](adrs/0002-workspace-store-decoupling.md)
- [ADR 0003: Search stays behind runtime services](adrs/0003-search-placement.md)
- [ADR 0004: Snapshot lifecycle stays store-owned](adrs/0004-snapshot-lifecycle.md)

Add a new ADR when one of these changes:

- dependency direction between subsystems
- source-of-truth location for persisted or derived data
- public service or contract surface
- rollout policy for staged versus live indexing behavior
