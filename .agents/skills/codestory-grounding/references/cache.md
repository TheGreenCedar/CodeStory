# `cache` - Worktree Cache Reuse

Copies a compatible CodeStory cache from one clean worktree into another so a
Codex child thread can avoid rebuilding the core grounding/index cache from
scratch.

## Usage

```text
<codestory-cli> cache rehydrate --from-project <parent-worktree> --project <child-worktree>
```

## Options

| Option | Default | Use |
|--------|---------|-----|
| `--project <path>` | `.` | Target child worktree. |
| `--cache-dir <path>` | auto | Target cache directory. Must be empty for reuse. |
| `--from-project <path>` | required | Source worktree with an existing CodeStory cache. |
| `--from-cache-dir <path>` | auto | Source cache directory when it is not the default for `--from-project`. |
| `--dry-run` | off | Report whether reuse is safe without copying. |
| `--format <markdown|json>` | `markdown` | Human or automation output. |

## Agent Path

1. Run `cache rehydrate --from-project <parent> --project <child>` before the
   first child-thread index.
2. If status is `reused`, run `doctor`, then `index --refresh incremental` if
   freshness is stale.
3. Run `retrieval index --refresh full` before using packet/search as
   agent-facing sidecar evidence; copied retrieval manifests are invalidated so
   sidecars rebuild under the child worktree identity.
4. If status is `skipped`, use the printed normal rebuild commands.

## Safety Contract

Reuse requires clean source and target worktrees, matching `origin` URLs,
matching Git tree ids, a source SQLite schema matching the running CLI, at
least one indexed source file, and an empty target cache directory. This command
reuses CodeStory cache artifacts only; it does not configure Rust compilation
cache such as `sccache`.
