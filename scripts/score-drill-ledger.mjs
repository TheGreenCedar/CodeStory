#!/usr/bin/env node

import { readFileSync, writeFileSync } from "node:fs";

const [, , reportPath, ledgerPath, outputPath] = process.argv;
if (!reportPath || !ledgerPath) {
  console.error(
    "usage: node scripts/score-drill-ledger.mjs <drill-or-suite-report.json> <ledger.json> [scored-report.json]",
  );
  process.exit(2);
}

const report = JSON.parse(readFileSync(reportPath, "utf8"));
const ledger = JSON.parse(readFileSync(ledgerPath, "utf8"));
const allowed = new Set(["correct", "partial", "misleading", "unsupported"]);
const textKey = (value) => String(value ?? "").trim().split(/\s+/).join(" ").toLowerCase();
const pathKey = (value) =>
  String(value ?? "")
    .trim()
    .replaceAll("\\", "/")
    .replace(/^(\.\/)+/, "")
    .replace(/^\/+|\/+$/g, "")
    .toLowerCase();
const nonEmpty = (key, values) => (values.length > 0 ? { [key]: values } : {});
const isObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);

function outputSlug(value) {
  let slug = "";
  for (const character of value) {
    if (/^[A-Za-z0-9_-]$/.test(character)) slug += character;
    else if (!slug.endsWith("-")) slug += "-";
  }
  return slug.replace(/^-+|-+$/g, "") || "anchor";
}

function requireString(value, label) {
  if (typeof value !== "string") throw new Error(`${label} must be a string`);
}

function optionalBoolean(value, label) {
  if (value !== undefined && value !== null && typeof value !== "boolean") {
    throw new Error(`${label} must be a boolean or null`);
  }
}

function stringArray(value, label) {
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string")) {
    throw new Error(`${label} must be an array of strings`);
  }
}

function validateClaims(claims, label, validateSuiteDto = false) {
  for (const [index, claim] of claims.entries()) {
    if (!isObject(claim)) throw new Error(`${label}.claims[${index}] must be an object`);
    if (!allowed.has(claim.classification)) {
      throw new Error(
        `${label}.claims[${index}].classification must be correct, partial, misleading, or unsupported`,
      );
    }
    if (validateSuiteDto) {
      requireString(claim.id, `${label}.claims[${index}].id`);
      requireString(claim.text, `${label}.claims[${index}].text`);
      optionalBoolean(
        claim.changed_after_source_read,
        `${label}.claims[${index}].changed_after_source_read`,
      );
      stringArray(claim.source_files ?? [], `${label}.claims[${index}].source_files`);
      if (claim.notes !== undefined && claim.notes !== null) {
        requireString(claim.notes, `${label}.claims[${index}].notes`);
      }
    }
  }
}

function validateSuiteLedger(sourceLedger) {
  if (!isObject(sourceLedger)) throw new Error("ledger must be an object");
  if (
    sourceLedger.schema_version !== undefined &&
    sourceLedger.schema_version !== null &&
    (!Number.isInteger(sourceLedger.schema_version) ||
      sourceLedger.schema_version < 0 ||
      sourceLedger.schema_version > 0xffffffff)
  ) {
    throw new Error("ledger.schema_version must be an unsigned 32-bit integer or null");
  }
  if (sourceLedger.suite !== undefined && sourceLedger.suite !== null) {
    requireString(sourceLedger.suite, "ledger.suite");
  }
  const cases = sourceLedger.cases ?? [];
  if (!Array.isArray(cases)) throw new Error("ledger.cases must be an array");
  for (const [caseIndex, ledgerCase] of cases.entries()) {
    const label = `ledger.cases[${caseIndex}]`;
    if (!isObject(ledgerCase)) throw new Error(`${label} must be an object`);
    requireString(ledgerCase.slug, `${label}.slug`);
    optionalBoolean(ledgerCase.draft_written, `${label}.draft_written`);
    const claims = ledgerCase.claims ?? [];
    if (!Array.isArray(claims)) throw new Error(`${label}.claims must be an array`);
    validateClaims(claims, label, true);
    const findings = ledgerCase.layer_findings ?? [];
    if (!Array.isArray(findings)) throw new Error(`${label}.layer_findings must be an array`);
    for (const [findingIndex, finding] of findings.entries()) {
      const findingLabel = `${label}.layer_findings[${findingIndex}]`;
      if (!isObject(finding)) throw new Error(`${findingLabel} must be an object`);
      requireString(finding.layer, `${findingLabel}.layer`);
      requireString(finding.status, `${findingLabel}.status`);
      requireString(finding.detail, `${findingLabel}.detail`);
    }
  }
  return cases;
}

function scoreSingleDrill(drill, sourceLedger) {
  const claims = Array.isArray(sourceLedger.claims) ? sourceLedger.claims : [];
  validateClaims(claims, "ledger");
  const count = (classification) =>
    claims.filter((claim) => claim.classification === classification).length;
  const correct = count("correct");
  const partial = count("partial");
  const citedFiles = new Set(
    (drill.evidence_packet?.answer?.citations ?? [])
      .map((citation) => citation.file_path)
      .filter(Boolean),
  );
  const ledgerFiles = new Set(claims.flatMap((claim) => claim.source_files ?? []));
  return {
    packet_id: drill.evidence_packet?.packet_id ?? null,
    packet_sufficiency: drill.evidence_packet?.sufficiency?.status ?? null,
    claim_count: claims.length,
    correct,
    partial,
    misleading: count("misleading"),
    unsupported: count("unsupported"),
    material_revision_count: claims.filter(
      (claim) => claim.changed_after_source_read === true,
    ).length,
    quality_score: claims.length === 0 ? 0 : (correct + 0.5 * partial) / claims.length,
    cited_file_count: citedFiles.size,
    ledger_file_count: ledgerFiles.size,
    uncited_ledger_files: [...ledgerFiles].filter((path) => !citedFiles.has(path)).sort(),
  };
}

function expectedFileStats(repo) {
  const expected = repo.expectations?.source_truth_files ?? [];
  const targets = new Set((repo.summary?.source_truth?.target_files ?? []).map(pathKey));
  const missing = expected.filter((path) => !targets.has(pathKey(path)));
  return {
    expected_file_count: expected.length,
    expected_file_found_count: expected.length - missing.length,
    expected_file_missing_count: missing.length,
    ...(expected.length > 0
      ? { expected_file_recall: (expected.length - missing.length) / expected.length }
      : {}),
    ...nonEmpty("missing_expected_files", missing),
  };
}

function scoreSuiteRepo(repo, ledgerCase, ledgerSupplied) {
  const expected = expectedFileStats(repo);
  const blocked = repo.summary?.verdict?.status === "blocked";
  const ledgerStatus = ledgerCase ? "present" : ledgerSupplied ? "case_missing" : "not_supplied";
  const claims = ledgerCase?.claims ?? [];
  validateClaims(claims, `ledger case ${repo.slug}`);
  const warnings = [];
  if (blocked) {
    return {
      ledger_status: ledgerStatus,
      final_answer_status: "blocked",
      ...(ledgerCase?.draft_written == null ? {} : { draft_written: ledgerCase.draft_written }),
      claim_count: claims.length,
      claim_correct_count: 0,
      claim_partial_count: 0,
      claim_misleading_count: 0,
      claim_unsupported_count: 0,
      claim_unclassified_count: 0,
      material_revision_count: 0,
      ...expected,
      forbidden_claim_count: 0,
      ...nonEmpty("layer_findings", ledgerCase?.layer_findings ?? []),
      warnings: ["drill blocked before answer-quality scoring could complete"],
    };
  }
  if (expected.expected_file_missing_count > 0) {
    warnings.push(
      `${expected.expected_file_missing_count} expected source-truth file(s) were not emitted as drill targets`,
    );
  }
  if (!ledgerCase) {
    warnings.push(
      ledgerSupplied
        ? "ledger was supplied, but this repo slug had no matching case"
        : "no source-truth ledger supplied; final answer quality is still pending",
    );
  }
  for (const claim of claims) {
    if ((claim.source_files ?? []).length === 0) {
      warnings.push(`ledger claim \`${claim.id}\` has no source_files verification evidence`);
    }
  }
  if (claims.length === 0 && ledgerCase) warnings.push("ledger case has no verified claims");
  if (ledgerCase?.draft_written === false) {
    warnings.push("ledger reports that no CodeStory-only draft was written");
  }
  const count = (classification) =>
    claims.filter((claim) => claim.classification === classification).length;
  const partial = count("partial");
  const misleading = count("misleading");
  const unsupported = count("unsupported");
  const revisions = claims.filter((claim) => claim.changed_after_source_read === true).length;
  const falseClaims = repo.expectations?.false_claims ?? [];
  const forbiddenHits = claims
    .filter((claim) => !["misleading", "unsupported"].includes(claim.classification))
    .filter((claim) =>
      falseClaims.some((value) => textKey(value) && textKey(claim.text).includes(textKey(value))),
    )
    .map((claim) => `${claim.id}: ${claim.text}`);
  const finalStatus = !ledgerCase || ledgerCase.draft_written === false || claims.length === 0
    ? "pending_source_verification"
    : unsupported > 0 || misleading > 0 || forbiddenHits.length > 0
      ? "failed"
      : partial > 0 || revisions > 0 || expected.expected_file_missing_count > 0
        ? "degraded"
        : "ready";
  return {
    ledger_status: ledgerStatus,
    final_answer_status: finalStatus,
    ...(ledgerCase?.draft_written == null ? {} : { draft_written: ledgerCase.draft_written }),
    claim_count: claims.length,
    claim_correct_count: count("correct"),
    claim_partial_count: partial,
    claim_misleading_count: misleading,
    claim_unsupported_count: unsupported,
    claim_unclassified_count: 0,
    material_revision_count: revisions,
    ...expected,
    forbidden_claim_count: forbiddenHits.length,
    ...nonEmpty("forbidden_claim_hits", forbiddenHits),
    ...nonEmpty("layer_findings", ledgerCase?.layer_findings ?? []),
    ...nonEmpty("warnings", warnings),
  };
}

function scoredNextAction(repo) {
  const quality = repo.answer_quality;
  if (quality.final_answer_status === "ready") {
    if (repo.summary?.verdict?.status === "ready") {
      return "answer is source-verified; keep the artifacts as the ready baseline";
    }
    if ((repo.summary?.bridges?.partial ?? 0) > 0 || (repo.summary?.bridges?.graph_path ?? 0) === 0) {
      return `answer is source-verified; improve graph/bridge evidence before promoting the mechanical verdict (${repo.summary?.bridges?.partial ?? 0} partial bridge(s), ${repo.summary?.bridges?.graph_path ?? 0} graph bridge(s))`;
    }
    return "answer is source-verified; inspect the mechanical degraded reason before promotion";
  }
  if (quality.final_answer_status === "degraded") {
    if (quality.material_revision_count > 0 || quality.claim_partial_count > 0) {
      return `revise partial or materially changed claims, then rerun with the updated ledger (partial=${quality.claim_partial_count}, revisions=${quality.material_revision_count})`;
    }
    return "inspect answer-quality warnings and update the ledger or expected evidence";
  }
  if (quality.final_answer_status === "failed") {
    return `remove or correct misleading/unsupported final claims before trusting the answer (misleading=${quality.claim_misleading_count}, unsupported=${quality.claim_unsupported_count})`;
  }
  return repo.summary?.verdict?.next_action ?? "inspect the scored report";
}

function scoreSuite(suite, sourceLedger) {
  const cases = new Map();
  for (const ledgerCase of validateSuiteLedger(sourceLedger)) {
    const slug = outputSlug(ledgerCase.slug);
    if (cases.has(slug)) throw new Error(`drill-suite ledger case slug \`${slug}\` is duplicated`);
    cases.set(slug, ledgerCase);
  }
  const scored = structuredClone(suite);
  for (const repo of scored.repos) {
    repo.answer_quality = scoreSuiteRepo(repo, cases.get(outputSlug(String(repo.slug))), true);
  }
  scored.next_actions = scored.repos.map((repo) => `${repo.slug}: ${scoredNextAction(repo)}`);
  const count = (status) =>
    scored.repos.filter((repo) => repo.answer_quality.final_answer_status === status).length;
  scored.answer_ready_count = count("ready");
  scored.answer_degraded_count = count("degraded");
  scored.answer_failed_count = count("failed");
  scored.answer_pending_count = scored.repos.filter((repo) =>
    ["pending_source_verification", "blocked"].includes(repo.answer_quality.final_answer_status),
  ).length;
  return scored;
}

const result = Array.isArray(report.repos) ? scoreSuite(report, ledger) : scoreSingleDrill(report, ledger);
const json = `${JSON.stringify(result, null, 2)}\n`;
if (outputPath) writeFileSync(outputPath, json);
process.stdout.write(json);
