# `doctor` - Project And Retrieval Health

Reads project/cache/index/retrieval health without mutating the index. Use it at the start of an LLM browser loop and when a read command fails unexpectedly.

## Usage

```
<codestory-cli> doctor [OPTIONS]
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
| Normal path | `<codestory-cli> doctor --project <target-workspace>` | Reports project root, cache path, indexed stats, retrieval state, managed embedding setup, environment hints, and next commands. |
| Failure path | If cache or index checks warn, run `index --project <target-workspace> --refresh full`; if managed embeddings are missing, run `setup embeddings --project <target-workspace>`; if semantic reports `semantic partial`, `semantic stale`, or `semantic failed`, rebuild before `context` or continue with `search --repo-text on --why` plus focused `symbol`/`trail`/`snippet`. | Separates missing index, missing managed assets, stale semantic docs, partial semantic docs, and lexical fallback. |
| Integration edge | Use doctor before `ground`, `search --why`, `explore`, `context`, or `serve`; its next commands are the safe follow-up loop. | Prevents read commands from silently querying the wrong or empty cache. |

## Notes

- `doctor` does not accept `--refresh`; it is a read-only health surface.
- The `attention:` block repeats warnings first so agents do not miss semantic partial/stale/failure messages buried in the full check list.
- Environment rows report retrieval-related variables such as `CODESTORY_EMBED_PROFILE`, `CODESTORY_EMBED_BACKEND`, and `CODESTORY_EMBED_RUNTIME_MODE`.
- The `managed_embeddings` check distinguishes missing managed ONNX assets, installed assets, disabled/hash mode, and intentionally selected external legacy llama.cpp backend state.
- Treat `semantic ok` as the only health state suitable for broad repository explanation prompts. Treat `semantic partial`, `semantic stale`, and `semantic failed` as instructions to rebuild or use lexical/repo-text fallback.
- Prefer JSON for CI or doc-contract checks.
