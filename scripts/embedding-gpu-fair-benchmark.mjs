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
];

const modelPaths = {
  minilmOnnx: path.join(root, "models/all-minilm-l6-v2/model.onnx"),
  bgeSmallOnnx: path.join(root, "models/bge-small-en-v1.5/onnx/model.onnx"),
  bgeBaseOnnx: path.join(root, "models/bge-base-en-v1.5/onnx/model.onnx"),
  minilmGguf: path.join(root, "models/gguf/all-minilm-l6-v2/all-minilm-l6-v2-q8_0.gguf"),
  bgeSmallGguf: path.join(root, "models/gguf/bge-small-en-v1.5/bge-small-en-v1.5-q8_0.gguf"),
  bgeBaseGguf: path.join(root, "models/gguf/bge-base-en-v1.5/bge-base-en-v1.5.Q8_0.gguf"),
  nomicV15Gguf: path.join(
    root,
    "models/gguf/nomic-embed-text-v1.5/nomic-embed-text-v1.5.Q8_0.gguf",
  ),
  gemmaGguf: path.join(root, "models/gguf/embeddinggemma-300m/embeddinggemma-300m-q8_0.gguf"),
  qwenGguf: path.join(root, "models/gguf/qwen3-embedding-0.6b/Qwen3-Embedding-0.6B-Q8_0.gguf"),
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
  if (config.repeatIndex !== undefined) {
    parts.push(`run${config.repeatIndex}`);
  }
  return parts
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

function allCases() {
  return [...stageSmoke(), ...stageAlias(), ...stageTuning(), ...stagePrompt(), ...stageFinalists()].map(
    (config, index) => {
      const withPort = config.kind === "llama" ? { port: portBase + index } : {};
      return {
        ...config,
        ...withPort,
        case_id: caseId(config),
      };
    },
  );
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

async function runCase(config) {
  const caseDir = path.join(outDir, config.case_id);
  const cacheDir = path.join(caseDir, "cache");
  const logsDir = path.join(caseDir, "logs");
  fs.rmSync(caseDir, { recursive: true, force: true });
  fs.mkdirSync(logsDir, { recursive: true });

  for (const required of requiredFiles(config)) {
    if (!fs.existsSync(required)) {
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
  const minMrr = Math.min(...mrrValues);
  const maxMrr = Math.max(...mrrValues);
  const minSpeed = Math.min(...speedValues);
  const maxSpeed = Math.max(...speedValues);
  for (const result of ok) {
    const quality =
      maxMrr === minMrr ? 1 : (result.score.mrr_at_10 - minMrr) / (maxMrr - minMrr);
    const speed =
      maxSpeed === minSpeed ? 1 : (result.docs_per_second - minSpeed) / (maxSpeed - minSpeed);
    result.combined_score = 0.7 * quality + 0.3 * speed;
  }
  return [...ok].sort((a, b) => b.combined_score - a.combined_score);
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
    "batch",
    "sessions",
    "requests",
    "parallel",
    "ctx",
    "pooling",
    "variant",
    "index_seconds",
    "semantic_seconds",
    "semantic_doc_count",
    "semantic_docs_embedded",
    "docs_per_second",
    "hit_at_1",
    "hit_at_10",
    "mrr_at_10",
    "mean_rank_when_found",
    "combined_score",
    "misses",
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
    result.batch,
    result.sessions ?? "",
    result.requests ?? "",
    result.parallel ?? "",
    result.ctx ?? "",
    result.pooling ?? "",
    result.variant ?? "",
    fmt(result.index_seconds),
    fmt(result.semantic_seconds),
    result.semantic_doc_count ?? "",
    result.semantic_docs_embedded ?? "",
    fmt(result.docs_per_second),
    fmt(result.score?.hit_at_1),
    fmt(result.score?.hit_at_10),
    fmt(result.score?.mrr_at_10),
    fmt(result.score?.mean_rank_when_found),
    fmt(result.combined_score),
    result.score?.misses?.join(";") ?? "",
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
    "",
    "## Ranking",
    "",
    "| Rank | Case | Doc mode | Backend | MRR@10 | Hit@10 | Docs/sec | Score |",
    "| ---: | --- | --- | --- | ---: | ---: | ---: | ---: |",
    ...ranked.map(
      (result, index) =>
        `| ${index + 1} | \`${result.case_id}\` | ${result.docMode} | ${result.kind} | ${fmt(
          result.score.mrr_at_10,
        )} | ${fmt(result.score.hit_at_10)} | ${fmt(result.docs_per_second)} | ${fmt(
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
    failed.length ? "## Failures" : "## Failures",
    "",
    ...(failed.length
      ? failed.map((result) => `- \`${result.case_id}\`: ${result.error}`)
      : ["- none"]),
    "",
    "Combined score: `0.70 * normalized(MRR@10) + 0.30 * normalized(docs/sec)` within this run.",
  ].join("\n");
  fs.writeFileSync(path.join(outDir, "report.md"), md);
}

const cases = selectedCases();
const results = [];
fs.mkdirSync(outDir, { recursive: true });
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
