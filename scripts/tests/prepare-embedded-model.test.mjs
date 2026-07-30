import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  link,
  mkdir,
  mkdtemp,
  readdir,
  readFile,
  realpath,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const script = resolve(repositoryRoot, "scripts/prepare-embedded-model.mjs");
const buildScript = resolve(repositoryRoot, "crates/codestory-llama-sys/build.rs");
const modelStaging = resolve(repositoryRoot, "crates/codestory-llama-sys/model_staging.rs");
const nativeStaging = resolve(repositoryRoot, "crates/codestory-llama-sys/native_staging.rs");
const canonicalContract = resolve(
  repositoryRoot,
  "crates/codestory-llama-sys/model-contract.json",
);
const llamaSource = resolve(repositoryRoot, "crates/codestory-llama-sys/src/lib.rs");
const retrievalEmbeddings = resolve(repositoryRoot, "crates/codestory-retrieval/src/embeddings.rs");
const retrievalEmbeddingContract = resolve(
  repositoryRoot,
  "crates/codestory-retrieval/src/embedding_contract.rs",
);
const embeddedVector = resolve(repositoryRoot, "crates/codestory-retrieval/src/embedded_vector.rs");

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function contractDigest(domain, value) {
  const hash = createHash("sha256");
  for (const text of [domain, value]) {
    const bytes = Buffer.from(text);
    const length = Buffer.alloc(8);
    length.writeBigUInt64LE(BigInt(bytes.length));
    hash.update(length);
    hash.update(bytes);
  }
  return hash.digest("hex");
}

async function fixture(t, expected = Buffer.from("good"), sourceCount = 1) {
  const directory = await realpath(
    await mkdtemp(resolve(tmpdir(), "codestory-model-contract-")),
  );
  t.after(() => rm(directory, { force: true, recursive: true }));
  const contract = resolve(directory, "contract.json");
  const modelSha256 = sha256(expected);
  const revision = "0123456789abcdef0123456789abcdef01234567";
  const urls = ["example.invalid", "mirror.example.invalid"]
    .slice(0, sourceCount)
    .map(host => `https://${host}/resolve/${revision}/model.gguf`);
  await writeFile(
    contract,
    JSON.stringify({
      schema_version: 1,
      model: {
        file_name: "model.gguf",
        size_bytes: expected.length,
        sha256: modelSha256,
        sources: urls.map(url => ({ url, revision })),
      },
      runtime: {
        embedding_family: "test",
        llama_cpp_crate_version: "test",
        llama_cpp_source_commit: revision,
      },
      embedding: {
        dimension: 768,
        query_prefix: "Represent this query for searching relevant code: ",
        document_prefix: "",
        pooling: "cls",
        normalization: "l2",
        element_type: "f32_le",
        vector_schema_version: 2,
      },
      tokenizer_config: {
        container: "gguf",
        tokenizer_sha256: contractDigest("tokenizer", modelSha256),
        config_sha256: contractDigest("config", `${modelSha256}:768:cls:l2`),
      },
      producer: {
        name: "codestory-llama-sys",
        version: "0.16.0",
      },
      license: {
        spdx_id: "MIT",
        source_url: `https://example.invalid/model/${revision}/license`,
      },
    }),
  );
  return { contract, directory, expected, urls };
}

function run(args, cwd, options = {}) {
  const { env, nodeArgs = [] } = options;
  return spawnSync(process.execPath, [...nodeArgs, script, ...args], {
    cwd,
    encoding: "utf8",
    env: env ? { ...process.env, ...env } : process.env,
  });
}

const fetchPreloadSource = `import { appendFileSync } from "node:fs";

const plan = (process.env.CODESTORY_TEST_FETCH_PLAN ?? "").split(",").filter(Boolean);
const body = Buffer.from(process.env.CODESTORY_TEST_FETCH_BODY ?? "", "base64");
const log = process.env.CODESTORY_TEST_FETCH_LOG;
let calls = 0;

globalThis.fetch = async url => {
  const step = plan[Math.min(calls, plan.length - 1)];
  calls += 1;
  if (log) appendFileSync(log, \`\${url}\\n\`);
  if (step === "ok") {
    return new Response(body, {
      status: 200,
      headers: { "content-length": String(body.length) },
    });
  }
  if (step === "corrupt") {
    const corrupt = Buffer.from(body);
    corrupt.reverse();
    return new Response(corrupt, {
      status: 200,
      headers: { "content-length": String(corrupt.length) },
    });
  }
  return new Response(
    new ReadableStream({
      start(controller) {
        controller.enqueue(body.subarray(0, Math.min(2, body.length)));
        controller.error(new Error("other side closed"));
      },
    }),
    { status: 200, headers: { "content-length": String(body.length) } },
  );
};
`;

async function transportFixture(t, plan, expected = Buffer.from("good"), sourceCount = 1) {
  const base = await fixture(t, expected, sourceCount);
  const preload = resolve(base.directory, "fetch-preload.mjs");
  const log = resolve(base.directory, "fetch-calls.log");
  await writeFile(preload, fetchPreloadSource);
  return {
    ...base,
    log,
    env: {
      CODESTORY_TEST_FETCH_PLAN: plan,
      CODESTORY_TEST_FETCH_BODY: expected.toString("base64"),
      CODESTORY_TEST_FETCH_LOG: log,
    },
    nodeArgs: ["--import", pathToFileURL(preload).href],
  };
}

async function fetchCalls(log) {
  try {
    return (await readFile(log, "utf8")).split("\n").filter(Boolean);
  } catch (error) {
    if (error?.code === "ENOENT") return [];
    throw error;
  }
}

test("copies and verifies an explicit source while offline", async (t) => {
  const { contract, directory, expected } = await fixture(t);
  const source = resolve(directory, "source.gguf");
  const output = resolve(directory, "output.gguf");
  await writeFile(source, expected);

  const result = run(
    ["--contract", contract, "--source", source, "--output", output, "--offline"],
    directory,
  );

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), output);
  assert.deepEqual(await readFile(output), expected);
});

test("persistent content root is keyed solely by the checked-in model SHA", async (t) => {
  const { contract, directory, expected } = await fixture(t);
  const source = resolve(directory, "source.gguf");
  const cacheRoot = resolve(directory, "persistent-model-cache");
  await writeFile(source, expected);

  const first = run(
    ["--contract", contract, "--source", source, "--cache-root", cacheRoot, "--offline"],
    directory,
  );

  assert.equal(first.status, 0, first.stderr);
  const expectedPath = resolve(cacheRoot, "sha256", sha256(expected), "model.gguf");
  assert.equal(first.stdout.trim(), expectedPath);
  assert.deepEqual(await readFile(expectedPath), expected);

  await writeFile(source, "baad");
  const hit = run(
    ["--contract", contract, "--cache-root", cacheRoot, "--offline"],
    directory,
  );
  assert.equal(hit.status, 0, hit.stderr);
  assert.equal(hit.stdout.trim(), expectedPath);
  assert.deepEqual(await readFile(expectedPath), expected);
});

test("persistent content root revalidates bytes before reporting a hit", async (t) => {
  const { contract, directory, expected } = await fixture(t);
  const source = resolve(directory, "source.gguf");
  const cacheRoot = resolve(directory, "persistent-model-cache");
  await writeFile(source, expected);

  const first = run(
    ["--contract", contract, "--source", source, "--cache-root", cacheRoot, "--offline"],
    directory,
  );
  assert.equal(first.status, 0, first.stderr);
  await writeFile(first.stdout.trim(), "baad");

  const rejected = run(
    ["--contract", contract, "--cache-root", cacheRoot, "--offline"],
    directory,
  );
  assert.notEqual(rejected.status, 0);
  assert.match(rejected.stderr, /offline model preparation requires/u);
});

test("persistent content root rejects a hardlinked cache destination", async (t) => {
  const { contract, directory, expected } = await fixture(t);
  const source = resolve(directory, "source.gguf");
  const cacheRoot = resolve(directory, "persistent-model-cache");
  await writeFile(source, expected);

  const first = run(
    ["--contract", contract, "--source", source, "--cache-root", cacheRoot, "--offline"],
    directory,
  );
  assert.equal(first.status, 0, first.stderr);
  await link(first.stdout.trim(), resolve(directory, "external-hardlink.gguf"));

  const rejected = run(
    ["--contract", contract, "--cache-root", cacheRoot, "--offline"],
    directory,
  );
  assert.notEqual(rejected.status, 0);
  assert.match(rejected.stderr, /offline model preparation requires/u);
});

test("persistent content root rejects a symlinked or reparse cache root", async (t) => {
  const { contract, directory, expected } = await fixture(t);
  const source = resolve(directory, "source.gguf");
  const external = resolve(directory, "external-cache");
  const cacheRoot = resolve(directory, "linked-cache");
  await writeFile(source, expected);
  await mkdir(external);
  await symlink(
    external,
    cacheRoot,
    process.platform === "win32" ? "junction" : "dir",
  );

  const rejected = run(
    ["--contract", contract, "--source", source, "--cache-root", cacheRoot, "--offline"],
    directory,
  );

  assert.notEqual(rejected.status, 0);
  assert.match(rejected.stderr, /must not traverse symbolic links, reparse points, or files/u);
  assert.deepEqual(await readdir(external), []);
});

test("persistent content root rejects a symlinked or reparse digest ancestor", async (t) => {
  const { contract, directory, expected } = await fixture(t);
  const source = resolve(directory, "source.gguf");
  const external = resolve(directory, "external-sha256");
  const cacheRoot = resolve(directory, "persistent-model-cache");
  await writeFile(source, expected);
  await mkdir(cacheRoot);
  await mkdir(external);
  await symlink(
    external,
    resolve(cacheRoot, "sha256"),
    process.platform === "win32" ? "junction" : "dir",
  );

  const rejected = run(
    ["--contract", contract, "--source", source, "--cache-root", cacheRoot, "--offline"],
    directory,
  );

  assert.notEqual(rejected.status, 0);
  assert.match(rejected.stderr, /must not traverse symbolic links, reparse points, or files/u);
  assert.deepEqual(await readdir(external), []);
});

test("explicit output cannot weaken the persistent content-addressed path", async (t) => {
  const { contract, directory, expected } = await fixture(t);
  const source = resolve(directory, "source.gguf");
  await writeFile(source, expected);

  const result = run(
    [
      "--contract",
      contract,
      "--source",
      source,
      "--cache-root",
      resolve(directory, "persistent-model-cache"),
      "--output",
      resolve(directory, "arbitrary.gguf"),
    ],
    directory,
  );

  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /mutually exclusive/u);
});

test("rejects a missing explicit source", async (t) => {
  const { contract, directory } = await fixture(t);
  const missing = resolve(directory, "missing.gguf");
  const output = resolve(directory, "output.gguf");

  const result = run(
    ["--contract", contract, "--source", missing, "--output", output],
    directory,
  );

  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /invalid pinned model source/u);
});

test("rejects a same-size source with the wrong digest", async (t) => {
  const { contract, directory } = await fixture(t);
  const source = resolve(directory, "source.gguf");
  const output = resolve(directory, "output.gguf");
  await writeFile(source, "baad");

  const result = run(
    ["--contract", contract, "--source", source, "--output", output],
    directory,
  );

  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /invalid pinned model source/u);
});

test("offline acquisition fails before any download", async (t) => {
  const { contract, directory } = await fixture(t);
  const output = resolve(directory, "output.gguf");

  const result = run(["--contract", contract, "--output", output, "--offline"], directory);

  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /offline model preparation requires/u);
});

test("retries a transport failure and succeeds within the attempt budget", async (t) => {
  const { contract, directory, env, expected, log, nodeArgs, urls } = await transportFixture(
    t,
    "closed,ok",
  );
  const output = resolve(directory, "output.gguf");

  const result = run(["--contract", contract, "--output", output], directory, { env, nodeArgs });

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), output);
  assert.deepEqual(await readFile(output), expected);
  assert.deepEqual(await fetchCalls(log), [urls[0], urls[0]]);
});

test("retries a corrupt payload so the checksum gates every attempt", async (t) => {
  const { contract, directory, env, expected, log, nodeArgs, urls } = await transportFixture(
    t,
    "corrupt,ok",
  );
  const output = resolve(directory, "output.gguf");

  const result = run(["--contract", contract, "--output", output], directory, { env, nodeArgs });

  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(await readFile(output), expected);
  assert.deepEqual(await fetchCalls(log), [urls[0], urls[0]]);
});

test("falls through to the next pinned source after exhausting one source", async (t) => {
  const { contract, directory, env, expected, log, nodeArgs, urls } = await transportFixture(
    t,
    "closed,closed,closed,ok",
    Buffer.from("good"),
    2,
  );
  const output = resolve(directory, "output.gguf");

  const result = run(["--contract", contract, "--output", output], directory, { env, nodeArgs });

  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(await readFile(output), expected);
  assert.deepEqual(await fetchCalls(log), [urls[0], urls[0], urls[0], urls[1]]);
});

test("fails closed with the aggregated error after bounded attempts", async (t) => {
  const { contract, directory, env, log, nodeArgs, urls } = await transportFixture(t, "closed");
  const output = resolve(directory, "output.gguf");

  const result = run(["--contract", contract, "--output", output], directory, { env, nodeArgs });

  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /model download failed after 3 attempts/u);
  assert.match(result.stderr, /other side closed/u);
  assert.deepEqual(await fetchCalls(log), [urls[0], urls[0], urls[0]]);
  await assert.rejects(readFile(output), { code: "ENOENT" });
  const leftovers = (await readdir(directory)).filter(name => name.endsWith(".partial"));
  assert.deepEqual(leftovers, []);
});

test("rejects a symlink model destination without replacing it", async (t) => {
  const { contract, directory, expected } = await fixture(t);
  const source = resolve(directory, "source.gguf");
  const output = resolve(directory, "output.gguf");
  await writeFile(source, expected);
  try {
    await symlink(source, output, "file");
  } catch (error) {
    if (error?.code === "EPERM") {
      t.skip("file symlinks are unavailable on this Windows host");
      return;
    }
    throw error;
  }

  const result = run(["--contract", contract, "--output", output, "--offline"], directory);

  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /model destination is not a regular file/u);
  assert.deepEqual(await readFile(output), expected);
});

test("Cargo's build boundary is process-free and release input is explicit", async () => {
  const source = `${await readFile(buildScript, "utf8")}\n${await readFile(modelStaging, "utf8")}`;

  assert.doesNotMatch(source, /std::process|Command::new|prepare_release_model/u);
  assert.doesNotMatch(source, /HOME|USERPROFILE|build-assets|download|fetch/u);
  assert.match(source, /release builds require CODESTORY_EMBED_MODEL_SOURCE/u);
});

test("acquisition, build, and Rust evidence consume the checked-in contract", async () => {
  const [acquisition, build, native, contract, llama, embeddings, embeddingContract, vectors] =
    await Promise.all([
      readFile(script, "utf8"),
      readFile(buildScript, "utf8"),
      readFile(nativeStaging, "utf8"),
      readFile(canonicalContract, "utf8"),
      readFile(llamaSource, "utf8"),
      readFile(retrievalEmbeddings, "utf8"),
      readFile(retrievalEmbeddingContract, "utf8"),
      readFile(embeddedVector, "utf8"),
    ]);
  const parsed = JSON.parse(contract);

  assert.equal(parsed.schema_version, 1);
  assert.equal(parsed.model.sources.length, 2);
  assert.equal(parsed.embedding.dimension, 768);
  assert.equal(parsed.embedding.pooling, "cls");
  assert.equal(parsed.embedding.normalization, "l2");
  assert.equal(parsed.license.spdx_id, "MIT");
  assert.match(acquisition, /model-contract\.json/u);
  assert.match(build, /model-contract\.json/u);
  assert.doesNotMatch(acquisition, new RegExp(parsed.model.sha256, "u"));
  assert.doesNotMatch(build, new RegExp(parsed.model.sha256, "u"));
  assert.match(build, /DEP_LLAMA_BACKENDS_DIR/u);
  assert.match(native, /codestory-native-runtime-files-v1\.txt/u);
  assert.match(llama, /CompiledModelCompatibility/u);
  assert.match(embeddings, /crate::embedding_contract::CODERANK_QUERY_PREFIX/u);
  assert.match(embeddingContract, /pub\(crate\) const EMBEDDING_NORMALIZATION/u);
  assert.match(embeddingContract, /pub\(crate\) const EMBEDDING_MODEL_ID/u);
  assert.match(embeddingContract, /normalize_and_validate_vectors/u);
  assert.match(embeddingContract, /NativeBackendRequest/u);
  assert.match(vectors, /codestory_llama_sys::MODEL_TOKENIZER_SHA256/u);
  assert.match(vectors, /codestory_llama_sys::MODEL_CONFIG_SHA256/u);
  assert.match(vectors, /crate::embedding_contract::EMBEDDING_ELEMENT_TYPE/u);
  assert.match(vectors, /crate::embedding_contract::EMBEDDING_MODEL_SHA256/u);
  assert.match(vectors, /codestory_llama_sys::MODEL_PRODUCER_NAME/u);
  assert.match(vectors, /codestory_llama_sys::MODEL_PRODUCER_VERSION/u);
});
