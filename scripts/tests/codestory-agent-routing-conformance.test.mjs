import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import {
  INSTALLED_IDENTITY_FIELDS,
  ROUTING_SCENARIOS,
  STATIC_PARITY_HOSTS,
  parseInstalledTranscript,
  validateInstalledSession,
  validateStaticHostParity,
} from "../codestory-agent-routing-conformance.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..", "..");
const pluginRoot = join(repoRoot, "plugins", "codestory");
const SHA_A = "a".repeat(64);
const SHA_B = "b".repeat(64);
const SHA_C = "c".repeat(64);
const SHA_D = "d".repeat(64);

const EXPECTED_IDENTITY = Object.freeze({
  package: Object.freeze({ name: "codestory", version: "0.18.0-candidate.7", sha256: SHA_A }),
  launcher: Object.freeze({ relative_path: "scripts/codestory-mcp.cjs", sha256: SHA_B }),
  cli: Object.freeze({ version: "0.18.0-candidate.7", sha256: SHA_C, source: "managed" }),
  publication: Object.freeze({ schema_version: 3 }),
  protocol: Object.freeze({
    revision: "2025-11-25",
    discovery_contract_sha256: SHA_D,
  }),
});

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

function proofContract() {
  return {
    source_text: "`crate::start` directly calls `crate::finish`.",
    clauses: [
      {
        clause_id: "start",
        start_byte: 0,
        end_byte_exclusive: 14,
        quote: "`crate::start`",
        classification: { kind: "resolved_material", fields: [{ kind: "start" }] },
      },
      {
        clause_id: "target",
        start_byte: 30,
        end_byte_exclusive: 45,
        quote: "`crate::finish`",
        classification: {
          kind: "resolved_material",
          fields: [{ kind: "step_target", step: 1 }, { kind: "directness", step: 1 }],
        },
      },
    ],
    spec: {
      start: { kind: "canonical_id", canonical_id: "rust:crate::start" },
      steps: [{ target: { kind: "canonical_id", canonical_id: "rust:crate::finish" } }],
      prohibit_traversal_through: [],
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

function proofBody(disposition, detail = {}) {
  return {
    kind: "complete",
    proof_contract_schema_version: 1,
    proof_domain: "indexed_source_call_path_v1",
    clause_guard_version: "clause_guard_v1",
    disposition: { kind: disposition, ...detail },
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
    installed_identity: clone(EXPECTED_IDENTITY),
    steps: [],
    final: "Completed from the permitted evidence.",
  };

  const mcp = (tool, args, body, options) => ({ kind: "mcp", tool, args, result: result(body, options) });
  const read = (path) => ({ kind: "source_read", path });

  switch (scenarioId) {
    case "named_file_direct_read":
      run.request.named_files = ["src/named.rs"];
      run.steps = [read("src/named.rs")];
      run.final = "The named file src/named.rs contains the requested definition.";
      break;
    case "exact_symbol_search":
      run.steps = [mcp("search", { project: "/workspace/repo", query: "ExactThing" }, {
        kind: "complete",
        leads: [{ canonical_id: "rust:crate::ExactThing" }],
        gaps: [],
      })];
      run.final = "Search found the discovery lead rust:crate::ExactThing; this is not a proof claim.";
      break;
    case "ambiguous_symbol_then_context":
      run.request.selected_target = "rust:crate::one::Thing";
      run.steps = [
        mcp("search", { project: "/workspace/repo", query: "Thing" }, {
          kind: "complete",
          leads: [
            { canonical_id: "rust:crate::one::Thing" },
            { canonical_id: "rust:crate::two::Thing" },
          ],
          gaps: [{ code: "selector_ambiguous" }],
        }),
        mcp("context", {
          project: "/workspace/repo",
          selector: { canonical_id: "rust:crate::one::Thing" },
        }, { kind: "complete", target: "rust:crate::one::Thing", evidence: ["src/one.rs:1"] }),
      ];
      run.final = "The search was selector_ambiguous. Context covers selected target rust:crate::one::Thing only.";
      break;
    case "selected_target_context":
      run.request.selected_target = "rust:crate::ExactThing";
      run.steps = [mcp("context", {
        project: "/workspace/repo",
        selector: { canonical_id: "rust:crate::ExactThing" },
      }, { kind: "complete", target: "rust:crate::ExactThing", evidence: ["src/lib.rs:10"] })];
      run.final = "Context provides evidence for selected target rust:crate::ExactThing only.";
      break;
    case "broad_packet":
      run.steps = [mcp("packet", { project: "/workspace/repo", question: "How does the flow work?" }, {
        kind: "complete",
        packet_id: "packet-1",
        status: "complete",
        evidence: [{ evidence_id: "evidence-1" }],
        gaps: [],
      })];
      run.final = "Packet evidence evidence-1 supports the broad answer; packet availability is not proof.";
      break;
    case "packet_single_continuation":
      run.steps = [
        mcp("packet", { project: "/workspace/repo", question: "How does the flow work?" }, {
          kind: "complete",
          packet_id: "packet-1",
          status: "continuation_available",
          publication: { core_generation_id: "core-1", retrieval_generation_id: "retrieval-1" },
          continuation: { continuation_id: "continuation-1", gap_ids: ["gap-1"] },
          evidence: [{ evidence_id: "evidence-1" }],
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
          evidence: [{ evidence_id: "evidence-2" }],
          gaps: [],
        }),
      ];
      run.final = "The one bounded continuation supplied evidence-2 and closed gap-1.";
      break;
    case "packet_gap_to_focused_source":
      run.steps = [
        mcp("packet", { project: "/workspace/repo", question: "How does the flow work?" }, {
          kind: "complete",
          packet_id: "packet-1",
          status: "no_useful_evidence",
          evidence: [],
          gaps: [{ gap_id: "gap-1", authorized_source_paths: ["src/gap.rs"] }],
        }),
        read("src/gap.rs"),
      ];
      run.final = "Packet gap gap-1 required a focused read of src/gap.rs; the source is the cited evidence.";
      break;
    case "packet_unavailable_to_source":
      run.steps = [
        mcp("packet", { project: "/workspace/repo", question: "How does the flow work?" }, {
          kind: "budget_exceeded",
          status: "unavailable",
          reason: "retrieval_unavailable",
          gaps: [],
        }),
        read("src/fallback.rs"),
      ];
      run.final = "Packet was retrieval_unavailable, so src/fallback.rs is the focused source evidence.";
      break;
    case "typed_proof_contract_proven":
      run.request.proof_contract = typed;
      run.steps = [mcp("prove_call_path", { project: "/workspace/repo", ...typed }, proofBody("contract_proven"))];
      run.final = "The typed verifier returned ContractProven for this indexed source call path only.";
      break;
    case "typed_proof_contract_refuted":
      run.request.proof_contract = typed;
      run.steps = [mcp("prove_call_path", { project: "/workspace/repo", ...typed }, proofBody("contract_refuted", {
        basis: { kind: "positive_contradiction" },
      }))];
      run.final = "The typed verifier returned ContractRefuted by positive_contradiction.";
      break;
    case "typed_proof_unknown":
      run.request.proof_contract = typed;
      run.steps = [mcp("prove_call_path", { project: "/workspace/repo", ...typed }, proofBody("unknown", {
        gaps: [{ code: "selector_missing" }],
      }))];
      run.final = "The typed verifier returned Unknown because selector_missing; this does not establish absence.";
      break;
    case "typed_proof_unavailable":
      run.request.proof_contract = typed;
      run.steps = [mcp("prove_call_path", { project: "/workspace/repo", ...typed }, proofBody("unavailable", {
        reason: "proof_semantic_projection_unavailable",
      }))];
      run.final = "The typed verifier returned Unavailable: proof_semantic_projection_unavailable.";
      break;
    case "malformed_proof_contract": {
      const malformed = { source_text: "A calls B", clauses: [] };
      run.request.proof_contract = malformed;
      run.steps = [mcp("prove_call_path", { project: "/workspace/repo", ...malformed }, {
        code: "invalid_proof_interpretation",
        message: "spec is required",
      }, { isError: true })];
      run.final = "The typed contract is invalid_proof_interpretation because spec is required; no proof disposition was produced.";
      break;
    }
    case "refuse_free_english_proof":
      run.request.text = "Prove from this sentence that start calls finish.";
      run.final = "I cannot construct an authoritative typed contract from free English; provide the typed contract.";
      break;
    case "proof_observational":
      run.request.proof_contract = typed;
      run.steps = [mcp("prove_call_path", { project: "/workspace/repo", ...typed }, proofBody("unknown", {
        gaps: [{ code: "edge_not_proof_authoritative" }],
      }))];
      run.final = "The observational verifier returned Unknown: edge_not_proof_authoritative; it did not activate semantic retrieval.";
      break;
    case "hidden_proof_tool_discovery":
      run.request.proof_contract = typed;
      run.steps = [
        { kind: "tool_search", query: "codestory mcp prove_call_path", tools: ["mcp__codestory__prove_call_path"] },
        mcp("prove_call_path", { project: "/workspace/repo", ...typed }, proofBody("contract_proven")),
      ];
      run.final = "After discovering only prove_call_path, the typed verifier returned ContractProven for the indexed source path.";
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
  events.push({ type: "item.completed", item: { id: "answer", type: "agent_message", text: run.final } });
  return `${events.map((event) => JSON.stringify(event)).join("\n")}\n`;
}

function cursorJsonl(run) {
  const events = [{ type: "system", subtype: "init", session_id: "cursor-session-1" }];
  let counter = 0;
  for (const step of run.steps) {
    counter += 1;
    const id = `call-${counter}`;
    let name;
    let input;
    let output;
    if (step.kind === "mcp") {
      name = `mcp_codestory_${step.tool}`;
      input = step.args;
      output = JSON.stringify(step.result);
    } else if (step.kind === "tool_search") {
      name = "tool_search";
      input = { query: step.query };
      output = JSON.stringify({ tools: step.tools });
    } else {
      name = "Shell";
      input = { command: `sed -n '1,120p' '${step.path}'` };
      output = "source fixture\n";
    }
    events.push({
      type: "assistant",
      message: { role: "assistant", content: [{ type: "tool_use", id, name, input }] },
    });
    events.push({
      type: "user",
      message: {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: id, content: output, is_error: false }],
      },
    });
  }
  events.push({
    type: "assistant",
    message: { role: "assistant", content: [{ type: "text", text: run.final }] },
  });
  return `${events.map((event) => JSON.stringify(event)).join("\n")}\n`;
}

function transcript(host, run) {
  return host === "codex" ? codexJsonl(run) : cursorJsonl(run);
}

function validate(host, run) {
  return validateInstalledSession({
    host,
    scenarioId: run.scenario_id,
    request: run.request,
    installedIdentity: run.installed_identity,
    expectedIdentity: EXPECTED_IDENTITY,
    transcript: transcript(host, run),
  });
}

test("freezes exactly the sixteen accepted routing scenarios", () => {
  assert.deepEqual(ROUTING_SCENARIOS.map(({ id }) => id), SCENARIO_IDS);
  assert.equal(new Set(ROUTING_SCENARIOS.map(({ id }) => id)).size, 16);
  for (const scenario of ROUTING_SCENARIOS) {
    assert.equal(typeof scenario.expected_first_tool, "string", scenario.id);
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

const MUTATIONS = [
  {
    name: "wrong first tool",
    scenario: "broad_packet",
    mutate(run) {
      run.steps.unshift(baseRun("selected_target_context").steps[0]);
    },
    error: /expected first tool packet/u,
  },
  {
    name: "forbidden followup",
    scenario: "broad_packet",
    mutate(run) {
      run.steps.push(baseRun("selected_target_context").steps[0]);
    },
    error: /follow-up context is not permitted/u,
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
      run.final = "This is not a proof claim, but search verified the call definitely exists.";
    },
    error: /missing required final claim|final claim violates/u,
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
    error: /expected proof disposition unknown/u,
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
    error: /at most one packet continuation/u,
  },
  {
    name: "selector relaxation retry",
    scenario: "typed_proof_contract_proven",
    mutate(run) {
      const retry = clone(run.steps[0]);
      retry.args.spec.start = { kind: "qualified", qualified_name: "crate::start" };
      run.steps.push(retry);
    },
    error: /follow-up prove_call_path is not permitted|proof request must preserve the host-supplied typed contract|proof may be called only once/u,
  },
  {
    name: "unknown becomes absence",
    scenario: "typed_proof_unknown",
    mutate(run) {
      run.final = "The call is absent and never happens.";
    },
    error: /Unknown must not become absence|missing required final claim/u,
  },
  {
    name: "silent material gap",
    scenario: "packet_gap_to_focused_source",
    mutate(run) {
      run.final = "The source answers the question.";
    },
    error: /missing required final claim.*gap-1/u,
  },
  {
    name: "free English proof construction",
    scenario: "refuse_free_english_proof",
    mutate(run) {
      run.steps.push(baseRun("typed_proof_contract_proven").steps[0]);
      run.final = "ContractProven.";
    },
    error: /proof requires a host-supplied typed contract|expected no tool/u,
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

test("every installed identity field is exact and every CodeStory result repeats runtime identity", () => {
  for (const field of INSTALLED_IDENTITY_FIELDS) {
    const run = baseRun("broad_packet");
    const parts = field.split(".");
    let owner = run.installed_identity;
    for (const part of parts.slice(0, -1)) owner = owner[part];
    const leaf = parts.at(-1);
    owner[leaf] = typeof owner[leaf] === "number" ? owner[leaf] + 1 : `${owner[leaf]}-drift`;
    assert.throws(() => validate("codex", run), new RegExp(field.replaceAll(".", "\\."), "u"), field);
  }

  const resultDrift = baseRun("broad_packet");
  resultDrift.steps[0].result._meta.codestory_protocol.discovery_contract_sha256 = "e".repeat(64);
  assert.throws(() => validate("codex", resultDrift), /result identity.*discovery/u);
});

test("packet continuation and selected-context correlation are exact", () => {
  const continuation = baseRun("packet_single_continuation");
  continuation.steps[1].args.option_ids = ["other-gap"];
  assert.throws(() => validate("cursor", continuation), /continuation arguments/u);

  const context = baseRun("ambiguous_symbol_then_context");
  context.steps[1].args.selector.canonical_id = "rust:crate::two::Thing";
  assert.throws(() => validate("codex", context), /selected target/u);
});

test("static Claude Code and Copilot surfaces bind one package, launcher, hook, and rule core", async () => {
  assert.deepEqual(Object.keys(STATIC_PARITY_HOSTS), ["claude_code", "copilot_cli", "copilot_editor"]);
  const manifest = JSON.parse(await readFile(join(pluginRoot, "plugin.json"), "utf8"));
  const pin = JSON.parse(await readFile(join(pluginRoot, "cli-version.json"), "utf8"));
  const catalog = JSON.parse(await readFile(join(pluginRoot, "generated-mcp-catalog.json"), "utf8"));
  const launcher = await readFile(join(pluginRoot, "scripts", "codestory-mcp.cjs"));
  const staticIdentity = clone(EXPECTED_IDENTITY);
  staticIdentity.package.version = manifest.version;
  staticIdentity.cli.version = pin.cli_version;
  staticIdentity.launcher.sha256 = createHash("sha256").update(launcher).digest("hex");
  staticIdentity.publication.schema_version = catalog.wireContract.publicationStampSchemaVersion;
  staticIdentity.protocol.revision = catalog.wireContract.preferredMcpProtocolVersion;
  staticIdentity.protocol.discovery_contract_sha256 =
    catalog.wireContract.discoveryContracts[staticIdentity.protocol.revision];

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
