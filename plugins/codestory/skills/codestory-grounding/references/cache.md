# `cache` - Worktree Cache Bootstrap

Checks whether a parent worktree cache is compatible with a child worktree,
snapshots the SQLite cache, and rebases path-bound SQLite graph/search/doc rows
so the child can use them under its own worktree path.

## Usage

```text
<codestory-cli> cache rehydrate --from-project <parent-worktree> --project <child-worktree>
```

## Options

| Option | Default | Use |
|--------|---------|-----|
| `--project <path>` | `.` | Target child worktree. |
| `--cache-dir <path>` | auto | Target cache directory. Must be empty. |
| `--from-project <path>` | required | Source worktree with an existing CodeStory cache. |
| `--from-cache-dir <path>` | auto | Source cache directory when it is not the default for `--from-project`. |
| `--dry-run` | off | Report whether the cache rehydrate is safe without copying. |
| `--format <markdown|json>` | `markdown` | Human or automation output. |

## Agent Path

1. Run `cache rehydrate --from-project <parent> --project <child>` before the
   first child-thread index.
2. If status is `rehydrated`, run the printed `doctor` command to inspect index
   freshness under the child worktree path.
3. Run the printed `retrieval index --refresh full` command before using
   packet/search as agent-facing retrieval evidence. Retrieval manifests are
   invalidated because retrieval generation ids are currently project-root
   derived. Portable v2 index artifact cache rows are preserved; older artifact
   rows are invalidated because they predate the portable-key contract.
4. If status is `skipped`, use the printed normal rebuild commands.

## Safety Contract

Rehydrate requires clean source and target worktrees, matching `origin` URLs,
matching Git tree ids, a source SQLite schema matching the running CLI, at
least one indexed source file, and an empty target cache directory. This command
preserves and rebases SQLite graph/search/doc rows, preserves portable v2 index
artifact cache rows, and invalidates retrieval generations across
worktree-root-derived project ids. It also does not configure Rust compilation
cache such as `sccache`.
