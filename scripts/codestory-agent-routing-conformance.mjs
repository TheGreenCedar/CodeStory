#!/usr/bin/env node

import { createHash } from "node:crypto";
import { lstatSync, readFileSync, realpathSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const SHA256 = /^[0-9a-f]{64}$/u;
const PROOF_CONTRACT_DIGEST_DOMAIN = Buffer.from("codestory.proof-contract.digest.v1\0", "utf8");
const PROOF_FACT_ID_DOMAIN = Buffer.from("codestory-proof-resolution-fact-id-v1\0", "utf8");
const PROOF_DISPOSITIONS = new Set([
  "contract_proven",
  "contract_refuted",
  "unknown",
  "unavailable",
]);
const GENERATED_MCP_CATALOG = JSON.parse(readFileSync(
  new URL("../plugins/codestory/generated-mcp-catalog.json", import.meta.url),
  "utf8",
));
const ROUTING_CORPUS_DOCUMENT = JSON.parse(readFileSync(
  new URL("./fixtures/codestory-agent-routing-corpus-v1.json", import.meta.url),
  "utf8",
));
const GENERATED_TOOL_SCHEMAS = new Map(GENERATED_MCP_CATALOG.tools.map((tool) => [tool.name, tool]));
const PROVE_CALL_PATH_INPUT_SCHEMA = GENERATED_TOOL_SCHEMAS.get("prove_call_path")?.inputSchema;
const ROUTING_ACTIONS = Object.freeze([
  "source_read",
  "search",
  "context",
  "packet",
  "prove_call_path",
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
  required = [],
  forbidden = [],
  disposition = null,
  typedContract = "none",
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
    packet_named_fallback_to_source: { authority: "source", outcome: "supported" },
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
    optional_prefixes: optionalPrefixes,
    optional_followups: optionalFollowups,
    permitted_followups: [...followups, ...optionalFollowups],
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
    optionalFollowups: ["source_read"],
    source: "user_named_file",
    required: ["one bounded continuation", "gap-1"],
    forbidden: NO_PROOF_CLAIMS,
  }),
  scenario({
    id: "packet_gap_to_focused_source",
    first: "packet",
    optionalFollowups: ["source_read"],
    source: "packet_evidence_gap",
    required: ["gap-1", "source"],
    forbidden: NO_PROOF_CLAIMS,
  }),
  scenario({
    id: "packet_named_fallback_to_source",
    first: "packet",
    followups: ["source_read"],
    source: "user_named_file",
    required: ["packet", "source"],
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
    first: "prove_call_path",
    optionalPrefixes: ["tool_search"],
    required: ["only prove_call_path", "ContractProven"],
    disposition: "contract_proven",
    typedContract: "valid",
  }),
]);

const SCENARIOS_BY_ID = new Map(ROUTING_SCENARIOS.map((entry) => [entry.id, entry]));

export function validateRoutingRequestCorpus(document = ROUTING_CORPUS_DOCUMENT) {
  requireExactKeys(document, ["schema_version", "scenarios"], "routing request corpus");
  if (document.schema_version !== 1 || !Array.isArray(document.scenarios)) fail("routing request corpus is invalid");
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
    const scenarioContract = SCENARIOS_BY_ID.get(entry.id);
    if (scenarioContract.typed_contract === "forbidden" && entry.request.proof_contract !== null) {
      fail(`${entry.id} must not contain a proof contract`);
    }
    if (["valid", "malformed"].includes(scenarioContract.typed_contract)) {
      validateProofCallInputAgainstCatalog({ project: "/routing-fixture", ...entry.request.proof_contract });
      const semanticallyValid = validTypedContract(entry.request.proof_contract);
      if ((scenarioContract.typed_contract === "valid") !== semanticallyValid) {
        fail(`${entry.id} proof contract does not match its declared semantic boundary`);
      }
    } else if (entry.request.proof_contract !== null) {
      fail(`${entry.id} unexpectedly contains a proof contract`);
    }
  });
  return true;
}

export const ROUTING_REQUEST_CORPUS = deepFreeze(structuredClone(ROUTING_CORPUS_DOCUMENT));

const FINAL_REPORT_INSTRUCTION = `Read an already named linked installed-guidance file only with a direct file read; never grep, rg, search, or probe the installed plugin package. When the scenario authorizes a direct source read, use the host's direct file-read action; never substitute CodeStory snippet or another MCP tool. An exact path appearing only in a CodeStory evidence row is not source-read authorization; do not read it unless the request or a material result gap separately authorizes that exact read. Do not add evidence through globbing, directory listing, repository search, shell commands, or another external repository tool; only the scenario-authorized direct source reads and CodeStory actions are permitted. Finish with only one raw JSON object and no markdown fence, explanation, prefix, or suffix, using exactly these keys: authority, outcome, target_id, evidence_ids, gap_ids, reason_codes, proof_disposition, refutation_basis, runtime_execution_claim, absence_claim, material_omissions. authority must be exactly one of source, search_lead, context_evidence, packet_evidence, typed_proof, none, chosen from the final evidence authority you actually used. outcome must be exactly one of supported, discovery_only, refuted, unknown, unavailable, invalid_contract, refused. Use supported for a direct source read unless it follows an unavailable CodeStory result: an authorized fallback read changes evidence authority but preserves an earlier unavailable outcome. Use supported only when the selected evidence authority resolves the requested material; if result-bound gaps leave any requested material unresolved, use unknown even when the tool result also returned useful evidence. Use discovery_only for a search lead, and preserve the exact proof result boundary. For a rejected typed interpretation, use authority none, outcome invalid_contract, and no proof disposition. Copy its reason code only when the tool payload supplies a machine-readable code explicitly; when it returns only human-readable validation text, keep reason_codes empty and never derive a code from that text. Use refused only when the user requested exact proof without supplying a typed interpretation; in that case call no product tool and do not substitute retrieval or source evidence. diagnostics.availability describes only the optional diagnostics artifact: never copy it into outcome or reason_codes, and determine result availability from top-level status and result-bound gaps. Use null for absent scalar identities and [] for absent lists. target_id must be null unless a CodeStory tool result returned a target identity. For a direct source read, record evidence identity source:<project-relative-path>; for CodeStory tool calls, copy evidence, gap, disposition, target, and refutation identities only from the tool results. For typed proof, evidence_ids contains only receipt_id values referenced by the disposition; never copy fact_id or edge_id. A typed proof gap has no gap_id: keep gap_ids empty and copy each disposition.gaps[].kind into reason_codes. Other reason_codes may contain only CodeStory tool result codes or typed_contract_required; use typed_contract_required only for a refused free-English proof request. refutation_basis must be null unless a ContractRefuted result supplied the basis; when supplied, copy only the refutation.kind string, never the whole refutation object. runtime_execution_claim and absence_claim must each be false. material_omissions contains only unresolved material requested by the user; limitations outside the requested claim are not omissions, so use [] when the request was fully answered within the selected authority. Never claim runtime execution or absence and never omit a material requested gap.`;

export function materializeRoutingRequests(projectRoot) {
  const project = realpathSync(projectRoot);
  return ROUTING_REQUEST_CORPUS.scenarios.map((entry) => {
    const proofInstruction = entry.request.proof_contract === null
      ? ""
      : `\nThe unchanged host-supplied proof contract is: ${JSON.stringify(entry.request.proof_contract)}`;
    return {
      scenario_id: entry.id,
      request: {
        ...structuredClone(entry.request),
        project_root: project,
        text: `${entry.prompt}\nThe exact project root for repository work is ${project}.${proofInstruction}\n${FINAL_REPORT_INSTRUCTION}`,
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

export function validateProofCallInputAgainstCatalog(input) {
  if (!plainObject(PROVE_CALL_PATH_INPUT_SCHEMA)
      || !matchesJsonSchema(input, PROVE_CALL_PATH_INPUT_SCHEMA)) {
    fail("prove_call_path input schema does not match the generated catalog");
  }
  return true;
}

function compareUtf8(left, right) {
  return Buffer.compare(Buffer.from(left), Buffer.from(right));
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
    /^wc\s+-l\s+(\S+)\s+&&\s+sed\s+-n\s+(?:'\d+,\d+p'|"\d+,\d+p"|\d+,\d+p)\s+(\S+)$/u,
  );
  if (countedRead) {
    const countedPath = singleShellWord(countedRead[1]);
    const readPath = singleShellWord(countedRead[2]);
    return countedPath && countedPath === readPath ? readPath : null;
  }
  const patterns = [
    /^sed\s+-n\s+(?:'\d+,(?:\d+|\$)p'|"\d+,\d+p"|\d+,\d+p)\s+(.+)$/u,
    /^(?:cat|type|nl)(?:\s+-[A-Za-z]+)*\s+(.+)$/u,
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

function proofFactId(evidenceSha256) {
  const length = Buffer.alloc(8);
  length.writeBigUInt64BE(BigInt(Buffer.byteLength(evidenceSha256)));
  return sha256Bytes(Buffer.concat([PROOF_FACT_ID_DOMAIN, length, Buffer.from(evidenceSha256)]));
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
      && typeof raw.content[0].text.text === "string") {
    return {
      raw,
      body: parseJsonText(raw.content[0].text.text),
      meta: null,
      isError: false,
      transport_projection: "cursor_content_text_v1",
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
  if (!plainObject(runtime) && (projected || action.kind !== "prove_call_path")) {
    fail(`${action.tool} result identity requires runtime identity outside the native proof result contract`);
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
    ["search", "context", "packet", "prove_call_path"].includes(kind)
  )));
  for (const action of actions) {
    const expectedSemanticError = scenarioContract.typed_contract === "malformed"
      && action.kind === "prove_call_path";
    if (expected.has(action.kind) && action.error && !expectedSemanticError) {
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
    const retry = actions[index + 1];
    if (!retry || retry.kind !== action.kind || retry.tool !== action.tool
        || !equalJson(retry.args, action.args)) {
      fail(`${action.tool} preparing result must be followed directly by the same tool and arguments`);
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
    const hasPath = Object.hasOwn(selector, "project_file_components");
    requireExactKeys(selector, hasPath
      ? ["kind", "qualified_name", "project_file_components"]
      : ["kind", "qualified_name"], label);
    if (!nonemptyString(selector.qualified_name) || (hasPath
      && !(selector.project_file_components === null || (Array.isArray(selector.project_file_components)
        && selector.project_file_components.length > 0)))) fail(`${label} is invalid`);
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

function normalizeTypedContract(contract) {
  requireExactKeys(contract, ["source_text", "clauses", "spec"], "typed proof contract");
  if (!nonemptyString(contract.source_text) || !Array.isArray(contract.clauses) || !plainObject(contract.spec)) {
    fail("typed proof contract source_text, clauses, and spec are required");
  }
  requireExactKeys(contract.spec, ["start", "steps", "prohibit_traversal_through", "exclude_from_projection"], "typed proof spec");
  if (!Array.isArray(contract.spec.steps) || !Array.isArray(contract.spec.prohibit_traversal_through)
      || !Array.isArray(contract.spec.exclude_from_projection)) fail("typed proof spec arrays are invalid");
  return {
    source_text: contract.source_text,
    clauses: contract.clauses.map((clause, index) => {
      requireExactKeys(
        clause,
        ["clause_id", "start_byte", "end_byte_exclusive", "quote", "classification"],
        `typed proof contract clause ${index}`,
      );
      if (!plainObject(clause.classification) || !nonemptyString(clause.classification.kind)) {
        fail(`typed proof contract clause ${index} classification is invalid`);
      }
      let fields = [];
      let reason = null;
      let nonMaterialKind = null;
      if (clause.classification.kind === "resolved_material") {
        requireExactKeys(clause.classification, ["kind", "fields"], `typed proof contract clause ${index} classification`);
        fields = clause.classification.fields;
      } else if (clause.classification.kind === "unresolved_material") {
        requireExactKeys(clause.classification, ["kind", "reason"], `typed proof contract clause ${index} classification`);
        reason = clause.classification.reason;
      } else if (clause.classification.kind === "non_material") {
        requireExactKeys(clause.classification, ["kind", "reason"], `typed proof contract clause ${index} classification`);
        nonMaterialKind = clause.classification.reason;
      } else {
        fail(`typed proof contract clause ${index} classification is invalid`);
      }
      return {
        start: clause.start_byte,
        end: clause.end_byte_exclusive,
        clause_id: clause.clause_id,
        quote: clause.quote,
        classification: clause.classification.kind,
        fields,
        reason,
        non_material_kind: nonMaterialKind,
      };
    }),
    spec: {
      start: contract.spec.start,
      steps: contract.spec.steps.map((step, index) => {
        requireExactKeys(step, ["target"], `typed proof step ${index}`);
        return { relation: "direct_outgoing_call", target: step.target };
      }),
      prohibit_traversal_through: contract.spec.prohibit_traversal_through,
      exclude_from_projection: contract.spec.exclude_from_projection,
    },
  };
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
    contract = normalizeTypedContract(contract);
    if (contract.clauses.length === 0) {
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

const PROOF_FIELD_RANK = Object.freeze({
  start: 0,
  step_target: 1,
  directness: 2,
  ordering: 3,
  relation: 4,
  traversal_prohibition: 5,
  projection_exclusion: 6,
});
const UNRESOLVED_REASON_RANK = Object.freeze({
  missing_selector_resolution: 0,
  ambiguous_selector_resolution: 1,
  unsupported_interpretation: 2,
});
const NON_MATERIAL_RANK = Object.freeze({ whitespace: 0, punctuation: 1, connector: 2, commentary: 3 });

function normalizedFieldOrder(field) {
  return [PROOF_FIELD_RANK[field.kind], field.step ?? field.index ?? -1];
}

function normalizedClassificationOrder(row) {
  if (row.classification === "resolved_material") return [0, 0];
  if (row.classification === "unresolved_material") return [1, UNRESOLVED_REASON_RANK[row.reason]];
  return [2, NON_MATERIAL_RANK[row.non_material_kind]];
}

function compareTuples(left, right) {
  for (let index = 0; index < Math.max(left.length, right.length); index += 1) {
    if (left[index] === right[index]) continue;
    return left[index] < right[index] ? -1 : 1;
  }
  return 0;
}

function normalizedContractClauses(contract) {
  const rows = [];
  for (const clause of contract.clauses) {
    if (clause.classification === "resolved_material") {
      const fields = [...clause.fields].sort((left, right) => compareTuples(normalizedFieldOrder(left), normalizedFieldOrder(right)));
      for (const field of fields) {
        rows.push({
          start: clause.start,
          end: clause.end,
          clause_id: clause.clause_id,
          quote: clause.quote,
          classification: clause.classification,
          field,
          reason: null,
          non_material_kind: null,
        });
      }
    } else {
      rows.push({
        start: clause.start,
        end: clause.end,
        clause_id: clause.clause_id,
        quote: clause.quote,
        classification: clause.classification,
        field: null,
        reason: clause.reason,
        non_material_kind: clause.non_material_kind,
      });
    }
  }
  rows.sort((left, right) => left.start - right.start
    || left.end - right.end
    || compareUtf8(left.clause_id, right.clause_id)
    || compareTuples(normalizedClassificationOrder(left), normalizedClassificationOrder(right))
    || compareTuples(left.field === null ? [-1, -1] : normalizedFieldOrder(left.field), right.field === null ? [-1, -1] : normalizedFieldOrder(right.field))
    || compareUtf8(left.quote, right.quote));
  return rows.filter((row, index) => index === 0 || !equalJson(row, rows[index - 1]));
}

function groupedContractClauses(contract) {
  const grouped = [];
  for (const row of normalizedContractClauses(contract)) {
    const prior = grouped.at(-1);
    const sameGroup = prior !== undefined
      && prior.start === row.start
      && prior.end === row.end
      && prior.clause_id === row.clause_id
      && prior.quote === row.quote
      && prior.classification === row.classification
      && prior.reason === row.reason
      && prior.non_material_kind === row.non_material_kind;
    if (sameGroup) {
      prior.fields.push(row.field);
    } else {
      grouped.push({
        start: row.start,
        end: row.end,
        clause_id: row.clause_id,
        quote: row.quote,
        classification: row.classification,
        fields: row.field === null ? [] : [row.field],
        reason: row.reason,
        non_material_kind: row.non_material_kind,
      });
    }
  }
  return grouped;
}

export function canonicalRequestContractDigest(contract) {
  if (!validTypedContract(contract)) fail("typed proof contract is not canonicalizable");
  const normalized = normalizeTypedContract(contract);
  const sourceTextSha256 = sha256Bytes(Buffer.from(normalized.source_text));
  const document = {
    schema_version: 1,
    proof_domain: "indexed_source_call_path_v1",
    guard_version: "clause_guard_v1",
    source_text_sha256: sourceTextSha256,
    clauses: normalizedContractClauses(normalized),
    spec: normalized.spec,
  };
  return sha256Bytes(Buffer.concat([PROOF_CONTRACT_DIGEST_DOMAIN, Buffer.from(canonical(document))]));
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

const SELECTOR_PROOF_GAP_RANKS = Object.freeze({ selector_missing: 0, selector_ambiguous: 1, non_callable_selector: 2 });
const STEP_PROOF_GAP_RANKS = Object.freeze({
  direct_call_missing: 3,
  recursive_call_not_representable: 4,
  source_window_too_large: 5,
  invalid_utf8: 6,
  source_line_out_of_range: 7,
  edge_containment_unproven: 8,
  missing_direct_call_receipt: 9,
  receipt_or_edge_already_used: 10,
  projection_exclusion_conflicts_with_required_receipt: 11,
});

function canonicalProofGapKey(gap, index, stepCount) {
  if (Object.hasOwn(SELECTOR_PROOF_GAP_RANKS, gap?.kind)) {
    requireExactKeys(gap, ["kind", "selector_index"], `proof result gap ${index}`);
    if (!Number.isInteger(gap.selector_index) || gap.selector_index < 0 || gap.selector_index > stepCount) {
      proofSemanticFail(`gap index ${gap.selector_index} exceeds selector boundary ${stepCount}`);
    }
    return [SELECTOR_PROOF_GAP_RANKS[gap.kind], gap.selector_index];
  }
  if (Object.hasOwn(STEP_PROOF_GAP_RANKS, gap?.kind)) {
    requireExactKeys(gap, ["kind", "step_index"], `proof result gap ${index}`);
    if (!Number.isInteger(gap.step_index) || gap.step_index < 0 || gap.step_index >= stepCount) {
      proofSemanticFail(`gap index ${gap.step_index} exceeds step boundary ${stepCount - 1}`);
    }
    return [STEP_PROOF_GAP_RANKS[gap.kind], gap.step_index];
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

function proofSemanticFail(reason) {
  fail(`proof result semantic invariant failed: ${reason}`);
}

function semanticReceiptSequence(values, receiptCount, label) {
  if (!Array.isArray(values) || new Set(values).size !== values.length) proofSemanticFail(`${label} is not edge-distinct`);
  values.forEach((receipt) => {
    if (!Number.isInteger(receipt) || receipt < 0 || receipt >= receiptCount) proofSemanticFail(`${label} contains an invalid receipt reference`);
  });
  return values;
}

function validateSemanticPrefix(steps, sequence, trailingStatus, terminal = null) {
  const terminalIndex = terminal?.index ?? sequence.length;
  if (sequence.length > steps.length || terminalIndex >= steps.length) proofSemanticFail("receipt prefix exceeds the proof steps");
  for (let index = 0; index < terminalIndex; index += 1) {
    if (steps[index].status !== "proven" || steps[index].receipt !== sequence[index]) {
      proofSemanticFail("ordered proven prefix does not match its receipt sequence");
    }
  }
  if (terminal) {
    const expectedReceipt = terminal.status === "certified_absence" ? null : sequence.at(-1);
    if (steps[terminal.index].status !== terminal.status || steps[terminal.index].receipt !== expectedReceipt) {
      proofSemanticFail("refutation step contradicts its disposition");
    }
  }
  const suffixStart = terminal ? terminal.index + 1 : sequence.length;
  for (const step of steps.slice(suffixStart)) {
    if (step.status !== trailingStatus || step.receipt !== null) proofSemanticFail("proof suffix contradicts its disposition");
  }
}

function dispositionReceiptSequence(result, stepCount) {
  const { disposition, receipts, steps } = result;
  if (disposition.kind === "contract_proven") {
    const sequence = semanticReceiptSequence(disposition.receipts, receipts.length, "ContractProven receipt sequence");
    if (sequence.length !== steps.length) proofSemanticFail("ContractProven must authorize one receipt per step");
    steps.forEach((step, index) => {
      if (step.status !== "proven" || step.receipt !== sequence[index]) proofSemanticFail("ContractProven step contradicts its receipt");
    });
    return sequence;
  }
  if (disposition.kind === "unknown") {
    const sequence = semanticReceiptSequence(disposition.connected_receipts, receipts.length, "Unknown connected receipt sequence");
    let priorGap = null;
    for (const [index, gap] of disposition.gaps.entries()) {
      const key = canonicalProofGapKey(gap, index, stepCount);
      if (priorGap !== null && compareTuples(priorGap, key) >= 0) proofSemanticFail("Unknown gaps are not canonical and edge-distinct");
      priorGap = key;
    }
    validateSemanticPrefix(steps, sequence, "unknown");
    return sequence;
  }
  if (disposition.kind === "contract_refuted") {
    const refutation = disposition.refutation;
    const sequence = semanticReceiptSequence(refutation.connected_receipts, receipts.length, "ContractRefuted connected receipt sequence");
    if (refutation.kind === "prohibited_scope_traversal") {
      if (sequence.length !== refutation.step_index + 1) proofSemanticFail("positive contradiction receipt sequence has the wrong length");
      validateSemanticPrefix(steps, sequence, "unknown", { index: refutation.step_index, status: "positive_contradiction" });
    } else {
      if (sequence.length !== refutation.step_index) proofSemanticFail("certified absence receipt sequence has the wrong length");
      validateSemanticPrefix(steps, sequence, "unknown", { index: refutation.step_index, status: "certified_absence" });
    }
    return sequence;
  }
  if (steps.some((step) => step.status !== "unavailable" || step.receipt !== null)) {
    proofSemanticFail("Unavailable disposition contains a non-Unavailable step or receipt");
  }
  const reasonOrder = [
    "validated_contract_hash_mismatch",
    "publication_pin_mismatch",
    "source_not_bound_to_publication",
    "proof_facts_unavailable",
    "proof_semantic_projection_unavailable",
  ];
  const ranks = disposition.reasons.map((reason) => reasonOrder.indexOf(reason));
  if (ranks.some((rank) => rank < 0) || ranks.some((rank, index) => index > 0 && ranks[index - 1] >= rank)) {
    proofSemanticFail("Unavailable reasons are not canonical");
  }
  return [];
}

function canonicalNonzeroInteger(value) {
  if (typeof value !== "string" || !/^-?[0-9]+$/u.test(value)) return null;
  try {
    const parsed = BigInt(value);
    return parsed !== 0n && parsed.toString() === value ? parsed : null;
  } catch {
    return null;
  }
}

function validateSemanticReceiptTable(result, sequence) {
  if (sequence.length !== result.receipts.length || sequence.some((receipt, index) => receipt !== index)) {
    proofSemanticFail("disposition does not exhaust receipts in canonical order");
  }
  const fileIds = new Set();
  const filePaths = new Set();
  result.identities.files.forEach((file, index) => {
    const fileId = canonicalNonzeroInteger(file.file_node_id);
    if (fileId === null || fileIds.has(file.file_node_id) || !SHA256.test(file.indexed_sha256)
        || !(file.observed_sha256 === null || file.observed_sha256 === file.indexed_sha256)) {
      proofSemanticFail(`file identity ${index} is not canonical and hash-bound`);
    }
    fileIds.add(file.file_node_id);
    if (file.project_file_components !== null) {
      if (file.project_file_components.length === 0) proofSemanticFail(`file identity ${index} has an empty path`);
      const path = canonical(file.project_file_components);
      if (filePaths.has(path)) proofSemanticFail(`file identity ${index} repeats a project path`);
      filePaths.add(path);
    }
  });
  const symbolIds = new Set();
  result.identities.symbols.forEach((symbol, index) => {
    if (symbolIds.has(symbol.node_id)) proofSemanticFail(`symbol identity ${index} is duplicated`);
    symbolIds.add(symbol.node_id);
  });
  const profiles = new Set();
  result.identities.provenance_profiles.forEach((profile, index) => {
    const key = canonical(profile);
    if (profiles.has(key)) proofSemanticFail(`provenance profile ${index} is duplicated`);
    profiles.add(key);
  });
  const factIds = new Set();
  const referencedProfiles = new Set();
  let nextProfile = 0;
  result.identities.evidence.forEach((evidence, index) => {
    if (factIds.has(evidence.fact_id) || proofFactId(evidence.provenance.evidence_sha256) !== evidence.fact_id) {
      proofSemanticFail(`exact-resolution evidence ${index} has an invalid fact identity`);
    }
    factIds.add(evidence.fact_id);
    const caller = result.identities.symbols[evidence.caller];
    const target = result.identities.symbols[evidence.target];
    if (!nonemptyString(caller.canonical_id) || !nonemptyString(caller.qualified_name) || caller.file === null
        || !nonemptyString(target.canonical_id) || !nonemptyString(target.qualified_name) || target.file === null) {
      proofSemanticFail(`exact-resolution evidence ${index} does not bind complete symbols`);
    }
    const chainArities = {
      same_file_declaration: 1,
      same_package_declaration: 1,
      static_import_binding: 2,
      qualified_path: null,
      explicit_receiver_type: 1,
      constructor_binding: 1,
      implicit_receiver: 1,
    };
    for (const entry of evidence.chain) {
      if (!Object.hasOwn(chainArities, entry.kind)
          || (chainArities[entry.kind] === null ? entry.symbols.length === 0 : entry.symbols.length !== chainArities[entry.kind])) {
        proofSemanticFail(`exact-resolution evidence ${index} has an invalid evidence chain`);
      }
    }
    const profile = evidence.provenance.profile;
    if (!referencedProfiles.has(profile)) {
      if (profile !== nextProfile) proofSemanticFail("provenance profiles are not referenced in canonical order");
      referencedProfiles.add(profile);
      nextProfile += 1;
    }
    if (evidence.provenance.dependency_files.length === 0) proofSemanticFail(`exact-resolution evidence ${index} has no dependency files`);
    let priorFileId = null;
    for (const fileIndex of evidence.provenance.dependency_files) {
      const fileId = canonicalNonzeroInteger(result.identities.files[fileIndex].file_node_id);
      if (fileId === null || (priorFileId !== null && priorFileId >= fileId)) {
        proofSemanticFail(`exact-resolution evidence ${index} dependency files are not canonical`);
      }
      priorFileId = fileId;
    }
  });
  if (referencedProfiles.size !== result.identities.provenance_profiles.length) {
    proofSemanticFail("a provenance profile is unreferenced");
  }
  const receiptIds = new Set();
  const edgeIds = new Set();
  const evidenceIndices = new Set();
  const sourceFiles = new Set();
  for (const [index, receipt] of result.receipts.entries()) {
    if (receiptIds.has(receipt.receipt_id) || edgeIds.has(receipt.edge_id) || evidenceIndices.has(receipt.evidence)) {
      proofSemanticFail("receipt identities, edges, and exact-resolution evidence must be edge-distinct");
    }
    receiptIds.add(receipt.receipt_id);
    edgeIds.add(receipt.edge_id);
    evidenceIndices.add(receipt.evidence);
    const source = result.identities.symbols[receipt.source];
    const target = result.identities.symbols[receipt.target];
    if (!nonemptyString(source.canonical_id) || !nonemptyString(source.qualified_name) || source.file === null
        || !nonemptyString(target.canonical_id) || !nonemptyString(target.qualified_name) || target.file === null) {
      proofSemanticFail(`receipt ${index} source and target must be complete symbols`);
    }
    if (source.file !== receipt.containment.file || receipt.containment.file !== receipt.line_window.file) {
      proofSemanticFail(`receipt ${index} source, containment, and line-window files disagree`);
    }
    const file = result.identities.files[source.file];
    if (!nonemptyString(file.file_node_id) || !SHA256.test(file.indexed_sha256)
        || file.observed_sha256 !== file.indexed_sha256) proofSemanticFail(`receipt ${index} line window is not hash-bound`);
    if (receipt.line_window.anchor_line < receipt.containment.start_line
        || receipt.line_window.anchor_line > receipt.containment.end_line
        || receipt.exact_callsite_start_byte < receipt.line_window.byte_start
        || receipt.exact_callsite_start_byte >= receipt.line_window.byte_end) {
      proofSemanticFail(`receipt ${index} callsite is outside its hash-bound source window`);
    }
    const evidence = result.identities.evidence[receipt.evidence];
    sourceFiles.add(source.file);
  }
  if (evidenceIndices.size !== result.identities.evidence.length) proofSemanticFail("exact-resolution evidence is unreferenced");
  result.identities.files.forEach((file, index) => {
    if (file.observed_sha256 !== null && !sourceFiles.has(index)) proofSemanticFail("observed source hash belongs to a non-callsite file");
  });
  for (let index = 1; index < sequence.length; index += 1) {
    if (result.receipts[sequence[index - 1]].target !== result.receipts[sequence[index]].source) {
      proofSemanticFail("receipt trail is disconnected");
    }
  }
}

function validateSelectorReceiptBinding(selector, expectedSymbol, label) {
  const isReference = ["pinned_node_ref", "canonical_id_ref", "qualified_name_ref"].includes(selector.kind);
  if (expectedSymbol === null) {
    if (isReference) proofSemanticFail(`${label} is a disconnected compact reference`);
  } else if (!isReference || selector.symbol !== expectedSymbol) {
    proofSemanticFail(`${label} does not match the authorized receipt endpoint`);
  }
}

function validateCanonicalProofResult(result, contract) {
  const normalized = normalizeTypedContract(contract);
  validateProofResult(result);
  const sequence = dispositionReceiptSequence(result, result.spec.steps.length);
  validateSemanticReceiptTable(result, sequence);
  if (result.source_text_sha256 !== sha256Bytes(Buffer.from(contract.source_text))) {
    proofSemanticFail("source_text_sha256 does not match the unchanged typed request");
  }
  const expectedDigest = canonicalRequestContractDigest(contract);
  if (result.contract_digest !== expectedDigest || result.disposition.contract_digest !== expectedDigest) {
    proofSemanticFail("contract digest is not derived from the unchanged typed request");
  }
  if (!equalJson(result.clauses, groupedContractClauses(normalized))) {
    proofSemanticFail("clauses do not match the canonical unchanged typed request");
  }
  const firstSource = sequence.length === 0 ? null : result.receipts[sequence[0]].source;
  validateSelectorReceiptBinding(result.spec.start, firstSource, "proof start selector");
  if (!equalJson(projectedSelectorValue(result.spec.start, result, "proof result start selector"), normalized.spec.start)) {
    proofSemanticFail("start selector does not match the unchanged typed request");
  }
  if (result.spec.steps.length !== normalized.spec.steps.length) proofSemanticFail("steps do not match the unchanged typed request");
  result.spec.steps.forEach((step, index) => {
    const expectedTarget = index < sequence.length ? result.receipts[sequence[index]].target : null;
    validateSelectorReceiptBinding(step.target, expectedTarget, `proof step ${index} target`);
    const projected = {
      relation: step.relation,
      target: projectedSelectorValue(step.target, result, `proof result step ${index} target`),
    };
    if (!equalJson(projected, normalized.spec.steps[index])) proofSemanticFail(`step ${index} does not match the unchanged typed request`);
  });
  if (!equalJson(result.spec.prohibit_traversal_through, normalized.spec.prohibit_traversal_through)
      || !equalJson(result.spec.exclude_from_projection, normalized.spec.exclude_from_projection)) {
    proofSemanticFail("scope selectors do not match the unchanged typed request");
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
  validateProofCallInputAgainstCatalog(proofCalls[0].args);
  if (!equalJson(stripProject(proofCalls[0].args), request.proof_contract)) {
    fail(`${scenarioContract.id} proof request must preserve the host-supplied typed contract exactly`);
  }
  const isValid = validTypedContract(request.proof_contract);
  if (scenarioContract.typed_contract === "valid" && !isValid) fail(`${scenarioContract.id} requires a complete typed contract`);
  if (scenarioContract.typed_contract === "malformed" && isValid) fail(`${scenarioContract.id} requires the malformed-contract boundary`);

  const projection = results.get(proofCalls[0]);
  const activated = projection?.meta?.codestory_execution?.semantic_retrieval_activated;
  if (activated === true) fail(`${scenarioContract.id} proof activated semantic retrieval`);
  if (scenarioContract.typed_contract === "malformed") {
    if (!projection?.isError || projection.raw?.structuredContent !== undefined) {
      fail(`${scenarioContract.id} malformed semantic contract must return isError without structured content`);
    }
    return;
  }
  if (projection?.isError) fail(`${scenarioContract.id} typed proof unexpectedly returned a tool error`);
  validateCanonicalProofResult(projection.body, request.proof_contract);
}

function validatePacketContinuation(scenarioContract, actions, results) {
  const packets = actions.filter((action) => action.kind === "packet");
  if (packets.length > 2) fail(`${scenarioContract.id} allows at most one packet continuation`);
  if (packets.length > 0) {
    const allowedInitialKeys = new Set([
      "project", "question", "budget", "task_class", "latency_budget_ms",
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
  for (const key of ["budget", "task_class", "latency_budget_ms"]) {
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

function validateHiddenDiscovery(scenarioContract, actions, results) {
  const searches = actions.filter((action) => action.kind === "tool_search");
  if (scenarioContract.id !== "hidden_proof_tool_discovery") {
    if (searches.length > 0) fail(`${scenarioContract.id} hidden-tool discovery is forbidden`);
    return;
  }
  if (searches.length === 0) return;
  if (searches.length !== 1) fail(`${scenarioContract.id} allows at most one hidden-tool discovery`);
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
  if (!Array.isArray(claim.material_omissions)
      || !claim.material_omissions.every(nonemptyString)) {
    fail(`${scenarioId} final claim material_omissions must be an array of nonempty strings`);
  }
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
  const reads = actions.filter((action) => action.kind === "source_read" && action.completed && !action.error);

  if (contexts.length > 0) {
    const body = results.get(contexts.at(-1)).body;
    expected.target_id = body.target.symbol_id;
    expected.evidence_ids = body.evidence.map(({ identity }) => identity.evidence_id);
  } else if (searches.length > 0) {
    const body = results.get(searches.at(-1)).body;
    if (body.evidence.length === 1) expected.target_id = body.evidence[0].symbol_id;
    expected.evidence_ids = body.evidence.map(({ identity }) => identity.evidence_id);
  } else if (packets.length > 0) {
    expected.evidence_ids = packets.flatMap((action) => (results.get(action).body.evidence ?? []).map(({ identity }) => identity.evidence_id));
  }
  if (reads.length > 0) {
    expected.authority = "source";
    if (scenarioContract.id === "packet_gap_to_focused_source") expected.outcome = "supported";
    expected.evidence_ids.push(...reads.map(({ path }) => `source:${path}`));
  }
  if (proof) {
    const disposition = results.get(proof).body?.disposition;
    if (disposition?.kind === "contract_proven") {
      const receipts = results.get(proof).body.receipts;
      expected.evidence_ids = disposition.receipts.map((index) => receipts[index]?.receipt_id);
    } else if (disposition?.kind === "contract_refuted") {
      const receipts = results.get(proof).body.receipts;
      expected.evidence_ids = disposition.refutation.connected_receipts.map((index) => receipts[index]?.receipt_id);
    } else {
      expected.evidence_ids = [];
    }
    if (disposition?.kind === "contract_refuted") expected.refutation_basis = disposition.refutation.kind;
  }

  for (const action of searches) {
    expected.gap_ids.push(...results.get(action).body.gaps.map((gap) => gap.identity.gap_id));
  }
  if (packets.length > 0) {
    expected.gap_ids.push(...results.get(packets.at(-1)).body.gaps.map((gap) => gap.identity.gap_id));
  }
  if (proof) {
    const disposition = results.get(proof).body?.disposition;
    if (Array.isArray(disposition?.gaps)) expected.reason_codes.push(...disposition.gaps.map(({ kind }) => kind));
    if (Array.isArray(disposition?.reasons)) expected.reason_codes.push(...disposition.reasons);
    if (results.get(proof).isError && nonemptyString(results.get(proof).body?.code)) {
      expected.reason_codes.push(results.get(proof).body.code);
    }
  }
  for (const action of packets) {
    const body = results.get(action).body;
    if (body.status === "unavailable") {
      expected.reason_codes.push(...body.gaps.map(({ kind }) => kind));
    }
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
  const proofAction = actions.find((action) => action.kind === "prove_call_path");
  const proofDisposition = proofAction ? results.get(proofAction)?.body?.disposition : null;
  const hasResultBoundGap = expected.gap_ids.length > 0
    || expected.reason_codes.length > 0
    || (Array.isArray(proofDisposition?.gaps) && proofDisposition.gaps.length > 0)
    || (Array.isArray(proofDisposition?.reasons) && proofDisposition.reasons.length > 0);
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
    for (const gap of body?.gaps ?? body?.disposition?.gaps ?? []) {
      if (nonemptyString(gap?.kind)) allowedReasonCodes.add(gap.kind);
    }
    for (const reason of body?.disposition?.reasons ?? []) {
      if (nonemptyString(reason)) allowedReasonCodes.add(reason);
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
  const proof = actions.some((action) => action.kind === "prove_call_path");
  const reads = actions.some((action) => action.kind === "source_read" && action.completed && !action.error);
  if (proof) {
    if (!equalJson(claim.evidence_ids, expected.evidence_ids)) {
      fail(`${scenarioContract.id} final claim evidence_ids does not match result-bound evidence`);
    }
  } else if (reads) {
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
    const sed = trimmed.match(/^sed\s+-n\s+(?:'1,(\d+)p'|"1,(\d+)p"|1,(\d+)p)\s+(\S+)$/u);
    if (sed) return { kind: "sed", endLine: Number(sed[1] ?? sed[2] ?? sed[3]), words: [sed[4]] };
    const cat = trimmed.match(/^cat\s+(.+)$/u);
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
    const wc = trimmed.match(/^wc\s+-l\s+(.+)$/u);
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
  const scenarioContract = SCENARIOS_BY_ID.get(scenarioId);
  if (!scenarioContract) fail(`unknown routing scenario ${JSON.stringify(scenarioId)}`);
  if (!plainObject(request)) fail(`${scenarioId} request must be an object`);
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
    const expectedSemanticError = scenarioContract.typed_contract === "malformed"
      && action.kind === "prove_call_path";
    if (["search", "context", "packet", "prove_call_path"].includes(action.kind)) {
      validateToolInputSchema(action);
      results.set(action, expectedSemanticError
        ? normalizedResult(action, normalizedHost)
        : validateResultIdentity(action, expectedIdentity, normalizedHost));
    } else {
      results.set(action, normalizedResult(action, normalizedHost));
    }
    const allowedOptionalSourceFailure = action.kind === "source_read"
      && scenarioContract.optional_followups.includes("source_read")
      && action.error;
    if (results.get(action).isError && !expectedSemanticError && !allowedOptionalSourceFailure) {
      fail(`${scenarioId} has an unexpected failed ${action.tool} action`);
    }
    if (!expectedSemanticError && ["search", "context", "packet", "prove_call_path"].includes(action.kind)) {
      validateToolResultSchema(action, results.get(action));
    }
  }

  validateSourceReads(scenarioContract, request, actions, results);
  validateProofCalls(scenarioContract, request, actions, results);
  validatePacketContinuation(scenarioContract, actions, results);
  validateSelectedContext(scenarioContract, request, actions, results);
  validateHiddenDiscovery(scenarioContract, actions, results);
  validateFinalClaims(scenarioContract, parsed.final, actions, results);

  return {
    schema_version: 1,
    status: "pass",
    host: String(host).toLowerCase(),
    scenario_id: scenarioId,
    identity_binding: "exact",
    actions: actions.map(actionName),
    proof_disposition: proofDisposition(actions, results),
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

function validateRoutingGuidance(text, label) {
  const requirements = [
    [/discovery leads?.*`search`/isu, "search discovery authority"],
    [/successful search.*stop.*(?:do not|never).*source/isu, "successful-search stop boundary"],
    [/successful search.*stop.*unless.*exact selection/isu, "preselected-target search exception"],
    [/symbol_id.*context.*(?:`id`|\.id)/isu, "stable context identity mapping"],
    [/selected target.*`context`/isu, "selected-target context authority"],
    [/supplied symbol name.*search\.query.*unchanged/isu, "exact search query preservation"],
    [/broad.*`packet`.*continuation.*once/isu, "bounded packet routing"],
    [/host-supplied.*`prove_call_path`/isu, "host-supplied proof routing"],
    [/semantic proof tool error.*invalid contract.*not\s+typed-proof evidence/isu, "semantic proof error boundary"],
    [/exact proof from English.*no complete typed\s+contract.*stop.*do not call a\s+repository tool/isu, "free-English proof refusal"],
    [/`unknown`.*not absence/isu, "unknown boundary"],
    [/runtime execution/iu, "runtime-execution boundary"],
    [/typed `Unavailable`.*terminal/isu, "typed-unavailable terminal boundary"],
    [/diagnostics\.availability.*optional diagnostics.*never overrides.*top-level/isu, "diagnostics availability boundary"],
    [/transport.*tool absence.*source/isu, "transport-unavailable source fallback"],
  ];
  for (const [pattern, requirement] of requirements) {
    if (!pattern.test(text)) fail(`${label} is missing ${requirement}`);
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
  const canonicalSkillText = await readFile(resolve(root, "skills/codestory-grounding/SKILL.md"), "utf8");
  const openAiMetadataText = await readFile(resolve(root, "skills/codestory-grounding/agents/openai.yaml"), "utf8");
  const searchReferenceText = await readFile(
    resolve(root, "skills/codestory-grounding/references/search.md"),
    "utf8",
  );
  const contextReferenceText = await readFile(
    resolve(root, "skills/codestory-grounding/references/context.md"),
    "utf8",
  );
  const packetReferenceText = await readFile(
    resolve(root, "skills/codestory-grounding/references/packet.md"),
    "utf8",
  );
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
  validateRoutingGuidance(canonicalSkillText, "canonical grounding skill");
  if (!/omit optional numeric bounds.*generated schema/isu.test(canonicalSkillText)
      || !/limit.*1.*50/isu.test(searchReferenceText)) {
    fail("canonical grounding guidance is missing the bounded optional-argument contract");
  }
  if (!/bare\s+symbol.*exact\s+path.*evidence\[\]\.symbol_id.*context\.id/isu.test(contextReferenceText)
      || !/do not combine.*name.*path.*free-text.*`query`/isu.test(contextReferenceText)) {
    fail("canonical context guidance is missing the returned-identity disambiguation contract");
  }
  if (!/continuation\.gap_ids.*map.*gap_id/isu.test(packetReferenceText)
      || !/fallback-only.*initial.*probe/isu.test(packetReferenceText)) {
    fail("canonical packet guidance is missing exact continuation and fallback-only argument rules");
  }
  if (!/search.*context.*packet.*prove_call_path/isu.test(openAiMetadataText)
      || !/host-supplied/iu.test(openAiMetadataText)
      || !/unknown.*not absence/isu.test(openAiMetadataText)
      || !/successful search.*unless.*exact selection/isu.test(openAiMetadataText)
      || !/typed `Unavailable`.*terminal/isu.test(openAiMetadataText)
      || !/transport.*tool absence.*source/isu.test(openAiMetadataText)) {
    fail("OpenAI skill metadata is missing the canonical routing boundary");
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
      if (!/^---\ndescription: CodeStory local grounding\./u.test(ruleText)
          || !ruleText.includes("alwaysApply: true")
          || !ruleText.includes("Call the CodeStory tool that matches the task")
          || !ruleText.includes("codestory-grounding")
          || !ruleText.includes("owns the detailed tool and evidence contract")) {
        fail("cursor rule does not delegate to the complete canonical grounding contract");
      }
      validateRoutingGuidance(ruleText, "cursor rule");
    } else if (!/^---\nname: codestory-grounding\n/iu.test(ruleText)
        || !ruleText.includes("## Direct Tool Loop")
        || !ruleText.includes("## Task Router")
        || !ruleText.includes("## Evidence Rules")
        || !ruleText.includes("`packet`")
        || !ruleText.includes("`context`")) {
      fail(`${host} rule input is not the complete canonical grounding contract`);
    } else {
      validateRoutingGuidance(ruleText, `${host} rule input`);
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
