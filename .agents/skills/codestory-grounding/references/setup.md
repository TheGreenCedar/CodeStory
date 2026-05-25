# `setup` - Managed Local Embedding Assets

Installs explicit local assets that normal read/index commands should not surprise-download.

## Usage

```
<codestory-cli> setup embeddings [OPTIONS]
```

## Options

| Option | Default | Use |
|--------|---------|-----|
| `--project <path>` | `.` | Target workspace used to resolve cache configuration. Always pass it explicitly. |
| `--cache-dir <path>` | auto | Use an isolated cache root, useful for tests and repros. |
| `--quant <q8_0|q4_k_m>` | `q8_0` | Legacy GGUF selector retained for CLI compatibility; managed setup now installs the pinned ONNX model. |
| `--variant <cpu|vulkan>` | `vulkan` | Legacy llama.cpp selector retained for CLI compatibility; managed setup now uses ONNX Runtime. |
| `--dry-run` | off | Show the managed ONNX asset plan without downloading anything. |
| `--no-start` | off | Compatibility flag; managed ONNX setup never starts a server. |
| `--format <markdown|json>` | `markdown` | Human or automation output. |
| `--output-file <path>` | stdout | Write output to an existing parent directory. |

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | `<codestory-cli> setup embeddings --project <target-workspace>` | Downloads pinned Qdrant BGE-base ONNX graph and tokenizer assets into the user cache, verifies checksums, and derives the pooled ONNX graph that runtime uses for embeddings. |
| Failure path | If setup fails, run `setup embeddings --project <target-workspace> --dry-run --format json` and inspect the selected asset URLs, cache root, output paths, and checksums. | Separates platform support, download, checksum, extraction, and graph-derivation failures. |
| Integration edge | Run `doctor --project <target-workspace>` after setup, then `index --project <target-workspace> --refresh full` when semantic docs need the managed runtime. | Keeps first-run model setup explicit and auditable. |

## Notes

- Normal commands may use already installed managed assets, but they do not download missing assets.
- Managed setup seeds local defaults for ONNX Runtime: DirectML on Windows, CPU elsewhere, doc batch `2048`, token budget `32768`, and stored vectors `int8` unless environment variables override them.
- Set `CODESTORY_EMBED_RUNTIME_MODE=hash` for deterministic local-dev checks without real model inference.
- Set `CODESTORY_EMBED_BACKEND=llamacpp` and `CODESTORY_EMBED_LLAMACPP_URL` only when intentionally using an external legacy llama.cpp server.
