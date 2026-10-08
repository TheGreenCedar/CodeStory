# `snippet` — Fetch Source Code Context Around a Symbol

Resolves a symbol and returns its source code with surrounding context lines. Useful for reading the actual implementation without opening the full file.

Use the host's ordinary file reads whenever they are more useful, including
for named files and source artifacts outside the index.

Markdown output uses ANSI syntax highlighting when stdout is an interactive terminal. Output files, pipes, and JSON output stay uncolored for automation.

## Syntax

See [generated MCP syntax](generated-mcp-syntax.md) for live fields. Do not send
CLI flags. Every call requires `project` (absolute repository root).

## Output

Markdown output includes `context: scope=<line_context|function_body> requested_lines=<n> max_snippet_bytes=<bytes>`. JSON includes the same `scope`, `requested_context`, `snippet_truncated`, and `max_snippet_bytes` fields, plus `range_source`, `fallback_reason`, and `truncation_guidance` when applicable. If `snippet_truncated` is true, the byte cap stopped the output; follow `truncation_guidance` rather than assuming a larger `context` will reveal the omitted code.

When MCP `scope` is `function_body` (or `function_body: true`), snippet prefers
an implementation/body-looking function or method hit over a declaration-looking
hit when possible. If indexed source ranges are missing or suspicious, supported
brace languages attempt a bounded brace-balanced fallback before degrading. If
fallback fails, output keeps `scope=line_context` and reports the fallback
reason explicitly.

The MCP `snippet` tool exposes `scope=line_context|function_body` and
`context=0..200`. `lines` aliases `context`, and `function_body` boolean aliases
`scope`. Pass only one member of each alias pair; unknown or conflicting fields
fail instead of being ignored.

A snippet resolved by stable symbol id is a source-backed read: while a refresh
is running it waits for the fresh complete index inside the call's deadline
instead of returning pre-refresh bytes, and returns `preparing` when the
deadline expires first. The `codestory://snippet/{id}` resource stays
observational — it never activates or waits, and reports `source_stale` when
the indexed bytes no longer match the file.

```
# Snippet
resolved: `AppController::new` -> [abc123] new [FUNCTION] `src/lib.rs`:100
file: `src/lib.rs`  lines: 96–115
context: requested_lines=4 max_snippet_bytes=20000

    96: // --- AppController ---
    97:
    98: impl AppController {
    99:     /// Creates a new controller instance.
   100:     pub fn new() -> Self {
   101:         Self {
   102:             storage: None,
   103:             event_bus: EventBus::new(),
   104:         }
   105:     }
   106:
   107:     /// Opens a project from the given root.
   108:     pub fn open_project(&mut self, root: PathBuf) -> Result<()> {
```

Pass `query` or `id` (or `paths` / `path` / `file_path` / `symbol_id`). There is
no `file` or `output_file` field.


## Exact path windows

Use `files` to locate paths when the question names files or directories; use
symbol search when it names a symbol. Feed exact returned paths to `snippet.paths`
without treating filenames as symbols. For example, after inventory returns
`src/parser.rs`, request `{"project":"/absolute/repo","paths":[{"path":"src/parser.rs","start_line":1,"end_line":80}]}`.

Keep the exact returned path, opaque ID and match coordinates while choosing
source for the connection the task still needs. For a relevant callable,
prefer an existing reliable body extent with `scope=function_body`; check its
range source, fallback, freshness and truncation fields. The bounded brace
fallback qualifies supported lexical syntax, and unresolved or ambiguous
closure reports line context; retain that limitation. A requested hydration
range is not automatically a complete symbol or body. If resolution selects a
more useful implementation, retain both the original anchor and resolved source
identity. A complete body does not establish the surrounding callers or uses.

When trustworthy structural extent is unavailable, a first 200-line fallback
preview starts at `max(1, anchor_start_line - 100)` and ends 199 lines later.
Keep an unknown anchor unknown. Interpret each read using its actual delivered
bounds; a complete window does not establish an enclosing block or file.

After a read, identify the missing relationship before requesting more source.
Use visible references and existing relationship or targeted lookup operations
to locate the relevant caller, called definition, registration, guard or parent.
Inspect that source rather than expanding every discovered lead. Batch
independent known path windows in `snippet.paths` or an existing host read when
the current source and response caps allow it. Keep receipts for every range
and leave capped or unavailable relationships explicit. This selection strategy
adds no parser, new relationship type, repository-specific rule or larger budget.

Each requested range receives a receipt. `start_line` and `end_line` describe the
request; nullable `delivered_start_line` and `delivered_end_line` describe the
source actually returned. `complete` means the requested window reached its end
or natural EOF, not that the file or repository was completely inspected.
`source_total_lines` is null unless this read observed EOF. `source_kind` is
`workspace_read`; `source_range_sha256` hashes only delivered raw source bytes,
including original LF/CRLF endings, and is not an indexed whole-file digest.
`last_line_terminated` preserves whether the final delivered physical line had LF.

The shared source allowance is 64KiB; structured result metadata and JSON escaping
have a separate 128KiB bound. Host text delivers all range receipts and the same
whole source lines under a dedicated bound; the generic summary cap does not
clip this operation. A metadata overflow returns `source_response_too_large`.
Empty or naturally EOF-clipped windows can be complete without source lines.

For an opt-in whole-response bound, pass `max_tool_result_bytes=256..524288`
with `paths`, `path` or `file_path`. The allowance measures the UTF-8 JSON
serialization of the served MCP tool result, including range receipts,
publication metadata and any revision-specific text/structured-content mirror.
It excludes the outer JSON-RPC frame and unexposed host framing. This field is
not accepted with symbol/query snippets; omit it to keep existing defaults.

For example, an exact path window can use
`{"project":"/absolute/repo","paths":[{"path":"src/parser.rs","start_line":1,"end_line":80}],"max_tool_result_bytes":4096}`.
The server reduces the shared source allowance until the complete result fits,
keeping whole physical lines and every requested range receipt. It does not
promise to fill the allowance. Continue incomplete ranges using their actual
`next_start_line`; delivered hashes still cover only returned raw source bytes.
If even zero-source receipts cannot fit, `source_response_budget_too_small`
returns a bounded semantic error. An early error whose details exceed the
allowance is replaced with `source_response_error_exceeds_budget`; increase the
allowance where permitted, or narrow the request. JSON-RPC protocol errors
are outside this tool-result bound. A single-call allowance does not enforce a
cumulative investigation budget: charge each attempted result and reserve room
for subsequent calls and any unknown framing.

When `complete=false`, continue at `next_start_line`, retaining the same path and
requested end. `line_exceeds_byte_budget` means a physical line cannot fit; a
larger line-oriented request will not resolve it. Use an appropriate ordinary
host file read and qualify any partial line instead. Budget-exhausted later ranges
remain visible as receipts rather than disappearing.

After reading a body, follow relevant called definitions and regression
assertions using returned paths or actual returned symbol IDs. Use existing
`scope=function_body` for a symbol body; its range-source/fallback and publication
contract remain separate from this workspace path-read contract. No mandatory
packet or status call precedes these reads.

## Response byte accounting

A source allowance, a serialized tool result and model display are different
measurements. The source and structured-result caps above do not bound a whole
investigation. Some MCP revisions mirror JSON in text and structured content;
account for both and their framing when a task limits returned response bytes.
Deduplicating display does not reduce the received wrapper.

In code-mode hosts, this optional counter uses ordinary JavaScript without
requiring `TextEncoder` or Node's `Buffer`:

```javascript
function returnedWrapperUtf8Bytes(result) {
  const serialized = JSON.stringify(result);
  if (typeof serialized !== "string") throw new Error("result_not_serializable");
  let bytes = 0;
  for (const character of serialized) {
    const codepoint = character.codePointAt(0);
    bytes += codepoint <= 0x7f ? 1 : codepoint <= 0x7ff ? 2 : codepoint <= 0xffff ? 3 : 4;
  }
  return bytes;
}
const returnedWrapperBytes = returnedWrapperUtf8Bytes(result);
// Retain result and record this size with the attempted call before display.
```

For an ordinary JSON-data tool wrapper, this measures the UTF-8 bytes of
`JSON.stringify(result)`. JavaScript serialization can omit non-JSON members or
invoke getters and `toJSON`; this is not a lossless wire counter for arbitrary
objects. It does not measure unexposed protocol framing, host envelopes or token
usage. Keep those unknown when they are not captured.
Retain the original result before formatting; if capture or sizing fails, count
the attempted call and record its byte amount as unknown rather than zero.

Before another call, compare its expected complete response cost with the
remaining task allowance. Prefer an evidence-derived body or smaller known path
windows over a wide file read when that resolves the missing relationship.
Batching reduces calls, but source windows and mirrored result metadata still
consume bytes. Follow actual delivery receipts for further pages; do not reopen
an exhausted or closed evaluation to complete a missing body.
