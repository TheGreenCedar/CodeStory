#!/usr/bin/env node
import { createHash, randomBytes } from "node:crypto";
import { existsSync, lstatSync, realpathSync, statSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs as parseNodeArgs } from "node:util";

const CASES_CONTRACT = "codestory.evidence-compiler-builder-blinded-cases/v1";
const MAP_CONTRACT = "codestory.evidence-compiler-builder-blinded-map/v1";
const JUDGMENTS_CONTRACT = "codestory.evidence-compiler-builder-blinded-judgments/v1";
const ADJUDICATION_CONTRACT = "codestory.evidence-compiler-builder-adjudication/v1";
const ADJUDICATED_ARMS = new Set([
  "exact_identity_source",
  "exact_plus_relations",
  "packet_semantic_off",
  "packet_semantic_on",
]);
const EXPECTED_CASES = 96;
const MAX_TRANSCRIPT_BYTES = 16 * 1024 * 1024;

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function parseJsonl(text, label) {
  return text.split(/\r?\n/u).filter(Boolean).map((line, index) => {
    try {
      return JSON.parse(line);
    } catch (error) {
      throw new Error(`invalid ${label} row ${index + 1}: ${error.message}`);
    }
  });
}

function pathInside(root, candidate, label) {
  const resolvedRoot = path.resolve(root);
  const resolved = path.resolve(candidate);
  const relative = path.relative(resolvedRoot, resolved);
  if (!relative || relative === "." || relative.startsWith("..") || path.isAbsolute(relative)) {
    throw new Error(`${label} must be a file inside ${resolvedRoot}`);
  }
  return resolved;
}

function artifactPath(runDir, value, label) {
  const raw = String(value ?? "").trim();
  if (!raw) throw new Error(`${label} is missing`);
  const candidate = pathInside(
    runDir,
    path.isAbsolute(raw) ? raw : path.join(runDir, raw),
    label,
  );
  const metadata = lstatSync(candidate);
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error(`${label} must name a regular non-symlink file`);
  }
  return pathInside(realpathSync(runDir), realpathSync(candidate), label);
}

function extractFinalAnswer(events) {
  let answer = null;
  for (const event of events) {
    const eventType = String(event?.type ?? event?.event ?? "").toLowerCase();
    const item = event?.item && typeof event.item === "object" ? event.item : {};
    if (
      (eventType === "item.completed" || eventType.endsWith(".completed")) &&
      item.type === "agent_message" &&
      typeof item.text === "string"
    ) {
      answer = item.text;
    }
  }
  if (!answer?.trim()) throw new Error("runner transcript contains no final agent answer");
  return answer;
}

async function answerFromRow(runDir, row) {
  const transcriptPath = artifactPath(runDir, row.stdout_path, "row stdout_path");
  const size = statSync(transcriptPath).size;
  if (size > MAX_TRANSCRIPT_BYTES) {
    throw new Error(`runner transcript exceeds ${MAX_TRANSCRIPT_BYTES} bytes: ${transcriptPath}`);
  }
  return extractFinalAnswer(parseJsonl(await readFile(transcriptPath, "utf8"), "runner transcript"));
}

function opaqueCaseId(used) {
  for (;;) {
    const candidate = randomBytes(12).toString("hex");
    if (!used.has(candidate)) {
      used.add(candidate);
      return candidate;
    }
  }
}

async function prepareBlindedCases({ runDir, outputDir }) {
  const runsPath = path.join(runDir, "runs.jsonl");
  if (!existsSync(runsPath)) throw new Error(`missing ${runsPath}`);
  const runBytes = await readFile(runsPath);
  const sourceRunsSha256 = sha256(runBytes);
  const rows = parseJsonl(runBytes.toString("utf8"), "runs.jsonl")
    .filter((row) => ADJUDICATED_ARMS.has(row.arm));
  if (rows.length !== EXPECTED_CASES) {
    throw new Error(`expected ${EXPECTED_CASES} CodeStory rows for blinded adjudication; received ${rows.length}`);
  }
  const keys = new Set(rows.map((row) => `${row.task_id}\t${row.arm}\t${row.repeat}`));
  if (keys.size !== rows.length) throw new Error("packet/control rows are not unique");

  const usedIds = new Set();
  const cases = [];
  const entries = [];
  for (const row of rows) {
    const answer = await answerFromRow(runDir, row);
    const caseId = opaqueCaseId(usedIds);
    const answerSha256 = sha256(Buffer.from(answer, "utf8"));
    cases.push({
      case_id: caseId,
      repository: row.repo,
      repository_path: row.repo_path,
      repository_commit: row.repo_provenance?.git_head ?? null,
      question: row.task_manifest_snapshot?.prompt ?? null,
      answer,
      answer_sha256: answerSha256,
    });
    entries.push({
      case_id: caseId,
      task_id: row.task_id,
      arm: row.arm,
      repeat: row.repeat,
      answer_sha256: answerSha256,
    });
  }
  cases.sort((left, right) => left.case_id.localeCompare(right.case_id));
  entries.sort((left, right) => left.case_id.localeCompare(right.case_id));
  const casesPayload = {
    contract: CASES_CONTRACT,
    source_runs_sha256: sourceRunsSha256,
    instructions: {
      critical_factual_error: "Count each unique materially wrong statement about the requested code behavior that could change the answer's conclusion.",
      unsupported_relation_claim: "Count each unique material call, import, implementation, data-flow, or control-flow claim that is not established by the answer's cited source or the pinned repository source.",
      output: "Return the judgments contract with every case_id exactly once. Include nonnegative integer counts plus unique critical_factual_finding_ids and unsupported_relation_finding_ids arrays of the same lengths. Reuse one stable finding id when two answers make the same error. Add concise source-backed notes. Do not identify or infer experimental arms.",
    },
    cases,
  };
  const casesBytes = Buffer.from(`${JSON.stringify(casesPayload, null, 2)}\n`);
  const casesSha256 = sha256(casesBytes);
  const mapPayload = {
    contract: MAP_CONTRACT,
    source_runs_sha256: sourceRunsSha256,
    source_cases_sha256: casesSha256,
    entries,
  };
  await mkdir(outputDir, { recursive: true });
  const casesPath = path.join(outputDir, "blinded-cases.json");
  const mapPath = path.join(outputDir, "private-case-map.json");
  await writeFile(casesPath, casesBytes, { mode: 0o600 });
  await writeFile(mapPath, `${JSON.stringify(mapPayload, null, 2)}\n`, { mode: 0o600 });
  return { casesPath, mapPath, casesSha256, sourceRunsSha256 };
}

function integerCount(value, label) {
  if (!Number.isInteger(value) || value < 0) throw new Error(`${label} must be a nonnegative integer`);
  return value;
}

function findingIds(value, expectedCount, label) {
  if (!Array.isArray(value) || value.length !== expectedCount) {
    throw new Error(`${label} must contain exactly ${expectedCount} finding ids`);
  }
  const ids = value.map((entry) => String(entry ?? "").trim());
  if (
    ids.some((entry) => !entry || entry.length > 256) ||
    new Set(ids).size !== ids.length
  ) {
    throw new Error(`${label} must contain unique nonempty ids of at most 256 characters`);
  }
  return ids;
}

async function finalizeAdjudication({ runDir, casesPath, mapPath, judgmentsPath, outputPath }) {
  const runBytes = await readFile(path.join(runDir, "runs.jsonl"));
  const sourceRunsSha256 = sha256(runBytes);
  const casesBytes = await readFile(casesPath);
  const cases = JSON.parse(casesBytes.toString("utf8"));
  const mapping = JSON.parse(await readFile(mapPath, "utf8"));
  const judgmentBytes = await readFile(judgmentsPath);
  const judgments = JSON.parse(judgmentBytes.toString("utf8"));
  const sourceCasesSha256 = sha256(casesBytes);
  if (
    cases.contract !== CASES_CONTRACT ||
    mapping.contract !== MAP_CONTRACT ||
    judgments.contract !== JUDGMENTS_CONTRACT
  ) {
    throw new Error("blinded adjudication contract mismatch");
  }
  if (
    cases.source_runs_sha256 !== sourceRunsSha256 ||
    mapping.source_runs_sha256 !== sourceRunsSha256 ||
    mapping.source_cases_sha256 !== sourceCasesSha256 ||
    judgments.source_cases_sha256 !== sourceCasesSha256
  ) {
    throw new Error("blinded adjudication inputs do not bind the exact run and case bytes");
  }
  if (!judgments.independent_reviewer || !Array.isArray(judgments.rows)) {
    throw new Error("judgments must name an independent reviewer and contain rows");
  }
  const caseIds = new Set(cases.cases?.map((entry) => entry.case_id) ?? []);
  const mapById = new Map(mapping.entries?.map((entry) => [entry.case_id, entry]) ?? []);
  const judgmentById = new Map(judgments.rows.map((entry) => [entry.case_id, entry]));
  if (
    caseIds.size !== EXPECTED_CASES ||
    mapById.size !== EXPECTED_CASES ||
    judgmentById.size !== EXPECTED_CASES ||
    judgments.rows.length !== EXPECTED_CASES ||
    [...caseIds].some((caseId) => !mapById.has(caseId) || !judgmentById.has(caseId))
  ) {
    throw new Error(`blinded cases, private map, and judgments must contain the same ${EXPECTED_CASES} unique case ids`);
  }
  const rows = [...caseIds].map((caseId) => {
    const mapped = mapById.get(caseId);
    const judgment = judgmentById.get(caseId);
    const answer = cases.cases.find((entry) => entry.case_id === caseId);
    if (
      mapped.answer_sha256 !== answer?.answer_sha256 ||
      answer?.answer_sha256 !== sha256(Buffer.from(String(answer?.answer ?? ""), "utf8"))
    ) {
      throw new Error(`answer identity mismatch for blinded case ${caseId}`);
    }
    const criticalFactualErrors = integerCount(
      judgment.critical_factual_errors,
      `${caseId} critical_factual_errors`,
    );
    const unsupportedRelationClaims = integerCount(
      judgment.unsupported_relation_claims,
      `${caseId} unsupported_relation_claims`,
    );
    return {
      task_id: mapped.task_id,
      arm: mapped.arm,
      repeat: mapped.repeat,
      critical_factual_errors: criticalFactualErrors,
      critical_factual_finding_ids: findingIds(
        judgment.critical_factual_finding_ids,
        criticalFactualErrors,
        `${caseId} critical_factual_finding_ids`,
      ),
      unsupported_relation_claims: unsupportedRelationClaims,
      unsupported_relation_finding_ids: findingIds(
        judgment.unsupported_relation_finding_ids,
        unsupportedRelationClaims,
        `${caseId} unsupported_relation_finding_ids`,
      ),
      notes: String(judgment.notes ?? "").trim() || null,
    };
  }).sort((left, right) =>
    left.task_id.localeCompare(right.task_id) ||
    left.arm.localeCompare(right.arm) ||
    left.repeat - right.repeat
  );
  const output = {
    contract: ADJUDICATION_CONTRACT,
    blinded: true,
    independent_reviewer: judgments.independent_reviewer,
    source_runs_sha256: sourceRunsSha256,
    source_cases_sha256: sourceCasesSha256,
    source_judgments_sha256: sha256(judgmentBytes),
    rows,
  };
  await writeFile(outputPath, `${JSON.stringify(output, null, 2)}\n`, { mode: 0o600 });
  return output;
}

function parseArgs(argv) {
  const { values } = parseNodeArgs({
    args: argv,
    allowPositionals: false,
    strict: true,
    options: {
      prepare: { type: "boolean" },
      finalize: { type: "boolean" },
      "run-dir": { type: "string" },
      "output-dir": { type: "string" },
      cases: { type: "string" },
      map: { type: "string" },
      judgments: { type: "string" },
      output: { type: "string" },
    },
  });
  if (Boolean(values.prepare) === Boolean(values.finalize) || !values["run-dir"]) {
    throw new Error("select exactly one of --prepare or --finalize and provide --run-dir");
  }
  if (values.prepare && !values["output-dir"]) {
    throw new Error("--prepare requires --output-dir");
  }
  if (values.finalize && (!values.cases || !values.map || !values.judgments || !values.output)) {
    throw new Error("--finalize requires --cases, --map, --judgments, and --output");
  }
  return {
    mode: values.prepare ? "prepare" : "finalize",
    runDir: path.resolve(values["run-dir"]),
    outputDir: values["output-dir"] ? path.resolve(values["output-dir"]) : null,
    casesPath: values.cases ? path.resolve(values.cases) : null,
    mapPath: values.map ? path.resolve(values.map) : null,
    judgmentsPath: values.judgments ? path.resolve(values.judgments) : null,
    outputPath: values.output ? path.resolve(values.output) : null,
  };
}

async function main(argv) {
  const opts = parseArgs(argv);
  if (opts.mode === "prepare") {
    const receipt = await prepareBlindedCases(opts);
    console.log(`wrote ${receipt.casesPath}`);
    console.log(`withheld ${receipt.mapPath}`);
    return;
  }
  await finalizeAdjudication(opts);
  console.log(`wrote ${opts.outputPath}`);
}

export {
  ADJUDICATION_CONTRACT,
  CASES_CONTRACT,
  JUDGMENTS_CONTRACT,
  MAP_CONTRACT,
  extractFinalAnswer,
  finalizeAdjudication,
  parseArgs,
  prepareBlindedCases,
};

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main(process.argv.slice(2)).catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exit(1);
  });
}
