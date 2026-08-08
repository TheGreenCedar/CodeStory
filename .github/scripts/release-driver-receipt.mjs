#!/usr/bin/env node

import { readFileSync, renameSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

export const RECEIPT_SCHEMA = "codestory.release-driver-receipt/v1";

const CONSTANT_SET_PATH =
  "crates/codestory-llama-sys/per-user-embedding-server-constant-set.json";
const PHASES = ["pre-freeze", "frozen", "published", "closeout"];
const RUN_GROUPS = new Set([
  "calibration-source-acceptance",
  "calibration",
  "frozen-candidate-acceptance",
  "source-proof",
  "package",
  "hardware",
  "installed-candidate",
  "qualification",
]);
const PRE_FREEZE_GROUPS = [
  "calibration-source",
  "pull-requests",
  "calibration-source-acceptance",
  "evidence",
  "next-action",
];
const FROZEN_GROUPS = [
  ...PRE_FREEZE_GROUPS,
  "calibration",
  "frozen-candidate",
  "frozen-candidate-acceptance",
  "source-proof",
  "package",
  "hardware",
  "installed-candidate",
  "qualification",
  "pre-publish-ledger",
];
const PUBLISHED_GROUPS = [
  ...FROZEN_GROUPS,
  "promotion",
  "publication",
];
const CLOSEOUT_GROUPS = [
  ...PUBLISHED_GROUPS,
  "post-publish-ledger",
  "catalog-delivery",
];
const OPTIONAL_GROUPS = new Set(["native-release-manifest"]);
const GROUPS_BY_PHASE = Object.freeze({
  "pre-freeze": Object.freeze(PRE_FREEZE_GROUPS),
  frozen: Object.freeze(FROZEN_GROUPS),
  published: Object.freeze(PUBLISHED_GROUPS),
  closeout: Object.freeze(CLOSEOUT_GROUPS),
});
const ALL_GROUPS = new Set([...CLOSEOUT_GROUPS, ...OPTIONAL_GROUPS]);
const POST_CALIBRATION_CASCADE = [
  "calibration-source",
  "pull-requests",
  "calibration-source-acceptance",
  "evidence",
  "calibration",
  "frozen-candidate",
  "frozen-candidate-acceptance",
  "source-proof",
  "package",
  "hardware",
  "installed-candidate",
  "qualification",
  "pre-publish-ledger",
  "promotion",
  "publication",
  "native-release-manifest",
  "post-publish-ledger",
  "catalog-delivery",
];
const POST_FREEZE_CASCADE = [
  "frozen-candidate",
  "frozen-candidate-acceptance",
  "source-proof",
  "package",
  "hardware",
  "installed-candidate",
  "qualification",
  "pre-publish-ledger",
  "promotion",
  "publication",
  "native-release-manifest",
  "post-publish-ledger",
  "catalog-delivery",
];

function fail(message) {
  throw new Error(message);
}

function present(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function object(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    fail(`${label} must be an object`);
  }
  return value;
}

function nonEmpty(value, label) {
  if (!present(value)) {
    fail(`${label} is required`);
  }
  return value;
}

function positiveInteger(value, label) {
  const number = Number(value);
  if (!Number.isSafeInteger(number) || number <= 0) {
    fail(`${label} must be a positive integer`);
  }
  return number;
}

function sha(value, label) {
  if (!/^[0-9a-f]{40}$/u.test(String(value ?? ""))) {
    fail(`${label} must be a full lowercase commit or tree SHA`);
  }
  return value;
}

function digest(value, label) {
  if (!/^(?:sha256:)?[0-9a-f]{64}$/u.test(String(value ?? ""))) {
    fail(`${label} digest must be a SHA-256 digest`);
  }
  return value;
}

function artifact(value, label) {
  nonEmpty(value, `${label} artifact`);
  if (value.includes("\n") || value.includes("\r")) {
    fail(`${label} artifact must be one immutable artifact name`);
  }
  return value;
}

function validateRunRow(value, label) {
  const row = object(value, label);
  nonEmpty(row.lane, `${label} lane`);
  const runId = positiveInteger(row.run_id, `${label} run_id`);
  const attempt = positiveInteger(row.attempt, `${label} attempt`);
  artifact(row.artifact, label);
  digest(row.digest, label);
  nonEmpty(row.identity, `${label} identity`);
  if (!new Set(["success", "failure"]).has(row.conclusion)) {
    fail(`${label} conclusion must be success or failure`);
  }
  sha(row.commit, `${label} commit`);
  sha(row.tree, `${label} tree`);
  return {
    ...row,
    run_id: runId,
    attempt,
  };
}

function validateArtifactRow(value, label) {
  const row = object(value, label);
  artifact(row.artifact, label);
  digest(row.digest, label);
  return row;
}

function validateGroupValue(group, value) {
  if (RUN_GROUPS.has(group)) {
    const rows = Array.isArray(value) ? value : [value];
    if (rows.length === 0) {
      fail(`${group} must contain at least one evidence row`);
    }
    const normalized = rows.map((row, index) =>
      validateRunRow(row, `${group}[${index}]`)
    );
    if (new Set(normalized.map(row => row.lane)).size !== normalized.length) {
      fail(`${group} lanes must be unique`);
    }
    if (group === "calibration" && normalized.length !== 3) {
      fail("calibration must contain exactly three run evidence rows");
    }
    return normalized;
  }

  const row = object(value, group);
  if (group === "calibration-source" || group === "frozen-candidate") {
    sha(row.commit, `${group} commit`);
    sha(row.tree, `${group} tree`);
  } else if (group === "pull-requests") {
    positiveInteger(row.release_pr, "pull-requests release_pr");
    if (
      !Array.isArray(row.integrated_support_prs)
      || !row.integrated_support_prs.every(number =>
        Number.isSafeInteger(number) && number > 0
      )
      || new Set(row.integrated_support_prs).size !== row.integrated_support_prs.length
    ) {
      fail("pull-requests integrated_support_prs must contain unique positive integers");
    }
  } else if (group === "evidence") {
    if (!Array.isArray(row.reusable) || !Array.isArray(row.invalidated)) {
      fail("evidence must contain reusable and invalidated arrays");
    }
    for (const [index, entry] of row.reusable.entries()) {
      object(entry, `evidence reusable[${index}]`);
      nonEmpty(entry.identity, `evidence reusable[${index}] identity`);
      sha(entry.commit, `evidence reusable[${index}] commit`);
      sha(entry.tree, `evidence reusable[${index}] tree`);
      artifact(entry.artifact, `evidence reusable[${index}]`);
      digest(entry.digest, `evidence reusable[${index}]`);
      nonEmpty(
        entry.machine_policy,
        `evidence reusable[${index}] machine_policy`,
      );
      nonEmpty(
        entry.evidence_window,
        `evidence reusable[${index}] evidence_window`,
      );
    }
    for (const [index, entryValue] of row.invalidated.entries()) {
      const entry = object(entryValue, `evidence invalidated[${index}]`);
      nonEmpty(entry.identity, `evidence invalidated[${index}] identity`);
      nonEmpty(entry.reason, `evidence invalidated[${index}] reason`);
      sha(entry.replacing_sha, `evidence invalidated[${index}] replacing_sha`);
    }
  } else if (group === "pre-publish-ledger" || group === "post-publish-ledger") {
    validateArtifactRow(row, group);
  } else if (group === "promotion") {
    positiveInteger(row.pull_request, "promotion pull_request");
    nonEmpty(row.approver, "promotion approver");
    if (!Number.isFinite(Date.parse(String(row.approved_at ?? "")))) {
      fail("promotion approved_at must be an ISO-compatible timestamp");
    }
  } else if (group === "publication") {
    sha(row.commit, "publication commit");
    sha(row.tree, "publication tree");
    nonEmpty(row.tag, "publication tag");
    nonEmpty(row.release_url, "publication release_url");
    row.release_run = validateRunRow(row.release_run, "publication release_run");
  } else if (group === "native-release-manifest") {
    validateArtifactRow(row, group);
    nonEmpty(row.identity, "native-release-manifest identity");
  } else if (group === "catalog-delivery") {
    nonEmpty(row.state, "catalog-delivery state");
    nonEmpty(row.installer_identity, "catalog-delivery installer_identity");
  } else if (group === "next-action") {
    nonEmpty(row.action, "next-action action");
    nonEmpty(row.owner, "next-action owner");
  } else {
    fail(`unknown field group ${group}`);
  }
  return row;
}

function groupRunRows(group, value) {
  if (RUN_GROUPS.has(group)) {
    return value;
  }
  return group === "publication" ? [value.release_run] : [];
}

function validateReceiptShape(receipt) {
  object(receipt, "receipt");
  if (receipt.schema !== RECEIPT_SCHEMA) {
    fail(`receipt schema must be ${RECEIPT_SCHEMA}`);
  }
  nonEmpty(receipt.version, "receipt version");
  if (!Number.isSafeInteger(receipt.sequence) || receipt.sequence < 0) {
    fail("receipt sequence is invalid");
  }
  object(receipt.groups, "receipt groups");
  if (!Array.isArray(receipt.attempt_history)) {
    fail("receipt attempt_history must be an array");
  }
  if (!Array.isArray(receipt.invalidations)) {
    fail("receipt invalidations must be an array");
  }
  for (const group of Object.keys(receipt.groups)) {
    if (!ALL_GROUPS.has(group)) {
      fail(`receipt contains unknown field group ${group}`);
    }
  }
  return receipt;
}

export function initReceipt(version) {
  nonEmpty(version, "--version");
  return {
    schema: RECEIPT_SCHEMA,
    version,
    sequence: 0,
    groups: {},
    attempt_history: [],
    invalidations: [],
  };
}

function compareAttempts(left, right) {
  const leftRun = BigInt(left.run_id);
  const rightRun = BigInt(right.run_id);
  if (leftRun !== rightRun) {
    return leftRun < rightRun ? -1 : 1;
  }
  return left.attempt - right.attempt;
}

function rejectAttemptFallback(receipt, group, rows) {
  for (const row of rows) {
    const sameAttempt = receipt.attempt_history.find(candidate =>
      candidate.group === group
      && candidate.lane === row.lane
      && candidate.run_id === row.run_id
      && candidate.attempt === row.attempt
      && candidate.conclusion !== row.conclusion
    );
    if (sameAttempt) {
      fail(
        `${group} lane ${row.lane} run ${row.run_id} attempt ${row.attempt} `
        + `is already recorded as ${sameAttempt.conclusion}`,
      );
    }
    if (row.conclusion !== "success") {
      continue;
    }
    const newerFailure = receipt.attempt_history.find(candidate =>
      candidate.group === group
      && candidate.lane === row.lane
      && candidate.conclusion === "failure"
      && compareAttempts(candidate, row) > 0
    );
    if (newerFailure) {
      fail(
        `${group} lane ${row.lane} refuses successful run ${row.run_id} `
        + `attempt ${row.attempt}: newer failed run ${newerFailure.run_id} `
        + `attempt ${newerFailure.attempt} exists`,
      );
    }
  }
}

export function recordGroup(receiptValue, group, value) {
  const receipt = structuredClone(validateReceiptShape(receiptValue));
  if (!ALL_GROUPS.has(group)) {
    fail(`unknown field group ${group}`);
  }
  const normalized = validateGroupValue(group, structuredClone(value));
  const rows = groupRunRows(group, normalized);
  rejectAttemptFallback(receipt, group, rows);

  const sequence = receipt.sequence + 1;
  receipt.sequence = sequence;
  if (rows.length > 0) {
    for (const row of rows) {
      receipt.attempt_history.push({
        group,
        lane: row.lane,
        run_id: row.run_id,
        attempt: row.attempt,
        conclusion: row.conclusion,
        sequence,
      });
    }
  }
  const successful = rows.length === 0
    || rows.every(row => row.conclusion === "success");
  receipt.groups[group] = {
    sequence,
    status: successful ? "active" : "blocked",
    value: normalized,
  };
  return receipt;
}

function changedPathsRequireCalibration(changedPaths) {
  return changedPaths.length === 0
    || changedPaths.some(changedPath => changedPath !== CONSTANT_SET_PATH);
}

export function invalidateReceipt(receiptValue, {
  event,
  groups = [],
  reason,
  replacingSha,
  changedPaths = [],
}) {
  const receipt = structuredClone(validateReceiptShape(receiptValue));
  nonEmpty(reason, "--reason");
  sha(replacingSha, "--replacing-sha");
  if (!Array.isArray(changedPaths) || !changedPaths.every(present)) {
    fail("--changed-path values must be non-empty paths");
  }

  let affected;
  let recalibrationRequired = false;
  let resumePhase;
  if (event === "post-calibration-commit") {
    if (
      changedPaths.length > 0
      && !changedPathsRequireCalibration(changedPaths)
    ) {
      fail("the generated constant-set child is planned and must not invalidate calibration-source acceptance");
    }
    affected = POST_CALIBRATION_CASCADE;
    recalibrationRequired = true;
    resumePhase = "calibration-source";
  } else if (event === "post-freeze-commit") {
    recalibrationRequired = changedPathsRequireCalibration(changedPaths);
    affected = recalibrationRequired
      ? POST_CALIBRATION_CASCADE
      : POST_FREEZE_CASCADE;
    resumePhase = recalibrationRequired ? "calibration-source" : "frozen-candidate";
  } else if (event === "evidence") {
    if (groups.length === 0) {
      fail("evidence invalidation requires at least one --group");
    }
    for (const group of groups) {
      if (!ALL_GROUPS.has(group)) {
        fail(`unknown field group ${group}`);
      }
    }
    affected = [...new Set(groups)];
    resumePhase = "evidence-replacement";
  } else {
    fail("--event must be post-calibration-commit, post-freeze-commit, or evidence");
  }

  const sequence = receipt.sequence + 1;
  receipt.sequence = sequence;
  const invalidation = {
    id: receipt.invalidations.length + 1,
    sequence,
    event,
    reason,
    replacing_sha: replacingSha,
    changed_paths: [...changedPaths],
    affected_groups: [...affected],
    recalibration_required: recalibrationRequired,
    resume_phase: resumePhase,
  };
  receipt.invalidations.push(invalidation);
  for (const group of affected) {
    if (receipt.groups[group]) {
      receipt.groups[group] = {
        ...receipt.groups[group],
        status: "invalidated",
        invalidation_id: invalidation.id,
      };
    }
  }
  return receipt;
}

function latestAttempt(receipt, group, lane) {
  return receipt.attempt_history
    .filter(entry => entry.group === group && entry.lane === lane)
    .reduce((latest, entry) =>
      !latest || compareAttempts(entry, latest) > 0 ? entry : latest
    , undefined);
}

function requireRunIdentity(receipt, group, commit, tree) {
  for (const row of groupRunRows(group, receipt.groups[group].value)) {
    if (row.commit !== commit || row.tree !== tree) {
      fail(`${group} must bind exact commit ${commit} and tree ${tree}`);
    }
  }
}

export function validatePhase(receiptValue, phase) {
  const receipt = validateReceiptShape(receiptValue);
  if (!PHASES.includes(phase)) {
    fail("--phase must be pre-freeze, frozen, published, or closeout");
  }
  const requiredGroups = new Set(GROUPS_BY_PHASE[phase]);
  const allowedGroups = new Set(requiredGroups);
  if (phase === "published" || phase === "closeout") {
    allowedGroups.add("native-release-manifest");
  }

  for (const group of requiredGroups) {
    const entry = receipt.groups[group];
    if (!entry) {
      fail(`${phase} requires field group ${group}`);
    }
    if (entry.status !== "active") {
      fail(`${phase} field group ${group} is ${entry.status} without replacement`);
    }
    const normalized = validateGroupValue(group, entry.value);
    const runRows = groupRunRows(group, normalized);
    if (runRows.length > 0) {
      for (const row of runRows) {
        const latest = latestAttempt(receipt, group, row.lane);
        if (
          !latest
          || latest.conclusion !== "success"
          || latest.run_id !== row.run_id
          || latest.attempt !== row.attempt
        ) {
          fail(
            `${phase} field group ${group} lane ${row.lane} does not carry `
            + "the latest successful attempt",
          );
        }
      }
    }
  }
  for (const [group, entry] of Object.entries(receipt.groups)) {
    if (!allowedGroups.has(group)) {
      fail(`${phase} does not allow later field group ${group}`);
    }
    if (entry.status === "invalidated") {
      fail(`${phase} field group ${group} is invalidated without replacement`);
    }
  }
  const calibrationSource = receipt.groups["calibration-source"].value;
  requireRunIdentity(
    receipt,
    "calibration-source-acceptance",
    calibrationSource.commit,
    calibrationSource.tree,
  );
  if (phase !== "pre-freeze") {
    requireRunIdentity(
      receipt,
      "calibration",
      calibrationSource.commit,
      calibrationSource.tree,
    );
    const frozenCandidate = receipt.groups["frozen-candidate"].value;
    for (const group of [
      "frozen-candidate-acceptance",
      "source-proof",
      "package",
      "hardware",
      "installed-candidate",
      "qualification",
    ]) {
      requireRunIdentity(
        receipt,
        group,
        frozenCandidate.commit,
        frozenCandidate.tree,
      );
    }
    if (phase === "published" || phase === "closeout") {
      const publication = receipt.groups.publication.value;
      if (publication.tree !== frozenCandidate.tree) {
        fail("publication tree must match the frozen-candidate tree");
      }
      requireRunIdentity(
        receipt,
        "publication",
        publication.commit,
        publication.tree,
      );
    }
  }
  return true;
}

export function summarizeReceipt(receiptValue) {
  const receipt = validateReceiptShape(receiptValue);
  const lines = [
    `Release ${receipt.version}`,
    `Schema: ${receipt.schema}`,
    "Field groups:",
  ];
  const names = Object.keys(receipt.groups).sort();
  if (names.length === 0) {
    lines.push("  (none)");
  } else {
    for (const name of names) {
      const entry = receipt.groups[name];
      const suffix = entry.invalidation_id
        ? ` (invalidation ${entry.invalidation_id})`
        : "";
      lines.push(`  ${name}: ${entry.status}${suffix}`);
    }
  }
  lines.push("Phase readiness:");
  for (const phase of PHASES) {
    try {
      validatePhase(receipt, phase);
      lines.push(`  ${phase}: ready`);
    } catch (error) {
      lines.push(`  ${phase}: blocked - ${error.message}`);
    }
  }
  if (receipt.invalidations.length > 0) {
    lines.push("Invalidations:");
    for (const entry of receipt.invalidations) {
      lines.push(
        `  ${entry.id}: ${entry.reason}; replacement ${entry.replacing_sha}; `
        + `resume ${entry.resume_phase}`,
      );
    }
  }
  return `${lines.join("\n")}\n`;
}

function values(args, name) {
  const result = [];
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === name) {
      const found = args[index + 1];
      if (!found || found.startsWith("--")) {
        fail(`${name} requires a value`);
      }
      result.push(found);
      index += 1;
    }
  }
  return result;
}

function value(args, name, fallback = undefined) {
  const found = values(args, name);
  if (found.length > 1) {
    fail(`${name} may be specified only once`);
  }
  return found[0] ?? fallback;
}

function required(args, name) {
  return nonEmpty(value(args, name), name);
}

function readReceipt(receiptPath) {
  try {
    return validateReceiptShape(JSON.parse(readFileSync(receiptPath, "utf8")));
  } catch (error) {
    fail(`receipt ${receiptPath} is not valid: ${error.message}`);
  }
}

function writeReceipt(receiptPath, receipt) {
  const temporary = path.join(
    path.dirname(receiptPath),
    `.${path.basename(receiptPath)}.${process.pid}.tmp`,
  );
  writeFileSync(temporary, `${JSON.stringify(receipt, null, 2)}\n`, { flag: "wx" });
  renameSync(temporary, receiptPath);
}

function parseData(args) {
  const json = value(args, "--data-json");
  const file = value(args, "--data-file");
  if (Boolean(json) === Boolean(file)) {
    fail("exactly one of --data-json or --data-file is required");
  }
  try {
    return JSON.parse(file ? readFileSync(file, "utf8") : json);
  } catch (error) {
    fail(`field group data is not valid JSON: ${error.message}`);
  }
}

function main() {
  const [command, ...args] = process.argv.slice(2);
  if (command === "init") {
    const receiptPath = required(args, "--receipt");
    writeReceipt(receiptPath, initReceipt(required(args, "--version")));
    process.stdout.write(`${receiptPath}\n`);
  } else if (command === "record") {
    const [group, ...options] = args;
    nonEmpty(group, "record field-group");
    const receiptPath = required(options, "--receipt");
    const receipt = recordGroup(readReceipt(receiptPath), group, parseData(options));
    writeReceipt(receiptPath, receipt);
    process.stdout.write(`${group}\n`);
  } else if (command === "invalidate") {
    const receiptPath = required(args, "--receipt");
    const receipt = invalidateReceipt(readReceipt(receiptPath), {
      event: required(args, "--event"),
      groups: values(args, "--group"),
      reason: required(args, "--reason"),
      replacingSha: required(args, "--replacing-sha"),
      changedPaths: values(args, "--changed-path"),
    });
    writeReceipt(receiptPath, receipt);
    process.stdout.write(`${receipt.invalidations.at(-1).id}\n`);
  } else if (command === "validate") {
    const receipt = readReceipt(required(args, "--receipt"));
    const phase = required(args, "--phase");
    validatePhase(receipt, phase);
    process.stdout.write(`${phase}: valid\n`);
  } else if (command === "show") {
    process.stdout.write(summarizeReceipt(readReceipt(required(args, "--receipt"))));
  } else {
    fail(
      "usage: release-driver-receipt.mjs "
      + "<init|record|invalidate|validate|show> ...",
    );
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`release driver receipt rejected: ${error.message}\n`);
    process.exitCode = 1;
  }
}
