import { spawn, spawnSync } from "node:child_process";
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
const portBase = Number(process.env.CODESTORY_EMBED_RESEARCH_PORT_BASE ?? 8170);

const blockedCandidates = [
  {
    id: "llama-nomic-v2",
    reason:
      "blocked until semantic docs have a token-aware budget; previous alias docs exceeded llama.cpp's 512-token cap",
  },
];

const researchSources = [
  {
    id: "sentence-transformers-embedding-quantization",
    url: "https://www.sbert.net/docs/package_reference/util/quantization.html",
    claim:
      "Embedding quantization is separate from model-weight quantization and supports float32, int8, uint8, binary, and ubinary corpus encodings with optional rescoring.",
  },
  {
    id: "onnx-runtime-quantization",
    url: "https://onnxruntime.ai/docs/performance/model-optimizations/quantization.html",
    claim:
      "ONNX Runtime supports dynamic/static int8 quantization and selected int4 weight-only quantization, but accuracy and operator support must be verified per model.",
  },
  {
    id: "onnx-directml-execution-provider",
    url: "https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html",
    claim:
      "DirectML requires sequential execution per session, so CodeStory benchmarks use separate ONNX sessions rather than parallel Run calls on one session.",
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
      "Nomic v2 MoE remains a candidate only after CodeStory can enforce a semantic-doc token budget compatible with its context limits.",
  },
  {
    id: "qwen3-embedding-06b",
    url: "https://huggingface.co/Qwen/Qwen3-Embedding-0.6B",
    claim:
      "Qwen3 0.6B remains a quality experiment candidate; CodeStory should not assume Matryoshka support unless the model card or implementation proves it.",
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
    id: "onnx-normalized-embeddings",
    query: "convert ONNX output tensors into normalized embedding vectors",
    expect: ["extract_onnx_embeddings"],
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
    id: "onnx-directml-provider",
    query: "DirectML ONNX execution provider environment variable configuration",
    expect: ["configure_embedding_execution_provider", "EMBEDDING_EXECUTION_PROVIDER_ENV"],
    bucket: "alias-sensitive",
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
];

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
  if (config.hybridWeights) {
    parts.push(
      `w${config.hybridWeights.lexical}-${config.hybridWeights.semantic}-${config.hybridWeights.graph}`,
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
  const llamaRows = [
    [baseProfiles.llamaBgeBase, modelPaths.bgeBaseGgufQ6, "q6_k"],
    [baseProfiles.llamaBgeBase, modelPaths.bgeBaseGgufQ5, "q5_k_m"],
    [baseProfiles.llamaBgeBase, modelPaths.bgeBaseGgufQ4, "q4_k_m"],
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
    [baseProfiles.onnxBgeBase, modelPaths.bgeBaseOnnxInt8, "int8-dynamic"],
    [baseProfiles.onnxBgeSmall, modelPaths.bgeSmallOnnxInt4, "int4-weight-only"],
    [baseProfiles.onnxBgeBase, modelPaths.bgeBaseOnnxInt4, "int4-weight-only"],
  ].map(([base, modelPath, quantization]) =>
    cloneCase(base, {
      stage: "weight-quant",
      modelPath,
      quantization,
      variant: "onnx",
      allowMissingArtifact: true,
    }),
  );

  return [...llamaRows, ...onnxRows];
}

function stageVectorQuant() {
  return ["float16", "int8", "uint8", "binary", "ubinary"].map((vectorEncoding) =>
    cloneCase(baseProfiles.onnxBgeSmall, {
      stage: "vector-quant",
      variant: `stored-${vectorEncoding}`,
      vectorEncoding,
      skipReason:
        "CodeStory does not yet have a quantized-vector storage/search implementation; this source-led lane is manifest-only until that lands.",
    }),
  );
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

  return [...nomicDims, ...negativeControls];
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
      id: "llama-nomic-v2",
      profile: "nomic-embed-text-v2-moe",
      docMode: "current_alias",
      skipReason:
        "Blocked until semantic docs expose a hard token budget; previous alias docs exceeded the model context limit.",
    }),
  ];

  return [...weightSweeps, ...docSweeps];
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
      cloneCase(baseProfiles.llamaBgeBase, {
        variant: "best-prior-throughput",
        docMode: "alias_variant",
        semanticScope: "all",
        requests: 4,
        parallel: 4,
      }),
      cloneCase(baseProfiles.onnxBgeSmall, {
        variant: "fast-profile",
        docMode: "no_alias",
        semanticScope: "all",
        batch: 256,
      }),
      cloneCase(baseProfiles.llamaNomicV15, {
        variant: "nomic-dim-256",
        docMode: "current_alias",
        truncateDim: 256,
        expectedDim: 256,
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
  const selected =
    selectedCaseIds.size > 0
      ? stageScopedCases.filter(
          (config) => selectedCaseIds.has(config.case_id) || selectedCaseIds.has(config.id),
        )
      : stageScopedCases;
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
  env.CODESTORY_EMBED_BACKEND = config.kind === "onnx" ? "onnx" : "llamacpp";
  env.CODESTORY_EMBED_RUNTIME_MODE = config.kind === "onnx" ? "onnx" : "llamacpp";

  setOrDelete(env, "CODESTORY_EMBED_MAX_TOKENS", config.maxTokens);
  setOrDelete(env, "CODESTORY_EMBED_QUERY_PREFIX", config.queryPrefix);
  setOrDelete(env, "CODESTORY_EMBED_DOCUMENT_PREFIX", config.documentPrefix);
  setOrDelete(env, "CODESTORY_EMBED_POOLING", config.embedPooling);
  setOrDelete(env, "CODESTORY_EMBED_LAYER_NORM", config.layerNorm);
  setOrDelete(env, "CODESTORY_EMBED_TRUNCATE_DIM", config.truncateDim);
  setOrDelete(env, "CODESTORY_EMBED_EXPECTED_DIM", config.expectedDim);
  delete env.CODESTORY_EMBED_INTRA_THREADS;
  delete env.CODESTORY_EMBED_INTER_THREADS;
  delete env.CODESTORY_EMBED_PARALLEL_EXECUTION;
  delete env.CODESTORY_EMBED_MODEL_ID;
  delete env.CODESTORY_EMBED_TOKENIZER_PATH;

  if (config.kind === "onnx") {
    env.CODESTORY_EMBED_EXECUTION_PROVIDER = "directml";
    env.CODESTORY_EMBED_MODEL_PATH = config.modelPath;
    env.CODESTORY_EMBED_SESSION_COUNT = String(config.sessions ?? 2);
    delete env.CODESTORY_EMBED_LLAMACPP_URL;
    delete env.CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT;
  } else {
    delete env.CODESTORY_EMBED_EXECUTION_PROVIDER;
    delete env.CODESTORY_EMBED_MODEL_PATH;
    delete env.CODESTORY_EMBED_SESSION_COUNT;
    env.CODESTORY_EMBED_LLAMACPP_URL = `http://127.0.0.1:${config.port}/v1/embeddings`;
    env.CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT = String(config.requests ?? 1);
  }
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
  };
}

function findRank(hits, query) {
  const expected = query.expect.map((item) => item.toLowerCase());
  const rank = hits.findIndex((hit) => {
    const display = String(hit.display_name ?? "").toLowerCase();
    return expected.some((needle) => display.includes(needle));
  });
  return rank < 0 ? null : rank + 1;
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
  const precision = config.vectorEncoding ?? "float32";
  const bytes = precisionBytes[precision] ?? precisionBytes.float32;
  return embeddingDimension(config) * bytes;
}

async function runCase(config) {
  const caseDir = path.join(outDir, config.case_id);
  const cacheDir = path.join(caseDir, "cache");
  const logsDir = path.join(caseDir, "logs");
  fs.rmSync(caseDir, { recursive: true, force: true });
  fs.mkdirSync(logsDir, { recursive: true });

  if (config.skipReason) {
    const skipped = {
      ...config,
      skipped: true,
      error: config.skipReason,
      model_size_mb: fileSizeMb(config.modelPath),
      vector_bytes_per_doc: vectorBytesPerDoc(config),
    };
    fs.writeFileSync(path.join(caseDir, "result.json"), JSON.stringify(skipped, null, 2));
    return skipped;
  }

  for (const required of requiredFiles(config)) {
    if (!fs.existsSync(required)) {
      if (config.allowMissingArtifact && required === config.modelPath) {
        const skipped = {
          ...config,
          skipped: true,
          error: `missing optional research artifact: ${required}`,
          model_size_mb: null,
          vector_bytes_per_doc: vectorBytesPerDoc(config),
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
    const index = runCli(
      ["index", "--project", root, "--cache-dir", cacheDir, "--refresh", "full", "--format", "json"],
      env,
      path.join(logsDir, "index.log"),
    );
    if (config.kind === "onnx" && /using CPU execution provider|built without the onnx-directml/i.test(index.stderr)) {
      throw new Error(`${config.case_id} did not use DirectML; refusing CPU ONNX benchmark`);
    }

    const indexJson = parseJson(index.stdout);
    const semanticDocsEmbedded = indexJson.phase_timings?.semantic_docs_embedded ?? null;
    const semanticDocCount =
      indexJson.retrieval?.semantic_doc_count ??
      indexJson.summary?.retrieval?.semantic_doc_count ??
      semanticDocsEmbedded ??
      null;
    const semanticSeconds = (indexJson.phase_timings?.semantic_embedding_ms ?? 0) / 1000;
    const searchResults = [];
    let searchElapsedMs = 0;
    for (const q of queries) {
      const search = runCli(
        [
          "search",
          "--project",
          root,
          "--cache-dir",
          cacheDir,
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
        rank: findRank(hits, q),
        top: hits.slice(0, 5).map((hit) => hit.display_name),
      });
    }
    return {
      ...config,
      semantic_doc_count: semanticDocCount,
      semantic_docs_embedded: semanticDocsEmbedded,
      semantic_docs_reused: indexJson.phase_timings?.semantic_docs_reused ?? null,
      index_seconds: index.elapsedMs / 1000,
      semantic_seconds: semanticSeconds,
      docs_per_second:
        semanticSeconds > 0 && semanticDocsEmbedded
          ? semanticDocsEmbedded / semanticSeconds
          : semanticSeconds > 0 && semanticDocCount
            ? semanticDocCount / semanticSeconds
            : null,
      search_seconds: searchElapsedMs / 1000,
      embedding_model:
        indexJson.retrieval?.embedding_model ?? indexJson.summary?.retrieval?.embedding_model ?? "",
      retrieval_mode: indexJson.retrieval?.mode ?? indexJson.summary?.retrieval?.mode ?? "",
      model_size_mb: fileSizeMb(config.modelPath),
      vector_bytes_per_doc: vectorBytesPerDoc(config),
      score: score(searchResults),
      queries: searchResults,
    };
  });
  const gpu = config.kind === "llama" ? validateLlamaGpu(caseDir) : { gpu_device: "DirectML" };
  const withGpu = { ...result, ...gpu };
  fs.writeFileSync(path.join(caseDir, "result.json"), JSON.stringify(withGpu, null, 2));
  return withGpu;
}

function normalizedCombined(results) {
  const ok = results.filter((result) => !result.error && result.docs_per_second !== null);
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
  const ok = results.filter((result) => !result.error);
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

function repeatBaseId(caseIdValue) {
  return caseIdValue.replace(/-run\d+$/, "");
}

function repeatSummaries(results) {
  const groups = new Map();
  for (const result of results.filter((item) => !item.error)) {
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
    query_count: queries.length,
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
        "GGUF and ONNX model-weight quantization rows. Missing quantized artifacts are reported as skipped rows.",
      "vector-quant":
        "Stored-vector quantization lane. Manifest-only until CodeStory has quantized-vector storage/search support.",
      dimension:
        "Matryoshka/dimension-shortening rows, plus BGE-small negative controls.",
      retrieval:
        "Hybrid weight, semantic scope, and alias-mode sweeps using the CLI search weight flags.",
      finalists2:
        "Run only after earlier lanes produce candidates; three-repeat comparison of selected rows.",
    },
    cases: cases.map((config) => ({
      case_id: config.case_id,
      stage: config.stage,
      id: config.id,
      kind: config.kind,
      hardware: config.hardware,
      profile: config.profile,
      semantic_scope: config.semanticScope,
      doc_mode: config.docMode,
      quantization: config.quantization,
      vector_encoding: config.vectorEncoding,
      truncate_dim: config.truncateDim,
      expected_dim: config.expectedDim,
      model_path: config.modelPath,
      model_size_mb: fileSizeMb(config.modelPath),
      vector_bytes_per_doc: vectorBytesPerDoc(config),
      batch: config.batch,
      sessions: config.sessions,
      requests: config.requests,
      parallel: config.parallel,
      ctx: config.ctx,
      pooling: config.pooling,
      hybrid_weights: config.hybridWeights,
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

function writeReports(results, cases) {
  const ranked = normalizedCombined(results);
  const rankById = new Map(ranked.map((result, index) => [result.case_id, index + 1]));
  const header = [
    "rank",
    "case_id",
    "stage",
    "doc_mode",
    "kind",
    "hardware",
    "profile",
    "semantic_scope",
    "quantization",
    "vector_encoding",
    "truncate_dim",
    "batch",
    "sessions",
    "requests",
    "parallel",
    "hybrid_weights",
    "ctx",
    "pooling",
    "variant",
    "model_size_mb",
    "vector_bytes_per_doc",
    "index_seconds",
    "semantic_seconds",
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
    result.stage,
    result.docMode,
    result.kind,
    result.hardware,
    result.profile,
    result.semanticScope ?? "",
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
    result.ctx ?? "",
    result.pooling ?? "",
    result.variant ?? "",
    fmt(result.model_size_mb),
    fmt(result.vector_bytes_per_doc),
    fmt(result.index_seconds),
    fmt(result.semantic_seconds),
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
  fs.writeFileSync(
    path.join(outDir, "results.csv"),
    [header.join(","), ...rows.map((row) => row.map(csvEscape).join(","))].join("\n"),
  );

  const queryHeader = [
    "case_id",
    "stage",
    "doc_mode",
    "profile",
    "query_id",
    "bucket",
    "rank",
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
      query.expected.join(";"),
      query.top.join(";"),
    ]),
  );
  fs.writeFileSync(
    path.join(outDir, "query-ranks.csv"),
    [queryHeader.join(","), ...queryRows.map((row) => row.map(csvEscape).join(","))].join("\n"),
  );

  const aliasRows = aliasComparisons(results);
  const aliasHeader = ["case_id", "base_id", "doc_mode", "delta_mrr", "delta_hit10", "regressions"];
  fs.writeFileSync(
    path.join(outDir, "alias-comparisons.csv"),
    [
      aliasHeader.join(","),
      ...aliasRows.map((row) =>
        [
          row.case_id,
          row.base_id,
          row.doc_mode,
          fmt(row.delta_mrr),
          fmt(row.delta_hit10),
          row.regressions,
        ]
          .map(csvEscape)
          .join(","),
      ),
    ].join("\n"),
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
    "avg_docs_per_second",
    "avg_hit_at_1",
    "avg_hit_at_10",
    "avg_persistent_hit_at_10",
    "avg_mrr_at_10",
    "avg_mean_rank_when_found",
    "misses",
  ];
  fs.writeFileSync(
    path.join(outDir, "repeat-summary.csv"),
    [
      repeatHeader.join(","),
      ...repeatRows.map((row) =>
        [
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
          fmt(row.docs_per_second),
          fmt(row.hit_at_1),
          fmt(row.hit_at_10),
          fmt(row.persistent_hit_at_10),
          fmt(row.mrr_at_10),
          fmt(row.mean_rank_when_found),
          row.misses,
        ]
          .map(csvEscape)
          .join(","),
      ),
    ].join("\n"),
  );

  fs.writeFileSync(path.join(outDir, "results.json"), JSON.stringify(results, null, 2));
  fs.writeFileSync(path.join(outDir, "cases.json"), JSON.stringify(cases, null, 2));
  fs.writeFileSync(path.join(outDir, "queries.json"), JSON.stringify(queries, null, 2));

  const failed = results.filter((result) => result.error);
  const md = [
    "# CodeStory Embedding Research",
    "",
    `Artifact root: \`${outDir}\``,
    `Stages: \`${[...requestedStages].join(",")}\``,
    `Cases selected: \`${cases.length}\``,
    `Queries: \`${queries.length}\``,
    "",
    "All decision-grade rows are GPU-only. ONNX rows require DirectML; llama.cpp rows require Vulkan0 and full model-layer offload.",
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
  fs.writeFileSync(path.join(outDir, "report.md"), md);
}

const cases = selectedCases();
const results = [];
fs.mkdirSync(outDir, { recursive: true });
writeManifest(cases);
if (process.env.CODESTORY_EMBED_RESEARCH_LIST === "1") {
  for (const config of cases) {
    console.log(config.case_id);
  }
  process.exit(0);
}
for (const config of cases) {
  try {
    results.push(await runCase(config));
  } catch (error) {
    console.error(`${config.case_id} failed: ${error.message}`);
    results.push({
      ...config,
      error: error.message,
    });
  }
}
writeReports(results, cases);
console.log(`wrote ${path.join(outDir, "results.csv")}`);
