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
| Normal path | `<codestory-cli> doctor --project <target-workspace>` | Reports project root, cache path, indexed stats, retrieval state, sidecar embedding setup, environment hints, and next commands. |
| Failure path | If cache or index checks warn, run `index --project <target-workspace> --refresh full`; if mandatory sidecars are missing or stale, run the setup/index commands surfaced by `doctor`; if semantic reports `semantic partial`, `semantic stale`, or `semantic failed`, rebuild before trusting broad packet/search evidence. | Separates missing index, stale semantic docs, partial semantic docs, and mandatory retrieval setup failures. |
| Integration edge | Use doctor before `ground`, `search --why`, `explore`, `context`, or `serve`; its next commands are the safe follow-up loop. | Prevents read commands from silently querying the wrong or empty cache. |

## Notes

- `doctor` does not accept `--refresh`; it is a read-only health surface.
- The `attention:` block repeats warnings first so agents do not miss semantic partial/stale/failure messages buried in the full check list.
- Environment rows report retrieval-related variables such as `CODESTORY_EMBED_BACKEND`, `CODESTORY_EMBED_LLAMACPP_URL`, and sidecar enablement flags.
- The embedding checks distinguish product llama.cpp sidecar state from hash, ONNX, disabled, or stale diagnostic states.
- Treat `semantic ok` plus `retrieval_mode=full` as the health state suitable for broad repository explanation prompts. Treat `semantic partial`, `semantic stale`, `semantic failed`, and non-`full` retrieval modes as instructions to repair setup or rebuild before trusting agent-facing evidence.
- Prefer JSON for CI or doc-contract checks.
