import { createHash } from "node:crypto";

const BUILDER_ABLATION_CONTRACT = "codestory.evidence-compiler-builder-ablation/v1";

const BUILDER_ABLATION_ARMS = Object.freeze([
  "native_tools",
  "exact_identity_source",
  "exact_plus_relations",
  "packet_semantic_off",
  "packet_semantic_on",
]);

const BUILDER_ABLATION_TASK_IDS = Object.freeze([
  "python-requests-session-flow",
  "rust-ripgrep-search-pipeline",
  "javascript-express-routing-flow",
  "typescript-swr-hook-flow",
  "c-redis-command-loop",
  "go-gin-route-dispatch",
  "swift-alamofire-request-flow",
  "dart-http-client-flow",
]);

const EXACT_IDENTITY_SOURCE_OPERATIONS = new Set([
  "search",
  "files",
  "symbol",
  "symbols",
  "get_node",
  "definition",
  "snippet",
]);

const EXPLICIT_RELATION_OPERATIONS = new Set([
  ...EXACT_IDENTITY_SOURCE_OPERATIONS,
  "context",
  "references",
  "trail",
  "callers",
  "callees",
  "trace",
  "neighbors",
  "shortest_path",
  "query_subgraph",
]);

const PACKET_OPERATIONS = new Set(["packet", "agent_packet"]);

function isBuilderAblationArm(arm) {
  return BUILDER_ABLATION_ARMS.includes(arm);
}

function isBuilderCodeStoryArm(arm) {
  return isBuilderAblationArm(arm) && arm !== "native_tools";
}

function isBuilderPacketArm(arm) {
  return arm === "packet_semantic_off" || arm === "packet_semantic_on";
}

function builderPacketRetrievalPolicy(arm) {
  if (arm === "packet_semantic_off") {
    return "repository_graph_lexical_dense_candidate_stage_disabled_v1";
  }
  if (arm === "packet_semantic_on") {
    return "repository_graph_lexical_dense_candidate_stage_enabled_v1";
  }
  return null;
}

function planBuilderAblationRuns(tasks, repeats = 3) {
  if (repeats !== 3) {
    throw new Error("builder ablation requires exactly three repeats");
  }
  const planned = [];
  for (const [taskIndex, task] of tasks.entries()) {
    for (let repeat = 1; repeat <= repeats; repeat += 1) {
      const rotation = (taskIndex * repeats + repeat - 1) % BUILDER_ABLATION_ARMS.length;
      for (let position = 0; position < BUILDER_ABLATION_ARMS.length; position += 1) {
        const arm = BUILDER_ABLATION_ARMS[(rotation + position) % BUILDER_ABLATION_ARMS.length];
        planned.push({ repo: task.repo, task, arm, repeat });
      }
    }
  }
  return planned;
}

function normalizeCodeStoryOperation(value) {
  return String(value ?? "")
    .trim()
    .toLowerCase()
    .replace(/^mcp__.*__/, "")
    .replaceAll("-", "_");
}

function hasUnquotedShellComposition(command) {
  const text = String(command ?? "");
  let quote = null;
  let escaped = false;
  for (let index = 0; index < text.length; index += 1) {
    const character = text[index];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (character === "\\" && quote !== "'") {
      escaped = true;
      continue;
    }
    if (quote === "'") {
      if (character === "'") quote = null;
      continue;
    }
    if (character === "`") return true;
    if (character === "$" && text[index + 1] === "(") return true;
    if (quote === '"') {
      if (character === '"') quote = null;
      continue;
    }
    if (character === "'" || character === '"') {
      quote = character;
      continue;
    }
    if (
      character === ";" || character === "|" || character === "&" ||
      character === "<" || character === ">" || character === "\n" ||
      character === "\r"
    ) {
      return true;
    }
  }
  return escaped || quote != null;
}

function shellWords(command) {
  if (hasUnquotedShellComposition(command)) return null;
  const text = String(command ?? "");
  const words = [];
  let word = "";
  let wordStarted = false;
  let quote = null;
  let escaped = false;
  const finishWord = () => {
    if (!wordStarted) return;
    words.push(word);
    word = "";
    wordStarted = false;
  };
  for (let index = 0; index < text.length; index += 1) {
    const character = text[index];
    if (escaped) {
      word += character;
      wordStarted = true;
      escaped = false;
      continue;
    }
    if (character === "\\" && quote !== "'") {
      escaped = true;
      wordStarted = true;
      continue;
    }
    if (quote) {
      if (character === quote) {
        quote = null;
      } else {
        word += character;
      }
      wordStarted = true;
      continue;
    }
    if (character === "'" || character === '"') {
      quote = character;
      wordStarted = true;
      continue;
    }
    if (/\s/u.test(character)) {
      finishWord();
      continue;
    }
    word += character;
    wordStarted = true;
  }
  if (escaped || quote) return null;
  finishWord();
  return words;
}

function directBenchmarkCliInvocation(command) {
  const text = String(command ?? "");
  const executable = String.raw`(?:"\$(?:CODESTORY_CLI|\{CODESTORY_CLI\})"|\$(?:CODESTORY_CLI|\{CODESTORY_CLI\}))`;
  if (!new RegExp(`^\\s*${executable}(?=\\s)`, "u").test(text)) return null;
  const words = shellWords(text);
  if (!words || words.length < 2) return null;
  if (!["$CODESTORY_CLI", "${CODESTORY_CLI}"].includes(words[0])) return null;
  if (!/^[a-z][a-z0-9-]*$/iu.test(words[1])) return null;
  return {
    operation: normalizeCodeStoryOperation(words[1]),
    args: words.slice(2),
  };
}

function codeStoryInvocationsFromCommand(command) {
  const direct = directBenchmarkCliInvocation(command);
  if (direct) {
    return [{ operation: direct.operation, checksum_bound: true }];
  }
  const text = String(command ?? "").replace(/\\"/g, '"');
  const pinnedVariable = String.raw`\$(?:\{(?:env:)?CODESTORY_CLI\}|(?:env:)?CODESTORY_CLI)`;
  const directExecutable = String.raw`(?:codestory-cli(?:\.exe)?|(?:[^\s;&|"']+[\\/])+codestory-cli(?:\.exe)?|"[^"]*[\\/]codestory-cli(?:\.exe)?"|'[^']*[\\/]codestory-cli(?:\.exe)?')`;
  const executable = `(?:(?<pinned>["']?${pinnedVariable}["']?)|(?<direct>${directExecutable}))`;
  const matches = [...text.matchAll(new RegExp(`${executable}\\s+([a-z][a-z0-9-]*)\\b`, "gi"))]
    .map((match) => ({
      operation: normalizeCodeStoryOperation(match[3]),
      checksum_bound: Boolean(match.groups?.pinned),
    }));
  if (matches.length) return matches;
  if (new RegExp(`${pinnedVariable}|\\bCODESTORY_CLI\\b|codestory-cli(?:\\.exe)?`, "i").test(text)) {
    return [{ operation: null, checksum_bound: false }];
  }
  return [];
}

function codeStoryOperationFromCommand(command) {
  return codeStoryInvocationsFromCommand(command)[0]?.operation ?? null;
}

function codeStoryOperationFromMcpTool(toolName) {
  const normalized = normalizeCodeStoryOperation(toolName);
  return normalized || null;
}

const TRAIL_ARGUMENTS = Object.freeze({
  values: new Set([
    "--project", "--id", "--query", "--file", "--choose", "--mode", "--depth",
    "--direction", "--max-nodes", "--layout", "--format",
  ]),
  booleans: new Set([
    "--include-tests", "--show-utility-calls", "--hide-speculative", "--story", "--mermaid",
  ]),
});

const EXACT_OPERATION_ARGUMENTS = Object.freeze({
  search: {
    values: new Set([
      "--project", "--query", "--limit", "--repo-text", "--profile", "--run-id",
      "--format",
    ]),
    booleans: new Set(["--why", "--plan-details"]),
  },
  files: {
    values: new Set(["--project", "--path", "--language", "--role", "--limit", "--format"]),
    booleans: new Set(),
  },
  symbol: {
    values: new Set(["--project", "--id", "--query", "--file", "--choose", "--format"]),
    booleans: new Set(["--mermaid"]),
  },
  snippet: {
    values: new Set([
      "--project", "--id", "--query", "--file", "--choose", "--context", "--lines",
      "--format",
    ]),
    booleans: new Set(["--function-body"]),
  },
  context: {
    values: new Set([
      "--project", "--id", "--query", "--bookmark", "--max-results", "--format",
    ]),
    booleans: new Set(["--no-evidence"]),
  },
  trail: TRAIL_ARGUMENTS,
  callers: TRAIL_ARGUMENTS,
  callees: TRAIL_ARGUMENTS,
  trace: TRAIL_ARGUMENTS,
});

function exactOperationArgumentViolations(operation, command, expectedProject) {
  const parsed = directBenchmarkCliInvocation(command);
  const schema = EXACT_OPERATION_ARGUMENTS[operation];
  if (!parsed || parsed.operation !== operation || !schema) {
    return [`CLI ${operation} has no permitted direct argument shape`];
  }
  if (typeof expectedProject !== "string" || !expectedProject) {
    return ["builder harness did not bind the expected project"];
  }

  const seen = new Map();
  const violations = [];
  for (let index = 0; index < parsed.args.length; index += 1) {
    const token = parsed.args[index];
    if (!token.startsWith("--")) {
      violations.push(`CLI ${operation} contains an unbound positional argument`);
      continue;
    }
    const equals = token.indexOf("=");
    const flag = equals >= 0 ? token.slice(0, equals) : token;
    const inlineValue = equals >= 0 ? token.slice(equals + 1) : null;
    if (schema.booleans.has(flag)) {
      if (inlineValue != null || seen.has(flag)) {
        violations.push(`CLI ${operation} has an invalid or duplicate ${flag}`);
      }
      seen.set(flag, true);
      continue;
    }
    if (!schema.values.has(flag)) {
      violations.push(`CLI ${operation} argument ${flag} is not permitted in the pinned ablation`);
      continue;
    }
    const value = inlineValue ?? parsed.args[index + 1];
    if (inlineValue == null) index += 1;
    if (!value || value.startsWith("--") || seen.has(flag)) {
      violations.push(`CLI ${operation} has a missing or duplicate ${flag}`);
      continue;
    }
    seen.set(flag, value);
  }

  if (seen.get("--project") !== expectedProject) {
    violations.push(`CLI ${operation} must bind --project to the pinned repository`);
  }
  if (operation === "search") {
    if (seen.get("--repo-text") !== "off") {
      violations.push("CLI search must declare --repo-text off in exact/relations arms");
    }
    if (seen.get("--profile") !== "agent" || seen.get("--run-id") !== "shared-agent") {
      violations.push("CLI search must bind the prepared agent profile and run id");
    }
  }
  return violations;
}

function commandUsesBenchmarkCli(command) {
  return directBenchmarkCliInvocation(command) != null;
}

function optionValues(args, name) {
  const values = [];
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === name) {
      if (index + 1 >= args.length || args[index + 1].startsWith("--")) return null;
      values.push(args[index + 1]);
      index += 1;
    } else if (args[index].startsWith(`${name}=`)) {
      const value = args[index].slice(name.length + 1);
      if (!value) return null;
      values.push(value);
    }
  }
  return values;
}

function singleOption(args, name) {
  const values = optionValues(args, name);
  return values?.length === 1 ? values[0] : null;
}

function packetContinuationShape(operation, arm, options = {}) {
  const raw = operation?.raw ?? {};
  const expected = options.continuationContract;
  const parsed = directBenchmarkCliInvocation(raw.command);
  if (operation?.transport !== "cli" || operation?.successful !== true || !expected || !parsed) {
    return false;
  }
  if (parsed.operation !== "packet") return false;
  const args = parsed.args;
  const allowedFlags = new Set([
    "--project",
    "--profile",
    "--run-id",
    "--question",
    "--budget",
    "--format",
    "--parent-packet-id",
    "--option-id",
    "--core-generation-id",
    "--retrieval-generation",
    "--benchmark-retrieval-proof-out",
    "--benchmark-disable-dense-semantic",
  ]);
  for (let index = 0; index < args.length; index += 1) {
    const token = args[index];
    if (!token.startsWith("--")) return false;
    const flag = token.split("=", 1)[0];
    if (!allowedFlags.has(flag)) return false;
    if (flag !== "--benchmark-disable-dense-semantic" && !token.includes("=")) index += 1;
  }
  const optionIds = optionValues(args, "--option-id");
  const allowedOptionIds = new Set(expected.allowed_option_ids ?? []);
  if (
    !optionIds?.length ||
    new Set(optionIds).size !== optionIds.length ||
    optionIds.some((value) => !allowedOptionIds.has(value))
  ) {
    return false;
  }
  for (const [flag, value] of [
    ["--project", expected.project],
    ["--profile", "agent"],
    ["--run-id", "shared-agent"],
    ["--question", expected.question],
    ["--budget", "standard"],
    ["--format", "json"],
    ["--parent-packet-id", expected.parent_packet_id],
    ["--core-generation-id", expected.core_generation_id],
    ["--retrieval-generation", expected.retrieval_generation],
    ["--benchmark-retrieval-proof-out", expected.proof_path],
  ]) {
    if (singleOption(args, flag) !== value) return false;
  }
  const denseDisabled = args.filter((arg) => arg === "--benchmark-disable-dense-semantic").length;
  if (args.some((arg) => arg.startsWith("--benchmark-disable-dense-semantic="))) {
    return false;
  }
  // A typed continuation is an exact-selector follow-up, so both packet arms
  // hold its dense stage disabled. The only semantic treatment difference is
  // the hidden initial packet prelude.
  return denseDisabled === 1;
}

function builderOperationViolations(arm, operations, options = {}) {
  const violations = [];
  if (!isBuilderAblationArm(arm)) {
    return [`unknown builder ablation arm: ${arm}`];
  }
  const attempted = operations ?? [];
  const successful = attempted.filter((entry) => entry?.successful === true);
  if (arm === "native_tools") {
    if (attempted.length) {
      violations.push(`native arm attempted CodeStory operation(s): ${attempted.map((entry) => entry.operation ?? "unknown").join(", ")}`);
    }
    return violations;
  }

  if (!successful.length) {
    return ["CodeStory arm executed no successful CodeStory operation"];
  }
  for (const entry of attempted) {
    if (entry.source !== "harness_packet_prelude" && entry.transport !== "cli") {
      violations.push("builder ablation permits only the checksum-bound CodeStory CLI");
    }
    if (
      entry.transport === "cli" &&
      entry.source !== "harness_packet_prelude" &&
      !commandUsesBenchmarkCli(entry.raw?.command)
    ) {
      violations.push("agent CLI operation did not execute the checksum-bound $CODESTORY_CLI");
    }
    if (entry.transport === "cli" && entry.source !== "harness_packet_prelude") {
      if (hasUnquotedShellComposition(entry.raw?.command)) {
        violations.push("agent CLI operation must be one uncomposed command");
      }
      const remainingCodeStoryEnvironment = String(entry.raw?.command ?? "")
        .replace(/\$(?:\{(?:env:)?CODESTORY_CLI\}|(?:env:)?CODESTORY_CLI)/gi, "");
      if (/\bCODESTORY_[A-Z0-9_]+\b/i.test(remainingCodeStoryEnvironment)) {
        violations.push("agent CLI operation attempted to inspect or override CodeStory benchmark state");
      }
    }
  }
  if (arm === "exact_identity_source" || arm === "exact_plus_relations") {
    const allowed = arm === "exact_identity_source"
      ? EXACT_IDENTITY_SOURCE_OPERATIONS
      : EXPLICIT_RELATION_OPERATIONS;
    for (const entry of attempted) {
      if (!entry.operation || !allowed.has(entry.operation)) {
        violations.push(`operation ${entry.operation ?? "unknown"} is forbidden in ${arm}`);
        continue;
      }
      violations.push(...exactOperationArgumentViolations(
        entry.operation,
        entry.raw?.command,
        options.expectedProject,
      ));
    }
    return violations;
  }

  if (attempted.length > 2) {
    violations.push(`packet arm attempted ${attempted.length} CodeStory operations; maximum is two`);
  }
  for (const entry of attempted) {
    if (!entry.operation || !PACKET_OPERATIONS.has(entry.operation)) {
      violations.push(`operation ${entry.operation ?? "unknown"} is forbidden in ${arm}`);
    }
  }
  if (attempted.length >= 1 && attempted[0].source !== "harness_packet_prelude") {
    violations.push("packet arm did not begin with the harness packet prelude");
  }
  if (attempted.length === 2 && !packetContinuationShape(attempted[1], arm, options)) {
    violations.push("packet continuation is missing generation-bound typed selectors, execution proof, or the requested dense policy");
  }
  if (attempted.length === 2 && !options.continuationContract) {
    violations.push("packet continuation was not offered by the initial packet");
  }
  return violations;
}

function finiteNonnegative(value) {
  return Number.isFinite(value) && value >= 0;
}

function mean(values) {
  return values.length ? values.reduce((sum, value) => sum + value, 0) / values.length : null;
}

function median(values) {
  if (!values.length) return null;
  const ordered = [...values].sort((left, right) => left - right);
  const midpoint = Math.floor(ordered.length / 2);
  return ordered.length % 2 === 0
    ? (ordered[midpoint - 1] + ordered[midpoint]) / 2
    : ordered[midpoint];
}

function ratio(numerator, denominator) {
  if (!finiteNonnegative(numerator) || !finiteNonnegative(denominator)) return null;
  if (denominator === 0) return numerator === 0 ? 1 : Number.POSITIVE_INFINITY;
  return numerator / denominator;
}

function receiptRatio(value) {
  return value === Number.POSITIVE_INFINITY ? "infinite" : value;
}

function pairedRows(rows, candidateArm, controlArm) {
  const control = new Map(
    rows
      .filter((row) => row.arm === controlArm)
      .map((row) => [`${row.task_id}\t${row.repeat}`, row]),
  );
  return rows
    .filter((row) => row.arm === candidateArm)
    .map((candidate) => ({
      candidate,
      control: control.get(`${candidate.task_id}\t${candidate.repeat}`) ?? null,
    }));
}

function timingPairIsEligible({ candidate, control }) {
  const candidateCohort = candidate?.installed_agent_timing?.timing_cohort_id;
  const controlCohort = control?.installed_agent_timing?.timing_cohort_id;
  return Boolean(
    control &&
    candidate?.installed_agent_timing_eligible === true &&
    control?.installed_agent_timing_eligible === true &&
    /^[0-9a-f]{64}$/u.test(String(candidateCohort ?? "")) &&
    candidateCohort === controlCohort,
  );
}

function taskPassCounts(rows, arm) {
  const byTask = new Map();
  for (const row of rows.filter((entry) => entry.arm === arm)) {
    const current = byTask.get(row.task_id) ?? { passes: 0, rows: 0 };
    current.rows += 1;
    if (row.quality?.pass === true) current.passes += 1;
    byTask.set(row.task_id, current);
  }
  return byTask;
}

function adjudicationKey(row) {
  return `${row.task_id}\t${row.arm}\t${row.repeat}`;
}

function normalizeAdjudication(adjudication) {
  if (adjudication?.contract !== "codestory.evidence-compiler-builder-adjudication/v1") {
    return { complete: false, byKey: new Map(), reason: "missing or invalid independent adjudication contract" };
  }
  const digest = /^[0-9a-f]{64}$/u;
  if (
    adjudication.blinded !== true ||
    typeof adjudication.independent_reviewer !== "string" ||
    !adjudication.independent_reviewer.trim() ||
    !digest.test(String(adjudication.source_cases_sha256 ?? "")) ||
    !digest.test(String(adjudication.source_judgments_sha256 ?? "")) ||
    !Array.isArray(adjudication.rows)
  ) {
    return {
      complete: false,
      byKey: new Map(),
      reason: "adjudication must carry blinded case, judgment, and independent-reviewer receipts",
    };
  }
  const byKey = new Map(adjudication.rows.map((row) => [adjudicationKey(row), row]));
  if (byKey.size !== adjudication.rows.length) {
    return { complete: false, byKey, reason: "adjudication contains duplicate task/arm/repeat rows" };
  }
  const adjudicatedArms = BUILDER_ABLATION_ARMS.filter((arm) => arm !== "native_tools");
  const expectedKeys = new Set(BUILDER_ABLATION_TASK_IDS.flatMap((taskId) =>
    adjudicatedArms.flatMap((arm) => [1, 2, 3].map(
      (repeat) => `${taskId}\t${arm}\t${repeat}`,
    )),
  ));
  if (
    byKey.size !== expectedKeys.size ||
    [...byKey.keys()].some((key) => !expectedKeys.has(key))
  ) {
    return {
      complete: false,
      byKey,
      reason: `adjudication must contain exactly ${expectedKeys.size} CodeStory-arm rows`,
    };
  }
  return {
    complete: true,
    byKey,
    reason: null,
  };
}

function criticalCount(adjudicatedRow) {
  const factual = Number(adjudicatedRow?.critical_factual_errors);
  const relations = Number(adjudicatedRow?.unsupported_relation_claims);
  const factualIds = adjudicatedRow?.critical_factual_finding_ids;
  const relationIds = adjudicatedRow?.unsupported_relation_finding_ids;
  if (
    !Number.isInteger(factual) || factual < 0 ||
    !Number.isInteger(relations) || relations < 0 ||
    !Array.isArray(factualIds) || factualIds.length !== factual ||
    !Array.isArray(relationIds) || relationIds.length !== relations ||
    new Set(factualIds).size !== factualIds.length ||
    new Set(relationIds).size !== relationIds.length
  ) {
    return null;
  }
  return factual + relations;
}

function criticalFindingIds(adjudicatedRow) {
  if (criticalCount(adjudicatedRow) == null) return null;
  return new Set([
    ...adjudicatedRow.critical_factual_finding_ids.map((id) => `factual:${id}`),
    ...adjudicatedRow.unsupported_relation_finding_ids.map((id) => `relation:${id}`),
  ]);
}

function sha256Text(value) {
  return createHash("sha256").update(String(value ?? ""), "utf8").digest("hex");
}

function requestReceiptMatchesInitial(receipt, question) {
  const request = receipt?.request;
  return Boolean(
    request &&
    request.question_sha256 === sha256Text(question) &&
    request.parent_packet_id == null &&
    Array.isArray(request.option_ids) &&
    request.option_ids.length === 0 &&
    request.core_generation_id == null &&
    request.retrieval_generation == null
  );
}

function requestReceiptMatchesContinuation(receipt, offer) {
  const request = receipt?.request;
  const selected = request?.option_ids;
  const allowed = new Set(offer?.allowed_option_ids ?? []);
  return Boolean(
    offer &&
    request &&
    request.question_sha256 === offer.question_sha256 &&
    request.parent_packet_id === offer.parent_packet_id &&
    request.core_generation_id === offer.core_generation_id &&
    request.retrieval_generation === offer.retrieval_generation &&
    Array.isArray(selected) &&
    selected.length > 0 &&
    new Set(selected).size === selected.length &&
    selected.every((value) => allowed.has(value))
  );
}

function candidateOnlyCriticalForPairs(pairs, normalizedAdjudication) {
  const missing = [];
  const candidateOnly = [];
  for (const { candidate, control } of pairs) {
    if (!control) {
      missing.push(`${candidate.task_id}/${candidate.repeat}`);
      continue;
    }
    const candidateJudgment = normalizedAdjudication.byKey.get(adjudicationKey(candidate));
    const controlJudgment = normalizedAdjudication.byKey.get(adjudicationKey(control));
    const candidateCount = criticalCount(candidateJudgment);
    const controlCount = criticalCount(controlJudgment);
    const candidateIds = criticalFindingIds(candidateJudgment);
    const controlIds = criticalFindingIds(controlJudgment);
    if (candidateCount == null || controlCount == null) {
      missing.push(`${candidate.task_id}/${candidate.repeat}`);
      continue;
    }
    const onlyIds = [...candidateIds].filter((id) => !controlIds.has(id));
    if (onlyIds.length) {
      candidateOnly.push({
        task_id: candidate.task_id,
        repeat: candidate.repeat,
        candidate_critical: candidateCount,
        control_critical: controlCount,
        candidate_only_finding_ids: onlyIds,
      });
    }
  }
  return {
    complete: normalizedAdjudication.complete && missing.length === 0,
    missing,
    candidate_only: candidateOnly,
  };
}

function aggregateRatio(pairs, selector) {
  const eligible = pairs.filter(({ candidate, control }) => control && selector(candidate) != null && selector(control) != null);
  if (!eligible.length) return null;
  return ratio(
    eligible.reduce((sum, { candidate }) => sum + selector(candidate), 0),
    eligible.reduce((sum, { control }) => sum + selector(control), 0),
  );
}

function medianPairedRatio(pairs, selector) {
  const ratios = pairs.flatMap(({ candidate, control }) => {
    if (!control) return [];
    const candidateValue = selector(candidate);
    const controlValue = selector(control);
    const value = ratio(candidateValue, controlValue);
    return value == null ? [] : [value];
  });
  return median(ratios);
}

function denseStageProofInvocations(receipt, arm) {
  const proof = receipt?.retrieval_proof;
  const expectedDense = arm === "packet_semantic_on";
  const invocations = proof?.dense_semantic_stage_invocations;
  const stageInvocations = proof?.descriptor_stage_invocations?.stage1b_semantic ?? 0;
  const identities = [
    receipt?.core_generation_id,
    receipt?.core_run_id,
    receipt?.retrieval_generation,
    receipt?.semantic_generation,
  ];
  if (
    receipt?.contract !== "codestory.packet-builder-ablation-receipt/v1" ||
    receipt?.requested_dense_semantic !== expectedDense ||
    proof?.contract !== "codestory.packet-dense-candidate-ablation-proof/v1" ||
    proof?.requested_policy !== builderPacketRetrievalPolicy(arm) ||
    !Number.isInteger(proof?.descriptor_query_count) || proof.descriptor_query_count < 1 ||
    !Number.isInteger(invocations) || invocations < 0 ||
    !Number.isInteger(stageInvocations) || stageInvocations !== invocations ||
    (!expectedDense && invocations !== 0) ||
    identities.some((value) => typeof value !== "string" || !value.trim())
  ) {
    return null;
  }
  return invocations;
}

function evidenceCompilerBuilderAcceptance(rows, adjudication, options = {}) {
  const reasons = [];
  const expectedTaskIds = options.taskIds ?? BUILDER_ABLATION_TASK_IDS;
  const expectedRepeats = options.repeats ?? 3;
  const expectedRows = expectedTaskIds.length * BUILDER_ABLATION_ARMS.length * expectedRepeats;
  const expectedKeys = new Set(expectedTaskIds.flatMap((taskId) =>
    BUILDER_ABLATION_ARMS.flatMap((arm) =>
      Array.from({ length: expectedRepeats }, (_, index) => `${taskId}\t${arm}\t${index + 1}`)
    )
  ));
  const actualKeys = new Set(rows.map((row) => `${row.task_id}\t${row.arm}\t${row.repeat}`));
  if (
    rows.length !== expectedRows ||
    actualKeys.size !== expectedRows ||
    [...actualKeys].some((key) => !expectedKeys.has(key))
  ) {
    reasons.push(`expected ${expectedRows} unique frozen rows; received ${rows.length} rows and ${actualKeys.size} unique keys`);
  }
  for (const row of rows) {
    if (row.status !== "pass") reasons.push(`row ${row.task_id}/${row.arm}/${row.repeat} status=${row.status}`);
    const violations = row.builder_ablation?.operation_violations;
    if (!Array.isArray(violations) || violations.length) {
      reasons.push(`row ${row.task_id}/${row.arm}/${row.repeat} has missing or forbidden operation telemetry`);
    }
    if (row.comparator_reuse_provenance || row.resume_provenance || row.reused_from) {
      reasons.push(`row ${row.task_id}/${row.arm}/${row.repeat} is not fresh`);
    }
    if (
      row.repo_provenance?.git_dirty !== false ||
      !/^[0-9a-f]{40}$/u.test(String(row.repo_provenance?.git_head ?? ""))
    ) {
      reasons.push(`row ${row.task_id}/${row.arm}/${row.repeat} lacks a clean pinned repository receipt`);
    }
    if (isBuilderCodeStoryArm(row.arm) && (
      row.builder_ablation?.first_codestory_required !== true ||
      row.builder_ablation?.first_codestory_pass !== true ||
      row.transcript_analysis?.codestory_was_first_repository_context_action !== true ||
      !Number.isInteger(
        row.transcript_analysis
          ?.exploratory_repository_context_actions_after_first_codestory,
      ) ||
      row.transcript_analysis
        .exploratory_repository_context_actions_after_first_codestory < 0
    )) {
      reasons.push(
        `row ${row.task_id}/${row.arm}/${row.repeat} lacks valid observed repository-context accounting`,
      );
    }
  }
  const codeStoryRows = rows.filter((row) => isBuilderCodeStoryArm(row.arm));
  const cliDigests = new Set(codeStoryRows.map((row) => row.codestory_prelude_cli_sha256).filter(Boolean));
  if (cliDigests.size !== 1 || codeStoryRows.some((row) => !row.codestory_prelude_cli_sha256)) {
    reasons.push("all four CodeStory arms must execute one identical benchmark CLI digest");
  }
  for (const taskId of expectedTaskIds) {
    const taskPacketRows = rows.filter((row) => row.task_id === taskId && isBuilderPacketArm(row.arm));
    const publications = new Set(taskPacketRows.map((row) => {
      const proof = row.codestory_harness_prelude?.packet_retrieval_proof;
      return [
        proof?.core_generation_id,
        proof?.core_run_id,
        proof?.retrieval_generation,
        proof?.semantic_generation,
      ].join("\t");
    }));
    if (
      taskPacketRows.length !== expectedRepeats * 2 ||
      publications.size !== 1 ||
      [...publications].some((identity) => identity.split("\t").some((part) => !part))
    ) {
      reasons.push(`${taskId} packet arms do not share one complete core/retrieval publication identity`);
    }
    const packetPublication = taskPacketRows[0]?.codestory_harness_prelude?.packet_retrieval_proof;
    const taskCodeStoryRows = rows.filter(
      (row) => row.task_id === taskId && isBuilderCodeStoryArm(row.arm),
    );
    const storagePaths = new Set(taskCodeStoryRows.map(
      (row) => row.codestory_cache_provenance?.storage_path,
    ));
    if (
      taskCodeStoryRows.length !== expectedRepeats * 4 ||
      storagePaths.size !== 1 ||
      [...storagePaths].some((value) => typeof value !== "string" || !value.trim()) ||
      taskCodeStoryRows.some((row) => {
        const retrieval = row.codestory_cache_provenance?.retrieval_status;
        return retrieval?.sidecar_generation !== packetPublication?.retrieval_generation ||
          retrieval?.semantic_generation !== packetPublication?.semantic_generation;
      })
    ) {
      reasons.push(`${taskId} CodeStory arms do not bind one prepared retrieval publication`);
    }
  }
  let denseSemanticOnInvocations = 0;
  for (const row of rows.filter((entry) => isBuilderPacketArm(entry.arm))) {
    const receipt = row.codestory_harness_prelude?.packet_retrieval_proof;
    const invocations = denseStageProofInvocations(receipt, row.arm);
    if (invocations == null) {
      reasons.push(`row ${row.task_id}/${row.arm}/${row.repeat} has invalid dense candidate-stage execution proof`);
      continue;
    }
    if (!requestReceiptMatchesInitial(receipt, row.task_manifest_snapshot?.prompt)) {
      reasons.push(`row ${row.task_id}/${row.arm}/${row.repeat} initial proof is not bound to its packet request`);
    }
    if (row.arm === "packet_semantic_on") denseSemanticOnInvocations += invocations;
    const continuationOperation = row.transcript_analysis?.codestory_operations?.find(
      (operation) => operation?.source === "agent_cli" && operation?.operation === "packet",
    ) ?? null;
    const continuationAttempted = continuationOperation != null;
    const continuationOffer = row.builder_ablation?.continuation_offer ?? null;
    const continuationReceipt = row.builder_ablation?.continuation_retrieval_proof ?? null;
    if (continuationAttempted) {
      const continuationInvocations = denseStageProofInvocations(continuationReceipt, row.arm);
      if (
        continuationOperation.successful !== true ||
        continuationInvocations == null ||
        !requestReceiptMatchesContinuation(continuationReceipt, continuationOffer) ||
        ["core_generation_id", "core_run_id", "retrieval_generation", "semantic_generation"]
          .some((field) => continuationReceipt?.[field] !== receipt?.[field])
      ) {
        reasons.push(`row ${row.task_id}/${row.arm}/${row.repeat} has invalid continuation execution proof`);
      } else if (row.arm === "packet_semantic_on") {
        denseSemanticOnInvocations += continuationInvocations;
      }
    } else if (continuationReceipt != null) {
      reasons.push(`row ${row.task_id}/${row.arm}/${row.repeat} has an unbound continuation execution proof`);
    }
    if (continuationOffer == null && continuationAttempted) {
      reasons.push(`row ${row.task_id}/${row.arm}/${row.repeat} executed a continuation the initial packet did not offer`);
    }
  }
  if (denseSemanticOnInvocations < 1) {
    reasons.push("semantic-on packet rows contain no executed dense candidate stage");
  }

  const packetArm = "packet_semantic_off";
  const controlArm = "exact_plus_relations";
  const packetCounts = taskPassCounts(rows, packetArm);
  const controlCounts = taskPassCounts(rows, controlArm);
  const taskQualityDeltas = expectedTaskIds.map((taskId) => ({
    task_id: taskId,
    packet_passes: packetCounts.get(taskId)?.passes ?? 0,
    control_passes: controlCounts.get(taskId)?.passes ?? 0,
    packet_q: (packetCounts.get(taskId)?.passes ?? 0) / expectedRepeats,
    control_q: (controlCounts.get(taskId)?.passes ?? 0) / expectedRepeats,
  })).map((entry) => ({ ...entry, difference: entry.packet_q - entry.control_q }));
  const meanTaskPassDifference = mean(taskQualityDeltas.map((entry) => entry.difference));
  if (meanTaskPassDifference == null || meanTaskPassDifference < -0.05) {
    reasons.push(`packet mean task-pass difference ${meanTaskPassDifference ?? "missing"} is below -0.05`);
  }
  for (const entry of taskQualityDeltas) {
    if (entry.packet_passes === 0 && entry.control_passes >= 2) {
      reasons.push(`${entry.task_id} is packet 0/${expectedRepeats} while primitives are ${entry.control_passes}/${expectedRepeats}`);
    }
  }

  const pairs = pairedRows(rows, packetArm, controlArm);
  if (pairs.length !== expectedTaskIds.length * expectedRepeats || pairs.some((pair) => !timingPairIsEligible(pair))) {
    reasons.push("packet and primitive timing rows do not share complete eligible cohort identities");
  }
  const exploratoryRepositoryContextActionRatio = medianPairedRatio(
    pairs,
    (row) => row.transcript_analysis
      ?.exploratory_repository_context_actions_after_first_codestory,
  );
  if (
    exploratoryRepositoryContextActionRatio == null ||
    exploratoryRepositoryContextActionRatio > 0.8
  ) {
    reasons.push(
      `packet exploratory repository-context-action ratio ${exploratoryRepositoryContextActionRatio ?? "missing"} exceeds 0.80`,
    );
  }
  const wholeTaskWallRatio = aggregateRatio(
    pairs,
    (row) => row.installed_agent_timing?.whole_task_wall_ms,
  );
  if (wholeTaskWallRatio == null || wholeTaskWallRatio > 1.05) {
    reasons.push(`packet whole-task wall ratio ${wholeTaskWallRatio ?? "missing"} exceeds 1.05`);
  }
  const inputContextRatio = aggregateRatio(pairs, (row) => row.usage?.input_tokens);
  if (inputContextRatio == null || inputContextRatio > 1.05) {
    reasons.push(`packet input-context ratio ${inputContextRatio ?? "missing"} exceeds 1.05`);
  }

  const normalizedAdjudication = normalizeAdjudication(adjudication);
  const packetCriticalComparison = candidateOnlyCriticalForPairs(pairs, normalizedAdjudication);
  if (!packetCriticalComparison.complete) {
    reasons.push(
      normalizedAdjudication.reason ??
      `independent critical-claim adjudication is incomplete for ${packetCriticalComparison.missing.join(", ")}`,
    );
  }
  if (packetCriticalComparison.candidate_only.length) {
    reasons.push(`packet-only critical factual or unsupported-relation claims=${packetCriticalComparison.candidate_only.length}`);
  }

  return {
    contract: BUILDER_ABLATION_CONTRACT,
    pass: reasons.length === 0,
    reasons: [...new Set(reasons)],
    packet_arm: packetArm,
    control_arm: controlArm,
    expected_rows: expectedRows,
    observed_rows: rows.length,
    task_quality_deltas: taskQualityDeltas,
    mean_task_pass_difference: meanTaskPassDifference,
    exploratory_repository_context_metric:
      "exploratory_repository_context_actions_after_first_codestory",
    exploratory_repository_context_aggregation: "median_paired_ratio",
    exploratory_repository_context_action_ratio:
      receiptRatio(exploratoryRepositoryContextActionRatio),
    whole_task_wall_ratio: receiptRatio(wholeTaskWallRatio),
    context_metric: "input_tokens",
    input_context_ratio: receiptRatio(inputContextRatio),
    adjudication_complete: packetCriticalComparison.complete,
    packet_only_critical_claims: packetCriticalComparison.candidate_only.map((finding) => ({
      task_id: finding.task_id,
      repeat: finding.repeat,
      packet_critical: finding.candidate_critical,
      control_critical: finding.control_critical,
      packet_only_finding_ids: finding.candidate_only_finding_ids,
    })),
  };
}

function marginalValue(rows, candidateArm, controlArm, adjudication) {
  const pairs = pairedRows(rows, candidateArm, controlArm);
  const timingCohortsComplete = pairs.length > 0 && pairs.every(timingPairIsEligible);
  const taskIds = [...new Set(rows.map((row) => row.task_id))];
  const candidateCounts = taskPassCounts(rows, candidateArm);
  const controlCounts = taskPassCounts(rows, controlArm);
  const passDifference = mean(taskIds.map((taskId) =>
    ((candidateCounts.get(taskId)?.passes ?? 0) - (controlCounts.get(taskId)?.passes ?? 0)) / 3
  ));
  const explorationRatio = aggregateRatio(
    pairs,
    (row) => row.transcript_analysis
      ?.exploratory_repository_context_actions_after_first_codestory,
  );
  const wallRatio = aggregateRatio(pairs, (row) => row.installed_agent_timing?.whole_task_wall_ms);
  const contextRatio = aggregateRatio(pairs, (row) => row.usage?.input_tokens);
  const ratios = [explorationRatio, wallRatio, contextRatio];
  const completeEfficiency = ratios.every((value) => value != null);
  const boundedCost = timingCohortsComplete && completeEfficiency && ratios.every((value) => value <= 1.05);
  const materialEfficiencyGain = completeEfficiency && ratios.some((value) => value <= 0.95);
  const improvesPass = passDifference != null && passDifference > 0 && boundedCost;
  const holdsPassAndImprovesEfficiency = passDifference === 0 && boundedCost && materialEfficiencyGain;
  const criticalComparison = candidateOnlyCriticalForPairs(
    pairs,
    normalizeAdjudication(adjudication),
  );
  return {
    candidate_arm: candidateArm,
    control_arm: controlArm,
    positive: criticalComparison.complete &&
      criticalComparison.candidate_only.length === 0 &&
      (improvesPass || holdsPassAndImprovesEfficiency),
    adjudication_complete: criticalComparison.complete,
    candidate_only_critical_claims: criticalComparison.candidate_only,
    timing_cohorts_complete: timingCohortsComplete,
    mean_task_pass_difference: passDifference,
    exploratory_repository_context_action_ratio: receiptRatio(explorationRatio),
    whole_task_wall_ratio: receiptRatio(wallRatio),
    input_context_ratio: receiptRatio(contextRatio),
  };
}

export {
  BUILDER_ABLATION_ARMS,
  BUILDER_ABLATION_CONTRACT,
  BUILDER_ABLATION_TASK_IDS,
  EXACT_IDENTITY_SOURCE_OPERATIONS,
  EXPLICIT_RELATION_OPERATIONS,
  builderOperationViolations,
  builderPacketRetrievalPolicy,
  codeStoryInvocationsFromCommand,
  codeStoryOperationFromCommand,
  codeStoryOperationFromMcpTool,
  evidenceCompilerBuilderAcceptance,
  isBuilderAblationArm,
  isBuilderCodeStoryArm,
  isBuilderPacketArm,
  marginalValue,
  planBuilderAblationRuns,
};
