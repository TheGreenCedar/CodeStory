#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SHA256 = /^[0-9a-f]{64}$/u;
const PROOF_DISPOSITIONS = new Set([
  "contract_proven",
  "contract_refuted",
  "unknown",
  "unavailable",
]);
const ROUTING_ACTIONS = Object.freeze([
  "source_read",
  "search",
  "context",
  "packet",
  "prove_call_path",
  "tool_search",
]);

export const INSTALLED_IDENTITY_FIELDS = Object.freeze([
  "package.name",
  "package.version",
  "package.sha256",
  "launcher.relative_path",
  "launcher.sha256",
  "cli.version",
  "cli.sha256",
  "cli.source",
  "publication.schema_version",
  "protocol.revision",
  "protocol.discovery_contract_sha256",
]);

const IDENTITY_REQUIREMENTS = Object.freeze({
  mode: "exact",
  fields: INSTALLED_IDENTITY_FIELDS,
});

function deepFreeze(value) {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    for (const child of Object.values(value)) deepFreeze(child);
    Object.freeze(value);
  }
  return value;
}

function scenario({
  id,
  first,
  followups = [],
  source = "none",
  required = [],
  forbidden = [],
  disposition = null,
  typedContract = "none",
}) {
  const allowed = new Set([first, ...followups].filter((item) => item !== "none"));
  return {
    id,
    expected_first_tool: first,
    permitted_followups: followups,
    forbidden_tools: ROUTING_ACTIONS.filter((item) => !allowed.has(item)),
    source_read_authorization: { kind: source },
    final_claim_constraints: {
      required_terms: required,
      forbidden_terms: forbidden,
      proof_disposition: disposition,
    },
    typed_contract: typedContract,
    identity_requirements: IDENTITY_REQUIREMENTS,
  };
}

const NO_PROOF_CLAIMS = ["ContractProven", "ContractRefuted"];

export const ROUTING_SCENARIOS = deepFreeze([
  scenario({
    id: "named_file_direct_read",
    first: "source_read",
    source: "user_named_file",
    forbidden: NO_PROOF_CLAIMS,
  }),
  scenario({
    id: "exact_symbol_search",
    first: "search",
    required: ["not a proof claim"],
    forbidden: NO_PROOF_CLAIMS,
  }),
  scenario({
    id: "ambiguous_symbol_then_context",
    first: "search",
    followups: ["context"],
    required: ["selector_ambiguous", "selected target"],
    forbidden: NO_PROOF_CLAIMS,
  }),
  scenario({
    id: "selected_target_context",
    first: "context",
    required: ["selected target", "only"],
    forbidden: NO_PROOF_CLAIMS,
  }),
  scenario({
    id: "broad_packet",
    first: "packet",
    required: ["packet", "not proof"],
    forbidden: NO_PROOF_CLAIMS,
  }),
  scenario({
    id: "packet_single_continuation",
    first: "packet",
    followups: ["packet"],
    required: ["one bounded continuation", "gap-1"],
    forbidden: NO_PROOF_CLAIMS,
  }),
  scenario({
    id: "packet_gap_to_focused_source",
    first: "packet",
    followups: ["source_read"],
    source: "packet_evidence_gap",
    required: ["gap-1", "source"],
    forbidden: NO_PROOF_CLAIMS,
  }),
  scenario({
    id: "packet_unavailable_to_source",
    first: "packet",
    followups: ["source_read"],
    source: "packet_unavailable",
    required: ["retrieval_unavailable", "source"],
    forbidden: NO_PROOF_CLAIMS,
  }),
  scenario({
    id: "typed_proof_contract_proven",
    first: "prove_call_path",
    required: ["ContractProven", "indexed source"],
    disposition: "contract_proven",
    typedContract: "valid",
  }),
  scenario({
    id: "typed_proof_contract_refuted",
    first: "prove_call_path",
    required: ["ContractRefuted", "positive_contradiction"],
    disposition: "contract_refuted",
    typedContract: "valid",
  }),
  scenario({
    id: "typed_proof_unknown",
    first: "prove_call_path",
    required: ["Unknown", "selector_missing", "does not establish absence"],
    disposition: "unknown",
    typedContract: "valid",
  }),
  scenario({
    id: "typed_proof_unavailable",
    first: "prove_call_path",
    required: ["Unavailable", "proof_semantic_projection_unavailable"],
    disposition: "unavailable",
    typedContract: "valid",
  }),
  scenario({
    id: "malformed_proof_contract",
    first: "prove_call_path",
    required: ["invalid_proof_interpretation", "no proof disposition"],
    forbidden: NO_PROOF_CLAIMS,
    typedContract: "malformed",
  }),
  scenario({
    id: "refuse_free_english_proof",
    first: "none",
    required: ["cannot construct", "typed contract"],
    forbidden: [...NO_PROOF_CLAIMS, "verified"],
    typedContract: "forbidden",
  }),
  scenario({
    id: "proof_observational",
    first: "prove_call_path",
    required: ["Unknown", "edge_not_proof_authoritative", "did not activate semantic retrieval"],
    disposition: "unknown",
    typedContract: "valid",
  }),
  scenario({
    id: "hidden_proof_tool_discovery",
    first: "tool_search",
    followups: ["prove_call_path"],
    required: ["only prove_call_path", "ContractProven"],
    disposition: "contract_proven",
    typedContract: "valid",
  }),
]);

const SCENARIOS_BY_ID = new Map(ROUTING_SCENARIOS.map((entry) => [entry.id, entry]));

export const STATIC_PARITY_HOSTS = deepFreeze({
  claude_code: {
    metadata: ".claude-plugin/plugin.json",
    hook: "hooks/claude-codex-hooks.json",
    rule: "skills/codestory-grounding/SKILL.md",
  },
  copilot_cli: {
    metadata: ".github/plugin/plugin.json",
    hook: "hooks/copilot-hooks.json",
    rule: "skills/codestory-grounding/SKILL.md",
  },
  copilot_editor: {
    metadata: ".github/plugin/plugin.json",
    hook: "hooks/copilot-hooks.json",
    rule: "skills/codestory-grounding/SKILL.md",
  },
});

class ConformanceError extends Error {
  constructor(message) {
    super(message);
    this.name = "ConformanceError";
  }
}

function fail(message) {
  throw new ConformanceError(message);
}

function plainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function parseJsonLines(text, host) {
  if (typeof text !== "string") fail(`${host} transcript must be JSONL text`);
  const events = [];
  let lineNumber = 0;
  for (const line of text.split(/\r?\n/u)) {
    lineNumber += 1;
    if (!line.trim()) continue;
    try {
      const event = JSON.parse(line);
      if (!plainObject(event)) fail(`${host} transcript line ${lineNumber} must be an object`);
      events.push(event);
    } catch (error) {
      if (error instanceof ConformanceError) throw error;
      fail(`${host} transcript has malformed JSONL at line ${lineNumber}`);
    }
  }
  if (events.length === 0) fail(`${host} transcript is empty`);
  return events;
}

function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (plainObject(value)) {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

function equalJson(left, right) {
  return canonical(left) === canonical(right);
}

function normalizeToolName(name, server = "") {
  const raw = String(name ?? "").trim();
  const lower = raw.toLowerCase();
  if (lower.startsWith("mcp__codestory__")) return lower.slice("mcp__codestory__".length);
  const cursorMcp = lower.match(/^mcp[_-]codestory[_-](.+)$/u);
  if (cursorMcp) return cursorMcp[1];
  if (String(server).toLowerCase() === "codestory") return lower;
  if (lower === "tool_search" || lower.endsWith("__tool_search")) return "tool_search";
  return null;
}

function normalizePath(path) {
  const normalized = String(path ?? "").trim().replace(/^['"]+|['"]+$/gu, "").replaceAll("\\", "/");
  if (!normalized || normalized.startsWith("/") || /^[a-z]:\//iu.test(normalized)) {
    fail(`source read path must be project-relative: ${JSON.stringify(path)}`);
  }
  const parts = normalized.split("/");
  if (parts.some((part) => !part || part === "." || part === ".." || part.includes("\0"))) {
    fail(`source read path is invalid: ${JSON.stringify(path)}`);
  }
  return parts.join("/");
}

function sourceReadPath(command) {
  const text = String(command ?? "").trim();
  const patterns = [
    /^sed\s+-n\s+['"][^'"]+['"]\s+['"]([^'"]+)['"]\s*$/u,
    /^(?:cat|type|nl)(?:\s+-[A-Za-z]+)*\s+['"]([^'"]+)['"]\s*$/u,
    /^Get-Content(?:\s+-(?:LiteralPath|Path))?\s+['"]([^'"]+)['"]\s*$/iu,
  ];
  for (const pattern of patterns) {
    const match = text.match(pattern);
    if (match) return normalizePath(match[1]);
  }
  return null;
}

function beginAction(state, id, action) {
  if (!id || state.open.has(id) || state.completed.has(id)) fail(`duplicate or missing tool call id ${JSON.stringify(id)}`);
  if (state.open.size > 0) {
    fail(`tool call ${JSON.stringify(id)} started before ${JSON.stringify([...state.open.keys()][0])} completed`);
  }
  state.open.set(id, action);
  state.actions.push(action);
}

function completeAction(state, id, result, error = false) {
  const action = state.open.get(id);
  if (!action) fail(`unmatched tool call result ${JSON.stringify(id)}`);
  state.open.delete(id);
  state.completed.add(id);
  action.result = result;
  action.error = error;
  action.completed = true;
}

function parseCodex(events) {
  const state = { actions: [], open: new Map(), completed: new Set(), final: "" };
  for (const event of events) {
    const type = String(event.type ?? "");
    if (["thread.started", "turn.started", "turn.completed"].includes(type)) continue;
    if (type !== "item.started" && type !== "item.completed") {
      fail(`unsupported Codex event ${JSON.stringify(type)}`);
    }
    const item = event.item;
    if (!plainObject(item)) fail("Codex item event is missing item");
    const itemType = String(item.type ?? "");
    if (type === "item.completed" && itemType === "agent_message") {
      if (typeof item.text !== "string") fail("Codex agent message is missing text");
      state.final = item.text;
      continue;
    }
    if (["reasoning", "error"].includes(itemType)) continue;
    if (type === "item.started") {
      if (itemType === "mcp_tool_call") {
        const tool = normalizeToolName(item.tool ?? item.name, item.server);
        beginAction(state, String(item.id ?? ""), {
          kind: tool ?? "external_tool",
          tool: tool ?? String(item.tool ?? item.name ?? "unknown"),
          args: item.arguments ?? item.args ?? {},
          server: String(item.server ?? ""),
        });
      } else if (itemType === "command_execution") {
        const path = sourceReadPath(item.command);
        beginAction(state, String(item.id ?? ""), {
          kind: path ? "source_read" : "shell",
          tool: path ? "source_read" : "shell",
          path,
          command: String(item.command ?? ""),
        });
      } else {
        beginAction(state, String(item.id ?? ""), {
          kind: "external_tool",
          tool: itemType || "unknown",
          args: item.arguments ?? {},
        });
      }
      continue;
    }
    completeAction(
      state,
      String(item.id ?? ""),
      item.result ?? item.aggregated_output ?? null,
      item.error != null || String(item.status ?? "completed").toLowerCase() === "failed" || item.exit_code > 0,
    );
  }
  return finishParsedState("Codex", state);
}

function cursorBlocks(event) {
  return Array.isArray(event?.message?.content) ? event.message.content : null;
}

function parseCursor(events) {
  const state = { actions: [], open: new Map(), completed: new Set(), final: "" };
  for (const event of events) {
    if (event.type === "system" && event.subtype === "init") continue;
    const blocks = cursorBlocks(event);
    if (!blocks || !["assistant", "user"].includes(event.type)) {
      fail(`unsupported Cursor event ${JSON.stringify(event.type ?? null)}`);
    }
    for (const block of blocks) {
      if (!plainObject(block)) fail("Cursor content block must be an object");
      if (event.type === "assistant" && block.type === "text") {
        if (typeof block.text !== "string") fail("Cursor text block is missing text");
        state.final = block.text;
      } else if (event.type === "assistant" && block.type === "tool_use") {
        const tool = normalizeToolName(block.name);
        const shell = ["shell", "bash", "exec_command"].includes(String(block.name ?? "").toLowerCase());
        const path = shell ? sourceReadPath(block.input?.command) : null;
        beginAction(state, String(block.id ?? ""), {
          kind: path ? "source_read" : tool ?? (shell ? "shell" : "external_tool"),
          tool: path ? "source_read" : tool ?? String(block.name ?? "unknown"),
          args: block.input ?? {},
          path,
          command: shell ? String(block.input?.command ?? "") : null,
        });
      } else if (event.type === "user" && block.type === "tool_result") {
        completeAction(state, String(block.tool_use_id ?? ""), block.content ?? null, block.is_error === true);
      } else {
        fail(`unsupported Cursor content block ${JSON.stringify(block.type ?? null)}`);
      }
    }
  }
  return finishParsedState("Cursor", state);
}

function finishParsedState(host, state) {
  if (state.open.size > 0) fail(`${host} transcript has unmatched tool call ${JSON.stringify([...state.open.keys()][0])}`);
  if (!state.final) fail(`${host} transcript is missing a final agent message`);
  return { actions: state.actions, final: state.final };
}

export function parseInstalledTranscript(host, transcript) {
  const normalized = String(host ?? "").toLowerCase();
  const events = parseJsonLines(transcript, normalized || "unknown");
  if (normalized === "codex") return parseCodex(events);
  if (normalized === "cursor") return parseCursor(events);
  fail(`unsupported host ${JSON.stringify(host)}`);
}

function valueAt(object, dotted) {
  return dotted.split(".").reduce((value, key) => value?.[key], object);
}

function validateIdentityShape(identity, label) {
  if (!plainObject(identity)) fail(`${label} must be an object`);
  for (const field of INSTALLED_IDENTITY_FIELDS) {
    const value = valueAt(identity, field);
    if (value === undefined || value === null || value === "") fail(`${label}.${field} is required`);
  }
  for (const field of ["package.sha256", "launcher.sha256", "cli.sha256", "protocol.discovery_contract_sha256"]) {
    const value = valueAt(identity, field);
    if (!SHA256.test(String(value)) || /^0{64}$/u.test(String(value))) fail(`${label}.${field} must be a nonzero lowercase SHA-256`);
  }
  if (identity.package.name !== "codestory") fail(`${label}.package.name must be codestory`);
  if (identity.launcher.relative_path !== "scripts/codestory-mcp.cjs") {
    fail(`${label}.launcher.relative_path must be scripts/codestory-mcp.cjs`);
  }
  if (identity.cli.source !== "managed") fail(`${label}.cli.source must be managed`);
  if (!Number.isInteger(identity.publication.schema_version) || identity.publication.schema_version < 1) {
    fail(`${label}.publication.schema_version must be a positive integer`);
  }
}

function validateExactIdentity(installed, expected) {
  validateIdentityShape(expected, "expected identity");
  validateIdentityShape(installed, "installed identity");
  for (const field of INSTALLED_IDENTITY_FIELDS) {
    if (valueAt(installed, field) !== valueAt(expected, field)) {
      fail(`installed identity ${field} does not match the exact expected identity`);
    }
  }
}

function parseJsonText(value) {
  if (typeof value !== "string") return null;
  try {
    return JSON.parse(value);
  } catch {
    return null;
  }
}

function decodeResultEnvelope(value, depth = 0) {
  if (depth > 4) fail("tool result nesting exceeds the installed transcript contract");
  if (typeof value === "string") {
    const parsed = parseJsonText(value);
    return parsed === null ? value : decodeResultEnvelope(parsed, depth + 1);
  }
  if (Array.isArray(value) && value.length === 1 && value[0]?.type === "text") {
    return decodeResultEnvelope(value[0].text, depth + 1);
  }
  return value;
}

function normalizedResult(action) {
  const raw = decodeResultEnvelope(action.result);
  if (!plainObject(raw)) return { raw, body: parseJsonText(raw), meta: null, isError: action.error };
  const structured = plainObject(raw.structuredContent) ? raw.structuredContent : null;
  const textBlock = Array.isArray(raw.content)
    ? raw.content.find((entry) => entry?.type === "text" && typeof entry.text === "string")
    : null;
  const textBody = parseJsonText(textBlock?.text);
  if (structured && textBlock && (!textBody || !equalJson(structured, textBody))) {
    fail(`${action.tool} structured and text results differ`);
  }
  return {
    raw,
    body: structured ?? textBody ?? (plainObject(raw) ? raw : null),
    meta: plainObject(raw._meta) ? raw._meta : null,
    isError: action.error || raw.isError === true,
  };
}

function validateResultIdentity(action, expected) {
  const normalized = normalizedResult(action);
  const publication = normalized.meta?.codestory_publication;
  const protocol = normalized.meta?.codestory_protocol;
  const runtime = publication?.contract_runtime;
  const mismatches = [
    ["package.version", runtime?.plugin_version, expected.package.version],
    ["cli.pinned_version", runtime?.plugin_cli_version, expected.cli.version],
    ["cli.version", runtime?.cli_version, expected.cli.version],
    ["cli.sha256", runtime?.cli_sha256, expected.cli.sha256],
    ["cli.source", runtime?.cli_source, expected.cli.source],
    ["publication.schema_version", publication?.schema_version, expected.publication.schema_version],
    ["protocol.revision", protocol?.negotiated, expected.protocol.revision],
    ["protocol.discovery_contract_sha256", protocol?.discovery_contract_sha256, expected.protocol.discovery_contract_sha256],
  ];
  for (const [field, observed, wanted] of mismatches) {
    if (observed !== wanted) fail(`${action.tool} result identity ${field} does not match installed identity`);
  }
  if (runtime?.pinned_pair_matches !== true || runtime?.known_override_skew_channel !== false) {
    fail(`${action.tool} result identity does not prove one pinned managed runtime`);
  }
  return normalized;
}

function actionName(action) {
  return action.kind;
}

function validateActionOrder(scenarioContract, actions) {
  if (scenarioContract.expected_first_tool === "none") {
    if (actions.length > 0) fail(`${scenarioContract.id} expected no tool but observed ${actionName(actions[0])}`);
    return;
  }
  if (actions.length === 0) fail(`${scenarioContract.id} expected first tool ${scenarioContract.expected_first_tool} but observed none`);
  if (actionName(actions[0]) !== scenarioContract.expected_first_tool) {
    fail(`${scenarioContract.id} expected first tool ${scenarioContract.expected_first_tool} but observed ${actionName(actions[0])}`);
  }
  for (const action of actions.slice(1)) {
    if (!scenarioContract.permitted_followups.includes(actionName(action))) {
      fail(`${scenarioContract.id} follow-up ${actionName(action)} is not permitted`);
    }
  }
  for (const action of actions) {
    if (scenarioContract.forbidden_tools.includes(actionName(action)) || !ROUTING_ACTIONS.includes(actionName(action))) {
      fail(`${scenarioContract.id} used forbidden tool ${actionName(action)}`);
    }
  }
}

function validateSourceReads(scenarioContract, request, actions, results) {
  const reads = actions.filter((action) => action.kind === "source_read");
  const kind = scenarioContract.source_read_authorization.kind;
  if (kind === "none") {
    if (reads.length > 0) fail(`${scenarioContract.id} source read is not authorized`);
    return;
  }
  if (reads.length === 0) fail(`${scenarioContract.id} requires one authorized source read`);
  if (kind === "user_named_file") {
    const named = new Set((request.named_files ?? []).map(normalizePath));
    for (const read of reads) {
      if (!named.has(read.path)) fail(`${scenarioContract.id} source read is not authorized by a user-named file`);
    }
    return;
  }
  const packet = actions.find((action) => action.kind === "packet");
  const body = packet ? results.get(packet)?.body : null;
  if (kind === "packet_evidence_gap") {
    if (body?.status !== "no_useful_evidence") fail(`${scenarioContract.id} source read lacks a terminal packet evidence gap`);
    const allowed = new Set(
      (body?.gaps ?? []).flatMap((gap) => gap?.authorized_source_paths ?? []).map(normalizePath),
    );
    for (const read of reads) {
      if (!allowed.has(read.path)) fail(`${scenarioContract.id} source read is not authorized by the packet evidence gap`);
    }
    return;
  }
  if (kind === "packet_unavailable") {
    if (body?.status !== "unavailable") fail(`${scenarioContract.id} source read requires packet Unavailable`);
    return;
  }
  fail(`${scenarioContract.id} has unsupported source-read authorization ${kind}`);
}

function stripProject(args) {
  if (!plainObject(args)) return args;
  const { project: _project, ...contract } = args;
  return contract;
}

function validTypedContract(contract) {
  return plainObject(contract)
    && typeof contract.source_text === "string"
    && contract.source_text.length > 0
    && Array.isArray(contract.clauses)
    && contract.clauses.length > 0
    && plainObject(contract.spec);
}

function validateProofCalls(scenarioContract, request, actions, results) {
  const proofCalls = actions.filter((action) => action.kind === "prove_call_path");
  if (proofCalls.length > 1) fail(`${scenarioContract.id} proof may be called only once; selector relaxation and retries are forbidden`);
  if (proofCalls.length === 0) {
    if (["valid", "malformed"].includes(scenarioContract.typed_contract)) {
      fail(`${scenarioContract.id} did not call the typed verifier`);
    }
    return;
  }
  if (!plainObject(request.proof_contract)) {
    fail(`${scenarioContract.id} proof requires a host-supplied typed contract; free-English construction is forbidden`);
  }
  if (!equalJson(stripProject(proofCalls[0].args), request.proof_contract)) {
    fail(`${scenarioContract.id} proof request must preserve the host-supplied typed contract exactly`);
  }
  const isValid = validTypedContract(request.proof_contract);
  if (scenarioContract.typed_contract === "valid" && !isValid) fail(`${scenarioContract.id} requires a complete typed contract`);
  if (scenarioContract.typed_contract === "malformed" && isValid) fail(`${scenarioContract.id} requires the malformed-contract boundary`);

  const projection = results.get(proofCalls[0]);
  const activated = projection?.meta?.codestory_execution?.semantic_retrieval_activated;
  if (activated !== false) fail(`${scenarioContract.id} proof activated semantic retrieval or omitted observational evidence`);
  if (scenarioContract.typed_contract === "malformed") {
    if (!projection?.isError || projection.raw?.structuredContent !== undefined) {
      fail(`${scenarioContract.id} malformed semantic contract must return isError without structured content`);
    }
    return;
  }
  if (projection?.isError) fail(`${scenarioContract.id} typed proof unexpectedly returned a tool error`);
}

function validatePacketContinuation(scenarioContract, actions, results) {
  const packets = actions.filter((action) => action.kind === "packet");
  if (packets.length > 2) fail(`${scenarioContract.id} allows at most one packet continuation`);
  if (packets.length < 2) return;
  const first = results.get(packets[0])?.body;
  if (first?.status !== "continuation_available") fail(`${scenarioContract.id} repeated packet without a continuation offer`);
  const expected = {
    project: packets[0].args.project,
    question: packets[0].args.question,
    parent_packet_id: first?.continuation?.continuation_id,
    option_ids: first?.continuation?.gap_ids,
    core_generation_id: first?.publication?.core_generation_id,
    retrieval_generation_id: first?.publication?.retrieval_generation_id,
  };
  if (!equalJson(packets[1].args, expected)) fail(`${scenarioContract.id} packet continuation arguments do not match the pinned offer`);
}

function validateSelectedContext(scenarioContract, request, actions) {
  const contexts = actions.filter((action) => action.kind === "context");
  if (contexts.length === 0) return;
  if (typeof request.selected_target !== "string" || !request.selected_target) {
    fail(`${scenarioContract.id} context requires one host-selected target`);
  }
  for (const action of contexts) {
    if (action.args?.selector?.canonical_id !== request.selected_target) {
      fail(`${scenarioContract.id} context selector does not match the selected target`);
    }
  }
}

function validateHiddenDiscovery(scenarioContract, actions, results) {
  const searches = actions.filter((action) => action.kind === "tool_search");
  if (scenarioContract.id !== "hidden_proof_tool_discovery") {
    if (searches.length > 0) fail(`${scenarioContract.id} hidden-tool discovery is forbidden`);
    return;
  }
  if (searches.length !== 1) fail(`${scenarioContract.id} requires exactly one hidden-tool discovery`);
  const search = searches[0];
  if (search.args?.query !== "codestory mcp prove_call_path") {
    fail(`${scenarioContract.id} hidden-tool discovery must name only prove_call_path`);
  }
  const searchBody = results.get(search)?.body;
  const tools = plainObject(searchBody) && Array.isArray(searchBody.tools) ? searchBody.tools : [];
  if (!equalJson(tools, ["mcp__codestory__prove_call_path"])) {
    fail(`${scenarioContract.id} hidden-tool discovery returned tools outside prove_call_path`);
  }
}

function proofDisposition(actions, results) {
  const proof = actions.find((action) => action.kind === "prove_call_path");
  if (!proof) return null;
  const kind = results.get(proof)?.body?.disposition?.kind;
  return typeof kind === "string" ? kind : null;
}

function materialGaps(actions, results) {
  const gaps = [];
  for (const action of actions) {
    if (!["search", "context", "packet", "prove_call_path"].includes(action.kind)) continue;
    const body = results.get(action)?.body;
    const collections = [body?.gaps, body?.disposition?.gaps];
    for (const entries of collections) {
      if (!Array.isArray(entries)) continue;
      for (const gap of entries) {
        const id = gap?.gap_id ?? gap?.code;
        if (typeof id === "string" && id) gaps.push(id);
      }
    }
  }
  return [...new Set(gaps)];
}

function validateFinalClaims(scenarioContract, final, actions, results) {
  const lower = final.toLowerCase();
  for (const term of scenarioContract.final_claim_constraints.required_terms) {
    if (!lower.includes(term.toLowerCase())) fail(`${scenarioContract.id} missing required final claim ${term}`);
  }
  for (const term of scenarioContract.final_claim_constraints.forbidden_terms) {
    if (lower.includes(term.toLowerCase())) fail(`${scenarioContract.id} final claim violates forbidden term ${term}`);
  }
  if (/execut(?:e|ed|es|ion) at runtime|runtime reach(?:es|able)|will execute/iu.test(final)) {
    fail(`${scenarioContract.id} final claim violates the no-runtime-claim boundary`);
  }
  for (const gap of materialGaps(actions, results)) {
    if (!lower.includes(gap.toLowerCase())) fail(`${scenarioContract.id} missing required final claim for material gap ${gap}`);
  }
  const observed = proofDisposition(actions, results);
  const expected = scenarioContract.final_claim_constraints.proof_disposition;
  if (observed === null && /\b(?:verified|certified|definitively|proven|refuted)\b/iu.test(final)) {
    fail(`${scenarioContract.id} final claim violates retrieval/proof authority separation`);
  }
  if (expected !== null && observed !== expected) {
    fail(`${scenarioContract.id} expected proof disposition ${expected} but observed ${observed ?? "none"}`);
  }
  if (observed !== null && !PROOF_DISPOSITIONS.has(observed)) fail(`${scenarioContract.id} observed unknown proof disposition ${observed}`);
  if (observed === "unknown" && /\b(absent|does not exist|never happens|cannot call)\b/iu.test(final)) {
    if (!lower.includes("does not establish absence")) fail(`${scenarioContract.id} Unknown must not become absence`);
  }
}

export function validateInstalledSession({
  host,
  scenarioId,
  request,
  installedIdentity,
  expectedIdentity,
  transcript,
}) {
  const scenarioContract = SCENARIOS_BY_ID.get(scenarioId);
  if (!scenarioContract) fail(`unknown routing scenario ${JSON.stringify(scenarioId)}`);
  if (!plainObject(request)) fail(`${scenarioId} request must be an object`);
  validateExactIdentity(installedIdentity, expectedIdentity);
  const parsed = parseInstalledTranscript(host, transcript);
  validateActionOrder(scenarioContract, parsed.actions);

  const results = new Map();
  for (const action of parsed.actions) {
    if (!action.completed) fail(`${scenarioId} has an incomplete ${action.tool} action`);
    if (["search", "context", "packet", "prove_call_path"].includes(action.kind)) {
      results.set(action, validateResultIdentity(action, expectedIdentity));
    } else {
      results.set(action, normalizedResult(action));
    }
    const expectedSemanticError = scenarioContract.typed_contract === "malformed"
      && action.kind === "prove_call_path";
    if (results.get(action).isError && !expectedSemanticError) {
      fail(`${scenarioId} has an unexpected failed ${action.tool} action`);
    }
  }

  validateSourceReads(scenarioContract, request, parsed.actions, results);
  validateProofCalls(scenarioContract, request, parsed.actions, results);
  validatePacketContinuation(scenarioContract, parsed.actions, results);
  validateSelectedContext(scenarioContract, request, parsed.actions);
  validateHiddenDiscovery(scenarioContract, parsed.actions, results);
  validateFinalClaims(scenarioContract, parsed.final, parsed.actions, results);

  return {
    schema_version: 1,
    status: "pass",
    host: String(host).toLowerCase(),
    scenario_id: scenarioId,
    identity_binding: "exact",
    actions: parsed.actions.map(actionName),
    proof_disposition: proofDisposition(parsed.actions, results),
  };
}

async function fileSha256(path) {
  return createHash("sha256").update(await readFile(path)).digest("hex");
}

async function readJson(path, label) {
  let value;
  try {
    value = JSON.parse(await readFile(path, "utf8"));
  } catch (error) {
    fail(`${label} is not readable canonical JSON: ${error.message}`);
  }
  if (!plainObject(value)) fail(`${label} must be a JSON object`);
  return value;
}

export async function validateStaticHostParity(pluginRoot, expectedIdentity) {
  validateIdentityShape(expectedIdentity, "expected identity");
  const root = resolve(pluginRoot);
  const portable = await readJson(resolve(root, "plugin.json"), "portable plugin manifest");
  const pin = await readJson(resolve(root, "cli-version.json"), "CLI version pin");
  const catalog = await readJson(resolve(root, "generated-mcp-catalog.json"), "generated MCP catalog");
  const mcp = await readJson(resolve(root, "mcp.json"), "portable MCP manifest");
  const launcherPath = resolve(root, expectedIdentity.launcher.relative_path);
  const launcherSha256 = await fileSha256(launcherPath);

  if (portable.name !== expectedIdentity.package.name || portable.version !== expectedIdentity.package.version) {
    fail("portable package metadata does not match expected package identity");
  }
  if (pin.cli_version !== expectedIdentity.cli.version) fail("CLI pin does not match expected CLI identity");
  if (launcherSha256 !== expectedIdentity.launcher.sha256) fail("launcher bytes do not match expected launcher identity");
  if (catalog.wireContract?.publicationStampSchemaVersion !== expectedIdentity.publication.schema_version) {
    fail("catalog schema does not match expected publication identity");
  }
  if (catalog.wireContract?.preferredMcpProtocolVersion !== expectedIdentity.protocol.revision) {
    fail("catalog revision does not match expected protocol identity");
  }
  if (catalog.wireContract?.discoveryContracts?.[expectedIdentity.protocol.revision]
      !== expectedIdentity.protocol.discovery_contract_sha256) {
    fail("catalog discovery digest does not match expected discovery identity");
  }
  const server = mcp.mcpServers?.codestory;
  if (server?.command !== "node"
      || !Array.isArray(server.args)
      || server.args.length !== 1
      || server.args[0] !== "${PLUGIN_ROOT}/scripts/codestory-mcp.cjs") {
    fail("portable MCP metadata does not bind the canonical launcher");
  }

  const hosts = [];
  for (const [host, inputs] of Object.entries(STATIC_PARITY_HOSTS)) {
    const metadataPath = resolve(root, inputs.metadata);
    const hookPath = resolve(root, inputs.hook);
    const rulePath = resolve(root, inputs.rule);
    const metadata = await readJson(metadataPath, `${host} metadata`);
    const hookText = await readFile(hookPath, "utf8");
    const ruleText = await readFile(rulePath, "utf8");
    if (metadata.name !== "codestory" || metadata.version !== portable.version) {
      fail(`${host} metadata does not match the portable package`);
    }
    const expectedHook = host === "claude_code" ? `./${inputs.hook}` : inputs.hook;
    if (metadata.hooks !== expectedHook) fail(`${host} metadata does not bind its declared hook`);
    if (host.startsWith("copilot") && metadata.skills !== "skills/") {
      fail(`${host} metadata does not bind the canonical rule/skill directory`);
    }
    if (!hookText.includes("codestory-activate.cjs")) fail(`${host} hook does not bind the canonical activation hook`);
    if (!ruleText.includes("# CodeStory Grounding")) fail(`${host} rule input is not the canonical grounding skill`);
    hosts.push({
      host,
      package_version: portable.version,
      package_sha256: expectedIdentity.package.sha256,
      launcher_sha256: launcherSha256,
      metadata_sha256: await fileSha256(metadataPath),
      hook_sha256: createHash("sha256").update(hookText).digest("hex"),
      rule_sha256: createHash("sha256").update(ruleText).digest("hex"),
      model_routing_evaluated: false,
    });
  }
  return { schema_version: 1, status: "pass", hosts };
}

function parseOptions(argv) {
  const allowed = new Set([
    "--host",
    "--scenario",
    "--request",
    "--transcript",
    "--installed-identity",
    "--expected-identity",
    "--plugin-root",
    "--static-parity",
  ]);
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const key = argv[index];
    if (!allowed.has(key)) fail(`unknown option ${key}`);
    if (key === "--static-parity") {
      options.staticParity = true;
      continue;
    }
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) fail(`${key} requires a value`);
    options[key.slice(2).replaceAll("-", "_")] = value;
    index += 1;
  }
  return options;
}

async function readInputJson(path, label) {
  return readJson(resolve(path), label);
}

async function main(argv) {
  const options = parseOptions(argv);
  if (!options.expected_identity) fail("--expected-identity is required");
  const expectedIdentity = await readInputJson(options.expected_identity, "expected identity");
  let report;
  if (options.staticParity) {
    if (!options.plugin_root) fail("--plugin-root is required with --static-parity");
    report = await validateStaticHostParity(options.plugin_root, expectedIdentity);
  } else {
    for (const required of ["host", "scenario", "request", "transcript", "installed_identity"]) {
      if (!options[required]) fail(`--${required.replaceAll("_", "-")} is required`);
    }
    report = validateInstalledSession({
      host: options.host,
      scenarioId: options.scenario,
      request: await readInputJson(options.request, "request"),
      transcript: await readFile(resolve(options.transcript), "utf8"),
      installedIdentity: await readInputJson(options.installed_identity, "installed identity"),
      expectedIdentity,
    });
  }
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch((error) => {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  });
}
