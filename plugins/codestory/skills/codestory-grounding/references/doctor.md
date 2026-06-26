# `doctor` - Project And Retrieval Health

Reads project/cache/index/retrieval health without mutating the index. Use it at the start of an LLM browser loop and when a read command fails unexpectedly. For MCP runtime truth and surface gating, see [status-contract.md](status-contract.md). Use `doctor` when MCP is missing, a repair transcript is needed, or status points to stale evidence.

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
| Failure path | If cache or index checks warn, run `ready --goal local --repair --project <target-workspace> --format json` or the setup/index command surfaced by `doctor`; use explicit `index --refresh full` only when the reported failure calls for a rebuild. If mandatory sidecars are missing or stale, run the sidecar setup/index commands surfaced by `doctor`; if symbol docs, dense anchors, policy version, Qdrant counts, or semantic health report partial/stale/failed state, repair before trusting broad packet/search evidence. | Separates missing index, stale symbol docs, partial dense anchors, and mandatory retrieval setup failures. |
| Integration edge | Use doctor before `ground`, `search --why`, `explore`, `context`, or `serve`; its next commands are the safe follow-up loop. | Prevents read commands from silently querying the wrong or empty cache. |

For MCP/runtime drift, collect binary evidence only after status is missing or
suspect (see [status-contract.md](status-contract.md#runtime-repair)):

```powershell
where.exe codestory-cli
codestory-cli --version
codestory-cli doctor --project <target-workspace> --format json
```

If PATH repair changes which binary `codestory-cli` resolves to, start a fresh
Codex host/app session before treating the plugin MCP runtime as updated.

## Notes

- `doctor` does not accept `--refresh`; it is a read-only health surface.
- The `attention:` block repeats warnings first so agents do not miss semantic partial/stale/failure messages buried in the full check list.
- Environment rows report retrieval-related variables such as `CODESTORY_EMBED_BACKEND`, `CODESTORY_EMBED_LLAMACPP_URL`, and sidecar enablement flags.
- The embedding checks distinguish product llama.cpp sidecar state from hash, ONNX, disabled, or stale diagnostic states.
- Treat `semantic ok` plus `retrieval_mode=full` as the health state suitable for broad repository explanation prompts. Under `graph_first_v1`, `full` may explicitly skip Qdrant only when dense-anchor count is zero and graph/lexical artifacts are current. Treat `semantic partial`, `semantic stale`, `semantic failed`, Qdrant count mismatch, and non-`full` retrieval modes as instructions to repair setup or rebuild before trusting agent-facing evidence.
- Prefer JSON for CI or doc-contract checks.
