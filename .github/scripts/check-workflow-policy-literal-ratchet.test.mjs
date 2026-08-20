// Source-text literal ratchet for the S1 policy-graph migration (#1913, refs #1670).
//
// With S1-V4 merged, every graph-convertible rule lives in release-claims.json and
// the checker keeps only the generic evaluator engine plus a reviewed set of bespoke
// sites (compound predicates, digest scans, sequence tables, helper seams). This test
// freezes that end state: it scans .github/scripts/check-workflow-policy.mjs for four
// classes of residual policy literal —
//   workflowNames      string content containing a *.yml / *.yaml file name token
//   stepNames          a string literal exactly equal to a real step name (harvested
//                      from the parsed workflows and the release-claim step rows)
//   digests            a 64-hex digest inside any string literal
//   permissionTuples   a literal (scope, access) permission pair in property-compare,
//                      object-entry, or pair-array form
// — attributes every occurrence to its enclosing top-level declaration (structural
// function-name boundaries, no marker comments), and requires the resulting
// per-zone counts to equal EXPECTED_LITERAL_SITES exactly.
//
// Fail-closed in both directions: a NEW literal anywhere (including a brand-new
// top-level zone) changes the scan and fails; REMOVING a recorded literal also
// fails, because the comparison is exact membership with exact counts, never >=.
// The generic evaluator engine (ENGINE_ZONES) must stay literal-free and may never
// be absorbed into the allowlist.
//
// Legitimate ways to change this baseline: migrate the rule into
// release-claims.json (count drops; edit the row here in the same reviewed change),
// or record a new bespoke disposition by editing EXPECTED_LITERAL_SITES in the same
// commit that introduces the literal, so the site is enumerated in review.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { loadReleaseClaimGraph } from "../../scripts/codestory-release-claims.mjs";
import { loadWorkflows } from "./check-workflow-policy.mjs";

const scriptsRoot = path.dirname(fileURLToPath(import.meta.url));
const repositoryRoot = path.resolve(scriptsRoot, "../..");
const checkerPath = path.join(scriptsRoot, "check-workflow-policy.mjs");
const checkerSource = readFileSync(checkerPath, "utf8");

// --- scanner -----------------------------------------------------------------

// Extracts every string literal (single-quoted, double-quoted, and template chunks;
// template chunks split by ${} interpolation are marked incomplete) and returns the
// source with comments blanked so line-based detectors never match commentary.
export function tokenize(source) {
  const literals = [];
  const codeChars = source.split("");
  const blank = (from, to) => {
    for (let k = from; k < to; k += 1) if (codeChars[k] !== "\n") codeChars[k] = " ";
  };
  const stack = [{ kind: "code" }];
  let i = 0;
  let lastSignificant = "";
  const regexPreceders = new Set([
    "(", ",", "=", ":", "[", "!", "&", "|", "?", "{", "}", ";", "+", "-", "*", "%",
    "<", ">", "^", "~", "",
  ]);
  while (i < source.length) {
    const frame = stack[stack.length - 1];
    const ch = source[i];
    if (frame.kind === "code") {
      if (ch === "/" && source[i + 1] === "/") {
        const end = source.indexOf("\n", i);
        const stop = end === -1 ? source.length : end;
        blank(i, stop);
        i = stop;
        continue;
      }
      if (ch === "/" && source[i + 1] === "*") {
        const end = source.indexOf("*/", i + 2);
        const stop = end === -1 ? source.length : end + 2;
        blank(i, stop);
        i = stop;
        continue;
      }
      if (ch === '"' || ch === "'") {
        const start = i;
        i += 1;
        let content = "";
        while (i < source.length && source[i] !== ch) {
          if (source[i] === "\\") {
            content += source[i] + (source[i + 1] ?? "");
            i += 2;
          } else {
            content += source[i];
            i += 1;
          }
        }
        i += 1;
        literals.push({ start, content, complete: true });
        lastSignificant = ch;
        continue;
      }
      if (ch === "`") {
        stack.push({ kind: "template", chunks: [""], start: i });
        i += 1;
        continue;
      }
      if (ch === "/") {
        // Regex-versus-division heuristic on the previous significant character.
        if (regexPreceders.has(lastSignificant)) {
          i += 1;
          let inClass = false;
          while (i < source.length) {
            const c = source[i];
            if (c === "\\") {
              i += 2;
              continue;
            }
            if (c === "[") inClass = true;
            else if (c === "]") inClass = false;
            else if (c === "/" && !inClass) break;
            i += 1;
          }
          i += 1;
          while (i < source.length && /[a-z]/iu.test(source[i])) i += 1;
          lastSignificant = "/";
          continue;
        }
        i += 1;
        lastSignificant = "/";
        continue;
      }
      if (ch === "}" && stack.length > 1) {
        const below = stack[stack.length - 2];
        if (below.kind === "template" && frame.braceDepth === 0) {
          stack.pop();
          below.chunks.push("");
          i += 1;
          continue;
        }
        if (frame.braceDepth !== undefined) frame.braceDepth -= 1;
      } else if (ch === "{" && frame.braceDepth !== undefined) {
        frame.braceDepth += 1;
      }
      if (!/\s/u.test(ch)) lastSignificant = ch;
      i += 1;
      continue;
    }
    // template-literal frame
    if (ch === "\\") {
      frame.chunks[frame.chunks.length - 1] += ch + (source[i + 1] ?? "");
      i += 2;
      continue;
    }
    if (ch === "`") {
      const interpolated = frame.chunks.length > 1;
      for (const chunk of frame.chunks) {
        if (chunk.length > 0) {
          literals.push({ start: frame.start, content: chunk, complete: !interpolated });
        }
      }
      stack.pop();
      lastSignificant = "`";
      i += 1;
      continue;
    }
    if (ch === "$" && source[i + 1] === "{") {
      stack.push({ kind: "code", braceDepth: 0 });
      i += 2;
      continue;
    }
    frame.chunks[frame.chunks.length - 1] += ch;
    i += 1;
  }
  return { literals, code: codeChars.join("") };
}

// Structural zone map: every column-zero top-level declaration starts a zone that
// runs to the next one, so literals attribute to functions and constant tables
// without marker comments in the checker.
export function topLevelZones(source) {
  const zones = [{ name: "(module prelude)", start: 0 }];
  const pattern = /^(?:export\s+)?(?:async\s+)?(?:function|class|const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)/gmu;
  for (const match of source.matchAll(pattern)) {
    zones.push({ name: match[1], start: match.index });
  }
  return zones;
}

function zoneAt(zones, offset) {
  let candidate = zones[0].name;
  for (const zone of zones) {
    if (zone.start <= offset) candidate = zone.name;
    else break;
  }
  return candidate;
}

const WORKFLOW_NAME = /[A-Za-z0-9][A-Za-z0-9_.-]*\.ya?ml\b/gu;
const DIGEST_64 = /[0-9a-f]{64}/giu;
const PERMISSION_SCOPES =
  "contents|actions|id-token|checks|statuses|pull-requests|issues|packages|pages|deployments|attestations|security-events";
// Three literal shapes: `.scope === "access"` (property or bracket compare),
// `scope: "access"` (object entry, quoted or bare key), `["scope", "access"]`
// (pair array). The generic reusable-permissions engine compares expanded
// ["*", ...] rows and graph-fed row.scope/row.access values, which contain no
// named-scope literal and therefore never match.
const PERMISSION_TUPLE = new RegExp(
  [
    `[.\\[]\\s*["']?(?:${PERMISSION_SCOPES})["']?\\s*\\]?\\s*===\\s*["'](?:read|write|none)["']`,
    `["']?\\b(?:${PERMISSION_SCOPES})["']?\\s*:\\s*["'](?:read|write|none)["']`,
    `\\[\\s*["'](?:${PERMISSION_SCOPES})["']\\s*,\\s*["'](?:read|write|none)["']\\s*\\]`,
  ].join("|"),
  "gu",
);

export function scanLiteralSites(source, stepNames) {
  const { literals, code } = tokenize(source);
  const zones = topLevelZones(source);
  const counts = new Map();
  const bump = (zone, klass, by = 1) => {
    if (by === 0) return;
    const row = counts.get(zone) ?? {};
    row[klass] = (row[klass] ?? 0) + by;
    counts.set(zone, row);
  };
  for (const literal of literals) {
    const zone = zoneAt(zones, literal.start);
    bump(zone, "workflowNames", [...literal.content.matchAll(WORKFLOW_NAME)].length);
    bump(zone, "digests", [...literal.content.matchAll(DIGEST_64)].length);
    if (literal.complete && stepNames.has(literal.content)) bump(zone, "stepNames", 1);
  }
  let lineStart = 0;
  for (const line of code.split("\n")) {
    const matches = [...line.matchAll(PERMISSION_TUPLE)].length;
    if (matches > 0) bump(zoneAt(zones, lineStart), "permissionTuples", matches);
    lineStart += line.length + 1;
  }
  const ordered = {};
  for (const zone of [...counts.keys()].sort()) {
    ordered[zone] = Object.fromEntries(
      Object.entries(counts.get(zone)).sort(([a], [b]) => a.localeCompare(b)),
    );
  }
  return ordered;
}

// Step-name ground truth: the names of real steps in the parsed workflow tree plus
// every step named by a release-claim step_fragments or pinned_programs row, so a
// literal pinning a brand-new step is caught in the same change that adds the step.
function harvestStepNames() {
  const names = new Set();
  for (const workflow of loadWorkflows().values()) {
    const jobs = workflow && typeof workflow === "object" ? workflow.jobs : null;
    if (!jobs || typeof jobs !== "object") continue;
    for (const job of Object.values(jobs)) {
      const steps = job && typeof job === "object" ? job.steps : null;
      if (!Array.isArray(steps)) continue;
      for (const step of steps) {
        if (step && typeof step === "object" && typeof step.name === "string") {
          names.add(step.name);
        }
      }
    }
  }
  const policy = loadReleaseClaimGraph(repositoryRoot).workflow_policy ?? {};
  for (const block of ["step_fragments", "pinned_programs"]) {
    for (const row of Object.values(policy[block] ?? {})) {
      if (row && typeof row === "object" && typeof row.step === "string") {
        names.add(row.step);
      }
    }
  }
  return names;
}

// --- recorded dispositions ---------------------------------------------------

// The generic evaluator engine: graph-driven evaluators, the predicate helpers the
// graph rows select, and the loader/dispatcher seam. These zones must stay free of
// all four literal classes and may never appear in the allowlist below.
const ENGINE_ZONES = [
  "add",
  "commandListRow",
  "conditionIsSatisfiable",
  "constantMirrorViolations",
  "evaluateCondition",
  "forbidStepRun",
  "loadWorkflows",
  "namedStep",
  "parseWorkflow",
  "pinnedProgramRow",
  "pinnedProgramViolations",
  "requireExactRawStepScript",
  "requireExactStepScript",
  "requireJob",
  "requireStepEnv",
  "requireStepRun",
  "requireStepUses",
  "reusableWorkflowPermissionViolations",
  "stepFragmentViolations",
  "stepIndex",
  "stepRun",
  "structuralPinViolations",
  "validateWorkflows",
];

// The recorded bespoke-disposition sites (S1a-S1v4 review record): every zone that
// legitimately still carries policy literals, with the exact per-class occurrence
// counts. Exact membership: a zone gaining, losing, or newly growing a literal —
// or a brand-new zone appearing — fails this test.
const EXPECTED_LITERAL_SITES = {
  annotationScopeViolations: { permissionTuples: 1 },
  catalogDeliveryOutcomeViolations: { stepNames: 9 },
  catalogDeliveryStateViolations: { stepNames: 3 },
  cpuTestSeamAllowedJobs: { workflowNames: 1 },
  crateDurabilityFile: { workflowNames: 1 },
  draftRunCommands: { stepNames: 9 },
  draftSourcePolicyViolations: { stepNames: 8 },
  draftStepSequence: { stepNames: 15 },
  draftWorkflowPaths: { workflowNames: 2 },
  draftWorkflowPolicyViolations: { permissionTuples: 1 },
  expectedQualificationDriverContract: { workflowNames: 1 },
  frozenCandidateQualityWorkflowRef: { workflowNames: 1 },
  lostRunnerRecoveryViolations: { permissionTuples: 3, stepNames: 8, workflowNames: 2 },
  packagedPrSigningViolations: { workflowNames: 4 },
  proofFloorPolicyViolations: { permissionTuples: 1, stepNames: 16 },
  protectedRunnerReservationViolations: { stepNames: 7, workflowNames: 1 },
  releaseFreezeBarrierWorkflowViolations: {
    permissionTuples: 7,
    stepNames: 36,
    workflowNames: 32,
  },
  releaseProofWorkflowFiles: { workflowNames: 2 },
  releaseWorkflowContractViolations: { stepNames: 2, workflowNames: 7 },
  requireCalibrationProducerAuthentication: { stepNames: 1, workflowNames: 1 },
  requireCalibrationProducerBoundary: { stepNames: 3 },
  requireExactResolverContract: { stepNames: 1 },
  retrievalFile: { workflowNames: 1 },
  retrievalProducerTriggerPaths: { workflowNames: 1 },
  structuralPinMessages: { workflowNames: 2 },
  validateIssueWorkflows: { workflowNames: 2 },
  validateMarketplaceSync: { stepNames: 5, workflowNames: 1 },
  validatePackageMatrixExpression: { workflowNames: 3 },
  validatePackagedCoordinator: { permissionTuples: 2, stepNames: 38, workflowNames: 11 },
  validatePackagedProof: { digests: 2, permissionTuples: 2, stepNames: 106, workflowNames: 2 },
  validatePluginAndDraftWorkflows: { stepNames: 32, workflowNames: 13 },
  validatePluginRelease: { permissionTuples: 1, stepNames: 17, workflowNames: 6 },
  validatePostPublish: { stepNames: 38, workflowNames: 1 },
  validateReleaseArtifactRerunSafety: { workflowNames: 13 },
  validateReleaseCellUploadOwnership: { workflowNames: 18 },
  validateReleaseCoordinator: { stepNames: 24, workflowNames: 11 },
  validateRemainingWorkflows: { stepNames: 135, workflowNames: 17 },
  windowsManifestProofPolicyViolations: { stepNames: 7 },
  windowsRunCommands: { stepNames: 4 },
  windowsStepSequence: { stepNames: 6 },
  withoutMarketplaceIdentity: { stepNames: 1 },
};

// --- tests -------------------------------------------------------------------

const SYNTHETIC_SOURCE = `
function engineHelper(row) {
  return row.file;
}
function bespokeValidator(job) {
  const file = "synthetic-workflow.yml";
  const step = namedStep(job, "Synthetic ratchet step");
  const image = "img@sha256:${"ab".repeat(32)}";
  return object(job.permissions).contents === "read" && file && step && image;
}
`;

test("scanner detects every literal class in the zone that carries it", () => {
  const scanned = scanLiteralSites(SYNTHETIC_SOURCE, new Set(["Synthetic ratchet step"]));
  assert.deepEqual(scanned, {
    bespokeValidator: {
      digests: 1,
      permissionTuples: 1,
      stepNames: 1,
      workflowNames: 1,
    },
  });
  assert.equal("engineHelper" in scanned, false);
});

test("a literal injected into the checker source fails the ratchet", () => {
  const stepNames = harvestStepNames();
  const baseline = scanLiteralSites(checkerSource, stepNames);
  const probed = scanLiteralSites(
    `${checkerSource}\nconst ratchetProbe = "ratchet-probe.yml";\n`,
    stepNames,
  );
  assert.notDeepEqual(probed, baseline);
  assert.deepEqual(probed.ratchetProbe, { workflowNames: 1 });
});

test("the generic evaluator engine stays literal-free and outside the allowlist", () => {
  const stepNames = harvestStepNames();
  const scanned = scanLiteralSites(checkerSource, stepNames);
  const zoneNames = new Set(topLevelZones(checkerSource).map(zone => zone.name));
  for (const zone of ENGINE_ZONES) {
    assert.equal(zoneNames.has(zone), true, `engine zone ${zone} must exist in the checker`);
    assert.equal(zone in scanned, false, `engine zone ${zone} must carry no policy literals`);
    assert.equal(
      zone in EXPECTED_LITERAL_SITES,
      false,
      `engine zone ${zone} may not be allowlisted`,
    );
  }
});

test("checker literal sites equal the recorded bespoke-disposition allowlist exactly", () => {
  const scanned = scanLiteralSites(checkerSource, harvestStepNames());
  const totals = {};
  for (const row of Object.values(scanned)) {
    for (const [klass, count] of Object.entries(row)) {
      totals[klass] = (totals[klass] ?? 0) + count;
    }
  }
  // Vacuity tripwire: every class must actually occur in the recorded baseline.
  for (const klass of ["digests", "permissionTuples", "stepNames", "workflowNames"]) {
    assert.ok(totals[klass] > 0, `scanner found no ${klass} in the checker at all`);
  }
  assert.deepEqual(scanned, EXPECTED_LITERAL_SITES);
});
