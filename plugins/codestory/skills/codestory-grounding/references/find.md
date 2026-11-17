# `find` - Search Live Literal Text With Enclosing Declarations

Searches one or more literal terms across the live workspace with optional path
filtering (`--file-glob`) and returns grouped matches inside their enclosing
declaration bodies (up to the explicit byte budget; default 7,500 bytes).

## Usage

```sh
cs find TERM1 TERM2 --file-glob "crates/codestory-runtime/**"
codestory-cli find --project <repo> --file-glob "*.rs" --no-bodies TERM
```

Over `CODESTORY_MCP_TOOL_SURFACE=lean`, call `find` with `{ project, terms, file_globs?, limit?, no_bodies?, max_bytes? }`.

See [generated CLI syntax](generated-cli-syntax.md) and [generated MCP syntax](generated-mcp-syntax.md).
