import assert from "node:assert/strict";
import { readFile, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { parseArgs } from "node:util";
import { fileURLToPath } from "node:url";
import { evaluateArm, maximizeCoveredAtoms, mean, percentile, requiredFragments, sha256 } from "./lib/etr1-evidence.mjs";
import { validateEtr1 } from "./codestory-etr1-validate.mjs";

async function readBound(file, digest) {
  const bytes = await readFile(file);
  assert.equal(sha256(bytes), digest, `artifact digest changed: ${file}`);
  return { bytes, value: JSON.parse(bytes) };
}

function comparableOptimum(actual, expected) {
  assert.equal(actual.covered, expected.covered, "oracle covered-atom optimum changed");
  assert.deepEqual(actual.selected, expected.selected, "oracle selected-row optimum changed");
  assert.equal(actual.rows, expected.rows, "oracle row optimum changed");
  assert.equal(actual.public_bytes, expected.public_bytes, "oracle byte optimum changed");
}

export async function reproduceOracleFixtures({ oracle, preparation, annotations }) {
  assert.equal(oracle.contract, "codestory.fragment-budget-oracle/v1");
  assert.equal(oracle.authority, "answer_aware_post_failure_diagnostic_only");
  assert.equal(oracle.production_use_forbidden, true);
  assert.equal(oracle.cases.length, 24, "retained oracle case count changed");
  const repositories = new Map(preparation.repositories.map((value) => [value.repository_id, value]));
  const fragments = new Map(preparation.fragments.map((value) => [value.fragment_id, value]));
  for (const binding of oracle.cases) {
    const { value: retained } = await readBound(binding.path, binding.sha256);
    assert.equal((await stat(binding.path)).size, binding.bytes, "retained oracle case length changed");
    assert.equal(retained.case_id, binding.case_id, "retained oracle case identity changed");
    const annotation = annotations.cases.find((value) => value.case_id === retained.case_id);
    assert.ok(annotation, "retained oracle annotation missing");
    const repository = repositories.get(retained.repository_id);
    assert.ok(repository, "retained oracle repository missing");
    const repositoryFragments = repository.fragment_ids.map((id) => fragments.get(id));
    const rowBytes = new Map(repositoryFragments.map((fragment, index) => [index, fragment.serialized_row_bytes]));
    assert.equal(retained.alternatives.length, annotation.acceptable_sets.length,
      "retained acceptable-set count changed");
    for (const alternative of retained.alternatives) {
      const set = annotation.acceptable_sets.find((value) => value.set_id === alternative.set_id);
      assert.ok(set, "retained acceptable set missing");
      const requirements = set.required_source_atoms.map(({ source_range }) =>
        requiredFragments(source_range, repositoryFragments));
      assert.deepEqual(requirements, alternative.requirements, "retained atom-to-fragment mapping changed");
      comparableOptimum(maximizeCoveredAtoms(requirements, rowBytes, repository.base_serialized_bytes),
        alternative.optimum);
    }
  }
  return { status: "reproduced", cases: oracle.cases.length };
}

function aggregateArm(cases, name) {
  const groups = [...new Set(cases.map((value) => value.group))].sort();
  return { mean_recall: mean(cases.map((value) => value[name].recall)),
    complete_set_rate: mean(cases.map((value) => value[name].complete_set_rate)),
    groups: Object.fromEntries(groups.map((group) => [group,
      { mean_recall: mean(cases.filter((value) => value.group === group).map((value) => value[name].recall)),
        complete_set_rate: mean(cases.filter((value) => value.group === group)
          .map((value) => value[name].complete_set_rate)) }])),
  };
}

export function gateOne(aggregate) {
  const gates = { mean_recall: aggregate.mean_recall >= 0.85,
    every_group_recall: Object.values(aggregate.groups).every((group) => group.mean_recall >= 0.70),
    complete_set_rate: aggregate.complete_set_rate >= 0.75,
    source_address_authentication: true };
  return { pass: Object.values(gates).every(Boolean), gates };
}

function buildCases(scoredRows) {
  const ids = [...new Set(scoredRows.map((value) => value.case_id))].sort();
  assert.equal(ids.length, 24, "evaluated case count changed");
  return ids.map((case_id) => {
    const rows = scoredRows.filter((value) => value.case_id === case_id)
      .toSorted((left, right) => left.phrasing_id.localeCompare(right.phrasing_id));
    assert.deepEqual(rows.map((value) => value.phrasing_id).sort(),
      ["original", "paraphrase_1", "paraphrase_2"], "case phrasing set changed");
    const arm = (name) => ({ recall: mean(rows.map((value) => value[name].recall)),
      complete_set_rate: mean(rows.map((value) => Number(value[name].complete_source_set))),
      incomplete_phrasings: rows.filter((value) => !value[name].complete_source_set).length,
      prepared_state_ns: rows.map((value) => value[name].prepared_state_ns) });
    const gainedPhrasings = rows.filter((row) =>
      row.candidate.reachable_atoms.some((atom) => !row.control.reachable_atoms.includes(atom))).length;
    return { case_id, group: rows[0].group, control: arm("control"), candidate: arm("candidate"),
      candidate_gained_atom_phrasings: gainedPhrasings,
      control_incomplete_for_gain: rows.filter((value) => !value.control.complete_source_set).length >= 2,
      candidate_gained_atom: gainedPhrasings >= 2 };
  });
}

export function gateTwo(cases, control, candidate, candidateGateOne) {
  const recallDelta = candidate.mean_recall - control.mean_recall;
  const completeDelta = candidate.complete_set_rate - control.complete_set_rate;
  const groupLosses = Object.keys(control.groups).map((group) =>
    candidate.groups[group].mean_recall - control.groups[group].mean_recall);
  const eligible = cases.filter((value) => value.control_incomplete_for_gain);
  const gainRate = eligible.length
    ? eligible.filter((value) => value.candidate_gained_atom).length / eligible.length : 0;
  const gates = { candidate_sufficient: candidateGateOne,
    material_recall_or_complete_gain: recallDelta >= 0.10 || completeDelta >= 0.10,
    other_measure_not_regressed: recallDelta >= 0 && completeDelta >= 0,
    no_group_loss_over_two_points: groupLosses.every((delta) => delta >= -0.02),
    new_atom_in_half_of_eligible_cases: gainRate >= 0.5 };
  return { pass: Object.values(gates).every(Boolean), gates,
    recall_delta: recallDelta, complete_set_rate_delta: completeDelta,
    group_recall_deltas: Object.fromEntries(Object.keys(control.groups)
      .map((group, index) => [group, groupLosses[index]])),
    eligible_incomplete_cases: eligible.length,
    gained_atom_cases: eligible.filter((value) => value.candidate_gained_atom).length,
    gained_atom_case_rate: gainRate };
}

export function decision(controlGate, candidateGate, conditioningGate) {
  if (!controlGate && !candidateGate) return { frontier: null,
    decision: "no_frontier_selected", reason: "neither_arm_sufficient" };
  if (controlGate && !conditioningGate) return { frontier: "control",
    decision: "unconditioned_frontier_selected",
    reason: candidateGate ? "conditioning_lacks_material_value" : "control_alone_sufficient" };
  if (candidateGate && conditioningGate) return { frontier: "candidate",
    decision: "conditioned_frontier_selected",
    reason: controlGate ? "conditioning_materially_better" : "conditioning_only_adequate_frontier" };
  return { frontier: null, decision: "no_frontier_selected",
    reason: "only_conditioned_arm_sufficient_without_material_conditioning_value" };
}

export async function evaluateEtr1({ validationPath, validationSha256, annotationsPath,
  annotationsSha256, oraclePath, oracleSha256, sourceRoot, allowCanary = false }) {
  const { value: validation } = await readBound(validationPath, validationSha256);
  assert.equal(validation.contract, "codestory.etr1-validation/v1");
  assert.equal(validation.experiment_status, "valid", "validator did not authorize annotation access");
  assert.equal(validation.decision, "not_evaluated");
  assert.equal(validation.annotation_access, "not_accessed");
  const validated = await validateEtr1({ runBinding: validation.run,
    runPath: validation.run.path, sourceRoot, executionBinding: validation.execution, allowCanary });
  assert.equal(validated.run.build.binary_sha256, validation.binary_sha256,
    "validation receipt no longer binds the run binary");
  // This is the first annotation read in the authoritative path.
  const { value: annotations } = await readBound(annotationsPath, annotationsSha256);
  const canary = validated.run.authority === "synthetic_canary_only";
  if (canary) {
    assert.equal(annotations.authority, "synthetic_canary_only");
    const rows = scoreRows(validated, annotations);
    assert.deepEqual(rows.map((row) => [row.control.recall, row.candidate.recall]), [[1, 1], [1, 1], [0, 0]],
      "synthetic oracle output changed");
    return { contract: "codestory.etr1-evaluation/v1", authority: "synthetic_canary_only",
      experiment_status: "valid", decision: "not_evaluated", packet_decision: "not_evaluated", rows };
  }
  assert.equal(annotations.authority, "visible_development_only");
  assert.equal(annotations.questions_sha256,
    validated.preparation.fixed_inputs.questions.sha256, "annotation question binding changed");
  const { value: oracle } = await readBound(oraclePath, oracleSha256);
  const oracle_reproduction = await reproduceOracleFixtures({ oracle,
    preparation: validated.preparation, annotations });
  const scoredRows = scoreRows(validated, annotations);
  const cases = buildCases(scoredRows), control = aggregateArm(cases, "control"),
    candidate = aggregateArm(cases, "candidate"), controlGate = gateOne(control),
    candidateGate = gateOne(candidate), conditioning = gateTwo(cases, control, candidate, candidateGate.pass);
  let selected = decision(controlGate.pass, candidateGate.pass, conditioning.pass);
  let latency = { status: "not_evaluated", p95_ns: null, threshold_ns: 1_250_000_000, pass: null };
  if (selected.frontier) {
    const timings = scoredRows.map((value) => value[selected.frontier].prepared_state_ns);
    const p95 = percentile(timings, 0.95);
    latency = { status: "evaluated", p95_ns: p95, threshold_ns: 1_250_000_000,
      pass: p95 <= 1_250_000_000 };
    if (!latency.pass) selected = { frontier: selected.frontier,
      decision: "authorize_one_byte_identical_latency_repair", reason: "chosen_frontier_latency_failed" };
  }
  return { contract: "codestory.etr1-evaluation/v1", authority: "visible_development_frontier_only",
    experiment_status: "valid", packet_decision: "not_evaluated", source_address_validity: 1,
    inputs: { validation: { path: validationPath, sha256: validationSha256 },
      annotations: { path: annotationsPath, sha256: annotationsSha256 },
      fragment_oracle: { path: oraclePath, sha256: oracleSha256 } },
    oracle_reproduction, aggregates: { control, candidate },
    gates: { control_frontier_sufficiency: controlGate,
      candidate_frontier_sufficiency: candidateGate, conditioning_value: conditioning,
      frontier_construction_latency: latency }, selected, cases, rows: scoredRows };
}

function scoreRows(validated, annotations) {
  const annotationByCase = new Map(annotations.cases.map((value) => [value.case_id, value]));
  const repositoryById = new Map(validated.preparation.repositories.map((value) => [value.repository_id, value]));
  const fragmentById = new Map(validated.preparation.fragments.map((value) => [value.fragment_id, value]));
  return validated.rows.map((row) => {
    const annotation = annotationByCase.get(row.case_id), repository = repositoryById.get(row.repository_id);
    assert.ok(annotation && repository, "evaluation binding missing");
    const fragments = repository.fragment_ids.map((id) => fragmentById.get(id));
    const score = (name) => ({ ...evaluateArm(annotation, fragments,
      row[name].legally_selectable_pool, repository.base_serialized_bytes),
      prepared_state_ns: row[name].timing.prepared_state_ns });
    return { case_id: row.case_id, phrasing_id: row.phrasing_id, group: row.group,
      control: score("control"), candidate: score("candidate") };
  });
}

async function main() {
  const { values } = parseArgs({ options: Object.fromEntries([
    "validation", "validation-sha256", "annotations", "annotations-sha256",
    "fragment-oracle", "fragment-oracle-sha256", "output",
  ].map((name) => [name, { type: "string" }])) });
  for (const name of ["validation", "validation-sha256", "annotations", "annotations-sha256",
    "fragment-oracle", "fragment-oracle-sha256", "output"])
    assert.ok(values[name], `missing --${name}`);
  const sourceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  let report;
  try {
    report = await evaluateEtr1({ validationPath: values.validation,
      validationSha256: values["validation-sha256"], annotationsPath: values.annotations,
      annotationsSha256: values["annotations-sha256"], oraclePath: values["fragment-oracle"],
      oracleSha256: values["fragment-oracle-sha256"], sourceRoot });
  } catch (error) {
    report = { contract: "codestory.etr1-evaluation/v1", experiment_status: "invalid",
      decision: "not_evaluated", packet_decision: "not_evaluated", error: error.message };
  }
  const bytes = `${JSON.stringify(report, null, 2)}\n`;
  await writeFile(values.output, bytes, { flag: "wx", mode: 0o600 });
  console.log(`${sha256(bytes)}  ${values.output}`);
  if (report.experiment_status !== "valid") process.exitCode = 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
