#!/usr/bin/env node

import { readFileSync } from "node:fs";

const [, , reportPath, ledgerPath] = process.argv;
if (!reportPath || !ledgerPath) {
  console.error("usage: node scripts/score-drill-ledger.mjs <drill-report.json> <ledger.json>");
  process.exit(2);
}

const report = JSON.parse(readFileSync(reportPath, "utf8"));
const ledger = JSON.parse(readFileSync(ledgerPath, "utf8"));
const claims = Array.isArray(ledger.claims) ? ledger.claims : [];
const allowed = new Set(["correct", "partial", "misleading", "unsupported"]);
for (const [index, claim] of claims.entries()) {
  if (!allowed.has(claim.classification)) {
    throw new Error(`claims[${index}].classification must be correct, partial, misleading, or unsupported`);
  }
}

const count = (classification) =>
  claims.filter((claim) => claim.classification === classification).length;
const correct = count("correct");
const partial = count("partial");
const misleading = count("misleading");
const unsupported = count("unsupported");
const materialRevisions = claims.filter((claim) => claim.changed_after_source_read === true).length;
const citedFiles = new Set(
  (report.evidence_packet?.answer?.citations ?? [])
    .map((citation) => citation.file_path)
    .filter(Boolean),
);
const ledgerFiles = new Set(claims.flatMap((claim) => claim.source_files ?? []));

process.stdout.write(
  `${JSON.stringify(
    {
      packet_id: report.evidence_packet?.packet_id ?? null,
      packet_sufficiency: report.evidence_packet?.sufficiency?.status ?? null,
      claim_count: claims.length,
      correct,
      partial,
      misleading,
      unsupported,
      material_revision_count: materialRevisions,
      quality_score: claims.length === 0 ? 0 : (correct + 0.5 * partial) / claims.length,
      cited_file_count: citedFiles.size,
      ledger_file_count: ledgerFiles.size,
      uncited_ledger_files: [...ledgerFiles].filter((path) => !citedFiles.has(path)).sort(),
    },
    null,
    2,
  )}\n`,
);
