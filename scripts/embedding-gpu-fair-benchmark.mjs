import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";

const root = process.env.CODESTORY_EMBED_RESEARCH_ROOT ?? process.env.CODESTORY_FAIR_BENCH_ROOT ?? process.cwd();
const bin =
  process.env.CODESTORY_EMBED_RESEARCH_BIN ??
  process.env.CODESTORY_FAIR_BENCH_BIN ??
  path.join(root, "target/release/codestory-cli.exe");
const llamaDir = process.env.CODESTORY_LLAMA_CPP_DIR ?? path.join(root, "target/llamacpp/b8840");
const llamaExe = process.env.CODESTORY_LLAMA_CPP_SERVER ?? path.join(llamaDir, "llama-server.exe");
const stamp = new Date().toISOString().replaceAll(/[-:]/g, "").replace(/\..+/, "");
const outDir =
  process.env.CODESTORY_EMBED_RESEARCH_OUT_DIR ??
  process.env.CODESTORY_FAIR_BENCH_OUT_DIR ??
  path.join(root, "target/embedding-research", stamp);
const cacheReplayFrom = process.env.CODESTORY_EMBED_RESEARCH_CACHE_FROM
  ? path.resolve(process.env.CODESTORY_EMBED_RESEARCH_CACHE_FROM)
  : "";

const requestedStages = new Set(
  (process.env.CODESTORY_EMBED_RESEARCH_STAGE ?? "smoke")
    .split(",")
    .map((stage) => stage.trim().toLowerCase())
    .filter(Boolean),
);
const selectedCaseIds = new Set(
  (process.env.CODESTORY_EMBED_RESEARCH_CASES ?? process.env.CODESTORY_FAIR_BENCH_PROFILES ?? "")
    .split(",")
    .map((id) => id.trim())
    .filter(Boolean),
);
const selectedQueryIds = new Set(
  (process.env.CODESTORY_EMBED_RESEARCH_QUERY_IDS ?? "")
    .split(",")
    .map((id) => id.trim())
    .filter(Boolean),
);
const selectedQueryBuckets = new Set(
  (process.env.CODESTORY_EMBED_RESEARCH_QUERY_BUCKETS ?? "")
    .split(",")
    .map((id) => id.trim())
    .filter(Boolean),
);
const queryLimit = parsePositiveIntEnv("CODESTORY_EMBED_RESEARCH_QUERY_LIMIT");
const portBase = Number(process.env.CODESTORY_EMBED_RESEARCH_PORT_BASE ?? 8170);

const blockedCandidates = [];

const researchSources = [
  {
    id: "sentence-transformers-embedding-quantization",
    url: "https://www.sbert.net/docs/package_reference/util/quantization.html",
    claim:
      "Embedding quantization is separate from model-weight quantization and supports float32, int8, uint8, binary, and ubinary corpus encodings with optional rescoring.",
  },
  {
    id: "llama-cpp-quantize",
    url: "https://github.com/ggml-org/llama.cpp/blob/master/tools/quantize/README.md",
    claim:
      "llama.cpp quantization reduces GGUF weight precision and model size, can affect quality, and can use imatrix calibration for better low-bit results.",
  },
  {
    id: "qdrant-vector-quantization",
    url: "https://qdrant.tech/documentation/manage-data/quantization/",
    claim:
      "Vector database quantization is a storage/search optimization lane distinct from embedding model inference.",
  },
  {
    id: "nomic-v15-matryoshka",
    url: "https://huggingface.co/nomic-ai/nomic-embed-text-v1.5",
    claim:
      "Nomic v1.5 documents Matryoshka dimensionality tradeoffs at 768, 512, 256, 128, and 64 dimensions.",
  },
  {
    id: "nomic-v2-moe",
    url: "https://huggingface.co/nomic-ai/nomic-embed-text-v2-moe",
    claim:
      "Nomic v2 MoE documents required search_query/search_document prefixes, a 512-token maximum input length, and Matryoshka truncation such as 256-dimensional embeddings.",
  },
  {
    id: "qwen3-embedding-06b",
    url: "https://huggingface.co/Qwen/Qwen3-Embedding-0.6B",
    claim:
      "Qwen3 0.6B documents 32k context, 1024-dimensional embeddings, MRL support, and user-defined output dimensions from 32 to 1024.",
  },
  {
    id: "embeddinggemma-300m-mrl",
    url: "https://huggingface.co/google/embeddinggemma-300m",
    claim:
      "EmbeddingGemma documents 768-dimensional output with Matryoshka options at 512, 256, and 128 dimensions after truncation and renormalization.",
  },
];

const precisionBytes = {
  float32: 4,
  float16: 2,
  int8: 1,
  uint8: 1,
  binary: 1 / 8,
  ubinary: 1 / 8,
};

const queries = [
  {
    id: "project-storage-open",
    query: "open a project with a specific sqlite database file",
    expect: ["open_project_with_storage_path"],
    bucket: "baseline",
  },
  {
    id: "grounding-overview",
    query: "make a compact grounding overview with coverage buckets and notes",
    expect: ["grounding_snapshot"],
    bucket: "baseline",
  },
  {
    id: "trail-neighborhood",
    query: "follow outgoing graph edges around a focus symbol",
    expect: ["trail_context", "graph_trail"],
    bucket: "persistent-miss",
  },
  {
    id: "semantic-sync",
    query: "synchronize persisted semantic documents after indexing",
    expect: ["sync_llm_symbol_projection"],
    bucket: "persistent-miss",
  },
  {
    id: "semantic-doc-text",
    query: "build the text that gets embedded for semantic search documents",
    expect: ["build_llm_symbol_doc_text"],
    bucket: "persistent-miss",
  },
  {
    id: "semantic-reload",
    query: "reload semantic documents from storage into the search engine",
    expect: ["reload_llm_docs_from_storage"],
    bucket: "baseline",
  },
  {
    id: "hybrid-enabled",
    query: "check whether hybrid retrieval is enabled by an environment flag",
    expect: ["hybrid_retrieval_enabled"],
    bucket: "baseline",
  },
  {
    id: "hybrid-weights",
    query: "normalize lexical semantic and graph weights for retrieval",
    expect: ["normalized_hybrid_weights"],
    bucket: "baseline",
  },
  {
    id: "search-rank",
    query: "rank search hits by exact matches symbol kind and score",
    expect: ["compare_search_hits", "search_match_rank"],
    bucket: "baseline",
  },
  {
    id: "natural-language-terms",
    query: "extract natural language search terms without stopwords",
    expect: ["extract_symbol_search_terms"],
    bucket: "baseline",
  },
  {
    id: "llamacpp-endpoint",
    query: "send an embeddings request to the local llama cpp server endpoint",
    expect: ["post_json_to_http_endpoint"],
    bucket: "baseline",
  },
  {
    id: "canonical-layout",
    query: "create a canonical graph layout for visualization",
    expect: ["build_canonical_layout"],
    bucket: "baseline",
  },
  {
    id: "refresh-plan",
    query: "prepare a workspace refresh plan from changed files",
    expect: ["build_refresh_plan"],
    bucket: "baseline",
  },
  {
    id: "index-file",
    query: "index one source file with tree sitter symbols and semantic edges",
    expect: ["index_file", "index_single_file"],
    bucket: "baseline",
  },
  {
    id: "search-markdown",
    query: "render CLI search results as markdown",
    expect: ["render_search_markdown"],
    bucket: "baseline",
  },
  {
    id: "resolve-target",
    query: "resolve a user query to the symbol id used by trail and snippet",
    expect: ["resolve_target", "targetargs::selection"],
    bucket: "persistent-miss",
  },
  {
    id: "search-json",
    query: "turn a search hit into JSON output with a relative path",
    expect: ["build_search_hit_output", "searchhitoutput"],
    bucket: "baseline",
  },
  {
    id: "camelcase-open-storage",
    query: "camel case openProjectWithStoragePath method for opening sqlite storage",
    expect: ["open_project_with_storage_path"],
    bucket: "alias-sensitive",
  },
  {
    id: "snakecase-doc-text",
    query: "snake case build_llm_symbol_doc_text semantic document builder",
    expect: ["build_llm_symbol_doc_text"],
    bucket: "alias-sensitive",
  },
  {
    id: "qualified-search-engine-runtime",
    query: "SearchEngine set embedding runtime from env qualified method",
    expect: ["SearchEngine::set_embedding_runtime_from_env", "set_embedding_runtime_from_env"],
    bucket: "qualified-name",
  },
  {
    id: "path-runtime-lib-sync",
    query: "crates codestory runtime src lib sync semantic docs",
    expect: ["sync_llm_symbol_projection"],
    bucket: "path-terms",
  },
  {
    id: "role-method-callable",
    query: "method member function callable routine open project storage path",
    expect: ["open_project_with_storage_path"],
    bucket: "role-terms",
  },
  {
    id: "role-struct-refresh-plan",
    query: "struct record data type refresh plan workspace indexing",
    expect: ["RefreshPlan"],
    bucket: "role-terms",
  },
  {
    id: "owner-appcontroller",
    query: "AppController owner open storage path project method",
    expect: ["AppController::open_project_with_storage_path", "open_project_with_storage_path"],
    bucket: "owner-terms",
  },
  {
    id: "storage-open-build",
    query: "Storage open_build sqlite build database connection",
    expect: ["Storage::open_build", "open_build"],
    bucket: "owner-terms",
  },
  {
    id: "language-rust-node-kind",
    query: "Rust enum graph node kind variants symbol kinds",
    expect: ["NodeKind"],
    bucket: "language-terms",
  },
  {
    id: "canonical-member-layout",
    query: "extract members and fold parallel edges for canonical graph layout",
    expect: ["extract_members", "fold_edges"],
    bucket: "expanded-suite",
  },
  {
    id: "canonical-depth",
    query: "compute signed depth by graph node for canonical visualization",
    expect: ["compute_signed_depth_by_node"],
    bucket: "expanded-suite",
  },
  {
    id: "canonical-visibility",
    query: "infer visibility for canonical member extraction",
    expect: ["infer_member_visibility"],
    bucket: "expanded-suite",
  },
  {
    id: "refresh-request",
    query: "resolve refresh request auto full incremental none for CLI reads",
    expect: ["resolve_refresh_request"],
    bucket: "expanded-suite",
  },
  {
    id: "cache-root-hash",
    query: "cache root for project hashes canonical project path",
    expect: ["cache_root_for_project"],
    bucket: "expanded-suite",
  },
  {
    id: "fnv1a-cache-hash",
    query: "fnv1a hex hash bytes for cache directory names",
    expect: ["fnv1a_hex"],
    bucket: "expanded-suite",
  },
  {
    id: "resolution-file-filter",
    query: "search hit matches file filter path fragment during target resolution",
    expect: ["search_hit_matches_file_filter"],
    bucket: "expanded-suite",
  },
  {
    id: "resolution-rank",
    query: "compare resolution hits exact symbol before ambiguous candidates",
    expect: ["compare_resolution_hits"],
    bucket: "expanded-suite",
  },
  {
    id: "normalize-path-fragment",
    query: "normalize path fragment for query resolution file filters",
    expect: ["normalize_path_fragment"],
    bucket: "path-terms",
  },
  {
    id: "semantic-symbol-aliases",
    query: "semantic symbol aliases split namespaces camel snake acronyms",
    expect: ["semantic_symbol_aliases"],
    bucket: "alias-sensitive",
  },
  {
    id: "semantic-path-aliases",
    query: "semantic path aliases from runtime lib path components",
    expect: ["semantic_path_aliases"],
    bucket: "alias-sensitive",
  },
  {
    id: "semantic-role-aliases",
    query: "semantic symbol role aliases for methods structs enums",
    expect: ["semantic_symbol_role_aliases"],
    bucket: "role-terms",
  },
  {
    id: "pending-doc-length-sort",
    query: "sort pending LLM symbol docs for embedding batches by text length",
    expect: ["sort_pending_llm_symbol_docs_for_embedding_batches"],
    bucket: "semantic-indexing",
  },
  {
    id: "flush-pending-docs",
    query: "flush pending LLM symbol docs into SQLite storage and search engine",
    expect: ["flush_pending_llm_symbol_docs"],
    bucket: "semantic-indexing",
  },
  {
    id: "llm-doc-hash",
    query: "LLM symbol document hash includes semantic doc version prefix",
    expect: ["llm_symbol_doc_hash"],
    bucket: "semantic-indexing",
  },
  {
    id: "map-llm-search-doc",
    query: "map persisted LLM symbol document to search document embedding",
    expect: ["map_llm_doc_to_search"],
    bucket: "semantic-indexing",
  },
  {
    id: "lexical-hybrid-fallback",
    query: "lexical hybrid hits fallback when semantic runtime unavailable",
    expect: ["lexical_hybrid_hits"],
    bucket: "retrieval-weights",
  },
  {
    id: "merge-search-hits",
    query: "merge search hits by node id preserving stronger expanded score",
    expect: ["merge_search_hits_by_node_id"],
    bucket: "retrieval-weights",
  },
  {
    id: "repo-text-hits",
    query: "collect repo text hits grouped by runtime owned source files",
    expect: ["collect_repo_text_hits"],
    bucket: "repo-text",
  },
  {
    id: "repo-text-mode",
    query: "repo text enabled for mode auto off on",
    expect: ["repo_text_enabled_for_mode"],
    bucket: "repo-text",
  },
  {
    id: "build-search-hit",
    query: "build search hit with declaration coordinates and occurrence fallback",
    expect: ["build_search_hit"],
    bucket: "search-output",
  },
  {
    id: "project-summary-storage",
    query: "project summary from storage with retrieval state",
    expect: ["project_summary_from_storage"],
    bucket: "runtime-state",
  },
  {
    id: "incremental-indexing-common",
    query: "run incremental indexing common refresh for changed files",
    expect: ["run_incremental_indexing_common"],
    bucket: "runtime-state",
  },
  {
    id: "workspace-refresh-inputs",
    query: "workspace refresh inputs from store inventory",
    expect: ["workspace_refresh_inputs"],
    bucket: "runtime-state",
  },
  {
    id: "rebuild-search-state",
    query: "rebuild search state from storage after indexing",
    expect: ["rebuild_search_state_from_storage"],
    bucket: "runtime-state",
  },
  {
    id: "refresh-caches",
    query: "refresh caches after indexing semantic docs",
    expect: ["refresh_caches"],
    bucket: "runtime-state",
  },
  {
    id: "search-hybrid-scored-inner",
    query: "search hybrid scored inner with graph boosts and weights",
    expect: ["search_hybrid_scored_inner"],
    bucket: "retrieval-weights",
  },
  {
    id: "node-details",
    query: "node details source occurrence edge digest for a symbol",
    expect: ["node_details"],
    bucket: "baseline",
  },
  {
    id: "read-file-text",
    query: "read file text from project path through runtime controller",
    expect: ["read_file_text"],
    bucket: "file-io",
  },
  {
    id: "write-file-text",
    query: "write file text validates path stays inside project root",
    expect: ["write_file_text"],
    bucket: "file-io",
  },
  {
    id: "agent-command-resolution",
    query: "resolve agent command local bin windows npm shim",
    expect: ["resolve_agent_command"],
    bucket: "agent",
  },
  {
    id: "codex-agent-runner",
    query: "run Codex agent and detect Windows batch command",
    expect: ["run_codex_agent", "is_windows_batch_command"],
    bucket: "agent",
  },
  {
    id: "trail-bfs",
    query: "trail store breadth first search distances outgoing incoming edges",
    expect: ["get_trail_bfs", "bfs_distances"],
    bucket: "trail",
  },
  {
    id: "trail-to-target",
    query: "get trail to target graph path",
    expect: ["get_trail_to_target"],
    bucket: "trail",
  },
  {
    id: "trail-edges-for-node",
    query: "get edges for node id in trail store",
    expect: ["get_edges_for_node_id"],
    bucket: "trail",
  },
  {
    id: "workspace-exclude-patterns",
    query: "compile exclude patterns for workspace source discovery",
    expect: ["compile_exclude_patterns"],
    bucket: "workspace",
  },
  {
    id: "workspace-normalize-path",
    query: "normalize lexical path for workspace file inventory",
    expect: ["normalize_lexical_path"],
    bucket: "workspace",
  },
  {
    id: "source-files-discovery",
    query: "workspace source files apply language filters and excludes",
    expect: ["source_files"],
    bucket: "workspace",
  },
  {
    id: "aggregate-symbol-matches",
    query: "aggregate symbol matches from direct and expanded search terms",
    expect: ["aggregate_symbol_matches"],
    bucket: "search-ranking",
  },
  {
    id: "expand-symbol-query",
    query: "should expand symbol query for sentence prompts",
    expect: ["should_expand_symbol_query"],
    bucket: "search-ranking",
  },
  {
    id: "file-text-match-line",
    query: "file text match line for repo text search terms",
    expect: ["file_text_match_line"],
    bucket: "repo-text",
  },
  {
    id: "preferred-occurrence",
    query: "preferred occurrence uses declaration before fallback occurrences",
    expect: ["preferred_occurrence"],
    bucket: "search-output",
  },
  {
    id: "holdout-cli-refresh-gate",
    query: "before a read-only CLI command runs decide if the repo index must be built or refreshed",
    expect: ["ensure_index_ready"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-graph-query-parser",
    query: "parse a piped graph mini-language expression into trail symbol search filter and limit steps",
    expect: ["parse_graph_query", "split_pipeline"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-query-markdown-render",
    query: "render the structured query pipeline results for a human terminal report",
    expect: ["render_query_markdown"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-stdio-tool-router",
    query: "dispatch JSON stdio tool calls to search symbol trail snippet and ask handlers",
    expect: [
      "handle_stdio_tool_call",
      "handle_stdio_search",
      "handle_stdio_symbol",
      "handle_stdio_trail",
      "handle_stdio_snippet",
      "handle_stdio_ask",
    ],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-stdio-resource-reader",
    query: "serve project summary grounding snapshot and symbol inventory resources over the stdio protocol",
    expect: [
      "read_stdio_resource",
      "open_project_summary",
      "GroundingService::grounding_snapshot",
      "list_root_symbols",
    ],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-agent-profile-router",
    query: "choose an automatic retrieval preset from the user's natural language agent prompt",
    expect: ["route_auto_preset", "resolve_profile"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-agent-source-context-gate",
    query: "decide whether an agent question needs source code context before composing the answer",
    expect: ["needs_source_context", "maybe_read_source_context"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-agent-term-planner",
    query: "ask a local agent to suggest extra search terms when retrieval needs a planner",
    expect: ["build_term_planner_prompt", "should_use_agent_term_planner"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-agent-prompt-builder",
    query: "build the constrained prompt sent to a local code assistant using indexed context only",
    expect: ["build_local_agent_prompt"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-agent-citation-map",
    query: "turn scored retrieval hits into answer citations with target paths and line numbers",
    expect: ["to_citation"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-embedding-profile-env",
    query: "load embedding model profile prefixes pooling dimensions and backend choices from environment",
    expect: ["EmbeddingProfile::from_env", "EmbeddingBackendSelection::from_env"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-query-embedding-prefix",
    query: "prepend the search-query instruction prefix before embedding a user's lookup text",
    expect: ["EmbeddingRuntime::embed_query"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-semantic-score-loop",
    query: "compute cosine similarity scores between the query vector and stored semantic documents",
    expect: ["semantic_scores", "cosine_similarity", "semantic_score_from_cosine"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-embedding-postprocess",
    query: "normalize truncate and postprocess embedding vectors after model inference",
    expect: ["postprocess_embeddings"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-search-doc-delete",
    query: "remove persisted semantic search documents for one changed source file",
    expect: ["SearchDocStore::delete_for_file", "delete_for_file"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-snapshot-promote",
    query: "prepare a staged grounding snapshot database then promote it over the live cache",
    expect: ["prepare_staged_publish", "promote_staged_snapshot", "SnapshotStore::promote_staged"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-resolution-support-cache",
    query: "reuse or rebuild the cached support tables used by semantic call and import resolution",
    expect: ["ResolutionSupport::prepare", "get_resolution_support_snapshot"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-common-call-noise",
    query: "avoid resolving noisy unqualified helper calls like clone into unrelated global symbols",
    expect: [
      "common_unqualified_call_names",
      "should_keep_common_call_resolution",
      "is_common_unqualified_call_name",
    ],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-sql-placeholder-builder",
    query: "build numbered and question mark placeholders for variable length SQLite queries",
    expect: ["numbered_placeholders", "question_placeholders"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-workspace-language-filter",
    query: "filter discovered workspace files by configured source language groups",
    expect: ["matches_source_group_language", "should_filter_source_group_language"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-workspace-ignore-rules",
    query: "skip ignored directories and excluded paths while discovering project source files",
    expect: [
      "WorkspaceDiscovery::source_files",
      "WorkspaceManifest::source_files",
      "should_include_discovered_path",
      "is_excluded_path",
      "compile_exclude_patterns",
    ],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-target-display-format",
    query: "format a search result target with path line column and stable symbol label",
    expect: ["format_search_hit_target"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-explore-report",
    query: "assemble an explore report that bundles symbol details trails snippets and nearby search hits",
    expect: ["run_explore", "render_explore_markdown"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-config-env-defaults",
    query: "apply project config defaults into missing process environment variables",
    expect: ["apply_env_defaults", "set_env_if_absent"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-config-file-merge",
    query: "merge a codestory config file into CLI runtime settings",
    expect: ["merge_config_file", "load_config"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-output-file-write",
    query: "write rendered command output to a requested file and reject missing parents",
    expect: ["write_output_file", "emit_text"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-search-why-render",
    query: "append explanation fields showing why a search hit matched",
    expect: ["append_search_hit_why", "explain_search_hit"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-trail-dot-render",
    query: "render a trail graph as Graphviz DOT notation",
    expect: ["render_trail_dot", "escape_dot"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-snippet-language",
    query: "choose syntax highlighting language and fence for a source snippet",
    expect: ["snippet_language", "snippet_fence"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-index-watch-output-guard",
    query: "reject index watch output files that would be written inside the indexed tree",
    expect: ["validate_index_watch_output_file"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-progress-printer",
    query: "format and print indexing progress events while files are processed",
    expect: ["spawn_progress_printer", "format_progress_bar"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-doctor-next-commands",
    query: "build doctor diagnostics and suggest next indexing commands",
    expect: ["build_doctor_output", "index_next_commands", "doctor_check"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-ask-artifact-bundle",
    query: "write agent ask bundle artifacts with a sanitized filename",
    expect: ["write_ask_bundle", "sanitize_artifact_name"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-http-server-router",
    query: "route small HTTP server requests to health search symbol trail and snippet responses",
    expect: ["handle_http_request", "write_http_json"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-http-query-string",
    query: "decode URL query strings into target selection parameters",
    expect: ["parse_query_string", "url_decode", "target_selection_from_params"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-stdio-target-selection",
    query: "extract a target symbol query from stdio tool request arguments",
    expect: ["stdio_target_selection"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-stdio-tool-specs",
    query: "produce JSON tool metadata for the stdio tools list response",
    expect: ["stdio_tools_list_json", "stdio_tool_spec"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-stdio-template-resource",
    query: "read parameterized stdio template resources for symbols trails snippets and references",
    expect: ["read_stdio_template_resource"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-search-output-groups",
    query: "build search output while preserving indexed symbol and repo text provenance groups",
    expect: ["build_search_output"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-query-resolution-output",
    query: "serialize query target resolution selector id and display metadata",
    expect: ["serialize_candidate_targets", "build_query_resolution_output"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-search-location-key",
    query: "deduplicate search hits using path line and column location keys",
    expect: [
      "dedupe_inexact_search_hits_by_display_key",
      "merge_search_hits",
      "search_hit_location_key",
    ],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-hide-speculative-trail",
    query: "hide speculative edges from trail context before displaying graph output",
    expect: ["hide_speculative_trail_edges", "is_speculative_certainty"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-repo-text-excerpt",
    query: "make compact excerpts for repository text matches in search results",
    expect: ["repo_text_excerpt", "compact_excerpt"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-resolution-ambiguity",
    query: "construct no-match and ambiguous-match errors for target resolution",
    expect: ["no_query_match_error", "ambiguous_query_error"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-resolution-candidate-rank",
    query: "rank candidate target matches by exactness kind and declaration anchors",
    expect: ["resolution_candidate_rank", "compare_resolution_candidates"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-resolution-file-read",
    query: "read file contents while resolving a target from a file filter",
    expect: ["read_file_contents_for_resolution"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-trail-request-build",
    query: "convert CLI trail mode depth direction and filters into a trail request",
    expect: ["build_trail_request", "build_trail_request_impl"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-agent-command-config",
    query: "choose configured local agent command and backend label",
    expect: ["configured_agent_command", "agent_backend_label"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-local-agent-response",
    query: "capture stdout stderr and exit status from a local agent command",
    expect: ["build_agent_response", "run_local_agent"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-agent-retrieval-markdown",
    query: "write markdown that explains retrieved context citations and source snippets for an agent answer",
    expect: ["retrieval_markdown", "render_agent_answer_markdown"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-agent-prompt-terms",
    query: "extract concise keyword search terms from a user's long natural language prompt",
    expect: ["prompt_search_terms"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-mermaid-fallback",
    query: "create fallback mermaid diagrams when graph retrieval has too little structure",
    expect: ["fallback_mermaid", "mermaid_flowchart", "mermaid_sequence"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-canonical-layout-dedupe",
    query: "deduplicate canonical graph nodes and merge certainty for repeated graph edges",
    expect: ["dedupe_key_for_node", "merge_certainty"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-canonical-node-sizing",
    query: "estimate graph node width and height from label text and member rows",
    expect: ["estimated_node_width", "estimated_node_height"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-embedding-session-count",
    query: "read embedding session count and parallel chunk size from environment settings",
    expect: ["embedding_session_count_from_env", "embedding_parallel_chunk_size"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-embedding-binary-cosine",
    query: "score packed binary embedding vectors using signed and unsigned cosine approximations",
    expect: [
      "QuantizedEmbedding::approximate_cosine",
      "signed_binary_cosine",
      "unsigned_binary_cosine",
      "pack_sign_bits",
    ],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-http-embedding-response",
    query: "split chunked HTTP responses returned by llama cpp embeddings endpoint",
    expect: ["split_http_response", "decode_chunked_http_body"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-openai-embedding-json",
    query: "parse OpenAI compatible embeddings JSON into ordered vector rows",
    expect: ["parse_openai_embeddings_response", "rows_to_vecs"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-llamacpp-runtime-build",
    query: "build a llama cpp embedding runtime with endpoint and profile settings",
    expect: ["EmbeddingRuntime::from_env", "LlamaCppEndpoint::from_env", "EmbeddingProfile::from_env"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-embedding-pooling-config",
    query: "parse embedding pooling mode from environment and include it in the llama cpp model cache id",
    expect: ["EmbeddingPooling::from_value", "EmbeddingProfile::cache_model_id", "postprocess_embeddings"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-hash-projection-embedding",
    query: "fallback hash projection embeds text into deterministic vector dimensions",
    expect: ["embed_text_with_hash_projection"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-vector-normalization",
    query: "apply layer normalization and L2 normalization to embedding vectors",
    expect: ["layer_normalize", "l2_normalize"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-semantic-score-sort",
    query: "sort semantic node scores descending and truncate to the requested candidate limit",
    expect: ["compare_node_scores_desc", "truncate_node_scores"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-storage-sidecars",
    query: "compute sqlite sidecar file paths and clean them up after storage operations",
    expect: ["sqlite_sidecar_paths", "cleanup_sqlite_sidecars"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-grounding-rank-sql",
    query: "generate SQL expressions for grounding display names and node rank ordering",
    expect: ["grounding_display_name_expr", "grounding_node_rank_sql"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-grounding-snapshot-states",
    query: "mark grounding summary and detail snapshots dirty or ready in storage metadata",
    expect: [
      "write_grounding_snapshot_states",
      "invalidate_grounding_snapshots",
      "mark_grounding_snapshots_dirty",
      "mark_grounding_detail_snapshots_dirty",
      "has_ready_grounding_summary_snapshots",
      "has_ready_grounding_detail_snapshots",
      "has_ready_grounding_snapshots",
    ],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-grounding-detail-hydrate",
    query: "hydrate detailed grounding node summaries and edge digests after summary snapshots exist",
    expect: ["hydrate_grounding_detail_snapshots"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-storage-schema-migrations",
    query: "apply storage schema migrations and create deferred secondary indexes",
    expect: ["apply_schema_migrations", "create_deferred_secondary_indexes"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-trail-node-filter",
    query: "filter trail graph nodes by caller scope and hide tests or utility helpers",
    expect: ["apply_trail_node_filter", "is_caller_scope_allowed", "should_ignore_call_resolution"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-resolution-transaction",
    query: "run call import and override resolution inside immediate SQLite transactions",
    expect: ["run_in_immediate_transaction", "resolve_edges_after_prepare"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-resolution-update-builder",
    query: "build resolved edge updates and candidate JSON payloads during semantic resolution",
    expect: ["build_resolved_edge_update", "candidate_json"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-resolution-semantic-key",
    query: "derive semantic resolution request keys and target names from unresolved edges",
    expect: ["semantic_request_key", "semantic_request_target_name"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-resolution-import-candidates",
    query: "create import name candidates and module prefixes for language-aware resolution",
    expect: ["import_name_candidates", "module_prefix"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-resolution-candidate-pool",
    query: "collect candidate pools from exact same file same module global and fuzzy matches",
    expect: [
      "collect_candidate_pool",
      "collect_candidate_pool_from_index",
      "find_same_file",
      "find_same_module",
      "find_fuzzy",
    ],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-semantic-language-family",
    query: "group languages into compatible semantic families for import and call resolution",
    expect: ["language_family_bucket", "compatible_language_families"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-semantic-import-normalize",
    query: "normalize JavaScript TypeScript C and C++ import or include symbols for matching",
    expect: ["normalize_import_symbol", "normalize_include_symbol", "strip_known_script_extension"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-workspace-inventory-map",
    query: "build a workspace inventory map from stored file records for incremental refresh",
    expect: ["WorkspaceInventory::from_records", "RefreshInputs::inventory_map"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-workspace-file-push",
    query: "push discovered source files with normalized compare keys for stable ordering",
    expect: ["push_discovered_file", "normalized_compare_key", "normalize_path_key"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-index-artifact-cache-key",
    query: "mix compiler and language settings into an index artifact cache key",
    expect: ["build_index_artifact_cache_key", "mix_compilation_info", "mix_optional_standard"],
    bucket: "holdout-adversarial",
  },
  {
    id: "holdout-bookmark-create",
    query: "create bookmark categories and update bookmark comments in storage",
    expect: [
      "create_bookmark_category",
      "Storage::update_bookmark_comment",
      "update_bookmark_comment",
    ],
    bucket: "holdout-adversarial",
  },
];

const benchmarkQueries = selectQueries(queries);

const modelPaths = {
  minilmOnnx: path.join(root, "models/all-minilm-l6-v2/model.onnx"),
  bgeSmallOnnx: path.join(root, "models/bge-small-en-v1.5/onnx/model.onnx"),
  bgeBaseOnnx: path.join(root, "models/bge-base-en-v1.5/onnx/model.onnx"),
  minilmGguf: path.join(root, "models/gguf/all-minilm-l6-v2/all-minilm-l6-v2-q8_0.gguf"),
  bgeSmallGguf: path.join(root, "models/gguf/bge-small-en-v1.5/bge-small-en-v1.5-q8_0.gguf"),
  bgeBaseGguf: path.join(root, "models/gguf/bge-base-en-v1.5/bge-base-en-v1.5.Q8_0.gguf"),
  bgeBaseGgufQ6: path.join(root, "models/gguf/bge-base-en-v1.5/bge-base-en-v1.5.Q6_K.gguf"),
  bgeBaseGgufQ5: path.join(root, "models/gguf/bge-base-en-v1.5/bge-base-en-v1.5.Q5_K_M.gguf"),
  bgeBaseGgufQ4: path.join(root, "models/gguf/bge-base-en-v1.5/bge-base-en-v1.5.Q4_K_M.gguf"),
  bgeSmallGgufQ6: path.join(root, "models/gguf/bge-small-en-v1.5/bge-small-en-v1.5-q6_k.gguf"),
  bgeSmallGgufQ5: path.join(root, "models/gguf/bge-small-en-v1.5/bge-small-en-v1.5-q5_k_m.gguf"),
  bgeSmallGgufQ4: path.join(root, "models/gguf/bge-small-en-v1.5/bge-small-en-v1.5-q4_k_m.gguf"),
  bgeSmallOnnxInt8: path.join(root, "models/bge-small-en-v1.5/onnx/model.int8.onnx"),
  bgeBaseOnnxInt8: path.join(root, "models/bge-base-en-v1.5/onnx/model.int8.onnx"),
  bgeSmallOnnxInt4: path.join(root, "models/bge-small-en-v1.5/onnx/model.int4.onnx"),
  bgeBaseOnnxInt4: path.join(root, "models/bge-base-en-v1.5/onnx/model.int4.onnx"),
  nomicV15Gguf: path.join(
    root,
    "models/gguf/nomic-embed-text-v1.5/nomic-embed-text-v1.5.Q8_0.gguf",
  ),
  nomicV2Gguf: path.join(
    root,
    "models/gguf/nomic-embed-text-v2-moe/nomic-embed-text-v2-moe.Q8_0.gguf",
  ),
  nomicV15GgufQ6: path.join(
    root,
    "models/gguf/nomic-embed-text-v1.5/nomic-embed-text-v1.5.Q6_K.gguf",
  ),
  nomicV15GgufQ5: path.join(
    root,
    "models/gguf/nomic-embed-text-v1.5/nomic-embed-text-v1.5.Q5_K_M.gguf",
  ),
  nomicV15GgufQ4: path.join(
    root,
    "models/gguf/nomic-embed-text-v1.5/nomic-embed-text-v1.5.Q4_K_M.gguf",
  ),
  gemmaGguf: path.join(root, "models/gguf/embeddinggemma-300m/embeddinggemma-300m-q8_0.gguf"),
  qwenGguf: path.join(root, "models/gguf/qwen3-embedding-0.6b/Qwen3-Embedding-0.6B-Q8_0.gguf"),
  qwenGgufQ6: path.join(root, "models/gguf/qwen3-embedding-0.6b/Qwen3-Embedding-0.6B-Q6_K.gguf"),
  qwenGgufQ5: path.join(root, "models/gguf/qwen3-embedding-0.6b/Qwen3-Embedding-0.6B-Q5_K_M.gguf"),
  qwenGgufQ4: path.join(root, "models/gguf/qwen3-embedding-0.6b/Qwen3-Embedding-0.6B-Q4_K_M.gguf"),
};

function onnxBase(id, profile, modelPath) {
  return {
    id,
    kind: "onnx",
    hardware: "DirectML",
    profile,
    modelPath,
    batch: 128,
    sessions: 2,
    docMode: "alias_variant",
    semanticScope: "all",
  };
}

function llamaBase(id, profile, modelPath, pooling, ctx = 4096) {
  return {
    id,
    kind: "llama",
    hardware: "Vulkan",
    profile,
    modelPath,
    pooling,
    ctx,
    serverBatch: 2048,
    serverUbatch: 2048,
    requests: 2,
    parallel: 2,
    flash: "auto",
    batch: 128,
    docMode: "alias_variant",
    semanticScope: "all",
  };
}

const baseProfiles = {
  onnxBgeBase: onnxBase("onnx-bge-base", "bge-base-en-v1.5", modelPaths.bgeBaseOnnx),
  onnxBgeSmall: onnxBase("onnx-bge-small", "bge-small-en-v1.5", modelPaths.bgeSmallOnnx),
  onnxMiniLm: onnxBase("onnx-minilm", "minilm", modelPaths.minilmOnnx),
  llamaBgeBase: llamaBase("llama-bge-base", "bge-base-en-v1.5", modelPaths.bgeBaseGguf, "cls"),
  llamaBgeSmall: llamaBase("llama-bge-small", "bge-small-en-v1.5", modelPaths.bgeSmallGguf, "cls"),
  llamaMiniLm: llamaBase("llama-minilm", "minilm", modelPaths.minilmGguf, "mean"),
  llamaNomicV15: llamaBase(
    "llama-nomic-v15",
    "nomic-embed-text-v1.5",
    modelPaths.nomicV15Gguf,
    "mean",
  ),
  llamaNomicV2: {
    ...llamaBase(
      "llama-nomic-v2",
      "nomic-embed-text-v2-moe",
      modelPaths.nomicV2Gguf,
      "mean",
      512,
    ),
    maxTokens: 512,
  },
  llamaGemma: llamaBase("llama-gemma", "embeddinggemma-300m", modelPaths.gemmaGguf, "mean"),
  llamaQwen: {
    ...llamaBase("llama-qwen", "qwen3-embedding-0.6b", modelPaths.qwenGguf, "last", 8192),
    requests: 1,
    parallel: 1,
    maxTokens: 8192,
  },
};

function cloneCase(base, overrides = {}) {
  return { ...base, ...overrides };
}

function parsePositiveIntEnv(name) {
  const value = process.env[name];
  if (value === undefined || value === "") {
    return null;
  }
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < 1) {
    throw new Error(`${name} must be a positive integer`);
  }
  return parsed;
}

function selectQueries(allQueries) {
  let out = allQueries;
  if (selectedQueryIds.size > 0) {
    out = out.filter((query) => selectedQueryIds.has(query.id));
  }
  if (selectedQueryBuckets.size > 0) {
    out = out.filter((query) => selectedQueryBuckets.has(query.bucket));
  }
  if (queryLimit !== null) {
    out = out.slice(0, queryLimit);
  }
  if (out.length === 0) {
    throw new Error("query selection produced no benchmark queries");
  }
  return out;
}

function caseId(config) {
  const parts = [
    config.id,
    config.docMode,
    config.semanticScope ? `scope-${config.semanticScope}` : "",
    `b${config.batch}`,
    config.kind === "onnx" ? `s${config.sessions}` : `r${config.requests}-np${config.parallel}`,
  ];
  if (config.kind === "llama") {
    parts.push(`ctx${config.ctx}`);
    parts.push(`pool-${config.pooling}`);
    if (config.flash !== "auto") {
      parts.push(`fa-${config.flash}`);
    }
  }
  if (config.variant) {
    parts.push(config.variant);
  }
  if (config.quantization) {
    parts.push(`q-${config.quantization}`);
  }
  if (config.vectorEncoding) {
    parts.push(`vec-${config.vectorEncoding}`);
  }
  if (config.truncateDim !== undefined) {
    parts.push(`dim${config.truncateDim}`);
  }
  if (config.semanticDocMaxTokens !== undefined) {
    parts.push(`doc-tok${config.semanticDocMaxTokens}`);
  }
  if (config.streamPendingSemanticDocs) {
    parts.push("stream-docs");
  }
  if (config.fullTextIndex === false) {
    parts.push("no-fulltext");
  }
  if (config.hybridWeights) {
    parts.push(
      `w${config.hybridWeights.lexical}-${config.hybridWeights.semantic}-${config.hybridWeights.graph}`,
    );
  }
  if (config.hybridLimits) {
    parts.push(
      `lim-l${config.hybridLimits.lexical ?? "d"}-s${config.hybridLimits.semantic ?? "d"}`,
    );
  }
  if (config.repeatIndex !== undefined) {
    parts.push(`run${config.repeatIndex}`);
  }
  return parts
    .filter(Boolean)
    .join("-")
    .replaceAll(/[^a-zA-Z0-9._-]+/g, "-")
    .replaceAll(/-+/g, "-")
    .toLowerCase();
}

function artifactCaseDirName(caseIdValue) {
  if (caseIdValue.length <= 56) {
    return caseIdValue;
  }
  const hash = crypto.createHash("sha1").update(caseIdValue).digest("hex").slice(0, 10);
  const prefix = caseIdValue.slice(0, 44).replaceAll(/[-.]+$/g, "");
  return `${prefix}-${hash}`;
}

function stageSmoke() {
  return [
    cloneCase(baseProfiles.onnxBgeBase, { stage: "smoke" }),
    cloneCase(baseProfiles.llamaBgeBase, { stage: "smoke" }),
  ];
}

function withRepeats(profiles, stage, repeats = 3) {
  return profiles.flatMap((profile) =>
    Array.from({ length: repeats }, (_, index) =>
      cloneCase(profile, { stage, repeatIndex: index + 1 }),
    ),
  );
}

function stageControls() {
  return withRepeats(
    [
      cloneCase(baseProfiles.onnxBgeSmall, {
        variant: "current-default",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 128,
        sessions: 2,
      }),
      cloneCase(baseProfiles.onnxBgeBase, {
        variant: "prior-best-onnx",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 128,
        sessions: 2,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "prior-best-llama",
        docMode: "alias_variant",
        semanticScope: "all",
        requests: 4,
        parallel: 4,
      }),
      cloneCase(baseProfiles.onnxBgeSmall, {
        variant: "fast-bge-small",
        docMode: "no_alias",
        semanticScope: "all",
        batch: 256,
        sessions: 2,
      }),
    ],
    "controls",
  );
}

function stageAlias() {
  const docModes = ["no_alias", "current_alias", "alias_variant"];
  const profiles = [
    baseProfiles.onnxBgeBase,
    baseProfiles.llamaBgeBase,
    baseProfiles.onnxBgeSmall,
    baseProfiles.llamaNomicV15,
    baseProfiles.llamaQwen,
    baseProfiles.llamaGemma,
  ];
  return profiles.flatMap((profile) =>
    docModes.map((docMode) => cloneCase(profile, { stage: "alias", docMode })),
  );
}

function stageWeightQuant() {
  const bgeBaseLlamaLeader = cloneCase(baseProfiles.llamaBgeBase, {
    requests: 4,
    parallel: 4,
  });
  const llamaRows = [
    [bgeBaseLlamaLeader, modelPaths.bgeBaseGgufQ6, "q6_k"],
    [bgeBaseLlamaLeader, modelPaths.bgeBaseGgufQ5, "q5_k_m"],
    [bgeBaseLlamaLeader, modelPaths.bgeBaseGgufQ4, "q4_k_m"],
    [baseProfiles.llamaBgeSmall, modelPaths.bgeSmallGguf, "q8_0"],
    [baseProfiles.llamaBgeSmall, modelPaths.bgeSmallGgufQ6, "q6_k"],
    [baseProfiles.llamaBgeSmall, modelPaths.bgeSmallGgufQ5, "q5_k_m"],
    [baseProfiles.llamaBgeSmall, modelPaths.bgeSmallGgufQ4, "q4_k_m"],
    [baseProfiles.llamaNomicV15, modelPaths.nomicV15GgufQ6, "q6_k"],
    [baseProfiles.llamaNomicV15, modelPaths.nomicV15GgufQ5, "q5_k_m"],
    [baseProfiles.llamaNomicV15, modelPaths.nomicV15GgufQ4, "q4_k_m"],
    [baseProfiles.llamaQwen, modelPaths.qwenGgufQ6, "q6_k"],
    [baseProfiles.llamaQwen, modelPaths.qwenGgufQ5, "q5_k_m"],
    [baseProfiles.llamaQwen, modelPaths.qwenGgufQ4, "q4_k_m"],
  ].map(([base, modelPath, quantization]) =>
    cloneCase(base, {
      stage: "weight-quant",
      modelPath,
      quantization,
      variant: "gguf",
      allowMissingArtifact: true,
    }),
  );

  const onnxRows = [
    [baseProfiles.onnxBgeSmall, modelPaths.bgeSmallOnnxInt8, "int8-dynamic"],
    [
      baseProfiles.onnxBgeSmall,
      modelPaths.bgeSmallOnnxInt8,
      "int8-dynamic",
      {
        variant: "fast-profile",
        docMode: "no_alias",
        semanticScope: "all",
        batch: 256,
        sessions: 2,
      },
    ],
    [baseProfiles.onnxBgeBase, modelPaths.bgeBaseOnnxInt8, "int8-dynamic"],
    [baseProfiles.onnxBgeSmall, modelPaths.bgeSmallOnnxInt4, "int4-weight-only"],
    [baseProfiles.onnxBgeBase, modelPaths.bgeBaseOnnxInt4, "int4-weight-only"],
  ].map(([base, modelPath, quantization, overrides = {}]) =>
    cloneCase(base, {
      stage: "weight-quant",
      modelPath,
      quantization,
      variant: "onnx",
      allowMissingArtifact: true,
      ...overrides,
    }),
  );

  return [...llamaRows, ...onnxRows];
}

function stageVectorQuant() {
  const winnerShape = {
    docMode: "no_alias",
    semanticScope: "all",
    batch: 256,
    sessions: 2,
  };
  return ["float32", "float16", "int8", "uint8", "binary", "ubinary"].map((vectorEncoding) => {
    const config = cloneCase(baseProfiles.onnxBgeSmall, {
      stage: "vector-quant",
      variant: vectorEncoding === "float32" ? "winner-float32-control" : `winner-stored-${vectorEncoding}`,
      vectorEncoding,
      ...winnerShape,
    });
    if (vectorEncoding === "float16") {
      config.skipReason =
        "CodeStory does not yet have a float16 stored-vector implementation; int8/uint8/binary/ubinary are benchmarkable through the quantized prefilter path.";
    }
    return config;
  });
}

function stageDimension() {
  const nomicDims = [768, 512, 256, 128, 64].map((dimension) =>
    cloneCase(baseProfiles.llamaNomicV15, {
      stage: "dimension",
      variant: `nomic-dim-${dimension}`,
      truncateDim: dimension,
      expectedDim: dimension,
      vectorEncoding: "float32",
      docMode: "current_alias",
    }),
  );

  const gemmaDims = [768, 512, 256, 128].map((dimension) =>
    cloneCase(baseProfiles.llamaGemma, {
      stage: "dimension",
      variant: `gemma-dim-${dimension}`,
      truncateDim: dimension,
      expectedDim: dimension,
      vectorEncoding: "float32",
      docMode: "no_alias",
    }),
  );

  const qwenDims = [1024, 512, 256, 128].map((dimension) =>
    cloneCase(baseProfiles.llamaQwen, {
      stage: "dimension",
      variant: `qwen-dim-${dimension}`,
      truncateDim: dimension,
      expectedDim: dimension,
      vectorEncoding: "float32",
      docMode: "alias_variant",
      ctx: 2048,
      maxTokens: 2048,
      requests: 1,
      parallel: 1,
    }),
  );

  const nomicV2Dims = [768, 256].map((dimension) =>
    cloneCase(baseProfiles.llamaNomicV2, {
      stage: "dimension",
      variant: `nomic-v2-dim-${dimension}`,
      truncateDim: dimension,
      expectedDim: dimension,
      vectorEncoding: "float32",
      docMode: "current_alias",
      semanticDocMaxTokens: 320,
      requests: 1,
      parallel: 1,
    }),
  );

  const negativeControls = [384, 256].map((dimension) =>
    cloneCase(baseProfiles.onnxBgeSmall, {
      stage: "dimension",
      variant: `bge-small-negative-dim-${dimension}`,
      truncateDim: dimension,
      expectedDim: dimension,
      vectorEncoding: "float32",
      docMode: "no_alias",
    }),
  );

  return [...nomicDims, ...gemmaDims, ...qwenDims, ...nomicV2Dims, ...negativeControls];
}

function stageRetrieval() {
  const weightSweeps = [
    ["default-weights", undefined],
    ["lexical-heavy", { lexical: 0.65, semantic: 0.3, graph: 0.05 }],
    ["semantic-heavy", { lexical: 0.15, semantic: 0.8, graph: 0.05 }],
    ["balanced", { lexical: 0.45, semantic: 0.45, graph: 0.1 }],
  ].map(([variant, hybridWeights]) =>
    cloneCase(baseProfiles.onnxBgeSmall, {
      stage: "retrieval",
      variant,
      semanticScope: "durable",
      docMode: "alias_variant",
      hybridWeights,
    }),
  );

  const docSweeps = [
    cloneCase(baseProfiles.onnxBgeSmall, {
      stage: "retrieval",
      variant: "scope-all",
      semanticScope: "all",
      docMode: "alias_variant",
    }),
    cloneCase(baseProfiles.onnxBgeSmall, {
      stage: "retrieval",
      variant: "no-alias",
      semanticScope: "durable",
      docMode: "no_alias",
    }),
    cloneCase(baseProfiles.onnxBgeSmall, {
      stage: "retrieval",
      variant: "full-alias",
      semanticScope: "durable",
      docMode: "current_alias",
    }),
    cloneCase(baseProfiles.llamaNomicV15, {
      stage: "retrieval",
      variant: "nomic-v2-token-budget-needed",
      ...baseProfiles.llamaNomicV2,
      docMode: "current_alias",
      skipReason:
        "Blocked until semantic docs expose a hard token budget; previous alias docs exceeded the model context limit.",
    }),
  ];

  return [...weightSweeps, ...docSweeps];
}

function stageBgeSmallCandidate() {
  return withRepeats(
    [
      cloneCase(baseProfiles.onnxBgeSmall, {
        stage: "bge-small-candidate",
        variant: "baseline-current-default",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 128,
        sessions: 2,
      }),
      cloneCase(baseProfiles.onnxBgeSmall, {
        stage: "bge-small-candidate",
        variant: "crossed-scope-all-no-alias-b256",
        docMode: "no_alias",
        semanticScope: "all",
        batch: 256,
        sessions: 2,
      }),
    ],
    "bge-small-candidate",
  );
}

function stageTuning() {
  const cases = [];
  for (const batch of [64, 128, 256]) {
    for (const sessions of [1, 2, 4]) {
      cases.push(
        cloneCase(baseProfiles.onnxBgeBase, {
          stage: "tuning",
          docMode: "alias_variant",
          batch,
          sessions,
        }),
      );
    }
  }
  for (const batch of [64, 128, 256]) {
    for (const sessions of [1, 2, 4]) {
      cases.push(
        cloneCase(baseProfiles.onnxBgeSmall, {
          stage: "tuning",
          docMode: "no_alias",
          batch,
          sessions,
        }),
      );
    }
  }
  for (const batch of [64, 128, 256]) {
    for (const parallel of [1, 2, 4]) {
      cases.push(
        cloneCase(baseProfiles.llamaBgeBase, {
          stage: "tuning",
          docMode: "alias_variant",
          batch,
          requests: parallel,
          parallel,
        }),
      );
      cases.push(
        cloneCase(baseProfiles.llamaNomicV15, {
          stage: "tuning",
          docMode: "current_alias",
          batch,
          requests: parallel,
          parallel,
        }),
      );
    }
  }
  for (const ctx of [1024, 2048, 8192]) {
    cases.push(
      cloneCase(baseProfiles.llamaQwen, {
        stage: "tuning",
        docMode: "alias_variant",
        ctx,
        maxTokens: ctx,
      }),
    );
  }
  for (const parallel of [1, 2]) {
    cases.push(
      cloneCase(baseProfiles.llamaGemma, {
        stage: "tuning",
        docMode: "no_alias",
        requests: parallel,
        parallel,
      }),
    );
  }
  return cases;
}

function stagePrompt() {
  return [
    cloneCase(baseProfiles.onnxBgeBase, {
      stage: "prompt",
      docMode: "alias_variant",
      variant: "bge-no-query-prefix",
      queryPrefix: "",
    }),
    cloneCase(baseProfiles.onnxBgeBase, {
      stage: "prompt",
      docMode: "alias_variant",
      variant: "bge-code-query-prefix",
      queryPrefix: "Represent this code search query for retrieving relevant symbols: ",
    }),
    cloneCase(baseProfiles.llamaBgeBase, {
      stage: "prompt",
      docMode: "alias_variant",
      variant: "bge-mean-pooling",
      pooling: "mean",
      embedPooling: "mean",
    }),
    cloneCase(baseProfiles.llamaNomicV15, {
      stage: "prompt",
      docMode: "current_alias",
      variant: "nomic-no-prefix",
      queryPrefix: "",
      documentPrefix: "",
    }),
    cloneCase(baseProfiles.llamaGemma, {
      stage: "prompt",
      docMode: "no_alias",
      variant: "gemma-no-doc-prefix",
      documentPrefix: "",
    }),
    cloneCase(baseProfiles.llamaQwen, {
      stage: "prompt",
      docMode: "alias_variant",
      variant: "qwen-symbol-instruction",
      queryPrefix: "Instruct: Retrieve the CodeStory symbol that implements the request\nQuery: ",
    }),
  ];
}

function stageFinalists() {
  const finalists = [
    cloneCase(baseProfiles.onnxBgeBase, { docMode: "alias_variant", sessions: 2 }),
    cloneCase(baseProfiles.onnxBgeBase, { docMode: "alias_variant", sessions: 4 }),
    cloneCase(baseProfiles.llamaBgeBase, {
      docMode: "alias_variant",
      requests: 1,
      parallel: 1,
    }),
    cloneCase(baseProfiles.llamaBgeBase, {
      docMode: "alias_variant",
      requests: 4,
      parallel: 4,
    }),
    cloneCase(baseProfiles.onnxBgeSmall, { docMode: "no_alias", batch: 256 }),
    cloneCase(baseProfiles.llamaQwen, {
      docMode: "alias_variant",
      ctx: 2048,
      maxTokens: 2048,
      requests: 1,
      parallel: 1,
    }),
  ];
  return finalists.flatMap((profile) =>
    [1, 2].map((repeatIndex) => cloneCase(profile, { stage: "finalists", repeatIndex })),
  );
}

function stageFinalistsRun2() {
  return withRepeats(
    [
      cloneCase(baseProfiles.onnxBgeSmall, {
        variant: "current-default",
        docMode: "alias_variant",
        semanticScope: "durable",
      }),
      cloneCase(baseProfiles.onnxBgeBase, {
        variant: "best-prior-quality",
        docMode: "alias_variant",
        semanticScope: "all",
      }),
      cloneCase(baseProfiles.onnxBgeBase, {
        variant: "best-prior-quality-pure-semantic-lex0-slim9",
        docMode: "alias_variant",
        semanticScope: "all",
        hybridWeights: { lexical: 0, semantic: 1, graph: 0 },
        hybridLimits: { lexical: 0, semantic: 9 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "best-prior-throughput",
        docMode: "alias_variant",
        semanticScope: "all",
        requests: 4,
        parallel: 4,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-current-alias",
        docMode: "current_alias",
        semanticScope: "all",
        requests: 4,
        parallel: 4,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-no-alias",
        docMode: "no_alias",
        semanticScope: "all",
        requests: 4,
        parallel: 4,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b128-r5",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 128,
        requests: 5,
        parallel: 5,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b256-r5",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 256,
        requests: 5,
        parallel: 5,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b384-r4",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 384,
        requests: 4,
        parallel: 4,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-balanced-weights",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0.45, semantic: 0.45, graph: 0.1 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-semantic-heavy",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0.2, semantic: 0.7, graph: 0.1 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-semantic-65",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0.25, semantic: 0.65, graph: 0.1 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-semantic-65-slim80",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0.25, semantic: 0.65, graph: 0.1 },
        hybridLimits: { semantic: 80 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-semantic-65-slim60",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0.25, semantic: 0.65, graph: 0.1 },
        hybridLimits: { semantic: 60 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-semantic-65-slim40",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0.25, semantic: 0.65, graph: 0.1 },
        hybridLimits: { semantic: 40 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-semantic-65-lex40",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0.25, semantic: 0.65, graph: 0.1 },
        hybridLimits: { lexical: 40 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-semantic-65-lex20",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0.25, semantic: 0.65, graph: 0.1 },
        hybridLimits: { lexical: 20 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-semantic-65-lex0",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0.25, semantic: 0.65, graph: 0.1 },
        hybridLimits: { lexical: 0 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-semantic-only-87-lex0",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0, semantic: 0.867, graph: 0.133 },
        hybridLimits: { lexical: 0 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-semantic-only-90-lex0",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0, semantic: 0.9, graph: 0.1 },
        hybridLimits: { lexical: 0 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-pure-semantic-lex0",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0, semantic: 1, graph: 0 },
        hybridLimits: { lexical: 0 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-pure-semantic-lex0-slim20",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0, semantic: 1, graph: 0 },
        hybridLimits: { lexical: 0, semantic: 20 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-pure-semantic-lex0-slim10",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0, semantic: 1, graph: 0 },
        hybridLimits: { lexical: 0, semantic: 10 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-pure-semantic-lex0-slim9",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0, semantic: 1, graph: 0 },
        hybridLimits: { lexical: 0, semantic: 9 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-pure-semantic-lex0-slim8",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0, semantic: 1, graph: 0 },
        hybridLimits: { lexical: 0, semantic: 8 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-semantic95-lex5-slim8",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0.05, semantic: 0.95, graph: 0 },
        hybridLimits: { lexical: 20, semantic: 8 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-pure-semantic-lex0-slim6",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0, semantic: 1, graph: 0 },
        hybridLimits: { lexical: 0, semantic: 6 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-durable-pure-semantic-lex0-slim6",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0, semantic: 1, graph: 0 },
        hybridLimits: { lexical: 0, semantic: 6 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-durable-runtime-default",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 512,
        requests: 4,
        parallel: 4,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-durable-runtime-default-stored-int8",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 512,
        requests: 4,
        parallel: 4,
        vectorEncoding: "int8",
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r5-durable-runtime-default-stored-int8",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 512,
        requests: 5,
        parallel: 5,
        vectorEncoding: "int8",
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r5-durable-runtime-default-stored-int8-ub1024",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 512,
        requests: 5,
        parallel: 5,
        serverUbatch: 1024,
        vectorEncoding: "int8",
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r6-durable-runtime-default-stored-int8-ub1024",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 512,
        requests: 6,
        parallel: 6,
        serverUbatch: 1024,
        vectorEncoding: "int8",
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r6-durable-runtime-default-stored-int8-sb1024-ub1024",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 512,
        requests: 6,
        parallel: 6,
        serverBatch: 1024,
        serverUbatch: 1024,
        vectorEncoding: "int8",
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r6-durable-runtime-default-stored-int8-sb1024-ub1024-no-fulltext",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 512,
        requests: 6,
        parallel: 6,
        serverBatch: 1024,
        serverUbatch: 1024,
        vectorEncoding: "int8",
        fullTextIndex: false,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-durable-runtime-default-stored-int8-ub1024",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 512,
        requests: 4,
        parallel: 4,
        serverUbatch: 1024,
        vectorEncoding: "int8",
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-durable-runtime-default-stored-int8-ub1024-tok320",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 512,
        requests: 4,
        parallel: 4,
        serverUbatch: 1024,
        vectorEncoding: "int8",
        semanticDocMaxTokens: 320,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-durable-runtime-default-stored-int8-ub1024-tok320-stream",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 512,
        requests: 4,
        parallel: 4,
        serverUbatch: 1024,
        vectorEncoding: "int8",
        semanticDocMaxTokens: 320,
        streamPendingSemanticDocs: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b768-r4-durable-runtime-default-stored-int8",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 768,
        requests: 4,
        parallel: 4,
        vectorEncoding: "int8",
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b768-r4-durable-runtime-default-stored-int8-ub1024",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 768,
        requests: 4,
        parallel: 4,
        serverUbatch: 1024,
        vectorEncoding: "int8",
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b896-r4-durable-runtime-default-stored-int8",
        docMode: "alias_variant",
        semanticScope: "durable",
        batch: 896,
        requests: 4,
        parallel: 4,
        vectorEncoding: "int8",
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-pure-semantic-lex0-slim5",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0, semantic: 1, graph: 0 },
        hybridLimits: { lexical: 0, semantic: 5 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-semantic-60",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0.3, semantic: 0.6, graph: 0.1 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-semantic-625",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0.275, semantic: 0.625, graph: 0.1 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-semantic-75",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0.15, semantic: 0.75, graph: 0.1 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b512-r4-graph-heavy",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0.3, semantic: 0.5, graph: 0.2 },
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-q5-b512-r4",
        modelPath: modelPaths.bgeBaseGgufQ5,
        quantization: "q5_k_m",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        allowMissingArtifact: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-q5-b512-r4-pure-semantic-lex0-slim9",
        modelPath: modelPaths.bgeBaseGgufQ5,
        quantization: "q5_k_m",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0, semantic: 1, graph: 0 },
        hybridLimits: { lexical: 0, semantic: 9 },
        allowMissingArtifact: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-q5-b512-r4-ub1024-pure-semantic-lex0-slim9",
        modelPath: modelPaths.bgeBaseGgufQ5,
        quantization: "q5_k_m",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        serverUbatch: 1024,
        hybridWeights: { lexical: 0, semantic: 1, graph: 0 },
        hybridLimits: { lexical: 0, semantic: 9 },
        allowMissingArtifact: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-q5-b512-r4-ub1024-pure-semantic-lex0-slim8",
        modelPath: modelPaths.bgeBaseGgufQ5,
        quantization: "q5_k_m",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        serverUbatch: 1024,
        hybridWeights: { lexical: 0, semantic: 1, graph: 0 },
        hybridLimits: { lexical: 0, semantic: 8 },
        allowMissingArtifact: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-q5-b512-r4-ub1024-semantic90-lex5-graph5-slim8",
        modelPath: modelPaths.bgeBaseGgufQ5,
        quantization: "q5_k_m",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        serverUbatch: 1024,
        hybridWeights: { lexical: 0.05, semantic: 0.9, graph: 0.05 },
        hybridLimits: { lexical: 20, semantic: 8 },
        allowMissingArtifact: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-q5-b512-r4-ub1024-semantic95-lex5-slim8",
        modelPath: modelPaths.bgeBaseGgufQ5,
        quantization: "q5_k_m",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        serverUbatch: 1024,
        hybridWeights: { lexical: 0.05, semantic: 0.95, graph: 0 },
        hybridLimits: { lexical: 20, semantic: 8 },
        allowMissingArtifact: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-q5-b512-r4-ub1024-semantic99-lex1-slim8",
        modelPath: modelPaths.bgeBaseGgufQ5,
        quantization: "q5_k_m",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        serverUbatch: 1024,
        hybridWeights: { lexical: 0.01, semantic: 0.99, graph: 0 },
        hybridLimits: { lexical: 20, semantic: 8 },
        allowMissingArtifact: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-q5-b512-r4-ub1024-semantic995-lex0p5-slim8",
        modelPath: modelPaths.bgeBaseGgufQ5,
        quantization: "q5_k_m",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        serverUbatch: 1024,
        hybridWeights: { lexical: 0.005, semantic: 0.995, graph: 0 },
        hybridLimits: { lexical: 20, semantic: 8 },
        allowMissingArtifact: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-q5-b512-r4-ub1024-semantic999-lex0p1-slim8",
        modelPath: modelPaths.bgeBaseGgufQ5,
        quantization: "q5_k_m",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        serverUbatch: 1024,
        hybridWeights: { lexical: 0.001, semantic: 0.999, graph: 0 },
        hybridLimits: { lexical: 20, semantic: 8 },
        allowMissingArtifact: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-q5-b512-r4-ub1024-semantic9999-lex0p01-slim8",
        modelPath: modelPaths.bgeBaseGgufQ5,
        quantization: "q5_k_m",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        serverUbatch: 1024,
        hybridWeights: { lexical: 0.0001, semantic: 0.9999, graph: 0 },
        hybridLimits: { lexical: 20, semantic: 8 },
        allowMissingArtifact: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-q5-b512-r4-semantic99-lex1-slim8",
        modelPath: modelPaths.bgeBaseGgufQ5,
        quantization: "q5_k_m",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        hybridWeights: { lexical: 0.01, semantic: 0.99, graph: 0 },
        hybridLimits: { lexical: 20, semantic: 8 },
        allowMissingArtifact: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-q5-b512-r4-ub1024-semantic98-lex2-slim8",
        modelPath: modelPaths.bgeBaseGgufQ5,
        quantization: "q5_k_m",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        serverUbatch: 1024,
        hybridWeights: { lexical: 0.02, semantic: 0.98, graph: 0 },
        hybridLimits: { lexical: 20, semantic: 8 },
        allowMissingArtifact: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-q5-b512-r4-ub1024-semantic95-graph5-slim8",
        modelPath: modelPaths.bgeBaseGgufQ5,
        quantization: "q5_k_m",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        serverUbatch: 1024,
        hybridWeights: { lexical: 0, semantic: 0.95, graph: 0.05 },
        hybridLimits: { lexical: 0, semantic: 8 },
        allowMissingArtifact: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-q5-b512-r4-ub1024",
        modelPath: modelPaths.bgeBaseGgufQ5,
        quantization: "q5_k_m",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        serverUbatch: 1024,
        allowMissingArtifact: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-q5-b512-r4-sb4096",
        modelPath: modelPaths.bgeBaseGgufQ5,
        quantization: "q5_k_m",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 512,
        requests: 4,
        parallel: 4,
        serverBatch: 4096,
        serverUbatch: 4096,
        allowMissingArtifact: true,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b768-r4",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 768,
        requests: 4,
        parallel: 4,
      }),
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "frontier-b1024-r4",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 1024,
        requests: 4,
        parallel: 4,
      }),
      cloneCase(baseProfiles.onnxBgeSmall, {
        variant: "fast-profile",
        docMode: "no_alias",
        semanticScope: "all",
        batch: 256,
      }),
      cloneCase(baseProfiles.onnxBgeSmall, {
        variant: "fast-profile-pure-semantic-lex0",
        docMode: "no_alias",
        semanticScope: "all",
        batch: 256,
        hybridWeights: { lexical: 0, semantic: 1, graph: 0 },
        hybridLimits: { lexical: 0 },
      }),
      cloneCase(baseProfiles.onnxBgeSmall, {
        variant: "fast-profile-pure-semantic-lex0-slim9",
        docMode: "no_alias",
        semanticScope: "all",
        batch: 256,
        hybridWeights: { lexical: 0, semantic: 1, graph: 0 },
        hybridLimits: { lexical: 0, semantic: 9 },
      }),
      cloneCase(baseProfiles.onnxBgeSmall, {
        variant: "fast-profile-semantic95-lex5-slim8",
        docMode: "no_alias",
        semanticScope: "all",
        batch: 256,
        hybridWeights: { lexical: 0.05, semantic: 0.95, graph: 0 },
        hybridLimits: { lexical: 20, semantic: 8 },
      }),
      cloneCase(baseProfiles.onnxBgeSmall, {
        variant: "fast-profile-semantic99-lex1-slim8",
        docMode: "no_alias",
        semanticScope: "all",
        batch: 256,
        hybridWeights: { lexical: 0.01, semantic: 0.99, graph: 0 },
        hybridLimits: { lexical: 20, semantic: 8 },
      }),
      cloneCase(baseProfiles.onnxBgeBase, {
        variant: "onnx-semantic99-lex1-slim8",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 256,
        sessions: 2,
        hybridWeights: { lexical: 0.01, semantic: 0.99, graph: 0 },
        hybridLimits: { lexical: 20, semantic: 8 },
      }),
      cloneCase(baseProfiles.onnxMiniLm, {
        variant: "minilm-semantic95-lex5-slim8",
        docMode: "no_alias",
        semanticScope: "all",
        batch: 256,
        sessions: 2,
        hybridWeights: { lexical: 0.05, semantic: 0.95, graph: 0 },
        hybridLimits: { lexical: 20, semantic: 8 },
      }),
      cloneCase(baseProfiles.llamaGemma, {
        variant: "gemma-semantic95-lex5-slim8",
        docMode: "no_alias",
        semanticScope: "all",
        batch: 128,
        requests: 2,
        parallel: 2,
        hybridWeights: { lexical: 0.05, semantic: 0.95, graph: 0 },
        hybridLimits: { lexical: 20, semantic: 8 },
      }),
      cloneCase(baseProfiles.llamaQwen, {
        variant: "qwen-dim-512-semantic95-lex5-slim8",
        docMode: "alias_variant",
        semanticScope: "all",
        batch: 128,
        requests: 1,
        parallel: 1,
        ctx: 2048,
        maxTokens: 2048,
        truncateDim: 512,
        expectedDim: 512,
        vectorEncoding: "float32",
        hybridWeights: { lexical: 0.05, semantic: 0.95, graph: 0 },
        hybridLimits: { lexical: 20, semantic: 8 },
      }),
      cloneCase(baseProfiles.llamaNomicV15, {
        variant: "nomic-dim-256",
        docMode: "current_alias",
        truncateDim: 256,
        expectedDim: 256,
      }),
      cloneCase(baseProfiles.llamaNomicV15, {
        variant: "nomic-semantic95-lex5-slim8",
        docMode: "current_alias",
        semanticScope: "all",
        hybridWeights: { lexical: 0.05, semantic: 0.95, graph: 0 },
        hybridLimits: { lexical: 20, semantic: 8 },
      }),
    ],
    "finalists2",
  );
}

function allCases() {
  return [
    ...stageSmoke(),
    ...stageControls(),
    ...stageAlias(),
    ...stageWeightQuant(),
    ...stageVectorQuant(),
    ...stageDimension(),
    ...stageRetrieval(),
    ...stageBgeSmallCandidate(),
    ...stageTuning(),
    ...stagePrompt(),
    ...stageFinalists(),
    ...stageFinalistsRun2(),
  ].map((config, index) => {
    const withPort = config.kind === "llama" ? { port: portBase + index } : {};
    return {
      ...config,
      ...withPort,
      case_id: caseId(config),
    };
  });
}

function selectedCases() {
  const cases = allCases();
  const stageScopedCases = requestedStages.has("all")
    ? cases
    : cases.filter((config) => requestedStages.has(config.stage));
  // ONNX case definitions are kept only so old case IDs in historical reports remain readable.
  // Active runs are restricted to the supported llama.cpp backend.
  if (selectedCaseIds.size > 0) {
    const unsupportedSelections = stageScopedCases
      .filter((config) => config.kind !== "llama")
      .filter((config) => selectedCaseIds.has(config.case_id) || selectedCaseIds.has(config.id))
      .map((config) => config.case_id)
      .sort();
    if (unsupportedSelections.length > 0) {
      throw new Error(
        `Removed benchmark backend selected: ${unsupportedSelections.join(
          ", ",
        )}. Active benchmark runs support llama.cpp cases only.`,
      );
    }
  }
  const supportedCases = stageScopedCases.filter((config) => config.kind === "llama");
  const selected =
    selectedCaseIds.size > 0
      ? supportedCases.filter(
          (config) => selectedCaseIds.has(config.case_id) || selectedCaseIds.has(config.id),
        )
      : supportedCases;
  const seen = new Set();
  const unique = [];
  for (const config of selected) {
    if (seen.has(config.case_id)) {
      continue;
    }
    seen.add(config.case_id);
    unique.push(config);
  }
  return unique;
}

function csvEscape(value) {
  return `"${String(value ?? "").replaceAll('"', '""')}"`;
}

function fmt(value) {
  if (value === null || value === undefined || Number.isNaN(value)) {
    return "";
  }
  return Number(value).toFixed(4);
}

function setOrDelete(env, key, value) {
  if (value === undefined || value === null) {
    delete env[key];
  } else {
    env[key] = String(value);
  }
}

function baseEnv(config) {
  const env = { ...process.env };
  env.CODESTORY_HYBRID_RETRIEVAL_ENABLED = "true";
  env.CODESTORY_SEMANTIC_DOC_SCOPE = config.semanticScope ?? "all";
  env.CODESTORY_SEMANTIC_DOC_ALIAS_MODE = config.docMode ?? "alias_variant";
  env.CODESTORY_LLM_DOC_EMBED_BATCH_SIZE = String(config.batch ?? 128);
  env.CODESTORY_EMBED_PROFILE = config.profile;
  env.CODESTORY_EMBED_BACKEND = "llamacpp";
  env.CODESTORY_EMBED_RUNTIME_MODE = "llamacpp";

  setOrDelete(env, "CODESTORY_EMBED_MAX_TOKENS", config.maxTokens);
  setOrDelete(env, "CODESTORY_EMBED_QUERY_PREFIX", config.queryPrefix);
  setOrDelete(env, "CODESTORY_EMBED_DOCUMENT_PREFIX", config.documentPrefix);
  setOrDelete(env, "CODESTORY_EMBED_POOLING", config.embedPooling);
  setOrDelete(env, "CODESTORY_EMBED_LAYER_NORM", config.layerNorm);
  setOrDelete(env, "CODESTORY_EMBED_TRUNCATE_DIM", config.truncateDim);
  setOrDelete(env, "CODESTORY_EMBED_EXPECTED_DIM", config.expectedDim);
  setOrDelete(env, "CODESTORY_SEMANTIC_DOC_MAX_TOKENS", config.semanticDocMaxTokens);
  setOrDelete(
    env,
    "CODESTORY_SEMANTIC_STREAM_PENDING_DOCS",
    config.streamPendingSemanticDocs ? "true" : null,
  );
  setOrDelete(
    env,
    "CODESTORY_STORED_VECTOR_ENCODING",
    config.vectorEncoding && config.vectorEncoding !== "float32" ? config.vectorEncoding : null,
  );
  setOrDelete(
    env,
    "CODESTORY_SYMBOL_FULL_TEXT_INDEX",
    config.fullTextIndex === false ? "false" : null,
  );
  delete env.CODESTORY_EMBED_MODEL_ID;
  env.CODESTORY_EMBED_LLAMACPP_URL = `http://127.0.0.1:${config.port}/v1/embeddings`;
  env.CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT = String(config.requests ?? 1);
  return env;
}

function runCli(args, env, logPath) {
  const started = Date.now();
  const result = spawnSync(bin, args, {
    cwd: root,
    env,
    encoding: "utf8",
    maxBuffer: 1024 * 1024 * 1024,
  });
  const elapsedMs = Date.now() - started;
  const payload = [
    `command=${bin} ${args.join(" ")}`,
    `exit=${result.status}`,
    `elapsed_ms=${elapsedMs}`,
    "--- stdout ---",
    result.stdout ?? "",
    "--- stderr ---",
    result.stderr ?? "",
  ].join("\n");
  fs.writeFileSync(logPath, payload);
  if (result.status !== 0) {
    throw new Error(`command failed; see ${logPath}`);
  }
  return { stdout: result.stdout, stderr: result.stderr, elapsedMs };
}

function parseJson(raw) {
  const start = raw.indexOf("{");
  if (start < 0) {
    throw new Error("command output did not include JSON");
  }
  return JSON.parse(raw.slice(start));
}

function readJsonFile(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function postEmbedding(port) {
  return new Promise((resolve, reject) => {
    const body = JSON.stringify({ model: "probe", input: ["probe"] });
    const req = http.request(
      {
        hostname: "127.0.0.1",
        port,
        path: "/v1/embeddings",
        method: "POST",
        headers: {
          "content-type": "application/json",
          "content-length": Buffer.byteLength(body),
        },
        timeout: 5000,
      },
      (res) => {
        let data = "";
        res.setEncoding("utf8");
        res.on("data", (chunk) => (data += chunk));
        res.on("end", () => {
          if (res.statusCode >= 200 && res.statusCode < 300) {
            resolve(data);
          } else {
            reject(new Error(`HTTP ${res.statusCode}: ${data.slice(0, 200)}`));
          }
        });
      },
    );
    req.on("timeout", () => req.destroy(new Error("timeout")));
    req.on("error", reject);
    req.write(body);
    req.end();
  });
}

async function waitForServer(port, timeoutMs = 120000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      await postEmbedding(port);
      return;
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 1500));
    }
  }
  throw new Error(`llama-server on port ${port} did not become ready: ${lastError?.message}`);
}

function llamaArgs(config) {
  return [
    "-m",
    config.modelPath,
    "--embedding",
    "--pooling",
    config.pooling,
    "--host",
    "127.0.0.1",
    "--port",
    String(config.port),
    "--device",
    "Vulkan0",
    "-ngl",
    "999",
    "-c",
    String(config.ctx ?? 4096),
    "-b",
    String(config.serverBatch ?? 2048),
    "-ub",
    String(config.serverUbatch ?? 2048),
    "-np",
    String(config.parallel ?? config.requests ?? 1),
    "-fa",
    config.flash ?? "auto",
  ];
}

async function withServer(config, caseDir, fn) {
  if (config.kind !== "llama") {
    return fn();
  }
  const stderrPath = path.join(caseDir, "llama-server.stderr.log");
  const stdoutPath = path.join(caseDir, "llama-server.stdout.log");
  const stderr = fs.openSync(stderrPath, "w");
  const stdout = fs.openSync(stdoutPath, "w");
  const server = spawn(llamaExe, llamaArgs(config), {
    cwd: llamaDir,
    stdio: ["ignore", stdout, stderr],
    windowsHide: true,
  });
  try {
    await waitForServer(config.port);
    return await fn();
  } finally {
    server.kill();
    await new Promise((resolve) => setTimeout(resolve, 1000));
    fs.closeSync(stderr);
    fs.closeSync(stdout);
  }
}

function validateLlamaGpu(caseDir) {
  const stderrPath = path.join(caseDir, "llama-server.stderr.log");
  const stderr = fs.existsSync(stderrPath) ? fs.readFileSync(stderrPath, "utf8") : "";
  if (!/using device Vulkan0/i.test(stderr)) {
    throw new Error("llama.cpp row did not log Vulkan0 GPU usage");
  }
  const offloadMatches = [...stderr.matchAll(/offloaded\s+(\d+)\/(\d+)\s+layers\s+to\s+GPU/gi)];
  const last = offloadMatches.at(-1);
  if (!last || Number(last[1]) === 0 || Number(last[1]) !== Number(last[2])) {
    throw new Error("llama.cpp row did not log all model layers offloaded to GPU");
  }
  return {
    gpu_device: "Vulkan0",
    offloaded_layers: `${last[1]}/${last[2]}`,
    provider_requested: "Vulkan0",
    provider_verified: true,
    provider_evidence: `llama.cpp stderr logged Vulkan0 and offloaded ${last[1]}/${last[2]} layers to GPU`,
  };
}

function findRank(hits, query) {
  const rank = hits.findIndex((hit) => {
    return query.expect.some((needle) => expectedSymbolMatchesHit(needle, hit));
  });
  return rank < 0 ? null : rank + 1;
}

function expectedSymbolMatchesHit(expected, hit) {
  const hitSegments = new Set(symbolSegments([hit.display_name, hit.node_ref, hit.file_path].filter(Boolean).join(" ")));
  const expectedSegments = symbolSegments(expected);
  if (!expectedSegments.length) return false;
  return expectedSegments.every((segment) => hitSegments.has(segment));
}

function symbolSegments(value) {
  return String(value ?? "")
    .match(/[a-z0-9_]+/gi)
    ?.map((segment) => segment.toLowerCase())
    .filter(Boolean) ?? [];
}

function score(searchResults) {
  const ranks = searchResults.map((result) => result.rank).filter((rank) => rank !== null);
  const count = searchResults.length;
  const persistent = searchResults.filter((result) => result.bucket === "persistent-miss");
  const persistentHits = persistent.filter((result) => result.rank !== null && result.rank <= 10);
  const hitAt = (k) => ranks.filter((rank) => rank <= k).length / count;
  const reciprocalSum = searchResults.reduce(
    (sum, result) => sum + (result.rank && result.rank <= 10 ? 1 / result.rank : 0),
    0,
  );
  return {
    hit_at_1: hitAt(1),
    hit_at_3: hitAt(3),
    hit_at_5: hitAt(5),
    hit_at_10: hitAt(10),
    mrr_at_10: reciprocalSum / count,
    mean_rank_when_found: ranks.length ? ranks.reduce((a, b) => a + b, 0) / ranks.length : null,
    persistent_hit_at_10: persistent.length ? persistentHits.length / persistent.length : null,
    misses: searchResults.filter((result) => result.rank === null).map((result) => result.id),
    regressions: [],
  };
}

function isDecisionGradeResult(result) {
  return !result.error && result.provider_verified === true;
}

function requiredFiles(config) {
  const files = [bin, config.modelPath];
  if (config.kind === "llama") {
    files.push(llamaExe);
  }
  return files;
}

function hybridWeightArgs(config) {
  if (!config.hybridWeights) {
    return [];
  }
  return [
    "--hybrid-lexical",
    String(config.hybridWeights.lexical),
    "--hybrid-semantic",
    String(config.hybridWeights.semantic),
    "--hybrid-graph",
    String(config.hybridWeights.graph),
  ];
}

function hybridLimitArgs(config) {
  if (!config.hybridLimits) {
    return [];
  }
  const args = [];
  if (config.hybridLimits.lexical !== undefined) {
    args.push("--hybrid-lexical-limit", String(config.hybridLimits.lexical));
  }
  if (config.hybridLimits.semantic !== undefined) {
    args.push("--hybrid-semantic-limit", String(config.hybridLimits.semantic));
  }
  return args;
}

function fileSizeMb(filePath) {
  if (!filePath || !fs.existsSync(filePath)) {
    return null;
  }
  return fs.statSync(filePath).size / 1024 / 1024;
}

function embeddingDimension(config) {
  if (config.expectedDim) {
    return config.expectedDim;
  }
  if (config.truncateDim) {
    return config.truncateDim;
  }
  if (config.profile === "qwen3-embedding-0.6b") {
    return 1024;
  }
  if (
    config.profile === "bge-base-en-v1.5" ||
    config.profile === "nomic-embed-text-v1.5" ||
    config.profile === "nomic-embed-text-v2-moe" ||
    config.profile === "embeddinggemma-300m"
  ) {
    return 768;
  }
  return 384;
}

function vectorBytesPerDoc(config) {
  // CODESTORY_STORED_VECTOR_ENCODING=int8 stores a compact versioned
  // llm_symbol_doc.embedding_blob with a scale header. Other encodings still persist raw f32.
  const dimension = embeddingDimension(config);
  if ((config.vectorEncoding ?? "float32") === "int8") {
    const versionedHeaderBytes = 9;
    return versionedHeaderBytes + dimension * precisionBytes.int8;
  }
  return dimension * precisionBytes.float32;
}

function prefilterVectorBytesPerDoc(config) {
  const precision = config.vectorEncoding ?? "float32";
  const bytes = precisionBytes[precision] ?? precisionBytes.float32;
  return embeddingDimension(config) * bytes;
}

async function runCase(config) {
  const artifact_case_dir = artifactCaseDirName(config.case_id);
  const caseDir = path.join(outDir, artifact_case_dir);
  const cacheDir = path.join(caseDir, "cache");
  const logsDir = path.join(caseDir, "logs");
  const replayCaseDir = cacheReplayFrom;
  const replayCacheDir = replayCaseDir ? path.join(replayCaseDir, "cache") : "";
  const replayResultPath = replayCaseDir ? path.join(replayCaseDir, "result.json") : "";
  fs.rmSync(caseDir, { recursive: true, force: true });
  fs.mkdirSync(logsDir, { recursive: true });

  if (config.skipReason) {
    const skipped = {
      ...config,
      artifact_case_dir,
      skipped: true,
      error: config.skipReason,
      model_size_mb: fileSizeMb(config.modelPath),
      vector_bytes_per_doc: vectorBytesPerDoc(config),
      prefilter_vector_bytes_per_doc: prefilterVectorBytesPerDoc(config),
    };
    fs.writeFileSync(path.join(caseDir, "result.json"), JSON.stringify(skipped, null, 2));
    return skipped;
  }

  for (const required of requiredFiles(config)) {
    if (!fs.existsSync(required)) {
      if (config.allowMissingArtifact && required === config.modelPath) {
        const skipped = {
          ...config,
          artifact_case_dir,
          skipped: true,
          error: `missing optional research artifact: ${required}`,
          model_size_mb: null,
          vector_bytes_per_doc: vectorBytesPerDoc(config),
          prefilter_vector_bytes_per_doc: prefilterVectorBytesPerDoc(config),
        };
        fs.writeFileSync(path.join(caseDir, "result.json"), JSON.stringify(skipped, null, 2));
        return skipped;
      }
      throw new Error(`missing required file: ${required}`);
    }
  }

  const env = baseEnv(config);
  console.log(`running ${config.case_id}`);
  const result = await withServer(config, caseDir, async () => {
    let provider = {};
    let semanticDocsEmbedded = null;
    let semanticDocsReused = null;
    let semanticDocCount = null;
    let semanticSeconds = null;
    let semanticDocBuildSeconds = null;
    let semanticDbUpsertSeconds = null;
    let semanticReloadSeconds = null;
    let graphPhaseSeconds = null;
    let parseIndexSeconds = null;
    let projectionFlushSeconds = null;
    let edgeResolutionSeconds = null;
    let errorFlushSeconds = null;
    let cleanupSeconds = null;
    let deferredIndexesSeconds = null;
    let summarySnapshotSeconds = null;
    let publishSeconds = null;
    let cacheRefreshSeconds = null;
    let docsPerSecond = null;
    let indexSeconds = 0;
    let embeddingModel = "";
    let retrievalMode = "";

    if (replayCaseDir) {
      if (!fs.existsSync(replayCacheDir)) {
        throw new Error(`cache replay source is missing cache dir: ${replayCacheDir}`);
      }
      if (!fs.existsSync(replayResultPath)) {
        throw new Error(`cache replay source is missing result.json: ${replayResultPath}`);
      }
      const replayResult = readJsonFile(replayResultPath);
      provider = {
        provider_requested: replayResult.provider_requested ?? "",
        provider_verified: replayResult.provider_verified ?? true,
        provider_evidence: replayResult.provider_evidence ?? "cache replay",
      };
      semanticDocCount = replayResult.semantic_doc_count ?? null;
      semanticDocsEmbedded = 0;
      semanticDocsReused = semanticDocCount;
      docsPerSecond = replayResult.docs_per_second ?? null;
      semanticSeconds = replayResult.semantic_seconds ?? null;
      semanticDocBuildSeconds = replayResult.semantic_doc_build_seconds ?? null;
      semanticDbUpsertSeconds = replayResult.semantic_db_upsert_seconds ?? null;
      semanticReloadSeconds = replayResult.semantic_reload_seconds ?? null;
      graphPhaseSeconds = replayResult.graph_phase_seconds ?? null;
      parseIndexSeconds = replayResult.parse_index_seconds ?? null;
      projectionFlushSeconds = replayResult.projection_flush_seconds ?? null;
      edgeResolutionSeconds = replayResult.edge_resolution_seconds ?? null;
      errorFlushSeconds = replayResult.error_flush_seconds ?? null;
      cleanupSeconds = replayResult.cleanup_seconds ?? null;
      deferredIndexesSeconds = replayResult.deferred_indexes_seconds ?? null;
      summarySnapshotSeconds = replayResult.summary_snapshot_seconds ?? null;
      publishSeconds = replayResult.publish_seconds ?? null;
      cacheRefreshSeconds = replayResult.cache_refresh_seconds ?? null;
      embeddingModel = replayResult.embedding_model ?? "";
      retrievalMode = replayResult.retrieval_mode ?? "";
    } else {
      const index = runCli(
        ["index", "--project", root, "--cache-dir", cacheDir, "--refresh", "full", "--format", "json"],
        env,
        path.join(logsDir, "index.log"),
      );

      const indexJson = parseJson(index.stdout);
      const phaseTimings = indexJson.phase_timings ?? {};
      provider = {};
      semanticDocsEmbedded = phaseTimings.semantic_docs_embedded ?? null;
      semanticDocCount =
        indexJson.retrieval?.semantic_doc_count ??
        indexJson.summary?.retrieval?.semantic_doc_count ??
        semanticDocsEmbedded ??
        null;
      semanticDocsReused = phaseTimings.semantic_docs_reused ?? null;
      semanticSeconds = (phaseTimings.semantic_embedding_ms ?? 0) / 1000;
      semanticDocBuildSeconds = (phaseTimings.semantic_doc_build_ms ?? 0) / 1000;
      semanticDbUpsertSeconds = (phaseTimings.semantic_db_upsert_ms ?? 0) / 1000;
      semanticReloadSeconds = (phaseTimings.semantic_reload_ms ?? 0) / 1000;
      parseIndexSeconds = (phaseTimings.parse_index_ms ?? 0) / 1000;
      projectionFlushSeconds = (phaseTimings.projection_flush_ms ?? 0) / 1000;
      edgeResolutionSeconds = (phaseTimings.edge_resolution_ms ?? 0) / 1000;
      errorFlushSeconds = (phaseTimings.error_flush_ms ?? 0) / 1000;
      cleanupSeconds = (phaseTimings.cleanup_ms ?? 0) / 1000;
      deferredIndexesSeconds = (phaseTimings.deferred_indexes_ms ?? 0) / 1000;
      summarySnapshotSeconds = (phaseTimings.summary_snapshot_ms ?? 0) / 1000;
      publishSeconds = (phaseTimings.publish_ms ?? 0) / 1000;
      cacheRefreshSeconds = (phaseTimings.cache_refresh_ms ?? 0) / 1000;
      graphPhaseSeconds =
        parseIndexSeconds +
        projectionFlushSeconds +
        edgeResolutionSeconds +
        errorFlushSeconds +
        cleanupSeconds;
      docsPerSecond =
        semanticSeconds > 0 && semanticDocsEmbedded
          ? semanticDocsEmbedded / semanticSeconds
          : semanticSeconds > 0 && semanticDocCount
            ? semanticDocCount / semanticSeconds
            : null;
      indexSeconds = index.elapsedMs / 1000;
      embeddingModel =
        indexJson.retrieval?.embedding_model ?? indexJson.summary?.retrieval?.embedding_model ?? "";
      retrievalMode = indexJson.retrieval?.mode ?? indexJson.summary?.retrieval?.mode ?? "";
    }

    const activeCacheDir = replayCaseDir ? replayCacheDir : cacheDir;
    const searchResults = [];
    let searchElapsedMs = 0;
    for (const q of benchmarkQueries) {
      const search = runCli(
        [
          "search",
          "--project",
          root,
          "--cache-dir",
          activeCacheDir,
          "--query",
          q.query,
          "--limit",
          "10",
          "--repo-text",
          "off",
          "--refresh",
          "none",
          "--format",
          "json",
          ...hybridWeightArgs(config),
          ...hybridLimitArgs(config),
        ],
        env,
        path.join(logsDir, `${q.id}.log`),
      );
      searchElapsedMs += search.elapsedMs;
      const json = parseJson(search.stdout);
      const hits = json.indexed_symbol_hits ?? [];
      searchResults.push({
        id: q.id,
        bucket: q.bucket,
        query: q.query,
        expected: q.expect,
        elapsed_ms: search.elapsedMs,
        rank: findRank(hits, q),
        top: hits.slice(0, 5).map((hit) => hit.display_name),
      });
    }
    const searchQueryMs = searchResults.map((result) => result.elapsed_ms);
    const slowestSearch = searchResults.reduce(
      (slowest, result) =>
        !slowest || result.elapsed_ms > slowest.elapsed_ms ? result : slowest,
      null,
    );
    return {
      ...config,
      artifact_case_dir,
      cache_replay_from: replayCaseDir,
      ...provider,
      semantic_doc_count: semanticDocCount,
      semantic_docs_embedded: semanticDocsEmbedded,
      semantic_docs_reused: semanticDocsReused,
      index_seconds: indexSeconds,
      semantic_seconds: semanticSeconds,
      semantic_doc_build_seconds: semanticDocBuildSeconds,
      semantic_db_upsert_seconds: semanticDbUpsertSeconds,
      semantic_reload_seconds: semanticReloadSeconds,
      graph_phase_seconds: graphPhaseSeconds,
      parse_index_seconds: parseIndexSeconds,
      projection_flush_seconds: projectionFlushSeconds,
      edge_resolution_seconds: edgeResolutionSeconds,
      error_flush_seconds: errorFlushSeconds,
      cleanup_seconds: cleanupSeconds,
      deferred_indexes_seconds: deferredIndexesSeconds,
      summary_snapshot_seconds: summarySnapshotSeconds,
      publish_seconds: publishSeconds,
      cache_refresh_seconds: cacheRefreshSeconds,
      docs_per_second: docsPerSecond,
      search_seconds: searchElapsedMs / 1000,
      search_query_ms_mean: mean(searchQueryMs),
      search_query_ms_p50: percentile(searchQueryMs, 0.5),
      search_query_ms_p95: percentile(searchQueryMs, 0.95),
      search_query_ms_max: percentile(searchQueryMs, 1),
      search_slowest_query_id: slowestSearch?.id ?? "",
      embedding_model: embeddingModel,
      retrieval_mode: retrievalMode,
      model_size_mb: fileSizeMb(config.modelPath),
      vector_bytes_per_doc: vectorBytesPerDoc(config),
      prefilter_vector_bytes_per_doc: prefilterVectorBytesPerDoc(config),
      score: score(searchResults),
      queries: searchResults,
    };
  });
  const gpu = config.kind === "llama" ? validateLlamaGpu(caseDir) : {};
  const withGpu = { ...result, ...gpu };
  fs.writeFileSync(path.join(caseDir, "result.json"), JSON.stringify(withGpu, null, 2));
  return withGpu;
}

function normalizedCombined(results) {
  const ok = results.filter(
    (result) => isDecisionGradeResult(result) && result.docs_per_second !== null,
  );
  if (ok.length === 0) {
    return [];
  }
  const mrrValues = ok.map((result) => result.score.mrr_at_10);
  const speedValues = ok.map((result) => result.docs_per_second);
  const footprintValues = ok
    .map((result) => result.model_size_mb)
    .filter((value) => Number.isFinite(value));
  const minMrr = Math.min(...mrrValues);
  const maxMrr = Math.max(...mrrValues);
  const minSpeed = Math.min(...speedValues);
  const maxSpeed = Math.max(...speedValues);
  const minFootprint = footprintValues.length ? Math.min(...footprintValues) : 0;
  const maxFootprint = footprintValues.length ? Math.max(...footprintValues) : 0;
  for (const result of ok) {
    const normalizedMrr =
      maxMrr === minMrr ? 1 : (result.score.mrr_at_10 - minMrr) / (maxMrr - minMrr);
    const speed =
      maxSpeed === minSpeed ? 1 : (result.docs_per_second - minSpeed) / (maxSpeed - minSpeed);
    const footprint =
      !Number.isFinite(result.model_size_mb) || maxFootprint === minFootprint
        ? 1
        : (maxFootprint - result.model_size_mb) / (maxFootprint - minFootprint);
    const quality =
      0.45 * normalizedMrr +
      0.2 * result.score.hit_at_10 +
      0.2 * result.score.hit_at_1 +
      0.15 * (result.score.persistent_hit_at_10 ?? result.score.hit_at_10);
    const reliability = result.skipped ? 0 : 1;
    result.quality_score = quality;
    result.speed_score = speed;
    result.footprint_score = footprint;
    result.reliability_score = reliability;
    result.decision_quality_gate = result.score.hit_at_10 >= 0.75;
    result.combined_score = 0.5 * quality + 0.25 * speed + 0.15 * footprint + 0.1 * reliability;
  }
  return [...ok].sort(
    (a, b) =>
      Number(b.decision_quality_gate) - Number(a.decision_quality_gate) ||
      b.combined_score - a.combined_score,
  );
}

function aliasComparisons(results) {
  const ok = results.filter(isDecisionGradeResult);
  const groups = new Map();
  for (const result of ok) {
    const key = [
      result.id,
      result.kind,
      result.profile,
      result.batch,
      result.sessions ?? "",
      result.requests ?? "",
      result.parallel ?? "",
      result.ctx ?? "",
      result.pooling ?? "",
      result.semanticScope ?? "",
      result.quantization ?? "",
      result.vectorEncoding ?? "",
      result.truncateDim ?? "",
      result.variant ?? "",
    ].join("|");
    const group = groups.get(key) ?? {};
    group[result.docMode] = result;
    groups.set(key, group);
  }
  const rows = [];
  for (const group of groups.values()) {
    const baseline = group.no_alias;
    if (!baseline) {
      continue;
    }
    for (const mode of ["current_alias", "alias_variant"]) {
      const candidate = group[mode];
      if (!candidate) {
        continue;
      }
      const baselineRanks = new Map(baseline.queries.map((query) => [query.id, query.rank]));
      const regressions = candidate.queries.filter((query) => {
        const before = baselineRanks.get(query.id);
        if (before === null && query.rank !== null) {
          return false;
        }
        if (before !== null && query.rank === null) {
          return true;
        }
        return before !== null && query.rank !== null && query.rank > before;
      }).length;
      rows.push({
        case_id: candidate.case_id,
        base_id: candidate.id,
        doc_mode: mode,
        delta_mrr: candidate.score.mrr_at_10 - baseline.score.mrr_at_10,
        delta_hit10: candidate.score.hit_at_10 - baseline.score.hit_at_10,
        regressions,
      });
    }
  }
  return rows.sort((a, b) => b.delta_mrr - a.delta_mrr);
}

function mean(values) {
  const numeric = values.filter((value) => Number.isFinite(value));
  if (numeric.length === 0) {
    return null;
  }
  return numeric.reduce((total, value) => total + value, 0) / numeric.length;
}

function percentile(values, quantile) {
  const numeric = values.filter((value) => Number.isFinite(value)).sort((a, b) => a - b);
  if (numeric.length === 0) {
    return null;
  }
  const clamped = Math.min(1, Math.max(0, quantile));
  const index = Math.ceil(clamped * numeric.length) - 1;
  return numeric[Math.max(0, index)];
}

function repeatBaseId(caseIdValue) {
  return caseIdValue.replace(/-run\d+$/, "");
}

function repeatSummaries(results) {
  const groups = new Map();
  for (const result of results.filter(isDecisionGradeResult)) {
    const key = repeatBaseId(result.case_id);
    const group = groups.get(key) ?? [];
    group.push(result);
    groups.set(key, group);
  }
  return [...groups.entries()]
    .filter(([, group]) => group.length > 1)
    .map(([case_id, group]) => ({
      case_id,
      runs: group.length,
      docMode: group[0].docMode,
      kind: group[0].kind,
      profile: group[0].profile,
      semanticScope: group[0].semanticScope ?? "",
      quantization: group[0].quantization ?? "",
      vectorEncoding: group[0].vectorEncoding ?? "",
      truncateDim: group[0].truncateDim ?? "",
      batch: group[0].batch,
      sessions: group[0].sessions ?? "",
      requests: group[0].requests ?? "",
      parallel: group[0].parallel ?? "",
      ctx: group[0].ctx ?? "",
      pooling: group[0].pooling ?? "",
      index_seconds: mean(group.map((result) => result.index_seconds)),
      semantic_seconds: mean(group.map((result) => result.semantic_seconds)),
      search_seconds: mean(group.map((result) => result.search_seconds)),
      docs_per_second: mean(group.map((result) => result.docs_per_second)),
      hit_at_1: mean(group.map((result) => result.score?.hit_at_1)),
      hit_at_10: mean(group.map((result) => result.score?.hit_at_10)),
      persistent_hit_at_10: mean(group.map((result) => result.score?.persistent_hit_at_10)),
      mrr_at_10: mean(group.map((result) => result.score?.mrr_at_10)),
      mean_rank_when_found: mean(group.map((result) => result.score?.mean_rank_when_found)),
      misses: group[0].score?.misses?.join(";") ?? "",
    }))
    .sort(
      (left, right) =>
        (right.mrr_at_10 ?? 0) - (left.mrr_at_10 ?? 0) ||
        (right.docs_per_second ?? 0) - (left.docs_per_second ?? 0),
    );
}

function writeManifest(cases) {
  const manifest = {
    generated_at: new Date().toISOString(),
    artifact_root: outDir,
    requested_stages: [...requestedStages],
    case_count: cases.length,
    query_count: benchmarkQueries.length,
    full_query_count: queries.length,
    query_filter: {
      ids: [...selectedQueryIds],
      buckets: [...selectedQueryBuckets],
      limit: queryLimit,
    },
    scoring:
      "Decision ranking applies a quality gate, then 50% quality, 25% speed, 15% footprint, and 10% reliability.",
    source_references: researchSources,
    blocked_candidates: blockedCandidates,
    stages: {
      "source-scan":
        "Read primary sources and generate this manifest; no model rows run in this stage.",
      controls:
        "Three-repeat baselines for current default, prior BGE-base candidates, and fast BGE-small.",
      "weight-quant":
        "GGUF model-weight quantization rows. Missing quantized artifacts are reported as skipped rows.",
      "vector-quant":
        "Quantized semantic prefilter lane. Persisted SQLite vector bytes remain float32 until compact vector storage support lands.",
      dimension:
        "Source-backed Matryoshka/dimension rows for Nomic v1.5, Nomic v2 MoE, Qwen3 0.6B, and EmbeddingGemma; BGE-small is a negative control.",
      retrieval:
        "Hybrid weight, semantic scope, and alias-mode sweeps using the CLI search weight flags.",
      "bge-small-candidate":
        "Three-repeat current-default versus crossed BGE-small scope=all + no_alias + b256 candidate.",
      finalists2:
        "Run only after earlier lanes produce candidates; three-repeat comparison of selected rows.",
    },
    cases: cases.map((config) => ({
      case_id: config.case_id,
      artifact_case_dir: artifactCaseDirName(config.case_id),
      stage: config.stage,
      id: config.id,
      kind: config.kind,
      hardware: config.hardware,
      provider_requested:
        "Vulkan0",
      profile: config.profile,
      semantic_scope: config.semanticScope,
      semantic_doc_max_tokens: config.semanticDocMaxTokens,
      doc_mode: config.docMode,
      quantization: config.quantization,
      vector_encoding: config.vectorEncoding,
      truncate_dim: config.truncateDim,
      expected_dim: config.expectedDim,
      model_path: config.modelPath,
      model_size_mb: fileSizeMb(config.modelPath),
      vector_bytes_per_doc: vectorBytesPerDoc(config),
      prefilter_vector_bytes_per_doc: prefilterVectorBytesPerDoc(config),
      batch: config.batch,
      sessions: config.sessions,
      requests: config.requests,
      parallel: config.parallel,
      ctx: config.ctx,
      pooling: config.pooling,
      hybrid_weights: config.hybridWeights,
      hybrid_limits: config.hybridLimits,
      variant: config.variant,
      skip_reason: config.skipReason,
      allow_missing_artifact: config.allowMissingArtifact,
    })),
  };
  fs.writeFileSync(path.join(outDir, "manifest.json"), JSON.stringify(manifest, null, 2));
  fs.writeFileSync(
    path.join(outDir, "sources.md"),
    [
      "# Embedding Research Sources",
      "",
      ...researchSources.map((source) => `- [${source.id}](${source.url}): ${source.claim}`),
      "",
      "## Blocked Candidates",
      "",
      ...blockedCandidates.map((candidate) => `- \`${candidate.id}\`: ${candidate.reason}`),
      "",
    ].join("\n"),
  );
}

function writeCsvFile(fileName, header, rows) {
  fs.writeFileSync(
    path.join(outDir, fileName),
    [header.join(","), ...rows.map((row) => row.map(csvEscape).join(","))].join("\n"),
  );
}

function buildReportMarkdown(ranked, aliasRows, repeatRows, failed, cases) {
  return [
    "# CodeStory Embedding Research",
    "",
    `Artifact root: \`${outDir}\``,
    `Stages: \`${[...requestedStages].join(",")}\``,
    `Cases selected: \`${cases.length}\``,
    `Queries: \`${benchmarkQueries.length}\` of \`${queries.length}\``,
    "",
    "All decision-grade rows are GPU-only and must set `provider_verified=true`. llama.cpp rows require Vulkan0 and full model-layer offload.",
    "Source-led lanes are recorded in `manifest.json` and `sources.md`; skipped rows mean an artifact or implementation prerequisite is still missing.",
    "",
    "## Ranking",
    "",
    "| Rank | Case | Stage | Doc mode | Backend | Quality gate | MRR@10 | Hit@10 | Persistent Hit@10 | Docs/sec | Footprint MB | Score |",
    "| ---: | --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ...ranked.map(
      (result, index) =>
        `| ${index + 1} | \`${result.case_id}\` | ${result.stage} | ${result.docMode} | ${result.kind} | ${
          result.decision_quality_gate ? "pass" : "fail"
        } | ${fmt(
          result.score.mrr_at_10,
        )} | ${fmt(result.score.hit_at_10)} | ${fmt(
          result.score.persistent_hit_at_10,
        )} | ${fmt(result.docs_per_second)} | ${fmt(result.model_size_mb)} | ${fmt(
          result.combined_score,
        )} |`,
    ),
    "",
    "## Alias Comparison",
    "",
    "| Case | Mode | Delta MRR@10 vs no_alias | Delta Hit@10 | Per-query regressions |",
    "| --- | --- | ---: | ---: | ---: |",
    ...aliasRows.map(
      (row) =>
        `| \`${row.base_id}\` | ${row.doc_mode} | ${fmt(row.delta_mrr)} | ${fmt(
          row.delta_hit10,
        )} | ${row.regressions} |`,
    ),
    "",
    ...(repeatRows.length
      ? [
          "## Repeat Summary",
          "",
          "| Case | Runs | MRR@10 | Hit@10 | Docs/sec | Index seconds |",
          "| --- | ---: | ---: | ---: | ---: | ---: |",
          ...repeatRows.map(
            (row) =>
              `| \`${row.case_id}\` | ${row.runs} | ${fmt(row.mrr_at_10)} | ${fmt(
                row.hit_at_10,
              )} | ${fmt(row.docs_per_second)} | ${fmt(row.index_seconds)} |`,
          ),
          "",
        ]
      : []),
    "",
    "## Blocked Candidates",
    "",
    ...blockedCandidates.map((candidate) => `- \`${candidate.id}\`: ${candidate.reason}`),
    "",
    "## Skipped Or Failed Rows",
    "",
    ...(failed.length
      ? failed.map((result) => `- \`${result.case_id}\`: ${result.error}`)
      : ["- none"]),
    "",
    "Combined score: quality gate first, then `0.50 * quality + 0.25 * speed + 0.15 * footprint + 0.10 * reliability` within this run.",
  ].join("\n");
}

function writeReports(results, cases) {
  const ranked = normalizedCombined(results);
  const rankById = new Map(ranked.map((result, index) => [result.case_id, index + 1]));
  const header = [
    "rank",
    "case_id",
    "artifact_case_dir",
    "cache_replay_from",
    "stage",
    "doc_mode",
    "kind",
    "hardware",
    "provider_requested",
    "provider_verified",
    "provider_evidence",
    "profile",
    "semantic_scope",
    "semantic_doc_max_tokens",
    "quantization",
    "vector_encoding",
    "truncate_dim",
    "batch",
    "sessions",
    "requests",
    "parallel",
    "hybrid_weights",
    "hybrid_limits",
    "ctx",
    "pooling",
    "variant",
    "model_size_mb",
    "vector_bytes_per_doc",
    "prefilter_vector_bytes_per_doc",
    "index_seconds",
    "semantic_seconds",
    "search_seconds",
    "search_query_ms_mean",
    "search_query_ms_p50",
    "search_query_ms_p95",
    "search_query_ms_max",
    "search_slowest_query_id",
    "semantic_doc_count",
    "semantic_docs_embedded",
    "docs_per_second",
    "hit_at_1",
    "hit_at_10",
    "persistent_hit_at_10",
    "mrr_at_10",
    "mean_rank_when_found",
    "quality_score",
    "speed_score",
    "footprint_score",
    "reliability_score",
    "decision_quality_gate",
    "combined_score",
    "misses",
    "skipped",
    "error",
  ];
  const rows = results.map((result) => [
    rankById.get(result.case_id) ?? "",
    result.case_id,
    result.artifact_case_dir ?? artifactCaseDirName(result.case_id),
    result.cache_replay_from ?? "",
    result.stage,
    result.docMode,
    result.kind,
    result.hardware,
    result.provider_requested ?? "",
    result.provider_verified ?? "",
    result.provider_evidence ?? "",
    result.profile,
    result.semanticScope ?? "",
    result.semanticDocMaxTokens ?? "",
    result.quantization ?? "",
    result.vectorEncoding ?? "",
    result.truncateDim ?? "",
    result.batch,
    result.sessions ?? "",
    result.requests ?? "",
    result.parallel ?? "",
    result.hybridWeights
      ? `${result.hybridWeights.lexical}/${result.hybridWeights.semantic}/${result.hybridWeights.graph}`
      : "",
    result.hybridLimits
      ? `${result.hybridLimits.lexical ?? ""}/${result.hybridLimits.semantic ?? ""}`
      : "",
    result.ctx ?? "",
    result.pooling ?? "",
    result.variant ?? "",
    fmt(result.model_size_mb),
    fmt(result.vector_bytes_per_doc),
    fmt(result.prefilter_vector_bytes_per_doc),
    fmt(result.index_seconds),
    fmt(result.semantic_seconds),
    fmt(result.search_seconds),
    fmt(result.search_query_ms_mean),
    fmt(result.search_query_ms_p50),
    fmt(result.search_query_ms_p95),
    fmt(result.search_query_ms_max),
    result.search_slowest_query_id ?? "",
    result.semantic_doc_count ?? "",
    result.semantic_docs_embedded ?? "",
    fmt(result.docs_per_second),
    fmt(result.score?.hit_at_1),
    fmt(result.score?.hit_at_10),
    fmt(result.score?.persistent_hit_at_10),
    fmt(result.score?.mrr_at_10),
    fmt(result.score?.mean_rank_when_found),
    fmt(result.quality_score),
    fmt(result.speed_score),
    fmt(result.footprint_score),
    fmt(result.reliability_score),
    result.decision_quality_gate ?? "",
    fmt(result.combined_score),
    result.score?.misses?.join(";") ?? "",
    result.skipped ? "true" : "",
    result.error ?? "",
  ]);
  writeCsvFile("results.csv", header, rows);

  const queryHeader = [
    "case_id",
    "stage",
    "doc_mode",
    "profile",
    "query_id",
    "bucket",
    "rank",
    "search_elapsed_ms",
    "expected",
    "top5",
  ];
  const queryRows = results.flatMap((result) =>
    (result.queries ?? []).map((query) => [
      result.case_id,
      result.stage,
      result.docMode,
      result.profile,
      query.id,
      query.bucket,
      query.rank ?? "",
      fmt(query.elapsed_ms),
      query.expected.join(";"),
      query.top.join(";"),
    ]),
  );
  writeCsvFile("query-ranks.csv", queryHeader, queryRows);

  const aliasRows = aliasComparisons(results);
  const aliasHeader = ["case_id", "base_id", "doc_mode", "delta_mrr", "delta_hit10", "regressions"];
  writeCsvFile(
    "alias-comparisons.csv",
    aliasHeader,
    aliasRows.map((row) => [
      row.case_id,
      row.base_id,
      row.doc_mode,
      fmt(row.delta_mrr),
      fmt(row.delta_hit10),
      row.regressions,
    ]),
  );

  const repeatRows = repeatSummaries(results);
  const repeatHeader = [
    "case_id",
    "runs",
    "doc_mode",
    "kind",
    "profile",
    "semantic_scope",
    "quantization",
    "vector_encoding",
    "truncate_dim",
    "batch",
    "sessions",
    "requests",
    "parallel",
    "ctx",
    "pooling",
    "avg_index_seconds",
    "avg_semantic_seconds",
    "avg_search_seconds",
    "avg_docs_per_second",
    "avg_hit_at_1",
    "avg_hit_at_10",
    "avg_persistent_hit_at_10",
    "avg_mrr_at_10",
    "avg_mean_rank_when_found",
    "misses",
  ];
  writeCsvFile(
    "repeat-summary.csv",
    repeatHeader,
    repeatRows.map((row) => [
      row.case_id,
      row.runs,
      row.docMode,
      row.kind,
      row.profile,
      row.semanticScope,
      row.quantization,
      row.vectorEncoding,
      row.truncateDim,
      row.batch,
      row.sessions,
      row.requests,
      row.parallel,
      row.ctx,
      row.pooling,
      fmt(row.index_seconds),
      fmt(row.semantic_seconds),
      fmt(row.search_seconds),
      fmt(row.docs_per_second),
      fmt(row.hit_at_1),
      fmt(row.hit_at_10),
      fmt(row.persistent_hit_at_10),
      fmt(row.mrr_at_10),
      fmt(row.mean_rank_when_found),
      row.misses,
    ]),
  );

  fs.writeFileSync(path.join(outDir, "results.json"), JSON.stringify(results, null, 2));
  fs.writeFileSync(path.join(outDir, "cases.json"), JSON.stringify(cases, null, 2));
  fs.writeFileSync(path.join(outDir, "queries.json"), JSON.stringify(benchmarkQueries, null, 2));

  const failed = results.filter((result) => result.error);
  fs.writeFileSync(
    path.join(outDir, "report.md"),
    buildReportMarkdown(ranked, aliasRows, repeatRows, failed, cases),
  );
}

function listSelectedCases(cases) {
  for (const config of cases) {
    console.log(config.case_id);
  }
}

async function main() {
  const cases = selectedCases();
  const results = [];
  fs.mkdirSync(outDir, { recursive: true });
  writeManifest(cases);
  if (process.env.CODESTORY_EMBED_RESEARCH_LIST === "1") {
    listSelectedCases(cases);
    return;
  }
  for (const config of cases) {
    try {
      results.push(await runCase(config));
    } catch (error) {
      console.error(`${config.case_id} failed: ${error.message}`);
      results.push({
        ...config,
        artifact_case_dir: artifactCaseDirName(config.case_id),
        error: error.message,
      });
    }
  }
  writeReports(results, cases);
  console.log(`wrote ${path.join(outDir, "results.csv")}`);
}

await main();
