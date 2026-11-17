# `slice` - Pack Multi-File Declarations Within Budget

Packs query-relevant declarations, 1-hop callees, and co-change test/fixture
companions from live files within `--max-bytes` (default 7,500 bytes). Uses the
pinned Core publication when live file hashes match, and falls back smoothly to
single-file tree-sitter AST extraction and 1-hop live import/callee discovery on
unindexed checkouts without whole-tree freshness walks.

## Usage

```sh
cs slice "how does request validation reach runtime?"
codestory-cli slice --project <repo> --max-bytes 7500 "QUESTION"
```

Over `CODESTORY_MCP_TOOL_SURFACE=lean`, call `slice` with `{ project, question, max_bytes? }`.

See [generated CLI syntax](generated-cli-syntax.md) and [generated MCP syntax](generated-mcp-syntax.md).
