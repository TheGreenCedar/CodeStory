# `cache` - Worktree Cache Bootstrap

Checks whether a parent worktree cache is compatible with a child worktree,
snapshots the SQLite cache shell, and clears path-bound index/search surfaces so
the child rebuilds them under its own worktree path.

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
| `--dry-run` | off | Report whether the cache shell copy is safe without copying. |
| `--format <markdown|json>` | `markdown` | Human or automation output. |

## Agent Path

1. Run `cache rehydrate --from-project <parent> --project <child>` before the
   first child-thread index.
2. If status is `prepared`, run the printed `index --refresh full` command. The
   command clears copied path-bound file/node/search/doc/artifact rows.
3. Run the printed `retrieval index --refresh full` command before using
   packet/search as agent-facing sidecar evidence.
4. If status is `skipped`, use the printed normal rebuild commands.

## Safety Contract

Preparation requires clean source and target worktrees, matching `origin` URLs,
matching Git tree ids, a source SQLite schema matching the running CLI, at
least one indexed source file, and an empty target cache directory. This command
does not preserve the path-bound CodeStory index yet; true #82 cache reuse still
needs path rebasing for absolute file/node/doc/artifact rows. It also does not
configure Rust compilation cache such as `sccache`.
