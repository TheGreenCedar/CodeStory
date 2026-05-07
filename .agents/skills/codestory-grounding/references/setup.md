# `setup` - Managed Local Assets

Installs explicit local assets that normal read/index commands should not surprise-download.

## Usage

```
target/release/codestory-cli(.exe) setup embeddings [OPTIONS]
```

## Options

| Option | Default | Use |
|--------|---------|-----|
| `--project <path>` | `.` | Project used to resolve cache configuration. Always pass it explicitly. |
| `--cache-dir <path>` | auto | Use an isolated cache root, useful for tests and repros. |
| `--quant <q8_0|q4_k_m>` | `q8_0` | Choose the managed BGE-base GGUF quantization. |
| `--variant <cpu|vulkan>` | `vulkan` | Choose the pinned llama.cpp binary variant. Use `cpu` as the fallback when Vulkan cannot start on the machine. |
| `--dry-run` | off | Show URLs, checksums, and paths without downloading or starting anything. |
| `--no-start` | off | Install and verify assets without starting `llama-server`. |
| `--format <markdown|json>` | `markdown` | Human or automation output. |
| `--output-file <path>` | stdout | Write output to an existing parent directory. |

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | `target/release/codestory-cli(.exe) setup embeddings --project .` | Downloads pinned Vulkan llama.cpp and BGE-base GGUF assets into the user cache, verifies checksums, extracts safely, and starts `llama-server` at the default endpoint. |
| Failure path | If setup fails, run `setup embeddings --project . --dry-run --format json` and inspect the selected platform asset, URL, cache root, and checksum. | Separates platform support, download, checksum, extraction, and server-start failures. |
| Integration edge | Run `doctor --project .` after setup, then `index --project . --refresh full` when semantic docs need the managed runtime. | Keeps first-run model setup explicit and auditable. |

## Notes

- Normal commands may start already installed managed assets, but they do not download missing assets.
- Use `setup embeddings --project . --variant cpu` when Vulkan startup fails or the platform has no pinned Vulkan asset.
- Set `CODESTORY_EMBED_RUNTIME_MODE=hash` for deterministic local-dev checks without llama.cpp.
- Set `CODESTORY_EMBED_LLAMACPP_URL` to use an external llama.cpp server instead of the managed endpoint.
