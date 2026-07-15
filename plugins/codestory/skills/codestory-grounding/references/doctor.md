# `doctor` - Project And Retrieval Health

Reads project/cache/index/retrieval health without mutating the index. Use it for maintainer/debug transcripts and when a read command fails unexpectedly. For agent MCP runtime truth, repair, and surface gating, see [status-contract.md](status-contract.md).

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
| Normal path | `<codestory-cli> doctor --project <target-workspace>` | Reports project root, cache path, indexed stats, retrieval state, in-process engine health, environment hints, and next commands. |
| Failure path | In MCP, retry the intended tool after its reported delay. If automatic preparation stops converging, continue with local navigation or ordinary source inspection and record the visibility gap. In a maintainer transcript, inspect `doctor` and run the specific index command it recommends; use an explicit full rebuild only when diagnostics identify stale or corrupt artifacts. If symbol docs, dense anchors, policy version, vector counts, or semantic health report partial/stale/failed state, do not trust broad evidence until the next complete publication is ready. | Separates user-facing capability state from maintainer diagnostics. |
| Integration edge | Use doctor before `ground`, `search --why`, `explore`, `context`, or `serve`; its next commands are the safe follow-up loop. | Prevents read commands from silently querying the wrong or empty cache. |

For MCP/runtime drift, collect binary evidence only after status is missing or
suspect (see [status-contract.md](status-contract.md#maintainer-recovery)). Installed
plugin MCP runtime changes require managed status/reinstall/reload, or an
explicit `CODESTORY_CLI` override for local development, before starting a fresh
Codex host/app session.

## Notes

- `doctor` does not accept `--refresh`; it is a read-only health surface.
- The `attention:` block repeats warnings first so agents do not miss semantic partial/stale/failure messages buried in the full check list.
- Environment rows report the explicit `CODESTORY_EMBED_ALLOW_CPU` policy when set.
- Maintainer JSON identifies the exact model digest, linked ggml build, selected backend and physical adapter, live smoke timing, and process engine identity. Ordinary tool UX reports only whether retrieval is ready.
- Treat `semantic ok` plus `retrieval_mode=full` as the health state suitable for broad repository explanation prompts. Under `graph_first_v1`, a zero dense-anchor count is valid only when graph and lexical artifacts are current. Treat `semantic partial`, `semantic stale`, `semantic failed`, vector-count mismatch, and non-`full` retrieval modes as unavailable broad evidence until automatic preparation or a maintainer-directed rebuild publishes a complete generation.
- Prefer JSON for CI or doc-contract checks.
