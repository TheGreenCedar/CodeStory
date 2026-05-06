# `doctor` - Project And Retrieval Health

Reads project/cache/index/retrieval health without mutating the index. Use it at the start of an LLM browser loop and when a read command fails unexpectedly.

## Usage

```
target/release/codestory-cli(.exe) doctor [OPTIONS]
```

## Options

| Option | Default | Use |
|--------|---------|-----|
| `--project <path>` | `.` | Repository root to inspect. Always pass it explicitly. |
| `--cache-dir <path>` | auto | Inspect a specific cache directory. |
| `--format <markdown|json>` | `markdown` | Human or automation output. |
| `--output-file <path>` | stdout | Write the report to an existing parent directory. |

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | `target/release/codestory-cli(.exe) doctor --project .` | Reports project root, cache path, indexed stats, retrieval state, environment hints, and next commands. |
| Failure path | If cache or index checks warn, run `index --project . --refresh full`; if semantic retrieval is not ready, continue with symbolic fallback unless the task specifically needs semantic proof. | Separates missing index from optional semantic fallback. |
| Integration edge | Use doctor before `ground`, `search --why`, `explore`, `ask`, or `serve`; its next commands are the safe follow-up loop. | Prevents read commands from silently querying the wrong or empty cache. |

## Notes

- `doctor` does not accept `--refresh`; it is a read-only health surface.
- Environment rows report retrieval-related variables such as `CODESTORY_EMBED_PROFILE`, `CODESTORY_EMBED_BACKEND`, and `CODESTORY_EMBED_RUNTIME_MODE`.
- Prefer JSON for CI or doc-contract checks.
