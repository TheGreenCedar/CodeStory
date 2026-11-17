# `read` - Read Bounded Live Files, Outlines, Symbols, or Line Ranges

Reads one or more selectors from live files inside an explicit byte budget
(`DEFAULT_READ_MAX_BYTES = 16,000` bytes; outlines are capped at 9,500 bytes).
A bare path prints small files whole or outlines large files; `path::Symbol`,
`path:LINE`, and `path:A-B` read exact targeted slices.

## Usage

```sh
cs read src/lib.rs::AppController::new
cs read crates/codestory-cli/src/app/proxy.rs:1-80
codestory-cli read --project <repo> --max-bytes 16000 path/to/file.rs
```

Over `CODESTORY_MCP_TOOL_SURFACE=lean`, call `read` with `{ project, selectors, max_bytes? }`.

See [generated CLI syntax](generated-cli-syntax.md) and [generated MCP syntax](generated-mcp-syntax.md).
