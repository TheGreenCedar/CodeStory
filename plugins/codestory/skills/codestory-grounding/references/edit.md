# `edit` - Atomic Syntax-Validated Declaration or Line-Range Edit

Replaces an exact `path::Symbol`, `path:LINE`, or `path:A-B` target atomically
only after tree-sitter syntax validation passes on the resulting file bytes. If
the selector is missing/ambiguous or the replacement introduces a syntax error,
`edit` fails closed without modifying the file.

## Usage

```sh
cs edit src/lib.rs::my_fn --stdin <<'EOF'
pub fn my_fn() -> bool {
    true
}
EOF
codestory-cli edit --project <repo> src/lib.rs:10-14 --replacement "..."
```

Over `CODESTORY_MCP_TOOL_SURFACE=lean`, call `edit` with `{ project, selector, replacement }`.

See [generated CLI syntax](generated-cli-syntax.md) and [generated MCP syntax](generated-mcp-syntax.md).
