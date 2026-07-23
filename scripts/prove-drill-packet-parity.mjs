#!/usr/bin/env node

import assert from "node:assert/strict";
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { parseArgs } from "node:util";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(scriptPath), "..");

function citationKeys(packet) {
  const citations = [
    ...(packet.answer?.citations ?? []),
    ...(packet.sufficiency?.covered_claims ?? []).flatMap((claim) => claim.citations ?? []),
  ];
  return [...new Set(citations.map((citation) => JSON.stringify([
    citation.node_id,
    citation.file_path ?? null,
    citation.line ?? null,
    citation.display_name,
  ])))].sort();
}

function requestedProbes(packet, anchors, label) {
  const queries = (packet.plan?.queries ?? []).map(({ query }) => query.trim().toLowerCase());
  for (const anchor of anchors) {
    assert.ok(queries.includes(anchor.trim().toLowerCase()), `${label} omitted requested probe ${anchor}`);
  }
  assert.ok(
    (packet.plan?.trace ?? []).includes(`explicit_extra_probes=${anchors.length} source=request`),
    `${label} did not trace requested probe provenance`,
  );
  return [...anchors].sort();
}

export class FullRetrievalBlockedError extends Error {}

export function evidenceStatusForError(error) {
  return error instanceof FullRetrievalBlockedError ? "blocked" : "failed";
}

function generation(status) {
  const contract = status.manifest_contract ?? {};
  return {
    source_root: contract.source_root,
    input_hash: contract.input_hash,
    generation: contract.generation,
    schema_version: contract.schema_version,
    graph_hash: contract.graph_hash,
  };
}

export function verifyDrillPacketParity({ packet, report, summary, markdown, anchors, beforeStatus, afterStatus, artifacts }) {
  assert.equal(beforeStatus.retrieval_mode, "full", `before retrieval_mode=${beforeStatus.retrieval_mode ?? "missing"}`);
  assert.equal(afterStatus.retrieval_mode, "full", `after retrieval_mode=${afterStatus.retrieval_mode ?? "missing"}`);
  assert.deepEqual(generation(afterStatus), generation(beforeStatus), "retrieval generation changed during proof");

  const drillPacket = report.evidence_packet;
  assert.deepEqual(drillPacket.sufficiency, packet.sufficiency, "sufficiency differs");
  assert.deepEqual(citationKeys(drillPacket), citationKeys(packet), "citations differ");
  assert.deepEqual(requestedProbes(drillPacket, anchors, "drill packet"), [...anchors].sort());
  assert.deepEqual(requestedProbes(packet, anchors, "paired packet"), [...anchors].sort());
  assert.deepEqual(drillPacket.sufficiency.follow_up_commands, packet.sufficiency.follow_up_commands, "follow-ups differ");
  assert.deepEqual(report.next_commands, packet.sufficiency.follow_up_commands, "drill report follow-ups differ");

  assert.equal(report.question_search?.command, "packet", "drill did not report packet execution");
  assert.deepEqual(report.question_supplemental_searches ?? [], [], "drill ran supplemental searches");
  assert.ok((report.anchors ?? []).every((anchor) => (anchor.commands ?? []).length === 0), "drill ran anchor commands");
  assert.deepEqual((report.execution_boundaries ?? []).map(({ command }) => command), ["packet"], "drill execution boundary is not exactly one packet");
  assert.ok((report.bridges ?? []).every((bridge) => bridge.command?.command === "packet"), "drill ran a separate bridge command");

  assert.equal(summary.full_report_json, "drill-report.json");
  assert.equal(summary.full_report_markdown, "drill-report.md");
  assert.deepEqual([...artifacts].sort(), ["drill-report.json", "drill-report.md", "drill-summary.json"]);
  assert.match(markdown, /^# Drill\r?\n/);
  assert.match(markdown, /evidence_packet:/);
  return {
    generation: generation(beforeStatus),
    sufficiency: packet.sufficiency.status,
    citation_count: citationKeys(packet).length,
    explicit_probes: requestedProbes(packet, anchors, "paired packet"),
    follow_up_commands: packet.sufficiency.follow_up_commands,
    packet_execution_count: 1,
    artifacts: [...artifacts].sort(),
  };
}

function parseJson(bytes, label) {
  try {
    return JSON.parse(bytes);
  } catch (error) {
    throw new Error(`${label} was not JSON: ${error.message}`);
  }
}

function runCli(cli, args, transcript, label) {
  const result = spawnSync(cli, args, { encoding: "utf8", windowsHide: true });
  transcript.push({ label, executable: cli, args, status: result.status, stdout: result.stdout, stderr: result.stderr });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${label} failed (${result.status}): ${result.stderr.trim()}`);
  return result.stdout;
}

function writeEvidence(outputDir, evidence) {
  mkdirSync(outputDir, { recursive: true });
  writeFileSync(path.join(outputDir, "drill-packet-parity-evidence.json"), `${JSON.stringify(evidence, null, 2)}\n`);
}

function main() {
  const { values } = parseArgs({
    options: {
      cli: { type: "string", default: path.join(repoRoot, "target", "release", process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli") },
      project: { type: "string", default: repoRoot },
      question: { type: "string" },
      anchor: { type: "string", multiple: true },
      "run-id": { type: "string" },
      "output-dir": { type: "string" },
    },
    strict: true,
  });
  if (!values.question || !values.anchor?.length || !values["output-dir"]) {
    throw new Error("usage: prove-drill-packet-parity.mjs --question <text> --anchor <symbol> [--anchor <symbol>] --output-dir <dir> [--run-id <agent-run>] [--cli <path>] [--project <repo>]");
  }

  const project = path.resolve(values.project);
  const outputDir = path.resolve(values["output-dir"]);
  const drillDir = path.join(outputDir, "drill");
  const anchors = [...new Set(values.anchor.map((anchor) => anchor.trim()).filter(Boolean))];
  if (!anchors.length) throw new Error("at least one non-empty --anchor is required");
  mkdirSync(drillDir, { recursive: true });
  const transcript = [];
  const evidence = {
    schema_version: 1,
    status: "running",
    environment: { platform: process.platform, arch: process.arch, node: process.version, cli: path.resolve(values.cli), project },
    transcript,
  };

  try {
    const agentSelection = ["--profile", "agent"];
    if (values["run-id"]) agentSelection.push("--run-id", values["run-id"]);
    const statusArgs = ["retrieval", "status", "--project", project, ...agentSelection, "--format", "json"];
    const beforeStatus = parseJson(runCli(values.cli, statusArgs, transcript, "retrieval-status-before"), "retrieval status before");
    evidence.readiness = { retrieval_mode: beforeStatus.retrieval_mode, degraded_reason: beforeStatus.degraded_reason ?? null, manifest_contract: beforeStatus.manifest_contract ?? null };
    if (beforeStatus.retrieval_mode !== "full") {
      throw new FullRetrievalBlockedError(`full-retrieval proof blocked: retrieval_mode=${beforeStatus.retrieval_mode ?? "missing"}; degraded_reason=${beforeStatus.degraded_reason ?? "missing"}`);
    }

    const packetArgs = ["packet", "--project", project, ...agentSelection, "--question", values.question, "--budget", "standard", "--refresh", "none", "--format", "json"];
    for (const anchor of anchors) packetArgs.push("--extra-probe", anchor);
    const packet = parseJson(runCli(values.cli, packetArgs, transcript, "packet"), "packet");
    const drillArgs = ["drill", "--project", project, ...agentSelection, "--question", values.question, "--anchors", anchors.join(","), "--output-dir", drillDir, "--refresh", "none", "--format", "json"];
    runCli(values.cli, drillArgs, transcript, "drill");
    const afterStatus = parseJson(runCli(values.cli, statusArgs, transcript, "retrieval-status-after"), "retrieval status after");
    const report = parseJson(readFileSync(path.join(drillDir, "drill-report.json")), "drill report");
    const summary = parseJson(readFileSync(path.join(drillDir, "drill-summary.json")), "drill summary");
    const markdown = readFileSync(path.join(drillDir, "drill-report.md"), "utf8");
    evidence.result = verifyDrillPacketParity({
      packet,
      report,
      summary,
      markdown,
      anchors,
      beforeStatus,
      afterStatus,
      artifacts: readdirSync(drillDir),
    });
    evidence.status = "passed";
    writeEvidence(outputDir, evidence);
  } catch (error) {
    evidence.status = evidenceStatusForError(error);
    evidence.error = error.message;
    writeEvidence(outputDir, evidence);
    throw error;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  try {
    main();
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
