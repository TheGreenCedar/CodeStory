# `refs` - List Live Definitions, Sites, and Referenced Symbols

Finds definitions, call/reference sites (filtered by optional `--file-glob`),
1-hop referenced callees, and co-change siblings for a symbol name using current
working-tree bytes enriched by matching Core graph facts.

## Usage

```sh
cs refs SymbolName --file-glob "crates/**"
codestory-cli refs --project <repo> --limit 30 --no-bodies SymbolName
```

Over `CODESTORY_MCP_TOOL_SURFACE=lean`, call `refs` with `{ project, name, file_globs?, limit?, no_bodies?, max_bytes? }`.

See [generated CLI syntax](generated-cli-syntax.md) and [generated MCP syntax](generated-mcp-syntax.md).
