import assert from "node:assert/strict";
import test from "node:test";
import {
  FullRetrievalBlockedError,
  evidenceStatusForError,
  verifyDrillPacketParity,
} from "../prove-drill-packet-parity.mjs";

function packet() {
  return {
    kind: "complete",
    schema_version: 3,
    identity: { packet_id: "packet-1" },
    status: "continuation_available",
    evidence: [{
      identity: { evidence_id: "evidence-1" },
      kind: "source_range",
      path: "src/lib.rs",
      symbol_id: "WorkspaceIndexer",
      start_line: 4,
      end_line: 4,
      summary: "WorkspaceIndexer evidence",
    }],
    gaps: [{
      identity: { gap_id: "gap-1" },
      kind: "continuation_required",
      message: "Additional evidence is required.",
    }],
    continuation: { continuation_id: "continuation-1", remaining_rounds: 1 },
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
      question_search: { command: "packet", status: pairedPacket.status },
      question_supplemental_searches: [],
      anchors: [{ anchor: "WorkspaceIndexer", commands: [] }],
      bridges: [{ command: { command: "packet" } }],
      execution_boundaries: [{ command: "packet" }],
      next_commands: [],
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
    availability: "continuation_available",
    evidence_count: 1,
    explicit_probes: ["WorkspaceIndexer"],
    follow_up_commands: [],
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

test("report anchor handoff keeps the explicit requested probe", () => {
  const duplicate = proof();
  duplicate.report.anchors[0].anchor = "OtherAnchor";

  assert.throws(() => verifyDrillPacketParity(duplicate), /requested probe/);
});

test("only observed non-full preflight is blocked", () => {
  assert.equal(evidenceStatusForError(new FullRetrievalBlockedError("not full")), "blocked");
  assert.equal(evidenceStatusForError(new Error("packet command failed")), "failed");

  const mismatch = proof();
  mismatch.report.next_commands = ["hidden-legacy-follow-up"];
  let failure;
  try {
    verifyDrillPacketParity(mismatch);
  } catch (error) {
    failure = error;
  }
  assert.equal(evidenceStatusForError(failure), "failed");
});
