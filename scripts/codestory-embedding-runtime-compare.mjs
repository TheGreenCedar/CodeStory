#!/usr/bin/env node
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, statSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const marker = "CODESTORY_EMBEDDING_IDENTITY_PROBE_JSON=";
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const query = "Where is the retrieval sidecar embedding contract enforced?";
const docs = [
  "llama.cpp embedding sidecar is mandatory for product retrieval_mode full.",
  "Managed ONNX assets are diagnostic until fresh quality evidence exists.",
  "Hash projection is deterministic but not semantic product readiness.",
  "Packet and search readiness require full sidecar retrieval evidence.",
];

function main() {
  if (process.argv.includes("--self-test")) {
    selfTest();
    console.log("self-test ok");
    return;
  }

  const llama = runProbe("llama_control_208", llamaEnv());
  const onnx = runProbe("onnx_candidate_diagnostic", onnxEnv());
  const report = {
    diagnostic_only: true,
    product_default_changes: false,
    full_ab_suite: false,
    hash_projection_product_readiness: "rejected",
    comparators: {
      llama_control_208: labelResult(llama, true),
      onnx_candidate_diagnostic: labelResult(onnx, false),
    },
    top_k_overlap: topKOverlap(llama, onnx),
  };

  console.log(JSON.stringify(report, null, 2));
}

function runProbe(name, extraEnv) {
  const env = {
    ...process.env,
    ...extraEnv,
    CODESTORY_EMBED_PROFILE: "bge-base-en-v1.5",
    CODESTORY_EMBED_IDENTITY_PROBE_QUERY: query,
    CODESTORY_EMBED_IDENTITY_PROBE_DOCS_JSON: JSON.stringify(docs),
  };
  if (!env.RUSTC_WRAPPER && process.env.USERPROFILE) {
    env.RUSTC_WRAPPER = path.join(process.env.USERPROFILE, ".cargo", "bin", "sccache.exe");
  }

  const args = [
    "test",
    "-p",
    "codestory-runtime",
    "embedding_identity_probe_from_env",
    "--",
    "--ignored",
    "--nocapture",
  ];
  const result = spawnSync("cargo", args, {
    cwd: repoRoot,
    env,
    encoding: "utf8",
    maxBuffer: 20 * 1024 * 1024,
  });
  const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
  const line = output.split(/\r?\n/).find((item) => item.startsWith(marker));
  let parsed = null;
  if (line) {
    parsed = JSON.parse(line.slice(marker.length));
  }
  return {
    name,
    command: `cargo ${args.join(" ")}`,
    exit_code: result.status,
    ...(
      parsed ?? {
        ok: false,
        failure_text: compactFailure(output) || result.error?.message || "probe emitted no JSON marker",
      }
    ),
  };
}

function llamaEnv() {
  return {
    CODESTORY_EMBED_BACKEND: "llamacpp",
    CODESTORY_EMBED_LLAMACPP_URL:
      process.env.CODESTORY_EMBED_LLAMACPP_URL ?? "http://127.0.0.1:8080/v1/embeddings",
  };
}

function onnxEnv() {
  const env = {
    CODESTORY_EMBED_BACKEND: "onnx",
    CODESTORY_EMBED_ONNX_PROVIDER: process.env.CODESTORY_EMBED_ONNX_PROVIDER ?? defaultOnnxProvider(),
  };
  if (process.env.CODESTORY_EMBED_ONNX_MODEL && process.env.CODESTORY_EMBED_ONNX_TOKENIZER) {
    env.CODESTORY_EMBED_ONNX_MODEL = process.env.CODESTORY_EMBED_ONNX_MODEL;
    env.CODESTORY_EMBED_ONNX_TOKENIZER = process.env.CODESTORY_EMBED_ONNX_TOKENIZER;
    return env;
  }

  const managed = findManagedOnnxAssets();
  if (managed) {
    env.CODESTORY_EMBED_ONNX_MODEL = managed.model;
    env.CODESTORY_EMBED_ONNX_TOKENIZER = managed.tokenizer;
  }
  return env;
}

function defaultOnnxProvider() {
  return process.platform === "win32" ? "directml" : "cpu";
}

function findManagedOnnxAssets() {
  const roots = [
    process.env.CODESTORY_MANAGED_EMBEDDINGS_ROOT,
    process.env.CODESTORY_CACHE_DIR && path.join(process.env.CODESTORY_CACHE_DIR, "managed-embeddings"),
    ...commonManagedRoots(),
  ].filter(Boolean);
  for (const root of roots) {
    const fromManifest = readManagedManifest(root);
    if (fromManifest) {
      return fromManifest;
    }
    const model = path.join(root, "models", "bge-base-en-v1.5-onnx-qdrant", "model_optimized_cls_pool.onnx");
    const tokenizer = path.join(root, "models", "bge-base-en-v1.5-onnx-qdrant", "tokenizer.json");
    if (existsSync(model) && existsSync(tokenizer)) {
      return { model, tokenizer };
    }
  }
  return null;
}

function commonManagedRoots() {
  if (process.platform === "win32") {
    const local = process.env.LOCALAPPDATA;
    if (!local) {
      return [];
    }
    return [
      path.join(local, "codestory", "codestory", "cache", "managed-embeddings"),
      path.join(local, "dev", "codestory", "codestory", "cache", "managed-embeddings"),
    ];
  }
  const home = os.homedir();
  return [
    path.join(home, ".cache", "codestory", "managed-embeddings"),
    path.join(home, ".cache", "dev", "codestory", "managed-embeddings"),
  ];
}

function readManagedManifest(root) {
  const manifestPath = path.join(root, "manifest.json");
  if (!existsSync(manifestPath)) {
    return null;
  }
  try {
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    const model = manifest.onnx_model_path;
    const tokenizer = manifest.onnx_tokenizer_path;
    if (model && tokenizer && existsSync(model) && existsSync(tokenizer)) {
      return { model, tokenizer };
    }
  } catch {
    return null;
  }
  return null;
}

function labelResult(result, productEligible) {
  const backend = result.name.startsWith("onnx") ? "onnx" : "llamacpp";
  return {
    ...result,
    backend,
    cache_bytes: result.cache_bytes ?? cacheBytesFromResult(result),
    diagnostic_label: productEligible
      ? "#208 llama sidecar control comparator"
      : "diagnostic only; cannot satisfy product retrieval_mode=full by itself",
    product_full_retrieval_eligible: productEligible,
  };
}

function cacheBytesFromResult(result) {
  if (!result.model_path || /^https?:/.test(result.model_path)) {
    return null;
  }
  try {
    return statSync(result.model_path).size;
  } catch {
    return null;
  }
}

function topKOverlap(left, right) {
  if (!left.ok || !right.ok) {
    return {
      status: "not_measured",
      reason: [left, right]
        .filter((item) => !item.ok)
        .map((item) => `${item.name}: ${item.failure_text ?? "unavailable"}`)
        .join("; "),
    };
  }
  const leftTop = topIndexes(left);
  const rightTop = topIndexes(right);
  const overlap = leftTop.filter((index) => rightTop.includes(index));
  return {
    status: "measured",
    k: Math.min(leftTop.length, rightTop.length),
    overlap_count: overlap.length,
    overlap_ratio: overlap.length / Math.max(1, Math.min(leftTop.length, rightTop.length)),
    llama_top_k: leftTop,
    onnx_top_k: rightTop,
  };
}

function topIndexes(result) {
  return (result.top_k ?? []).map((item) => item.index);
}

function compactFailure(output) {
  return output
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .slice(-12)
    .join("\n");
}

function selfTest() {
  const left = { name: "left", ok: true, top_k: [{ index: 1 }, { index: 2 }, { index: 3 }] };
  const right = { name: "right", ok: true, top_k: [{ index: 2 }, { index: 4 }, { index: 1 }] };
  assert.equal(topKOverlap(left, right).overlap_count, 2);
  assert.equal(labelResult({ name: "onnx_candidate_diagnostic", ok: true }, false).product_full_retrieval_eligible, false);
  assert.equal(labelResult({ name: "llama_control_208", ok: true }, true).diagnostic_label, "#208 llama sidecar control comparator");
}

main();
