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
| Normal path | `target/release/codestory-cli(.exe) doctor --project .` | Reports project root, cache path, indexed stats, retrieval state, managed embedding setup, environment hints, and next commands. |
| Failure path | If cache or index checks warn, run `index --project . --refresh full`; if managed embeddings are missing or stopped, run `setup embeddings --project .`; if semantic retrieval is still not ready, continue with symbolic fallback unless the task specifically needs semantic proof. | Separates missing index, missing model runtime, and optional symbolic fallback. |
| Integration edge | Use doctor before `ground`, `search --why`, `explore`, `ask`, or `serve`; its next commands are the safe follow-up loop. | Prevents read commands from silently querying the wrong or empty cache. |

## Notes

- `doctor` does not accept `--refresh`; it is a read-only health surface.
- Environment rows report retrieval-related variables such as `CODESTORY_EMBED_PROFILE`, `CODESTORY_EMBED_BACKEND`, and `CODESTORY_EMBED_RUNTIME_MODE`.
- The `managed_embeddings` check distinguishes `missing_managed_assets`, `managed_server_stopped`, external llama endpoint state, and disabled/hash mode.
- Prefer JSON for CI or doc-contract checks.
