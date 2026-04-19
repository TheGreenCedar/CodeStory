import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";

const root = process.env.CODESTORY_FAIR_BENCH_ROOT ?? process.cwd();
const bin =
  process.env.CODESTORY_FAIR_BENCH_BIN ?? path.join(root, "target/release/codestory-cli.exe");
const llamaDir = process.env.CODESTORY_LLAMA_CPP_DIR ?? path.join(root, "target/llamacpp/b8840");
const llamaExe = process.env.CODESTORY_LLAMA_CPP_SERVER ?? path.join(llamaDir, "llama-server.exe");
const stamp = new Date().toISOString().replaceAll(/[-:]/g, "").replace(/\..+/, "");
const outDir =
  process.env.CODESTORY_FAIR_BENCH_OUT_DIR ??
  path.join(root, "target/embedding-gpu-fair-benchmark", stamp);

const batchSize = 128;
const defaultLlamaParallel = 2;
const onnxSessions = 2;

const queries = [
  {
    id: "project-storage-open",
    query: "open a project with a specific sqlite database file",
    expect: ["open_project_with_storage_path"],
  },
  {
    id: "grounding-overview",
    query: "make a compact grounding overview with coverage buckets and notes",
    expect: ["grounding_snapshot"],
  },
  {
    id: "trail-neighborhood",
    query: "follow outgoing graph edges around a focus symbol",
    expect: ["trail_context", "graph_trail"],
  },
  {
    id: "semantic-sync",
    query: "synchronize persisted semantic documents after indexing",
    expect: ["sync_llm_symbol_projection"],
  },
  {
    id: "semantic-doc-text",
    query: "build the text that gets embedded for semantic search documents",
    expect: ["build_llm_symbol_doc_text"],
  },
  {
    id: "semantic-reload",
    query: "reload semantic documents from storage into the search engine",
    expect: ["reload_llm_docs_from_storage"],
  },
  {
    id: "hybrid-enabled",
    query: "check whether hybrid retrieval is enabled by an environment flag",
    expect: ["hybrid_retrieval_enabled"],
  },
  {
    id: "hybrid-weights",
    query: "normalize lexical semantic and graph weights for retrieval",
    expect: ["normalized_hybrid_weights"],
  },
  {
    id: "search-rank",
    query: "rank search hits by exact matches symbol kind and score",
    expect: ["compare_search_hits", "search_match_rank"],
  },
  {
    id: "natural-language-terms",
    query: "extract natural language search terms without stopwords",
    expect: ["extract_symbol_search_terms"],
  },
  {
    id: "onnx-normalized-embeddings",
    query: "convert ONNX output tensors into normalized embedding vectors",
    expect: ["extract_onnx_embeddings"],
  },
  {
    id: "llamacpp-endpoint",
    query: "send an embeddings request to the local llama cpp server endpoint",
    expect: ["post_json_to_http_endpoint"],
  },
  {
    id: "canonical-layout",
    query: "create a canonical graph layout for visualization",
    expect: ["build_canonical_layout"],
  },
  {
    id: "refresh-plan",
    query: "prepare a workspace refresh plan from changed files",
    expect: ["build_refresh_plan"],
  },
  {
    id: "index-file",
    query: "index one source file with tree sitter symbols and semantic edges",
    expect: ["index_file", "index_single_file"],
  },
  {
    id: "search-markdown",
    query: "render CLI search results as markdown",
    expect: ["render_search_markdown"],
  },
  {
    id: "resolve-target",
    query: "resolve a user query to the symbol id used by trail and snippet",
    expect: ["resolve_target", "targetargs::selection"],
  },
  {
    id: "search-json",
    query: "turn a search hit into JSON output with a relative path",
    expect: ["build_search_hit_output", "searchhitoutput"],
  },
];

const profiles = [
  {
    id: "onnx-minilm-b128-s2-directml",
    kind: "onnx",
    profile: "minilm",
    modelPath: path.join(root, "models/all-minilm-l6-v2/model.onnx"),
  },
  {
    id: "onnx-bge-small-b128-s2-directml",
    kind: "onnx",
    profile: "bge-small-en-v1.5",
    modelPath: path.join(root, "models/bge-small-en-v1.5/onnx/model.onnx"),
  },
  {
    id: "onnx-bge-base-b128-s2-directml",
    kind: "onnx",
    profile: "bge-base-en-v1.5",
    modelPath: path.join(root, "models/bge-base-en-v1.5/onnx/model.onnx"),
  },
  {
    id: "llama-minilm-b128-r2-np2-vulkan",
    kind: "llama",
    profile: "minilm",
    port: 8170,
    modelPath: path.join(root, "models/gguf/all-minilm-l6-v2/all-minilm-l6-v2-q8_0.gguf"),
    pooling: "mean",
    ctx: 4096,
  },
  {
    id: "llama-bge-small-b128-r2-np2-vulkan",
    kind: "llama",
    profile: "bge-small-en-v1.5",
    port: 8172,
    modelPath: path.join(root, "models/gguf/bge-small-en-v1.5/bge-small-en-v1.5-q8_0.gguf"),
    pooling: "cls",
    ctx: 4096,
  },
  {
    id: "llama-bge-base-b128-r2-np2-vulkan",
    kind: "llama",
    profile: "bge-base-en-v1.5",
    port: 8174,
    modelPath: path.join(root, "models/gguf/bge-base-en-v1.5/bge-base-en-v1.5.Q8_0.gguf"),
    pooling: "cls",
    ctx: 4096,
  },
  {
    id: "llama-nomic-v15-b128-r2-np2-vulkan",
    kind: "llama",
    profile: "nomic-embed-text-v1.5",
    port: 8176,
    modelPath: path.join(
      root,
      "models/gguf/nomic-embed-text-v1.5/nomic-embed-text-v1.5.Q8_0.gguf",
    ),
    pooling: "mean",
    ctx: 4096,
  },
  {
    id: "llama-nomic-v2-b128-r2-np2-vulkan",
    kind: "llama",
    profile: "nomic-embed-text-v2-moe",
    port: 8178,
    modelPath: path.join(
      root,
      "models/gguf/nomic-embed-text-v2-moe/nomic-embed-text-v2-moe.Q8_0.gguf",
    ),
    pooling: "mean",
    ctx: 4096,
  },
  {
    id: "llama-gemma-b128-r2-np2-vulkan",
    kind: "llama",
    profile: "embeddinggemma-300m",
    port: 8180,
    modelPath: path.join(root, "models/gguf/embeddinggemma-300m/embeddinggemma-300m-q8_0.gguf"),
    pooling: "mean",
    ctx: 4096,
  },
  {
    id: "llama-qwen-b128-r1-np1-vulkan",
    kind: "llama",
    profile: "qwen3-embedding-0.6b",
    port: 8182,
    modelPath: path.join(root, "models/gguf/qwen3-embedding-0.6b/Qwen3-Embedding-0.6B-Q8_0.gguf"),
    pooling: "last",
    ctx: 8192,
    maxTokens: 8192,
    requests: 1,
    parallel: 1,
  },
];

function selectedProfiles() {
  const selected = process.env.CODESTORY_FAIR_BENCH_PROFILES;
  if (!selected) {
    return profiles;
  }
  const ids = new Set(selected.split(",").map((item) => item.trim()).filter(Boolean));
  return profiles.filter((profile) => ids.has(profile.id));
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

function baseEnv(profile) {
  const env = { ...process.env };
  env.CODESTORY_HYBRID_RETRIEVAL_ENABLED = "true";
  env.CODESTORY_SEMANTIC_DOC_SCOPE = "all";
  env.CODESTORY_LLM_DOC_EMBED_BATCH_SIZE = String(batchSize);
  env.CODESTORY_EMBED_PROFILE = profile.profile;
  env.CODESTORY_EMBED_BACKEND = profile.kind === "onnx" ? "onnx" : "llamacpp";
  env.CODESTORY_EMBED_RUNTIME_MODE = profile.kind === "onnx" ? "onnx" : "llamacpp";
  if (profile.maxTokens) {
    env.CODESTORY_EMBED_MAX_TOKENS = String(profile.maxTokens);
  } else {
    delete env.CODESTORY_EMBED_MAX_TOKENS;
  }
  delete env.CODESTORY_EMBED_INTRA_THREADS;
  delete env.CODESTORY_EMBED_INTER_THREADS;
  delete env.CODESTORY_EMBED_PARALLEL_EXECUTION;
  delete env.CODESTORY_EMBED_QUERY_PREFIX;
  delete env.CODESTORY_EMBED_DOCUMENT_PREFIX;
  delete env.CODESTORY_EMBED_POOLING;
  delete env.CODESTORY_EMBED_LAYER_NORM;
  delete env.CODESTORY_EMBED_TRUNCATE_DIM;
  delete env.CODESTORY_EMBED_EXPECTED_DIM;
  delete env.CODESTORY_EMBED_MODEL_ID;
  delete env.CODESTORY_EMBED_TOKENIZER_PATH;

  if (profile.kind === "onnx") {
    env.CODESTORY_EMBED_EXECUTION_PROVIDER = "directml";
    env.CODESTORY_EMBED_MODEL_PATH = profile.modelPath;
    env.CODESTORY_EMBED_SESSION_COUNT = String(onnxSessions);
    delete env.CODESTORY_EMBED_LLAMACPP_URL;
    delete env.CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT;
  } else {
    delete env.CODESTORY_EMBED_EXECUTION_PROVIDER;
    delete env.CODESTORY_EMBED_MODEL_PATH;
    delete env.CODESTORY_EMBED_SESSION_COUNT;
    env.CODESTORY_EMBED_LLAMACPP_URL = `http://127.0.0.1:${profile.port}/v1/embeddings`;
    env.CODESTORY_EMBED_LLAMACPP_REQUEST_COUNT = String(llamaRequestCount(profile));
  }
  return env;
}

function llamaRequestCount(profile) {
  return profile.requests ?? defaultLlamaParallel;
}

function llamaParallelSlots(profile) {
  return profile.parallel ?? llamaRequestCount(profile);
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

function llamaArgs(profile) {
  return [
    "-m",
    profile.modelPath,
    "--embedding",
    "--pooling",
    profile.pooling,
    "--host",
    "127.0.0.1",
    "--port",
    String(profile.port),
    "--device",
    "Vulkan0",
    "-ngl",
    "999",
    "-c",
    String(profile.ctx),
    "-b",
    "2048",
    "-ub",
    "2048",
    "-np",
    String(llamaParallelSlots(profile)),
    "-fa",
    "auto",
  ];
}

async function withServer(profile, caseDir, fn) {
  if (profile.kind !== "llama") {
    return fn();
  }
  const stderr = fs.openSync(path.join(caseDir, "llama-server.stderr.log"), "w");
  const stdout = fs.openSync(path.join(caseDir, "llama-server.stdout.log"), "w");
  const server = spawn(llamaExe, llamaArgs(profile), {
    cwd: llamaDir,
    stdio: ["ignore", stdout, stderr],
    windowsHide: true,
  });
  try {
    await waitForServer(profile.port);
    return await fn();
  } finally {
    server.kill();
    await new Promise((resolve) => setTimeout(resolve, 1000));
    fs.closeSync(stderr);
    fs.closeSync(stdout);
  }
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
  };
}

async function runProfile(profile) {
  const caseDir = path.join(outDir, profile.id);
  const cacheDir = path.join(caseDir, "cache");
  const logsDir = path.join(caseDir, "logs");
  fs.rmSync(caseDir, { recursive: true, force: true });
  fs.mkdirSync(logsDir, { recursive: true });

  for (const required of [bin, profile.modelPath]) {
    if (!fs.existsSync(required)) {
      throw new Error(`missing required file: ${required}`);
    }
  }
  if (profile.kind === "llama" && !fs.existsSync(llamaExe)) {
    throw new Error(`missing llama-server: ${llamaExe}`);
  }

  const env = baseEnv(profile);
  console.log(`running ${profile.id}`);
  return withServer(profile, caseDir, async () => {
    const index = runCli(
      ["index", "--project", root, "--cache-dir", cacheDir, "--refresh", "full", "--format", "json"],
      env,
      path.join(logsDir, "index.log"),
    );
    if (profile.kind === "onnx" && /using CPU execution provider|built without the onnx-directml/i.test(index.stderr)) {
      throw new Error(`${profile.id} did not use DirectML; refusing CPU ONNX benchmark`);
    }
    const indexJson = parseJson(index.stdout);
    const semanticDocs =
      indexJson.phase_timings?.semantic_docs_embedded ??
      indexJson.retrieval?.semantic_doc_count ??
      indexJson.summary?.retrieval?.semantic_doc_count ??
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
        query: q.query,
        expected: q.expect,
        rank: findRank(hits, q),
        top: hits.slice(0, 5).map((hit) => hit.display_name),
      });
    }
    const result = {
      id: profile.id,
      kind: profile.kind,
      hardware: profile.kind === "onnx" ? "DirectML" : "Vulkan",
      profile: profile.profile,
      batch: batchSize,
      sessions: profile.kind === "onnx" ? onnxSessions : null,
      requests: profile.kind === "llama" ? llamaRequestCount(profile) : null,
      parallel: profile.kind === "llama" ? llamaParallelSlots(profile) : null,
      ctx: profile.kind === "llama" ? profile.ctx : null,
      semantic_docs: semanticDocs,
      index_seconds: index.elapsedMs / 1000,
      semantic_seconds: semanticSeconds,
      docs_per_second: semanticSeconds > 0 && semanticDocs ? semanticDocs / semanticSeconds : null,
      search_seconds: searchElapsedMs / 1000,
      embedding_model:
        indexJson.retrieval?.embedding_model ?? indexJson.summary?.retrieval?.embedding_model ?? "",
      score: score(searchResults),
      queries: searchResults,
    };
    fs.writeFileSync(path.join(caseDir, "result.json"), JSON.stringify(result, null, 2));
    return result;
  });
}

function normalizedCombined(results) {
  const ok = results.filter((result) => !result.error);
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

function writeReports(results) {
  const header = [
    "rank",
    "id",
    "kind",
    "hardware",
    "profile",
    "batch",
    "sessions",
    "requests",
    "index_seconds",
    "semantic_seconds",
    "semantic_docs",
    "docs_per_second",
    "hit_at_1",
    "hit_at_10",
    "mrr_at_10",
    "mean_rank_when_found",
    "combined_score",
    "misses",
  ];
  const ranked = normalizedCombined(results);
  const rankById = new Map(ranked.map((result, index) => [result.id, index + 1]));
  const rows = results.map((result) => {
    if (result.error) {
      return [
        "",
        result.id,
        result.kind,
        result.kind === "onnx" ? "DirectML" : "Vulkan",
        result.profile,
        batchSize,
        result.kind === "onnx" ? onnxSessions : "",
        result.kind === "llama" ? (result.requests ?? defaultLlamaParallel) : "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        result.error,
      ];
    }
    return [
      rankById.get(result.id),
      result.id,
      result.kind,
      result.hardware,
      result.profile,
      result.batch,
      result.sessions ?? "",
      result.requests ?? "",
      fmt(result.index_seconds),
      fmt(result.semantic_seconds),
      result.semantic_docs,
      fmt(result.docs_per_second),
      fmt(result.score.hit_at_1),
      fmt(result.score.hit_at_10),
      fmt(result.score.mrr_at_10),
      fmt(result.score.mean_rank_when_found),
      fmt(result.combined_score),
      result.score.misses.join(";"),
    ];
  });
  const csv = [header.join(","), ...rows.map((row) => row.map(csvEscape).join(","))].join("\n");
  fs.writeFileSync(path.join(outDir, "results.csv"), csv);
  fs.writeFileSync(path.join(outDir, "results.json"), JSON.stringify(results, null, 2));

  const md = [
    "# GPU Fair Embedding Benchmark",
    "",
    `Artifact root: \`${outDir}\``,
    "",
    "All rows use GPU-only embedding paths, batch size `128`, and the current alias-enriched semantic doc text.",
    "ONNX rows use DirectML. llama.cpp rows use Vulkan device `Vulkan0`.",
    "",
    "| Rank | Config | Hardware | MRR@10 | Hit@10 | Docs/sec | Combined score |",
    "| ---: | --- | --- | ---: | ---: | ---: | ---: |",
    ...ranked.map(
      (result, index) =>
        `| ${index + 1} | \`${result.id}\` | ${result.hardware} | ${fmt(result.score.mrr_at_10)} | ${fmt(
          result.score.hit_at_10,
        )} | ${fmt(result.docs_per_second)} | ${fmt(result.combined_score)} |`,
    ),
    "",
    "Combined score: `0.70 * normalized(MRR@10) + 0.30 * normalized(docs/sec)` within this fair GPU-only run.",
  ].join("\n");
  fs.writeFileSync(path.join(outDir, "report.md"), md);
}

const results = [];
fs.mkdirSync(outDir, { recursive: true });
for (const profile of selectedProfiles()) {
  try {
    results.push(await runProfile(profile));
  } catch (error) {
    console.error(`${profile.id} failed: ${error.message}`);
    results.push({
      id: profile.id,
      kind: profile.kind,
      profile: profile.profile,
      requests: profile.kind === "llama" ? llamaRequestCount(profile) : null,
      parallel: profile.kind === "llama" ? llamaParallelSlots(profile) : null,
      error: error.message,
    });
  }
}
writeReports(results);
console.log(`wrote ${path.join(outDir, "results.csv")}`);
