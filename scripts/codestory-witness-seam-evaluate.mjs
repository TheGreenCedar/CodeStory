import assert from "node:assert/strict";
import { readFile, realpath, writeFile } from "node:fs/promises";
import path from "node:path";
import { parseArgs } from "node:util";
import { fileURLToPath } from "node:url";
import { authenticateRange, phase1AGate, scoreWitnessArm, sha256, verifyPairedInputs } from "./lib/witness-seam-evidence.mjs";

async function boundJson(file, digest) {
  const bytes = await readFile(file);
  assert.equal(sha256(bytes), digest, `artifact digest mismatch: ${file}`);
  return JSON.parse(bytes);
}

/** Evaluate every frozen case or emit no aggregate. No candidate selection. */
export async function evaluateWitnessRun({ questions, annotations, runs }) {
  assert.equal(questions.authority, "visible_development_only");
  assert.equal(questions.cases.length, 24);
  assert.equal(questions.repositories.length, 12);
  assert.equal(runs.rows.length, questions.cases.length * 3);
  const identities = new Set();
  const results = [];
  let build;
  for (const row of runs.rows) {
    const key = `${row.case_id}/${row.phrasing_id}`;
    assert.ok(!identities.has(key), "duplicate case/phrasing");
    identities.add(key);
    const task = questions.cases.find((value) => value.case_id === row.case_id);
    assert.ok(task, "unexpected evidence task");
    const phrasing = ["original", "paraphrase_1", "paraphrase_2"].indexOf(row.phrasing_id);
    assert.ok(phrasing >= 0, "unexpected phrasing");
    const question = [task.question, ...task.paraphrases][phrasing];
    assert.equal(row.exit_code, 0, "required witness replay failed");
    const manifest = await boundJson(row.manifest_path, row.manifest_sha256);
    const receipt = await boundJson(row.receipt_path, row.receipt_sha256);
    assert.equal(manifest.case_id, row.case_id);
    assert.equal(manifest.phrasing_id, row.phrasing_id);
    assert.equal(manifest.capture.question_sha256, sha256(question));
    assert.equal(manifest.capture.query_ordinal, 0);
    assert.equal(manifest.capture.candidate_limit, 16);
    assert.equal(manifest.capture.candidate_count, manifest.descriptors.length);
    assert.equal(manifest.capture.semantic, false);
    assert.equal(manifest.capture.graph, false);
    assert.equal(receipt.manifest_sha256, row.manifest_sha256);
    assert.equal(receipt.descriptors_sha256, sha256(JSON.stringify(manifest.descriptors)));
    build ??= receipt.build;
    assert.deepEqual(receipt.build, build, "mixed replay binaries");
    assert.equal(manifest.capture.binary_sha256, build.binary_sha256, "capture and replay binaries differ");
    verifyPairedInputs(receipt, manifest);
    const repository = questions.repositories.find((value) => value.id === task.repository_id);
    assert.ok(repository, "missing repository binding");
    const root = await realpath(repository.local_root);
    assert.equal(await realpath(manifest.project_root), root);
    const annotation = annotations.cases.find((value) => value.case_id === row.case_id);
    assert.ok(annotation, "missing reconciled annotation");
    const annotationRanges = annotation.acceptable_sets.flatMap((set) => [
      ...set.required_source_atoms.map((atom) => atom.source_range), ...set.allowed_support_ranges,
      ...(set.required_relation_atoms ?? []).flatMap((atom) => [atom.occurrence, atom.from.source_range, atom.to.source_range]),
    ]);
    const paths = new Set([...manifest.descriptors.filter((value) => value.content_digest).map((value) => value.path), ...annotationRanges.map((range) => range.path)]);
    const sources = new Map();
    for (const relative of paths) {
      const absolute = await realpath(path.join(root, relative));
      assert.ok(absolute.startsWith(root + path.sep), "source escapes repository");
      sources.set(relative, await readFile(absolute));
    }
    for (const range of annotationRanges) authenticateRange(range, sources);
    for (const descriptor of manifest.descriptors) {
      if (descriptor.content_digest) {
        assert.equal(sha256(sources.get(descriptor.path)), descriptor.content_digest, "descriptor source changed");
      }
      if (descriptor.anchor?.kind === "match") {
        authenticateRange({ ...descriptor.anchor, path: descriptor.path, content_digest: descriptor.content_digest }, sources);
      } else if (descriptor.anchor?.kind === "indexed_node") {
        authenticateRange(descriptor.anchor.source_range, sources);
      }
    }
    for (const arm of [receipt.control, receipt.addressed]) {
      arm.input.sources.forEach((source) => {
        const descriptor = manifest.descriptors.find((value) => value.admission.stable_identity === source.stable_identity);
        assert.equal(source.path, descriptor.path, "hydration changed candidate path");
      });
      for (const output of arm.support.filter((value) => value.kind === "source_range")) {
        assert.ok(arm.input.sources.some((source) => source.path === output.path && source.start_line === output.start_line
          && source.end_line === output.end_line && source.source === output.snippet), "public source differs from hydrated input");
      }
    }
    results.push({ case_id: row.case_id, phrasing_id: row.phrasing_id,
      control: scoreWitnessArm(receipt.control, annotation, sources, { headerControl: true }),
      addressed: scoreWitnessArm(receipt.addressed, annotation, sources) });
  }
  return { ...phase1AGate(results, questions.cases.map((value) => value.case_id)),
    source_address_validity: 1, build, rows: results };
}

async function main() {
  const { values } = parseArgs({ options: Object.fromEntries([
    "questions", "questions-sha256", "annotations", "annotations-sha256", "runs", "runs-sha256", "output",
  ].map((name) => [name, { type: "string" }])) });
  for (const name of ["questions", "questions-sha256", "annotations", "annotations-sha256", "runs", "runs-sha256", "output"])
    assert.ok(values[name], `missing --${name}`);
  const inputs = { questions: values["questions-sha256"], annotations: values["annotations-sha256"], runs: values["runs-sha256"] };
  let report;
  try {
    const questions = await boundJson(values.questions, inputs.questions);
    const annotations = await boundJson(values.annotations, inputs.annotations);
    assert.equal(annotations.questions_sha256, inputs.questions);
    const runs = await boundJson(values.runs, inputs.runs);
    assert.equal(runs.questions_sha256, inputs.questions);
    assert.equal(runs.annotations_sha256, inputs.annotations);
    report = { experiment_status: "valid", ...await evaluateWitnessRun({ questions, annotations, runs }), inputs };
  } catch (error) {
    report = { contract: "codestory.witness-seam-evaluation/v1", experiment_status: "invalid",
      phase1a: "blocked", packet_decision: "not_evaluated", inputs, error: error.message };
  }
  const bytes = JSON.stringify(report, null, 2) + "\n";
  await writeFile(values.output, bytes, { flag: "wx" });
  console.log(`${sha256(bytes)}  ${values.output}`);
  if (report.experiment_status !== "valid" || report.phase1a !== "pass") process.exitCode = 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
