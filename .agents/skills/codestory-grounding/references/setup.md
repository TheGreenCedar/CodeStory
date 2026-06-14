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
| `--quant <q8_0|q4_k_m>` | `q8_0` | Legacy compatibility selector. Managed `setup embeddings` installs pinned ONNX assets for the local semantic runtime; GGUF llama.cpp sidecar model setup is handled by the retrieval sidecar setup path. |
| `--variant <cpu|vulkan>` | `vulkan` | Compatibility selector for older setup flows; product sidecar use is governed by `retrieval bootstrap`. |
| `--dry-run` | off | Show the asset plan without downloading anything. |
| `--no-start` | off | Compatibility flag; product setup is handled by retrieval sidecar bootstrap. |
| `--format <markdown|json>` | `markdown` | Human or automation output. |
| `--output-file <path>` | stdout | Write output to an existing parent directory. |

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | `node scripts/setup-retrieval-env.mjs --fetch-embed-model`, then `<codestory-cli> retrieval bootstrap --project <target-workspace>` | Downloads the pinned bge-base GGUF for the local llama.cpp sidecar, verifies the artifact size/SHA-256 before accepting it, starts local sidecars, and prepares the product retrieval environment. |
| Failure path | If setup fails, run `setup embeddings --project <target-workspace> --dry-run --format json` and inspect the selected asset URLs, cache root, output paths, and checksums. | Separates platform support, download, checksum, extraction, and sidecar-readiness failures. |
| Integration edge | Run `retrieval index --project <target-workspace> --refresh full`, then `retrieval status --project <target-workspace> --format json`. | Product search/packet paths are usable only when status reports `retrieval_mode=full`. |

## Notes

- Normal commands may use already installed assets, but they do not download
  missing assets.
- Plain `index` builds the core SQLite code index only. Run `retrieval index`
  after sidecars are configured, then require `retrieval status --format json`
  to report `retrieval_mode=full` before relying on packet/search evidence.
- Product sidecar evidence requires `CODESTORY_EMBED_BACKEND=llamacpp`, the
  local llama.cpp endpoint, and a manifest embedding backend of
  `llamacpp:bge-base-en-v1.5`.
- The retrieval setup wrapper accepts only `bge-base-en-v1.5.Q8_0.gguf` files
  matching size `117974304` and SHA-256
  `ad1afe72cd6654a558667a3db10878b049a75bfd72912e1dabb91310d671173c`; fallback
  URLs are mirror candidates gated by that same checksum.
- Hash embeddings, ONNX-only flows, and non-sidecar embedding paths are
  diagnostic or historical comparison modes only.
