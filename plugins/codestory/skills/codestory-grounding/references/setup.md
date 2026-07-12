# `setup` - Local Retrieval Assets

Prepares explicit local assets that normal read/index commands should not
surprise-download. Agent-facing packet/search evidence still requires
`retrieval bootstrap`, `retrieval index`, and `retrieval_mode=full`.

## Usage

```
<codestory-cli> setup embeddings [OPTIONS]
```

## Options

| Option | Default | Use |
|--------|---------|-----|
| `--project <path>` | `.` | Target workspace used to resolve cache configuration. Always pass it explicitly. |
| `--cache-dir <path>` | auto | Use an isolated cache root, useful for tests and repros. |
| `--quant <q8_0|q4_k_m>` | `q8_0` | Legacy compatibility selector. `setup embeddings` installs diagnostic ONNX assets only; GGUF llama.cpp sidecar model setup is handled by the retrieval sidecar setup path. |
| `--variant <cpu|vulkan>` | `vulkan` | Compatibility selector for older setup flows; product sidecar use is governed by `retrieval bootstrap`. |
| `--dry-run` | off | Show the asset plan without downloading anything. |
| `--no-start` | off | Compatibility flag; product setup is handled by retrieval sidecar bootstrap. |
| `--format <markdown|json>` | `markdown` | Human or automation output. |
| `--output-file <path>` | stdout | Write output to an existing parent directory. |

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Product packet/search setup | Follow `docs/ops/retrieval-sidecars.md`, then run `retrieval bootstrap`, `retrieval index`, and `retrieval status` for the target workspace. | Product search/packet paths are usable only when status reports `retrieval_mode=full`. |
| Legacy diagnostic assets | `setup embeddings --project <target-workspace> --dry-run --format json`, then `setup embeddings --project <target-workspace>` if the plan is expected. | Installs local ONNX assets for semantic diagnostics; it does not start sidecars, set product defaults, or prove packet/search readiness. |
| Failure path | Use the sidecar runbook for llama.cpp/GGUF/manifest failures; use `setup embeddings --dry-run --format json` only for legacy managed asset diagnosis. | Keeps product sidecar setup separate from legacy asset setup. |

## Notes

- Normal commands may use already installed assets, but they do not download missing assets.
- Release and default source builds exclude the legacy ONNX runtime. Maintainers
  who need to execute or verify those diagnostic assets must build from source
  with `cargo build -p codestory-cli --features diagnostic-onnx`; product
  packet/search uses the llama.cpp sidecar and does not require this feature.
- Plain `index` builds the core SQLite code index only; run `retrieval index` after sidecars are configured. Packet/search readiness: [status-contract.md](status-contract.md) and `docs/ops/retrieval-sidecars.md`.
- Hash embeddings, ONNX-only flows, and non-sidecar embedding paths are diagnostic or historical comparison modes only.
