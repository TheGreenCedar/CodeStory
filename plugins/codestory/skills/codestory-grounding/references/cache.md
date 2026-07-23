# `cache` - Worktree Cache Bootstrap

Checks whether a parent worktree cache is compatible with a child worktree,
snapshots the SQLite cache, and rebases path-bound SQLite graph/search/doc rows
so the child can use them under its own worktree path.

## Syntax

See [generated CLI syntax](generated-cli-syntax.md) for the current command usage.
Use `<codestory-cli> <command> --help` for the complete option set.

## Agent Path

1. Run `cache rehydrate --from-project <parent> --project <child>` before the
   first child-thread index.
2. If status is `rehydrated`, call the intended `packet`, `search`, or `context`
   tool next. That call owns activation, validates the rebased core publication,
   and prepares a compatible retrieval publication when needed. If it reports
   `preparing`, wait `retry_after_ms` and retry the same call.
3. Rehydration invalidates root-bound retrieval generations while preserving
   portable v2 index artifact rows. The next broad tool call must publish and
   pin a retrieval generation for the child before its evidence is usable.
4. If status is `skipped`, use the printed rebuild guidance. `doctor` and manual
   index commands are maintainer diagnosis or proof surfaces after automatic
   activation stops converging.

## Safety Contract

Rehydrate requires clean source and target worktrees, matching `origin` URLs,
matching Git tree ids, a source SQLite schema matching the running CLI, at
least one indexed source file, and an empty target cache directory. This command
preserves and rebases SQLite graph/search/doc rows, preserves portable v2 index
artifact cache rows, and invalidates retrieval generations across
worktree-root-derived project ids. It also does not configure Rust compilation
cache such as `sccache`.
