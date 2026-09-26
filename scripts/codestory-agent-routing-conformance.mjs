#!/usr/bin/env node

import { createHash } from "node:crypto";
import { lstatSync, readFileSync, realpathSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const SHA256 = /^[0-9a-f]{64}$/u;
const GENERATED_MCP_CATALOG = JSON.parse(readFileSync(
  new URL("../plugins/codestory/generated-mcp-catalog.json", import.meta.url),
  "utf8",
));
const ROUTING_CORPUS_DOCUMENT = JSON.parse(readFileSync(
  new URL("./fixtures/codestory-agent-routing-corpus-v1.json", import.meta.url),
  "utf8",
));
const GENERATED_TOOL_SCHEMAS = new Map(GENERATED_MCP_CATALOG.tools.map((tool) => [tool.name, tool]));
const ROUTING_ACTIONS = Object.freeze([
  "source_read",
  "search",
  "context",
  "packet",
  "tool_search",
]);
export const MCP_PROTOCOL_REVISIONS = Object.freeze([
  "2024-11-05",
  "2025-03-26",
  "2025-06-18",
  "2025-11-25",
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
  ...MCP_PROTOCOL_REVISIONS.map((revision) => `protocol.discovery_contracts.${revision}`),
]);

const IDENTITY_REQUIREMENTS = Object.freeze({
  mode: "exact",
  fields: INSTALLED_IDENTITY_FIELDS,
});

export const ROUTING_PACKET_QUESTIONS = deepFreeze({
  broad_packet: "Explain how routing_fixture::start reaches finish across the project.",
  packet_single_continuation: "Trace the complete routing flow and account for src/unread.rs if the index cannot cover it.",
  packet_gap_to_focused_source: "Investigate the missing route branch.",
  packet_named_fallback_to_source: "Explain how the routing catalog works.",
});

const ROUTING_SEARCH_QUERIES = deepFreeze({
  exact_symbol_search: "start",
  ambiguous_symbol_then_context: "Thing",
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
  optionalFollowups = [],
  optionalPrefixes = [],
  source = "none",
}) {
  const allowed = new Set([first, ...followups, ...optionalFollowups, ...optionalPrefixes]
    .filter((item) => item !== "none"));
  const finalConstraints = {
    named_file_direct_read: { authority: "source", outcome: "supported" },
    exact_symbol_search: { authority: "search_lead", outcome: "discovery_only" },
    ambiguous_symbol_then_context: { authority: "context_evidence", outcome: "supported" },
    selected_target_context: { authority: "context_evidence", outcome: "supported" },
    broad_packet: { authority: "packet_evidence", outcome: "supported" },
    packet_single_continuation: { authority: "packet_evidence", outcome: "supported" },
    packet_gap_to_focused_source: { authority: "packet_evidence", outcome: "unknown" },
    packet_named_fallback_to_source: { authority: "packet_evidence", outcome: "unknown" },
  }[id];
  return {
    id,
    expected_first_tool: first,
    required_action_sequence: first === "none" ? [] : [first, ...followups],
    optional_prefixes: optionalPrefixes,
    optional_followups: optionalFollowups,
    permitted_followups: [...followups, ...optionalFollowups],
    forbidden_tools: ROUTING_ACTIONS.filter((item) => !allowed.has(item)),
    source_read_authorization: { kind: source },
    final_claim_constraints: finalConstraints,
    identity_requirements: IDENTITY_REQUIREMENTS,
  };
}

export const ROUTING_SCENARIOS = deepFreeze([
  scenario({
    id: "named_file_direct_read",
    first: "source_read",
    source: "user_named_file",
  }),
  scenario({
    id: "exact_symbol_search",
    first: "search",
  }),
  scenario({
    id: "ambiguous_symbol_then_context",
    first: "search",
    followups: ["context"],
  }),
  scenario({
    id: "selected_target_context",
    first: "context",
  }),
  scenario({
    id: "broad_packet",
    first: "packet",
  }),
  scenario({
    id: "packet_single_continuation",
    first: "packet",
    followups: ["packet"],
    optionalFollowups: ["source_read"],
    source: "user_named_file",
  }),
  scenario({
    id: "packet_gap_to_focused_source",
    first: "packet",
    optionalFollowups: ["source_read"],
    source: "packet_evidence_gap",
  }),
  scenario({
    id: "packet_named_fallback_to_source",
    first: "packet",
    followups: ["source_read"],
    source: "user_named_file",
  }),
]);

const SCENARIOS_BY_ID = new Map(ROUTING_SCENARIOS.map((entry) => [entry.id, entry]));

// These scenarios depended on an advanced verifier absent from the public catalog.
export const RETIRED_ROUTING_SCENARIOS = deepFreeze([
  "typed_proof_contract_proven",
  "typed_proof_contract_refuted",
  "typed_proof_unknown",
  "typed_proof_unavailable",
  "malformed_proof_contract",
  "refuse_free_english_proof",
  "proof_observational",
  "hidden_proof_tool_discovery",
]);

export function requireSupportedRoutingScenario(scenarioId) {
  if (RETIRED_ROUTING_SCENARIOS.includes(scenarioId)) {
    fail(`unsupported routing scenario ${JSON.stringify(scenarioId)}: typed-proof qualification is retired from the public catalog`);
  }
  const supported = SCENARIOS_BY_ID.get(scenarioId);
  if (!supported) fail(`unknown routing scenario ${JSON.stringify(scenarioId)}`);
  return supported;
}

export function validateRoutingRequestCorpus(document = ROUTING_CORPUS_DOCUMENT) {
  requireExactKeys(document, ["schema_version", "scenarios"], "routing request corpus");
  if (document.schema_version !== 1 || !Array.isArray(document.scenarios)) fail("routing request corpus is invalid");
  document.scenarios.forEach(({ id }) => requireSupportedRoutingScenario(id));
  validateSupportedRoutingCatalog(GENERATED_MCP_CATALOG);
  const expectedIds = ROUTING_SCENARIOS.map(({ id }) => id);
  const observedIds = document.scenarios.map(({ id }) => id);
  if (!equalJson(observedIds, expectedIds) || new Set(observedIds).size !== expectedIds.length) {
    fail("routing request corpus must contain each frozen scenario exactly once in canonical order");
  }
  document.scenarios.forEach((entry, index) => {
    requireExactKeys(entry, ["id", "prompt", "request"], `routing request corpus scenario ${index}`);
    if (!nonemptyString(entry.prompt) || !plainObject(entry.request)) fail(`routing request corpus scenario ${index} is invalid`);
    requireExactKeys(
      entry.request,
      ["named_files", "selected_target", "gap_source_paths", "proof_contract"],
      `routing request corpus scenario ${entry.id} request`,
    );
    if (!Array.isArray(entry.request.named_files) || !entry.request.named_files.every(nonemptyString)
        || !Array.isArray(entry.request.gap_source_paths) || !entry.request.gap_source_paths.every(nonemptyString)
        || !(entry.request.selected_target === null || nonemptyString(entry.request.selected_target))) {
      fail(`routing request corpus scenario ${entry.id} request is invalid`);
    }
    if (entry.request.proof_contract !== null) fail(`${entry.id} cannot contain an unsupported proof contract`);
  });
  return true;
}

export const ROUTING_REQUEST_CORPUS = deepFreeze(structuredClone(ROUTING_CORPUS_DOCUMENT));

const FINAL_REPORT_INSTRUCTION = `Read an already named linked installed-guidance file only with a direct file read; never grep, rg, search, or probe the installed plugin package. When the scenario authorizes a direct source read, use the host's direct file-read action; never substitute CodeStory snippet or another MCP tool. An exact path appearing only in a CodeStory evidence row is not source-read authorization; do not read it unless the request or a material result gap separately authorizes that exact read. Do not add evidence through globbing, directory listing, repository search, shell commands, or another external repository tool; only the scenario-authorized direct source reads and CodeStory actions are permitted. Finish with only one raw JSON object and no markdown fence, explanation, prefix, or suffix, using exactly these keys: authority, outcome, target_id, evidence_ids, gap_ids, reason_codes, proof_disposition, refutation_basis, runtime_execution_claim, absence_claim, material_omissions. authority must be exactly one of source, search_lead, context_evidence, packet_evidence, none, chosen from the final evidence authority you actually used. outcome must be exactly one of supported, discovery_only, unknown, unavailable. Use supported for a direct source read only when that source evidence resolves the requested material. A fallback read that resolves the material changes evidence authority to source. A supplemental read after packet that leaves result-bound packet gaps unresolved keeps authority packet_evidence, uses outcome unknown, includes the source evidence identity, and preserves the packet gap identities. An authorized fallback read after an unavailable CodeStory result may change evidence authority but preserves the earlier unavailable outcome. Use supported only when the selected evidence authority resolves the requested material; if result-bound gaps leave any requested material unresolved, use unknown even when the tool result also returned useful evidence. Use discovery_only for a search lead. diagnostics.availability describes only the optional diagnostics artifact: never copy it into outcome or reason_codes, and determine result availability from top-level status and result-bound gaps. Use null for absent scalar identities and [] for absent lists. target_id must be null unless a CodeStory tool result returned a target identity. For a successful direct source read, record evidence identity source:<project-relative-path>. A failed direct read contributes no source evidence identity; keep the unresolved requested material in material_omissions instead. For CodeStory tool calls, copy evidence, gap, and target identities only from the tool results. proof_disposition and refutation_basis must be null; ordinary evidence carries no proof authority. reason_codes may contain only CodeStory tool result codes. runtime_execution_claim and absence_claim must each be false. material_omissions contains only unresolved material requested by the user; limitations outside the requested claim are not omissions, so use [] when the request was fully answered within the selected authority. Never claim runtime execution or absence and never omit a material requested gap.`;
const SCORING_REPORT_INSTRUCTION = FINAL_REPORT_INSTRUCTION
  .replaceAll("target_id", "target_symbol_id")
  .replace(
    "target_symbol_id must be null unless a CodeStory tool result returned a target identity.",
    "target_symbol_id must equal the final context result's target.symbol_id when present, or the sole search evidence symbol_id when the result has exactly one evidence row; otherwise it must be null.",
  );
const CONTEXT_EVIDENCE_INSTRUCTION = "A context evidence row matching the returned target symbol_id is focused identity and location evidence even when its optional excerpt is null. That null alone does not create material_omissions or an unknown outcome unless the request explicitly asks for source text or a claim the remaining row fields cannot support.";
const DIRECT_FILE_READ_INSTRUCTION = "A scenario that says to read a user-named file requires one direct read before the final response. On Codex, one bounded cat or sed command for that exact file is the direct-file action and is allowed; the shell prohibition applies only to search, probing, and recovery. Never report that authorized named file unavailable before attempting the read.";

export function materializeRoutingRequests(projectRoot) {
  const project = realpathSync(projectRoot);
  return ROUTING_REQUEST_CORPUS.scenarios.map((entry) => {
    return {
      scenario_id: entry.id,
      request: {
        ...structuredClone(entry.request),
        project_root: project,
        text: `${entry.prompt}\nThe exact project root for repository work is ${project}.\n${SCORING_REPORT_INSTRUCTION} ${CONTEXT_EVIDENCE_INSTRUCTION} ${DIRECT_FILE_READ_INSTRUCTION}`,
      },
    };
  });
}

export const STATIC_PARITY_HOSTS = deepFreeze({
  cursor: {
    metadata: ".cursor-plugin/plugin.json",
    hook: "hooks/cursor-hooks.json",
    rule: "rules/codestory.mdc",
  },
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
]);
const CODEX_GUIDANCE_PATHS = new Set([
  "skills/codestory-grounding/SKILL.md",
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

function matchesJsonSchema(value, schema) {
  if (schema === true) return true;
  if (schema === false || !plainObject(schema)) return false;
  if (Array.isArray(schema.allOf) && !schema.allOf.every((candidate) => matchesJsonSchema(value, candidate))) return false;
  if (Array.isArray(schema.anyOf) && !schema.anyOf.some((candidate) => matchesJsonSchema(value, candidate))) return false;
  if (Array.isArray(schema.oneOf)) {
    return schema.oneOf.filter((candidate) => matchesJsonSchema(value, candidate)).length === 1;
  }
  if (plainObject(schema.not) && matchesJsonSchema(value, schema.not)) return false;
  if (Object.hasOwn(schema, "const") && !equalJson(schema.const, value)) return false;
  if (Array.isArray(schema.enum) && !schema.enum.some((candidate) => equalJson(candidate, value))) return false;
  const types = Array.isArray(schema.type) ? schema.type : [schema.type];
  if (schema.type !== undefined && !types.some((type) => {
    if (type === "null") return value === null;
    if (type === "array") return Array.isArray(value);
    if (type === "object") return plainObject(value);
    if (type === "integer") return Number.isInteger(value);
    if (type === "number") return typeof value === "number" && Number.isFinite(value);
    return typeof value === type;
  })) return false;
  if (typeof value === "string" && Number.isInteger(schema.minLength) && value.length < schema.minLength) return false;
  if (typeof value === "string" && Number.isInteger(schema.maxLength) && value.length > schema.maxLength) return false;
  if (typeof value === "string" && typeof schema.pattern === "string" && !new RegExp(schema.pattern, "u").test(value)) return false;
  if (typeof value === "number") {
    if (typeof schema.minimum === "number" && value < schema.minimum) return false;
    if (typeof schema.maximum === "number" && value > schema.maximum) return false;
  }
  if (Array.isArray(value)) {
    if (Number.isInteger(schema.minItems) && value.length < schema.minItems) return false;
    if (Number.isInteger(schema.maxItems) && value.length > schema.maxItems) return false;
    if (schema.uniqueItems === true && new Set(value.map(canonical)).size !== value.length) return false;
    if (plainObject(schema.items) && !value.every((item) => matchesJsonSchema(item, schema.items))) return false;
  }
  if (plainObject(value)) {
    const properties = plainObject(schema.properties) ? schema.properties : {};
    if (Number.isInteger(schema.minProperties) && Object.keys(value).length < schema.minProperties) return false;
    if (Number.isInteger(schema.maxProperties) && Object.keys(value).length > schema.maxProperties) return false;
    if (Array.isArray(schema.required) && schema.required.some((key) => !Object.hasOwn(value, key))) return false;
    if (schema.additionalProperties === false && Object.keys(value).some((key) => !Object.hasOwn(properties, key))) return false;
    for (const [key, child] of Object.entries(value)) {
      if (Object.hasOwn(properties, key) && !matchesJsonSchema(child, properties[key])) return false;
    }
  }
  return true;
}

function normalizeToolName(name, server = "") {
  const raw = String(name ?? "").trim();
  const lower = raw.toLowerCase();
  if (lower.startsWith("mcp__codestory__")) return lower.slice("mcp__codestory__".length);
  const cursorMcp = lower.match(/^mcp[_-]codestory[_-](.+)$/u);
  if (cursorMcp) return cursorMcp[1];
  if (["codestory", "plugin-codestory-codestory"].includes(String(server).toLowerCase())) return lower;
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

function materialGapMessageAuthorizesPath(value, normalizedPath) {
  const materialGap = /\b(?:unknown|unavailable|not_established|evidence gap|material gap|missing evidence|evidence_missing|retrieval_unavailable|source_unavailable)\b/iu;
  const text = String(value ?? "").replaceAll("\\", "/");
  if (!materialGap.test(text)) return false;
  const quoted = [...text.matchAll(/`([^`\r\n]+)`/gu)];
  if (quoted.length !== 1) return false;
  let candidate;
  try {
    candidate = normalizePath(quoted[0][1]);
  } catch {
    return false;
  }
  if (candidate !== normalizedPath) return false;
  const remainder = `${text.slice(0, quoted[0].index)}${text.slice(quoted[0].index + quoted[0][0].length)}`;
  return !/\S+\/\S+/u.test(remainder);
}

function unwrapCodexShell(command) {
  const text = String(command ?? "").trim();
  const wrapped = text.match(/^\/bin\/zsh -(?:l)?c (.+)$/su);
  if (!wrapped) return text;
  const wrapper = wrapped[1];
  if (wrapper.startsWith("'") && wrapper.endsWith("'") && !wrapper.slice(1, -1).includes("'")) {
    return wrapper.slice(1, -1).trim();
  }
  if (wrapper.startsWith('"') && wrapper.endsWith('"')
      && !/["$`\\]/u.test(wrapper.slice(1, -1))) {
    return wrapper.slice(1, -1).trim();
  }
  const concatenatedLiteral = wrapper.match(/^"([^"$`\\]*)"'([^']*)'"([^"$`\\]*)"$/u);
  if (concatenatedLiteral) {
    return `${concatenatedLiteral[1]}${concatenatedLiteral[2]}${concatenatedLiteral[3]}`.trim();
  }
  try {
    const inner = JSON.parse(wrapper);
    return typeof inner === "string" ? inner.trim() : null;
  } catch {
    return null;
  }
}

function singleShellWord(value) {
  const text = String(value ?? "");
  if (text.startsWith("'") && text.endsWith("'") && !text.slice(1, -1).includes("'")) {
    return text.slice(1, -1);
  }
  if (text.startsWith('"') && text.endsWith('"') && !/["$`\\]/u.test(text.slice(1, -1))) {
    return text.slice(1, -1);
  }
  return /^\/?[A-Za-z0-9._@+-]+(?:\/[A-Za-z0-9._@+-]+)*$/u.test(text) ? text : null;
}

function singleFileReadPath(command) {
  const text = unwrapCodexShell(command);
  if (!text) return null;
  const countedRead = text.match(
    /^(?:(?:\/usr)?\/bin\/)?wc\s+-l\s+(\S+)\s+&&\s+(?:(?:\/usr)?\/bin\/)?sed\s+-n\s+(?:'\d+,\d+p'|"\d+,\d+p"|\d+,\d+p)\s+(\S+)$/u,
  );
  if (countedRead) {
    const countedPath = singleShellWord(countedRead[1]);
    const readPath = singleShellWord(countedRead[2]);
    return countedPath && countedPath === readPath ? readPath : null;
  }
  const patterns = [
    /^(?:(?:\/usr)?\/bin\/)?sed\s+-n\s+(?:'\d+,(?:\d+|\$)p'|"\d+,\d+p"|\d+,\d+p)\s+(.+)$/u,
    /^(?:(?:(?:\/usr)?\/bin\/)?(?:cat|nl)|type)(?:\s+(?:--|-[A-Za-z]+))*\s+(.+)$/u,
    /^Get-Content(?:\s+-(?:LiteralPath|Path))?\s+(.+)$/iu,
  ];
  for (const pattern of patterns) {
    const match = text.match(pattern);
    if (match) return singleShellWord(match[1]);
  }
  return null;
}

function sourceReadPath(command) {
  const path = singleFileReadPath(command);
  if (!path) return null;
  if (path.startsWith("/") || /^[a-z]:[\\/]/iu.test(path)) return path.replaceAll("\\", "/");
  return normalizePath(path);
}

function beginAction(state, id, action, allowOverlap = false) {
  if (!id || state.open.has(id) || state.completed.has(id)) fail(`duplicate or missing tool call id ${JSON.stringify(id)}`);
  if (!allowOverlap && state.open.size > 0) {
    fail(`tool call ${JSON.stringify(id)} started before ${JSON.stringify([...state.open.keys()][0])} completed`);
  }
  action.overlaps = [...state.open.values()];
  action.transcript_action_index = state.actions.length;
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
        }, true);
      } else if (itemType === "command_execution") {
        const path = sourceReadPath(item.command);
        beginAction(state, String(item.id ?? ""), {
          kind: path ? "source_read" : "shell",
          tool: path ? "source_read" : "shell",
          path,
          command: String(item.command ?? ""),
        }, true);
      } else {
        beginAction(state, String(item.id ?? ""), {
          kind: "external_tool",
          tool: itemType || "unknown",
          args: item.arguments ?? {},
        }, true);
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

function unwrapCursorToolCall(event, callId) {
  if (!plainObject(event.tool_call)) fail("Cursor tool_call event is missing tool_call");
  const keys = Object.keys(event.tool_call);
  const payloadKeys = keys.filter((key) => /^[A-Za-z][A-Za-z0-9]*ToolCall$/u.test(key));
  if (payloadKeys.length !== 1) {
    fail("Cursor tool_call must contain exactly one *ToolCall payload");
  }
  const key = payloadKeys[0];
  const wrapper = event.tool_call[key];
  if (!plainObject(wrapper)) fail("Cursor tool_call payload is invalid");
  const metadataKeys = keys.filter((candidate) => candidate !== key).sort();
  if (metadataKeys.length === 0) return { key, wrapper, envelope: null };
  const expectedKeys = event.subtype === "completed"
    ? ["completedAtMs", "hookAdditionalContexts", "startedAtMs", "toolCallId"]
    : ["hookAdditionalContexts", "startedAtMs", "toolCallId"];
  const metadata = event.tool_call;
  const validTimestamp = (value) => typeof value === "string" && /^[0-9]+$/u.test(value);
  if (!equalJson(metadataKeys, expectedKeys)
      || !Array.isArray(metadata.hookAdditionalContexts) || metadata.hookAdditionalContexts.length !== 0
      || metadata.toolCallId !== callId || !validTimestamp(metadata.startedAtMs)
      || !nonemptyString(event.model_call_id)
      || (event.subtype === "completed"
        && (!validTimestamp(metadata.completedAtMs)
          || BigInt(metadata.completedAtMs) < BigInt(metadata.startedAtMs)))) {
    fail("Cursor tool_call envelope is invalid");
  }
  return {
    key,
    wrapper,
    envelope: {
      toolCallId: metadata.toolCallId,
      startedAtMs: metadata.startedAtMs,
      modelCallId: event.model_call_id,
    },
  };
}

function cursorStartedAction(callId, key, args) {
  if (!plainObject(args)) fail("Cursor started tool call is missing args");
  if (key === "readToolCall") {
    return { kind: "source_read", tool: "source_read", path: String(args.path ?? ""), args, cursor_key: key };
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
  if (key === "getMcpToolsToolCall") {
    const keys = Object.keys(args).sort();
    const exactTool = equalJson(keys, ["server", "toolCallId", "toolName"])
      && nonemptyString(args.toolName);
    const patternSearch = equalJson(keys, ["pattern", "server", "toolCallId"])
      && nonemptyString(args.pattern);
    if ((!exactTool && !patternSearch)
        || args.server !== "plugin-codestory-codestory" || args.toolCallId !== callId) {
      fail("Cursor getMcpToolsToolCall args are invalid");
    }
    return {
      kind: "cursor_tool_discovery",
      tool: "cursor_tool_discovery",
      args,
      cursor_key: key,
      cursor_args: args,
    };
  }
  if (key === "awaitToolCall") {
    if (!equalJson(Object.keys(args).sort(), ["blockUntilMs", "taskId"])
        || args.taskId !== ""
        || !Number.isSafeInteger(args.blockUntilMs)
        || args.blockUntilMs < 1) {
      fail("Cursor awaitToolCall is not a bounded delay-only preparing retry wait");
    }
    return {
      kind: "cursor_retry_wait",
      tool: "awaitToolCall",
      args,
      cursor_key: key,
      cursor_args: args,
    };
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
  let assistantFragments = "";
  let lastAssistantSnapshot = "";
  let userText = "";
  const snapshotStream = events.some((event) => event.type === "thinking"
    || (event.type === "assistant" && (event.timestamp_ms != null || event.model_call_id != null)));
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
    if (event.type === "thinking") {
      if (event.subtype === "delta" && nonemptyString(event.text)) return;
      if (["started", "completed"].includes(event.subtype) && event.text == null) return;
      fail(`unsupported Cursor thinking event ${JSON.stringify(event.subtype ?? null)}`);
    }
    if (event.type === "assistant") {
      const text = cursorText(event, "assistant");
      if (!snapshotStream) {
        assistantDeltas += text;
        return;
      }
      const snapshot = event.model_call_id != null || event.timestamp_ms == null;
      if (snapshot) {
        if (assistantFragments !== text) fail("Cursor assistant snapshot does not match streamed deltas");
        assistantDeltas += text;
        lastAssistantSnapshot = text;
        assistantFragments = "";
      } else {
        assistantFragments += text;
      }
      return;
    }
    if (event.type === "tool_call") {
      const callId = String(event.call_id ?? "");
      const { key, wrapper, envelope } = unwrapCursorToolCall(event, callId);
      if (event.subtype === "started") {
        if (Object.hasOwn(wrapper, "result")) fail("Cursor started tool call must not contain a result");
        beginAction(
          state,
          callId,
          { ...cursorStartedAction(callId, key, wrapper.args), cursor_envelope: envelope },
          true,
        );
        return;
      }
      if (event.subtype !== "completed") fail(`unsupported Cursor tool_call subtype ${JSON.stringify(event.subtype)}`);
      const action = state.open.get(callId);
      if (!action) fail(`unmatched tool call result ${JSON.stringify(callId)}`);
      if (action.cursor_key !== key
          || !equalJson(action.cursor_envelope, envelope)
          || (wrapper.args != null && !equalJson(action.cursor_args ?? action.args, wrapper.args))) {
        fail(`Cursor completed tool call ${JSON.stringify(callId)} does not match its start`);
      }
      if (!plainObject(wrapper.result) || Object.keys(wrapper.result).length !== 1
          || (!Object.hasOwn(wrapper.result, "success") && !Object.hasOwn(wrapper.result, "error"))) {
        fail(`Cursor completed tool call ${JSON.stringify(callId)} must contain exactly one success or error result`);
      }
      if (event.truncated != null || wrapper.result.truncated != null) {
        fail(`Cursor completed tool call ${JSON.stringify(callId)} is partial`);
      }
      if (Object.hasOwn(wrapper.result, "error")) {
        const error = wrapper.result.error;
        const readError = key === "readToolCall" && wrapper.args == null && plainObject(error)
          && equalJson(Object.keys(error), ["errorMessage"])
          && nonemptyString(error.errorMessage);
        const mcpError = key === "mcpToolCall" && wrapper.args == null && plainObject(error)
          && equalJson(Object.keys(error).sort(), ["error", "readToolDefReminder"])
          && nonemptyString(error.error) && nonemptyString(error.readToolDefReminder);
        if (!readError && !mcpError) {
          fail(`Cursor failed tool call ${JSON.stringify(callId)} has an invalid error result`);
        }
        completeAction(state, callId, error, true);
        return;
      }
      const success = wrapper.result.success;
      if (!plainObject(wrapper.args) && !["getMcpToolsToolCall", "mcpToolCall"].includes(key)) {
        fail(`Cursor completed tool call ${JSON.stringify(callId)} is missing args`);
      }
      if (key === "readToolCall" && (!plainObject(success) || success.exceededLimit !== false)) {
        fail(`Cursor completed tool call ${JSON.stringify(callId)} contains a partial read`);
      }
      if (key === "awaitToolCall") {
        const complete = success?.complete;
        if (!plainObject(success)
            || !equalJson(Object.keys(success), ["complete"])
            || !plainObject(complete)
            || !equalJson(Object.keys(complete).sort(), [
              "outputFilePath", "outputLength", "regexRequested", "runtimeMs", "taskId",
            ])
            || complete.taskId !== ""
            || complete.runtimeMs !== String(action.args.blockUntilMs)
            || complete.outputFilePath !== ""
            || complete.outputLength !== "0"
            || complete.regexRequested !== false) {
          fail("Cursor awaitToolCall is not a bounded delay-only preparing retry wait");
        }
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
      if (assistantFragments) fail("Cursor terminal result arrived before an assistant snapshot");
      if (assistantDeltas !== event.result) fail("Cursor assistant deltas do not match terminal result");
      state.final = snapshotStream ? lastAssistantSnapshot : event.result;
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
  for (const field of [
    "receipt.sha256", "package.sha256", "launcher.sha256", "cli.sha256",
    "protocol.discovery_contract_sha256",
    ...MCP_PROTOCOL_REVISIONS.map((revision) => `protocol.discovery_contracts.${revision}`),
  ]) {
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
  if (!plainObject(identity.protocol.discovery_contracts)
      || !equalJson(Object.keys(identity.protocol.discovery_contracts).sort(), [...MCP_PROTOCOL_REVISIONS].sort())) {
    fail(`${label}.protocol.discovery_contracts must contain the four supported revisions exactly`);
  }
  if (identity.protocol.discovery_contracts[identity.protocol.revision]
      !== identity.protocol.discovery_contract_sha256) {
    fail(`${label}.protocol.revision and protocol.discovery_contract_sha256 do not match its roster`);
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

function authenticateCursorInstalledPluginRoot(installedPluginRoot, expected) {
  if (!installedPluginRoot) fail("Cursor qualification requires the installed plugin root");
  if (!plainObject(expected.static_roster)
      || !Object.hasOwn(expected.static_roster, "generated-mcp-catalog.json")) {
    fail("expected static digest roster is missing the generated MCP catalog");
  }
  for (const [path, digest] of Object.entries(expected.static_roster)) {
    if (!SHA256.test(String(digest)) || /^0{64}$/u.test(String(digest))) {
      fail(`expected static digest roster ${path} is invalid`);
    }
    const bytes = readFileSync(fileInsideInstalledRoot(
      installedPluginRoot, path, `installed Cursor plugin ${path}`,
    ));
    if (sha256Bytes(bytes) !== digest) {
      fail(`installed Cursor plugin ${path} does not match authenticated package bytes`);
    }
  }
  const launcherBytes = readFileSync(fileInsideInstalledRoot(
    installedPluginRoot, expected.launcher.relative_path, "installed Cursor plugin launcher",
  ));
  if (sha256Bytes(launcherBytes) !== expected.launcher.sha256) {
    fail("installed Cursor plugin launcher does not match authenticated launcher bytes");
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

function normalizedResult(action, host) {
  const raw = decodeResultEnvelope(action.result);
  if (!plainObject(raw)) return { raw, body: parseJsonText(raw), meta: null, isError: action.error };
  if (host === "cursor"
      && equalJson(Object.keys(raw).sort(), ["content", "isError"])
      && raw.isError === false
      && Array.isArray(raw.content) && raw.content.length === 1
      && plainObject(raw.content[0]) && equalJson(Object.keys(raw.content[0]), ["text"])
      && plainObject(raw.content[0].text) && equalJson(Object.keys(raw.content[0].text), ["text"])
      && nonemptyString(raw.content[0].text.text)) {
    const body = parseJsonText(raw.content[0].text.text);
    return {
      raw,
      body,
      meta: null,
      isError: body === null,
      transport_projection: body === null
        ? "cursor_semantic_error_text_v1"
        : "cursor_content_text_v1",
    };
  }
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

function validateResultIdentity(action, expected, host) {
  const normalized = normalizedResult(action, host);
  if (normalized.transport_projection === "cursor_content_text_v1") {
    // Cursor exposes only the MCP text block. The authenticated launcher checks
    // the negotiated revision and discovery digest before it relays any runtime
    // result; the caller authenticates those launcher bytes before parsing here.
    if (!plainObject(normalized.body)) fail(`${action.tool} Cursor result text is not a JSON object`);
    return normalized;
  }
  const publication = normalized.meta?.codestory_publication;
  const protocol = normalized.meta?.codestory_protocol;
  const runtime = publication?.contract_runtime;
  const nativeRevision = normalized.meta?.["com.thegreencedar.codestory/protocolRevision"];
  const projected = plainObject(protocol);
  const projectedRevision = projected ? protocol.negotiated : null;
  if (projected && (!nonemptyString(projectedRevision)
      || !SHA256.test(String(protocol.discovery_contract_sha256)))) {
    fail(`${action.tool} result identity projected protocol metadata requires its revision and discovery digest`);
  }
  if (nativeRevision != null && projectedRevision != null && nativeRevision !== projectedRevision) {
    fail(`${action.tool} result identity protocol revision metadata conflicts`);
  }
  const negotiatedRevision = nativeRevision ?? projectedRevision;
  if (!nonemptyString(negotiatedRevision)
      || !Object.hasOwn(expected.protocol.discovery_contracts, negotiatedRevision)) {
    fail(`${action.tool} result identity negotiated protocol revision is outside the authenticated roster`);
  }
  const negotiatedDiscovery = expected.protocol.discovery_contracts[negotiatedRevision];
  if (protocol?.preferred != null && protocol.preferred !== expected.protocol.revision) {
    fail(`${action.tool} result identity preferred protocol revision does not match installed identity`);
  }
  if (projected && protocol.discovery_contract_sha256 !== negotiatedDiscovery) {
    fail(`${action.tool} result identity protocol.discovery_contract_sha256 does not match the negotiated revision`);
  }
  if (!plainObject(runtime)) {
    fail(`${action.tool} result identity requires runtime identity`);
  }
  const mismatches = [
    ["publication.schema_version", publication?.schema_version, expected.publication.schema_version],
    ...(plainObject(runtime) ? [
      ["package.version", runtime.plugin_version, expected.package.version],
      ["cli.pinned_version", runtime.plugin_cli_version, expected.cli.version],
      ["cli.version", runtime.cli_version, expected.cli.version],
      ["cli.sha256", runtime.cli_sha256, expected.cli.sha256],
      ["cli.source", runtime.cli_source, expected.cli.source],
    ] : []),
  ];
  for (const [field, observed, wanted] of mismatches) {
    if (observed !== wanted) fail(`${action.tool} result identity ${field} does not match installed identity`);
  }
  if (plainObject(runtime)
      && (runtime.pinned_pair_matches !== true || runtime.known_override_skew_channel !== false)) {
    fail(`${action.tool} result identity does not prove one pinned managed runtime`);
  }
  return normalized;
}

function actionName(action) {
  return action.kind;
}

function validateExpectedMcpAvailability(scenarioContract, actions) {
  const expected = new Set(scenarioContract.required_action_sequence.filter((kind) => (
    ["search", "context", "packet"].includes(kind)
  )));
  for (const action of actions) {
    if (expected.has(action.kind) && action.error) {
      fail(`${scenarioContract.id} has an unexpected failed ${action.tool} action`);
    }
  }
}

function collapsePreparingRetries(actions, expectedIdentity, host) {
  const collapsed = [];
  let consecutivePreparing = 0;
  for (let index = 0; index < actions.length; index += 1) {
    const action = actions[index];
    if (!["search", "context", "packet"].includes(action.kind)) {
      consecutivePreparing = 0;
      collapsed.push(action);
      continue;
    }
    const observed = normalizedResult(action, host);
    if (action.error || observed.isError || observed.body?.kind !== "preparing") {
      consecutivePreparing = 0;
      collapsed.push(action);
      continue;
    }
    const normalized = validateResultIdentity(action, expectedIdentity, host);
    validateToolInputSchema(action);
    const outputSchema = GENERATED_TOOL_SCHEMAS.get(action.kind)?.outputSchema;
    if (!plainObject(outputSchema) || !matchesJsonSchema(normalized.body, outputSchema)) {
      fail(`${action.tool} preparing result does not match the generated catalog output schema`);
    }
    consecutivePreparing += 1;
    if (consecutivePreparing > 3) fail(`${action.tool} exceeded the bounded preparing retry limit`);
    let retryIndex = index + 1;
    const wait = actions[retryIndex];
    let expectedTranscriptActionIndex = action.transcript_action_index + 1;
    if (wait?.kind === "cursor_retry_wait") {
      const retryAfterMs = normalized.body.retry_after_ms;
      if (host !== "cursor"
          || !wait.completed
          || wait.error
          || wait.overlaps.length !== 0
          || wait.transcript_action_index !== expectedTranscriptActionIndex
          || wait.args.blockUntilMs < retryAfterMs
          || wait.args.blockUntilMs > retryAfterMs * 2) {
        fail(`${action.tool} preparing retry has an invalid bounded delay-only preparing retry wait`);
      }
      retryIndex += 1;
      index += 1;
      expectedTranscriptActionIndex += 1;
    }
    const retry = actions[retryIndex];
    if (!retry || retry.kind !== action.kind || retry.tool !== action.tool
        || retry.transcript_action_index !== expectedTranscriptActionIndex
        || !equalJson(retry.args, action.args)) {
      fail(`${action.tool} preparing result must be followed directly by the same tool and arguments, apart from at most one bounded retry wait`);
    }
  }
  return collapsed;
}

function validateActionOrder(scenarioContract, actions) {
  const prefixCount = actions.length > 0
    && scenarioContract.optional_prefixes.includes(actionName(actions[0])) ? 1 : 0;
  const routedActions = actions.slice(prefixCount);
  const observedSequence = routedActions.map(actionName);
  const required = scenarioContract.required_action_sequence;
  const extras = observedSequence.slice(required.length);
  if (observedSequence.length < required.length
      || required.some((name, index) => observedSequence[index] !== name)
      || extras.some((name) => !scenarioContract.optional_followups.includes(name))
      || new Set(extras).size !== extras.length) {
    fail(`${scenarioContract.id} required action sequence ${JSON.stringify(scenarioContract.required_action_sequence)} but observed ${JSON.stringify(observedSequence)}`);
  }
  if (scenarioContract.expected_first_tool === "none") {
    if (actions.length > 0) fail(`${scenarioContract.id} expected no tool but observed ${actionName(actions[0])}`);
    return;
  }
  if (routedActions.length === 0) fail(`${scenarioContract.id} expected first tool ${scenarioContract.expected_first_tool} but observed none`);
  if (actionName(routedActions[0]) !== scenarioContract.expected_first_tool) {
    fail(`${scenarioContract.id} expected first tool ${scenarioContract.expected_first_tool} but observed ${actionName(routedActions[0])}`);
  }
  for (const action of routedActions.slice(1)) {
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
  const successfulReads = reads.filter((action) => action.completed && !action.error);
  const kind = scenarioContract.source_read_authorization.kind;
  if (kind === "none") {
    if (reads.length > 0) fail(`${scenarioContract.id} source read is not authorized`);
    return;
  }
  if (reads.length === 0) {
    if (scenarioContract.optional_followups.includes("source_read")) return;
    fail(`${scenarioContract.id} requires one authorized source read`);
  }
  if (!scenarioContract.optional_followups.includes("source_read") && successfulReads.length === 0) {
    fail(`${scenarioContract.id} requires one successful authorized source read`);
  }
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
    const materialGaps = Array.isArray(body?.gaps)
      ? body.gaps.filter((gap) => ["evidence_missing", "retrieval_unavailable", "source_unavailable"].includes(gap?.kind))
      : [];
    if (materialGaps.length === 0) fail(`${scenarioContract.id} source read lacks an explicit packet evidence gap`);
    const allowed = new Set((request.gap_source_paths ?? []).map(normalizePath));
    for (const read of reads) {
      if (!allowed.has(read.path)) fail(`${scenarioContract.id} source read is not authorized by the packet evidence gap`);
      if (!materialGaps.some((gap) => materialGapMessageAuthorizesPath(gap.message, read.path))) {
        fail(`${scenarioContract.id} source read is not correlated with the packet evidence gap`);
      }
    }
    return;
  }
  fail(`${scenarioContract.id} has unsupported source-read authorization ${kind}`);
}

function requireExactKeys(value, keys, label) {
  if (!plainObject(value) || !equalJson(Object.keys(value).sort(), [...keys].sort())) {
    fail(`${label} does not match its required schema`);
  }
}

function nonemptyString(value) {
  return typeof value === "string" && value.length > 0;
}

function validateRequestIdentity(value, label) {
  requireExactKeys(value, ["packet_id", "request_id", "question_sha256"], label);
  if (!nonemptyString(value.packet_id) || !nonemptyString(value.request_id) || !SHA256.test(value.question_sha256)) {
    fail(`${label} is invalid`);
  }
}

function validatePublication(value, label) {
  requireExactKeys(value, ["core", "retrieval"], label);
  requireExactKeys(value.core, ["project_id", "generation_id", "run_id"], `${label}.core`);
  if (![value.core.project_id, value.core.generation_id, value.core.run_id].every(nonemptyString)) fail(`${label}.core is invalid`);
  if (value.retrieval !== null) {
    requireExactKeys(value.retrieval, [
      "core_generation_id", "core_run_id", "retrieval_generation",
      "retrieval_input_sha256", "semantic_generation",
    ], `${label}.retrieval`);
    if (![value.retrieval.core_generation_id, value.retrieval.core_run_id,
      value.retrieval.retrieval_generation, value.retrieval.semantic_generation].every(nonemptyString)
      || !SHA256.test(value.retrieval.retrieval_input_sha256)) fail(`${label}.retrieval is invalid`);
  }
}

function validateProjectionGap(gap, label) {
  requireExactKeys(gap, ["identity", "kind", "message"], label);
  requireExactKeys(gap.identity, ["gap_id"], `${label}.identity`);
  if (!nonemptyString(gap.identity.gap_id)
      || !["evidence_missing", "retrieval_unavailable", "source_unavailable", "continuation_required", "output_budget_exceeded"].includes(gap.kind)
      || !(gap.message === null || typeof gap.message === "string")) fail(`${label} is invalid`);
}

function validateProjectionEnvelope(body, label) {
  if (body.kind !== "complete" || body.schema_version !== 3
      || !["available", "continuation_available", "no_useful_evidence", "unavailable"].includes(body.status)) {
    fail(`${label} is incomplete`);
  }
  validateRequestIdentity(body.identity, `${label} identity`);
  validatePublication(body.publication, `${label} publication`);
  if (!Array.isArray(body.evidence) || !Array.isArray(body.gaps)) fail(`${label} is incomplete`);
  body.gaps.forEach((gap, index) => validateProjectionGap(gap, `${label} gap ${index}`));
}

function validateSearchResult(body) {
  validateProjectionEnvelope(body, "search result");
  if (body.evidence.length === 0) fail("search result is incomplete");
  body.evidence.forEach((entry, index) => {
    requireExactKeys(entry, ["identity", "path", "symbol_id", "start_line", "end_line", "excerpt"], `search result evidence ${index}`);
    requireExactKeys(entry.identity, ["evidence_id"], `search result evidence ${index}.identity`);
    if (!nonemptyString(entry.identity.evidence_id) || !nonemptyString(entry.path)
        || !(entry.symbol_id === null || nonemptyString(entry.symbol_id))) fail(`search result evidence ${index} is invalid`);
  });
}

function validateContextResult(body) {
  validateProjectionEnvelope(body, "context result");
  requireExactKeys(body.target, ["path", "symbol_id"], "context result target");
  if ((body.target.path === null && body.target.symbol_id === null) || body.evidence.length === 0) {
    fail("context result is incomplete");
  }
  body.evidence.forEach((entry, index) => {
    requireExactKeys(entry, ["identity", "path", "symbol_id", "start_line", "end_line", "excerpt"], `context result evidence ${index}`);
    requireExactKeys(entry.identity, ["evidence_id"], `context result evidence ${index}.identity`);
    if (!nonemptyString(entry.identity.evidence_id) || !nonemptyString(entry.path)) fail(`context result evidence ${index} is invalid`);
  });
}

function validatePacketResult(body) {
  if (body.kind === "budget_exceeded") {
    if (body.schema_version !== 3 || body.status !== "unavailable" || !Array.isArray(body.gaps)
        || body.gaps.length === 0 || !Number.isSafeInteger(body.maximum_bytes)
        || !Number.isSafeInteger(body.required_complete_bytes)) fail("packet result budget fallback is invalid");
    validateRequestIdentity(body.identity, "packet result identity");
    validatePublication(body.publication, "packet result publication");
    body.gaps.forEach((gap, index) => validateProjectionGap(gap, `packet result gap ${index}`));
    return;
  }
  validateProjectionEnvelope(body, "packet result");
  body.evidence.forEach((entry, index) => {
    requireExactKeys(entry, ["identity", "kind", "path", "symbol_id", "start_line", "end_line", "summary"], `packet result evidence ${index}`);
    requireExactKeys(entry.identity, ["evidence_id"], `packet result evidence ${index}.identity`);
    if (!nonemptyString(entry.identity.evidence_id)
        || !["exact_source", "structural_source", "graph_relation", "retrieval_excerpt"].includes(entry.kind)) {
      fail(`packet result evidence ${index} is invalid`);
    }
  });
  if (body.status === "continuation_available") {
    requireExactKeys(body.continuation, ["continuation_id", "remaining_rounds", "gap_ids"], "packet result continuation");
    if (!nonemptyString(body.continuation.continuation_id) || !Number.isInteger(body.continuation.remaining_rounds)
        || body.continuation.remaining_rounds < 1 || !Array.isArray(body.continuation.gap_ids)
        || body.continuation.gap_ids.length === 0) fail("packet result continuation is invalid");
    body.continuation.gap_ids.forEach((gap, index) => {
      requireExactKeys(gap, ["gap_id"], `packet result continuation gap ${index}`);
      if (!nonemptyString(gap.gap_id)) fail(`packet result continuation gap ${index} is invalid`);
    });
  } else if (body.continuation !== null) {
    fail("packet result has a continuation outside continuation_available");
  }
}

function validateToolResultSchema(action, projection) {
  if (!plainObject(projection.body)) fail(`${action.tool} result is not a JSON object`);
  if (["search", "context", "packet"].includes(action.kind)) {
    const outputSchema = GENERATED_TOOL_SCHEMAS.get(action.kind)?.outputSchema;
    if (!plainObject(outputSchema) || !matchesJsonSchema(projection.body, outputSchema)) {
      fail(`${action.tool} result does not match the generated catalog output schema`);
    }
  }
  if (action.kind === "search") validateSearchResult(projection.body);
  if (action.kind === "context") validateContextResult(projection.body);
  if (action.kind === "packet") validatePacketResult(projection.body);
}

function validateToolInputSchema(action) {
  const inputSchema = GENERATED_TOOL_SCHEMAS.get(action.kind)?.inputSchema;
  if (!plainObject(inputSchema) || !matchesJsonSchema(action.args, inputSchema)) {
    fail(`${action.tool} request does not match the generated catalog input schema`);
  }
}

function validatePacketContinuation(scenarioContract, actions, results) {
  const packets = actions.filter((action) => action.kind === "packet");
  if (packets.length > 2) fail(`${scenarioContract.id} allows at most one packet continuation`);
  if (packets.length > 0) {
    const allowedInitialKeys = new Set([
      "project", "question", "budget", "latency_budget_ms",
    ]);
    if (!plainObject(packets[0].args)
        || !nonemptyString(packets[0].args.project)
        || !nonemptyString(packets[0].args.question)
        || Object.keys(packets[0].args).some((key) => !allowedInitialKeys.has(key))) {
      fail(`${scenarioContract.id} initial packet arguments does not match its required schema`);
    }
    const expectedQuestion = ROUTING_PACKET_QUESTIONS[scenarioContract.id];
    if (expectedQuestion && packets[0].args.question !== expectedQuestion) {
      fail(`${scenarioContract.id} initial packet question does not match the preflighted fixture`);
    }
  }
  if (packets.length < 2) return;
  const first = results.get(packets[0])?.body;
  if (first?.status !== "continuation_available") fail(`${scenarioContract.id} repeated packet without a continuation offer`);
  const expected = {
    project: packets[0].args.project,
    question: packets[0].args.question,
    parent_packet_id: first?.continuation?.continuation_id,
    option_ids: first?.continuation?.gap_ids?.map(({ gap_id: gapId }) => gapId),
    core_generation_id: first?.publication?.core?.generation_id,
    retrieval_generation: first?.publication?.retrieval?.retrieval_generation,
  };
  for (const key of ["budget", "latency_budget_ms"]) {
    if (Object.hasOwn(packets[0].args, key)) expected[key] = packets[0].args[key];
  }
  if (!equalJson(packets[1].args, expected)) fail(`${scenarioContract.id} packet continuation arguments do not match the pinned offer`);
}

function projectRelativeSelectionPath(path, projectRoot) {
  if (!nonemptyString(path) || !nonemptyString(projectRoot)) return null;
  const root = resolve(projectRoot);
  const candidate = resolve(root, path);
  const candidateRelative = relative(root, candidate);
  if (!candidateRelative || candidateRelative === ".." || candidateRelative.startsWith(`..${sep}`)) return null;
  return candidateRelative.split(sep).join("/");
}

function validateSelectedContext(scenarioContract, request, actions, results) {
  const contexts = actions.filter((action) => action.kind === "context");
  if (contexts.length === 0) return;
  if (typeof request.selected_target !== "string" || !request.selected_target) {
    fail(`${scenarioContract.id} context requires one host-selected target`);
  }
  const search = actions.find((action) => action.kind === "search");
  if (search) {
    const selectedPath = projectRelativeSelectionPath(request.selected_target, search.args?.project);
    const selected = results.get(search).body.evidence.filter((entry) => (
      nonemptyString(entry.symbol_id)
      && (entry.symbol_id === request.selected_target
        || (selectedPath !== null
          && projectRelativeSelectionPath(entry.path, search.args?.project) === selectedPath))
    ));
    if (selected.length !== 1 || !nonemptyString(selected[0].symbol_id)) {
      fail(`${scenarioContract.id} selected target does not identify exactly one search evidence row`);
    }
    for (const action of contexts) {
      if (action.args?.id !== selected[0].symbol_id
          || results.get(action)?.body?.target?.symbol_id !== selected[0].symbol_id) {
        fail(`${scenarioContract.id} context result does not match the selected search target`);
      }
    }
    return;
  }
  for (const action of contexts) {
    if (!(action.args?.id === request.selected_target || action.args?.query === request.selected_target)) {
      fail(`${scenarioContract.id} context selector does not match the selected target`);
    }
    const target = results.get(action)?.body?.target;
    if (!nonemptyString(target?.symbol_id)
        || !results.get(action).body.evidence.some((entry) => entry.symbol_id === target.symbol_id)) {
      fail(`${scenarioContract.id} context result does not bind its returned target to evidence`);
    }
  }
}

function validateSearchQueries(scenarioContract, actions) {
  const expected = ROUTING_SEARCH_QUERIES[scenarioContract.id];
  if (!expected) return;
  const searches = actions.filter((action) => action.kind === "search");
  if (searches.length !== 1 || searches[0].args?.query !== expected) {
    fail(`${scenarioContract.id} search query must preserve the exact supplied symbol name`);
  }
}

const FINAL_CLAIM_KEYS = Object.freeze([
  "authority",
  "outcome",
  "target_symbol_id",
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
      || !(claim.target_symbol_id === null || nonemptyString(claim.target_symbol_id))
      || claim.proof_disposition !== null
      || claim.refutation_basis !== null
      || typeof claim.runtime_execution_claim !== "boolean" || typeof claim.absence_claim !== "boolean") {
    fail(`${scenarioId} final claim has invalid typed fields`);
  }
  uniqueStrings(claim.evidence_ids, `${scenarioId} final claim evidence_ids`);
  uniqueStrings(claim.gap_ids, `${scenarioId} final claim gap_ids`);
  uniqueStrings(claim.reason_codes, `${scenarioId} final claim reason_codes`);
  if (!Array.isArray(claim.material_omissions)
      || !claim.material_omissions.every(nonemptyString)) {
    fail(`${scenarioId} final claim material_omissions must be an array of nonempty strings`);
  }
  return claim;
}

function expectedFinalClaim(scenarioContract, actions, results) {
  const expected = {
    ...scenarioContract.final_claim_constraints,
    target_symbol_id: null,
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
  const reads = actions.filter((action) => action.kind === "source_read" && action.completed && !action.error);

  if (contexts.length > 0) {
    const body = results.get(contexts.at(-1)).body;
    expected.target_symbol_id = body.target.symbol_id;
    expected.evidence_ids = body.evidence.map(({ identity }) => identity.evidence_id);
  } else if (searches.length > 0) {
    const body = results.get(searches.at(-1)).body;
    if (body.evidence.length === 1) expected.target_symbol_id = body.evidence[0].symbol_id;
    expected.evidence_ids = body.evidence.map(({ identity }) => identity.evidence_id);
  } else if (packets.length > 0) {
    expected.evidence_ids = packets.flatMap((action) => (results.get(action).body.evidence ?? []).map(({ identity }) => identity.evidence_id));
  }
  if (reads.length > 0) {
    if (scenarioContract.id !== "packet_named_fallback_to_source") expected.authority = "source";
    if (scenarioContract.id === "packet_gap_to_focused_source") expected.outcome = "supported";
    expected.evidence_ids.push(...reads.map(({ path }) => `source:${path}`));
  }
  for (const action of searches) {
    expected.gap_ids.push(...results.get(action).body.gaps.map((gap) => gap.identity.gap_id));
  }
  if (packets.length > 0) {
    expected.gap_ids.push(...results.get(packets.at(-1)).body.gaps.map((gap) => gap.identity.gap_id));
  }
  for (const action of packets) {
    const body = results.get(action).body;
    if (body.status === "unavailable") {
      expected.reason_codes.push(...body.gaps.map(({ kind }) => kind));
    }
  }
  expected.evidence_ids = [...new Set(expected.evidence_ids)];
  expected.gap_ids = [...new Set(expected.gap_ids.filter(nonemptyString))];
  expected.reason_codes = [...new Set(expected.reason_codes)];
  return expected;
}

function validateFinalClaims(scenarioContract, final, actions, results) {
  const claim = parseFinalClaim(final, scenarioContract.id);
  const expected = expectedFinalClaim(scenarioContract, actions, results);
  const hasResultBoundGap = expected.gap_ids.length > 0 || expected.reason_codes.length > 0;
  if (claim.material_omissions.length > 0
      && hasResultBoundGap
      && expected.outcome === "supported") {
    expected.outcome = "unknown";
  }
  if (claim.material_omissions.length > 0 && !hasResultBoundGap) {
    fail(`${scenarioContract.id} final claim contains omissions without a result-bound gap`);
  }
  if (claim.material_omissions.length > 0 && claim.outcome === "supported") {
    fail(`${scenarioContract.id} final claim cannot call unresolved requested material supported`);
  }
  const allowedReasonCodes = new Set(expected.reason_codes);
  for (const action of actions) {
    const body = results.get(action)?.body;
    for (const gap of body?.gaps ?? []) {
      if (nonemptyString(gap?.kind)) allowedReasonCodes.add(gap.kind);
    }
    if (nonemptyString(body?.code)) allowedReasonCodes.add(body.code);
  }
  if (claim.reason_codes.some((reason) => !allowedReasonCodes.has(reason))
      || expected.reason_codes.some((reason) => !claim.reason_codes.includes(reason))) {
    fail(`${scenarioContract.id} final claim reason_codes do not match result-bound codes`);
  }
  for (const key of FINAL_CLAIM_KEYS) {
    if (["evidence_ids", "reason_codes", "material_omissions"].includes(key)) continue;
    if (!equalJson(claim[key], expected[key])) {
      fail(`${scenarioContract.id} final claim ${key} does not match result-bound evidence`);
    }
  }
  const reads = actions.some((action) => action.kind === "source_read" && action.completed && !action.error);
  if (reads) {
    const allowedEvidenceIds = new Set(expected.evidence_ids);
    const sourceEvidenceIds = actions
      .filter((action) => action.kind === "source_read" && action.completed && !action.error)
      .map(({ path }) => `source:${path}`);
    if (claim.evidence_ids.some((evidenceId) => !allowedEvidenceIds.has(evidenceId))) {
      fail(`${scenarioContract.id} final claim evidence_ids does not match result-bound evidence`);
    }
    if (sourceEvidenceIds.some((evidenceId) => !claim.evidence_ids.includes(evidenceId))) {
      fail(`${scenarioContract.id} final claim evidence_ids omit successful source evidence`);
    }
  } else {
    const allowedEvidenceIds = new Set(expected.evidence_ids);
    if (claim.evidence_ids.some((evidenceId) => !allowedEvidenceIds.has(evidenceId))
        || (allowedEvidenceIds.size > 0 && claim.evidence_ids.length === 0)) {
      fail(`${scenarioContract.id} final claim evidence_ids does not match result-bound evidence`);
    }
    const context = actions.filter((action) => action.kind === "context").at(-1);
    if (context) {
      const body = results.get(context).body;
      const targetEvidenceIds = new Set(body.evidence
        .filter(({ symbol_id: symbolId }) => symbolId === body.target.symbol_id)
        .map(({ identity }) => identity.evidence_id));
      if (targetEvidenceIds.size > 0
          && !claim.evidence_ids.some((evidenceId) => targetEvidenceIds.has(evidenceId))) {
        fail(`${scenarioContract.id} final claim evidence_ids omit the selected target evidence`);
      }
    }
  }
  if (claim.runtime_execution_claim) fail(`${scenarioContract.id} final claim makes a runtime execution claim`);
  if (claim.absence_claim) fail(`${scenarioContract.id} final claim absence_claim contradicts Unknown or retrieval authority`);
}

function sedPrefix(text, endLine) {
  const lines = text.match(/[^\n]*\n|[^\n]+$/gu) ?? [];
  if (lines.length > endLine) return null;
  return lines.join("");
}

function authenticatedCodexGuidanceRead(action, installedPluginRoot, expectedIdentity) {
  if (!["shell", "source_read"].includes(action.kind)
      || !action.completed || action.error || typeof action.result !== "string") {
    return null;
  }
  const command = unwrapCodexShell(action.command);
  if (!command || !installedPluginRoot) return null;
  let root;
  try {
    root = realpathSync(installedPluginRoot);
  } catch {
    return null;
  }
  const segments = command.split(/(?:[ \t]+&&[ \t]+|[ \t]*;[ \t]*|\r?\n)/u);
  const trailingCandidate = segments.length > 1 ? sourceReadPath(segments.at(-1)) : null;
  let trailingIsGuidance = false;
  if (trailingCandidate && isAbsolute(trailingCandidate)) {
    try {
      const actual = realpathSync(trailingCandidate);
      const rel = relative(root, actual);
      trailingIsGuidance = Boolean(rel) && rel !== ".." && !rel.startsWith(`..${sep}`)
        && resolve(root, rel) === actual && CODEX_GUIDANCE_PATHS.has(rel.split(sep).join("/"));
    } catch {
      trailingIsGuidance = false;
    }
  }
  const trailingSourcePath = trailingIsGuidance ? null : trailingCandidate;
  if (trailingSourcePath) segments.pop();
  const reads = segments.map((segment) => {
    const trimmed = segment.trim();
    const sed = trimmed.match(/^(?:(?:\/usr)?\/bin\/)?sed\s+-n\s+(?:'1,(\d+)p'|"1,(\d+)p"|1,(\d+)p)\s+(\S+)$/u);
    if (sed) return { kind: "sed", endLine: Number(sed[1] ?? sed[2] ?? sed[3]), words: [sed[4]] };
    const cat = trimmed.match(/^(?:(?:\/usr)?\/bin\/)?cat\s+(.+)$/u);
    if (cat) {
      const words = [];
      let rest = cat[1].trim();
      while (rest) {
        const match = rest.match(/^('[^']*'|"[^"$`\\]*"|\/?[A-Za-z0-9._@+-]+(?:\/[A-Za-z0-9._@+-]+)*)(?:\s+|$)/u);
        if (!match || !singleShellWord(match[1])) return null;
        words.push(match[1]);
        rest = rest.slice(match[0].length).trimStart();
      }
      return words.length > 0 ? { kind: "cat", words } : null;
    }
    const wc = trimmed.match(/^(?:(?:\/usr)?\/bin\/)?wc\s+-l\s+(.+)$/u);
    if (!wc) return null;
    const words = [];
    let rest = wc[1].trim();
    while (rest) {
      const match = rest.match(/^('[^']*'|"[^"$`\\]*"|\/?[A-Za-z0-9._@+-]+(?:\/[A-Za-z0-9._@+-]+)*)(?:\s+|$)/u);
      if (!match || !singleShellWord(match[1])) return null;
      words.push(match[1]);
      rest = rest.slice(match[0].length).trimStart();
    }
    return words.length > 0 ? { kind: "wc", words } : null;
  });
  if (reads.length === 0 || reads.some((read) => read === null)) return null;
  const paths = new Set();
  let remainingOutput = action.result;
  for (const read of reads) {
    let totalLines = 0;
    for (const word of read.words) {
      const candidate = singleShellWord(word);
      if (!candidate) return null;
      const normalizedCandidate = candidate.replaceAll("\\", "/");
      const candidatePath = isAbsolute(candidate)
        ? candidate
        : resolve(root, "skills/codestory-grounding", normalizedCandidate);
      let actual;
      try {
        if (!normalizedCandidate.startsWith("references/") && !isAbsolute(candidate)) return null;
        if (!lstatSync(candidatePath).isFile()) return null;
        actual = realpathSync(candidatePath);
      } catch {
        return null;
      }
      const rel = relative(root, actual);
      if (!rel || rel === ".." || rel.startsWith(`..${sep}`) || resolve(root, rel) !== actual) return null;
      const rosterPath = rel.split(sep).join("/");
      if (!CODEX_GUIDANCE_PATHS.has(rosterPath)) return null;
      const expectedDigest = expectedIdentity.static_roster?.[rosterPath];
      if (!SHA256.test(String(expectedDigest)) || /^0{64}$/u.test(String(expectedDigest))) return null;
      const bytes = readFileSync(actual);
      if (sha256Bytes(bytes) !== expectedDigest) return null;
      paths.add(rosterPath);
      if (read.kind === "sed" || read.kind === "cat") {
        if (read.kind === "sed" && (!Number.isSafeInteger(read.endLine) || read.endLine < 1)) return null;
        const output = read.kind === "cat"
          ? bytes.toString("utf8")
          : sedPrefix(bytes.toString("utf8"), read.endLine);
        if (output === null || !remainingOutput.startsWith(output)) return null;
        remainingOutput = remainingOutput.slice(output.length);
      } else {
        const newlineCount = bytes.reduce((count, byte) => count + Number(byte === 0x0a), 0);
        totalLines += newlineCount;
        const line = remainingOutput.match(/^[ \t]*(\d+)[ \t]+([^\n]+)\n/u);
        if (!line || Number(line[1]) !== newlineCount || line[2] !== candidate) return null;
        remainingOutput = remainingOutput.slice(line[0].length);
      }
    }
    if (read.kind === "wc" && read.words.length > 1) {
      const total = remainingOutput.match(/^[ \t]*(\d+)[ \t]+total\n/u);
      if (!total || Number(total[1]) !== totalLines) return null;
      remainingOutput = remainingOutput.slice(total[0].length);
    }
  }
  if (!trailingSourcePath && remainingOutput !== "") return null;
  return {
    paths: [...paths],
    sourcePath: trailingSourcePath,
    sourceOutput: trailingSourcePath ? remainingOutput : null,
  };
}

function authenticatedCursorGuidanceRead(action, installedPluginRoot, expectedIdentity) {
  if (action.kind !== "source_read" || !action.completed || action.error
      || !plainObject(action.result) || !isAbsolute(action.path) || !installedPluginRoot) {
    return false;
  }
  let root;
  let actual;
  try {
    root = realpathSync(installedPluginRoot);
    actual = realpathSync(action.path);
  } catch {
    return false;
  }
  const rel = relative(root, actual);
  if (!rel || rel === ".." || rel.startsWith(`..${sep}`) || resolve(root, rel) !== actual) return false;
  const rosterPath = rel.split(sep).join("/");
  if (!CODEX_GUIDANCE_PATHS.has(rosterPath)) return false;
  const expectedDigest = expectedIdentity.static_roster?.[rosterPath];
  const bytes = readFileSync(actual);
  if (!SHA256.test(String(expectedDigest)) || sha256Bytes(bytes) !== expectedDigest) return false;
  const result = action.result;
  const contentKey = Object.hasOwn(result, "content") ? "content" : "contentBlobId";
  const resultKeys = [
    contentKey, "exceededLimit", "fileSize", "isEmpty", "path", "readRange",
    "relatedCursorRulePaths", "relatedCursorRules", "totalLines",
  ].sort();
  const text = bytes.toString("utf8");
  const totalLines = text.length === 0 ? 0 : text.split("\n").length;
  if (!equalJson(Object.keys(result).sort(), resultKeys)
      || !["content", "contentBlobId"].includes(contentKey)
      || result.exceededLimit !== false || result.isEmpty !== (bytes.length === 0)
      || result.fileSize !== bytes.length || result.path !== action.path
      || !equalJson(result.readRange, { startLine: 1, endLine: totalLines })
      || result.totalLines !== totalLines
      || !equalJson(result.relatedCursorRulePaths, []) || !equalJson(result.relatedCursorRules, [])) {
    return false;
  }
  if (contentKey === "content") return result.content === text;
  return result.contentBlobId === createHash("sha256").update(bytes).digest("base64");
}

function authenticatedCursorToolDiscovery(action, installedPluginRoot, expectedIdentity) {
  if (action.kind !== "cursor_tool_discovery" || !action.completed || action.error
      || !plainObject(action.result) || !nonemptyString(action.result.content) || !installedPluginRoot) {
    return false;
  }
  let root;
  let catalogPath;
  try {
    root = realpathSync(installedPluginRoot);
    catalogPath = realpathSync(resolve(root, "generated-mcp-catalog.json"));
  } catch {
    return false;
  }
  if (relative(root, catalogPath) !== "generated-mcp-catalog.json") return false;
  const bytes = readFileSync(catalogPath);
  const expectedDigest = expectedIdentity.static_roster?.["generated-mcp-catalog.json"];
  if (!SHA256.test(String(expectedDigest)) || sha256Bytes(bytes) !== expectedDigest
      || !equalJson(Object.keys(action.result), ["content"])) {
    return false;
  }
  let catalog;
  let observed;
  try {
    catalog = JSON.parse(bytes.toString("utf8"));
    observed = JSON.parse(action.result.content);
  } catch {
    return false;
  }
  if (!Array.isArray(catalog.tools)) return false;
  if (nonemptyString(action.args.toolName)) {
    const matches = catalog.tools.filter(({ name }) => name === action.args.toolName);
    return matches.length === 1 && equalJson(observed, {
      tool: matches[0].name,
      description: matches[0].description,
      inputSchema: matches[0].inputSchema,
    });
  }
  if (!nonemptyString(action.args.pattern)
      || !equalJson(Object.keys(observed).sort(), ["matches", "mode", "pattern"])
      || observed.mode !== "search" || observed.pattern !== action.args.pattern
      || !Array.isArray(observed.matches) || observed.matches.length === 0) {
    return false;
  }
  const catalogByName = new Map(catalog.tools.map((tool) => [tool.name, tool]));
  const seen = new Set();
  return observed.matches.every((match) => {
    if (!plainObject(match)
        || !equalJson(Object.keys(match).sort(), ["description", "namespace", "tool"])
        || match.namespace !== "plugin-codestory-codestory"
        || !nonemptyString(match.tool) || seen.has(match.tool)) {
      return false;
    }
    seen.add(match.tool);
    const expected = catalogByName.get(match.tool);
    return expected?.description === match.description;
  });
}

function productRoutingActions(host, actions, installedPluginRoot, expectedIdentity) {
  const normalizedHost = String(host).toLowerCase();
  if (normalizedHost === "cursor") {
    const product = [];
    const metadata = new Set();
    for (const action of actions) {
      if (authenticatedCursorGuidanceRead(action, installedPluginRoot, expectedIdentity)
          || authenticatedCursorToolDiscovery(action, installedPluginRoot, expectedIdentity)) {
        metadata.add(action);
      } else {
        product.push(action);
      }
    }
    if (actions.some((action) => action.overlaps.some((other) => metadata.has(action) !== metadata.has(other)))) {
      fail("Cursor transcript overlaps authenticated metadata with a product action");
    }
    const productActions = new Set(product);
    if (product.some((action) => action.overlaps.some((other) => productActions.has(other)))) {
      fail("Cursor transcript contains overlapping product actions");
    }
    return product;
  }
  if (normalizedHost !== "codex") return actions;
  const product = [];
  const guidance = new Set();
  for (const action of actions) {
    const authenticatedRead = authenticatedCodexGuidanceRead(action, installedPluginRoot, expectedIdentity);
    if (authenticatedRead) {
      if (authenticatedRead.sourcePath) {
        action.kind = "source_read";
        action.tool = "source_read";
        action.path = authenticatedRead.sourcePath;
        action.result = authenticatedRead.sourceOutput;
        product.push(action);
      } else {
        guidance.add(action);
      }
    } else {
      product.push(action);
    }
  }
  if (actions.some((action) => action.overlaps.some((other) => (
    guidance.has(action) !== guidance.has(other)
  )))) {
    fail("Codex transcript overlaps authenticated installed guidance with a product action");
  }
  const productActions = new Set(product);
  if (product.some((action) => action.overlaps.some((other) => productActions.has(other)))) {
    fail("Codex transcript contains overlapping product actions");
  }
  return product;
}

function normalizeSourceReadPath(path, projectRoot) {
  const raw = String(path ?? "").trim().replace(/^['"]+|['"]+$/gu, "").replaceAll("\\", "/");
  if (!isAbsolute(raw)) return normalizePath(raw);
  if (!nonemptyString(projectRoot)) fail("absolute source read path requires the declared project root");
  const root = resolve(projectRoot);
  const candidate = resolve(raw);
  const rel = relative(root, candidate);
  if (!rel || rel === ".." || rel.startsWith(`..${sep}`)) {
    fail(`source read path escapes the declared project root: ${JSON.stringify(path)}`);
  }
  return normalizePath(rel.split(sep).join("/"));
}

export function validateInstalledSession({
  host,
  scenarioId,
  request,
  installedRoot,
  installedReceipt,
  expectedIdentity,
  installedPluginRoot = null,
  transcript,
}) {
  const scenarioContract = requireSupportedRoutingScenario(scenarioId);
  if (!plainObject(request)) fail(`${scenarioId} request must be an object`);
  if (request.proof_contract != null) fail(`${scenarioId} cannot contain an unsupported proof contract`);
  authenticateInstalledIdentity(installedRoot, installedReceipt, expectedIdentity);
  const normalizedHost = String(host).toLowerCase();
  if (normalizedHost === "cursor") {
    authenticateCursorInstalledPluginRoot(installedPluginRoot, expectedIdentity);
  }
  const parsed = parseInstalledTranscript(host, transcript);
  if (normalizedHost === "cursor" && parsed.user_text !== request.text) {
    fail(`${scenarioId} Cursor user text does not match the declared request`);
  }
  const productActions = productRoutingActions(host, parsed.actions, installedPluginRoot, expectedIdentity);
  const actions = collapsePreparingRetries(productActions, expectedIdentity, normalizedHost);
  for (const action of actions) {
    if (action.kind === "source_read") action.path = normalizeSourceReadPath(action.path, request.project_root);
  }
  validateExpectedMcpAvailability(scenarioContract, actions);
  validateActionOrder(scenarioContract, actions);
  validateSearchQueries(scenarioContract, actions);

  const results = new Map();
  for (const action of actions) {
    if (!action.completed) fail(`${scenarioId} has an incomplete ${action.tool} action`);
    if (["search", "context", "packet"].includes(action.kind)) {
      validateToolInputSchema(action);
      results.set(action, validateResultIdentity(action, expectedIdentity, normalizedHost));
    } else {
      results.set(action, normalizedResult(action, normalizedHost));
    }
    const allowedOptionalSourceFailure = action.kind === "source_read"
      && scenarioContract.optional_followups.includes("source_read")
      && action.error;
    if (results.get(action).isError && !allowedOptionalSourceFailure) {
      fail(`${scenarioId} has an unexpected failed ${action.tool} action`);
    }
    if (["search", "context", "packet"].includes(action.kind)) {
      validateToolResultSchema(action, results.get(action));
    }
  }

  validateSourceReads(scenarioContract, request, actions, results);
  validatePacketContinuation(scenarioContract, actions, results);
  validateSelectedContext(scenarioContract, request, actions, results);
  validateFinalClaims(scenarioContract, parsed.final, actions, results);

  return {
    schema_version: 1,
    status: "pass",
    host: String(host).toLowerCase(),
    scenario_id: scenarioId,
    identity_binding: "exact",
    actions: actions.map(actionName),
    proof_disposition: null,
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

function validateSupportedRoutingCatalog(catalog) {
  const tools = new Map(catalog.tools?.map((tool) => [tool.name, tool]) ?? []);
  if (tools.has("verify_indexed_direct_calls")) fail("routing catalog unexpectedly exposes the retired proof verifier");
  for (const name of ["search", "context", "packet"]) {
    const tool = tools.get(name);
    if (!plainObject(tool?.inputSchema) || !plainObject(tool?.outputSchema)) {
      fail(`routing catalog lacks supported ${name} contracts`);
    }
  }
  if (!equalJson(catalog.tools, GENERATED_MCP_CATALOG.tools)) {
    fail("routing catalog does not match the supported generated catalog");
  }
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
  const cursorMcp = await readJson(resolve(root, "mcp.cursor.json"), "Cursor MCP manifest");
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
  if (!equalJson(catalog.wireContract?.discoveryContracts, expectedIdentity.protocol.discovery_contracts)) {
    fail("catalog discovery roster does not match expected discovery identity");
  }
  const server = mcp.mcpServers?.codestory;
  if (server?.command !== "node"
      || !Array.isArray(server.args)
      || server.args.length !== 1
      || server.args[0] !== "${PLUGIN_ROOT}/scripts/codestory-mcp.cjs") {
    fail("portable MCP metadata does not bind the canonical launcher");
  }
  const cursorServer = cursorMcp.mcpServers?.codestory;
  if (cursorServer?.command !== "node"
      || !Array.isArray(cursorServer.args)
      || cursorServer.args.length !== 2
      || cursorServer.args[0] !== "-e"
      || typeof cursorServer.args[1] !== "string"
      || !cursorServer.args[1].includes("Module.runMain()")
      || !cursorServer.args[1].includes("codestory_cursor_mcp_launcher_not_found")) {
    fail("Cursor MCP metadata does not bind the canonical launcher resolver");
  }
  validateSupportedRoutingCatalog(catalog);
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
    const expectedHook = host === "claude_code" || host === "cursor"
      ? `./${inputs.hook}`
      : inputs.hook;
    if (metadata.hooks !== expectedHook) fail(`${host} metadata does not bind its declared hook`);
    if (host === "cursor" && metadata.mcpServers !== "./mcp.cursor.json") {
      fail("cursor metadata does not bind its declared MCP manifest");
    }
    if (host.startsWith("copilot") && metadata.skills !== "skills/") {
      fail(`${host} metadata does not bind the canonical rule/skill directory`);
    }
    if (host === "cursor") {
      const sessionStart = hook.hooks?.sessionStart;
      if (hook.version !== 1 || !Array.isArray(sessionStart) || sessionStart.length !== 1) {
        fail("cursor hook structure is invalid");
      }
      const command = sessionStart[0];
      requireExactKeys(command, ["command", "timeout"], "cursor hook command");
      if (command.command !== "node \"${CURSOR_PLUGIN_ROOT}/hooks/codestory-activate.cjs\""
          || command.timeout !== 300) fail("cursor hook command is not the canonical launcher");
    } else if (host === "claude_code") {
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
    if (host === "cursor") {
      if (!/^---\ndescription: Load the canonical CodeStory grounding skill\.\n/u.test(ruleText)
          || !ruleText.includes("alwaysApply: true")
          || !ruleText.includes("[canonical codestory-grounding skill](../skills/codestory-grounding/SKILL.md)")
          || !ruleText.includes("sole source of truth")
          || !ruleText.includes("adds no parallel instructions")) {
        fail("cursor rule is not the canonical skill pointer");
      }
      if (/Routing contract:|Discovery leads come from|verify_indexed_direct_calls|Inspect source after a packet/u.test(ruleText)) {
        fail("cursor rule duplicates the canonical grounding contract");
      }
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
