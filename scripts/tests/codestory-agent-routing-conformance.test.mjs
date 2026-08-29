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
  symlinkSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test, { after } from "node:test";

import {
  INSTALLED_IDENTITY_FIELDS,
  ROUTING_PACKET_QUESTIONS,
  ROUTING_REQUEST_CORPUS,
  ROUTING_SCENARIOS,
  STATIC_PARITY_HOSTS,
  canonicalRequestContractDigest,
  materializeRoutingRequests,
  parseInstalledTranscript,
  validateProofCallInputAgainstCatalog,
  validateRoutingRequestCorpus,
  validateInstalledSession,
  validateStaticHostParity,
} from "../codestory-agent-routing-conformance.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..", "..");
const pluginRoot = join(repoRoot, "plugins", "codestory");
const SHA_D = "d".repeat(64);
const PROFILE_DIGESTS = Object.freeze({
  "2024-11-05": "a".repeat(64),
  "2025-03-26": "b".repeat(64),
  "2025-06-18": "c".repeat(64),
  "2025-11-25": SHA_D,
});
const codexControlRoot = realpathSync(mkdtempSync(join(tmpdir(), "codestory-routing-codex-home-")));
const codexPluginRoot = join(
  codexControlRoot, "plugins", "cache", "RoutingCandidate", "codestory", "0.17.4",
);
const codexSkillPath = join(codexPluginRoot, "skills", "codestory-grounding", "SKILL.md");
const codexGuidancePaths = [
  "skills/codestory-grounding/SKILL.md",
  "skills/codestory-grounding/references/generated-mcp-syntax.md",
  "skills/codestory-grounding/references/search.md",
  "skills/codestory-grounding/references/context.md",
];
for (const path of codexGuidancePaths) {
  const destination = join(codexPluginRoot, path);
  mkdirSync(dirname(destination), { recursive: true });
  copyFileSync(join(pluginRoot, path), destination);
}

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
  protocol: {
    revision: "2025-11-25",
    discovery_contract_sha256: SHA_D,
    discovery_contracts: PROFILE_DIGESTS,
  },
};
const installedReceipt = join(installedRoot, "installed-receipt.json");
writeFileSync(installedReceipt, `${JSON.stringify({ schema_version: 1, identity: installedIdentity }, null, 2)}\n`);
const EXPECTED_IDENTITY = Object.freeze({
  ...cloneForFreeze(installedIdentity),
  receipt: Object.freeze({
    relative_path: "installed-receipt.json",
    sha256: sha256(readFileSync(installedReceipt)),
  }),
  static_roster: Object.freeze({
    ...Object.fromEntries(codexGuidancePaths.map((path) => [
      path,
      sha256(readFileSync(join(codexPluginRoot, path))),
    ])),
  }),
});

function cloneForFreeze(value) {
  if (Array.isArray(value)) return Object.freeze(value.map(cloneForFreeze));
  if (value && typeof value === "object") {
    return Object.freeze(Object.fromEntries(Object.entries(value).map(([key, child]) => [key, cloneForFreeze(child)])));
  }
  return value;
}

after(() => {
  rmSync(installedRoot, { recursive: true, force: true });
  rmSync(codexControlRoot, { recursive: true, force: true });
});

const SCENARIO_IDS = [
  "named_file_direct_read",
  "exact_symbol_search",
  "ambiguous_symbol_then_context",
  "selected_target_context",
  "broad_packet",
  "packet_single_continuation",
  "packet_gap_to_focused_source",
  "packet_named_fallback_to_source",
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
        start_byte: 0,
        end_byte_exclusive: Buffer.byteLength(sourceText),
        quote: sourceText,
        classification: { kind: "resolved_material", fields },
      },
    ],
    spec: {
      start: { kind: "canonical_id", canonical_id: "rust:crate::start" },
      steps: [{ target: { kind: "canonical_id", canonical_id: "rust:crate::finish" } }],
      prohibit_traversal_through: prohibited
        ? [{ kind: "canonical_id", canonical_id: "rust:crate::blocked" }]
        : [],
      exclude_from_projection: [],
    },
  };
}

function projectedProofClauses(contract) {
  return contract.clauses.map((clause) => ({
    start: clause.start_byte,
    end: clause.end_byte_exclusive,
    clause_id: clause.clause_id,
    quote: clause.quote,
    classification: clause.classification.kind,
    fields: clause.classification.kind === "resolved_material" ? clone(clause.classification.fields) : [],
    reason: clause.classification.kind === "unresolved_material" ? clause.classification.reason : null,
    non_material_kind: clause.classification.kind === "non_material" ? clause.classification.reason : null,
  }));
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

function v3Publication() {
  return {
    core: { project_id: "project-1", generation_id: "core-1", run_id: "run-1" },
    retrieval: {
      core_generation_id: "core-1",
      core_run_id: "run-1",
      retrieval_generation: "retrieval-1",
      retrieval_input_sha256: "e".repeat(64),
      semantic_generation: "semantic-1",
    },
  };
}

function v3Identity(packetId = "packet-1") {
  return {
    packet_id: packetId,
    request_id: `request-${packetId}`,
    question_sha256: "f".repeat(64),
  };
}

function v3Gap(gapId, kind = "evidence_missing", message = null) {
  return { identity: { gap_id: gapId }, kind, message };
}

function v3SearchEvidence(evidenceId, path, symbolId) {
  return {
    identity: { evidence_id: evidenceId },
    path,
    symbol_id: symbolId,
    start_line: 1,
    end_line: 1,
    excerpt: "fixture evidence",
  };
}

function v3ContextEvidence(evidenceId, path, symbolId) {
  return v3SearchEvidence(evidenceId, path, symbolId);
}

function v3PacketEvidence(evidenceId, path, symbolId = null) {
  return {
    identity: { evidence_id: evidenceId },
    kind: "exact_source",
    path,
    symbol_id: symbolId,
    start_line: 1,
    end_line: 1,
    summary: "fixture evidence",
  };
}

function v3Search({ evidence, gaps = [], status = "available" }) {
  return {
    kind: "complete",
    schema_version: 3,
    identity: v3Identity("search-1"),
    publication: v3Publication(),
    status,
    evidence,
    gaps,
    continuation: null,
    retrieval: { state: "full", generation_id: "retrieval-1" },
    diagnostics: { availability: "unavailable" },
  };
}

function v3Context({ target, evidence, gaps = [], status = "available" }) {
  return {
    kind: "complete",
    schema_version: 3,
    identity: v3Identity("context-1"),
    publication: v3Publication(),
    status,
    target,
    evidence,
    gaps,
    continuation: null,
    diagnostics: { availability: "unavailable" },
  };
}

function v3Packet({
  packetId = "packet-1",
  evidence = [],
  gaps = [],
  status = "available",
  continuation = null,
  retrievalState = "full",
}) {
  return {
    kind: "complete",
    schema_version: 3,
    identity: v3Identity(packetId),
    publication: v3Publication(),
    status,
    retrieval: { state: retrievalState, generation_id: retrievalState === "unavailable" ? null : "retrieval-1" },
    evidence,
    gaps,
    continuation,
    diagnostics: { availability: "unavailable" },
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
    clauses: projectedProofClauses(contract),
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
      project_root: "/workspace/repo",
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
      run.steps = [mcp("search", { project: "/workspace/repo", query: "start" }, v3Search({
        evidence: [v3SearchEvidence("lead-1", "src/lib.rs", "rust:crate::start")],
      }))];
      run.final = finalClaim({
        authority: "search_lead",
        outcome: "discovery_only",
        target_id: "rust:crate::start",
        evidence_ids: ["lead-1"],
      });
      break;
    case "ambiguous_symbol_then_context":
      run.request.selected_target = "rust:crate::one::Thing";
      run.steps = [
        mcp("search", { project: "/workspace/repo", query: "Thing" }, {
          ...v3Search({
            evidence: [
              v3SearchEvidence("lead-1", "src/one.rs", "rust:crate::one::Thing"),
              v3SearchEvidence("lead-2", "src/two.rs", "rust:crate::two::Thing"),
            ],
            gaps: [v3Gap("selector_ambiguous")],
          }),
        }),
        mcp("context", {
          project: "/workspace/repo",
          id: "rust:crate::one::Thing",
        }, v3Context({
          target: { path: "src/one.rs", symbol_id: "rust:crate::one::Thing" },
          evidence: [v3ContextEvidence("context-1", "src/one.rs", "rust:crate::one::Thing")],
        })),
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
        id: "rust:crate::ExactThing",
      }, v3Context({
        target: { path: "src/lib.rs", symbol_id: "rust:crate::ExactThing" },
        evidence: [v3ContextEvidence("context-1", "src/lib.rs", "rust:crate::ExactThing")],
      }))];
      run.final = finalClaim({
        authority: "context_evidence",
        target_id: "rust:crate::ExactThing",
        evidence_ids: ["context-1"],
      });
      break;
    case "broad_packet":
      run.steps = [mcp("packet", { project: "/workspace/repo", question: ROUTING_PACKET_QUESTIONS.broad_packet }, v3Packet({
        evidence: [v3PacketEvidence("evidence-1", "src/flow.rs")],
      }))];
      run.final = finalClaim({ authority: "packet_evidence", evidence_ids: ["evidence-1"] });
      break;
    case "packet_single_continuation":
      run.request.named_files = ["src/unread.rs"];
      run.steps = [
        mcp("packet", { project: "/workspace/repo", question: ROUTING_PACKET_QUESTIONS.packet_single_continuation }, v3Packet({
          status: "continuation_available",
          continuation: {
            continuation_id: "continuation-1",
            remaining_rounds: 1,
            gap_ids: [{ gap_id: "gap-1" }],
          },
          evidence: [v3PacketEvidence("evidence-1", "src/flow.rs")],
          gaps: [v3Gap("gap-1", "continuation_required")],
        })),
        mcp("packet", {
          project: "/workspace/repo",
          question: ROUTING_PACKET_QUESTIONS.packet_single_continuation,
          parent_packet_id: "continuation-1",
          option_ids: ["gap-1"],
          core_generation_id: "core-1",
          retrieval_generation: "retrieval-1",
        }, v3Packet({
          packetId: "packet-2",
          evidence: [v3PacketEvidence("evidence-2", "src/more.rs")],
        })),
      ];
      run.final = finalClaim({
        authority: "packet_evidence",
        evidence_ids: ["evidence-1", "evidence-2"],
      });
      break;
    case "packet_gap_to_focused_source":
      run.request.gap_source_paths = ["src/gap.rs"];
      run.steps = [mcp("packet", { project: "/workspace/repo", question: ROUTING_PACKET_QUESTIONS.packet_gap_to_focused_source }, v3Packet({
          evidence: [v3PacketEvidence("evidence-1", "src/catalog.rs")],
          status: "no_useful_evidence",
          gaps: [v3Gap("gap-1", "evidence_missing", "The missing route remains unresolved.")],
        }))];
      run.final = finalClaim({
        authority: "packet_evidence",
        outcome: "unknown",
        evidence_ids: ["evidence-1"],
        gap_ids: ["gap-1"],
      });
      break;
    case "packet_named_fallback_to_source":
      run.request.named_files = ["src/fallback.rs"];
      run.steps = [
        mcp("packet", {
          project: "/workspace/repo",
          question: ROUTING_PACKET_QUESTIONS.packet_named_fallback_to_source,
        }, v3Packet({
          evidence: [v3PacketEvidence("evidence-1", "src/catalog.rs")],
          gaps: [v3Gap("gap-1", "evidence_missing", "The named fallback remains unresolved.")],
        })),
        read("src/fallback.rs"),
      ];
      run.final = finalClaim({
        authority: "source",
        evidence_ids: ["evidence-1", "source:src/fallback.rs"],
        gap_ids: ["gap-1"],
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
      run.final = finalClaim({ authority: "typed_proof", outcome: "unknown", reason_codes: ["selector_missing"], proof_disposition: "unknown" });
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
      const malformed = { ...typed, source_text: "A calls B", clauses: [] };
      run.request.proof_contract = malformed;
      run.steps = [mcp("prove_call_path", { project: "/workspace/repo", ...malformed }, {
        code: "invalid_proof_interpretation",
        message: "source text is unclassified",
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
        reason_codes: ["direct_call_missing"],
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
      events.push({
        type: "item.completed",
        item: step.hostFailed
          ? { ...item, status: "failed", result: step.result }
          : step.failed
          ? { ...item, status: "failed", error: { message: "user cancelled MCP tool call" } }
          : { ...item, status: "completed", result: step.result },
      });
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
      const bare = `sed -n '1,120p' ${step.wrapped ? step.path : `'${step.path}'`}`;
      const item = {
        id,
        type: "command_execution",
        command: step.wrapped ? `/bin/zsh -lc ${JSON.stringify(bare)}` : bare,
      };
      events.push({ type: "item.started", item });
      events.push({
        type: "item.completed",
        item: step.failed
          ? { ...item, status: "failed", exit_code: 1, aggregated_output: "source fixture unavailable\n" }
          : { ...item, status: "completed", exit_code: 0, aggregated_output: "source fixture\n" },
      });
    } else if (step.kind === "host_guidance_read") {
      const item = {
        id,
        type: "command_execution",
        command: `/bin/zsh -lc ${JSON.stringify(`sed -n '1,240p' ${step.path}`)}`,
      };
      events.push({ type: "item.started", item });
      events.push({
        type: "item.completed",
        item: {
          ...item,
          status: "completed",
          exit_code: 0,
          aggregated_output: step.content ?? readFileSync(step.path, "utf8"),
        },
      });
    } else if (step.kind === "shell") {
      const item = { id, type: "command_execution", command: step.command };
      events.push({ type: "item.started", item });
      events.push({
        type: "item.completed",
        item: { ...item, status: "completed", exit_code: 0, aggregated_output: step.output ?? "" },
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
        completed: step.failed ? {
          readToolCall: {
            result: { error: { errorMessage: "File not found" } },
          },
        } : {
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
    installedPluginRoot: host === "codex" ? codexPluginRoot : null,
    transcript: transcript(host, run),
  });
}

test("Codex excludes only roster-authenticated installed guidance reads around product routing", () => {
  const run = baseRun("named_file_direct_read");
  run.steps[0].wrapped = true;
  run.steps.unshift({ kind: "host_guidance_read", path: codexSkillPath });
  assert.deepEqual(validate("codex", run).actions, ["source_read"]);

  const linkedGuidance = baseRun("exact_symbol_search");
  const linkedPaths = codexGuidancePaths.slice(1).map((path) => join(codexPluginRoot, path));
  linkedGuidance.steps.unshift(
    { kind: "host_guidance_read", path: codexSkillPath },
    {
      kind: "shell",
      command: `/bin/zsh -lc ${JSON.stringify(linkedPaths.map((path) => `sed -n '1,260p' ${path}`).join(" && "))}`,
      output: linkedPaths.map((path) => readFileSync(path, "utf8")).join(""),
    },
  );
  assert.deepEqual(validate("codex", linkedGuidance).actions, ["search"]);

  const newlineGuidance = baseRun("exact_symbol_search");
  newlineGuidance.steps.unshift({
    kind: "shell",
    command: `/bin/zsh -lc ${JSON.stringify(linkedPaths.map((path) => `sed -n '1,260p' ${path}`).join("\n"))}`,
    output: linkedPaths.map((path) => readFileSync(path, "utf8")).join(""),
  });
  assert.deepEqual(validate("codex", newlineGuidance).actions, ["search"]);

  const nativeNewlineGuidance = baseRun("exact_symbol_search");
  nativeNewlineGuidance.steps.unshift({
    kind: "shell",
    command: `/bin/zsh -lc "${linkedPaths.map((path) => `sed -n '1,260p' ${path}`).join("\n")}"`,
    output: linkedPaths.map((path) => readFileSync(path, "utf8")).join(""),
  });
  assert.deepEqual(validate("codex", nativeNewlineGuidance).actions, ["search"]);

  const concurrentGuidanceEvents = codexJsonl(linkedGuidance)
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));
  [concurrentGuidanceEvents[2], concurrentGuidanceEvents[3]] = [
    concurrentGuidanceEvents[3], concurrentGuidanceEvents[2],
  ];
  assert.equal(validateInstalledSession({
    host: "codex",
    scenarioId: linkedGuidance.scenario_id,
    request: linkedGuidance.request,
    installedRoot,
    installedReceipt,
    expectedIdentity: EXPECTED_IDENTITY,
    installedPluginRoot: codexPluginRoot,
    transcript: `${concurrentGuidanceEvents.map(JSON.stringify).join("\n")}\n`,
  }).status, "pass");

  const semicolonGuidance = baseRun("exact_symbol_search");
  semicolonGuidance.steps.unshift({
    kind: "shell",
    command: `/bin/zsh -lc ${JSON.stringify(linkedPaths.map((path) => `sed -n '1,260p' ${path}`).join("; "))}`,
    output: linkedPaths.map((path) => readFileSync(path, "utf8")).join(""),
  });
  assert.deepEqual(validate("codex", semicolonGuidance).actions, ["search"]);

  const relativeGuidance = baseRun("exact_symbol_search");
  const relativeContextPath = join(codexPluginRoot, "skills/codestory-grounding/references/context.md");
  relativeGuidance.steps.unshift(
    { kind: "host_guidance_read", path: codexSkillPath },
    {
      kind: "shell",
      command: `/bin/zsh -lc ${JSON.stringify("sed -n '1,260p' references/context.md")}`,
      output: readFileSync(relativeContextPath, "utf8"),
    },
  );
  assert.deepEqual(validate("codex", relativeGuidance).actions, ["search"]);

  const tamperedRelativeGuidance = clone(relativeGuidance);
  tamperedRelativeGuidance.steps[1].output += "tampered\n";
  assert.throws(
    () => validate("codex", tamperedRelativeGuidance),
    /required action sequence|forbidden tool/u,
  );

  const countedGuidance = baseRun("exact_symbol_search");
  const countedPaths = codexGuidancePaths.slice(1, 3).map((path) => join(codexPluginRoot, path));
  countedGuidance.steps.unshift(
    { kind: "host_guidance_read", path: codexSkillPath },
    ...countedPaths.map((path) => ({
      kind: "shell",
      command: `/bin/zsh -lc ${JSON.stringify(`wc -l ${JSON.stringify(path)} && sed -n '1,260p' ${JSON.stringify(path)}`)}`,
      output: `${readFileSync(path, "utf8").match(/\n/gu)?.length ?? 0} ${path}\n${readFileSync(path, "utf8")}`,
    })),
  );
  assert.deepEqual(validate("codex", countedGuidance).actions, ["search"]);

  const groupedGuidance = baseRun("exact_symbol_search");
  const groupedCounts = countedPaths.map((path) => readFileSync(path, "utf8").match(/\n/gu)?.length ?? 0);
  groupedGuidance.steps.unshift({
    kind: "shell",
    command: `/bin/zsh -lc ${JSON.stringify(`wc -l ${countedPaths.map((path) => JSON.stringify(path)).join(" ")} && ${countedPaths.map((path) => `sed -n '1,260p' ${JSON.stringify(path)}`).join(" && ")}`)}`,
    output: `${groupedCounts.map((count, index) => `${String(count).padStart(8)} ${countedPaths[index]}\n`).join("")}${String(groupedCounts.reduce((sum, count) => sum + count, 0)).padStart(8)} total\n${countedPaths.map((path) => readFileSync(path, "utf8")).join("")}`,
  });
  assert.deepEqual(validate("codex", groupedGuidance).actions, ["search"]);

  const directGuidance = baseRun("exact_symbol_search");
  directGuidance.steps.unshift({
    kind: "shell",
    command: `/bin/zsh -lc ${JSON.stringify(`cat ${JSON.stringify(countedPaths[0])}`)}`,
    output: readFileSync(countedPaths[0], "utf8"),
  });
  assert.deepEqual(validate("codex", directGuidance).actions, ["search"]);

  const lateGuidance = baseRun("exact_symbol_search");
  lateGuidance.steps.push({
    kind: "host_guidance_read",
    path: codexSkillPath,
  });
  assert.deepEqual(validate("codex", lateGuidance).actions, ["search"]);

  const overlappingLateEvents = codexJsonl(lateGuidance)
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));
  [overlappingLateEvents[2], overlappingLateEvents[3]] = [
    overlappingLateEvents[3], overlappingLateEvents[2],
  ];
  assert.throws(
    () => validateInstalledSession({
      host: "codex",
      scenarioId: lateGuidance.scenario_id,
      request: lateGuidance.request,
      installedRoot,
      installedReceipt,
      expectedIdentity: EXPECTED_IDENTITY,
      installedPluginRoot: codexPluginRoot,
      transcript: `${overlappingLateEvents.map(JSON.stringify).join("\n")}\n`,
    }),
    /overlaps authenticated installed guidance with a product action/u,
  );

  const multiCatGuidance = baseRun("exact_symbol_search");
  multiCatGuidance.steps.unshift({
    kind: "shell",
    command: `/bin/zsh -lc ${JSON.stringify(`cat ${linkedPaths.map((path) => JSON.stringify(path)).join(" ")}`)}`,
    output: linkedPaths.map((path) => readFileSync(path, "utf8")).join(""),
  });
  assert.deepEqual(validate("codex", multiCatGuidance).actions, ["search"]);

  const countedRead = baseRun("named_file_direct_read");
  countedRead.steps = [{
    kind: "shell",
    command: `/bin/zsh -lc ${JSON.stringify("wc -l src/named.rs && sed -n '1,240p' src/named.rs")}`,
    output: "1 src/named.rs\nsource fixture\n",
  }];
  assert.deepEqual(validate("codex", countedRead).actions, ["source_read"]);

  const guidanceThenSource = baseRun("named_file_direct_read");
  guidanceThenSource.steps = [{
    kind: "shell",
    command: `/bin/zsh -lc ${JSON.stringify(`sed -n '1,240p' ${codexSkillPath} && sed -n '1,240p' src/named.rs`)}`,
    output: `${readFileSync(codexSkillPath, "utf8")}source fixture\n`,
  }];
  assert.deepEqual(validate("codex", guidanceThenSource).actions, ["source_read"]);

  const tamperedGuidanceThenSource = clone(guidanceThenSource);
  tamperedGuidanceThenSource.steps[0].output = `tampered\n${tamperedGuidanceThenSource.steps[0].output}`;
  assert.throws(
    () => validate("codex", tamperedGuidanceThenSource),
    /required action sequence|forbidden tool/u,
  );

  const shellConcatenatedRange = baseRun("named_file_direct_read");
  shellConcatenatedRange.steps = [{
    kind: "shell",
    command: "/bin/zsh -lc \"sed -n '1,\"'$p'\"' src/named.rs\"",
    output: "source fixture\n",
  }];
  assert.deepEqual(validate("codex", shellConcatenatedRange).actions, ["source_read"]);

  const singleQuotedWrapper = baseRun("named_file_direct_read");
  singleQuotedWrapper.steps = [{
    kind: "shell",
    command: "/bin/zsh -lc 'nl -ba src/named.rs'",
    output: "     1\tsource fixture\n",
  }];
  assert.deepEqual(validate("codex", singleQuotedWrapper).actions, ["source_read"]);

  const tampered = baseRun("named_file_direct_read");
  tampered.steps.unshift({
    kind: "host_guidance_read",
    path: codexSkillPath,
    content: `${readFileSync(codexSkillPath, "utf8")}tampered\n`,
  });
  assert.throws(() => validate("codex", tampered), /required action sequence|forbidden tool/u);

  const wrongDigest = clone(EXPECTED_IDENTITY);
  wrongDigest.static_roster["skills/codestory-grounding/SKILL.md"] = "f".repeat(64);
  const wrongDigestRun = baseRun("named_file_direct_read");
  wrongDigestRun.steps.unshift({ kind: "host_guidance_read", path: codexSkillPath });
  assert.throws(
    () => validate("codex", wrongDigestRun, wrongDigest),
    /required action sequence|forbidden tool/u,
  );

  const zeroDigest = clone(EXPECTED_IDENTITY);
  zeroDigest.static_roster["skills/codestory-grounding/SKILL.md"] = "0".repeat(64);
  assert.throws(
    () => validate("codex", wrongDigestRun, zeroDigest),
    /required action sequence|forbidden tool/u,
  );

  const originalSkill = readFileSync(codexSkillPath);
  try {
    writeFileSync(codexSkillPath, Buffer.concat([originalSkill, Buffer.from("tampered\n")]));
    const changedInstalledBytes = baseRun("named_file_direct_read");
    changedInstalledBytes.steps.unshift({ kind: "host_guidance_read", path: codexSkillPath });
    assert.throws(
      () => validate("codex", changedInstalledBytes),
      /required action sequence|forbidden tool/u,
    );
  } finally {
    writeFileSync(codexSkillPath, originalSkill);
  }

  const symlinkTarget = join(codexControlRoot, "outside-skill.md");
  writeFileSync(symlinkTarget, originalSkill);
  try {
    unlinkSync(codexSkillPath);
    symlinkSync(symlinkTarget, codexSkillPath);
    const symlinked = baseRun("named_file_direct_read");
    symlinked.steps.unshift({ kind: "host_guidance_read", path: codexSkillPath });
    assert.throws(() => validate("codex", symlinked), /required action sequence|forbidden tool/u);
  } finally {
    unlinkSync(codexSkillPath);
    writeFileSync(codexSkillPath, originalSkill);
  }

  const escaped = baseRun("named_file_direct_read");
  escaped.steps.unshift({
    kind: "host_guidance_read",
    path: join(pluginRoot, "skills", "codestory-grounding", "SKILL.md"),
  });
  assert.throws(() => validate("codex", escaped), /required action sequence|forbidden tool/u);

  const duplicate = baseRun("named_file_direct_read");
  duplicate.steps.unshift(
    { kind: "host_guidance_read", path: codexSkillPath },
    { kind: "host_guidance_read", path: codexSkillPath },
  );
  assert.deepEqual(validate("codex", duplicate).actions, ["source_read"]);

  const late = baseRun("named_file_direct_read");
  late.steps.push({ kind: "host_guidance_read", path: codexSkillPath });
  assert.deepEqual(validate("codex", late).actions, ["source_read"]);

  const arbitraryShell = baseRun("named_file_direct_read");
  arbitraryShell.steps.unshift({ kind: "shell", command: "/bin/zsh -lc \"pwd\"" });
  assert.throws(() => validate("codex", arbitraryShell), /required action sequence|forbidden tool/u);

  const unrosteredPath = join(codexPluginRoot, "skills", "codestory-grounding", "references", "unrostered.md");
  writeFileSync(unrosteredPath, "unrostered guidance\n");
  const unrostered = baseRun("exact_symbol_search");
  unrostered.steps.unshift({ kind: "host_guidance_read", path: unrosteredPath });
  assert.throws(() => validate("codex", unrostered), /required action sequence|forbidden tool/u);

  const nonguidancePath = join(codexPluginRoot, "plugin.json");
  writeFileSync(nonguidancePath, "{}\n");
  const nonguidanceIdentity = clone(EXPECTED_IDENTITY);
  nonguidanceIdentity.static_roster["plugin.json"] = sha256(readFileSync(nonguidancePath));
  const nonguidance = baseRun("exact_symbol_search");
  nonguidance.steps.unshift({ kind: "host_guidance_read", path: nonguidancePath });
  assert.throws(
    () => validate("codex", nonguidance, nonguidanceIdentity),
    /required action sequence|forbidden tool/u,
  );

  for (const command of [
    "/bin/zsh -lc \"cat src/lib.rs | head\"",
    "/bin/zsh -lc \"cat src/lib.rs > /tmp/out\"",
    "/bin/zsh -lc \"cat src/lib.rs; pwd\"",
    "/bin/zsh -lc \"cat $(pwd)/src/lib.rs\"",
    "/bin/zsh -lc \"cat `pwd`/src/lib.rs\"",
    "/bin/zsh -lc \"cat src/*.rs\"",
    "/bin/zsh -lc \"cat src/lib.rs src/one.rs\"",
    "/bin/zsh -lc \"wc -l src/lib.rs && sed -n '1,240p' src/one.rs\"",
    "/bin/zsh -lc \"wc -l src/lib.rs; sed -n '1,240p' src/lib.rs\"",
    "/bin/zsh -lc \"sed -n '1,\"'$p'\"' src/lib.rs; pwd\"",
    "/bin/zsh -lc \"cat ~/secret\"",
    "/bin/zsh -lc \"cat =ls\"",
  ]) {
    const hostile = baseRun("named_file_direct_read");
    hostile.steps.unshift({ kind: "shell", command });
    assert.throws(() => validate("codex", hostile), /required action sequence|forbidden tool/u, command);
  }
});

test("Codex reports a failed expected MCP call before rejecting retry and shell fallback", () => {
  const failed = baseRun("exact_symbol_search");
  failed.steps[0].failed = true;
  failed.steps.push(clone(failed.steps[0]), {
    kind: "shell",
    command: "/bin/zsh -lc \"rg -n start src\"",
  });
  assert.throws(() => validate("codex", failed), /unexpected failed search action/u);

  const completed = baseRun("exact_symbol_search");
  completed.steps.push(clone(completed.steps[0]), {
    kind: "shell",
    command: "/bin/zsh -lc \"rg -n start src\"",
  });
  assert.throws(() => validate("codex", completed), /required action sequence/u);
});

test("freezes exactly the sixteen accepted routing scenarios", () => {
  assert.deepEqual(ROUTING_SCENARIOS.map(({ id }) => id), SCENARIO_IDS);
  assert.equal(new Set(ROUTING_SCENARIOS.map(({ id }) => id)).size, 16);
  for (const scenario of ROUTING_SCENARIOS) {
    assert.equal(typeof scenario.expected_first_tool, "string", scenario.id);
    assert.ok(Array.isArray(scenario.required_action_sequence), scenario.id);
    assert.ok(Array.isArray(scenario.optional_prefixes), scenario.id);
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

test("checked-in request corpus covers the routing matrix exactly once", () => {
  assert.equal(validateRoutingRequestCorpus(), true);
  assert.deepEqual(
    ROUTING_REQUEST_CORPUS.scenarios.map(({ id }) => id),
    SCENARIO_IDS,
  );
  assert.equal(Object.isFrozen(ROUTING_REQUEST_CORPUS), true);
  for (const entry of ROUTING_REQUEST_CORPUS.scenarios) {
    assert.doesNotMatch(entry.prompt, /\b(search|context|packet|prove_call_path)\b/iu, entry.id);
  }
});

test("installed-host prompts close the final claim vocabulary and direct-read identity contract", () => {
  const materialized = materializeRoutingRequests(repoRoot);
  const prompts = materialized.map(({ request }) => request.text);
  assert.equal(prompts.length, SCENARIO_IDS.length);
  for (const prompt of prompts) {
    assert.match(prompt, /authority must be exactly one of source, search_lead, context_evidence, packet_evidence, typed_proof, none/u);
    assert.match(prompt, /only one raw JSON object and no markdown fence, explanation, prefix, or suffix/u);
    assert.match(prompt, /outcome must be exactly one of supported, discovery_only, refuted, unknown, unavailable, invalid_contract, refused/u);
    assert.match(prompt, /For a direct source read, record evidence identity source:<project-relative-path>/u);
    assert.match(prompt, /authorized fallback read changes evidence authority but preserves an earlier unavailable outcome/u);
    assert.match(prompt, /rejected typed interpretation.*authority none.*outcome invalid_contract.*no proof disposition/u);
    assert.match(prompt, /human-readable validation text.*reason_codes empty.*never derive a code/u);
    assert.match(prompt, /Use refused only when the user requested exact proof without supplying a typed interpretation/u);
    assert.match(prompt, /in that case call no product tool and do not substitute retrieval or source evidence/u);
    assert.match(prompt, /target_id must be null unless a CodeStory tool result returned a target identity/u);
    assert.match(prompt, /reason_codes may contain only CodeStory tool result codes or typed_contract_required/u);
    assert.match(prompt, /refutation_basis must be null unless a ContractRefuted result supplied the basis/u);
    assert.match(prompt, /runtime_execution_claim and absence_claim must each be false/u);
    assert.match(prompt, /material_omissions contains only unresolved material requested by the user/u);
    assert.match(prompt, /typed proof.*receipt_id.*never copy fact_id or edge_id/u);
    assert.match(prompt, /refutation_basis.*refutation\.kind string.*never the whole refutation object/u);
    assert.match(prompt, /typed proof gap has no gap_id.*disposition\.gaps\[\]\.kind.*reason_codes/u);
    assert.match(prompt, /diagnostics\.availability.*optional diagnostics artifact.*never copy it into outcome or reason_codes/u);
    assert.match(prompt, /never grep, rg, search, or probe the installed plugin package/u);
  }
  const discoveryPrompt = materialized.find(({ scenario_id }) => scenario_id === "exact_symbol_search").request.text;
  assert.match(discoveryPrompt, /discovery candidates only/iu);
  assert.match(discoveryPrompt, /pass `start` unchanged as the query/iu);
  assert.match(discoveryPrompt, /do not select or verify one in this turn/iu);
  const ambiguousPrompt = materialized.find(({ scenario_id }) => scenario_id === "ambiguous_symbol_then_context").request.text;
  assert.match(ambiguousPrompt, /candidate list for Thing first/iu);
  assert.match(ambiguousPrompt, /returned identity whose path is src\/one\.rs/iu);
  assert.match(ambiguousPrompt, /do not combine the name and path into a free-text target/iu);
  const selectedPrompt = materialized.find(({ scenario_id }) => scenario_id === "selected_target_context").request.text;
  assert.match(selectedPrompt, /already selected exact symbol dynamic_start/iu);
  assert.match(selectedPrompt, /use that exact selector without discovering or broadening/iu);
  const refusalPrompt = materialized.find(({ scenario_id }) => scenario_id === "refuse_free_english_proof").request.text;
  assert.match(refusalPrompt, /do not call any repository tool as a substitute.*refuse the proof request/iu);
  for (const scenarioId of [
    "packet_single_continuation",
    "packet_gap_to_focused_source",
    "packet_named_fallback_to_source",
  ]) {
    const prompt = materialized.find(({ scenario_id }) => scenario_id === scenarioId).request.text;
    assert.match(prompt, /fallback-only.*initial broad request.*do not add probes or continuation pins/isu, scenarioId);
  }
});

test("terminal routing scenarios reject every unauthorized source upgrade", () => {
  const authorized = new Set([
    "named_file_direct_read",
    "packet_single_continuation",
    "packet_gap_to_focused_source",
    "packet_named_fallback_to_source",
  ]);
  for (const scenarioId of SCENARIO_IDS.filter((id) => !authorized.has(id))) {
    const run = baseRun(scenarioId);
    run.steps.push({ kind: "source_read", path: "src/lib.rs" });
    assert.throws(
      () => validate("codex", run),
      /required action sequence|forbidden tool|source read is not authorized/u,
      scenarioId,
    );
  }
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

const PROOF_CALL_SCENARIOS = [
  "typed_proof_contract_proven",
  "typed_proof_contract_refuted",
  "typed_proof_unknown",
  "typed_proof_unavailable",
  "malformed_proof_contract",
  "proof_observational",
  "hidden_proof_tool_discovery",
];

test("all proof scenarios preserve the public input DTO through both installed-host parsers", () => {
  for (const scenarioId of PROOF_CALL_SCENARIOS) {
    const run = baseRun(scenarioId);
    const expected = run.request.proof_contract;
    const proofStep = run.steps.find((step) => step.tool === "prove_call_path");
    assert.deepEqual(
      { source_text: proofStep.args.source_text, clauses: proofStep.args.clauses, spec: proofStep.args.spec },
      expected,
      scenarioId,
    );
    assert.equal(validateProofCallInputAgainstCatalog(proofStep.args), true, scenarioId);
    for (const host of ["codex", "cursor"]) {
      assert.equal(validate(host, run).status, "pass", `${host}:${scenarioId}`);
    }
  }
});

test("hidden proof discovery is optional only when the verifier is directly visible", () => {
  for (const host of ["codex", "cursor"]) {
    const visible = baseRun("hidden_proof_tool_discovery");
    visible.steps.shift();
    assert.deepEqual(validate(host, visible).actions, ["prove_call_path"]);

    const lateDiscovery = baseRun("hidden_proof_tool_discovery");
    lateDiscovery.steps.reverse();
    assert.throws(
      () => validate(host, lateDiscovery),
      /required action sequence|follow-up tool_search is not permitted/u,
    );
  }
});

test("the old normalized proof-response projection is rejected as public tool input", () => {
  const input = { project: "/workspace/repo", ...proofContract() };
  input.clauses = input.clauses.map((clause) => ({
    start: clause.start_byte,
    end: clause.end_byte_exclusive,
    clause_id: clause.clause_id,
    quote: clause.quote,
    classification: clause.classification.kind,
    fields: clause.classification.fields,
    reason: null,
    non_material_kind: null,
  }));
  input.spec.steps = input.spec.steps.map((step) => ({ relation: "direct_outgoing_call", ...step }));
  assert.throws(
    () => validateProofCallInputAgainstCatalog(input),
    /prove_call_path input schema/u,
  );
});

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

  const overlapping = codexJsonl(baseRun("packet_single_continuation"))
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));
  [overlapping[2], overlapping[3]] = [overlapping[3], overlapping[2]];
  const overlappingTranscript = `${overlapping.map(JSON.stringify).join("\n")}\n`;
  assert.equal(parseInstalledTranscript("codex", overlappingTranscript).actions.length, 2);
  const overlappingRun = baseRun("packet_single_continuation");
  assert.throws(() => validateInstalledSession({
    host: "codex",
    scenarioId: overlappingRun.scenario_id,
    request: overlappingRun.request,
    installedRoot,
    installedReceipt,
    expectedIdentity: EXPECTED_IDENTITY,
    installedPluginRoot: codexPluginRoot,
    transcript: overlappingTranscript,
  }), /overlapping product actions/u);
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

  const composer = clone(CURSOR_CAPTURED_DIRECT_READ);
  composer.splice(2, 0,
    { type: "thinking", subtype: "delta", text: "Reading the named file", session_id: "captured-1", timestamp_ms: 1 },
    { type: "thinking", subtype: "completed", session_id: "captured-1", timestamp_ms: 2 },
  );
  const firstAssistant = composer.findIndex((event) => event.type === "assistant");
  const finalText = composer.at(-1).result;
  composer.splice(firstAssistant, 2,
    { type: "assistant", message: { role: "assistant", content: [{ type: "text", text: finalText.slice(0, 24) }] }, session_id: "captured-1", timestamp_ms: 3 },
    { type: "assistant", message: { role: "assistant", content: [{ type: "text", text: finalText.slice(24) }] }, session_id: "captured-1", timestamp_ms: 4 },
    { type: "assistant", message: { role: "assistant", content: [{ type: "text", text: finalText }] }, session_id: "captured-1", model_call_id: "composer-call", timestamp_ms: 5 },
  );
  assert.equal(parseInstalledTranscript("cursor", capturedJsonl(composer)).final, finalText);

  const multiTurnComposer = clone(composer);
  const toolStart = multiTurnComposer.findIndex((event) => event.type === "tool_call" && event.subtype === "started");
  const preamble = "Reading the named file.\n";
  multiTurnComposer.splice(toolStart, 0,
    { type: "assistant", message: { role: "assistant", content: [{ type: "text", text: preamble }] }, session_id: "captured-1", timestamp_ms: 2.1 },
    { type: "assistant", message: { role: "assistant", content: [{ type: "text", text: preamble }] }, session_id: "captured-1", model_call_id: "composer-preamble", timestamp_ms: 2.2 },
  );
  multiTurnComposer.at(-1).result = `${preamble}${finalText}`;
  assert.equal(parseInstalledTranscript("cursor", capturedJsonl(multiTurnComposer)).final, finalText);

  const invalidThinking = clone(composer);
  invalidThinking[2].subtype = "opaque";
  assert.throws(
    () => parseInstalledTranscript("cursor", capturedJsonl(invalidThinking)),
    /unsupported Cursor thinking event/u,
  );

  const mismatchedSnapshot = clone(composer);
  mismatchedSnapshot.find((event) => event.model_call_id).message.content[0].text = "different";
  assert.throws(
    () => parseInstalledTranscript("cursor", capturedJsonl(mismatchedSnapshot)),
    /assistant snapshot does not match streamed deltas/u,
  );

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

  const absoluteRead = baseRun("named_file_direct_read");
  absoluteRead.steps[0].path = "/workspace/repo/src/named.rs";
  assert.equal(validate("cursor", absoluteRead).status, "pass");

  const escapedRead = baseRun("named_file_direct_read");
  escapedRead.steps[0].path = "/workspace/other/named.rs";
  assert.throws(() => validate("cursor", escapedRead), /escapes the declared project root/u);
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
      run.steps.push({ kind: "source_read", path: "src/unrelated.rs" });
    },
    error: /source read is not authorized/u,
  },
  {
    name: "gap path prefix does not authorize a different file",
    scenario: "packet_gap_to_focused_source",
    mutate(run) {
      run.request.gap_source_paths = ["src/gap.rs.bak"];
      run.steps.push({ kind: "source_read", path: "src/gap.rs.bak" });
    },
    error: /source read is not correlated with the packet evidence gap/u,
  },
  {
    name: "material gap citation does not authorize a different file",
    scenario: "packet_gap_to_focused_source",
    mutate(run) {
      mutateBody(run, 0, (body) => {
        body.gaps[0].message = "Missing evidence for src/other.rs, cited src/gap.rs";
      });
      run.steps.push({ kind: "source_read", path: "src/gap.rs" });
    },
    error: /source read is not correlated with the packet evidence gap/u,
  },
  {
    name: "named packet fallback does not authorize an unnamed source read",
    scenario: "packet_named_fallback_to_source",
    mutate(run) {
      run.request.named_files = [];
    },
    error: /source read is not authorized by a user-named file/u,
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
      mutateBody(run, 0, (body) => delete body.evidence);
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
        body.target.symbol_id = "rust:crate::OtherThing";
      });
    },
    error: /context result does not (?:match the selected target|bind its returned target to evidence)/u,
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
      boundary.final.reason_codes = [kind];
      assert.equal(validate(host, boundary).status, "pass", `${kind} accepted boundary`);

      const outOfRange = baseRun("typed_proof_unknown");
      mutateBody(outOfRange, 0, (body) => {
        body.disposition.gaps = [{ kind, [indexField]: invalidIndex }];
      });
      outOfRange.final.reason_codes = [kind];
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

test("tool results bind the host-negotiated revision to the authenticated discovery roster", () => {
  const june = baseRun("exact_symbol_search");
  const juneMeta = june.steps[0].result._meta;
  delete juneMeta.codestory_protocol;
  juneMeta["com.thegreencedar.codestory/protocolRevision"] = "2025-06-18";
  assert.equal(validate("codex", june).status, "pass");

  const nativeProof = baseRun("typed_proof_contract_proven");
  nativeProof.steps[0].result._meta = {
    "com.thegreencedar.codestory/protocolRevision": "2025-06-18",
    codestory_publication: {
      schema_version: 3,
      minimum_compatible_schema_version: 3,
    },
  };
  assert.equal(validate("codex", nativeProof).status, "pass");

  const opaqueProofCallsite = baseRun("typed_proof_contract_proven");
  mutateBody(opaqueProofCallsite, 0, (body) => {
    body.receipts[0].callsite_identity = "opaque-after-admission";
    body.identities.evidence[0].callsite_identity = "opaque-after-admission";
  });
  assert.equal(validate("codex", opaqueProofCallsite).status, "pass");

  const nativeSearchWithoutRuntime = baseRun("exact_symbol_search");
  nativeSearchWithoutRuntime.steps[0].result._meta = {
    "com.thegreencedar.codestory/protocolRevision": "2025-06-18",
    codestory_publication: {
      schema_version: 3,
      minimum_compatible_schema_version: 3,
    },
  };
  assert.throws(
    () => validate("codex", nativeSearchWithoutRuntime),
    /requires runtime identity outside the native proof result contract/u,
  );

  const projectedMissingDigest = baseRun("exact_symbol_search");
  delete projectedMissingDigest.steps[0].result._meta.codestory_protocol.discovery_contract_sha256;
  assert.throws(
    () => validate("codex", projectedMissingDigest),
    /projected protocol metadata.*discovery digest/u,
  );

  const unknown = baseRun("exact_symbol_search");
  delete unknown.steps[0].result._meta.codestory_protocol;
  unknown.steps[0].result._meta["com.thegreencedar.codestory/protocolRevision"] = "2099-01-01";
  assert.throws(() => validate("codex", unknown), /negotiated protocol revision.*authenticated roster/u);
  for (const inherited of ["toString", "constructor", "__proto__"]) {
    const hostile = baseRun("exact_symbol_search");
    delete hostile.steps[0].result._meta.codestory_protocol;
    hostile.steps[0].result._meta["com.thegreencedar.codestory/protocolRevision"] = inherited;
    assert.throws(
      () => validate("codex", hostile),
      /negotiated protocol revision.*authenticated roster/u,
      inherited,
    );
  }

  const conflict = baseRun("exact_symbol_search");
  conflict.steps[0].result._meta["com.thegreencedar.codestory/protocolRevision"] = "2025-06-18";
  assert.throws(() => validate("codex", conflict), /protocol revision metadata conflicts/u);

  const projectedMissingRuntime = baseRun("exact_symbol_search");
  delete projectedMissingRuntime.steps[0].result._meta.codestory_publication.contract_runtime;
  assert.throws(
    () => validate("codex", projectedMissingRuntime),
    /requires runtime identity outside the native proof result contract/u,
  );
});

test("semantic proof tool errors use the explicit error contract without result identity metadata", () => {
  const malformed = baseRun("malformed_proof_contract");
  malformed.steps[0].result._meta = {
    codestory_execution: { semantic_retrieval_activated: false },
  };
  assert.equal(validate("codex", malformed).status, "pass");

  const hostFailed = baseRun("malformed_proof_contract");
  hostFailed.steps[0].hostFailed = true;
  assert.equal(validate("codex", hostFailed).status, "pass");

  const plainText = baseRun("malformed_proof_contract");
  plainText.steps[0].hostFailed = true;
  plainText.steps[0].result = {
    content: [{ type: "text", text: "MissingResolvedMaterialAnchor { field: Start }" }],
    isError: true,
  };
  plainText.final.reason_codes = [];
  assert.equal(validate("codex", plainText).status, "pass");
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
  context.steps[1].args.id = "rust:crate::two::Thing";
  assert.throws(() => validate("codex", context), /selected (?:search )?target/u);

  const mappedPath = baseRun("ambiguous_symbol_then_context");
  mappedPath.request.selected_target = "src/one.rs";
  mutateBody(mappedPath, 0, (body) => {
    body.evidence[0].path = "/workspace/repo/src/one.rs";
    body.evidence.push(v3SearchEvidence("repo-text", "src/one.rs", null));
  });
  assert.equal(validate("codex", mappedPath).status, "pass");

  const ambiguousPath = clone(mappedPath);
  mutateBody(ambiguousPath, 0, (body) => {
    body.evidence.push(v3SearchEvidence("typed-duplicate", "/workspace/repo/src/one.rs", "rust:crate::duplicate::Thing"));
  });
  assert.throws(() => validate("codex", ambiguousPath), /selected target does not identify exactly one/u);

  const focusedEvidence = clone(mappedPath);
  mutateBody(focusedEvidence, 1, (body) => {
    body.evidence.push(v3ContextEvidence("other-context", "src/two.rs", "rust:crate::two::Thing"));
  });
  focusedEvidence.final.evidence_ids = ["context-1"];
  assert.equal(validate("codex", focusedEvidence).status, "pass");

  const unrelatedEvidence = clone(focusedEvidence);
  unrelatedEvidence.final.evidence_ids = ["other-context"];
  assert.throws(() => validate("codex", unrelatedEvidence), /omit the selected target evidence/u);

  const initialProbe = baseRun("packet_gap_to_focused_source");
  initialProbe.steps[0].args.probes = [{ kind: "exact_path", path: "src/gap.rs" }];
  assert.throws(() => validate("codex", initialProbe), /initial packet arguments/u);

  const authorizedGapRead = baseRun("packet_gap_to_focused_source");
  mutateBody(authorizedGapRead, 0, (body) => {
    body.gaps[0].message = "Missing evidence for `src/gap.rs`";
  });
  authorizedGapRead.steps.push({ kind: "source_read", path: "src/gap.rs" });
  authorizedGapRead.final = finalClaim({
    authority: "source",
    evidence_ids: ["evidence-1", "source:src/gap.rs"],
    gap_ids: ["gap-1"],
  });
  assert.equal(validate("codex", authorizedGapRead).status, "pass");

  const supportedUnresolvedGap = baseRun("packet_gap_to_focused_source");
  supportedUnresolvedGap.final.outcome = "supported";
  supportedUnresolvedGap.final.material_omissions = ["The missing route branch remains unresolved."];
  assert.throws(
    () => validate("codex", supportedUnresolvedGap),
    /cannot call unresolved requested material supported/u,
  );

  const resultBoundPacketReason = baseRun("packet_gap_to_focused_source");
  resultBoundPacketReason.final.reason_codes = ["evidence_missing"];
  assert.equal(validate("codex", resultBoundPacketReason).status, "pass");

  const inventedPacketReason = baseRun("packet_gap_to_focused_source");
  inventedPacketReason.final.reason_codes = ["invented_reason"];
  assert.throws(
    () => validate("codex", inventedPacketReason),
    /reason_codes do not match result-bound codes/u,
  );

  for (const scenarioId of ["exact_symbol_search", "ambiguous_symbol_then_context"]) {
    const rewrittenSearch = baseRun(scenarioId);
    rewrittenSearch.steps[0].args.query = `declarations named ${rewrittenSearch.steps[0].args.query}`;
    assert.throws(
      () => validate("codex", rewrittenSearch),
      /search query must preserve the exact supplied symbol name/u,
    );
  }

  const diagnosticsOnlyUnavailable = baseRun("selected_target_context");
  diagnosticsOnlyUnavailable.final.outcome = "unavailable";
  diagnosticsOnlyUnavailable.final.reason_codes = ["unavailable"];
  assert.throws(
    () => validate("codex", diagnosticsOnlyUnavailable),
    /reason_codes do not match result-bound codes/u,
  );

  const refusedWithOmission = baseRun("refuse_free_english_proof");
  refusedWithOmission.final.material_omissions = ["whether start calls finish"];
  assert.equal(validate("codex", refusedWithOmission).status, "pass");

  const continuedSourceFallback = baseRun("packet_single_continuation");
  continuedSourceFallback.steps.push({ kind: "source_read", path: "src/unread.rs" });
  continuedSourceFallback.final = finalClaim({
    authority: "source",
    evidence_ids: ["evidence-1", "evidence-2", "source:src/unread.rs"],
  });
  assert.equal(validate("codex", continuedSourceFallback).status, "pass");
  assert.equal(validate("cursor", continuedSourceFallback).status, "pass");

  const unresolvedNamedSourceFallback = baseRun("packet_named_fallback_to_source");
  unresolvedNamedSourceFallback.final.outcome = "unknown";
  unresolvedNamedSourceFallback.final.evidence_ids = ["source:src/fallback.rs"];
  unresolvedNamedSourceFallback.final.material_omissions = ["How the routing catalog works remains unresolved."];
  assert.equal(validate("codex", unresolvedNamedSourceFallback).status, "pass");

  const missingNamedSourceEvidence = baseRun("packet_named_fallback_to_source");
  missingNamedSourceEvidence.final.evidence_ids = ["evidence-1"];
  assert.throws(
    () => validate("codex", missingNamedSourceEvidence),
    /omit successful source evidence/u,
  );

  const failedContinuedSourceFallback = baseRun("packet_single_continuation");
  failedContinuedSourceFallback.steps.push({ kind: "source_read", path: "src/unread.rs", failed: true });
  assert.equal(validate("codex", failedContinuedSourceFallback).status, "pass");
  assert.equal(validate("cursor", failedContinuedSourceFallback).status, "pass");

  const failedRequiredSourceRead = baseRun("named_file_direct_read");
  failedRequiredSourceRead.steps[0].failed = true;
  assert.throws(
    () => validate("codex", failedRequiredSourceRead),
    /unexpected failed source_read action/u,
  );

  const unrelatedContinuedSourceFallback = baseRun("packet_single_continuation");
  unrelatedContinuedSourceFallback.steps.push({ kind: "source_read", path: "src/other.rs" });
  assert.throws(
    () => validate("codex", unrelatedContinuedSourceFallback),
    /source read is not authorized by a user-named file/u,
  );

  for (const message of [
    "Missing evidence for `src/gap.rs#copy`",
    "Missing evidence for `src/gap.rsé`",
    "Missing evidence for `src/gap.rs` and src/β.rs",
  ]) {
    const inexactGapRead = baseRun("packet_gap_to_focused_source");
    mutateBody(inexactGapRead, 0, (body) => { body.gaps[0].message = message; });
    inexactGapRead.steps.push({ kind: "source_read", path: "src/gap.rs" });
    assert.throws(
      () => validate("codex", inexactGapRead),
      /source read is not correlated with the packet evidence gap/u,
      message,
    );
  }

  const disclosedOmission = baseRun("broad_packet");
  mutateBody(disclosedOmission, 0, (body) => {
    body.gaps.push(v3Gap("material-gap", "evidence_missing", "A requested route remains unproven."));
  });
  disclosedOmission.final.gap_ids = ["material-gap"];
  disclosedOmission.final.outcome = "unknown";
  disclosedOmission.final.material_omissions = ["The requested route remains unproven."];
  assert.equal(validate("codex", disclosedOmission).status, "pass");

  const inventedOmission = baseRun("broad_packet");
  inventedOmission.final.material_omissions = ["An unsupported limitation."];
  assert.throws(() => validate("codex", inventedOmission), /omissions without a result-bound gap/u);
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
  staticIdentity.protocol.discovery_contracts = clone(catalog.wireContract.discoveryContracts);
  const rosterPaths = [
    "plugin.json",
    "cli-version.json",
    "generated-mcp-catalog.json",
    "mcp.json",
    "scripts/codestory-mcp.cjs",
    ".claude-plugin/plugin.json",
    ".github/plugin/plugin.json",
    ".cursor-plugin/plugin.json",
    "hooks/claude-codex-hooks.json",
    "hooks/codestory-activate.cjs",
    "hooks/copilot-hooks.json",
    "hooks/cursor-hooks.json",
    "mcp.cursor.json",
    "rules/codestory.mdc",
    "skills/codestory-grounding/SKILL.md",
    "skills/codestory-grounding/agents/openai.yaml",
    "skills/codestory-grounding/references/generated-mcp-syntax.md",
    "skills/codestory-grounding/references/status-contract.md",
    "skills/codestory-grounding/references/ground.md",
    "skills/codestory-grounding/references/files.md",
    "skills/codestory-grounding/references/affected.md",
    "skills/codestory-grounding/references/packet.md",
    "skills/codestory-grounding/references/search.md",
    "skills/codestory-grounding/references/context.md",
    "skills/codestory-grounding/references/symbol.md",
    "skills/codestory-grounding/references/trail.md",
    "skills/codestory-grounding/references/snippet.md",
  ];
  staticIdentity.static_roster = Object.fromEntries(
    rosterPaths.map((path) => [path, sha256(readFileSync(join(root, path)))]),
  );
  return staticIdentity;
}

test("static Cursor Claude Code and Copilot surfaces bind one package launcher hook and rule core", async () => {
  assert.deepEqual(Object.keys(STATIC_PARITY_HOSTS), ["cursor", "claude_code", "copilot_cli", "copilot_editor"]);
  const manifest = JSON.parse(await readFile(join(pluginRoot, "plugin.json"), "utf8"));
  const staticIdentity = staticIdentityFor(pluginRoot);

  const report = await validateStaticHostParity(pluginRoot, staticIdentity);
  assert.equal(report.status, "pass");
  assert.deepEqual(report.hosts.map(({ host }) => host), ["cursor", "claude_code", "copilot_cli", "copilot_editor"]);
  for (const host of report.hosts) {
    assert.equal(host.package_version, manifest.version);
    assert.equal(host.launcher_sha256, staticIdentity.launcher.sha256);
    assert.equal(host.rule_sha256.length, 64);
    assert.equal(host.hook_sha256.length, 64);
    assert.equal(host.model_routing_evaluated, false);
  }

  const skill = await readFile(join(pluginRoot, "skills", "codestory-grounding", "SKILL.md"), "utf8");
  const cursorRule = await readFile(join(pluginRoot, "rules", "codestory.mdc"), "utf8");
  const openAiMetadata = await readFile(
    join(pluginRoot, "skills", "codestory-grounding", "agents", "openai.yaml"),
    "utf8",
  );
  const searchReference = await readFile(
    join(pluginRoot, "skills", "codestory-grounding", "references", "search.md"),
    "utf8",
  );
  const contextReference = await readFile(
    join(pluginRoot, "skills", "codestory-grounding", "references", "context.md"),
    "utf8",
  );
  const packetReference = await readFile(
    join(pluginRoot, "skills", "codestory-grounding", "references", "packet.md"),
    "utf8",
  );
  for (const [label, guidance] of [["skill", skill], ["Cursor rule", cursorRule]]) {
    assert.match(guidance, /discovery leads?.*`search`/isu, label);
    assert.match(guidance, /successful search.*stop.*(?:do not|never).*source/isu, label);
    assert.match(guidance, /successful search.*stop.*unless.*exact selection/isu, label);
    assert.match(guidance, /symbol_id.*context.*(?:`id`|\.id)/isu, label);
    assert.match(guidance, /selected target.*`context`/isu, label);
    assert.match(guidance, /supplied symbol name.*search\.query.*unchanged/isu, label);
    assert.match(guidance, /broad.*`packet`.*continuation.*once/isu, label);
    assert.match(guidance, /host-supplied.*`prove_call_path`/isu, label);
    assert.match(guidance, /semantic proof tool error.*invalid contract.*not\s+typed-proof evidence/isu, label);
    assert.match(guidance, /exact proof from English.*no complete typed\s+contract.*stop.*do not call a\s+repository tool/isu, label);
    assert.match(guidance, /`unknown`.*not absence/isu, label);
    assert.match(guidance, /runtime execution/iu, label);
    assert.match(guidance, /typed `Unavailable`.*terminal/isu, label);
    assert.match(guidance, /diagnostics\.availability.*optional diagnostics.*never overrides.*top-level/isu, label);
    assert.match(guidance, /transport.*tool absence.*source/isu, label);
  }
  assert.match(openAiMetadata, /search.*context.*packet.*prove_call_path/isu);
  assert.match(openAiMetadata, /host-supplied/iu);
  assert.match(openAiMetadata, /unknown.*not absence/isu);
  assert.match(openAiMetadata, /successful search.*unless.*exact selection/isu);
  assert.match(openAiMetadata, /typed `Unavailable`.*terminal/isu);
  assert.match(openAiMetadata, /transport.*tool absence.*source/isu);
  assert.match(skill, /omit optional numeric bounds.*generated schema/isu);
  assert.match(searchReference, /limit.*1.*50/isu);
  assert.match(contextReference, /bare\s+symbol.*exact\s+path.*evidence\[\]\.symbol_id.*context\.id/isu);
  assert.match(contextReference, /do not combine.*name.*path.*free-text\s+`query`/isu);
  assert.match(packetReference, /continuation\.gap_ids.*map.*gap_id/isu);
  assert.match(packetReference, /fallback-only.*initial.*probe/isu);
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
    await assert.rejects(validateStaticHostParity(root, headingOnly), /search discovery authority/u);

    cpSync(pluginRoot, root, { recursive: true, force: true });
    const skillPath = join(root, "skills", "codestory-grounding", "SKILL.md");
    writeFileSync(
      skillPath,
      readFileSync(skillPath, "utf8").replace(
        /Discovery leads come from `search`; they identify candidates and never prove a claim\./u,
        "Use ordinary symbol lookup for candidates.",
      ),
    );
    const incompleteSkill = staticIdentityFor(root);
    await assert.rejects(validateStaticHostParity(root, incompleteSkill), /search discovery authority/u);

    cpSync(pluginRoot, root, { recursive: true, force: true });
    writeFileSync(join(root, "rules", "codestory.mdc"), `---
description: CodeStory local grounding. Use repo evidence before source claims.
globs:
alwaysApply: true
---

# CodeStory Grounding

Call the CodeStory tool that matches the task. The codestory-grounding skill owns the detailed tool and evidence contract.
`);
    const delegatedOnlyCursorRule = staticIdentityFor(root);
    await assert.rejects(validateStaticHostParity(root, delegatedOnlyCursorRule), /search discovery authority/u);

    cpSync(pluginRoot, root, { recursive: true, force: true });
    const openAiMetadataPath = join(root, "skills", "codestory-grounding", "agents", "openai.yaml");
    writeFileSync(
      openAiMetadataPath,
      readFileSync(openAiMetadataPath, "utf8").replace(
        /A typed `Unavailable` result is terminal\. MCP transport or tool absence may authorize ordinary source inspection; a successful unavailable result does not unless an exact file or focused gap authorizes it\. /u,
        "",
      ),
    );
    const incompleteOpenAiMetadata = staticIdentityFor(root);
    await assert.rejects(validateStaticHostParity(root, incompleteOpenAiMetadata), /OpenAI skill metadata/u);

    for (const relativePath of [
      ".cursor-plugin/plugin.json",
      "hooks/cursor-hooks.json",
      "mcp.cursor.json",
      "rules/codestory.mdc",
    ]) {
      cpSync(pluginRoot, root, { recursive: true, force: true });
      const authenticated = staticIdentityFor(root);
      writeFileSync(join(root, relativePath), "substituted cursor surface\n");
      await assert.rejects(
        validateStaticHostParity(root, authenticated),
        new RegExp(`static digest roster.*${relativePath.split("/").pop().replaceAll(".", "\\.")}`, "u"),
        relativePath,
      );
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
