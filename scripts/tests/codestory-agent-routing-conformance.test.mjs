import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test, { after } from "node:test";

import {
  INSTALLED_IDENTITY_FIELDS,
  ROUTING_SCENARIOS,
  STATIC_PARITY_HOSTS,
  canonicalRequestContractDigest,
  parseInstalledTranscript,
  validateInstalledSession,
  validateStaticHostParity,
} from "../codestory-agent-routing-conformance.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..", "..");
const pluginRoot = join(repoRoot, "plugins", "codestory");
const SHA_D = "d".repeat(64);

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

const installedRoot = realpathSync(mkdtempSync(join(tmpdir(), "codestory-routing-installed-")));
mkdirSync(join(installedRoot, "archives"), { recursive: true });
mkdirSync(join(installedRoot, "scripts"), { recursive: true });
mkdirSync(join(installedRoot, "managed"), { recursive: true });
writeFileSync(join(installedRoot, "archives", "codestory-0.18.tgz"), "authenticated package fixture\n");
copyFileSync(
  join(pluginRoot, "scripts", "codestory-mcp.cjs"),
  join(installedRoot, "scripts", "codestory-mcp.cjs"),
);
writeFileSync(join(installedRoot, "managed", "codestory-cli"), "authenticated managed CLI fixture\n");

const installedIdentity = {
  installation: { root: installedRoot },
  package: {
    name: "codestory",
    version: "0.18.0-candidate.7",
    archive_relative_path: "archives/codestory-0.18.tgz",
    sha256: sha256(readFileSync(join(installedRoot, "archives", "codestory-0.18.tgz"))),
  },
  launcher: {
    relative_path: "scripts/codestory-mcp.cjs",
    sha256: sha256(readFileSync(join(installedRoot, "scripts", "codestory-mcp.cjs"))),
  },
  cli: {
    relative_path: "managed/codestory-cli",
    version: "0.18.0-candidate.7",
    sha256: sha256(readFileSync(join(installedRoot, "managed", "codestory-cli"))),
    source: "managed",
  },
  publication: { schema_version: 3 },
  protocol: { revision: "2025-11-25", discovery_contract_sha256: SHA_D },
};
const installedReceipt = join(installedRoot, "installed-receipt.json");
writeFileSync(installedReceipt, `${JSON.stringify({ schema_version: 1, identity: installedIdentity }, null, 2)}\n`);
const EXPECTED_IDENTITY = Object.freeze({
  ...cloneForFreeze(installedIdentity),
  receipt: Object.freeze({
    relative_path: "installed-receipt.json",
    sha256: sha256(readFileSync(installedReceipt)),
  }),
});

function cloneForFreeze(value) {
  if (Array.isArray(value)) return Object.freeze(value.map(cloneForFreeze));
  if (value && typeof value === "object") {
    return Object.freeze(Object.fromEntries(Object.entries(value).map(([key, child]) => [key, cloneForFreeze(child)])));
  }
  return value;
}

after(() => rmSync(installedRoot, { recursive: true, force: true }));

const SCENARIO_IDS = [
  "named_file_direct_read",
  "exact_symbol_search",
  "ambiguous_symbol_then_context",
  "selected_target_context",
  "broad_packet",
  "packet_single_continuation",
  "packet_gap_to_focused_source",
  "packet_unavailable_to_source",
  "typed_proof_contract_proven",
  "typed_proof_contract_refuted",
  "typed_proof_unknown",
  "typed_proof_unavailable",
  "malformed_proof_contract",
  "refuse_free_english_proof",
  "proof_observational",
  "hidden_proof_tool_discovery",
];

function clone(value) {
  return structuredClone(value);
}

function proofContract({ prohibited = false } = {}) {
  const sourceText = "`crate::start` directly calls `crate::finish`.";
  const fields = [
    { kind: "start" },
    { kind: "step_target", step: 0 },
    { kind: "directness", step: 0 },
    { kind: "ordering", step: 0 },
    { kind: "relation", step: 0 },
    ...(prohibited ? [{ kind: "traversal_prohibition", index: 0 }] : []),
  ];
  return {
    source_text: sourceText,
    clauses: [
      {
        clause_id: "contract",
        start: 0,
        end: Buffer.byteLength(sourceText),
        quote: sourceText,
        classification: "resolved_material",
        fields,
        reason: null,
        non_material_kind: null,
      },
    ],
    spec: {
      start: { kind: "canonical_id", canonical_id: "rust:crate::start" },
      steps: [{
        relation: "direct_outgoing_call",
        target: { kind: "canonical_id", canonical_id: "rust:crate::finish" },
      }],
      prohibit_traversal_through: prohibited
        ? [{ kind: "canonical_id", canonical_id: "rust:crate::blocked" }]
        : [],
      exclude_from_projection: [],
    },
  };
}

function runtimeMeta(overrides = {}) {
  return {
    codestory_publication: {
      schema_version: EXPECTED_IDENTITY.publication.schema_version,
      contract_runtime: {
        plugin_version: EXPECTED_IDENTITY.package.version,
        plugin_cli_version: EXPECTED_IDENTITY.cli.version,
        cli_version: EXPECTED_IDENTITY.cli.version,
        cli_sha256: EXPECTED_IDENTITY.cli.sha256,
        cli_source: EXPECTED_IDENTITY.cli.source,
        pinned_pair_matches: true,
        known_override_skew_channel: false,
      },
    },
    codestory_protocol: {
      negotiated: EXPECTED_IDENTITY.protocol.revision,
      discovery_contract_sha256: EXPECTED_IDENTITY.protocol.discovery_contract_sha256,
    },
    codestory_execution: { semantic_retrieval_activated: false },
    ...overrides,
  };
}

function result(body, { isError = false, meta = runtimeMeta() } = {}) {
  return {
    content: [{ type: "text", text: JSON.stringify(body) }],
    ...(isError ? { isError: true } : { structuredContent: body }),
    _meta: meta,
  };
}

function proofBody(disposition, contract, detail = {}) {
  const contractDigest = canonicalRequestContractDigest(contract);
  const common = { kind: disposition, contract_digest: contractDigest };
  let projectedDisposition;
  let stepStatus;
  const hasReceipt = ["contract_proven", "contract_refuted"].includes(disposition);
  const receipts = hasReceipt ? [{
    receipt_id: "receipt-1",
    edge_id: "edge-1",
    source: 0,
    target: 1,
    evidence: 0,
    exact_callsite_start_byte: 0,
    callsite_identity: "1:1:1:2|rust",
    column_or_ordinal: 1,
    containment: { file: 0, owner: 0, start_line: 1, end_line: 1 },
    line_window: {
      kind: "indexed_line_v1",
      file: 0,
      anchor_line: 1,
      byte_start: 0,
      byte_end: 10,
      text: "finish();\n",
    },
  }] : [];
  if (disposition === "contract_proven") {
    projectedDisposition = { ...common, receipts: [0] };
    stepStatus = "proven";
  } else if (disposition === "contract_refuted") {
    projectedDisposition = {
      ...common,
      refutation: {
        kind: detail.basis?.kind ?? "prohibited_scope_traversal",
        step_index: 0,
        prohibition_index: 0,
        connected_receipts: [0],
      },
    };
    stepStatus = "positive_contradiction";
  } else if (disposition === "unknown") {
    projectedDisposition = {
      ...common,
      gaps: (detail.gaps ?? []).map(({ code }) => code.startsWith("selector_")
        ? { kind: code, selector_index: 0 }
        : { kind: code, step_index: 0 }),
      connected_receipts: [],
    };
    stepStatus = "unknown";
  } else {
    projectedDisposition = { ...common, reasons: detail.reasons ?? [detail.reason] };
    stepStatus = "unavailable";
  }
  return {
    kind: "complete",
    schema_version: 1,
    domain: "indexed_source_call_path_v1",
    contract_interpretation: "host_supplied",
    guard_version: "clause_guard_v1",
    source_text_sha256: sha256(Buffer.from(contract.source_text)),
    contract_digest: contractDigest,
    core_publication: { project_id: "project-1", generation_id: "core-1", run_id: "run-1" },
    identities: hasReceipt ? {
      files: [{
        file_node_id: "1",
        project_file_components: ["src", "lib.rs"],
        indexed_sha256: "a".repeat(64),
        observed_sha256: "a".repeat(64),
      }],
      symbols: [
        { node_id: "1", canonical_id: "rust:crate::start", qualified_name: "crate::start", file: 0 },
        { node_id: "2", canonical_id: "rust:crate::finish", qualified_name: "crate::finish", file: 0 },
      ],
      provenance_profiles: [{
        producer: "codestory-internal",
        fact_schema_version: 1,
        algorithm: "exact-call-resolution-v1",
        language_adapter: "rust",
        language_adapter_version: "fixture-v1",
        parser_fingerprint: "b".repeat(64),
      }],
      evidence: [{
        fact_id: "c475943eeae97a7565be3dba007562b65e662b5111d1165b72ce2401e0d88eac",
        caller: 0,
        target: 1,
        edge_id: "edge-1",
        callsite_identity: "1:1:1:2|rust",
        chain: [{ kind: "same_file_declaration", symbols: [1] }],
        provenance: { profile: 0, dependency_files: [0], evidence_sha256: "d".repeat(64) },
      }],
    } : { files: [], symbols: [], provenance_profiles: [], evidence: [] },
    spec: {
      start: hasReceipt ? { kind: "canonical_id_ref", symbol: 0 } : clone(contract.spec.start),
      steps: [{
        relation: "direct_outgoing_call",
        target: hasReceipt ? { kind: "canonical_id_ref", symbol: 1 } : clone(contract.spec.steps[0].target),
      }],
      prohibit_traversal_through: clone(contract.spec.prohibit_traversal_through),
      exclude_from_projection: clone(contract.spec.exclude_from_projection),
    },
    clauses: clone(contract.clauses),
    disposition: projectedDisposition,
    steps: [{ step_index: 0, status: stepStatus, receipt: hasReceipt ? 0 : null }],
    receipts,
  };
}

function finalClaim(overrides = {}) {
  return {
    authority: "none",
    outcome: "supported",
    target_id: null,
    evidence_ids: [],
    gap_ids: [],
    reason_codes: [],
    proof_disposition: null,
    refutation_basis: null,
    runtime_execution_claim: false,
    absence_claim: false,
    material_omissions: [],
    ...overrides,
  };
}

function baseRun(scenarioId) {
  const typed = proofContract();
  const run = {
    scenario_id: scenarioId,
    request: {
      text: "Inspect the requested repository evidence.",
      named_files: [],
      selected_target: null,
      proof_contract: null,
    },
    steps: [],
    final: finalClaim(),
  };

  const mcp = (tool, args, body, options) => ({ kind: "mcp", tool, args, result: result(body, options) });
  const read = (path) => ({ kind: "source_read", path });

  switch (scenarioId) {
    case "named_file_direct_read":
      run.request.named_files = ["src/named.rs"];
      run.steps = [read("src/named.rs")];
      run.final = finalClaim({ authority: "source", evidence_ids: ["source:src/named.rs"] });
      break;
    case "exact_symbol_search":
      run.steps = [mcp("search", { project: "/workspace/repo", query: "ExactThing" }, {
        kind: "complete",
        publication: { core_generation_id: "core-1", retrieval_generation_id: "retrieval-1" },
        leads: [{ lead_id: "lead-1", canonical_id: "rust:crate::ExactThing" }],
        gaps: [],
      })];
      run.final = finalClaim({
        authority: "search_lead",
        outcome: "discovery_only",
        target_id: "rust:crate::ExactThing",
        evidence_ids: ["lead-1"],
      });
      break;
    case "ambiguous_symbol_then_context":
      run.request.selected_target = "rust:crate::one::Thing";
      run.steps = [
        mcp("search", { project: "/workspace/repo", query: "Thing" }, {
          kind: "complete",
          publication: { core_generation_id: "core-1", retrieval_generation_id: "retrieval-1" },
          leads: [
            { lead_id: "lead-1", canonical_id: "rust:crate::one::Thing" },
            { lead_id: "lead-2", canonical_id: "rust:crate::two::Thing" },
          ],
          gaps: [{ code: "selector_ambiguous" }],
        }),
        mcp("context", {
          project: "/workspace/repo",
          selector: { canonical_id: "rust:crate::one::Thing" },
        }, {
          kind: "complete",
          publication: { core_generation_id: "core-1", retrieval_generation_id: "retrieval-1" },
          target: { canonical_id: "rust:crate::one::Thing" },
          evidence: [{ evidence_id: "context-1", path: "src/one.rs" }],
          gaps: [],
        }),
      ];
      run.final = finalClaim({
        authority: "context_evidence",
        target_id: "rust:crate::one::Thing",
        evidence_ids: ["context-1"],
        gap_ids: ["selector_ambiguous"],
      });
      break;
    case "selected_target_context":
      run.request.selected_target = "rust:crate::ExactThing";
      run.steps = [mcp("context", {
        project: "/workspace/repo",
        selector: { canonical_id: "rust:crate::ExactThing" },
      }, {
        kind: "complete",
        publication: { core_generation_id: "core-1", retrieval_generation_id: "retrieval-1" },
        target: { canonical_id: "rust:crate::ExactThing" },
        evidence: [{ evidence_id: "context-1", path: "src/lib.rs" }],
        gaps: [],
      })];
      run.final = finalClaim({
        authority: "context_evidence",
        target_id: "rust:crate::ExactThing",
        evidence_ids: ["context-1"],
      });
      break;
    case "broad_packet":
      run.steps = [mcp("packet", { project: "/workspace/repo", question: "How does the flow work?" }, {
        kind: "complete",
        packet_id: "packet-1",
        status: "complete",
        publication: { core_generation_id: "core-1", retrieval_generation_id: "retrieval-1" },
        evidence: [{ evidence_id: "evidence-1", path: "src/flow.rs" }],
        gaps: [],
      })];
      run.final = finalClaim({ authority: "packet_evidence", evidence_ids: ["evidence-1"] });
      break;
    case "packet_single_continuation":
      run.steps = [
        mcp("packet", { project: "/workspace/repo", question: "How does the flow work?" }, {
          kind: "complete",
          packet_id: "packet-1",
          status: "continuation_available",
          publication: { core_generation_id: "core-1", retrieval_generation_id: "retrieval-1" },
          continuation: { continuation_id: "continuation-1", gap_ids: ["gap-1"] },
          evidence: [{ evidence_id: "evidence-1", path: "src/flow.rs" }],
          gaps: [{ gap_id: "gap-1" }],
        }),
        mcp("packet", {
          project: "/workspace/repo",
          question: "How does the flow work?",
          parent_packet_id: "continuation-1",
          option_ids: ["gap-1"],
          core_generation_id: "core-1",
          retrieval_generation_id: "retrieval-1",
        }, {
          kind: "complete",
          packet_id: "packet-2",
          status: "complete",
          publication: { core_generation_id: "core-1", retrieval_generation_id: "retrieval-1" },
          evidence: [{ evidence_id: "evidence-2", path: "src/more.rs" }],
          gaps: [],
        }),
      ];
      run.final = finalClaim({
        authority: "packet_evidence",
        evidence_ids: ["evidence-1", "evidence-2"],
        gap_ids: ["gap-1"],
      });
      break;
    case "packet_gap_to_focused_source":
      run.steps = [
        mcp("packet", { project: "/workspace/repo", question: "How does the flow work?" }, {
          kind: "complete",
          packet_id: "packet-1",
          status: "no_useful_evidence",
          publication: { core_generation_id: "core-1", retrieval_generation_id: "retrieval-1" },
          evidence: [],
          gaps: [{ gap_id: "gap-1", authorized_source_paths: ["src/gap.rs"] }],
        }),
        read("src/gap.rs"),
      ];
      run.final = finalClaim({ authority: "source", evidence_ids: ["source:src/gap.rs"], gap_ids: ["gap-1"] });
      break;
    case "packet_unavailable_to_source":
      run.steps = [
        mcp("packet", { project: "/workspace/repo", question: "How does the flow work?" }, {
          kind: "budget_exceeded",
          status: "unavailable",
          reason: "retrieval_unavailable",
          publication: { core_generation_id: "core-1", retrieval_generation_id: "retrieval-1" },
          evidence: [],
          gaps: [],
        }),
        read("src/fallback.rs"),
      ];
      run.final = finalClaim({
        authority: "source",
        outcome: "unavailable",
        evidence_ids: ["source:src/fallback.rs"],
        reason_codes: ["retrieval_unavailable"],
      });
      break;
    case "typed_proof_contract_proven":
      run.request.proof_contract = typed;
      run.steps = [mcp("prove_call_path", { project: "/workspace/repo", ...typed }, proofBody("contract_proven", typed))];
      run.final = finalClaim({ authority: "typed_proof", evidence_ids: ["receipt-1"], proof_disposition: "contract_proven" });
      break;
    case "typed_proof_contract_refuted":
      const refutedContract = proofContract({ prohibited: true });
      run.request.proof_contract = refutedContract;
      run.steps = [mcp("prove_call_path", { project: "/workspace/repo", ...refutedContract }, proofBody("contract_refuted", refutedContract, {
        basis: { kind: "prohibited_scope_traversal" },
      }))];
      run.final = finalClaim({
        authority: "typed_proof",
        outcome: "refuted",
        evidence_ids: ["receipt-1"],
        proof_disposition: "contract_refuted",
        refutation_basis: "prohibited_scope_traversal",
      });
      break;
    case "typed_proof_unknown":
      run.request.proof_contract = typed;
      run.steps = [mcp("prove_call_path", { project: "/workspace/repo", ...typed }, proofBody("unknown", typed, {
        gaps: [{ code: "selector_missing" }],
      }))];
      run.final = finalClaim({ authority: "typed_proof", outcome: "unknown", gap_ids: ["selector_missing"], proof_disposition: "unknown" });
      break;
    case "typed_proof_unavailable":
      run.request.proof_contract = typed;
      run.steps = [mcp("prove_call_path", { project: "/workspace/repo", ...typed }, proofBody("unavailable", typed, {
        reasons: ["proof_semantic_projection_unavailable"],
      }))];
      run.final = finalClaim({
        authority: "typed_proof",
        outcome: "unavailable",
        reason_codes: ["proof_semantic_projection_unavailable"],
        proof_disposition: "unavailable",
      });
      break;
    case "malformed_proof_contract": {
      const malformed = { source_text: "A calls B", clauses: [] };
      run.request.proof_contract = malformed;
      run.steps = [mcp("prove_call_path", { project: "/workspace/repo", ...malformed }, {
        code: "invalid_proof_interpretation",
        message: "spec is required",
      }, { isError: true })];
      run.final = finalClaim({ authority: "none", outcome: "invalid_contract", reason_codes: ["invalid_proof_interpretation"] });
      break;
    }
    case "refuse_free_english_proof":
      run.request.text = "Prove from this sentence that start calls finish.";
      run.final = finalClaim({ authority: "none", outcome: "refused", reason_codes: ["typed_contract_required"] });
      break;
    case "proof_observational":
      run.request.proof_contract = typed;
      run.steps = [mcp("prove_call_path", { project: "/workspace/repo", ...typed }, proofBody("unknown", typed, {
        gaps: [{ code: "direct_call_missing" }],
      }))];
      run.final = finalClaim({
        authority: "typed_proof",
        outcome: "unknown",
        gap_ids: ["direct_call_missing"],
        proof_disposition: "unknown",
      });
      break;
    case "hidden_proof_tool_discovery":
      run.request.proof_contract = typed;
      run.steps = [
        { kind: "tool_search", query: "codestory mcp prove_call_path", tools: ["mcp__codestory__prove_call_path"] },
        mcp("prove_call_path", { project: "/workspace/repo", ...typed }, proofBody("contract_proven", typed)),
      ];
      run.final = finalClaim({ authority: "typed_proof", evidence_ids: ["receipt-1"], proof_disposition: "contract_proven" });
      break;
    default:
      throw new Error(`unknown scenario ${scenarioId}`);
  }
  return run;
}

function codexJsonl(run) {
  const events = [{ type: "thread.started", thread_id: "thread-1" }];
  let counter = 0;
  for (const step of run.steps) {
    counter += 1;
    const id = `call-${counter}`;
    if (step.kind === "mcp") {
      const item = {
        id,
        type: "mcp_tool_call",
        server: "codestory",
        tool: step.tool,
        arguments: step.args,
      };
      events.push({ type: "item.started", item });
      events.push({ type: "item.completed", item: { ...item, status: "completed", result: step.result } });
    } else if (step.kind === "tool_search") {
      const item = {
        id,
        type: "mcp_tool_call",
        server: "codex-tools",
        tool: "tool_search",
        arguments: { query: step.query },
      };
      events.push({ type: "item.started", item });
      events.push({ type: "item.completed", item: { ...item, status: "completed", result: { tools: step.tools } } });
    } else if (step.kind === "source_read") {
      const item = { id, type: "command_execution", command: `sed -n '1,120p' '${step.path}'` };
      events.push({ type: "item.started", item });
      events.push({
        type: "item.completed",
        item: { ...item, status: "completed", exit_code: 0, aggregated_output: "source fixture\n" },
      });
    }
  }
  events.push({
    type: "item.completed",
    item: { id: "answer", type: "agent_message", text: JSON.stringify(run.final) },
  });
  return `${events.map((event) => JSON.stringify(event)).join("\n")}\n`;
}

function cursorJsonl(run) {
  const sessionId = "cursor-session-1";
  const events = [
    {
      type: "system",
      subtype: "init",
      apiKeySource: "login",
      cwd: "/workspace/repo",
      session_id: sessionId,
      model: "fixture-model",
      permissionMode: "default",
    },
    {
      type: "user",
      message: { role: "user", content: [{ type: "text", text: run.request.text }] },
      session_id: sessionId,
    },
  ];
  let counter = 0;
  for (const step of run.steps) {
    counter += 1;
    const id = `call-${counter}`;
    let wrapper;
    if (step.kind === "mcp") {
      const args = {
        name: `codestory-${step.tool}`,
        args: step.args,
        toolCallId: id,
        providerIdentifier: "codestory",
        toolName: step.tool,
      };
      wrapper = {
        started: { mcpToolCall: { args } },
        completed: { mcpToolCall: { args, result: { success: step.result } } },
      };
    } else if (step.kind === "tool_search") {
      const args = { query: step.query };
      wrapper = {
        started: { toolSearchToolCall: { args } },
        completed: { toolSearchToolCall: { args, result: { success: { tools: step.tools } } } },
      };
    } else {
      const args = { path: step.path };
      wrapper = {
        started: { readToolCall: { args } },
        completed: {
          readToolCall: {
            args,
            result: {
              success: {
                content: "source fixture\n",
                isEmpty: false,
                exceededLimit: false,
                totalLines: 1,
                totalChars: 15,
              },
            },
          },
        },
      };
    }
    events.push({
      type: "tool_call",
      subtype: "started",
      call_id: id,
      tool_call: wrapper.started,
      session_id: sessionId,
    });
    events.push({
      type: "tool_call",
      subtype: "completed",
      call_id: id,
      tool_call: wrapper.completed,
      session_id: sessionId,
    });
  }
  const final = JSON.stringify(run.final);
  const split = Math.floor(final.length / 2);
  events.push({
    type: "assistant",
    message: { role: "assistant", content: [{ type: "text", text: final.slice(0, split) }] },
    session_id: sessionId,
  });
  events.push({
    type: "assistant",
    message: { role: "assistant", content: [{ type: "text", text: final.slice(split) }] },
    session_id: sessionId,
  });
  events.push({
    type: "result",
    subtype: "success",
    duration_ms: 10,
    duration_api_ms: 9,
    is_error: false,
    result: final,
    session_id: sessionId,
  });
  return `${events.map((event) => JSON.stringify(event)).join("\n")}\n`;
}

function transcript(host, run) {
  return host === "codex" ? codexJsonl(run) : cursorJsonl(run);
}

function validate(host, run, expectedIdentity = EXPECTED_IDENTITY, receipt = installedReceipt, root = installedRoot) {
  return validateInstalledSession({
    host,
    scenarioId: run.scenario_id,
    request: run.request,
    installedRoot: root,
    installedReceipt: receipt,
    expectedIdentity,
    transcript: transcript(host, run),
  });
}

test("freezes exactly the sixteen accepted routing scenarios", () => {
  assert.deepEqual(ROUTING_SCENARIOS.map(({ id }) => id), SCENARIO_IDS);
  assert.equal(new Set(ROUTING_SCENARIOS.map(({ id }) => id)).size, 16);
  for (const scenario of ROUTING_SCENARIOS) {
    assert.equal(typeof scenario.expected_first_tool, "string", scenario.id);
    assert.ok(Array.isArray(scenario.required_action_sequence), scenario.id);
    assert.ok(Array.isArray(scenario.permitted_followups), scenario.id);
    assert.ok(Array.isArray(scenario.forbidden_tools), scenario.id);
    assert.equal(typeof scenario.source_read_authorization?.kind, "string", scenario.id);
    assert.equal(typeof scenario.final_claim_constraints, "object", scenario.id);
    assert.deepEqual(scenario.identity_requirements.fields, INSTALLED_IDENTITY_FIELDS, scenario.id);
    assert.equal(scenario.identity_requirements.mode, "exact", scenario.id);
  }
  assert.equal(Object.isFrozen(ROUTING_SCENARIOS), true);
  assert.equal(Object.isFrozen(ROUTING_SCENARIOS[0]), true);
});

for (const host of ["codex", "cursor"]) {
  test(`${host} real-session parser accepts all sixteen frozen scenarios`, () => {
    for (const scenarioId of SCENARIO_IDS) {
      const report = validate(host, baseRun(scenarioId));
      assert.equal(report.status, "pass", scenarioId);
      assert.equal(report.host, host, scenarioId);
      assert.equal(report.scenario_id, scenarioId, scenarioId);
    }
  });
}

test("actual parsers reject malformed, incomplete, and cross-host transcripts", () => {
  assert.throws(() => parseInstalledTranscript("codex", "{not-json}\n"), /malformed JSONL/u);
  assert.throws(
    () => parseInstalledTranscript("codex", `${JSON.stringify({
      type: "item.started",
      item: { id: "dangling", type: "mcp_tool_call", server: "codestory", tool: "packet" },
    })}\n`),
    /unmatched tool call/u,
  );
  assert.throws(() => parseInstalledTranscript("cursor", codexJsonl(baseRun("broad_packet"))), /Cursor/u);
  assert.throws(() => parseInstalledTranscript("unknown", "{}\n"), /unsupported host/u);

  const overlapping = codexJsonl(baseRun("packet_gap_to_focused_source"))
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));
  [overlapping[2], overlapping[3]] = [overlapping[3], overlapping[2]];
  assert.throws(
    () => parseInstalledTranscript("codex", `${overlapping.map(JSON.stringify).join("\n")}\n`),
    /started before .* completed/u,
  );
});

const CURSOR_CAPTURED_DIRECT_READ = [
  { type: "system", subtype: "init", apiKeySource: "login", cwd: "/workspace/repo", session_id: "captured-1", model: "Claude 4 Sonnet", permissionMode: "default" },
  { type: "user", message: { role: "user", content: [{ type: "text", text: "Read src/named.rs" }] }, session_id: "captured-1" },
  { type: "tool_call", subtype: "started", call_id: "toolu_read", tool_call: { readToolCall: { args: { path: "src/named.rs" } } }, session_id: "captured-1" },
  { type: "tool_call", subtype: "completed", call_id: "toolu_read", tool_call: { readToolCall: { args: { path: "src/named.rs" }, result: { success: { content: "fn named() {}\n", isEmpty: false, exceededLimit: false, totalLines: 1, totalChars: 14 } } } }, session_id: "captured-1" },
  { type: "assistant", message: { role: "assistant", content: [{ type: "text", text: "{\"authority\":\"source\"," }] }, session_id: "captured-1" },
  { type: "assistant", message: { role: "assistant", content: [{ type: "text", text: "\"outcome\":\"supported\"}" }] }, session_id: "captured-1" },
  { type: "result", subtype: "success", duration_ms: 12, duration_api_ms: 10, is_error: false, result: "{\"authority\":\"source\",\"outcome\":\"supported\"}", session_id: "captured-1", request_id: "request-1" },
];

const CURSOR_CAPTURED_MCP = [
  { type: "system", subtype: "init", apiKeySource: "login", cwd: "/workspace/repo", session_id: "captured-2", model: "Claude 4 Sonnet", permissionMode: "default" },
  { type: "user", message: { role: "user", content: [{ type: "text", text: "Search ExactThing" }] }, session_id: "captured-2" },
  { type: "tool_call", subtype: "started", call_id: "toolu_mcp", tool_call: { mcpToolCall: { args: { name: "codestory-search", args: { project: "/workspace/repo", query: "ExactThing" }, toolCallId: "toolu_mcp", providerIdentifier: "codestory", toolName: "search" } } }, session_id: "captured-2" },
  { type: "tool_call", subtype: "completed", call_id: "toolu_mcp", tool_call: { mcpToolCall: { args: { name: "codestory-search", args: { project: "/workspace/repo", query: "ExactThing" }, toolCallId: "toolu_mcp", providerIdentifier: "codestory", toolName: "search" }, result: { success: { content: [{ type: "text", text: "{}" }] } } } }, session_id: "captured-2" },
  { type: "assistant", message: { role: "assistant", content: [{ type: "text", text: "{}" }] }, session_id: "captured-2" },
  { type: "result", subtype: "success", duration_ms: 12, duration_api_ms: 10, is_error: false, result: "{}", session_id: "captured-2" },
];

function capturedJsonl(events) {
  return `${events.map((event) => JSON.stringify(event)).join("\n")}\n`;
}

test("Cursor official stream-json captured shapes are correlated and terminal", () => {
  const direct = parseInstalledTranscript("cursor", capturedJsonl(CURSOR_CAPTURED_DIRECT_READ));
  assert.deepEqual(direct.actions.map(({ kind }) => kind), ["source_read"]);
  assert.equal(direct.final, "{\"authority\":\"source\",\"outcome\":\"supported\"}");
  assert.equal(direct.user_text, "Read src/named.rs");

  const mcp = parseInstalledTranscript("cursor", capturedJsonl(CURSOR_CAPTURED_MCP));
  assert.deepEqual(mcp.actions.map(({ kind }) => kind), ["search"]);

  const failure = clone(CURSOR_CAPTURED_DIRECT_READ);
  failure.at(-1).subtype = "error";
  failure.at(-1).is_error = true;
  assert.throws(() => parseInstalledTranscript("cursor", capturedJsonl(failure)), /terminal result.*success/u);

  assert.throws(
    () => parseInstalledTranscript("cursor", capturedJsonl(CURSOR_CAPTURED_DIRECT_READ.slice(0, -1))),
    /terminal result/u,
  );

  const partial = clone(CURSOR_CAPTURED_DIRECT_READ);
  delete partial[3].tool_call.readToolCall.result;
  assert.throws(() => parseInstalledTranscript("cursor", capturedJsonl(partial)), /completed tool call.*success/u);

  const overlap = clone(CURSOR_CAPTURED_DIRECT_READ);
  overlap.splice(3, 0, clone(CURSOR_CAPTURED_MCP[2]));
  overlap[3].session_id = "captured-1";
  assert.throws(() => parseInstalledTranscript("cursor", capturedJsonl(overlap)), /started before .* completed/u);

  const terminalMismatch = clone(CURSOR_CAPTURED_DIRECT_READ);
  terminalMismatch.at(-1).result = "different";
  assert.throws(() => parseInstalledTranscript("cursor", capturedJsonl(terminalMismatch)), /assistant deltas.*terminal result/u);
});

function mutateBody(run, index, mutate) {
  const body = run.steps[index].result.structuredContent;
  mutate(body);
  run.steps[index].result.content[0].text = JSON.stringify(body);
}

const MUTATIONS = [
  {
    name: "missing required packet continuation",
    scenario: "packet_single_continuation",
    mutate(run) {
      run.steps.pop();
    },
    error: /required action sequence/u,
  },
  {
    name: "missing required selected-target context",
    scenario: "ambiguous_symbol_then_context",
    mutate(run) {
      run.steps.pop();
    },
    error: /required action sequence/u,
  },
  {
    name: "wrong first tool",
    scenario: "broad_packet",
    mutate(run) {
      run.steps.unshift(baseRun("selected_target_context").steps[0]);
    },
    error: /required action sequence|expected first tool packet/u,
  },
  {
    name: "forbidden followup",
    scenario: "broad_packet",
    mutate(run) {
      run.steps.push(baseRun("selected_target_context").steps[0]);
    },
    error: /required action sequence|follow-up context is not permitted/u,
  },
  {
    name: "unauthorized source read",
    scenario: "packet_gap_to_focused_source",
    mutate(run) {
      run.steps[1].path = "src/unrelated.rs";
    },
    error: /source read is not authorized/u,
  },
  {
    name: "authority escalation in final claim",
    scenario: "exact_symbol_search",
    mutate(run) {
      run.final.authority = "typed_proof";
      run.final.proof_disposition = "contract_proven";
    },
    error: /final claim authority/u,
  },
  {
    name: "prose cannot replace structured final claims",
    scenario: "typed_proof_unknown",
    mutate(run) {
      run.final = "Unknown, so I did not establish absence.";
    },
    error: /final claim.*required schema/u,
  },
  {
    name: "final evidence must come from the selected result",
    scenario: "selected_target_context",
    mutate(run) {
      run.final.evidence_ids = ["fabricated-evidence"];
    },
    error: /final claim evidence_ids/u,
  },
  {
    name: "search result without leads",
    scenario: "exact_symbol_search",
    mutate(run) {
      mutateBody(run, 0, (body) => delete body.leads);
    },
    error: /search result/u,
  },
  {
    name: "context result without evidence",
    scenario: "selected_target_context",
    mutate(run) {
      mutateBody(run, 0, (body) => delete body.evidence);
    },
    error: /context result/u,
  },
  {
    name: "packet result without evidence",
    scenario: "broad_packet",
    mutate(run) {
      mutateBody(run, 0, (body) => delete body.evidence);
    },
    error: /packet result/u,
  },
  {
    name: "complete proof result without steps",
    scenario: "typed_proof_contract_proven",
    mutate(run) {
      mutateBody(run, 0, (body) => delete body.steps);
    },
    error: /proof result/u,
  },
  {
    name: "proof result contract projection drift",
    scenario: "typed_proof_contract_proven",
    mutate(run) {
      mutateBody(run, 0, (body) => {
        body.source_text_sha256 = "a".repeat(64);
      });
    },
    error: /semantic invariant.*source_text_sha256/u,
  },
  {
    name: "proof receipt must match exact-resolution evidence",
    scenario: "typed_proof_contract_proven",
    mutate(run) {
      mutateBody(run, 0, (body) => {
        body.receipts[0].callsite_identity = "different-callsite";
      });
    },
    error: /does not match exact-resolution evidence/u,
  },
  {
    name: "unexpected CodeStory tool error",
    scenario: "broad_packet",
    mutate(run) {
      run.steps[0].result.isError = true;
      delete run.steps[0].result.structuredContent;
    },
    error: /unexpected failed packet action/u,
  },
  {
    name: "disposition drift",
    scenario: "typed_proof_unknown",
    mutate(run) {
      run.steps[0].result.structuredContent.disposition.kind = "contract_proven";
      run.steps[0].result.content[0].text = JSON.stringify(run.steps[0].result.structuredContent);
    },
    error: /proof result.*disposition/u,
  },
  {
    name: "proof retrieval activation",
    scenario: "proof_observational",
    mutate(run) {
      run.steps[0].result._meta.codestory_execution.semantic_retrieval_activated = true;
    },
    error: /proof activated semantic retrieval/u,
  },
  {
    name: "hidden discovery broadens lookup",
    scenario: "hidden_proof_tool_discovery",
    mutate(run) {
      run.steps[0].query = "codestory mcp";
      run.steps[0].tools.push("mcp__codestory__packet");
    },
    error: /hidden-tool discovery/u,
  },
  {
    name: "second packet continuation",
    scenario: "packet_single_continuation",
    mutate(run) {
      run.steps.push(clone(run.steps[1]));
    },
    error: /required action sequence|at most one packet continuation/u,
  },
  {
    name: "selector relaxation retry",
    scenario: "typed_proof_contract_proven",
    mutate(run) {
      const retry = clone(run.steps[0]);
      retry.args.spec.start = { kind: "qualified", qualified_name: "crate::start" };
      run.steps.push(retry);
    },
    error: /required action sequence|follow-up prove_call_path is not permitted|proof request must preserve the host-supplied typed contract|proof may be called only once/u,
  },
  {
    name: "unknown becomes absence",
    scenario: "typed_proof_unknown",
    mutate(run) {
      run.final.absence_claim = true;
    },
    error: /final claim absence_claim|Unknown must not become absence/u,
  },
  {
    name: "silent material gap",
    scenario: "packet_gap_to_focused_source",
    mutate(run) {
      run.final.gap_ids = [];
    },
    error: /final claim gap_ids/u,
  },
  {
    name: "free English proof construction",
    scenario: "refuse_free_english_proof",
    mutate(run) {
      run.steps.push(baseRun("typed_proof_contract_proven").steps[0]);
      run.final = finalClaim({ authority: "typed_proof", proof_disposition: "contract_proven" });
    },
    error: /required action sequence|proof requires a host-supplied typed contract|expected no tool/u,
  },
  {
    name: "unavailable reason omitted",
    scenario: "typed_proof_unavailable",
    mutate(run) {
      run.final.reason_codes = [];
    },
    error: /reason_codes/u,
  },
  {
    name: "refutation basis omitted",
    scenario: "typed_proof_contract_refuted",
    mutate(run) {
      run.final.refutation_basis = null;
    },
    error: /refutation_basis/u,
  },
  {
    name: "packet claim contradicts packet authority",
    scenario: "broad_packet",
    mutate(run) {
      run.final.absence_claim = true;
    },
    error: /absence_claim|packet authority/u,
  },
  {
    name: "context result target must match the selected target",
    scenario: "selected_target_context",
    mutate(run) {
      mutateBody(run, 0, (body) => {
        body.target.canonical_id = "rust:crate::OtherThing";
      });
    },
    error: /context result does not match the selected target/u,
  },
];

for (const host of ["codex", "cursor"]) {
  test(`${host} hostile routing matrix fails closed through its parser`, () => {
    for (const mutation of MUTATIONS) {
      const run = baseRun(mutation.scenario);
      mutation.mutate(run);
      assert.throws(() => validate(host, run), mutation.error, mutation.name);
    }
  });
}

const SEMANTIC_BINDING_MUTATIONS = [
  {
    name: "ContractProven with Unknown step and no receipt",
    scenario: "typed_proof_contract_proven",
    mutate(body) {
      body.steps[0].status = "unknown";
      body.steps[0].receipt = null;
    },
  },
  {
    name: "ContractRefuted with Unknown step",
    scenario: "typed_proof_contract_refuted",
    mutate(body) {
      body.steps[0].status = "unknown";
      body.steps[0].receipt = null;
    },
  },
  {
    name: "Unknown with Proven step",
    scenario: "typed_proof_unknown",
    mutate(body) {
      body.steps[0].status = "proven";
    },
  },
  {
    name: "Unavailable with Proven step",
    scenario: "typed_proof_unavailable",
    mutate(body) {
      body.steps[0].status = "proven";
    },
  },
  {
    name: "changed digest not derived from the request",
    scenario: "typed_proof_contract_proven",
    mutate(body) {
      body.contract_digest = "a".repeat(64);
      body.disposition.contract_digest = body.contract_digest;
    },
  },
  {
    name: "exact callsite start outside its hash-bound window",
    scenario: "typed_proof_contract_proven",
    mutate(body) {
      body.receipts[0].exact_callsite_start_byte = body.receipts[0].line_window.byte_end;
    },
  },
];

for (const host of ["codex", "cursor"]) {
  test(`${host} canonical proof semantic-binding matrix fails closed`, () => {
    const accepted = [];
    for (const mutation of SEMANTIC_BINDING_MUTATIONS) {
      const run = baseRun(mutation.scenario);
      mutateBody(run, 0, mutation.mutate);
      try {
        validate(host, run);
        accepted.push(mutation.name);
      } catch (error) {
        assert.match(error.message, /proof result semantic invariant/u, mutation.name);
      }
    }
    assert.deepEqual(accepted, [], `semantic mutations accepted through ${host}`);
  });
}

const SELECTOR_GAP_KINDS = ["selector_missing", "selector_ambiguous", "non_callable_selector"];
const STEP_GAP_KINDS = [
  "direct_call_missing",
  "recursive_call_not_representable",
  "source_window_too_large",
  "invalid_utf8",
  "source_line_out_of_range",
  "edge_containment_unproven",
  "missing_direct_call_receipt",
  "receipt_or_edge_already_used",
  "projection_exclusion_conflicts_with_required_receipt",
];

for (const host of ["codex", "cursor"]) {
  test(`${host} canonical proof gap indices use the projected step count`, () => {
    const acceptedOutOfRange = [];
    for (const [kind, indexField, validIndex, invalidIndex] of [
      ...SELECTOR_GAP_KINDS.map((kind) => [kind, "selector_index", 1, 2]),
      ...STEP_GAP_KINDS.map((kind) => [kind, "step_index", 0, 1]),
    ]) {
      const boundary = baseRun("typed_proof_unknown");
      mutateBody(boundary, 0, (body) => {
        body.disposition.gaps = [{ kind, [indexField]: validIndex }];
      });
      boundary.final.gap_ids = [kind];
      assert.equal(validate(host, boundary).status, "pass", `${kind} accepted boundary`);

      const outOfRange = baseRun("typed_proof_unknown");
      mutateBody(outOfRange, 0, (body) => {
        body.disposition.gaps = [{ kind, [indexField]: invalidIndex }];
      });
      outOfRange.final.gap_ids = [kind];
      try {
        validate(host, outOfRange);
        acceptedOutOfRange.push(kind);
      } catch (error) {
        assert.match(error.message, /proof result semantic invariant.*gap index/u, kind);
      }
    }
    assert.deepEqual(acceptedOutOfRange, [], `out-of-range gaps accepted through ${host}`);
  });
}

test("every installed identity field is exact and every CodeStory result repeats runtime identity", () => {
  for (const field of INSTALLED_IDENTITY_FIELDS) {
    const expected = clone(EXPECTED_IDENTITY);
    const parts = field.split(".");
    let owner = expected;
    for (const part of parts.slice(0, -1)) owner = owner[part];
    const leaf = parts.at(-1);
    owner[leaf] = typeof owner[leaf] === "number" ? owner[leaf] + 1 : `${owner[leaf]}-drift`;
    const expectedError = field === "receipt.relative_path"
      ? /installed receipt/u
      : new RegExp(field.replaceAll(".", "\\."), "u");
    assert.throws(
      () => validate("codex", baseRun("broad_packet"), expected),
      expectedError,
      field,
    );
  }

  const resultDrift = baseRun("broad_packet");
  resultDrift.steps[0].result._meta.codestory_protocol.discovery_contract_sha256 = "e".repeat(64);
  assert.throws(() => validate("codex", resultDrift), /result identity.*discovery/u);
});

test("installed receipt authentication rejects substituted package launcher CLI and forged receipt", () => {
  for (const relativePath of [
    "archives/codestory-0.18.tgz",
    "scripts/codestory-mcp.cjs",
    "managed/codestory-cli",
  ]) {
    const path = join(installedRoot, relativePath);
    const original = readFileSync(path);
    try {
      writeFileSync(path, "substituted bytes\n");
      assert.throws(() => validate("codex", baseRun("broad_packet")), /authenticated .* bytes/u, relativePath);
    } finally {
      writeFileSync(path, original);
    }
  }

  const originalReceipt = readFileSync(installedReceipt);
  try {
    const forged = JSON.parse(originalReceipt.toString("utf8"));
    forged.identity.package.sha256 = "1".repeat(64);
    const forgedBytes = Buffer.from(`${JSON.stringify(forged, null, 2)}\n`);
    writeFileSync(installedReceipt, forgedBytes);
    const expected = clone(EXPECTED_IDENTITY);
    expected.package.sha256 = forged.identity.package.sha256;
    expected.receipt.sha256 = sha256(forgedBytes);
    assert.throws(
      () => validate("codex", baseRun("broad_packet"), expected),
      /package\.sha256 does not match authenticated package bytes/u,
    );
  } finally {
    writeFileSync(installedReceipt, originalReceipt);
  }

  try {
    writeFileSync(installedReceipt, "{not-json}\n");
    const expected = clone(EXPECTED_IDENTITY);
    expected.receipt.sha256 = sha256(readFileSync(installedReceipt));
    assert.throws(() => validate("codex", baseRun("broad_packet"), expected), /receipt is invalid JSON/u);
  } finally {
    writeFileSync(installedReceipt, originalReceipt);
  }
});

test("packet continuation and selected-context correlation are exact", () => {
  const continuation = baseRun("packet_single_continuation");
  continuation.steps[1].args.option_ids = ["other-gap"];
  assert.throws(() => validate("cursor", continuation), /continuation arguments/u);

  const context = baseRun("ambiguous_symbol_then_context");
  context.steps[1].args.selector.canonical_id = "rust:crate::two::Thing";
  assert.throws(() => validate("codex", context), /selected target/u);
});

function staticIdentityFor(root) {
  const manifest = JSON.parse(readFileSync(join(root, "plugin.json"), "utf8"));
  const pin = JSON.parse(readFileSync(join(root, "cli-version.json"), "utf8"));
  const catalog = JSON.parse(readFileSync(join(root, "generated-mcp-catalog.json"), "utf8"));
  const launcher = readFileSync(join(root, "scripts", "codestory-mcp.cjs"));
  const staticIdentity = clone(EXPECTED_IDENTITY);
  staticIdentity.package.version = manifest.version;
  staticIdentity.cli.version = pin.cli_version;
  staticIdentity.launcher.sha256 = createHash("sha256").update(launcher).digest("hex");
  staticIdentity.publication.schema_version = catalog.wireContract.publicationStampSchemaVersion;
  staticIdentity.protocol.revision = catalog.wireContract.preferredMcpProtocolVersion;
  staticIdentity.protocol.discovery_contract_sha256 =
    catalog.wireContract.discoveryContracts[staticIdentity.protocol.revision];
  const rosterPaths = [
    "plugin.json",
    "cli-version.json",
    "generated-mcp-catalog.json",
    "mcp.json",
    "scripts/codestory-mcp.cjs",
    ".claude-plugin/plugin.json",
    ".github/plugin/plugin.json",
    "hooks/claude-codex-hooks.json",
    "hooks/copilot-hooks.json",
    "skills/codestory-grounding/SKILL.md",
  ];
  staticIdentity.static_roster = Object.fromEntries(
    rosterPaths.map((path) => [path, sha256(readFileSync(join(root, path)))]),
  );
  return staticIdentity;
}

test("static Claude Code and Copilot surfaces bind one package, launcher, hook, and rule core", async () => {
  assert.deepEqual(Object.keys(STATIC_PARITY_HOSTS), ["claude_code", "copilot_cli", "copilot_editor"]);
  const manifest = JSON.parse(await readFile(join(pluginRoot, "plugin.json"), "utf8"));
  const staticIdentity = staticIdentityFor(pluginRoot);

  const report = await validateStaticHostParity(pluginRoot, staticIdentity);
  assert.equal(report.status, "pass");
  assert.deepEqual(report.hosts.map(({ host }) => host), ["claude_code", "copilot_cli", "copilot_editor"]);
  for (const host of report.hosts) {
    assert.equal(host.package_version, manifest.version);
    assert.equal(host.launcher_sha256, staticIdentity.launcher.sha256);
    assert.equal(host.rule_sha256.length, 64);
    assert.equal(host.hook_sha256.length, 64);
    assert.equal(host.model_routing_evaluated, false);
  }
});

test("static parity rejects substituted bytes invalid or no-op hooks metadata drift and heading-only rules", async () => {
  const root = mkdtempSync(join(tmpdir(), "codestory-routing-static-"));
  cpSync(pluginRoot, root, { recursive: true });
  try {
    const launcherIdentity = staticIdentityFor(root);
    writeFileSync(join(root, "scripts", "codestory-mcp.cjs"), "substituted launcher\n");
    await assert.rejects(validateStaticHostParity(root, launcherIdentity), /static digest roster.*codestory-mcp/u);

    cpSync(join(pluginRoot, "scripts", "codestory-mcp.cjs"), join(root, "scripts", "codestory-mcp.cjs"));
    writeFileSync(join(root, "hooks", "copilot-hooks.json"), "{not-json}\n");
    const invalidJson = staticIdentityFor(root);
    await assert.rejects(validateStaticHostParity(root, invalidJson), /hook.*canonical JSON/u);

    writeFileSync(join(root, "hooks", "copilot-hooks.json"), JSON.stringify({
      version: 1,
      hooks: { sessionStart: [{ type: "command", bash: "echo codestory-activate.cjs", powershell: "echo codestory-activate.cjs", timeoutSec: 300 }] },
    }));
    const noOpHook = staticIdentityFor(root);
    await assert.rejects(validateStaticHostParity(root, noOpHook), /hook command is not the canonical launcher/u);

    cpSync(join(pluginRoot, "hooks", "copilot-hooks.json"), join(root, "hooks", "copilot-hooks.json"));
    const copilotMetadataPath = join(root, ".github", "plugin", "plugin.json");
    const metadata = JSON.parse(readFileSync(copilotMetadataPath, "utf8"));
    metadata.hooks = "hooks/other.json";
    writeFileSync(copilotMetadataPath, JSON.stringify(metadata));
    const driftedMetadata = staticIdentityFor(root);
    await assert.rejects(validateStaticHostParity(root, driftedMetadata), /metadata does not bind/u);

    cpSync(join(pluginRoot, ".github", "plugin", "plugin.json"), copilotMetadataPath);
    writeFileSync(join(root, "skills", "codestory-grounding", "SKILL.md"), "---\nname: codestory-grounding\n---\n# CodeStory Grounding\n");
    const headingOnly = staticIdentityFor(root);
    await assert.rejects(validateStaticHostParity(root, headingOnly), /complete canonical grounding contract/u);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
