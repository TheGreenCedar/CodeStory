import assert from "node:assert/strict";
import test from "node:test";
import {
  FullRetrievalBlockedError,
  evidenceStatusForError,
  verifyDrillPacketParity,
} from "../prove-drill-packet-parity.mjs";

function packet() {
  const citation = { node_id: "node-1", file_path: "src/lib.rs", line: 4, display_name: "WorkspaceIndexer" };
  return {
    plan: {
      queries: [{ query: "WorkspaceIndexer", purpose: "explicit symbol probe from packet request" }],
      trace: ["explicit_extra_probes=1 source=request"],
    },
    answer: { citations: [citation] },
    sufficiency: {
      status: "partial",
      covered_claims: [{ claim: "indexing feeds search", citations: [citation] }],
      follow_up_commands: ["codestory-cli snippet --query WorkspaceIndexer --project ."],
    },
  };
}

function status() {
  return {
    retrieval_mode: "full",
    manifest_contract: {
      source_root: "/repo",
      input_hash: "input",
      generation: "generation",
      schema_version: 3,
      graph_hash: "graph",
    },
  };
}

function proof() {
  const pairedPacket = packet();
  return {
    packet: pairedPacket,
    report: {
      evidence_packet: structuredClone(pairedPacket),
      question_search: { command: "packet" },
      question_supplemental_searches: [],
      anchors: [{ commands: [] }],
      bridges: [{ command: { command: "packet" } }],
      execution_boundaries: [{ command: "packet" }],
      next_commands: pairedPacket.sufficiency.follow_up_commands,
    },
    summary: { full_report_json: "drill-report.json", full_report_markdown: "drill-report.md" },
    markdown: "# Drill\nevidence_packet: packet\n",
    anchors: ["WorkspaceIndexer"],
    beforeStatus: status(),
    afterStatus: status(),
    artifacts: ["drill-summary.json", "drill-report.md", "drill-report.json"],
  };
}

test("paired packet and drill proof accepts one matching packet execution", () => {
  assert.deepEqual(verifyDrillPacketParity(proof()), {
    generation: {
      source_root: "/repo",
      input_hash: "input",
      generation: "generation",
      schema_version: 3,
      graph_hash: "graph",
    },
    sufficiency: "partial",
    citation_count: 1,
    explicit_probes: ["WorkspaceIndexer"],
    follow_up_commands: ["codestory-cli snippet --query WorkspaceIndexer --project ."],
    packet_execution_count: 1,
    artifacts: ["drill-report.json", "drill-report.md", "drill-summary.json"],
  });
});

test("paired proof rejects generation drift and duplicate drill commands", () => {
  const drift = proof();
  drift.afterStatus.manifest_contract.generation = "next-generation";
  assert.throws(() => verifyDrillPacketParity(drift), /retrieval generation changed/);

  const duplicate = proof();
  duplicate.report.anchors[0].commands.push({ command: "search" });
  assert.throws(() => verifyDrillPacketParity(duplicate), /anchor commands/);
});

test("question and anchor dedupe keeps executed probe provenance", () => {
  const duplicate = proof();
  duplicate.packet.plan.queries[0].purpose = "original task phrasing for retrieval-primary source-backed retrieval";
  duplicate.report.evidence_packet.plan.queries[0].purpose = duplicate.packet.plan.queries[0].purpose;

  assert.equal(verifyDrillPacketParity(duplicate).packet_execution_count, 1);
});

test("only observed non-full preflight is blocked", () => {
  assert.equal(evidenceStatusForError(new FullRetrievalBlockedError("not full")), "blocked");
  assert.equal(evidenceStatusForError(new Error("packet command failed")), "failed");

  const mismatch = proof();
  mismatch.report.next_commands = [];
  let failure;
  try {
    verifyDrillPacketParity(mismatch);
  } catch (error) {
    failure = error;
  }
  assert.equal(evidenceStatusForError(failure), "failed");
});
