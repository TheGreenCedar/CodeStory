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
`plugins/codestory/skills/codestory-grounding/references/*.md`. User guides in
`docs/users/` should stay workflow-oriented and link to those sources instead of
copying complete option matrices.

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

Generated help and [../../users/cli-reference.md](../../users/cli-reference.md)
own command syntax, workflow examples, and option semantics. This subsystem
page owns the adapter boundary:

- parse CLI arguments without embedding runtime policy;
- keep rendering in `output.rs`;
- delegate refresh, retrieval, packet sufficiency, query planning, and health
  checks to `codestory-runtime`;
- report stale, partial, ambiguous, or degraded evidence instead of silently
  hiding it;
- keep mutating setup paths explicit so read commands do not download assets or
  change sidecar state.

Broad question surfaces (`packet`, sidecar-backed `search`, and `drill`) should
remain separate from exact target context (`context`, `symbol`, `trail`,
`snippet`, and local graph exploration). Generated help is the source of truth
for the current flags on each command.

`task brief` is an owner-directed implementation workflow view over `packet`.
It must keep the stable JSON and Markdown brief contracts in the CLI adapter,
reuse packet sufficiency/citations/follow-up commands, and avoid adding storage
or separate `scout`, `where`, or `onboard` implementations in this slice.

## Search And Context Research Boundary

`codestory-cli search` and `codestory-cli context` keep production behavior on
runtime defaults; public hybrid tuning flags have been removed. Internal
packet and retrieval planning may still use hybrid weights, but the CLI does
not expose ranking knobs for product search/context calls.

## Serving And Integration Surface

HTTP serving keeps the current small GET/query-string shape. It is local-only by default: non-loopback binds and non-loopback `Host`/`Origin` headers are rejected unless the operator passes `--allow-non-loopback` behind an intentional network boundary. The stable routes are `/health`, `/search`, `/symbol`, `/definition`, `/references`, `/symbols`, and `/trail`. Definition and references accept either `q` or `id`, so agents can resolve from a query first and then reuse exact node ids.

`serve --stdio` is MCP-style JSON lines. It exposes tools for ground, files, affected, packet, search, context, symbol, callers, callees, trace, trail, definition, references, symbols, snippet, and warm graph primitives (`get_node`, `neighbors`, `shortest_path`, and `query_subgraph`); resources for project, grounding, and root symbols; resource templates for node-specific symbol/reference/snippet/trail reads; and prompts for explain-symbol, callflow tracing, and impact analysis.

The warm graph primitives are intentionally narrower than `packet`. They resolve exact node ids or bounded local graph neighborhoods before an agent asks for a broad evidence packet. Their responses include stable node ids, project-relative file refs when available, certainty metadata, count/truncation fields, and explicit result limits. `packet` remains the broad task tool with sufficiency, citations, retrieval traces, and budget accounting.

## Failure Signatures

- CLI depends directly on `codestory-store` or `codestory-indexer`
- output helpers start opening files or stores on their own
- command-specific orchestration is copied instead of delegated
