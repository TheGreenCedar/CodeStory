#!/usr/bin/env node

import { createHash } from "node:crypto";
import { lstatSync, readFileSync, realpathSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { relative, resolve, sep } from "node:path";
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
  "installation.root",
  "receipt.relative_path",
  "receipt.sha256",
  "package.name",
  "package.version",
  "package.archive_relative_path",
  "package.sha256",
  "launcher.relative_path",
  "launcher.sha256",
  "cli.version",
  "cli.relative_path",
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
  const finalConstraints = {
    named_file_direct_read: { authority: "source", outcome: "supported" },
    exact_symbol_search: { authority: "search_lead", outcome: "discovery_only" },
    ambiguous_symbol_then_context: { authority: "context_evidence", outcome: "supported" },
    selected_target_context: { authority: "context_evidence", outcome: "supported" },
    broad_packet: { authority: "packet_evidence", outcome: "supported" },
    packet_single_continuation: { authority: "packet_evidence", outcome: "supported" },
    packet_gap_to_focused_source: { authority: "source", outcome: "supported" },
    packet_unavailable_to_source: { authority: "source", outcome: "unavailable" },
    typed_proof_contract_proven: { authority: "typed_proof", outcome: "supported", proof_disposition: "contract_proven" },
    typed_proof_contract_refuted: { authority: "typed_proof", outcome: "refuted", proof_disposition: "contract_refuted" },
    typed_proof_unknown: { authority: "typed_proof", outcome: "unknown", proof_disposition: "unknown" },
    typed_proof_unavailable: { authority: "typed_proof", outcome: "unavailable", proof_disposition: "unavailable" },
    malformed_proof_contract: { authority: "none", outcome: "invalid_contract" },
    refuse_free_english_proof: { authority: "none", outcome: "refused" },
    proof_observational: { authority: "typed_proof", outcome: "unknown", proof_disposition: "unknown" },
    hidden_proof_tool_discovery: { authority: "typed_proof", outcome: "supported", proof_disposition: "contract_proven" },
  }[id];
  return {
    id,
    expected_first_tool: first,
    required_action_sequence: first === "none" ? [] : [first, ...followups],
    permitted_followups: followups,
    forbidden_tools: ROUTING_ACTIONS.filter((item) => !allowed.has(item)),
    source_read_authorization: { kind: source },
    final_claim_constraints: finalConstraints,
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

const STATIC_ROSTER_PATHS = Object.freeze([
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
]);

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

function cursorText(event, role) {
  if (event?.message?.role !== role || !Array.isArray(event?.message?.content)) {
    fail(`Cursor ${role} event has invalid message content`);
  }
  let text = "";
  for (const block of event.message.content) {
    if (!plainObject(block) || block.type !== "text" || typeof block.text !== "string") {
      fail(`Cursor ${role} event contains a non-text block`);
    }
    text += block.text;
  }
  return text;
}

function unwrapCursorToolCall(event) {
  if (!plainObject(event.tool_call)) fail("Cursor tool_call event is missing tool_call");
  const keys = Object.keys(event.tool_call);
  if (keys.length !== 1 || !/^[A-Za-z][A-Za-z0-9]*ToolCall$/u.test(keys[0])) {
    fail("Cursor tool_call must contain exactly one *ToolCall payload");
  }
  const wrapper = event.tool_call[keys[0]];
  if (!plainObject(wrapper) || !plainObject(wrapper.args)) fail("Cursor tool_call payload is missing args");
  return { key: keys[0], wrapper };
}

function cursorStartedAction(callId, key, args) {
  if (key === "readToolCall") {
    return { kind: "source_read", tool: "source_read", path: normalizePath(args.path), args, cursor_key: key };
  }
  if (key === "mcpToolCall") {
    if (args.toolCallId !== callId || typeof args.providerIdentifier !== "string"
        || typeof args.toolName !== "string" || !plainObject(args.args)) {
      fail("Cursor mcpToolCall args are incomplete or do not match call_id");
    }
    const tool = normalizeToolName(args.toolName, args.providerIdentifier);
    return {
      kind: tool ?? "external_tool",
      tool: tool ?? args.toolName,
      args: args.args,
      server: args.providerIdentifier,
      cursor_key: key,
      cursor_args: args,
    };
  }
  if (key === "toolSearchToolCall") {
    if (typeof args.query !== "string" || !args.query) fail("Cursor toolSearchToolCall is missing query");
    return { kind: "tool_search", tool: "tool_search", args, cursor_key: key, cursor_args: args };
  }
  return { kind: "external_tool", tool: key, args, cursor_key: key, cursor_args: args };
}

function parseCursor(events) {
  const state = { actions: [], open: new Map(), completed: new Set(), final: "" };
  let initSeen = false;
  let userSeen = false;
  let terminalSeen = false;
  let sessionId = null;
  let assistantDeltas = "";
  let userText = "";
  events.forEach((event, index) => {
    if (terminalSeen) fail("Cursor terminal result must be the final stream event");
    if (typeof event.session_id !== "string" || !event.session_id) fail("Cursor event is missing session_id");
    if (sessionId === null) sessionId = event.session_id;
    if (event.session_id !== sessionId) fail("Cursor session_id changed within one transcript");

    if (event.type === "system") {
      if (index !== 0 || initSeen || event.subtype !== "init") fail("Cursor stream must begin with one system init event");
      initSeen = true;
      return;
    }
    if (!initSeen) fail("Cursor stream is missing system init");
    if (event.type === "user") {
      if (userSeen || state.actions.length > 0) fail("Cursor stream has duplicate or late user input");
      userText = cursorText(event, "user");
      if (!userText) fail("Cursor user input is empty");
      userSeen = true;
      return;
    }
    if (!userSeen) fail("Cursor stream is missing user input before agent activity");
    if (event.type === "assistant") {
      assistantDeltas += cursorText(event, "assistant");
      return;
    }
    if (event.type === "tool_call") {
      const callId = String(event.call_id ?? "");
      const { key, wrapper } = unwrapCursorToolCall(event);
      if (event.subtype === "started") {
        if (Object.hasOwn(wrapper, "result")) fail("Cursor started tool call must not contain a result");
        beginAction(state, callId, cursorStartedAction(callId, key, wrapper.args));
        return;
      }
      if (event.subtype !== "completed") fail(`unsupported Cursor tool_call subtype ${JSON.stringify(event.subtype)}`);
      const action = state.open.get(callId);
      if (!action) fail(`unmatched tool call result ${JSON.stringify(callId)}`);
      if (action.cursor_key !== key || !equalJson(action.cursor_args ?? action.args, wrapper.args)) {
        fail(`Cursor completed tool call ${JSON.stringify(callId)} does not match its start`);
      }
      if (!plainObject(wrapper.result) || Object.keys(wrapper.result).length !== 1
          || !Object.hasOwn(wrapper.result, "success")) {
        fail(`Cursor completed tool call ${JSON.stringify(callId)} must contain exactly one success result`);
      }
      if (event.truncated != null || wrapper.result.truncated != null) {
        fail(`Cursor completed tool call ${JSON.stringify(callId)} is partial`);
      }
      const success = wrapper.result.success;
      if (key === "readToolCall" && (!plainObject(success) || success.exceededLimit !== false)) {
        fail(`Cursor completed tool call ${JSON.stringify(callId)} contains a partial read`);
      }
      completeAction(state, callId, success, false);
      return;
    }
    if (event.type === "result") {
      terminalSeen = true;
      if (event.subtype !== "success" || event.is_error !== false || typeof event.result !== "string") {
        fail("Cursor terminal result must have subtype success and is_error false");
      }
      if (state.open.size > 0) fail(`Cursor terminal result has unmatched tool call ${JSON.stringify([...state.open.keys()][0])}`);
      if (assistantDeltas !== event.result) fail("Cursor assistant deltas do not match terminal result");
      state.final = event.result;
      return;
    }
    fail(`unsupported Cursor event ${JSON.stringify(event.type ?? null)}`);
  });
  if (!terminalSeen) fail("Cursor transcript is missing a terminal result");
  const parsed = finishParsedState("Cursor", state);
  return { ...parsed, user_text: userText };
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
  for (const field of ["receipt.sha256", "package.sha256", "launcher.sha256", "cli.sha256", "protocol.discovery_contract_sha256"]) {
    const value = valueAt(identity, field);
    if (!SHA256.test(String(value)) || /^0{64}$/u.test(String(value))) fail(`${label}.${field} must be a nonzero lowercase SHA-256`);
  }
  if (identity.package.name !== "codestory") fail(`${label}.package.name must be codestory`);
  if (identity.launcher.relative_path !== "scripts/codestory-mcp.cjs") {
    fail(`${label}.launcher.relative_path must be scripts/codestory-mcp.cjs`);
  }
  if (identity.cli.source !== "managed") fail(`${label}.cli.source must be managed`);
  for (const field of ["receipt.relative_path", "package.archive_relative_path", "launcher.relative_path", "cli.relative_path"]) {
    normalizePath(valueAt(identity, field));
  }
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

function sha256Bytes(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function fileInsideInstalledRoot(root, relativePath, label) {
  const normalized = normalizePath(relativePath);
  const lexical = resolve(root, normalized);
  let actual;
  try {
    if (!lstatSync(lexical).isFile()) fail(`${label} must be a regular file`);
    actual = realpathSync(lexical);
  } catch (error) {
    if (error instanceof ConformanceError) throw error;
    fail(`${label} is not readable: ${error.message}`);
  }
  const escaped = relative(root, actual);
  if (!escaped || escaped === ".." || escaped.startsWith(`..${sep}`) || resolve(root, escaped) !== actual) {
    fail(`${label} escapes the authenticated installed root`);
  }
  return actual;
}

function authenticateInstalledIdentity(installedRoot, installedReceipt, expected) {
  validateIdentityShape(expected, "expected identity");
  let root;
  try {
    root = realpathSync(installedRoot);
  } catch (error) {
    fail(`installed root is not readable: ${error.message}`);
  }
  if (root !== expected.installation.root) fail("installed identity installation.root does not match the authenticated root");
  const receiptPath = fileInsideInstalledRoot(root, expected.receipt.relative_path, "installed receipt");
  if (realpathSync(installedReceipt) !== receiptPath) fail("installed receipt path does not match the expected installed receipt");
  const receiptBytes = readFileSync(receiptPath);
  if (sha256Bytes(receiptBytes) !== expected.receipt.sha256) fail("installed identity receipt.sha256 does not match receipt bytes");
  let receipt;
  try {
    receipt = JSON.parse(receiptBytes.toString("utf8"));
  } catch {
    fail("installed receipt is invalid JSON");
  }
  if (!plainObject(receipt) || receipt.schema_version !== 1 || !plainObject(receipt.identity)) {
    fail("installed receipt must use schema_version 1 and contain identity");
  }
  const installed = { ...receipt.identity, receipt: expected.receipt };
  validateExactIdentity(installed, expected);
  const artifacts = [
    ["package", installed.package.archive_relative_path, installed.package.sha256],
    ["launcher", installed.launcher.relative_path, installed.launcher.sha256],
    ["cli", installed.cli.relative_path, installed.cli.sha256],
  ];
  for (const [label, path, digest] of artifacts) {
    const bytes = readFileSync(fileInsideInstalledRoot(root, path, `installed ${label}`));
    if (sha256Bytes(bytes) !== digest || digest !== valueAt(expected, `${label}.sha256`)) {
      fail(`installed identity ${label}.sha256 does not match authenticated ${label} bytes`);
    }
  }
  return installed;
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
  const observedSequence = actions.map(actionName);
  if (!equalJson(observedSequence, scenarioContract.required_action_sequence)) {
    fail(`${scenarioContract.id} required action sequence ${JSON.stringify(scenarioContract.required_action_sequence)} but observed ${JSON.stringify(observedSequence)}`);
  }
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

function requireExactKeys(value, keys, label) {
  if (!plainObject(value) || !equalJson(Object.keys(value).sort(), [...keys].sort())) {
    fail(`${label} does not match its required schema`);
  }
}

function nonemptyString(value) {
  return typeof value === "string" && value.length > 0;
}

function validateSelector(selector, label) {
  if (!plainObject(selector) || !nonemptyString(selector.kind)) fail(`${label} is invalid`);
  if (selector.kind === "canonical_id") {
    requireExactKeys(selector, ["kind", "canonical_id"], label);
    if (!nonemptyString(selector.canonical_id)) fail(`${label}.canonical_id is required`);
    return;
  }
  if (selector.kind === "qualified_name") {
    requireExactKeys(selector, ["kind", "qualified_name", "project_file_components"], label);
    if (!nonemptyString(selector.qualified_name)
        || !(selector.project_file_components === null || (Array.isArray(selector.project_file_components)
          && selector.project_file_components.length > 0))) fail(`${label} is invalid`);
    return;
  }
  if (selector.kind === "pinned_node") {
    requireExactKeys(selector, ["kind", "project_id", "core_generation_id", "core_run_id", "node_id"], label);
    if (![selector.project_id, selector.core_generation_id, selector.core_run_id, selector.node_id].every(nonemptyString)) {
      fail(`${label} is invalid`);
    }
    return;
  }
  fail(`${label} uses an unsupported selector kind`);
}

function validateScopeSelectorList(selectors, label) {
  selectors.forEach((selector, index) => validateSelector(selector, `${label} ${index}`));
}

function proofContractFieldKey(field, stepCount, prohibitionCount, exclusionCount, label) {
  if (!plainObject(field) || !nonemptyString(field.kind)) fail(`${label} is invalid`);
  if (field.kind === "start") {
    requireExactKeys(field, ["kind"], label);
    return "start";
  }
  if (["step_target", "directness", "ordering", "relation"].includes(field.kind)) {
    requireExactKeys(field, ["kind", "step"], label);
    if (!Number.isInteger(field.step) || field.step < 0 || field.step >= stepCount) fail(`${label}.step is invalid`);
    return `${field.kind}:${field.step}`;
  }
  if (["traversal_prohibition", "projection_exclusion"].includes(field.kind)) {
    requireExactKeys(field, ["kind", "index"], label);
    const limit = field.kind === "traversal_prohibition" ? prohibitionCount : exclusionCount;
    if (!Number.isInteger(field.index) || field.index < 0 || field.index >= limit) fail(`${label}.index is invalid`);
    return `${field.kind}:${field.index}`;
  }
  fail(`${label} uses an unsupported proof contract field`);
}

function validTypedContract(contract) {
  try {
    requireExactKeys(contract, ["source_text", "clauses", "spec"], "typed proof contract");
    if (!nonemptyString(contract.source_text) || !Array.isArray(contract.clauses) || contract.clauses.length === 0) {
      fail("typed proof contract source_text and clauses are required");
    }
    requireExactKeys(contract.spec, ["start", "steps", "prohibit_traversal_through", "exclude_from_projection"], "typed proof spec");
    validateSelector(contract.spec.start, "typed proof start selector");
    if (!Array.isArray(contract.spec.steps) || contract.spec.steps.length < 1 || contract.spec.steps.length > 6
        || !Array.isArray(contract.spec.prohibit_traversal_through)
        || !Array.isArray(contract.spec.exclude_from_projection)) fail("typed proof spec arrays are invalid");
    contract.spec.steps.forEach((step, index) => {
      requireExactKeys(step, ["relation", "target"], `typed proof step ${index}`);
      if (step.relation !== "direct_outgoing_call") fail(`typed proof step ${index} relation is invalid`);
      validateSelector(step.target, `typed proof step ${index} target`);
    });
    validateScopeSelectorList(contract.spec.prohibit_traversal_through, "typed proof traversal prohibition");
    validateScopeSelectorList(contract.spec.exclude_from_projection, "typed proof projection exclusion");

    const sourceBytes = Buffer.from(contract.source_text);
    const covered = new Uint8Array(sourceBytes.length);
    const resolvedFields = new Set();
    for (const [index, clause] of contract.clauses.entries()) {
      requireExactKeys(
        clause,
        ["start", "end", "clause_id", "quote", "classification", "fields", "reason", "non_material_kind"],
        `typed proof contract clause ${index}`,
      );
      if (!Number.isInteger(clause.start) || !Number.isInteger(clause.end) || clause.start < 0
          || clause.end <= clause.start || !nonemptyString(clause.clause_id) || typeof clause.quote !== "string"
          || !["resolved_material", "unresolved_material", "non_material"].includes(clause.classification)
          || !Array.isArray(clause.fields) || clause.end > sourceBytes.length) fail(`typed proof contract clause ${index} is invalid`);
      if (sourceBytes.subarray(clause.start, clause.end).toString("utf8") !== clause.quote) {
        fail(`typed proof contract clause ${index} quote does not match source bytes`);
      }
      covered.fill(1, clause.start, clause.end);
      const fields = clause.fields.map((field, fieldIndex) => proofContractFieldKey(
        field,
        contract.spec.steps.length,
        contract.spec.prohibit_traversal_through.length,
        contract.spec.exclude_from_projection.length,
        `typed proof contract clause ${index} field ${fieldIndex}`,
      ));
      if (new Set(fields).size !== fields.length) fail(`typed proof contract clause ${index} repeats a field`);
      if (clause.classification === "resolved_material") {
        if (fields.length === 0 || clause.reason !== null || clause.non_material_kind !== null) {
          fail(`typed proof contract clause ${index} resolved classification is invalid`);
        }
        fields.forEach((field) => resolvedFields.add(field));
      } else if (clause.classification === "unresolved_material") {
        if (fields.length !== 0 || !["missing_selector_resolution", "ambiguous_selector_resolution", "unsupported_interpretation"].includes(clause.reason)
            || clause.non_material_kind !== null) fail(`typed proof contract clause ${index} unresolved classification is invalid`);
      } else if (fields.length !== 0 || clause.reason !== null
          || !["whitespace", "punctuation", "connector", "commentary"].includes(clause.non_material_kind)) {
        fail(`typed proof contract clause ${index} non-material classification is invalid`);
      }
    }
    let byteOffset = 0;
    for (const character of contract.source_text) {
      const width = Buffer.byteLength(character);
      if (!/^\s$/u.test(character)) {
        for (let index = byteOffset; index < byteOffset + width; index += 1) {
          if (covered[index] !== 1) fail("typed proof contract leaves source text unclassified");
        }
      }
      byteOffset += width;
    }
    const requiredFields = ["start"];
    contract.spec.steps.forEach((_, index) => requiredFields.push(
      `step_target:${index}`, `directness:${index}`, `ordering:${index}`, `relation:${index}`,
    ));
    contract.spec.prohibit_traversal_through.forEach((_, index) => requiredFields.push(`traversal_prohibition:${index}`));
    contract.spec.exclude_from_projection.forEach((_, index) => requiredFields.push(`projection_exclusion:${index}`));
    if (requiredFields.some((field) => !resolvedFields.has(field))) fail("typed proof contract is missing required resolved fields");
    return true;
  } catch (error) {
    if (error instanceof ConformanceError) return false;
    throw error;
  }
}

function validatePublication(value, label) {
  requireExactKeys(value, ["core_generation_id", "retrieval_generation_id"], label);
  if (!nonemptyString(value.core_generation_id) || !nonemptyString(value.retrieval_generation_id)) fail(`${label} is invalid`);
}

function validateSearchResult(body) {
  requireExactKeys(body, ["kind", "publication", "leads", "gaps"], "search result");
  if (body.kind !== "complete" || !Array.isArray(body.leads) || body.leads.length === 0 || !Array.isArray(body.gaps)) {
    fail("search result is incomplete");
  }
  validatePublication(body.publication, "search result publication");
  body.leads.forEach((lead, index) => {
    requireExactKeys(lead, ["lead_id", "canonical_id"], `search result lead ${index}`);
    if (!nonemptyString(lead.lead_id) || !nonemptyString(lead.canonical_id)) fail(`search result lead ${index} is invalid`);
  });
  body.gaps.forEach((gap, index) => {
    requireExactKeys(gap, ["code"], `search result gap ${index}`);
    if (!nonemptyString(gap.code)) fail(`search result gap ${index} is invalid`);
  });
}

function validateContextResult(body) {
  requireExactKeys(body, ["kind", "publication", "target", "evidence", "gaps"], "context result");
  if (body.kind !== "complete" || !plainObject(body.target) || !nonemptyString(body.target.canonical_id)
      || !Array.isArray(body.evidence) || body.evidence.length === 0 || !Array.isArray(body.gaps)) {
    fail("context result is incomplete");
  }
  validatePublication(body.publication, "context result publication");
  requireExactKeys(body.target, ["canonical_id"], "context result target");
  body.evidence.forEach((entry, index) => {
    requireExactKeys(entry, ["evidence_id", "path"], `context result evidence ${index}`);
    if (!nonemptyString(entry.evidence_id) || !nonemptyString(entry.path)) fail(`context result evidence ${index} is invalid`);
  });
  body.gaps.forEach((gap, index) => {
    requireExactKeys(gap, ["code"], `context result gap ${index}`);
    if (!nonemptyString(gap.code)) fail(`context result gap ${index} is invalid`);
  });
}

function validatePacketResult(body) {
  const keys = ["kind", "packet_id", "status", "publication", "evidence", "gaps"];
  if (body?.status === "continuation_available") keys.push("continuation");
  if (body?.status === "unavailable") keys.splice(1, 1, "reason");
  requireExactKeys(body, keys, "packet result");
  if (!["complete", "budget_exceeded"].includes(body.kind)
      || !["complete", "continuation_available", "no_useful_evidence", "unavailable"].includes(body.status)
      || !Array.isArray(body.evidence) || !Array.isArray(body.gaps)) fail("packet result is incomplete");
  validatePublication(body.publication, "packet result publication");
  if (body.status !== "unavailable" && !nonemptyString(body.packet_id)) fail("packet result packet_id is required");
  if (body.status === "unavailable" && !nonemptyString(body.reason)) fail("packet result reason is required");
  body.evidence.forEach((entry, index) => {
    requireExactKeys(entry, ["evidence_id", "path"], `packet result evidence ${index}`);
    if (!nonemptyString(entry.evidence_id) || !nonemptyString(entry.path)) fail(`packet result evidence ${index} is invalid`);
  });
  body.gaps.forEach((gap, index) => {
    const gapKeys = Array.isArray(gap?.authorized_source_paths) ? ["gap_id", "authorized_source_paths"] : ["gap_id"];
    requireExactKeys(gap, gapKeys, `packet result gap ${index}`);
    if (!nonemptyString(gap.gap_id)
        || (gapKeys.length === 2 && (!gap.authorized_source_paths.every(nonemptyString)
          || gap.authorized_source_paths.length === 0))) fail(`packet result gap ${index} is invalid`);
  });
  if (body.status === "continuation_available") {
    requireExactKeys(body.continuation, ["continuation_id", "gap_ids"], "packet result continuation");
    if (!nonemptyString(body.continuation.continuation_id) || !Array.isArray(body.continuation.gap_ids)
        || body.continuation.gap_ids.length === 0) fail("packet result continuation is invalid");
  }
}

function proofIndex(value, length, label) {
  if (!Number.isInteger(value) || value < 0 || value >= length) fail(`${label} is out of range`);
  return value;
}

function validateProjectedProofSelector(selector, identities, label) {
  if (!plainObject(selector) || !nonemptyString(selector.kind)) fail(`${label} is invalid`);
  if (["pinned_node", "canonical_id", "qualified_name"].includes(selector.kind)) {
    validateSelector(selector, label);
    return;
  }
  if (["pinned_node_ref", "canonical_id_ref"].includes(selector.kind)) {
    requireExactKeys(selector, ["kind", "symbol"], label);
  } else if (selector.kind === "qualified_name_ref") {
    requireExactKeys(selector, ["kind", "symbol", "path_binding"], label);
    if (!["none", "exact_file"].includes(selector.path_binding)) fail(`${label}.path_binding is invalid`);
  } else {
    fail(`${label} uses an unsupported projected selector kind`);
  }
  proofIndex(selector.symbol, identities.symbols.length, `${label}.symbol`);
}

function validateProofClauseSchema(clause, index) {
  requireExactKeys(
    clause,
    ["start", "end", "clause_id", "quote", "classification", "fields", "reason", "non_material_kind"],
    `proof result clause ${index}`,
  );
  if (!Number.isInteger(clause.start) || clause.start < 0 || !Number.isInteger(clause.end) || clause.end <= clause.start
      || !nonemptyString(clause.clause_id) || typeof clause.quote !== "string" || !Array.isArray(clause.fields)
      || !["resolved_material", "unresolved_material", "non_material"].includes(clause.classification)) {
    fail(`proof result clause ${index} is invalid`);
  }
  clause.fields.forEach((field, fieldIndex) => proofContractFieldKey(
    field, 6, 256, 256, `proof result clause ${index} field ${fieldIndex}`,
  ));
}

function validateProofGap(gap, index) {
  const selectorKinds = ["selector_missing", "selector_ambiguous", "non_callable_selector"];
  const stepKinds = [
    "direct_call_missing", "recursive_call_not_representable", "source_window_too_large", "invalid_utf8",
    "source_line_out_of_range", "edge_containment_unproven", "missing_direct_call_receipt",
    "receipt_or_edge_already_used", "projection_exclusion_conflicts_with_required_receipt",
  ];
  if (selectorKinds.includes(gap?.kind)) {
    requireExactKeys(gap, ["kind", "selector_index"], `proof result gap ${index}`);
    if (!Number.isInteger(gap.selector_index) || gap.selector_index < 0 || gap.selector_index > 6) fail(`proof result gap ${index} is invalid`);
    return;
  }
  if (stepKinds.includes(gap?.kind)) {
    requireExactKeys(gap, ["kind", "step_index"], `proof result gap ${index}`);
    if (!Number.isInteger(gap.step_index) || gap.step_index < 0 || gap.step_index > 5) fail(`proof result gap ${index} is invalid`);
    return;
  }
  fail(`proof result gap ${index} has an unsupported kind`);
}

function validateProofReceipt(receipt, index, identities) {
  requireExactKeys(receipt, [
    "receipt_id", "edge_id", "source", "target", "evidence", "exact_callsite_start_byte",
    "callsite_identity", "column_or_ordinal", "containment", "line_window",
  ], `proof result receipt ${index}`);
  if (!nonemptyString(receipt.receipt_id) || !nonemptyString(receipt.edge_id)
      || !Number.isInteger(receipt.exact_callsite_start_byte) || receipt.exact_callsite_start_byte < 0
      || !nonemptyString(receipt.callsite_identity) || !Number.isInteger(receipt.column_or_ordinal)
      || receipt.column_or_ordinal < 0) {
    fail(`proof result receipt ${index} is invalid`);
  }
  proofIndex(receipt.source, identities.symbols.length, `proof result receipt ${index} source`);
  proofIndex(receipt.target, identities.symbols.length, `proof result receipt ${index} target`);
  proofIndex(receipt.evidence, identities.evidence.length, `proof result receipt ${index} evidence`);
  requireExactKeys(receipt.containment, ["file", "owner", "start_line", "end_line"], `proof result receipt ${index} containment`);
  requireExactKeys(receipt.line_window, ["kind", "file", "anchor_line", "byte_start", "byte_end", "text"], `proof result receipt ${index} line_window`);
  proofIndex(receipt.containment.file, identities.files.length, `proof result receipt ${index} containment.file`);
  proofIndex(receipt.containment.owner, identities.symbols.length, `proof result receipt ${index} containment.owner`);
  proofIndex(receipt.line_window.file, identities.files.length, `proof result receipt ${index} line_window.file`);
  if (receipt.line_window.kind !== "indexed_line_v1" || !Number.isInteger(receipt.line_window.anchor_line)
      || receipt.line_window.anchor_line < 1 || !Number.isInteger(receipt.line_window.byte_start)
      || receipt.line_window.byte_start < 0 || !Number.isInteger(receipt.line_window.byte_end)
      || receipt.line_window.byte_end < receipt.line_window.byte_start || typeof receipt.line_window.text !== "string") {
    fail(`proof result receipt ${index} line_window is invalid`);
  }
  if (receipt.line_window.byte_end - receipt.line_window.byte_start !== Buffer.byteLength(receipt.line_window.text)
      || receipt.containment.file !== receipt.line_window.file || receipt.containment.owner !== receipt.source
      || !Number.isInteger(receipt.containment.start_line) || receipt.containment.start_line < 1
      || !Number.isInteger(receipt.containment.end_line) || receipt.containment.end_line < receipt.containment.start_line) {
    fail(`proof result receipt ${index} containment or source window is inconsistent`);
  }
  const evidence = identities.evidence[receipt.evidence];
  if (evidence.caller !== receipt.source || evidence.target !== receipt.target || evidence.edge_id !== receipt.edge_id
      || evidence.callsite_identity !== receipt.callsite_identity) fail(`proof result receipt ${index} does not match exact-resolution evidence`);
}

function validateProofResult(body) {
  requireExactKeys(body, [
    "kind", "schema_version", "domain", "contract_interpretation", "guard_version",
    "source_text_sha256", "contract_digest", "core_publication", "identities", "spec",
    "clauses", "disposition", "steps", "receipts",
  ], "proof result");
  if (body.kind !== "complete" || body.schema_version !== 1 || body.domain !== "indexed_source_call_path_v1"
      || body.contract_interpretation !== "host_supplied" || body.guard_version !== "clause_guard_v1"
      || !SHA256.test(body.source_text_sha256) || !SHA256.test(body.contract_digest)
      || !plainObject(body.core_publication) || !plainObject(body.identities) || !plainObject(body.spec)
      || !Array.isArray(body.clauses) || body.clauses.length === 0 || !Array.isArray(body.steps) || !Array.isArray(body.receipts)
      || !plainObject(body.disposition)) fail("proof result is incomplete");
  requireExactKeys(body.core_publication, ["project_id", "generation_id", "run_id"], "proof result core_publication");
  if (![body.core_publication.project_id, body.core_publication.generation_id, body.core_publication.run_id].every(nonemptyString)) {
    fail("proof result core_publication is invalid");
  }
  requireExactKeys(body.identities, ["files", "symbols", "provenance_profiles", "evidence"], "proof result identities");
  if (![body.identities.files, body.identities.symbols, body.identities.provenance_profiles, body.identities.evidence].every(Array.isArray)) {
    fail("proof result identities are invalid");
  }
  body.identities.files.forEach((file, index) => {
    requireExactKeys(file, ["file_node_id", "project_file_components", "indexed_sha256", "observed_sha256"], `proof result file ${index}`);
    if (!(file.file_node_id === null || nonemptyString(file.file_node_id))
        || !(file.project_file_components === null || (Array.isArray(file.project_file_components)
          && file.project_file_components.every(nonemptyString)))
        || !(file.indexed_sha256 === null || SHA256.test(file.indexed_sha256))
        || !(file.observed_sha256 === null || SHA256.test(file.observed_sha256))) fail(`proof result file ${index} is invalid`);
  });
  body.identities.symbols.forEach((symbol, index) => {
    requireExactKeys(symbol, ["node_id", "canonical_id", "qualified_name", "file"], `proof result symbol ${index}`);
    if (!nonemptyString(symbol.node_id) || !(symbol.canonical_id === null || nonemptyString(symbol.canonical_id))
        || !(symbol.qualified_name === null || nonemptyString(symbol.qualified_name))) fail(`proof result symbol ${index} is invalid`);
    if (symbol.file !== null) proofIndex(symbol.file, body.identities.files.length, `proof result symbol ${index}.file`);
  });
  body.identities.provenance_profiles.forEach((profile, index) => {
    requireExactKeys(profile, ["producer", "fact_schema_version", "algorithm", "language_adapter", "language_adapter_version", "parser_fingerprint"], `proof result provenance profile ${index}`);
    if (profile.producer !== "codestory-internal" || profile.fact_schema_version !== 1
        || profile.algorithm !== "exact-call-resolution-v1" || !nonemptyString(profile.language_adapter)
        || !nonemptyString(profile.language_adapter_version) || !SHA256.test(profile.parser_fingerprint)) {
      fail(`proof result provenance profile ${index} is invalid`);
    }
  });
  body.identities.evidence.forEach((evidence, index) => {
    requireExactKeys(evidence, ["fact_id", "caller", "target", "edge_id", "callsite_identity", "chain", "provenance"], `proof result evidence ${index}`);
    if (!SHA256.test(evidence.fact_id) || !nonemptyString(evidence.edge_id) || !nonemptyString(evidence.callsite_identity)
        || !Array.isArray(evidence.chain) || !plainObject(evidence.provenance)) fail(`proof result evidence ${index} is invalid`);
    proofIndex(evidence.caller, body.identities.symbols.length, `proof result evidence ${index}.caller`);
    proofIndex(evidence.target, body.identities.symbols.length, `proof result evidence ${index}.target`);
    evidence.chain.forEach((entry, chainIndex) => {
      requireExactKeys(entry, ["kind", "symbols"], `proof result evidence ${index} chain ${chainIndex}`);
      if (!nonemptyString(entry.kind) || !Array.isArray(entry.symbols)) fail(`proof result evidence ${index} chain ${chainIndex} is invalid`);
      entry.symbols.forEach((symbol) => proofIndex(symbol, body.identities.symbols.length, `proof result evidence ${index} chain ${chainIndex} symbol`));
    });
    requireExactKeys(evidence.provenance, ["profile", "dependency_files", "evidence_sha256"], `proof result evidence ${index} provenance`);
    proofIndex(evidence.provenance.profile, body.identities.provenance_profiles.length, `proof result evidence ${index} provenance.profile`);
    if (!Array.isArray(evidence.provenance.dependency_files) || !SHA256.test(evidence.provenance.evidence_sha256)) {
      fail(`proof result evidence ${index} provenance is invalid`);
    }
    evidence.provenance.dependency_files.forEach((file) => proofIndex(file, body.identities.files.length, `proof result evidence ${index} dependency file`));
  });
  requireExactKeys(body.spec, ["start", "steps", "prohibit_traversal_through", "exclude_from_projection"], "proof result spec");
  validateProjectedProofSelector(body.spec.start, body.identities, "proof result start selector");
  if (!Array.isArray(body.spec.steps) || body.spec.steps.length < 1 || body.spec.steps.length > 6
      || !Array.isArray(body.spec.prohibit_traversal_through) || !Array.isArray(body.spec.exclude_from_projection)) {
    fail("proof result spec is invalid");
  }
  body.spec.steps.forEach((step, index) => {
    requireExactKeys(step, ["relation", "target"], `proof result spec step ${index}`);
    if (step.relation !== "direct_outgoing_call") fail(`proof result spec step ${index} is invalid`);
    validateProjectedProofSelector(step.target, body.identities, `proof result spec step ${index} target`);
  });
  validateScopeSelectorList(body.spec.prohibit_traversal_through, "proof result traversal prohibition");
  validateScopeSelectorList(body.spec.exclude_from_projection, "proof result projection exclusion");
  body.clauses.forEach(validateProofClauseSchema);
  if (body.disposition.contract_digest !== body.contract_digest || !PROOF_DISPOSITIONS.has(body.disposition.kind)) {
    fail("proof result disposition is invalid");
  }
  const disposition = body.disposition;
  if (disposition.kind === "contract_proven") {
    requireExactKeys(disposition, ["kind", "contract_digest", "receipts"], "proof result ContractProven disposition");
    if (!Array.isArray(disposition.receipts) || disposition.receipts.length === 0 || new Set(disposition.receipts).size !== disposition.receipts.length) {
      fail("proof result ContractProven receipts are missing");
    }
  } else if (disposition.kind === "contract_refuted") {
    requireExactKeys(disposition, ["kind", "contract_digest", "refutation"], "proof result ContractRefuted disposition");
    const refutation = disposition.refutation;
    if (refutation?.kind === "prohibited_scope_traversal") {
      requireExactKeys(refutation, ["kind", "step_index", "prohibition_index", "connected_receipts"], "proof result refutation basis");
      if (!Number.isInteger(refutation.prohibition_index) || refutation.prohibition_index < 0
          || refutation.prohibition_index >= body.spec.prohibit_traversal_through.length) fail("proof result refutation basis is invalid");
    } else if (refutation?.kind === "certified_absence") {
      requireExactKeys(refutation, ["kind", "step_index", "extractor_capability_receipt_id", "untruncated_enumeration_receipt_id", "connected_receipts"], "proof result refutation basis");
      if (!nonemptyString(refutation.extractor_capability_receipt_id)
          || !nonemptyString(refutation.untruncated_enumeration_receipt_id)) fail("proof result refutation basis is invalid");
    } else {
      fail("proof result refutation basis is missing");
    }
    if (!Number.isInteger(refutation.step_index) || refutation.step_index < 0 || refutation.step_index >= body.spec.steps.length
        || !Array.isArray(refutation.connected_receipts)) fail("proof result refutation basis is invalid");
  } else if (disposition.kind === "unknown") {
    requireExactKeys(disposition, ["kind", "contract_digest", "gaps", "connected_receipts"], "proof result Unknown disposition");
    if (!Array.isArray(disposition.gaps) || disposition.gaps.length === 0 || !Array.isArray(disposition.connected_receipts)) {
      fail("proof result Unknown gaps are missing");
    }
    disposition.gaps.forEach(validateProofGap);
  } else {
    requireExactKeys(disposition, ["kind", "contract_digest", "reasons"], "proof result Unavailable disposition");
    if (!Array.isArray(disposition.reasons) || disposition.reasons.length === 0 || !disposition.reasons.every(nonemptyString)) {
      fail("proof result Unavailable reasons are missing");
    }
  }
  body.receipts.forEach((receipt, index) => validateProofReceipt(receipt, index, body.identities));
  const receiptReferences = disposition.kind === "contract_proven"
    ? disposition.receipts
    : disposition.kind === "contract_refuted" ? disposition.refutation.connected_receipts
      : disposition.kind === "unknown" ? disposition.connected_receipts : [];
  receiptReferences.forEach((receipt) => proofIndex(receipt, body.receipts.length, "proof result disposition receipt"));
  if (body.steps.length !== body.spec.steps?.length) fail("proof result steps do not match spec");
  body.steps.forEach((step, index) => {
    requireExactKeys(step, ["step_index", "status", "receipt"], `proof result step ${index}`);
    if (step.step_index !== index || !["proven", "positive_contradiction", "certified_absence", "unavailable", "unknown"].includes(step.status)
        || !(step.receipt === null || Number.isInteger(step.receipt))) fail(`proof result step ${index} is invalid`);
    if (step.receipt !== null) proofIndex(step.receipt, body.receipts.length, `proof result step ${index}.receipt`);
  });
}

function validateToolResultSchema(action, projection) {
  if (!plainObject(projection.body)) fail(`${action.tool} result is not a JSON object`);
  if (action.kind === "search") validateSearchResult(projection.body);
  if (action.kind === "context") validateContextResult(projection.body);
  if (action.kind === "packet") validatePacketResult(projection.body);
  if (action.kind === "prove_call_path" && !projection.isError) validateProofResult(projection.body);
}

function projectedSelectorValue(selector, result, label) {
  if (["pinned_node", "canonical_id", "qualified_name"].includes(selector.kind)) return selector;
  const symbol = result.identities.symbols[selector.symbol];
  if (selector.kind === "canonical_id_ref") {
    if (!nonemptyString(symbol.canonical_id)) fail(`${label} canonical identity is unavailable`);
    return { kind: "canonical_id", canonical_id: symbol.canonical_id };
  }
  if (selector.kind === "pinned_node_ref") {
    return {
      kind: "pinned_node",
      project_id: result.core_publication.project_id,
      core_generation_id: result.core_publication.generation_id,
      core_run_id: result.core_publication.run_id,
      node_id: symbol.node_id,
    };
  }
  if (!nonemptyString(symbol.qualified_name)) fail(`${label} qualified identity is unavailable`);
  let projectFileComponents = null;
  if (selector.path_binding === "exact_file") {
    if (symbol.file === null) fail(`${label} exact file binding is unavailable`);
    projectFileComponents = result.identities.files[symbol.file].project_file_components;
    if (!Array.isArray(projectFileComponents)) fail(`${label} exact file binding is invalid`);
  }
  return {
    kind: "qualified_name",
    qualified_name: symbol.qualified_name,
    project_file_components: projectFileComponents,
  };
}

function validateProofResultAgainstRequest(result, contract) {
  if (result.source_text_sha256 !== sha256Bytes(Buffer.from(contract.source_text))) {
    fail("proof result source_text_sha256 does not match the host-supplied contract");
  }
  if (!equalJson(result.clauses, contract.clauses)) fail("proof result clauses do not match the host-supplied contract");
  if (!equalJson(projectedSelectorValue(result.spec.start, result, "proof result start selector"), contract.spec.start)) {
    fail("proof result start selector does not match the host-supplied contract");
  }
  if (result.spec.steps.length !== contract.spec.steps.length) fail("proof result steps do not match the host-supplied contract");
  result.spec.steps.forEach((step, index) => {
    const projected = {
      relation: step.relation,
      target: projectedSelectorValue(step.target, result, `proof result step ${index} target`),
    };
    if (!equalJson(projected, contract.spec.steps[index])) fail(`proof result step ${index} does not match the host-supplied contract`);
  });
  if (!equalJson(result.spec.prohibit_traversal_through, contract.spec.prohibit_traversal_through)
      || !equalJson(result.spec.exclude_from_projection, contract.spec.exclude_from_projection)) {
    fail("proof result scope selectors do not match the host-supplied contract");
  }
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
  validateProofResultAgainstRequest(projection.body, request.proof_contract);
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

function validateSelectedContext(scenarioContract, request, actions, results) {
  const contexts = actions.filter((action) => action.kind === "context");
  if (contexts.length === 0) return;
  if (typeof request.selected_target !== "string" || !request.selected_target) {
    fail(`${scenarioContract.id} context requires one host-selected target`);
  }
  for (const action of contexts) {
    if (action.args?.selector?.canonical_id !== request.selected_target) {
      fail(`${scenarioContract.id} context selector does not match the selected target`);
    }
    if (results.get(action)?.body?.target?.canonical_id !== request.selected_target) {
      fail(`${scenarioContract.id} context result does not match the selected target`);
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

const FINAL_CLAIM_KEYS = Object.freeze([
  "authority",
  "outcome",
  "target_id",
  "evidence_ids",
  "gap_ids",
  "reason_codes",
  "proof_disposition",
  "refutation_basis",
  "runtime_execution_claim",
  "absence_claim",
  "material_omissions",
]);

function uniqueStrings(values, label) {
  if (!Array.isArray(values) || !values.every(nonemptyString) || new Set(values).size !== values.length) {
    fail(`${label} must be unique nonempty strings`);
  }
  return values;
}

function parseFinalClaim(final, scenarioId) {
  const claim = parseJsonText(final);
  requireExactKeys(claim, FINAL_CLAIM_KEYS, `${scenarioId} final claim`);
  if (!nonemptyString(claim.authority) || !nonemptyString(claim.outcome)
      || !(claim.target_id === null || nonemptyString(claim.target_id))
      || !(claim.proof_disposition === null || PROOF_DISPOSITIONS.has(claim.proof_disposition))
      || !(claim.refutation_basis === null || nonemptyString(claim.refutation_basis))
      || typeof claim.runtime_execution_claim !== "boolean" || typeof claim.absence_claim !== "boolean") {
    fail(`${scenarioId} final claim has invalid typed fields`);
  }
  uniqueStrings(claim.evidence_ids, `${scenarioId} final claim evidence_ids`);
  uniqueStrings(claim.gap_ids, `${scenarioId} final claim gap_ids`);
  uniqueStrings(claim.reason_codes, `${scenarioId} final claim reason_codes`);
  if (!Array.isArray(claim.material_omissions)) fail(`${scenarioId} final claim material_omissions must be an array`);
  return claim;
}

function expectedFinalClaim(scenarioContract, actions, results) {
  const expected = {
    ...scenarioContract.final_claim_constraints,
    target_id: null,
    evidence_ids: [],
    gap_ids: [],
    reason_codes: [],
    proof_disposition: scenarioContract.final_claim_constraints.proof_disposition ?? null,
    refutation_basis: null,
    runtime_execution_claim: false,
    absence_claim: false,
    material_omissions: [],
  };
  const contexts = actions.filter((action) => action.kind === "context");
  const packets = actions.filter((action) => action.kind === "packet");
  const searches = actions.filter((action) => action.kind === "search");
  const proof = actions.find((action) => action.kind === "prove_call_path");
  const reads = actions.filter((action) => action.kind === "source_read");

  if (contexts.length > 0) {
    const body = results.get(contexts.at(-1)).body;
    expected.target_id = body.target.canonical_id;
    expected.evidence_ids = body.evidence.map(({ evidence_id }) => evidence_id);
  } else if (searches.length > 0) {
    const body = results.get(searches.at(-1)).body;
    if (body.leads.length === 1) expected.target_id = body.leads[0].canonical_id;
    expected.evidence_ids = body.leads.map(({ lead_id }) => lead_id);
  } else if (packets.length > 0) {
    expected.evidence_ids = packets.flatMap((action) => results.get(action).body.evidence.map(({ evidence_id }) => evidence_id));
  }
  if (reads.length > 0) expected.evidence_ids = reads.map(({ path }) => `source:${path}`);
  if (proof) {
    const disposition = results.get(proof).body?.disposition;
    if (disposition?.kind === "contract_proven") {
      const receipts = results.get(proof).body.receipts;
      expected.evidence_ids = disposition.receipts.map((index) => receipts[index]?.receipt_id);
    } else {
      expected.evidence_ids = [];
    }
    if (disposition?.kind === "contract_refuted") expected.refutation_basis = disposition.refutation.kind;
  }

  for (const action of [...searches, ...packets]) {
    expected.gap_ids.push(...results.get(action).body.gaps.map((gap) => gap.gap_id ?? gap.code));
  }
  if (proof) {
    const disposition = results.get(proof).body?.disposition;
    if (Array.isArray(disposition?.gaps)) expected.gap_ids.push(...disposition.gaps.map(({ kind }) => kind));
    if (Array.isArray(disposition?.reasons)) expected.reason_codes.push(...disposition.reasons);
    if (results.get(proof).isError && nonemptyString(results.get(proof).body?.code)) {
      expected.reason_codes.push(results.get(proof).body.code);
    }
  }
  for (const action of packets) {
    const reason = results.get(action).body.reason;
    if (nonemptyString(reason)) expected.reason_codes.push(reason);
  }
  if (scenarioContract.id === "refuse_free_english_proof") expected.reason_codes.push("typed_contract_required");
  expected.evidence_ids = [...new Set(expected.evidence_ids)];
  expected.gap_ids = [...new Set(expected.gap_ids.filter(nonemptyString))];
  expected.reason_codes = [...new Set(expected.reason_codes)];
  return expected;
}

function validateFinalClaims(scenarioContract, final, actions, results) {
  const claim = parseFinalClaim(final, scenarioContract.id);
  const expected = expectedFinalClaim(scenarioContract, actions, results);
  for (const key of FINAL_CLAIM_KEYS) {
    if (!equalJson(claim[key], expected[key])) {
      fail(`${scenarioContract.id} final claim ${key} does not match result-bound evidence`);
    }
  }
  if (claim.runtime_execution_claim) fail(`${scenarioContract.id} final claim makes a runtime execution claim`);
  if (claim.absence_claim) fail(`${scenarioContract.id} final claim absence_claim contradicts Unknown or retrieval authority`);
  if (claim.material_omissions.length > 0) fail(`${scenarioContract.id} final claim contains material omissions`);
}

export function validateInstalledSession({
  host,
  scenarioId,
  request,
  installedRoot,
  installedReceipt,
  expectedIdentity,
  transcript,
}) {
  const scenarioContract = SCENARIOS_BY_ID.get(scenarioId);
  if (!scenarioContract) fail(`unknown routing scenario ${JSON.stringify(scenarioId)}`);
  if (!plainObject(request)) fail(`${scenarioId} request must be an object`);
  authenticateInstalledIdentity(installedRoot, installedReceipt, expectedIdentity);
  const parsed = parseInstalledTranscript(host, transcript);
  if (String(host).toLowerCase() === "cursor" && parsed.user_text !== request.text) {
    fail(`${scenarioId} Cursor user text does not match the declared request`);
  }
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
    if (!expectedSemanticError && ["search", "context", "packet", "prove_call_path"].includes(action.kind)) {
      validateToolResultSchema(action, results.get(action));
    }
  }

  validateSourceReads(scenarioContract, request, parsed.actions, results);
  validateProofCalls(scenarioContract, request, parsed.actions, results);
  validatePacketContinuation(scenarioContract, parsed.actions, results);
  validateSelectedContext(scenarioContract, request, parsed.actions, results);
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
  requireExactKeys(expectedIdentity.static_roster, STATIC_ROSTER_PATHS, "expected static digest roster");
  for (const path of STATIC_ROSTER_PATHS) {
    const digest = expectedIdentity.static_roster[path];
    if (!SHA256.test(digest) || /^0{64}$/u.test(digest)) fail(`expected static digest roster ${path} is invalid`);
    if (await fileSha256(resolve(root, path)) !== digest) fail(`static digest roster ${path} does not match package bytes`);
  }
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
    const hook = await readJson(hookPath, `${host} hook`);
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
    if (host === "claude_code") {
      const sessionStart = hook.hooks?.SessionStart;
      if (!Array.isArray(sessionStart) || sessionStart.length !== 1
          || sessionStart[0].matcher !== "startup|resume|clear|compact"
          || !Array.isArray(sessionStart[0].hooks) || sessionStart[0].hooks.length !== 1) {
        fail("claude_code hook structure is invalid");
      }
      const command = sessionStart[0].hooks[0];
      requireExactKeys(command, ["type", "command", "commandWindows", "timeout", "statusMessage"], "claude_code hook command");
      if (command.type !== "command"
          || command.command !== "command -v node >/dev/null 2>&1 && node \"${CLAUDE_PLUGIN_ROOT}/hooks/codestory-activate.cjs\" || exit 0"
          || command.commandWindows !== "if (Get-Command node -ErrorAction SilentlyContinue) { node \"$env:CLAUDE_PLUGIN_ROOT\\hooks\\codestory-activate.cjs\" }"
          || command.timeout !== 300) fail("claude_code hook command is not the canonical launcher");
    } else {
      if (hook.version !== 1 || !Array.isArray(hook.hooks?.sessionStart) || hook.hooks.sessionStart.length !== 1) {
        fail(`${host} hook structure is invalid`);
      }
      const command = hook.hooks.sessionStart[0];
      requireExactKeys(command, ["type", "bash", "powershell", "timeoutSec"], `${host} hook command`);
      if (command.type !== "command"
          || command.bash !== "node \"${PLUGIN_ROOT}/hooks/codestory-activate.cjs\""
          || command.powershell !== "node \"${PLUGIN_ROOT}\\hooks\\codestory-activate.cjs\""
          || command.timeoutSec !== 300) fail(`${host} hook command is not the canonical launcher`);
    }
    if (!/^---\nname: codestory-grounding\n/iu.test(ruleText)
        || !ruleText.includes("## Direct Tool Loop")
        || !ruleText.includes("## Task Router")
        || !ruleText.includes("## Evidence Rules")
        || !ruleText.includes("`packet`")
        || !ruleText.includes("`context`")) {
      fail(`${host} rule input is not the complete canonical grounding contract`);
    }
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
    "--installed-root",
    "--installed-receipt",
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
    for (const required of ["host", "scenario", "request", "transcript", "installed_root", "installed_receipt"]) {
      if (!options[required]) fail(`--${required.replaceAll("_", "-")} is required`);
    }
    report = validateInstalledSession({
      host: options.host,
      scenarioId: options.scenario,
      request: await readInputJson(options.request, "request"),
      transcript: await readFile(resolve(options.transcript), "utf8"),
      installedRoot: resolve(options.installed_root),
      installedReceipt: resolve(options.installed_receipt),
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
