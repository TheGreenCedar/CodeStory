# CLI Subsystem

`codestory-cli` is the thin adapter for indexing, grounding reads, DB-first target context packets, graph-query helpers, local exploration, health checks, and lightweight serving.

## Ownership

- parse command-line arguments
- resolve project and cache paths
- call runtime services
- render text or JSON

## Entry Points

- `crates/codestory-cli/src/main.rs`
- `crates/codestory-cli/src/args.rs`
- `crates/codestory-cli/src/runtime.rs`
- `crates/codestory-cli/src/output.rs`

## Extension Points

- add commands in `args.rs` and `main.rs`
- add renderers in `output.rs`
- keep business logic in runtime, not here

## Command Reference Ownership

This page documents CLI ownership and subsystem boundaries, not detailed option
semantics. The canonical option contract is the generated CLI help from
`crates/codestory-cli/src/args.rs`; the agent-facing operational reference is
`.agents/skills/codestory-grounding/references/*.md`. README and usage docs
should stay workflow-oriented and link to those sources instead of copying
complete option matrices.

Refresh behavior belongs to runtime, not the CLI adapter. The CLI parses the
requested refresh mode, resolves project/cache paths, delegates to runtime, and
renders the returned summary. Semantic indexing is part of the runtime-owned
index path when embedding assets are available.

## Configuration Files

The CLI loads optional `.codestory.toml` defaults from the user home directory and then from the selected project root. Project config may override home config for project-safe preferences. Cache roots, network endpoints, model selectors for source-egress calls, credentials, and source-text egress settings must come from trusted user config, explicit environment variables, or CLI options; project files cannot set `cache_dir`, `summary_endpoint`, `summary_model`, or embedding endpoint fields unless `CODESTORY_ALLOW_PROJECT_NETWORK_CONFIG=1` is set deliberately for that run. Explicit environment variables override both config files because config values are only applied when the matching runtime env var is absent.

Embedding config keys map to the runtime env names:

| `.codestory.toml` key | Runtime env var | Notes |
| --- | --- | --- |
| `embedding_profile` | `CODESTORY_EMBED_PROFILE` | Selects a built-in profile such as `bge-base-en-v1.5`, `bge-small-en-v1.5`, or `custom`. |
| `embedding_model_id` | `CODESTORY_EMBED_MODEL_ID` | Overrides the resolved model id for the selected profile. |
| `embedding_model` | `CODESTORY_EMBED_MODEL_ID` | Deprecated alias for `embedding_model_id`; prefer the explicit key in new config. |
| `embedding_endpoint` | `CODESTORY_EMBED_LLAMACPP_URL` | Trusted-only endpoint for the product llama.cpp embedding sidecar. |

The CLI should not set stale embedding env aliases that the runtime does not read.

Index output should expose:

- project and storage paths
- resolved refresh mode
- graph stats and retrieval state
- graph phase timings
- semantic timings and doc counts when semantic sync was considered
- resolution diagnostics when the indexer reports them

## Read And Query Output

Read commands default to `--refresh none` so they query the current cache unless
the caller requests a refresh.

`ground`, `search`, `context`, `symbol`, `trail`, `snippet`, `query`, and
`explore` all support `--format markdown|json` and `--output-file <PATH>`.
`trail` additionally supports Graphviz DOT via `--format dot`; `symbol` and
`trail` support Mermaid via `--mermaid`.

`search --query` accepts field-qualified filters such as `kind:function`,
`path:routes.ts`, `name:listUsers`, and `lang:typescript` for narrowing
candidate sets without hiding the original query text. `search --why` keeps
operator provenance compact by default; broad architecture-style `search`
responses include the full Search Plan only when `--plan-details` is also
requested. Treat that plan as discovery evidence and next-command guidance, not
final answer prose.

`drill` is the exception to the default refresh posture: it defaults to
`--refresh full` so generated report bundles are mechanically fresh. Its
agent-quality classification details live in the grounding skill references
rather than the general CLI architecture page.

`query` is intentionally small. It parses source operations (`search`, `symbol`, `trail`) followed by stream refinements (`filter`, `limit`) and rejects malformed or unknown named arguments rather than silently ignoring typos.

`context` is target context: DB-first evidence for one concrete target. It resolves that target from `--id`, `--query`, or `--bookmark`, delegates to `codestory-runtime` retrieval orchestration, includes citations and retrieval traces, and always uses DB-first synthesis. It is not a natural-language question-answering surface and is not a replacement for broad `packet`, `search`, or `drill` questions. `--bundle <DIR>` writes Markdown, JSON, and Mermaid artifacts for handoff.

`doctor` is a read-only health report for project path resolution, cache presence, index counts, retrieval state, managed embedding setup, relevant embedding environment variables, and next commands. It should stay diagnostic; it should not mutate caches or fetch model assets. `setup embeddings` is the explicit mutating path for installing pinned ONNX Runtime BGE-base assets in the user cache. Managed setup does not launch or retain an embedding server.

## Search And Context Research Boundary

`codestory-cli search` and `codestory-cli context` keep production behavior on
runtime defaults; public hybrid tuning flags have been removed. Internal
packet and retrieval planning may still use hybrid weights, but the CLI does
not expose ranking knobs for product search/context calls.

## Serving And Integration Surface

HTTP serving keeps the current small GET/query-string shape. The stable routes are `/health`, `/search`, `/symbol`, `/definition`, `/references`, `/symbols`, and `/trail`. Definition and references accept either `q` or `id`, so agents can resolve from a query first and then reuse exact node ids.

`serve --stdio` is MCP-style JSON lines. It exposes tools for search, context, symbol, trail, definition, references, symbols, snippet, and warm graph primitives (`get_node`, `neighbors`, `shortest_path`, and `query_subgraph`); resources for project, grounding, and root symbols; resource templates for node-specific symbol/reference/snippet/trail reads; and prompts for explain-symbol, callflow tracing, and impact analysis.

The warm graph primitives are intentionally narrower than `packet`. They resolve exact node ids or bounded local graph neighborhoods before an agent asks for a broad evidence packet. Their responses include stable node ids, project-relative file refs when available, certainty metadata, count/truncation fields, and explicit result limits. `packet` remains the broad task tool with sufficiency, citations, retrieval traces, and budget accounting.

## Failure Signatures

- CLI depends directly on `codestory-store` or `codestory-indexer`
- output helpers start opening files or stores on their own
- command-specific orchestration is copied instead of delegated
