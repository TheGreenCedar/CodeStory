#!/usr/bin/env node
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { LineCounter, parseDocument } from "yaml";
import { loadReleaseClaimGraph } from "../../scripts/codestory-release-claims.mjs";
import {
  LOST_RUNNER_ANNOTATION,
  MAXIMUM_RUN_ATTEMPTS,
  RERUN_DISPATCH_SCHEMA,
  RERUN_PLAN_SCHEMA,
  RESERVATION_RECEIPT_SCHEMA,
} from "./lost-runner-recovery.mjs";
import {
  LINK_PHASE as WINDOWS_LINK_PHASE,
  SCHEMA as WINDOWS_LINK_TIMING_SCHEMA,
  UNAVAILABLE_REASONS as WINDOWS_LINK_TIMING_REASONS,
} from "./windows-link-timing.mjs";

const workflowRoot = path.join(".github", "workflows");
const retrievalFile = "retrieval-engine-smoke.yml";
const crateDurabilityFile = "crate-durability.yml";
const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const retrievalGeneralizationSuiteFile = path.join(
  "scripts",
  "tests",
  "lint-retrieval-generalization.test.mjs",
);
const legacyRetrievalGeneralizationWrapper = path.join(
  "crates",
  "codestory-runtime",
  "tests",
  "retrieval_generalization_guard.rs",
);
const runtimeIntegrationTestRoot = path.join(
  "crates",
  "codestory-runtime",
  "tests",
);
const trustedActionOwners = new Set(["actions", "github"]);
const fullSha = /^[0-9a-f]{40}$/iu;
const sccacheAction = "mozilla-actions/sccache-action@9e7fa8a12102821edf02ca5dbea1acd0f89a2696";
const sccacheVersion = "v0.16.0";
const nextestVersion = "0.9.98";
const sccacheCacheSize = "1G";
const windowsSccacheCacheSize = "2G";

export { crateDurabilityFile, retrievalFile };

function tomlSection(source, section) {
  const header = `[${section}]`;
  const start = source.indexOf(`${header}\n`);
  if (start < 0) return null;
  const bodyStart = start + header.length + 1;
  const next = source.slice(bodyStart).search(/^\[[^\]]+\]\s*$/mu);
  return next < 0
    ? source.slice(bodyStart)
    : source.slice(bodyStart, bodyStart + next);
}

export function benchmarkDependencyIsolationViolations(source) {
  const violations = [];
  const dependencies = tomlSection(source, "dependencies");
  const devDependencies = tomlSection(source, "dev-dependencies");
  if (dependencies === null || devDependencies === null) {
    return ["codestory-bench must separate product-driver and benchmark dependencies"];
  }
  const benchmarkOnly = [
    "codestory-cli",
    "codestory-contracts",
    "codestory-indexer",
    "codestory-runtime",
    "codestory-store",
    "criterion",
    "uuid",
  ];
  const dependencyNames = new Set(
    [...dependencies.matchAll(/^([A-Za-z0-9_-]+)\s*=/gmu)]
      .map((match) => match[1]),
  );
  const devDependencyNames = new Set(
    [...devDependencies.matchAll(/^([A-Za-z0-9_-]+)\s*=/gmu)]
      .map((match) => match[1]),
  );
  add(
    violations,
    benchmarkOnly.every(
      (name) => !dependencyNames.has(name) && devDependencyNames.has(name),
    ),
    "codestory-bench benchmark-only dependencies must not enter packaged qualification binaries",
  );
  add(
    violations,
    /^codestory-runtime\s*=\s*\{\s*workspace\s*=\s*true,\s*features\s*=\s*\["benchmark-support"\]\s*\}\s*$/mu
      .test(devDependencies)
      && !/\b(?:benchmark-support|test-support)\b/u.test(dependencies),
    "codestory-bench product dependencies must not enable benchmark-support or test-support",
  );
  return violations;
}

export function rustRetrievalWrapperSourcePresent(source) {
  return (
    /lint-retrieval-generalization|retrieval[_-]generalization[_-](?:guard|lint)/u
      .test(source)
    || /\b(?:std|tokio|async_std)::process\b|\bprocess::Command\b|\bCommand\s*::\s*(?:new|from)\s*\(|\b(?:assert_cmd|duct|xshell)\b/u
      .test(source)
  );
}

function serializedRustRetrievalWrapperPresent() {
  const root = path.join(repositoryRoot, runtimeIntegrationTestRoot);
  if (!fs.existsSync(root)) return false;
  const pending = [root];
  while (pending.length > 0) {
    const current = pending.pop();
    for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
      const entryPath = path.join(current, entry.name);
      if (entry.isDirectory()) {
        pending.push(entryPath);
      } else if (entry.name.endsWith(".rs")) {
        const source = fs.readFileSync(entryPath, "utf8");
        if (rustRetrievalWrapperSourcePresent(source)) {
          return true;
        }
      }
    }
  }
  return false;
}

export function retrievalGeneralizationSuitePolicyViolations(
  source,
  {
    legacyWrapperPresent = false,
  } = {},
) {
  const violations = [];
  const invocationCount = source.match(/\brunRetrievalGeneralizationLint\s*\(/gu)?.length ?? 0;
  const lintReferenceCount =
    source.match(/\brunRetrievalGeneralizationLint\b/gu)?.length ?? 0;
  const checkoutDigestCount = source.match(/\btreeDigest\(repositoryRoot\)/gu)?.length ?? 0;
  const repositoryRootReferenceCount = source.match(/\brepositoryRoot\b/gu)?.length ?? 0;
  const fixtureRootReferenceCount = source.match(/\bfixtureRoot\b/gu)?.length ?? 0;
  const productionRepositoryRootReferenceCount =
    source.match(/\bproductionRepositoryRoot\b/gu)?.length ?? 0;
  const temporaryRootCount = source.match(/\bos\.tmpdir\(\)/gu)?.length ?? 0;
  const temporaryTreeCount = source.match(/\bfs\.mkdtempSync\s*\(/gu)?.length ?? 0;
  const dynamicImportCount = source.match(/\bimport\s*\(/gu)?.length ?? 0;
  const fsReferenceCount = source.match(/\bfs\b/gu)?.length ?? 0;
  const filesystemMemberReferences = [...source.matchAll(
    /\bfs\.([A-Za-z_$][\w$]*)\b/gu,
  )].map((match) => match[1]);
  const expectedFilesystemMemberCounts = {
    existsSync: 1,
    mkdirSync: 7,
    mkdtempSync: 1,
    // Two of these read the shipped pending inventory and the registry document it points at,
    // so the claim-profile ratchet is checked against the tree that ships, not a synthetic one.
    readFileSync: 6,
    readdirSync: 4,
    readlinkSync: 1,
    rmSync: 1,
    writeFileSync: 1,
  };
  const filesystemMemberCounts = new Map();
  for (const name of filesystemMemberReferences) {
    filesystemMemberCounts.set(
      name,
      (filesystemMemberCounts.get(name) ?? 0) + 1,
    );
  }
  const fixtureFilesystemShapeIsExact =
    fsReferenceCount === 24
    && filesystemMemberReferences.length === 22
    && Object.entries(expectedFilesystemMemberCounts).every(
      ([name, count]) => (filesystemMemberCounts.get(name) ?? 0) === count,
    )
    && [
      "const destination = path.join(root, relativePath);",
      "fs.mkdirSync(path.dirname(destination), { recursive: true });",
      "fs.writeFileSync(destination, contents);",
      "fs.mkdirSync(rustRoot, { recursive: true });",
      "fs.mkdirSync(retrievalRoot, { recursive: true });",
      "fs.mkdirSync(agentRoot, { recursive: true });",
      "fs.mkdirSync(extraRustRoot);",
      "fs.mkdirSync(nonRustRoot);",
      "fs.mkdirSync(taskRoot);",
      "fs.rmSync(fixtureRoot, { recursive: true, force: true });",
    ].every((fragment) => source.includes(fragment));
  const writeReferenceCount = source.match(/\bwrite\b/gu)?.length ?? 0;
  const writeFirstArguments = [...source.matchAll(
    /\bwrite\s*\(\s*([A-Za-z_$][\w$]*)/gu,
  )].map((match) => match[1]);
  const registeredWriteRoots = new Set([
    "root",
    "rustRoot",
    "retrievalRoot",
    "agentRoot",
    "extraRustRoot",
    "nonRustRoot",
    "taskRoot",
  ]);
  const syntheticWritesStayInRegisteredRoots =
    writeReferenceCount === 15
    && writeFirstArguments.length === writeReferenceCount
    && writeFirstArguments.every((root) => registeredWriteRoots.has(root));
  const fixturePathReferenceShapeIsExact =
    repositoryRootReferenceCount === 14
    && fixtureRootReferenceCount === 10
    && productionRepositoryRootReferenceCount === 5;
  const protectedRetrievalWorkflow = `.github/workflows/${retrievalFile}`;
  const retainedDynamicImportFixtures = [
    "await import(harness);",
    String.raw`await import(\"${protectedRetrievalWorkflow}\");`,
    `await import("${protectedRetrievalWorkflow}");`,
  ];
  const importSpecifiers = [...source.matchAll(
    /^\s*import\s+(?:[\s\S]*?\s+from\s+)?["']([^"']+)["'];\s*$/gmu,
  )].map((match) => match[1]);
  const allowedImports = [
    "node:assert/strict",
    "node:crypto",
    "node:fs",
    "node:os",
    "node:path",
    "node:test",
    "node:url",
    "../lib/retrieval-generalization-lint.mjs",
  ];
  const forbiddenConcurrencySurface = [
    /(?:node:)?child_process/u,
    /(?:node:)?worker_threads/u,
    /(?:node:)?cluster/u,
    /\b(?:createRequire|getBuiltinModule)\b/u,
    /\bprocess\s*\[/u,
    /\bprocess\.(?:binding|_linkedBinding)\s*\(/u,
    /\b(?:Function|eval)\s*\(/u,
    /\bglobalThis\b/u,
    /\bReflect\.(?:apply|construct|get)\b/u,
    /\bmodule\s*\.\s*(?:constructor|createRequire)\b/u,
    /\bWebAssembly\b/u,
    /\brequire\s*\(/u,
    /\bBun\.(?:spawn|spawnSync)\b/u,
    /\bDeno\.Command\b/u,
  ].some((pattern) => pattern.test(source));
  const forbiddenLockSurface = [
    /\b(?:flock|lock_exclusive|try_lock_exclusive|proper-lockfile)\b/iu,
    /\bopenSync\s*\(/u,
    /\bAtomics\.wait(?:Async)?\s*\(/u,
    /\b(?:fs|os)\s*\[/u,
    /retrieval-generalization(?:-guard)?\.lock/iu,
    /process\.env\.(?:RUNNER_TEMP|TEMP|TMP|TMPDIR)\b/u,
    /["']\/tmp(?:\/|["'])/u,
  ].some((pattern) => pattern.test(source));

  add(
    violations,
    !legacyWrapperPresent,
    `${legacyRetrievalGeneralizationWrapper} must stay deleted so workspace nextest cannot rediscover the serialized Rust wrapper`,
  );
  add(
    violations,
    invocationCount === 1 && lintReferenceCount === 2,
    `${retrievalGeneralizationSuiteFile} must execute the hostile fixture matrix through one in-process lint invocation`,
  );
  add(
    violations,
    !forbiddenConcurrencySurface
      && dynamicImportCount === retainedDynamicImportFixtures.length
      && retainedDynamicImportFixtures.every((fixture) => source.includes(fixture))
      && importSpecifiers.length === allowedImports.length
      && sameMembers(importSpecifiers, allowedImports),
    `${retrievalGeneralizationSuiteFile} must not create subprocesses, workers, or clusters for hostile fixtures`,
  );
  add(
    violations,
    !forbiddenLockSurface,
    `${retrievalGeneralizationSuiteFile} must not restore a global or cross-process fixture lock`,
  );
  add(
    violations,
    fixtureFilesystemShapeIsExact
      && syntheticWritesStayInRegisteredRoots
      && fixturePathReferenceShapeIsExact
      && !/\bfs\.promises\b/u.test(source),
    `${retrievalGeneralizationSuiteFile} must confine every filesystem mutation to registered roots in its one synthetic fixture tree`,
  );
  add(
    violations,
    source.includes(
      'fs.mkdtempSync(path.join(os.tmpdir(), "codestory-generalization-"))',
    )
      && temporaryRootCount === 1
      && temporaryTreeCount === 1
      && source.includes(
        'assert.ok(\n    path.relative(repositoryRoot, fixtureRoot).startsWith(".."),',
      )
      && source.includes("const productionRepositoryRoot = path.join(\n    fixtureRoot,")
      && source.includes("const extraRustRoot = path.join(fixtureRoot,")
      && source.includes("const nonRustRoot = path.join(fixtureRoot,")
      && source.includes("const taskRoot = path.join(fixtureRoot,"),
    `${retrievalGeneralizationSuiteFile} must keep every mutable hostile fixture under one temporary tree outside the checkout`,
  );
  add(
    violations,
    checkoutDigestCount === 2
      && source.includes("const checkoutBefore = treeDigest(repositoryRoot);")
      && source.includes(
        "assert.equal(\n      treeDigest(repositoryRoot),\n      checkoutBefore,",
      )
      && source.includes(
        '"lint changed the whole checkout tree, including tracked bytes or untracked paths"',
      ),
    `${retrievalGeneralizationSuiteFile} must prove the real checkout is byte-for-byte read-only`,
  );
  add(
    violations,
    source.includes("structuralScanRoots: [rustRoot, extraRustRoot],")
      && source.includes("CODESTORY_RETRIEVAL_GENERALIZATION_EXTRA_SCAN_ROOTS: extraRustRoot,")
      && source.includes("CODESTORY_RETRIEVAL_GENERALIZATION_EXTRA_TASK_ROOTS: taskRoot,"),
    `${retrievalGeneralizationSuiteFile} must register every additive hostile fixture root in the single lint invocation`,
  );
  add(
    violations,
    source.includes("fs.rmSync(fixtureRoot, { recursive: true, force: true });"),
    `${retrievalGeneralizationSuiteFile} must remove its isolated fixture tree after the matrix`,
  );
  return violations;
}

function object(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function list(value) {
  if (value === undefined || value === null) return [];
  return Array.isArray(value) ? value : [value];
}

function at(value, ...keys) {
  let current = value;
  for (const key of keys) {
    if (current === null || typeof current !== "object") return undefined;
    current = current[key];
  }
  return current;
}

function canonicalJson(value) {
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(",")}]`;
  }
  if (value !== null && typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map(key => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

// A parsed job is not its complete execution contract. Workflow-level environment and run
// defaults execute inside every job, while triggers, permissions, concurrency, and future
// top-level fields can change when or with what authority it runs. Hash the entire parsed
// workflow except `jobs`; the acceptance manifest hashes those bodies separately.
function workflowExecutionContext(workflowValue) {
  return Object.fromEntries(
    Object.entries(object(workflowValue)).filter(([key]) => key !== "jobs"),
  );
}

function scalarStrings(value, found = []) {
  if (typeof value === "string") {
    found.push(value);
  } else if (Array.isArray(value)) {
    for (const item of value) scalarStrings(item, found);
  } else if (value !== null && typeof value === "object") {
    for (const item of Object.values(value)) scalarStrings(item, found);
  }
  return found;
}

function includesAll(values, expected) {
  const present = new Set(list(values));
  return expected.every(value => present.has(value));
}

function sameMembers(actual, expected) {
  const left = [...new Set(list(actual))].sort();
  const right = [...new Set(expected)].sort();
  return JSON.stringify(left) === JSON.stringify(right);
}

function needs(job) {
  return list(job?.needs);
}

function namedStep(job, name) {
  const matches = list(job?.steps).filter(step => object(step).name === name);
  return matches.length === 1 ? matches[0] : undefined;
}

function stepRun(job, name) {
  const run = namedStep(job, name)?.run;
  return typeof run === "string" ? run : "";
}

function executableRunText(run) {
  return run
    .split(/\r?\n/u)
    .filter(line => !/^\s*#/u.test(line))
    .join("\n");
}

// Shell concatenates adjacent quoted and unquoted literal fragments before it
// dispatches a command: `cpu_"explicit"` is the exact `cpu_explicit` argument
// and `"c"pu` is `cpu`. Policy checks must inspect that executable spelling,
// not the source-level quote placement an evasion chose.
function shellLiteralNormalizedText(run) {
  return executableRunText(String(run ?? "")).replaceAll(/['"]/gu, "");
}

function hasNonLiteralCpuAssignment(value) {
  const normalized = shellLiteralNormalizedText(value);
  return [...normalized.matchAll(
    /\bCODESTORY_EMBED_ALLOW_CPU\s*=\s*([^\s;&|]+)/giu,
  )].some(([, assigned]) => assigned !== "0");
}

function hasShellLoop(run) {
  return /(?:^|[;\n])\s*(?:for|select|until|while)\b/imu
    .test(shellLiteralNormalizedText(run));
}

function add(violations, condition, message) {
  if (!condition) violations.push(message);
}

function requireStepRun(violations, file, job, name, fragments) {
  const run = executableRunText(stepRun(job, name));
  add(violations, run.length > 0, `${file} must contain named step ${name}`);
  for (const fragment of fragments) {
    add(
      violations,
      run.includes(fragment),
      `${file} step ${name} must run ${fragment}`,
    );
  }
}

function forbidStepRun(violations, file, job, name, fragments) {
  const run = executableRunText(stepRun(job, name));
  for (const fragment of fragments) {
    add(
      violations,
      !run.includes(fragment),
      `${file} step ${name} must not run ${fragment}`,
    );
  }
}

function occurrenceCount(value, fragment) {
  return value.split(fragment).length - 1;
}

// A backslash-continued shell command is one logical invocation. Asserting a
// flag appears "somewhere after" an anchor is defeatable by parking the flag on
// a later decoy line, so every invocation-level pin below reads the single
// logical command that carries its anchor.
function shellInvocationsContaining(run, anchor) {
  const commands = [];
  let current = [];
  for (const line of executableRunText(run).split(/\r?\n/u)) {
    current.push(line);
    if (!/\\\s*$/u.test(line)) {
      commands.push(current.join("\n"));
      current = [];
    }
  }
  if (current.length > 0) commands.push(current.join("\n"));
  return commands.filter(command => command.includes(anchor));
}

function jobShellInvocationsContaining(job, anchor) {
  return list(job?.steps).flatMap(step =>
    shellInvocationsContaining(
      shellLiteralNormalizedText(object(step).run),
      shellLiteralNormalizedText(anchor),
    ));
}

function requireFlagOnInvocation(violations, message, run, anchor, flag) {
  const invocations = shellInvocationsContaining(run, anchor);
  add(
    violations,
    invocations.length === 1
      && invocations[0].includes(flag)
      && occurrenceCount(executableRunText(run), flag) === 1,
    message,
  );
}

// ---------------------------------------------------------------------------
// Reachability of a guarded step.
//
// A policy that only asserts a flag is present proves nothing when the step
// carrying it cannot run: that is exactly how the calibration freeze lineage
// guard sat "enabled" on a branch gated behind an input every caller pinned
// empty. The helpers below evaluate a step's own `if` against the input
// bindings a named caller actually passes, so making the step unreachable --
// by narrowing the condition or by stopping the caller forwarding what it
// reads -- is a policy violation rather than a silent regression.
const conditionTokenPattern = /^(&&|\|\||!=|==|!|\(|\)|'[^']*'|[A-Za-z_][A-Za-z0-9_.-]*)/u;

function tokenizeCondition(expression) {
  const tokens = [];
  let rest = String(expression).replace(/\s+/gu, " ").trim();
  while (rest.length > 0) {
    const match = rest.match(conditionTokenPattern);
    if (!match) {
      throw new Error(`unsupported condition syntax near ${JSON.stringify(rest)}`);
    }
    tokens.push(match[1]);
    rest = rest.slice(match[1].length).trimStart();
  }
  return tokens;
}

function conditionTruthy(value) {
  return typeof value === "string" ? value !== "" : Boolean(value);
}

function evaluateCondition(expression, lookup) {
  const tokens = tokenizeCondition(expression);
  let position = 0;
  const peek = () => tokens[position];
  const take = () => tokens[position++];
  function primary() {
    const token = take();
    if (token === undefined) throw new Error("condition ended early");
    if (token === "(") {
      const value = disjunction();
      if (take() !== ")") throw new Error("unbalanced condition parentheses");
      return value;
    }
    if (token === "!") return !conditionTruthy(primary());
    if (token === "true") return true;
    if (token === "false") return false;
    if (token.startsWith("'")) return token.slice(1, -1);
    return lookup(token);
  }
  function comparison() {
    const left = primary();
    if (peek() === "==" || peek() === "!=") {
      const operator = take();
      const right = primary();
      return operator === "==" ? left === right : left !== right;
    }
    return left;
  }
  function conjunction() {
    let value = comparison();
    while (peek() === "&&") {
      take();
      const right = comparison();
      value = conditionTruthy(value) ? right : value;
    }
    return value;
  }
  function disjunction() {
    let value = conjunction();
    while (peek() === "||") {
      take();
      const right = conjunction();
      value = conditionTruthy(value) ? value : right;
    }
    return value;
  }
  const result = disjunction();
  if (position !== tokens.length) throw new Error("trailing condition tokens");
  return conditionTruthy(result);
}

function calleeInputSpecifications(workflow) {
  const declared = object(at(workflow, "on", "workflow_call", "inputs"));
  const specifications = new Map();
  for (const [name, raw] of Object.entries(declared)) {
    const specification = object(raw);
    const boolean = specification.type === "boolean";
    specifications.set(name, {
      boolean,
      default: specification.default ?? (boolean ? false : ""),
    });
  }
  return specifications;
}

// Classify what a caller can make each callee input be. A literal is fixed for
// every run of that caller; ANY interpolated value is free, so this check never
// invents reachability the caller cannot actually deliver.
//
// There used to be a `${{ inputs.x || '' }}` regex here that separated a
// verbatim-forwarded dispatch input from every other expression -- and then set
// both to the same free binding. The two branches were identical, so the regex
// decided nothing while the comment above it claimed a distinction the code did
// not make. Freeness is the conservative answer for every expression, which is
// what this now says once.
export function callerInputBindings(callerJob, specifications) {
  const supplied = object(callerJob.with);
  const bindings = new Map();
  for (const [name, specification] of specifications) {
    const domain = specification.boolean ? [true, false] : ["", "supplied-by-dispatch"];
    if (!(name in supplied)) {
      bindings.set(name, { fixed: true, values: [specification.default] });
      continue;
    }
    const value = supplied[name];
    if (typeof value !== "string" || !value.includes("${{")) {
      bindings.set(name, { fixed: true, values: [value] });
      continue;
    }
    bindings.set(name, { fixed: false, values: domain });
  }
  return bindings;
}

// Enumerate every value the named caller can produce for the identifiers the
// condition reads, and report whether any of them makes the step run.
function conditionIsSatisfiable(condition, bindings, extraDomains) {
  let identifiers;
  try {
    identifiers = [...new Set(tokenizeCondition(condition).filter(token =>
      token.includes(".")))];
  } catch {
    return { satisfiable: false, reason: "condition syntax is not evaluable" };
  }
  const domains = [];
  for (const identifier of identifiers) {
    if (identifier in extraDomains) {
      domains.push([identifier, extraDomains[identifier]]);
      continue;
    }
    if (!identifier.startsWith("inputs.")) {
      return { satisfiable: false, reason: `${identifier} is not a caller-bound input` };
    }
    const binding = bindings.get(identifier.slice("inputs.".length));
    if (binding === undefined) {
      return { satisfiable: false, reason: `${identifier} is not a declared input` };
    }
    domains.push([identifier, binding.values]);
  }
  const assignment = new Map();
  const search = (index) => {
    if (index === domains.length) {
      try {
        return evaluateCondition(condition, name => {
          if (!assignment.has(name)) throw new Error(`unbound ${name}`);
          return assignment.get(name);
        });
      } catch {
        return false;
      }
    }
    const [identifier, values] = domains[index];
    for (const value of values) {
      assignment.set(identifier, value);
      if (search(index + 1)) return true;
    }
    return false;
  };
  return search(0)
    ? { satisfiable: true, reason: "" }
    : { satisfiable: false, reason: "no caller dispatch satisfies the condition" };
}

function requireNoCalibrationReferences(
  violations,
  file,
  workflow,
  allowedSteps = [],
) {
  const inspected = structuredClone(workflow);
  for (const [jobName, job] of Object.entries(object(inspected.jobs))) {
    if (!Array.isArray(job.steps)) continue;
    job.steps = job.steps.filter(
      step => !allowedSteps.some(
        ([allowedJob, allowedName]) =>
          allowedJob === jobName && allowedName === object(step).name,
      ),
    );
  }
  add(
    violations,
    !JSON.stringify(inspected).toLowerCase().includes("calibration"),
    `${file} standard release path must not reference calibration`,
  );
}

function requireOptionalStringInput(violations, file, workflow, event, key) {
  const input = object(at(workflow, "on", event, "inputs", key));
  add(
    violations,
    input.required === false && input.type === "string" && input.default === "",
    `${file} ${event} ${key} must be an optional empty string`,
  );
}

function exactResolverRunText(run) {
  return run.replace(/\r\n/gu, "\n");
}

function requireExactResolverContract(violations, file, job, expectedDigest) {
  const run = exactResolverRunText(stepRun(job, "Resolve trusted exact head"));
  const digest = createHash("sha256").update(run).digest("hex");
  add(
    violations,
    run.length > 0 && digest === expectedDigest,
    `${file} resolver must match the exact normalized trusted resolver script contract`,
  );
}

// Fragment assertions are substring matches, so they prove a string is present and nothing about
// what it does: a guard body can be replaced with `true '<the pinned regex>'` and still satisfy
// them. Digesting the executable text pins the whole script, so any rewrite has to be reviewed
// rather than merely keep the quoted evidence around. Comments are stripped so prose can be
// improved without churning the constant.
function requireExactStepScript(violations, file, job, name, expectedDigest, subject) {
  const run = executableRunText(stepRun(job, name)).replace(/\r\n/gu, "\n");
  const digest = createHash("sha256").update(run).digest("hex");
  add(
    violations,
    run.length > 0 && digest === expectedDigest,
    `${file} step ${name} must match the reviewed ${subject} script exactly`,
  );
}

// A line that begins with `#` is not necessarily a shell comment when the
// preceding line opened a quote. Hash raw text for scripts with multiline
// quoted programs so quote-context rewrites cannot disappear from the digest.
function requireExactRawStepScript(violations, file, job, name, expectedDigest, subject) {
  const run = exactResolverRunText(stepRun(job, name));
  const digest = createHash("sha256").update(run).digest("hex");
  add(
    violations,
    run.length > 0 && digest === expectedDigest,
    `${file} step ${name} must match the reviewed ${subject} script exactly`,
  );
}

function stepIndex(job, name) {
  return list(job?.steps).map(object).findIndex(step => step.name === name);
}

function qualificationDriverHandoffIsSealed(
  job,
  verifierName,
  engineName,
  expectedIntermediateNames,
) {
  const steps = list(job?.steps).map(object);
  const verifierIndex = stepIndex(job, verifierName);
  const engineIndex = stepIndex(job, engineName);
  if (verifierIndex < 0 || engineIndex <= verifierIndex) return false;
  const intermediate = steps.slice(verifierIndex + 1, engineIndex);
  if (
    JSON.stringify(intermediate.map(step => step.name))
      !== JSON.stringify(expectedIntermediateNames)
  ) {
    return false;
  }
  return intermediate.every(step =>
    !scalarStrings(step).some(value =>
      value.includes("qualification-driver")
      || value.includes("VERIFIED_QUALIFICATION_DRIVER")
      || value.includes("codestory_embedding_qualification")));
}

function cacheSteps(job) {
  return list(job?.steps)
    .map(object)
    .filter(step => typeof step.uses === "string"
      && /^actions\/cache\/(?:restore|save)@/iu.test(step.uses));
}

function cachePaths(step) {
  return String(object(step?.with).path ?? "")
    .split(/\r?\n/u)
    .map(value => value.trim())
    .filter(Boolean);
}

function cachePathsExcludeExactOutputs(job) {
  const forbidden = /(^|[/\\])target($|[/\\])|release-dist|native-seeds|embedded-model|notarization|qualification|proof|\.tar\.gz$|\.zip$|sha256/iu;
  return cacheSteps(job).every(step =>
    cachePaths(step).every(cachePath => !forbidden.test(cachePath)));
}

/// Routing a dispatched value through `env:` moves it out of the script's text, and out of reach
/// of a fragment pin that used to name it there: `--expected-sha "$INPUT_REF"` reads the same
/// whether `INPUT_REF` carries `inputs.ref` or the pull request head an attacker controls. The
/// fragment pin and this binding pin are two halves of one assertion -- the script names a
/// variable, and the variable names the value the step was reviewed with.
function requireStepEnv(violations, file, job, name, bindings) {
  const env = object(namedStep(job, name)?.env);
  for (const [key, expected] of Object.entries(bindings)) {
    add(
      violations,
      env[key] === expected,
      `${file} step ${name} must bind ${key} to ${expected}`,
    );
  }
}

function requireStepUses(violations, file, job, name, expected) {
  add(
    violations,
    namedStep(job, name)?.uses === expected,
    `${file} step ${name} must use ${expected}`,
  );
}

function requireCalibrationProducerAuthentication(violations, file, job) {
  requireStepRun(violations, file, job, "Authenticate calibration bundle producer", [
    "actions/runs/",
    ".github/workflows/packaged-platform-pr.yml",
    "workflow_dispatch",
    "success",
    "embedding-calibration-bundle-",
    "artifacts?per_page=100",
    "expired",
  ]);
}

// The protected proof files' producer-authentication step fragments are rule
// instances and live in release-claims.json under workflow_policy.step_fragments;
// stepFragmentViolations evaluates them, so this boundary pins only the download
// action and the condition and binding contract. Callers outside that migrated
// family still pin authentication through
// requireCalibrationProducerAuthentication.
function requireCalibrationProducerBoundary(
  violations,
  file,
  job,
  expectedCondition,
) {
  requireStepUses(
    violations,
    file,
    job,
    "Download frozen calibration bundle",
    "actions/download-artifact@v8.0.1",
  );
  const authentication = namedStep(job, "Authenticate calibration bundle producer");
  const download = namedStep(job, "Download frozen calibration bundle");
  add(
    violations,
    authentication?.if === expectedCondition && download?.if === expectedCondition,
    `${file} calibration authentication and download must run only for qualification or freeze lineage`,
  );
  add(
    violations,
    object(download?.with)["run-id"] === "${{ inputs.calibration_bundle_run_id }}"
      && object(download?.with).name === "${{ inputs.calibration_bundle_artifact }}"
      && object(download?.with)["github-token"] === "${{ github.token }}",
    `${file} frozen calibration download must bind its artifact name, prior run, and token`,
  );
}

const qualificationDriverIdentityFields = [
  "schema_version",
  "source.commit",
  "source.tree",
  "release_version",
  "asset_target",
  "archive.file",
  "archive.bytes",
  "archive.sha256",
  "driver.file",
  "driver.bytes",
  "driver.sha256",
];

function expectedQualificationDriverContract() {
  return {
    producer_workflow: "packaged-platform-proof.yml",
    producer_job: "build",
    artifact_name_template: "codestory-qualification-driver-{asset_target}",
    artifact_directory_template: ".",
    identity_file: "qualification-driver-identity.json",
    identity_schema_version: 1,
    identity_fields: qualificationDriverIdentityFields,
    build_invocations_per_platform: 1,
    reuse_required: true,
    public_release_asset: false,
  };
}

export function qualificationDriverArtifactViolations(
  source,
  graph = loadReleaseClaimGraph(repositoryRoot),
) {
  const violations = [];
  const contract = object(
    object(graph.workflow_policy).qualification,
  ).driver_contract;
  add(
    violations,
    JSON.stringify(contract) === JSON.stringify(expectedQualificationDriverContract()),
    "release-claims.json must bind the private archive-qualified driver contract exactly",
  );
  const normalized = String(source ?? "").replace(/\r\n/gu, "\n");
  // The pinned digest holds both sides of the private-artifact contract: the producer
  // may read Cargo's trusted hard-linked build output but retains only a new singly
  // linked copy bound to the exact candidate archive, and the consumer rejects
  // symlinks, retained hardlinks, extra files, identity drift, and byte drift before
  // restoring execute permission. Any helper edit therefore requires policy and
  // mutation-test review in the same PR.
  add(
    violations,
    createHash("sha256").update(normalized).digest("hex")
      === pinnedProgramRow(graph, "qualification_driver_artifact").sha256,
    "qualification-driver-artifact.mjs must match the reviewed archive-bound producer and verifier contract",
  );
  for (const fragment of [
    '"linux-x64", {',
    'archiveExtension: "tar.gz"',
    'binary: "codestory_embedding_qualification"',
    'rustTarget: "x86_64-unknown-linux-gnu"',
    '"macos-arm64", {',
    'rustTarget: "aarch64-apple-darwin"',
    '"windows-x64", {',
    'archiveExtension: "zip"',
    'binary: "codestory_embedding_qualification.exe"',
    'rustTarget: "x86_64-pc-windows-msvc"',
    "metadata.isSymbolicLink()\n    || !metadata.isFile()\n    || metadata.nlink !== 1",
    "function regularBuildOutput(file, label)",
    "!Number.isSafeInteger(metadata.nlink)\n    || metadata.nlink < 1",
    "metadata.isSymbolicLink() || !metadata.isDirectory()",
    'fail("qualification driver helper arguments changed")',
    "containedRelativePath(root, candidate, label)",
    "rejectSymlinkedPath({",
    "const rootMetadata = lstatSync(resolvedRoot)",
    'fail(`${label} must not traverse symbolic links`)',
    "`codestory-cli-v${version}-${assetTarget}.${contract.archiveExtension}`",
    'targetDir,\n    contract.rustTarget,\n    "release",\n    contract.binary',
    'const sourceMetadata = regularBuildOutput(',
    'fail("qualification driver artifact directory must start empty")',
    "copyFileSync(source, staged)",
    'const stagedMetadata = regularFile(staged, "staged qualification driver")',
    "archiveBytes: archiveMetadata.size",
    "archiveDigest: sha256(archivePath)",
    "archiveFile: expectedArchiveFile",
    "identity.archive.file !== expectedArchiveFile",
    "archiveMetadata.size !== identity.archive.bytes",
    "sha256(archivePath) !== identity.archive.sha256",
    "metadata.size !== identity.driver.bytes",
    "sha256(driver) !== identity.driver.sha256",
    '"qualification-driver-identity.json"',
    'fail("qualification driver artifact directory contains unexpected files")',
    "chmodSync(driver, 0o755)",
    'archive: required(values, "--archive")',
    'trustedRoot: required(values, "--trusted-root")',
    'targetDir: required(values, "--target-dir")',
    'requireExactFlags(values, [...commonFlags, "--out-dir", "--target-dir"])',
    'requireExactFlags(values, [...commonFlags, "--artifact-dir"])',
  ]) {
    add(
      violations,
      normalized.includes(fragment),
      `qualification-driver-artifact.mjs must retain ${fragment}`,
    );
  }
  return violations;
}

function requireJob(violations, file, workflow, name) {
  const found = object(workflow.jobs)[name];
  add(violations, found !== undefined, `${file} must contain job ${name}`);
  return object(found);
}

const draftCachePaths = [
  "~/.cargo/registry",
  "~/.cargo/git",
  "target",
];
// Every exact sha256 pin is a rule INSTANCE and lives in release-claims.json under
// workflow_policy.pinned_programs; scripts/codestory-release-claims.mjs fails graph
// loading unless that block names exactly the reviewed pinned set. This table keeps
// only what stays code: the predicate implementation per subject kind and the
// reviewed violation message for each pinned program. Adding or re-pinning a program
// is a graph edit plus one row here; the digests themselves never return to code.
const pinnedProgramContracts = {
  packaged_platform_proof_workflow: {
    violation: row => `${row.file} must match the reviewed canonical workflow structure`,
  },
  packaged_platform_pr_workflow: {
    violation: row => `${row.file} must match the reviewed frozen-candidate coordinator structure`,
  },
  frozen_candidate_quality_workflow: {
    violation: row => `${row.file} must match the reviewed isolated evaluation-owner structure`,
  },
  macos_metal_proof_workflow: {
    violation: row => `${row.file} must match the reviewed protected Metal workflow structure`,
  },
  windows_vulkan_proof_workflow: {
    violation: row => `${row.file} must match the reviewed protected Windows Vulkan workflow structure`,
  },
  linux_vulkan_proof_workflow: {
    violation: row => `${row.file} must match the reviewed protected Linux Vulkan workflow structure`,
  },
  release_source_proof_sentinel: {
    violation: row => `${row.file} source proof placeholder must match the reviewed fail-closed sentinel`,
  },
  source_proof_resolver: { resolver: true },
  packaged_platform_pr_resolver: { resolver: true },
  marketplace_sync_dispatch_guard: { script: "dispatch coordinate guard" },
  packaged_platform_pr_closeout: { script: "coordinator closeout" },
  packaged_sccache_identity: { script: "pinned sccache identity capture" },
  packaged_linux_build: { script: "Linux container build and compiler-server ownership" },
  packaged_compile_clock_stop: { script: "compiler clock stop" },
  packaged_host_compiler_finalizer: { script: "host compiler-server finalizer" },
};

function pinnedProgramRow(graph, name) {
  return object(object(object(graph.workflow_policy).pinned_programs)[name]);
}

// One generic evaluator walks the graph-owned digest pins. A missing workflow is
// reported by the structural validator that owns the file, so each row evaluates
// only when its subject workflow parsed; every other absence (missing job, missing
// step, empty script) hashes to a mismatch exactly as the retired inline checks did.
export function pinnedProgramViolations(workflows, graph = loadReleaseClaimGraph(repositoryRoot)) {
  const violations = [];
  for (const [name, contract] of Object.entries(pinnedProgramContracts)) {
    const row = pinnedProgramRow(graph, name);
    const workflow = workflows.get(row.file);
    if (!workflow) continue;
    if (row.subject === "parsed_workflow_json") {
      add(
        violations,
        createHash("sha256").update(JSON.stringify(workflow)).digest("hex") === row.sha256,
        contract.violation(row),
      );
      continue;
    }
    const job = object(object(workflow.jobs)[row.job]);
    if (row.subject === "parsed_job_json") {
      add(
        violations,
        createHash("sha256").update(JSON.stringify(job)).digest("hex") === row.sha256,
        contract.violation(row),
      );
    } else if (row.subject === "step_script_executable_text") {
      requireExactStepScript(violations, row.file, job, row.step, row.sha256, contract.script);
    } else if (contract.resolver) {
      requireExactResolverContract(violations, row.file, job, row.sha256);
    } else {
      requireExactRawStepScript(violations, row.file, job, row.step, row.sha256, contract.script);
    }
  }
  return violations;
}

// Every step-fragment rule INSTANCE for a migrated workflow family lives in
// release-claims.json under workflow_policy.step_fragments; scripts/
// codestory-release-claims.mjs fails graph loading unless that block names exactly
// the reviewed rule rows, so membership cannot drift by data edit. The checker keeps
// only what stays code: the requireStepRun/forbidStepRun predicates each row's kind
// selects, whose violation text is byte-identical to the retired inline calls.
// A missing workflow is reported by the structural validator that owns the file, so
// each row evaluates only when its subject workflow parsed; every other absence
// (missing job, missing step, empty script) fails the fragment predicates exactly as
// the retired inline checks did.
export function stepFragmentViolations(workflows, graph = loadReleaseClaimGraph(repositoryRoot)) {
  const violations = [];
  for (const value of Object.values(object(object(graph.workflow_policy).step_fragments))) {
    const row = object(value);
    const workflow = workflows.get(row.file);
    if (!workflow) continue;
    const job = object(object(workflow.jobs)[row.job]);
    const evaluate = row.kind === "forbid" ? forbidStepRun : requireStepRun;
    evaluate(violations, row.file, job, row.step, list(row.fragments));
  }
  return violations;
}

// Every structural-pin rule INSTANCE for a migrated workflow family lives in
// release-claims.json under workflow_policy.structural_pins; scripts/
// codestory-release-claims.mjs fails graph loading unless that block names exactly
// the reviewed pin rows, so membership cannot drift by data edit. The checker keeps
// only what stays code: the job-existence, needs-edge, and workflow- or job-scoped
// permission predicates each row's kind selects, whose violation text is
// byte-identical to the retired inline calls.
// A missing workflow is reported by the structural validator that owns the file, so
// each row evaluates only when its subject workflow parsed; a missing job, a
// missing or widened needs edge, or an absent, weaker, or wider permission scope
// fails the pin predicates exactly as the retired inline checks did.
//
// Some retired inline checks named the artifact a scope protects or the role a job
// plays rather than the scope or edge itself; that reviewed violation text stays
// code, keyed by pin name, and the derived wording remains the default for every
// other row.
const structuralPinMessages = {
  post_publish_smoke_actions_read:
    row => `${row.file} must read the accepted pre-publish closeout`,
  auto_release_release_needs_detect_version:
    row => `${row.file} release must need version detection`,
  auto_release_release_contents_write:
    row => `${row.file} release caller must grant contents write`,
  auto_release_release_actions_write:
    row => `${row.file} release caller must grant actions write for superseded-run cancellation`,
  auto_release_release_pull_requests_read:
    row => `${row.file} release caller must pass pull-request metadata access to exact source proof`,
  release_actions_write:
    row => `${row.file} must cancel superseded proof runs before starting release work`,
  release_pull_requests_read:
    row => `${row.file} must grant the exact source resolver pull-request metadata access`,
  packaged_platform_pr_actions_write:
    row => `${row.file} must cancel superseded proof runs before package or hardware work`,
  packaged_platform_pr_contents_read:
    row => `${row.file} must use read-only contents permission`,
  packaged_platform_pr_macos_metal_proof_needs:
    row => `${row.file} Metal proof must wait only for routing and package proof`,
  packaged_platform_pr_windows_vulkan_proof_needs:
    row => `${row.file} Windows qualification must run independently of optional Metal quality`,
  packaged_platform_pr_linux_vulkan_proof_needs:
    row => `${row.file} Linux Vulkan proof must wait only for routing and package proof`,
  packaged_platform_pr_closeout_needs:
    row => `${row.file} closeout must wait for every selected platform proof`,
  freeze_barrier_source_proof_statuses_write:
    () => "[freeze_barrier] source-proof.yml acceptance must publish an exact-head commit status",
  freeze_barrier_source_proof_actions_write:
    row => `[freeze_barrier] ${row.file} must be able to cancel superseded runs`,
  freeze_barrier_packaged_platform_pr_statuses_read:
    () =>
      "[freeze_barrier] packaged-platform-pr.yml must authenticate the exact-head freeze status without broad workflow authority",
  freeze_barrier_packaged_platform_pr_actions_write:
    row => `[freeze_barrier] ${row.file} must be able to cancel superseded runs`,
};

export function structuralPinViolations(workflows, graph = loadReleaseClaimGraph(repositoryRoot)) {
  const violations = [];
  for (const [name, value] of Object.entries(object(object(graph.workflow_policy).structural_pins))) {
    const row = object(value);
    const workflow = workflows.get(row.file);
    if (!workflow) continue;
    if (row.kind === "job") {
      add(
        violations,
        object(workflow.jobs)[row.job] !== undefined,
        `${row.file} must contain job ${row.job}`,
      );
    } else if (row.kind === "needs") {
      const message = structuralPinMessages[name];
      add(
        violations,
        sameMembers(needs(object(object(workflow.jobs)[row.job])), list(row.needs)),
        message === undefined
          ? `${row.file} ${String(row.job).replaceAll("-", " ")} must need ${list(row.needs).join(", ")}`
          : message(row),
      );
    } else if (row.kind === "job_permission") {
      const message = structuralPinMessages[name];
      add(
        violations,
        object(object(object(workflow.jobs)[row.job]).permissions)[row.scope] === row.access,
        message === undefined
          ? `${row.file} ${String(row.job).replaceAll("-", " ")} must ${row.access} ${String(row.scope).replaceAll("-", " ")}`
          : message(row),
      );
    } else {
      const message = structuralPinMessages[name];
      add(
        violations,
        object(workflow.permissions)[row.scope] === row.access,
        message === undefined
          ? `${row.file} must ${row.access} ${String(row.scope).replaceAll("-", " ")}`
          : message(row),
      );
    }
  }
  return violations;
}

// Every cross-file constant-mirror rule INSTANCE (a repository file that must repeat
// a reviewed constant verbatim) lives in release-claims.json under
// workflow_policy.constant_mirrors; scripts/codestory-release-claims.mjs fails graph
// loading unless that block names exactly the reviewed mirror rows, so membership
// cannot drift by data edit. The checker keeps only what stays code: the one
// source-inclusion predicate every row binds and its reviewed violation message,
// byte-identical to the retired inline file/fragment table. Each mirror file is read
// exactly as the retired table read it - relative to the process working directory -
// so a missing mirror file still throws out of the validator instead of degrading
// into a softer violation.
export function constantMirrorViolations(graph = loadReleaseClaimGraph(repositoryRoot)) {
  const violations = [];
  for (const value of Object.values(object(object(graph.workflow_policy).constant_mirrors))) {
    const row = object(value);
    const source = fs.readFileSync(String(row.file), "utf8");
    for (const fragment of list(row.fragments)) {
      add(violations, source.includes(fragment), `${row.file} must preserve locked Cargo contract ${fragment}`);
    }
  }
  return violations;
}

// Every draft command-list rule INSTANCE (the exact serial proof commands the
// draft lane must run and the exact seed commands the retrieval producer must
// build) lives in release-claims.json under workflow_policy.command_lists;
// scripts/codestory-release-claims.mjs fails graph loading unless that block names
// exactly the reviewed list rows, so membership cannot drift by data edit. The
// checker keeps only what stays code: the one sequence-equality predicate every
// row binds, its reviewed violation messages, and the cache-key composition that
// digests the graph-declared seed commands into the shared proof topology.
function commandListRow(graph, name) {
  return object(object(object(graph.workflow_policy).command_lists)[name]);
}
const cacheRunner = "${{ runner.os }}";
const cacheRustVersion = "${{ steps.rust-cache-key.outputs.version }}";
const cacheTarget = "${{ steps.rust-cache-key.outputs.target }}";
const cacheManifests = "${{ hashFiles('Cargo.toml', 'crates/**/Cargo.toml', 'vendor/**/Cargo.toml') }}";
const cacheLock = "${{ hashFiles('Cargo.lock') }}";
const cacheSaveCondition = "success() && steps.cargo-cache-restore.outputs.cache-hit != 'true' && steps.cargo-cache-restore.outputs.cache-primary-key != ''";
const draftCompilerCachePath = "${{ runner.temp }}/codestory-draft-sccache";
const draftCompilerCachePrefix =
  "${{ runner.os }}-draft-sccache-v1-${{ steps.rust-cache-key.outputs.version }}-${{ steps.rust-cache-key.outputs.target }}";
const draftCompilerCachePrimary = `${draftCompilerCachePrefix}-${cacheLock}`;
const draftCompilerSaveCondition =
  "success() && steps.compiler-cache-restore.outputs.cache-hit != 'true' && steps.compiler-cache-restore.outputs.cache-primary-key != ''";
const draftCompilerSaveKey = "${{ steps.compiler-cache-restore.outputs.cache-primary-key }}";
const cacheSaveKey = "${{ steps.cargo-cache-restore.outputs.cache-primary-key }}";
const crateDurabilityDependencyTriggerPaths = [
  "Cargo.toml",
  "Cargo.lock",
  "vendor/**",
];
const draftWorkflowPaths = [
  "Cargo.lock",
  "Cargo.toml",
  "crates/**",
  ".github/scripts/check-runtime-config-boundary.mjs",
  ".github/scripts/check-runtime-config-boundary.test.mjs",
  ".github/scripts/install-linux-vulkan-build-deps.sh",
  ".github/scripts/check-workflow-policy.mjs",
  ".github/scripts/route-ci-proof.mjs",
  ".github/workflows/rust-ci.yml",
  ".github/workflows/source-proof.yml",
  "plugins/codestory/generated-mcp-catalog.json",
  "plugins/codestory/skills/codestory-grounding/**",
  "scripts/generate-codestory-skill-syntax.mjs",
];
const retrievalProducerTriggerPaths = [
  "crates/**/Cargo.toml",
  "vendor/**/Cargo.toml",
  ".github/scripts/install-windows-vulkan-sdk.ps1",
  ".github/workflows/rust-ci.yml",
  "scripts/lint-retrieval-generalization.mjs",
  "scripts/lib/retrieval-generalization-lint.mjs",
  "scripts/tests/lint-retrieval-generalization.test.mjs",
];
const windowsVulkanInstaller = ".github/scripts/install-windows-vulkan-sdk.ps1";
const windowsNativeGenerator = "Ninja";
const windowsReadyCommand = "cargo test --locked -p codestory-cli --test ready_command";
const windowsReadyProofTopologyDigest = createHash("sha256")
  .update(`${windowsReadyCommand}\nCMAKE_GENERATOR=${windowsNativeGenerator}`)
  .digest("hex");
const windowsReadyProofTopology = `ready-command-v2-${windowsReadyProofTopologyDigest}`;
const windowsInstallerHash = `\${{ hashFiles('${windowsVulkanInstaller}') }}`;
const windowsCachePrimary = [
  cacheRunner,
  "cargo-stable",
  cacheRustVersion,
  cacheTarget,
  "windows",
  windowsReadyProofTopology,
  "generator",
  windowsNativeGenerator.toLowerCase(),
  "cmake",
  "${{ steps.rust-cache-key.outputs.cmake }}",
  "ninja",
  "${{ steps.rust-cache-key.outputs.ninja }}",
  "default-features",
  cacheManifests,
  windowsInstallerHash,
  cacheLock,
].join("-");
const windowsStepSequence = [
  { uses: "actions/checkout@v5", keys: ["uses"] },
  { name: "Install Rust stable", keys: ["name", "shell", "run"] },
  {
    name: "Install checksum-pinned Windows Vulkan SDK",
    keys: ["name", "shell", "run"],
  },
  { name: "Capture Rust cache identity", keys: ["name", "id", "shell", "run"] },
  {
    name: "Restore Windows Cargo inputs and output",
    keys: ["name", "id", "uses", "continue-on-error", "with"],
  },
  {
    name: "Prove Windows ready_command manifest-missing contract",
    keys: ["name", "shell", "run"],
  },
  {
    name: "Save Windows Cargo inputs and output",
    keys: ["name", "if", "uses", "continue-on-error", "with"],
  },
];
const windowsRunCommands = new Map([
  ["Install Rust stable", [
    "rustup toolchain install stable --profile minimal",
    "rustup default stable",
  ]],
  ["Install checksum-pinned Windows Vulkan SDK", [windowsVulkanInstaller]],
  ["Capture Rust cache identity", [
    "$release = rustc -Vv | Select-String '^release: ' | ForEach-Object { $_.ToString().Substring(9) }",
    "$target = rustc -Vv | Select-String '^host: ' | ForEach-Object { $_.ToString().Substring(6) }",
    "$cmake = (cmake --version | Select-Object -First 1) -replace '^cmake version ', ''",
    "$ninja = (ninja --version).Trim()",
    `"version=$release" | Out-File -FilePath $env:GITHUB_OUTPUT -Append`,
    `"target=$target" | Out-File -FilePath $env:GITHUB_OUTPUT -Append`,
    `"cmake=$cmake" | Out-File -FilePath $env:GITHUB_OUTPUT -Append`,
    `"ninja=$ninja" | Out-File -FilePath $env:GITHUB_OUTPUT -Append`,
  ]],
  ["Prove Windows ready_command manifest-missing contract", [windowsReadyCommand]],
]);
const draftStepSequence = [
  { uses: "actions/checkout@v5", keys: ["uses"] },
  { name: "Install Rust stable", keys: ["name", "run"] },
  { name: "Install Linux Vulkan build dependencies", keys: ["name", "run"] },
  { name: "Install pinned sccache", keys: ["name", "uses", "with"] },
  { name: "Configure bounded compiler cache", keys: ["name", "shell", "run"] },
  { name: "Capture Rust cache identity", keys: ["name", "id", "shell", "run"] },
  {
    name: "Restore Cargo inputs and output",
    keys: ["name", "id", "uses", "continue-on-error", "with"],
  },
  {
    name: "Restore compiler objects",
    keys: ["name", "id", "uses", "continue-on-error", "with"],
  },
  { name: "Check formatting", keys: ["name", "run"] },
  { name: "Check immutable runtime configuration boundary", keys: ["name", "run"] },
  { name: "Check the workspace", keys: ["name", "run"] },
  { name: "Check generated CodeStory syntax and MCP catalog", keys: ["name", "run"] },
  { name: "Lint workspace libraries", keys: ["name", "run"] },
  { name: "Prove focused publication contracts", keys: ["name", "run"] },
  {
    name: "Save Cargo inputs and output",
    keys: ["name", "if", "uses", "continue-on-error", "with"],
  },
  {
    name: "Save compiler objects",
    keys: ["name", "if", "uses", "continue-on-error", "with"],
  },
];
const draftRunCommands = new Map([
  ["Configure bounded compiler cache", [
    "{",
    'echo "SCCACHE_DIR=$RUNNER_TEMP/codestory-draft-sccache"',
    'echo "SCCACHE_CACHE_SIZE=1G"',
    'echo "RUSTC_WRAPPER=sccache"',
    'echo "CARGO_INCREMENTAL=0"',
    'echo "CMAKE_C_COMPILER_LAUNCHER=sccache"',
    'echo "CMAKE_CXX_COMPILER_LAUNCHER=sccache"',
    '} >> "$GITHUB_ENV"',
  ]],
  ["Install Rust stable", [
    "rustup toolchain install stable --profile minimal --component clippy --component rustfmt",
    "rustup default stable",
  ]],
  ["Install Linux Vulkan build dependencies", [
    "bash .github/scripts/install-linux-vulkan-build-deps.sh",
  ]],
  ["Capture Rust cache identity", [
    `echo "version=$(rustc -Vv | sed -n 's/^release: //p')" >> "$GITHUB_OUTPUT"`,
    `echo "target=$(rustc -Vv | sed -n 's/^host: //p')" >> "$GITHUB_OUTPUT"`,
  ]],
  ["Check formatting", ["cargo fmt --check"]],
  ["Check immutable runtime configuration boundary", [
    "node --test .github/scripts/check-runtime-config-boundary.test.mjs",
    "node .github/scripts/check-runtime-config-boundary.mjs",
  ]],
  ["Check the workspace", ["cargo check --workspace --locked"]],
  ["Check generated CodeStory syntax and MCP catalog", [
    "cargo build --locked -p codestory-cli",
    "node scripts/generate-codestory-skill-syntax.mjs --check",
  ]],
  ["Lint workspace libraries", [
    "cargo clippy --workspace --lib --locked -- -D warnings",
  ]],
]);

function nonCommentLines(value) {
  return String(value ?? "")
    .split(/\r?\n/u)
    .map(line => line.trim())
    .filter(line => line.length > 0 && !line.startsWith("#"));
}

function sameStrings(actual, expected) {
  return JSON.stringify(actual) === JSON.stringify(expected);
}

function hasExactKeys(value, expected) {
  return sameMembers(Object.keys(object(value)), expected);
}

export function draftWorkflowPolicyViolations(workflowValue) {
  const violations = [];
  const workflow = object(workflowValue);
  const triggers = object(workflow.on);
  const pullRequest = object(triggers.pull_request);
  const permissions = object(workflow.permissions);
  const concurrency = object(workflow.concurrency);
  const jobs = object(workflow.jobs);

  add(
    violations,
    hasExactKeys(workflow, ["name", "on", "permissions", "concurrency", "jobs"]),
    "draft source workflow must keep its exact top-level policy shape",
  );
  add(
    violations,
    workflow.name === "Draft source checks",
    "draft source workflow name must remain Draft source checks",
  );
  add(
    violations,
    hasExactKeys(triggers, ["pull_request", "workflow_dispatch"]),
    "draft source workflow must use only pull_request and workflow_dispatch",
  );
  add(
    violations,
    hasExactKeys(pullRequest, ["paths"])
      && list(pullRequest.paths).length === draftWorkflowPaths.length
      && sameMembers(pullRequest.paths, draftWorkflowPaths),
    "draft source pull_request trigger must keep the exact path set",
  );
  add(
    violations,
    triggers.workflow_dispatch === null,
    "draft source workflow_dispatch trigger must remain input-free",
  );
  add(
    violations,
    hasExactKeys(permissions, ["contents"]) && permissions.contents === "read",
    "draft source workflow permissions must remain contents: read only",
  );
  add(
    violations,
    hasExactKeys(concurrency, ["group", "cancel-in-progress"])
      && concurrency.group === "rust-ci-${{ github.event.pull_request.number || github.ref }}"
      && concurrency["cancel-in-progress"] === true,
    "draft source workflow concurrency must keep its exact PR/ref cancellation contract",
  );
  add(
    violations,
    hasExactKeys(jobs, ["linux-draft"]),
    "draft source workflow must contain exactly the linux-draft job",
  );
  return violations;
}

export function retrievalProducerTriggerPolicyViolations(workflowValue) {
  const violations = [];
  const workflow = object(workflowValue);
  add(
    violations,
    includesAll(at(workflow, "on", "pull_request", "paths"), retrievalProducerTriggerPaths),
    "retrieval cache producer pull_request paths must cover every manifest and draft consumer change",
  );
  add(
    violations,
    includesAll(at(workflow, "on", "push", "branches"), ["dev/codestory-next"]),
    "retrieval cache producer must run on dev/codestory-next pushes",
  );
  add(
    violations,
    includesAll(at(workflow, "on", "push", "paths"), retrievalProducerTriggerPaths),
    "retrieval cache producer dev push paths must cover every manifest and draft consumer change",
  );
  return violations;
}

export function proofFloorPolicyViolations(
  workflows,
  graph = loadReleaseClaimGraph(repositoryRoot),
) {
  const violations = [];
  const policy = object(object(graph.workflow_policy).proof_floor);
  const architecture = object(policy.architecture_contract);
  add(
    violations,
    architecture.workflow === retrievalFile,
    `architecture proof must remain owned by ${retrievalFile}`,
  );
  const architectureWorkflow = workflows.get(architecture.workflow);
  add(
    violations,
    architectureWorkflow !== undefined,
    `${architecture.workflow} must exist for the architecture proof floor`,
  );
  const architectureJob = object(at(
    architectureWorkflow,
    "jobs",
    architecture.job,
  ));
  add(
    violations,
    hasExactKeys(architectureJob, ["runs-on", "timeout-minutes", "env", "steps"])
      && architectureJob["runs-on"] === "ubuntu-latest",
    `${architecture.workflow} ${architecture.job} must remain an unconditional universal job`,
  );
  const architectureStep = namedStep(
    architectureJob,
    "Architecture ownership contract tests",
  );
  add(
    violations,
    sameStrings(nonCommentLines(architectureStep?.run), [architecture.command])
      && architectureStep?.if === undefined
      && architectureStep?.["continue-on-error"] === undefined,
    `${architecture.workflow} ${architecture.job} must run the exact blocking architecture contract`,
  );
  add(
    violations,
    stepIndex(architectureJob, "Architecture ownership contract tests")
      < stepIndex(architectureJob, "Seed draft proof test-profile artifacts"),
    `${architecture.workflow} architecture contract must complete before draft artifacts are seeded and saved`,
  );

  const mergedLanes = object(policy.merged_suite_lanes);
  add(
    violations,
    mergedLanes.workflow === retrievalFile
      && mergedLanes.job === architecture.job,
    `merged suite lanes must remain owned by the ${retrievalFile} universal job`,
  );
  const mergedLanesJob = object(at(
    workflows.get(String(mergedLanes.workflow)),
    "jobs",
    mergedLanes.job,
  ));
  const mergedLanesStep = namedStep(mergedLanesJob, String(mergedLanes.step));
  add(
    violations,
    sameStrings(
      nonCommentLines(mergedLanesStep?.run),
      list(mergedLanes.commands).map(String),
    )
      && mergedLanesStep?.if === undefined
      && mergedLanesStep?.["continue-on-error"] === undefined,
    `${mergedLanes.workflow} ${mergedLanes.job} must run the exact blocking merged suite lanes`,
  );
  add(
    violations,
    stepIndex(mergedLanesJob, String(mergedLanes.step))
      < stepIndex(mergedLanesJob, "Seed draft proof test-profile artifacts"),
    `${mergedLanes.workflow} merged suite lanes must complete before draft artifacts are seeded and saved`,
  );

  const durability = object(policy.crate_durability);
  const file = String(durability.workflow ?? crateDurabilityFile);
  const workflow = workflows.get(file);
  add(violations, workflow !== undefined, `${file} must exist`);
  const triggers = object(workflow?.on);
  const pullRequest = object(triggers.pull_request);
  const push = object(triggers.push);
  const permissions = object(workflow?.permissions);
  const concurrency = object(workflow?.concurrency);
  const jobs = object(workflow?.jobs);
  const job = object(jobs[durability.job]);
  const steps = list(job.steps).map(object);

  add(
    violations,
    hasExactKeys(workflow, ["name", "on", "permissions", "concurrency", "jobs"])
      && workflow?.name === "crate-durability",
    `${file} must keep the exact source-only top-level shape`,
  );
  add(
    violations,
    hasExactKeys(triggers, ["pull_request", "push", "workflow_dispatch"])
      && triggers.workflow_dispatch === null,
    `${file} must run only on path-scoped pull requests, base pushes, and input-free dispatch`,
  );
  for (const [event, eventValue] of [
    ["pull_request", pullRequest],
    ["push", push],
  ]) {
    add(
      violations,
      list(eventValue.paths).length === list(durability.paths).length
        && sameMembers(eventValue.paths, durability.paths),
      `${file} ${event} paths must match the exact owning-path set`,
    );
    add(
      violations,
      includesAll(eventValue.paths, crateDurabilityDependencyTriggerPaths),
      `${file} ${event} paths must cover root Cargo manifests, the lockfile, and vendored dependencies`,
    );
  }
  add(
    violations,
    hasExactKeys(pullRequest, ["paths"])
      && hasExactKeys(push, ["branches", "paths"])
      && list(push.branches).length === list(durability.branches).length
      && sameMembers(push.branches, durability.branches),
    `${file} push must seed only the declared integration branches`,
  );
  add(
    violations,
    hasExactKeys(permissions, ["contents"]) && permissions.contents === "read",
    `${file} permissions must remain contents: read only`,
  );
  add(
    violations,
    hasExactKeys(concurrency, ["group", "cancel-in-progress"])
      && concurrency.group
        === "crate-durability-${{ github.event.pull_request.number || github.ref }}"
      && concurrency["cancel-in-progress"] === true,
    `${file} must cancel stale work within one PR or branch`,
  );
  add(
    violations,
    hasExactKeys(jobs, [durability.job])
      && hasExactKeys(job, ["runs-on", "timeout-minutes", "steps"])
      && job["runs-on"] === "ubuntu-latest"
      && job["timeout-minutes"] === durability.timeout_minutes,
    `${file} must contain one bounded Ubuntu durability job`,
  );

  const expectedSteps = [
    { uses: "actions/checkout@v5", keys: ["uses"] },
    { name: "Install Rust stable", keys: ["name", "run"] },
    { name: "Capture Rust cache identity", keys: ["name", "id", "shell", "run"] },
    {
      name: "Restore durability Cargo cache",
      keys: ["name", "id", "uses", "continue-on-error", "with"],
    },
    { name: "Run durability and coverage contracts", keys: ["name", "run"] },
    {
      name: "Save durability Cargo cache",
      keys: ["name", "if", "uses", "continue-on-error", "with"],
    },
  ];
  add(
    violations,
    steps.length === expectedSteps.length
      && steps.every((step, index) => {
        const expected = expectedSteps[index];
        return hasExactKeys(step, expected.keys)
          && (expected.name === undefined || step.name === expected.name)
          && (expected.uses === undefined || step.uses === expected.uses);
      }),
    `${file} must retain the exact serial checkout, cache, proof, and save sequence`,
  );

  const install = namedStep(job, "Install Rust stable");
  add(
    violations,
    sameStrings(nonCommentLines(install?.run), [
      "rustup toolchain install stable --profile minimal",
      "rustup default stable",
    ]),
    `${file} must use the stable minimal Rust toolchain`,
  );
  const capture = namedStep(job, "Capture Rust cache identity");
  add(
    violations,
    sameStrings(nonCommentLines(capture?.run), [
      `echo "version=$(rustc -Vv | sed -n 's/^release: //p')" >> "$GITHUB_OUTPUT"`,
      `echo "target=$(rustc -Vv | sed -n 's/^host: //p')" >> "$GITHUB_OUTPUT"`,
    ]),
    `${file} cache identity must bind the Rust release and host target`,
  );

  const restore = namedStep(job, "Restore durability Cargo cache");
  const restoreWith = object(restore?.with);
  const expectedCacheKey = [
    "${{ runner.os }}",
    durability.cache_namespace,
    "${{ steps.rust-cache-key.outputs.version }}",
    "${{ steps.rust-cache-key.outputs.target }}",
    "${{ hashFiles('Cargo.toml', 'crates/**/Cargo.toml', 'vendor/**/Cargo.toml') }}",
    "${{ hashFiles('Cargo.lock') }}",
  ].join("-");
  add(
    violations,
    restore?.uses === "actions/cache/restore@v5"
      && restore?.["continue-on-error"] === true
      && hasExactKeys(restoreWith, ["path", "key"])
      && sameStrings(nonCommentLines(restoreWith.path), draftCachePaths)
      && restoreWith.key === expectedCacheKey,
    `${file} must use its isolated exact Cargo cache without fallback prefixes`,
  );

  const proof = namedStep(job, "Run durability and coverage contracts");
  add(
    violations,
    sameStrings(nonCommentLines(proof?.run), durability.commands)
      && proof?.if === undefined
      && proof?.["continue-on-error"] === undefined,
    `${file} must run the three declared durability commands serially and blocking`,
  );

  const save = namedStep(job, "Save durability Cargo cache");
  const saveWith = object(save?.with);
  add(
    violations,
    save?.uses === "actions/cache/save@v5"
      && save?.["continue-on-error"] === true
      && save?.if === cacheSaveCondition
      && hasExactKeys(saveWith, ["path", "key"])
      && sameStrings(nonCommentLines(saveWith.path), draftCachePaths)
      && saveWith.key === cacheSaveKey
      && stepIndex(job, "Save durability Cargo cache")
        > stepIndex(job, "Run durability and coverage contracts"),
    `${file} must save the exact cache only after every durability command passes`,
  );
  add(
    violations,
    durability.artifact_free === true
      && !scalarStrings(workflow).some(value => /actions\/upload-artifact@/u.test(value)),
    `${file} must remain artifact-free`,
  );
  return violations;
}

export function windowsManifestProofPolicyViolations(workflowValue) {
  const violations = [];
  const workflow = object(workflowValue);
  const triggers = object(workflow.on);
  const job = object(at(workflow, "jobs", "windows-manifest-missing"));
  const linux = object(at(workflow, "jobs", "linux-contracts"));
  const steps = list(job.steps).map(object);

  add(
    violations,
    hasExactKeys(workflow.jobs, ["linux-contracts", "windows-manifest-missing"]),
    "Windows manifest proof workflow must contain exactly linux-contracts and windows-manifest-missing jobs",
  );
  add(
    violations,
    hasExactKeys(linux.env, ["CODESTORY_TEST_EMBED_ALLOW_CPU"])
      && linux.env.CODESTORY_TEST_EMBED_ALLOW_CPU === "1",
    "retrieval source tests must opt into the CPU test seam explicitly",
  );
  const nodeSetup = list(linux.steps).map(object)
    .find((step) => step.uses === "actions/setup-node@v5");
  add(
    violations,
    object(nodeSetup?.with)["node-version"] === "24"
      && object(nodeSetup?.with)["package-manager-cache"] === false
      && nodeSetup?.["continue-on-error"] === undefined,
    "retrieval generalization producer must use blocking Node 24 without a package-manager cache",
  );
  for (const [name, command] of [
    ["Generalization lint (production paths)", "node scripts/lint-retrieval-generalization.mjs"],
    [
      "Generalization lint hostile matrix",
      "node --test scripts/tests/lint-retrieval-generalization.test.mjs",
    ],
  ]) {
    const step = namedStep(linux, name);
    add(
      violations,
      sameStrings(nonCommentLines(step?.run), [command])
        && step?.["continue-on-error"] === undefined
        && step?.if === undefined,
      `retrieval generalization producer ${name} must run its exact blocking Node command`,
    );
  }
  add(
    violations,
    !scalarStrings(linux).some((value) =>
      value.includes("cargo test --locked -p codestory-runtime --test retrieval_generalization_guard")
    ),
    "retrieval generalization producer must not restore the serialized Rust subprocess wrapper",
  );
  add(
    violations,
    workflow.env === undefined,
    "Windows manifest proof workflow must not define top-level env",
  );
  add(
    violations,
    workflow.defaults === undefined,
    "Windows manifest proof workflow must not define top-level defaults",
  );
  for (const event of ["pull_request", "push"]) {
    add(
      violations,
      includesAll(at(triggers, event, "paths"), [windowsVulkanInstaller]),
      `Windows manifest proof ${event} paths must cover the Vulkan installer`,
    );
  }
  add(
    violations,
    triggers.workflow_dispatch === null,
    "Windows manifest proof workflow_dispatch must remain input-free",
  );
  add(
    violations,
    hasExactKeys(job, ["if", "runs-on", "timeout-minutes", "env", "steps"]),
    "Windows manifest proof job must keep its exact required serial shape",
  );
  add(
    violations,
    job.if === "github.event_name == 'workflow_dispatch'",
    "Windows manifest proof must be workflow-dispatch only",
  );
  add(
    violations,
    !scalarStrings(job).some(value => value.includes("labels")),
    "Windows manifest proof must not be label-triggered",
  );
  add(violations, job["runs-on"] === "windows-latest", "Windows manifest proof must use windows-latest");
  add(violations, job["timeout-minutes"] === 30, "Windows manifest proof timeout must remain 30 minutes");
  add(
    violations,
    hasExactKeys(job.env, ["CODESTORY_TEST_EMBED_ALLOW_CPU", "CMAKE_GENERATOR"])
      && job.env.CODESTORY_TEST_EMBED_ALLOW_CPU === "1"
      && job.env.CMAKE_GENERATOR === windowsNativeGenerator,
    "Windows manifest proof source test must explicitly use the CPU test seam and Ninja native generator",
  );
  add(
    violations,
    steps.length === windowsStepSequence.length,
    "Windows manifest proof must keep its exact serialized step count",
  );
  for (const [index, expected] of windowsStepSequence.entries()) {
    const step = steps[index];
    const matches = expected.name === undefined
      ? step?.uses === expected.uses
      : step?.name === expected.name;
    add(
      violations,
      matches,
      `Windows manifest proof step ${index + 1} must remain ${expected.name ?? expected.uses}`,
    );
    add(
      violations,
      hasExactKeys(step, expected.keys),
      `Windows manifest proof step ${index + 1} must keep the exact ${expected.name ?? expected.uses} key shape`,
    );
  }

  for (const [name, commands] of windowsRunCommands) {
    const step = namedStep(job, name);
    add(violations, step !== undefined, `Windows manifest proof must contain one ${name} step`);
    add(
      violations,
      sameStrings(nonCommentLines(step?.run), commands),
      `Windows manifest proof step ${name} must keep its exact required command sequence`,
    );
    add(
      violations,
      step?.shell === "pwsh",
      `Windows manifest proof step ${name} must use pwsh`,
    );
  }

  const identity = namedStep(job, "Capture Rust cache identity");
  add(
    violations,
    identity?.id === "rust-cache-key",
    "Windows manifest proof cache identity must keep its stable output id",
  );

  const restore = namedStep(job, "Restore Windows Cargo inputs and output");
  const restoreWith = object(restore?.with);
  add(
    violations,
    restore?.id === "cargo-cache-restore",
    "Windows manifest proof cache restore must keep its stable step id",
  );
  add(
    violations,
    restore?.uses === "actions/cache/restore@v5",
    "Windows manifest proof cache restore must use actions/cache/restore@v5",
  );
  add(
    violations,
    restore?.["continue-on-error"] === true && restore?.if === undefined,
    "Windows manifest proof cache restore must remain optional without conditional bypasses",
  );
  add(
    violations,
    hasExactKeys(restoreWith, ["path", "key"]),
    "Windows manifest proof cache restore must use an exact primary without fallbacks",
  );
  add(
    violations,
    sameStrings(nonCommentLines(restoreWith.path), draftCachePaths),
    "Windows manifest proof cache restore must use only Cargo registry, git, and default target paths",
  );
  add(
    violations,
    restoreWith.key === windowsCachePrimary,
    "Windows manifest proof cache key must bind OS, Rust, target, proof topology, default features, manifests, installer, and lock identities",
  );

  const proof = namedStep(job, "Prove Windows ready_command manifest-missing contract");
  const install = namedStep(job, "Install checksum-pinned Windows Vulkan SDK");
  const restoreIndex = steps.indexOf(restore);
  const proofIndex = steps.indexOf(proof);
  add(
    violations,
    steps.indexOf(install) < proofIndex && restoreIndex < proofIndex,
    "Windows manifest proof must install the SDK and restore only compatible output before the Cargo proof",
  );

  const save = namedStep(job, "Save Windows Cargo inputs and output");
  const saveWith = object(save?.with);
  add(
    violations,
    save?.uses === "actions/cache/save@v5",
    "Windows manifest proof cache save must use actions/cache/save@v5",
  );
  add(
    violations,
    save?.["continue-on-error"] === true,
    "Windows manifest proof cache save must remain non-blocking",
  );
  add(
    violations,
    save?.if === cacheSaveCondition,
    "Windows manifest proof cache save must require full proof success and skip exact hits",
  );
  add(
    violations,
    hasExactKeys(saveWith, ["path", "key"]),
    "Windows manifest proof cache save inputs must keep their exact shape",
  );
  add(
    violations,
    sameStrings(nonCommentLines(saveWith.path), draftCachePaths),
    "Windows manifest proof cache save must use the exact restore path set",
  );
  add(
    violations,
    saveWith.key === cacheSaveKey,
    "Windows manifest proof cache save must use the exact primary rather than a matched key",
  );
  add(
    violations,
    proofIndex + 1 === steps.indexOf(save),
    "Windows manifest proof cache save must immediately follow the successful Cargo proof",
  );

  return violations;
}

export function draftSourcePolicyViolations(
  jobValue,
  retrievalJobValue,
  graph = loadReleaseClaimGraph(repositoryRoot),
) {
  const violations = [];
  const job = object(jobValue);
  const retrievalJob = object(retrievalJobValue);
  const steps = list(job.steps).map(object);
  const proofRow = commandListRow(graph, "draft_proof_commands");
  const seedRow = commandListRow(graph, "draft_seed_commands");
  const proofCommands = list(proofRow.commands).map(String);
  const seedCommands = list(seedRow.commands).map(String);
  const draftProofTopology = `proof5-v1-${createHash("sha256")
    .update(seedCommands.join("\n"))
    .digest("hex")}`;
  const draftCachePrefix = [
    cacheRunner,
    "draft-v2",
    cacheRustVersion,
    cacheTarget,
    "workspace",
    draftProofTopology,
    "default-features",
    cacheManifests,
  ].join("-");
  const retrievalCachePrefix = [
    cacheRunner,
    "cargo-stable",
    cacheRustVersion,
    cacheTarget,
    "retrieval-contracts",
    draftProofTopology,
    "default-features",
    cacheManifests,
  ].join("-");
  const draftCachePrimary = `${draftCachePrefix}-${cacheLock}`;
  const retrievalCachePrimary = `${retrievalCachePrefix}-${cacheLock}`;
  const draftCacheRestoreKeys = [
    retrievalCachePrimary,
    `${draftCachePrefix}-`,
    `${retrievalCachePrefix}-`,
  ];

  add(
    violations,
    retrievalJob["timeout-minutes"] === 60,
    "retrieval cache producer timeout must remain 60 minutes",
  );
  add(
    violations,
    hasExactKeys(job, ["name", "runs-on", "timeout-minutes", "steps"]),
    "draft source job must keep its exact required serial shape",
  );
  add(
    violations,
    job.name === "Ubuntu draft source checks",
    "draft source job name must remain Ubuntu draft source checks",
  );
  add(violations, job["runs-on"] === "ubuntu-latest", "draft source job must use ubuntu-latest");
  add(violations, job["timeout-minutes"] === 45, "draft source job timeout must remain 45 minutes");
  add(violations, job.env === undefined && job.defaults === undefined, "draft source job must not override the proof environment or defaults");
  add(violations, job["continue-on-error"] === undefined && job.strategy === undefined, "draft source job must remain one required serial lane");
  add(violations, steps.length === draftStepSequence.length, "draft source job must keep its exact serialized step count");
  for (const [index, expected] of draftStepSequence.entries()) {
    const step = steps[index];
    const matches = expected.name === undefined
      ? step?.uses === expected.uses
      : step?.name === expected.name;
    add(violations, matches, `draft source step ${index + 1} must remain ${expected.name ?? expected.uses}`);
    add(
      violations,
      hasExactKeys(step, expected.keys),
      `draft source step ${index + 1} must keep the exact ${expected.name ?? expected.uses} key shape`,
    );
  }

  for (const [name, commands] of [
    ...draftRunCommands,
    [String(proofRow.step), proofCommands],
  ]) {
    const step = namedStep(job, name);
    add(violations, step !== undefined, `draft source job must contain one ${name} step`);
    add(violations, sameStrings(nonCommentLines(step?.run), commands), `draft source step ${name} must keep its exact serial command sequence`);
    add(violations, step?.["continue-on-error"] === undefined && step?.if === undefined, `draft source step ${name} must remain required`);
    add(violations, step?.env === undefined && step?.["working-directory"] === undefined, `draft source step ${name} must use the shared default build environment`);
  }

  const identity = namedStep(job, "Capture Rust cache identity");
  add(violations, identity?.id === "rust-cache-key" && identity?.shell === "bash", "draft source cache identity must keep its stable bash output contract");

  const restore = namedStep(job, "Restore Cargo inputs and output");
  const restoreWith = object(restore?.with);
  add(violations, restore?.id === "cargo-cache-restore", "draft source cache restore must keep its stable step id");
  add(violations, restore?.uses === "actions/cache/restore@v5", "draft source cache restore must use actions/cache/restore@v5");
  add(violations, restore?.["continue-on-error"] === true && restore?.if === undefined, "draft source cache restore must remain optional without conditional bypasses");
  add(
    violations,
    hasExactKeys(restoreWith, ["path", "key", "restore-keys"]),
    "draft source cache restore inputs must keep their exact key shape",
  );
  add(violations, sameStrings(nonCommentLines(restoreWith.path), draftCachePaths), "draft source cache restore must use only the Cargo registry, git, and default target paths");
  add(violations, restoreWith.key === draftCachePrimary, "draft source cache primary must bind the v2 platform, toolchain, target, proof topology, feature, manifest, and lock identity");
  add(violations, sameStrings(nonCommentLines(restoreWith["restore-keys"]), draftCacheRestoreKeys), "draft source cache fallbacks must keep the exact seeded retrieval, prior draft, then prior retrieval order and omit only the lock identity from prior prefixes");

  const sccache = namedStep(job, "Install pinned sccache");
  add(violations, sccache?.uses === sccacheAction, "draft source sccache must use the pinned mozilla-actions release");
  add(
    violations,
    object(sccache?.with).version === sccacheVersion
      && object(sccache?.with).disable_annotations === true,
    "draft source sccache must pin the shared sccache version without annotations",
  );

  const compilerRestore = namedStep(job, "Restore compiler objects");
  const compilerRestoreWith = object(compilerRestore?.with);
  add(violations, compilerRestore?.id === "compiler-cache-restore", "draft compiler cache restore must keep its stable step id");
  add(violations, compilerRestore?.uses === "actions/cache/restore@v5", "draft compiler cache restore must use actions/cache/restore@v5");
  add(violations, compilerRestore?.["continue-on-error"] === true && compilerRestore?.if === undefined, "draft compiler cache restore must remain optional without conditional bypasses");
  add(violations, compilerRestoreWith.path === draftCompilerCachePath, "draft compiler cache must live in the runner-temp sccache directory");
  add(violations, compilerRestoreWith.key === draftCompilerCachePrimary, "draft compiler cache primary must bind platform, toolchain, target, and lock identity");
  add(violations, sameStrings(nonCommentLines(compilerRestoreWith["restore-keys"]), [`${draftCompilerCachePrefix}-`]), "draft compiler cache fallback must omit only the lock identity");

  const compilerSave = namedStep(job, "Save compiler objects");
  const compilerSaveWith = object(compilerSave?.with);
  add(violations, compilerSave?.uses === "actions/cache/save@v5", "draft compiler cache save must use actions/cache/save@v5");
  add(violations, compilerSave?.["continue-on-error"] === true, "draft compiler cache save must remain non-blocking");
  add(violations, compilerSave?.if === draftCompilerSaveCondition, "draft compiler cache must save only on a successful run that missed its primary key");
  add(violations, compilerSaveWith.path === draftCompilerCachePath && compilerSaveWith.key === draftCompilerSaveKey, "draft compiler cache save must publish the restored primary key path");

  const retrievalRestore = namedStep(retrievalJob, "Restore Cargo registry, git sources, and build output");
  const retrievalRestoreWith = object(retrievalRestore?.with);
  add(
    violations,
    hasExactKeys(retrievalRestore, ["name", "id", "uses", "continue-on-error", "with"]),
    "retrieval cache producer restore must keep its exact step shape",
  );
  add(violations, retrievalRestore?.id === "cargo-cache-restore", "retrieval cache producer must keep its stable restore id");
  add(violations, retrievalRestore?.uses === "actions/cache/restore@v5", "retrieval cache producer must use actions/cache/restore@v5");
  add(violations, retrievalRestore?.["continue-on-error"] === true && retrievalRestore?.if === undefined, "retrieval cache producer restore must remain non-blocking without conditional bypasses");
  add(
    violations,
    hasExactKeys(retrievalRestoreWith, ["path", "key"]),
    "retrieval cache producer restore inputs must keep their exact key shape",
  );
  add(violations, sameStrings(nonCommentLines(retrievalRestoreWith.path), draftCachePaths), "retrieval cache producer must retain the proof-compatible path set");
  add(violations, retrievalRestoreWith.key === retrievalCachePrimary, "retrieval cache producer key must match the draft exact-lock, manifest, feature, and proof-topology fallback");

  const retrievalSeed = namedStep(retrievalJob, String(seedRow.step));
  add(
    violations,
    hasExactKeys(retrievalSeed, ["name", "run"]),
    "retrieval cache producer seed must keep its exact required step shape",
  );
  add(
    violations,
    sameStrings(nonCommentLines(retrievalSeed?.run), seedCommands),
    "retrieval cache producer must seed the exact five test-profile targets in serial order",
  );

  const retrievalSave = namedStep(retrievalJob, "Save Cargo registry, git sources, and build output");
  const retrievalSaveWith = object(retrievalSave?.with);
  add(
    violations,
    hasExactKeys(retrievalSave, ["name", "if", "uses", "continue-on-error", "with"]),
    "retrieval cache producer save must keep its exact post-proof step shape",
  );
  add(violations, retrievalSave?.uses === "actions/cache/save@v5", "retrieval cache producer must use actions/cache/save@v5");
  add(violations, retrievalSave?.["continue-on-error"] === true, "retrieval cache producer save must remain non-blocking");
  add(violations, retrievalSave?.if === cacheSaveCondition, "retrieval cache producer must save only after every retrieval and seed proof succeeds");
  add(
    violations,
    hasExactKeys(retrievalSaveWith, ["path", "key"]),
    "retrieval cache producer save inputs must keep their exact key shape",
  );
  add(violations, sameStrings(nonCommentLines(retrievalSaveWith.path), draftCachePaths), "retrieval cache producer save must retain the proof-compatible path set");
  add(violations, retrievalSaveWith.key === cacheSaveKey, "retrieval cache producer must save its exact primary rather than a matched key");
  const retrievalSteps = list(retrievalJob.steps).map(object);
  add(
    violations,
    retrievalSteps.indexOf(retrievalSeed) + 1 === retrievalSteps.indexOf(retrievalSave),
    "retrieval cache producer must seed the exact proof targets immediately before saving",
  );

  const save = namedStep(job, "Save Cargo inputs and output");
  const saveWith = object(save?.with);
  add(violations, save?.uses === "actions/cache/save@v5", "draft source cache promotion must use actions/cache/save@v5");
  add(violations, save?.["continue-on-error"] === true, "draft source cache promotion must remain non-blocking");
  add(
    violations,
    hasExactKeys(saveWith, ["path", "key"]),
    "draft source cache promotion inputs must keep their exact key shape",
  );
  add(
    violations,
    save?.if === cacheSaveCondition,
    "draft source cache promotion must require complete proof and a partial or missing primary",
  );
  add(violations, sameStrings(nonCommentLines(saveWith.path), draftCachePaths), "draft source cache promotion must use the exact restore path set");
  add(violations, saveWith.key === cacheSaveKey, "draft source cache promotion must save the exact primary rather than a matched fallback");

  return violations;
}

export const frozenCandidateQualityWorkflowRef = "./.github/workflows/frozen-candidate-quality.yml";

export function macosCliDistributionViolations(assessmentStep, executionStep, quarantinedPath) {
  const violations = [];
  const assessment = executableRunText(String(assessmentStep?.run ?? ""));
  const execution = executableRunText(String(executionStep?.run ?? ""));
  const assessmentLines = assessment.split("\n");
  const executionLines = execution.split("\n");
  const lineHas = (lines, ...fragments) => lines.some(line => fragments.every(fragment => line.includes(fragment)));
  add(violations, lineHas(assessmentLines, "xattr -w com.apple.quarantine", quarantinedPath), "macOS CLI proof must quarantine the assessed executable");
  add(violations, lineHas(assessmentLines, "xattr -p com.apple.quarantine", quarantinedPath), "macOS CLI proof must record the executable quarantine");
  add(violations, lineHas(assessmentLines, "spctl --assess --type execute --verbose=4", quarantinedPath), "macOS CLI proof must retain the spctl diagnostic for that executable");
  add(violations, assessment.includes("spctl_status=$?"), "macOS CLI proof must record the spctl diagnostic status");
  add(violations, assessment.includes("does not seem to be an app"), "macOS CLI proof must recognize the bare-executable spctl result");
  add(violations, !/(^|\n)\s*accepted=false\s*($|\n)/u.test(assessment), "macOS CLI proof must not require spctl application acceptance");
  add(violations, lineHas(executionLines, quarantinedPath, "--version") && lineHas(executionLines, quarantinedPath, "--help"), "macOS CLI proof must execute that quarantined binary's version and help");
  return violations;
}

export function parseWorkflow(source, file = "workflow") {
  const document = parseDocument(source, {
    lineCounter: new LineCounter(),
    prettyErrors: true,
    schema: "core",
    strict: true,
    uniqueKeys: true,
  });
  if (document.errors.length > 0) {
    throw new Error(document.errors.map(error => error.message).join("\n"));
  }
  const parsed = document.toJS({ maxAliasCount: 50 });
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`${file} must contain one YAML mapping`);
  }
  return parsed;
}

export function loadWorkflows(root = workflowRoot) {
  const loaded = new Map();
  for (const file of fs.readdirSync(root).filter(name => /\.ya?ml$/u.test(name)).sort()) {
    const source = fs.readFileSync(path.join(root, file), "utf8");
    loaded.set(file, parseWorkflow(source, file));
  }
  return loaded;
}

function trigger(workflow, name) {
  return object(workflow.on)[name];
}

function concurrencyCancels(workflow) {
  return object(workflow.concurrency)["cancel-in-progress"] === true;
}

function executableCargoLines(run) {
  return run
    .split(/\r?\n/u)
    .map((line, index) => ({ line, number: index + 1 }))
    .filter(({ line }) =>
      /^\s*(?:[A-Z_][A-Z0-9_]*=\S+\s+)*(?:sudo\s+)?cargo\s+(?:build|check|test|clippy|doc|run)\b/u.test(line),
    );
}

function walk(value, visit, trail = []) {
  if (Array.isArray(value)) {
    value.forEach((item, index) => walk(item, visit, [...trail, index]));
    return;
  }
  if (value === null || typeof value !== "object") return;
  for (const [key, child] of Object.entries(value)) {
    visit(key, child, [...trail, key]);
    walk(child, visit, [...trail, key]);
  }
}

export function basicWorkflowViolations(file, workflow) {
  const violations = [];
  const triggers = object(workflow.on);
  if (triggers.pull_request !== undefined || triggers.pull_request_target !== undefined) {
    add(
      violations,
      concurrencyCancels(workflow),
      `${file} pull-request runs must cancel stale work`,
    );
  }

  // Least privilege is not the repository default, so every workflow states its own scopes.
  add(
    violations,
    workflow.permissions !== undefined,
    `${file} must declare a top-level permissions block`,
  );

  // Reusable-workflow callers inherit the callee's budget and cannot set one themselves, so the
  // rule applies only to jobs that own steps.
  for (const [jobName, rawJob] of Object.entries(object(workflow.jobs))) {
    const job = object(rawJob);
    if (!Array.isArray(job.steps)) continue;
    add(
      violations,
      job["timeout-minutes"] !== undefined,
      `${file} jobs.${jobName} must declare timeout-minutes`,
    );
  }

  walk(workflow, (key, value, trail) => {
    if (key === "key" && typeof value === "string" && value.includes("github.sha")) {
      violations.push(`${file} ${trail.join(".")} Cargo cache key must not include commit SHA`);
    }
    if (key !== "uses" || typeof value !== "string" || value.startsWith("./")) return;
    const separator = value.lastIndexOf("@");
    if (separator < 0) {
      violations.push(`${file} ${value} is missing an action ref`);
      return;
    }
    const owner = value.slice(0, separator).split("/")[0];
    const ref = value.slice(separator + 1);
    if (!trustedActionOwners.has(owner) && !fullSha.test(ref)) {
      violations.push(`${file} ${value} must pin third-party actions to a full-length SHA`);
    }
  });

  for (const [jobName, job] of Object.entries(object(workflow.jobs))) {
    for (const [stepIndex, step] of list(object(job).steps).entries()) {
      if (typeof step?.run !== "string") continue;
      for (const { line, number } of executableCargoLines(step.run)) {
        if (!/(?:^|\s)--locked(?:\s|$)/u.test(line)) {
          violations.push(
            `${file} jobs.${jobName}.steps.${stepIndex}.run:${number} dependency-resolving Cargo command must use --locked`,
          );
        }
      }
    }
  }
  return violations;
}

export function packagedPrSigningViolations(workflow) {
  const violations = [];
  const job = object(object(workflow.jobs)["packaged-proof"]);
  add(
    violations,
    object(job.with).sign_macos === false,
    "packaged-platform-pr.yml packaged-proof must set sign_macos to false",
  );
  add(
    violations,
    job.secrets === undefined,
    "packaged-platform-pr.yml packaged-proof must not receive caller secrets",
  );
  const scalars = scalarStrings(workflow);
  let referencesAppleSecret = false;
  walk(workflow, (key, value) => {
    if (/^APPLE_[A-Z0-9_]+$/u.test(key)) referencesAppleSecret = true;
    if (typeof value === "string" && /\bAPPLE_[A-Z0-9_]+\b/u.test(value)) {
      referencesAppleSecret = true;
    }
  });
  add(
    violations,
    !referencesAppleSecret,
    "packaged-platform-pr.yml must not reference Apple secret identifiers",
  );
  add(
    violations,
    !scalars.some(value => value.includes("macos-release-signing")),
    "packaged-platform-pr.yml must not reference the release signing environment",
  );
  return violations;
}

export function notaryStepViolations(step) {
  const run = typeof step?.run === "string" ? step.run : "";
  return run
    .split(/\r?\n/u)
    .some(line => /^\s*--wait(?:\s|\\|$)/u.test(line) || /notarytool\s+submit.*\s--wait(?:\s|$)/u.test(line))
    ? ["notarization must poll explicitly instead of using notarytool --wait"]
    : [];
}

function validateIssueWorkflows(workflows, violations) {
  const sagaFile = "saga-issue-link-guard.yml";
  const saga = workflows.get(sagaFile);
  if (!saga) {
    violations.push(`${sagaFile} must exist`);
  } else {
    add(violations, trigger(saga, "pull_request_target") !== undefined, `${sagaFile} must use pull_request_target`);
    // The guard's structural pins (job existence and its permission scope) are rule
    // instances and live in release-claims.json under
    // workflow_policy.structural_pins; structuralPinViolations evaluates them.
    const job = object(object(saga.jobs)["require-closing-issue-link"]);
    for (const fragment of ["codex/", "review/codestory-saga-", "[codex]", "saga:codestory-intelligence"]) {
      add(violations, String(job.if ?? "").includes(fragment), `${sagaFile} guarded condition must include ${fragment}`);
    }
    // The guard's step fragments are rule instances and live in release-claims.json
    // under workflow_policy.step_fragments; stepFragmentViolations evaluates them.
  }

  const closeFile = "close-dev-issues.yml";
  const close = workflows.get(closeFile);
  if (!close) {
    violations.push(`${closeFile} must exist`);
  } else {
    add(violations, includesAll(at(close, "on", "push", "branches"), ["dev/codestory-next"]), `${closeFile} must run on dev/codestory-next pushes`);
    // The close automation's structural pins (job existence and both permission
    // scopes) are rule instances and live in release-claims.json under
    // workflow_policy.structural_pins; structuralPinViolations evaluates them.
    // The close automation's step fragments are rule instances and live in
    // release-claims.json under workflow_policy.step_fragments;
    // stepFragmentViolations evaluates them.
  }
}

function validatePluginAndDraftWorkflows(workflows, violations, graph) {
  const pluginFile = "plugin-static.yml";
  const plugin = workflows.get(pluginFile);
  if (!plugin) {
    violations.push(`${pluginFile} must exist`);
  } else {
    const requiredPaths = [
      "plugins/codestory/**",
      ".github/scripts/check-workflow-policy.mjs",
      ".github/scripts/check-workflow-policy.test.mjs",
      ".github/scripts/cargo-cache-contract.mjs",
      ".github/scripts/cargo-cache-contract.test.mjs",
      ".github/scripts/windows-link-timing.mjs",
      ".github/scripts/windows-link-timing.test.mjs",
      ".github/scripts/install-codestory-marketplace-proof.mjs",
      ".github/scripts/install-codestory-marketplace-proof.test.mjs",
      ".github/scripts/fixtures/workflow-policy-invalid.json",
      ".github/scripts/fixtures/actionlint-invalid.yml",
      ".github/scripts/run-actionlint.mjs",
      ".github/scripts/run-actionlint.test.mjs",
      ".github/actionlint.yaml",
      "release-claims.json",
      "scripts/codestory-release-claims.mjs",
      "scripts/codestory-release-closeout.mjs",
      "scripts/codestory-release-evidence-gate.mjs",
      "scripts/tests/codestory-release-claims.test.mjs",
      "scripts/tests/codestory-release-closeout.test.mjs",
      "scripts/tests/codestory-release-evidence-gate.test.mjs",
      "scripts/tests/fixtures/release-claims/**",
      ".github/scripts/publish-marketplace-catalog.mjs",
      ".github/scripts/publish-marketplace-catalog.test.mjs",
      "benchmarks/release-evidence/**",
      ".github/workflows/**",
      ".github/workflows/release.yml",
      ".github/workflows/packaged-platform-pr.yml",
      ".github/workflows/packaged-platform-proof.yml",
      ".github/workflows/macos-metal-proof.yml",
      ".github/workflows/windows-vulkan-proof.yml",
      ".github/workflows/retrieval-engine-smoke.yml",
      ".github/workflows/source-proof.yml",
      "package.json",
      "package-lock.json",
      "scripts/codex-worktree-setup.*",
      "scripts/install-codestory.ps1",
      "scripts/prepare-embedded-model.mjs",
      "scripts/tests/prepare-embedded-model.test.mjs",
      "scripts/prove-plugin-pinned-provision.mjs",
      "scripts/build-release-manifest.mjs",
      "scripts/lib/wait-for-managed-runtime.mjs",
      "scripts/lib/pinned-archive-digests.mjs",
      "scripts/lib/provision-proof-lanes.mjs",
      "scripts/lib/release-manifest.mjs",
      "scripts/tests/prove-plugin-pinned-provision.test.mjs",
      "crates/codestory-llama-sys/model-contract.json",
      "crates/codestory-llama-sys/build.rs",
      "crates/codestory-llama-sys/model_staging.rs",
      "crates/codestory-llama-sys/Cargo.toml",
      "crates/codestory-llama-sys/tests/model_staging.rs",
      "scripts/release-evidence/**",
      "scripts/release-evidence/serde-json-codestory-project.json",
      "scripts/tests/release-evidence-runner-contract.test.mjs",
    ];
    for (const event of ["pull_request", "push"]) {
      add(violations, includesAll(at(plugin, "on", event, "paths"), requiredPaths), `${pluginFile} ${event} paths must cover policy and release surfaces`);
    }
    add(violations, includesAll(at(plugin, "on", "push", "branches"), ["dev/codestory-next"]), `${pluginFile} must run on dev pushes`);
    // The plugin lane's structural pin (job existence) is a rule instance and
    // lives in release-claims.json under workflow_policy.structural_pins;
    // structuralPinViolations evaluates it. The lane's step fragments - the
    // policy, wiring, proof, and contract suites the surviving job must run -
    // are rule instances and live in release-claims.json under
    // workflow_policy.step_fragments; stepFragmentViolations evaluates them.
  }

  const rustFile = "rust-ci.yml";
  const rust = workflows.get(rustFile);
  if (!rust) {
    violations.push(`${rustFile} must exist`);
  } else {
    for (const violation of draftWorkflowPolicyViolations(rust)) {
      violations.push(`${rustFile} ${violation}`);
    }
    add(violations, trigger(rust, "push") === undefined, `${rustFile} draft checks must not run on push`);
    add(violations, includesAll(at(rust, "on", "pull_request", "paths"), [
      "Cargo.lock",
      "Cargo.toml",
      "crates/**",
      "plugins/codestory/generated-mcp-catalog.json",
      "plugins/codestory/skills/codestory-grounding/**",
      "scripts/generate-codestory-skill-syntax.mjs",
    ]), `${rustFile} must cover workspace source and generated catalog changes`);
    // The draft lane's structural pin (job existence) is a rule instance and
    // lives in release-claims.json under workflow_policy.structural_pins;
    // structuralPinViolations evaluates it.
    const job = object(object(rust.jobs)["linux-draft"]);
    const retrievalWorkflow = workflows.get(retrievalFile);
    for (const violation of retrievalProducerTriggerPolicyViolations(retrievalWorkflow)) {
      violations.push(`${retrievalFile} ${violation}`);
    }
    const retrievalJob = object(at(
      retrievalWorkflow,
      "jobs",
      "linux-contracts",
    ));
    for (const violation of draftSourcePolicyViolations(job, retrievalJob, graph)) {
      violations.push(`${rustFile} ${violation}`);
    }
  }

  const sourceFile = "source-proof.yml";
  const source = workflows.get(sourceFile);
  if (!source) {
    violations.push(`${sourceFile} must exist`);
  } else {
    const promotion = graph.workflow_policy.promotion;
    const sourceConcurrency = [
      "source-proof-",
      promotion.proof_run_sha_expression,
      "-${{ inputs.proof_key || inputs.pr_number || github.ref }}",
    ].join("");
    add(
      violations,
      trigger(source, "pull_request") === undefined,
      `${sourceFile} support PR labels must not trigger broad source proof`,
    );
    add(
      violations,
      at(source, "concurrency", "group") === sourceConcurrency,
      `${sourceFile} concurrency must bind the Actions SHA, proof identity, and exact label`,
    );
    add(
      violations,
      String(at(source, "on", "workflow_dispatch", "inputs", "pr_number", "description") ?? "")
        .includes(promotion.manual_pr_ref_hint),
      `${sourceFile} manual PR input must require ${promotion.manual_pr_ref_hint}`,
    );
    add(violations, trigger(source, "pull_request_target") === undefined, `${sourceFile} must not execute pull-request code through pull_request_target`);
    // The resolve job's structural pin (job existence) is a rule instance and
    // lives in release-claims.json under workflow_policy.structural_pins;
    // structuralPinViolations evaluates it.
    const resolve = object(object(source.jobs).resolve);
    add(
      violations,
      resolve.if === undefined,
      `${sourceFile} resolve job must execute only explicit dispatch and reusable calls`,
    );
    // The resolve job's step fragments - the trusted-head resolution, the
    // exact-head gate reuse, and the executable release freeze - are rule
    // instances and live in release-claims.json under
    // workflow_policy.step_fragments; stepFragmentViolations evaluates them.
    // The full source gate's structural pins (job existence and its needs edge
    // to resolve) are rule instances and live in release-claims.json under
    // workflow_policy.structural_pins; structuralPinViolations evaluates them.
    const full = object(object(source.jobs)["full-source-gate"]);
    add(
      violations,
      full.if === "${{ !inputs.acceptance_only && needs.resolve.outputs.reuse != 'true' }}",
      `${sourceFile} full source gate may skip only a completed exact-head proof`,
    );
    // The retrieval generalization job's structural pin (job existence) is a
    // rule instance and lives in release-claims.json under
    // workflow_policy.structural_pins; structuralPinViolations evaluates it.
    const generalization = object(object(source.jobs)["retrieval-generalization"]);
    add(
      violations,
      hasExactKeys(generalization, [
        "name",
        "needs",
        "if",
        "runs-on",
        "timeout-minutes",
        "steps",
      ]),
      `${sourceFile} retrieval generalization job must keep its exact blocking shape`,
    );
    add(
      violations,
      generalization.name === "retrieval-generalization"
        && sameMembers(needs(generalization), ["resolve"])
        && generalization.if
          === "${{ !inputs.acceptance_only && needs.resolve.outputs.reuse != 'true' }}"
        && generalization["runs-on"] === "ubuntu-latest"
        && generalization["timeout-minutes"] === 5
        && generalization["continue-on-error"] === undefined,
      `${sourceFile} retrieval generalization job must run in parallel on the resolved exact head`,
    );
    const generalizationSteps = list(generalization.steps).map(object);
    add(
      violations,
      generalizationSteps.length === 4,
      `${sourceFile} retrieval generalization job must contain exactly checkout, Node, smoke, and matrix steps`,
    );
    const generalizationCheckout = generalizationSteps[0];
    add(
      violations,
      generalizationCheckout?.uses === "actions/checkout@v5"
        && hasExactKeys(generalizationCheckout, ["uses", "with"])
        && object(generalizationCheckout?.with).ref === "${{ needs.resolve.outputs.ref }}"
        && generalizationCheckout?.["continue-on-error"] === undefined,
      `${sourceFile} retrieval generalization must check out the resolved exact ref`,
    );
    const generalizationNode = generalizationSteps[1];
    add(
      violations,
      generalizationNode?.uses === "actions/setup-node@v5"
        && hasExactKeys(generalizationNode, ["uses", "with"])
        && object(generalizationNode?.with)["node-version"] === "24"
        && object(generalizationNode?.with)["package-manager-cache"] === false
        && generalizationNode?.["continue-on-error"] === undefined,
      `${sourceFile} retrieval generalization must use blocking Node 24 without a package-manager cache`,
    );
    for (const [name, command] of [
      ["Generalization lint (production paths)", "node scripts/lint-retrieval-generalization.mjs"],
      [
        "Generalization lint hostile matrix",
        "node --test scripts/tests/lint-retrieval-generalization.test.mjs",
      ],
    ]) {
      const step = namedStep(generalization, name);
      add(
        violations,
        sameStrings(nonCommentLines(step?.run), [command])
          && hasExactKeys(step, ["name", "run"])
          && step?.["continue-on-error"] === undefined,
        `${sourceFile} retrieval generalization ${name} must run its exact blocking Node command`,
      );
    }
    // The Windows native contracts job's structural pin (job existence) is a
    // rule instance and lives in release-claims.json under
    // workflow_policy.structural_pins; structuralPinViolations evaluates it.
    const windowsNative = object(object(source.jobs)["windows-native-contracts"]);
    add(
      violations,
      hasExactKeys(windowsNative, [
        "name",
        "needs",
        "if",
        "runs-on",
        "timeout-minutes",
        "env",
        "steps",
      ])
        && windowsNative.name === "windows-native-contracts"
        && sameMembers(needs(windowsNative), ["resolve"])
        && windowsNative.if
          === "${{ !inputs.acceptance_only && needs.resolve.outputs.reuse != 'true' }}"
        && windowsNative["runs-on"] === "windows-latest"
        && windowsNative["timeout-minutes"] === 15
        && object(windowsNative.env).CMAKE_GENERATOR === "Ninja"
        && windowsNative["continue-on-error"] === undefined,
      `${sourceFile} Windows native source contracts must run in parallel on the resolved exact head`,
    );
    const windowsNativeSteps = list(windowsNative.steps).map(object);
    add(
      violations,
      windowsNativeSteps.length === 9
        && windowsNativeSteps[0]?.uses === "actions/checkout@v5"
        && object(windowsNativeSteps[0]?.with).ref === "${{ needs.resolve.outputs.ref }}"
        && windowsNativeSteps.every(step => step?.["continue-on-error"] === undefined),
      `${sourceFile} Windows native source contracts must keep the exact blocking nine-step shape`,
    );
    // The qualification harness regression runs before the toolchain steps: it
    // needs no build, and a Windows-native reproduction of the control
    // directory mismatch is worth nothing if it only reports after a Vulkan
    // SDK install and a release Cargo test have already spent the job.
    add(
      violations,
      windowsNativeSteps[1]?.name
        === "Prove the Windows-native qualification harness contracts",
      `${sourceFile} Windows native source contracts must prove the qualification harness before any build step`,
    );
    // The Windows native contracts job's step fragments - the qualification
    // harness, toolchain, short-target, embedded-model, Vulkan SDK, and
    // source-contract steps - are rule instances and live in release-claims.json
    // under workflow_policy.step_fragments; stepFragmentViolations evaluates
    // them.
    const windowsNativeRun = shellLiteralNormalizedText(stepRun(
      windowsNative,
      "Prove Windows path and native-staging source contracts",
    ));
    add(
      violations,
      shellInvocationsContaining(windowsNativeRun, "cargo test").length === 1
        && jobShellInvocationsContaining(windowsNative, "cargo build").length === 0
        && jobShellInvocationsContaining(windowsNative, "cargo check").length === 0,
      `${sourceFile} Windows path and native-staging contracts must share one source-only Cargo invocation`,
    );
    const windowsNativeReceiptName = "Emit authenticated Windows-native source receipt";
    const windowsNativeReceipt = namedStep(windowsNative, windowsNativeReceiptName);
    const windowsNativeReceiptRun = executableRunText(
      stepRun(windowsNative, windowsNativeReceiptName),
    );
    add(
      violations,
      windowsNativeSteps[7]?.name === windowsNativeReceiptName
        && windowsNativeReceipt?.id === "windows-native-source-proof"
        && windowsNativeReceipt?.shell === "pwsh"
        && windowsNativeReceipt?.["continue-on-error"] === undefined
        && hasExactKeys(windowsNativeReceipt, ["name", "id", "shell", "env", "run"]),
      `${sourceFile} Windows-native source receipt must be the blocking terminal evidence producer`,
    );
    requireStepEnv(
      violations,
      sourceFile,
      windowsNative,
      windowsNativeReceiptName,
      { EXPECTED_HEAD_SHA: "${{ needs.resolve.outputs.ref }}" },
    );
    add(
      violations,
      [
        "$commit = (git rev-parse HEAD).Trim()",
        '$sourceTree = (git rev-parse "HEAD^{tree}").Trim()',
        "if ($commit -ne $env:EXPECTED_HEAD_SHA) {",
        "git status --porcelain=v1",
        "if ($LASTEXITCODE -ne 0 -or $dirty.Count -ne 0) {",
        'schema = "codestory.windows-native-source-proof/v1"',
        'status = "pass"',
        "repository = $env:GITHUB_REPOSITORY",
        "commit = $commit",
        "source_tree = $sourceTree",
        'workflow_path = ".github/workflows/source-proof.yml"',
        'job = "windows-native-contracts"',
        "run_id = [Int64]$env:GITHUB_RUN_ID",
        "run_attempt = [Int64]$env:GITHUB_RUN_ATTEMPT",
        "qualification_harness_self_test = $true",
        "control_directory_inflight = $true",
        "windows_path_identity = $true",
        "native_staging = $true",
        '"commit=$commit" | Out-File -FilePath $env:GITHUB_OUTPUT',
      ].every(fragment => windowsNativeReceiptRun.includes(fragment)),
      `${sourceFile} Windows-native source receipt must bind the clean routed head and every proved contract`,
    );
    const windowsNativeUploadName = "Upload authenticated Windows-native source receipt";
    const windowsNativeUpload = namedStep(windowsNative, windowsNativeUploadName);
    add(
      violations,
      windowsNativeSteps[8]?.name === windowsNativeUploadName
        && windowsNativeUpload?.uses === "actions/upload-artifact@v7.0.1"
        && windowsNativeUpload?.["continue-on-error"] === undefined
        && hasExactKeys(windowsNativeUpload, ["name", "uses", "with"])
        && hasExactKeys(object(windowsNativeUpload?.with), [
          "name",
          "path",
          "if-no-files-found",
          "retention-days",
        ])
        && object(windowsNativeUpload?.with).name
          === "windows-native-source-proof-${{ steps.windows-native-source-proof.outputs.commit }}-attempt-${{ github.run_attempt }}"
        && object(windowsNativeUpload?.with).path
          === "target/windows-native-source-proof/windows-native-source-proof.json"
        && object(windowsNativeUpload?.with)["if-no-files-found"] === "error"
        && object(windowsNativeUpload?.with)["retention-days"] === 30,
      `${sourceFile} Windows-native source receipt upload must retain one exact-head attempt-qualified JSON artifact`,
    );
    add(
      violations,
      object(source.env).SCCACHE_VERSION === sccacheVersion
        && object(source.env).SCCACHE_CACHE_SIZE === sccacheCacheSize
        && object(source.env).CARGO_DEPENDENCY_CACHE_MAX_BYTES === "1073741824",
      `${sourceFile} must pin bounded compiler and dependency caches`,
    );
    const sccacheSetup = namedStep(full, "Install pinned sccache");
    add(
      violations,
      sccacheSetup?.uses === sccacheAction
        && object(sccacheSetup?.with).version === "${{ env.SCCACHE_VERSION }}",
      `${sourceFile} must install the pinned sccache action and binary`,
    );
    // The full source gate's step fragments - the bounded cache environment, the
    // cache-bound and reporting steps, the compile, test, lint, and outcome
    // steps, and the source release cell - are rule instances and live in
    // release-claims.json under workflow_policy.step_fragments;
    // stepFragmentViolations evaluates them.
    const identity = namedStep(full, "Capture reusable build cache contract");
    const identityRun = executableRunText(String(identity?.run ?? ""));
    add(
      violations,
      identity?.id === "build-cache"
        && identity?.shell === "bash"
        && identityRun.includes(`--namespace ${promotion.source_cache_namespace}`)
        && identityRun.includes('--exact-sha "$EXACT_SHA"')
        && identityRun.includes('--os "$RUNNER_OS"')
        && identityRun.includes('--target "$target"')
        && identityRun.includes('--rust-version "$rust_version"')
        && identityRun.includes("--features workspace-test-default-and-clippy-all-targets-all-features")
        && identityRun.includes('--native-toolchain "$native_toolchain"')
        && identityRun.includes("--generator unix-makefiles")
        && identityRun.includes('--cmake-version "$cmake_version"')
        && identityRun.includes('--ninja-version "$ninja_version"')
        && identityRun.includes("--lock-file Cargo.lock")
        && identityRun.includes("--cargo-config .cargo/config.toml")
        && identityRun.includes("--sccache-version \"$SCCACHE_VERSION\"")
        && identityRun.includes(".cargo/llama-dynamic-backends.cmake")
        && identityRun.includes("git ls-files '*Cargo.toml'")
        && identityRun.includes("model-contract.json")
        && identityRun.includes("--identity cargo_incremental=0"),
      `${sourceFile} must compute one reusable compiler compatibility contract`,
    );
    const dependencyRestore = namedStep(full, "Restore Cargo dependency inputs");
    const dependencyRestoreWith = object(dependencyRestore?.with);
    add(
      violations,
      dependencyRestore?.uses === "actions/cache/restore@v5"
        && dependencyRestore?.["continue-on-error"] === true
        && sameMembers(cachePaths(dependencyRestore), [
          "${{ runner.temp }}/codestory-source-cargo/registry",
          "${{ runner.temp }}/codestory-source-cargo/git",
        ])
        && dependencyRestoreWith.key === "${{ steps.build-cache.outputs.dependency-key }}"
        && dependencyRestoreWith["restore-keys"] === undefined,
      `${sourceFile} dependency cache must be exact-input-only and exclude compiler output`,
    );
    const compilerRestore = namedStep(full, "Restore compatible compiler objects");
    const compilerRestoreWith = object(compilerRestore?.with);
    add(
      violations,
      compilerRestore?.uses === "actions/cache/restore@v5"
        && compilerRestore?.["continue-on-error"] === true
        && sameMembers(cachePaths(compilerRestore), ["${{ runner.temp }}/codestory-source-sccache"])
        && compilerRestoreWith.key === "${{ steps.build-cache.outputs.compiler-key }}"
        && String(compilerRestoreWith["restore-keys"] ?? "").trim()
          === "${{ steps.build-cache.outputs.compiler-prefix }}",
      `${sourceFile} compiler cache must restore the newest compatible prior candidate`,
    );
    const dependencySave = namedStep(full, "Save Cargo dependency inputs");
    const compilerSave = namedStep(full, "Save compiler objects after compilation");
    add(
      violations,
      dependencySave?.uses === "actions/cache/save@v5"
        && String(dependencySave?.if ?? "").includes("always()")
        && String(dependencySave?.if ?? "").includes("steps.compile-workspace.outcome == 'success'")
        && String(dependencySave?.if ?? "")
          .includes("steps.cargo-dependency-cache-size.outputs.within-limit == 'true'")
        && object(dependencySave?.with).key
          === "${{ steps.cargo-dependency-cache.outputs.cache-primary-key }}"
        && sameMembers(cachePaths(dependencySave), [
          "${{ runner.temp }}/codestory-source-cargo/registry",
          "${{ runner.temp }}/codestory-source-cargo/git",
        ]),
      `${sourceFile} dependency cache must save immediately after successful compilation`,
    );
    add(
      violations,
      compilerSave?.uses === "actions/cache/save@v5"
        && String(compilerSave?.if ?? "").includes("always()")
        && String(compilerSave?.if ?? "").includes("steps.compile-workspace.outcome == 'success'")
        && object(compilerSave?.with).key
          === "${{ steps.compiler-cache-restore.outputs.cache-primary-key }}"
        && sameMembers(cachePaths(compilerSave), ["${{ runner.temp }}/codestory-source-sccache"]),
      `${sourceFile} compiler cache must save a new exact-SHA suffix after successful compilation`,
    );
    add(
      violations,
      cachePathsExcludeExactOutputs(full),
      `${sourceFile} cache paths must exclude Cargo target and exact proof outputs`,
    );
    const compilerSaveIndex = stepIndex(full, "Save compiler objects after compilation");
    add(
      violations,
      compilerSaveIndex > stepIndex(full, "Lint every workspace target and feature once")
        && stepIndex(full, "Stop compilation clock")
          > stepIndex(full, "Lint every workspace target and feature once")
        && stepIndex(full, "Stop compilation clock") < compilerSaveIndex
        && stepIndex(full, "Start compiler cache save clock")
          > stepIndex(full, "Save Cargo dependency inputs")
        && stepIndex(full, "Start compiler cache save clock") < compilerSaveIndex
        && compilerSaveIndex < stepIndex(full, "Test the complete workspace once")
        && compilerSaveIndex < stepIndex(full, "Emit authenticated source release cell"),
      `${sourceFile} compiler cache must save before test execution or release-cell failure`,
    );
    const compile = namedStep(full, "Compile the complete workspace test suite");
    const lint = namedStep(full, "Lint every workspace target and feature once");
    add(
      violations,
      compile?.["continue-on-error"] === true
        && lint?.["continue-on-error"] === true
        && lint?.if === "steps.compile-workspace.outcome == 'success'",
      `${sourceFile} compilation and lint must preserve cache state before reporting failure`,
    );
    requireStepRun(violations, sourceFile, full, "Install pinned cargo-nextest", [
      `cargo-nextest-${nextestVersion}-x86_64-unknown-linux-gnu.tar.gz`,
      `${pinnedProgramRow(graph, "nextest_linux_archive").sha256}  $RUNNER_TEMP/cargo-nextest.tar.gz`,
      "sha256sum --check --strict",
      "cargo nextest --version",
    ]);
    add(
      violations,
      stepIndex(full, "Install pinned cargo-nextest") > stepIndex(full, "Install pinned sccache")
        && stepIndex(full, "Install pinned cargo-nextest")
          < stepIndex(full, "Test the complete workspace once"),
      `${sourceFile} must install the pinned test runner before the workspace test step`,
    );
    requireStepEnv(violations, sourceFile, full, "Emit authenticated source release cell", {
      RESOLVED_REF: "${{ needs.resolve.outputs.ref }}",
    });
    const sourceCellUpload = namedStep(full, "Upload authenticated source release cell");
    add(
      violations,
      sourceCellUpload?.uses === "actions/upload-artifact@v7.0.1"
        && sourceCellUpload?.if === "success()"
        && !scalarStrings(source).some(value => value.includes("emit_release_cells")),
      `${sourceFile} source release cell must be an unconditional success-only retained artifact`,
    );
  }
}

// Both release lanes point the catalog at what they just published, and both do it after the tag
// and the GitHub release already exist. Failing either lane on a credential or a rejected push
// would turn a recoverable delivery gap into an unrecoverable one, so publication is delivery: the
// job absorbs its own failure. That is only honest if the run then SAYS which state it ended in,
// which is what these rules force. Every conjunct below is load-bearing; dropping any one of them
// lets a run that never touched the catalog report that it did.
function catalogDeliveryOutcomeViolations(file, job, delivery) {
  const violations = [];
  const checkout = list(job.steps).find(
    (step) => String(object(step).uses ?? "").startsWith("actions/checkout@"),
  );
  add(
    violations,
    object(object(checkout).with)["fetch-depth"] === 0,
    `${file} marketplace publication must retain history needed to restore the recorded prior SHA`,
  );
  const tokenStep = namedStep(job, "Mint a scoped marketplace token");
  add(
    violations,
    tokenStep?.["continue-on-error"] === true,
    `${file} marketplace token failure must not fail an already-published release`,
  );
  const catalogPush = namedStep(job, "Point the catalog at the published release");
  add(
    violations,
    catalogPush?.["continue-on-error"] === true
      && catalogPush?.if === "steps.token.outcome == 'success'",
    `${file} catalog push must run only with a minted token and must not fail the release`,
  );
  // The step that mints `catalog_published` reads THIS step's outcome, so a push step that does
  // not push would let a run claim a catalog update it never attempted. Turning the gate into
  // delivery replaced the rule that checked this body; it belongs to both lanes, so it lives
  // here rather than in either lane's own rules.
  requireStepRun(violations, file, job, "Point the catalog at the published release", [
    "publish-marketplace-catalog.mjs",
    '--commit "$GITHUB_SHA"',
    '--github-output "$GITHUB_OUTPUT"',
  ]);
  const liveSmoke = namedStep(job, "Smoke the published live catalog");
  add(
    violations,
    liveSmoke?.["continue-on-error"] === true
      && liveSmoke?.if === "steps.publish.outcome == 'success'",
    `${file} live-catalog smoke must be a recorded post-push outcome, not a release gate`,
  );
  requireStepRun(violations, file, job, "Smoke the published live catalog", [
    "install-codestory-marketplace-proof.mjs",
    "--marketplace-source TheGreenCedar/AgentPluginMarketplace",
    '--marketplace-revision "$MARKETPLACE_REVISION"',
    "--local-fixture false",
  ]);
  const restore = namedStep(job, "Restore the previous catalog pin");
  add(
    violations,
    delivery.recovery.automatic_restore === true
      && restore?.["continue-on-error"] === true
      && restore?.if
        === "steps.publish.outcome == 'success' && steps.publish.outputs.catalog_changed == 'true' && steps.live-catalog-smoke.outcome == 'failure'",
    `${file} must automatically restore after a failed smoke without failing the irreversible release`,
  );
  requireStepRun(violations, file, job, "Restore the previous catalog pin", [
    "publish-marketplace-catalog.mjs",
    '--commit "$PREVIOUS_PLUGIN_SHA"',
    '--version "$PREVIOUS_PLUGIN_VERSION"',
    '--expected-current-revision "$PUBLISHED_REVISION"',
    "--expected-current-plugin-sha",
    "--expected-current-plugin-version",
    '--github-output "$GITHUB_OUTPUT"',
  ]);
  add(
    violations,
    object(restore?.env).GH_TOKEN === "${{ steps.token.outputs.token }}"
      && object(restore?.env).PREVIOUS_PLUGIN_SHA
        === "${{ steps.publish.outputs.previous_plugin_sha }}"
      && object(restore?.env).PREVIOUS_PLUGIN_VERSION
        === "${{ steps.publish.outputs.previous_plugin_version }}"
      && object(restore?.env).PUBLISHED_REVISION
        === "${{ steps.publish.outputs.marketplace_revision }}",
    `${file} automatic restore must use the scoped token and the exact pin the push replaced`,
  );
  const deliveryOutcome = namedStep(job, "Record catalog delivery outcome");
  add(
    violations,
    deliveryOutcome?.if === "always()",
    `${file} catalog delivery outcome must be recorded whatever the catalog push did`,
  );
  add(
    violations,
    object(deliveryOutcome?.env).TOKEN_OUTCOME === "${{ steps.token.outcome }}"
      && object(deliveryOutcome?.env).PUBLISH_OUTCOME === "${{ steps.publish.outcome }}"
      && object(deliveryOutcome?.env).LIVE_SMOKE_OUTCOME
        === "${{ steps.live-catalog-smoke.outcome }}"
      && object(deliveryOutcome?.env).RESTORE_OUTCOME === "${{ steps.restore.outcome }}"
      && object(deliveryOutcome?.env).RESTORED_REVISION
        === "${{ steps.restore.outputs.marketplace_revision }}"
      && object(deliveryOutcome?.env).PUBLISHED_REVISION
        === "${{ steps.publish.outputs.marketplace_revision }}",
    `${file} catalog delivery outcome must read the real token, push, and revision results`,
  );
  add(
    violations,
    object(deliveryOutcome?.env).RECOVERY_WORKFLOW === delivery.recovery_workflow,
    `${file} deferred catalog delivery must name ${delivery.recovery_workflow} as the recovery path`,
  );
  requireStepRun(violations, file, job, "Record catalog delivery outcome", [
    "catalog_published=false",
    "catalog_delivery_state=deferred",
    '[ "$TOKEN_OUTCOME" = "success" ]',
    '[ "$PUBLISH_OUTCOME" = "success" ]',
    `printf '%s' "$PUBLISHED_REVISION" | grep -Eq '^[0-9a-f]{40}$'`,
    'echo "catalog_published=$catalog_published"',
    'echo "catalog_delivery_state=$catalog_delivery_state"',
    'echo "marketplace_revision=$marketplace_revision"',
    // The delivery state and the rollback target leave this step through one grouped redirect,
    // so the redirect itself is pinned: the echoes alone would be satisfied by a step that
    // computed the state and wrote it nowhere.
    '} >> "$GITHUB_OUTPUT"',
    "::warning::Catalog publication deferred",
    "recover with $RECOVERY_WORKFLOW",
    // The recovery workflow mints the SAME credential from the SAME environment, so it recovers
    // a rejected push and not a missing credential. A run that defers because the credential is
    // absent must say that, or the ledger records an instruction nobody can follow.
    'if [ "$TOKEN_OUTCOME" != "success" ]; then',
    "provision the marketplace-publish credential",
    '[ "$LIVE_SMOKE_OUTCOME" = "success" ]',
    '[ "$LIVE_SMOKE_OUTCOME" = "failure" ]',
    '[ "$RESTORE_OUTCOME" = "success" ]',
    "catalog_delivery_state=published",
    `catalog_delivery_state=${delivery.recovery.restored_state}`,
    "catalog_delivery_state=unresolved",
    "automatic restore did not complete",
  ]);
  add(
    violations,
    object(job.outputs).catalog_published === "${{ steps.delivery.outputs.catalog_published }}"
      && object(job.outputs).catalog_delivery_state
        === "${{ steps.delivery.outputs.catalog_delivery_state }}"
      && object(job.outputs).marketplace_revision === "${{ steps.delivery.outputs.marketplace_revision }}",
    `${file} marketplace publication must publish the recorded delivery state, not the raw push result`,
  );
  // The rollback target. A catalog move with no recorded predecessor is a move nobody can undo, so
  // every name the graph declares has to leave the push step, be written by the delivery step, and
  // leave the job -- a break anywhere in that chain loses the only record of what to restore.
  for (const name of list(object(delivery.recovery).previous_pin_outputs)) {
    add(
      violations,
      object(job.outputs)[name] === `\${{ steps.delivery.outputs.${name} }}`,
      `${file} marketplace publication must publish the recorded ${name}`,
    );
    add(
      violations,
      executableRunText(String(deliveryOutcome?.run ?? "")).includes(`echo "${name}=`),
      `${file} catalog delivery outcome must record ${name}`,
    );
  }
  add(
    violations,
    object(deliveryOutcome?.env).PREVIOUS_REVISION
        === "${{ steps.publish.outputs.previous_marketplace_revision }}"
      && object(deliveryOutcome?.env).PREVIOUS_PLUGIN_SHA
        === "${{ steps.publish.outputs.previous_plugin_sha }}"
      && object(deliveryOutcome?.env).PREVIOUS_PLUGIN_VERSION
        === "${{ steps.publish.outputs.previous_plugin_version }}",
    `${file} catalog delivery outcome must read the rollback target the catalog move recorded`,
  );
  // A retry would collapse distinguishable failures into one opaque one and could publish on a
  // second attempt after the first was already recorded, so this job gets exactly one attempt.
  for (const step of list(job.steps)) {
    const run = executableRunText(String(object(step).run ?? ""));
    add(
      violations,
      !/\b(?:until|while)\b|for\s+attempt|--retry\b/u.test(run),
      `${file} marketplace publication step ${object(step).name ?? "<unnamed>"} must not retry a recorded delivery outcome`,
    );
  }
  return violations;
}

function validateReleaseCoordinator(workflows, violations, graph) {
  const releaseChain = graph.workflow_policy.release_chain;
  const catalogDelivery = graph.workflow_policy.catalog_delivery;
  const releaseFile = "release.yml";
  const release = workflows.get(releaseFile);
  if (!release) {
    violations.push(`${releaseFile} must exist`);
    return;
  }
  const releaseCallers = [...workflows.entries()]
    .filter(([file, workflow]) => file !== releaseFile
      && scalarStrings(workflow).some(value => value === "./.github/workflows/release.yml"))
    .map(([file]) => file)
    .sort();
  add(
    violations,
    JSON.stringify(releaseCallers) === JSON.stringify(["auto-release.yml"]),
    `${releaseFile} publication authority must have only the trusted auto-release.yml caller`,
  );
  // The coordinator's workflow-scoped permission grants (actions write for
  // superseded-run cancellation, pull-requests read for the exact source
  // resolver) are rule instances and live in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates them
  // with their reviewed violation text.
  const callExpectedHead = object(at(release, "on", "workflow_call", "inputs", "expected_head_sha"));
  add(
    violations,
    callExpectedHead.required === false && callExpectedHead.type === "string" && callExpectedHead.default === "",
    `${releaseFile} workflow_call expected_head_sha must be an optional empty string`,
  );
  const callPublish = object(at(release, "on", "workflow_call", "inputs", "publish_release"));
  add(
    violations,
    callPublish.required === false && callPublish.type === "boolean" && callPublish.default === false,
    `${releaseFile} workflow_call publish_release must be a fail-closed boolean`,
  );
  for (const input of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
    add(
      violations,
      at(release, "on", "workflow_call", "inputs", input) === undefined
        && at(release, "on", "workflow_dispatch", "inputs", input) === undefined,
      `${releaseFile} must not accept calibration bundle inputs; lineage comes from the frozen constant set`,
    );
  }
  const dispatchExpectedHead = object(at(release, "on", "workflow_dispatch", "inputs", "expected_head_sha"));
  add(
    violations,
    dispatchExpectedHead.required === true && dispatchExpectedHead.type === "string",
    `${releaseFile} workflow_dispatch expected_head_sha must be a required string`,
  );
  add(
    violations,
    at(release, "on", "workflow_dispatch", "inputs", "publish_release") === undefined,
    `${releaseFile} workflow_dispatch must not expose publication authority`,
  );
  const releaseLineageStepName = "Verify release-head calibration lineage";
  add(
    violations,
    release.env === undefined && release.defaults === undefined,
    `${releaseFile} release workflow must not override the release-head calibration execution environment`,
  );
  // The workflow-policy job's structural pin (job existence) is a rule
  // instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it. The
  // job's step fragments - the dependency install, the syntax gate, the claim
  // and evidence contract suites, and the policy enforcement commands - are
  // rule instances and live in release-claims.json under
  // workflow_policy.step_fragments; stepFragmentViolations evaluates them.
  const policy = object(object(release.jobs)["workflow-policy"]);
  // The reuse-binding contracts resolve real release commits, which a depth-1
  // clone does not carry: it answered only while the referenced commit happened
  // to be HEAD.
  add(
    violations,
    list(object(policy).steps).some(
      (step) =>
        object(step).uses?.startsWith("actions/checkout")
        && object(object(step).with)["fetch-depth"] === 0,
    ),
    `${releaseFile} workflow-policy must check out full history for the reuse-binding contracts`,
  );

  // The preflight job's structural pin (job existence) is a rule instance and
  // lives in release-claims.json under workflow_policy.structural_pins;
  // structuralPinViolations evaluates it. Its step fragments - the release
  // authority refusal, the calibration lineage command, the changelog and tag
  // gates, the marketplace install proof, and the reusable-evidence resolver -
  // are rule instances and live in release-claims.json under
  // workflow_policy.step_fragments; stepFragmentViolations evaluates them.
  const preflight = object(object(release.jobs).preflight);
  add(
    violations,
    hasExactKeys(
      preflight,
      ["name", "needs", "runs-on", "timeout-minutes", "outputs", "steps"],
    )
      && preflight.name === "Release preflight"
      && preflight["runs-on"] === "ubuntu-latest"
      && preflight["timeout-minutes"] === 10,
    `${releaseFile} preflight must retain the exact trusted job environment`,
  );
  // DISPOSITION: the needs edge stays code because it already consults the
  // graph-owned release_chain dependencies; converting it to a structural
  // needs pin would duplicate that block's data under a second name.
  add(violations, sameMembers(needs(preflight), releaseChain.dependencies.preflight), `${releaseFile} preflight dependencies must match the release claim graph`);
  const releaseLineage = namedStep(preflight, releaseLineageStepName);
  add(
    violations,
    preflight.if === undefined
      && preflight["continue-on-error"] === undefined
      && hasExactKeys(
        releaseLineage,
        ["name", "id", "env", "shell", "working-directory", "run"],
      )
      && releaseLineage?.id === "lineage"
      && hasExactKeys(object(releaseLineage?.env), ["BASH_ENV", "PUBLISH_RELEASE"])
      && object(releaseLineage?.env).BASH_ENV === "/dev/null"
      && object(releaseLineage?.env).PUBLISH_RELEASE === "${{ inputs.publish_release }}"
      && releaseLineage?.shell === "/bin/bash --noprofile --norc -e -o pipefail {0}"
      && releaseLineage?.["working-directory"] === "${{ github.workspace }}",
    `${releaseFile} release-head calibration lineage must be unconditional and fail closed`,
  );
  const preflightCheckout = namedStep(preflight, "Checkout");
  add(
    violations,
    hasExactKeys(preflightCheckout, ["name", "uses", "with"])
      && preflightCheckout?.uses === "actions/checkout@v5"
      && hasExactKeys(object(preflightCheckout?.with), ["fetch-depth"])
      && object(preflightCheckout?.with)["fetch-depth"] === 0,
    `${releaseFile} preflight checkout must retain the exact trusted shape`,
  );
  add(
    violations,
    stepIndex(preflight, "Checkout") === 0
      && stepIndex(preflight, "Cancel superseded proof runs") === 1
      && stepIndex(preflight, releaseLineageStepName) === 2
      && stepIndex(preflight, releaseLineageStepName)
        < stepIndex(preflight, "Validate release authority")
      && stepIndex(preflight, releaseLineageStepName)
        < stepIndex(preflight, "Verify release version"),
    `${releaseFile} release-head calibration lineage must run immediately after checkout and before other release work`,
  );
  const marketplacePreflight = namedStep(
    preflight,
    "Prove the public marketplace install path",
  );
  add(
    violations,
    marketplacePreflight?.if === "inputs.publish_release"
      && marketplacePreflight?.["continue-on-error"] === undefined,
    `${releaseFile} public marketplace preflight must run before publication and fail closed`,
  );
  add(
    violations,
    object(preflight.outputs).marketplace_revision
      === "${{ steps.marketplace.outputs.marketplace_revision }}",
    `${releaseFile} preflight must publish the proved immutable marketplace revision`,
  );

  // The source-proof job's structural pin (job existence) is a rule instance
  // and lives in release-claims.json under workflow_policy.structural_pins;
  // structuralPinViolations evaluates it. The placeholder's refusal step
  // fragments are rule instances and live in release-claims.json under
  // workflow_policy.step_fragments; stepFragmentViolations evaluates them.
  const source = object(object(release.jobs)["source-proof"]);
  add(
    violations,
    source.uses === undefined
      && source["runs-on"] === "ubuntu-latest"
      && source["timeout-minutes"] === 1
      && permissionMapMatches(source.permissions, {})
      && object(source.env).SOURCE_SHA === "${{ github.sha }}",
    `${releaseFile} source proof placeholder must fail closed without calling the broad source workflow`,
  );
  add(violations, sameMembers(needs(source), releaseChain.dependencies["source-proof"]), `${releaseFile} source proof dependencies must match the release claim graph`);
  // Reuse is admissible only through the authenticated closeout binding, never by simply
  // dropping the gate: the job may be skipped, and only when preflight resolved reusable
  // evidence for this exact tree.
  add(
    violations,
    String(source.if ?? "") === "needs.preflight.outputs.source_proof_reused != 'true'",
    `${releaseFile} source proof may be skipped only when preflight resolved reusable evidence`,
  );
  // The resolve-reusable-evidence fragments (both the required resolution
  // conjuncts and the forbidden freeze-status replay) are rule instances and
  // live in release-claims.json under workflow_policy.step_fragments;
  // stepFragmentViolations evaluates them.
  // DISPOSITION: the retired inline calls each re-required the preflight job,
  // so a release.yml without one reported that violation three times. The
  // job-existence rule lives in its structural pin; these two duplicate
  // occurrences stay code so the reviewed verdict multiset is unchanged.
  requireJob(violations, releaseFile, release, "preflight");
  requireJob(violations, releaseFile, release, "preflight");
  // The pre-publish-closeout job's structural pin (job existence) is a rule
  // instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it. The
  // closeout's reuse-binding fragment is a rule instance and lives in
  // release-claims.json under workflow_policy.step_fragments;
  // stepFragmentViolations evaluates it.
  const closeout = object(object(release.jobs)["pre-publish-closeout"]);
  add(
    violations,
    String(closeout.if ?? "").includes("needs.source-proof.result == 'skipped'")
      && String(closeout.if ?? "").includes("needs.preflight.result == 'success'"),
    `${releaseFile} closeout must accept a skipped source gate only alongside a successful preflight`,
  );
  add(
    violations,
    source.with === undefined
      && source.uses === undefined
      && list(source.steps).length === 1,
    `${releaseFile} unreachable source fallback must remain a one-step fail-closed sentinel`,
  );

  // The packaged-proof job's structural pin (job existence) is a rule
  // instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it.
  const packaged = object(object(release.jobs)["packaged-proof"]);
  add(violations, packaged.uses === "./.github/workflows/packaged-platform-proof.yml", `${releaseFile} packaged-proof must call the package workflow`);
  add(violations, sameMembers(needs(packaged), releaseChain.dependencies["packaged-proof"]), `${releaseFile} packaged-proof dependencies must match the release claim graph`);
  add(violations, object(packaged.with).sign_macos === true, `${releaseFile} packaged-proof must sign Mac assets`);
  add(violations, object(packaged.with).emit_release_cells === true, `${releaseFile} packaged-proof must emit all package release cells`);
  add(
    violations,
    object(packaged.with).hermetic_linux === false,
    `${releaseFile} main release must not repeat frozen-candidate Linux qualification`,
  );
  add(
    violations,
    object(packaged.with).scope === "full",
    `${releaseFile} packaged-proof must build the graph-declared release targets`,
  );
  for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
    add(
      violations,
      object(packaged.with)[key] === undefined,
      `${releaseFile} packaged proof must not receive ${key}`,
    );
  }
  const expectedSecrets = [
    "APPLE_DEVELOPER_ID_P12_BASE64",
    "APPLE_DEVELOPER_ID_P12_PASSWORD",
    "APPLE_SIGNING_IDENTITY",
    "APPLE_NOTARY_KEY_P8_BASE64",
    "APPLE_NOTARY_KEY_ID",
    "APPLE_NOTARY_ISSUER_ID",
  ];
  add(violations, sameMembers(Object.keys(object(packaged.secrets)), expectedSecrets), `${releaseFile} packaged-proof must pass exactly the Apple signing secrets`);

  add(
    violations,
    at(release, "jobs", "release-evidence") === undefined,
    `${releaseFile} must not make optional performance or answer-quality evaluation release-blocking`,
  );

  // The macos-metal-proof job's structural pin (job existence) is a rule
  // instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it.
  const metal = object(object(release.jobs)["macos-metal-proof"]);
  add(violations, metal.uses === "./.github/workflows/macos-metal-proof.yml", `${releaseFile} must call protected Metal proof`);
  add(violations, sameMembers(needs(metal), releaseChain.dependencies["macos-metal-proof"]), `${releaseFile} Metal proof dependencies must match the release claim graph`);
  add(violations, object(metal.with).use_packaged_cli_artifact === true, `${releaseFile} Metal proof must use the packaged CLI`);
  add(violations, object(metal.with).emit_release_cells === true, `${releaseFile} Metal proof must emit its authenticated release cell`);
  add(
    violations,
    object(metal.with).candidate_installed_proof === true
      && object(metal.with).candidate_installed_only === undefined
      && object(metal.with).server_behavior_only === true
      && object(metal.with).quality_evidence_artifact === undefined,
    `${releaseFile} Mac proof must close Metal and candidate-installed claims without optional quality evidence`,
  );
  // The release chain's exact --cell-id and post-publish artifact-name
  // fragments for the macOS retrieval-readiness cell are a rule instance and
  // live in release-claims.json under workflow_policy.step_fragments;
  // stepFragmentViolations evaluates them beside the proof workflow's own
  // cell-emission rows.

  // The windows-vulkan-proof job's structural pin (job existence) is a rule
  // instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it.
  const vulkan = object(object(release.jobs)["windows-vulkan-proof"]);
  add(violations, vulkan.uses === "./.github/workflows/windows-vulkan-proof.yml", `${releaseFile} must call protected Vulkan proof`);
  add(violations, sameMembers(needs(vulkan), releaseChain.dependencies["windows-vulkan-proof"]), `${releaseFile} Vulkan proof dependencies must match the release claim graph`);
  add(violations, object(vulkan.with).use_packaged_cli_artifact === true, `${releaseFile} Vulkan proof must use the packaged CLI`);
  add(violations, object(vulkan.with).emit_release_cells === true, `${releaseFile} Vulkan proof must emit its authenticated release cell`);
  add(
    violations,
    object(vulkan.with).candidate_installed_proof === true
      && object(vulkan.with).candidate_installed_only === undefined
      && object(vulkan.with).server_behavior_only === true
      && object(vulkan.with).quality_evidence_artifact === undefined,
    `${releaseFile} Windows proof must close Vulkan and candidate-installed claims without optional quality evidence`,
  );
  // The release chain's exact --cell-id and post-publish artifact-name
  // fragments for the Windows retrieval-readiness cell are a rule instance
  // and live in release-claims.json under workflow_policy.step_fragments;
  // stepFragmentViolations evaluates them beside the proof workflow's own
  // cell-emission rows.
  // The self-hosted Windows service account runs under the default Restricted execution
  // policy, so a run step left on the default shell dies before executing anything:
  // every step must pick bash or a powershell invocation that bypasses the policy.
  for (const step of at(workflows.get("windows-vulkan-proof.yml"), "jobs", "packaged-vulkan", "steps") ?? []) {
    if (typeof object(step).run !== "string") continue;
    const shell = object(step).shell;
    add(
      violations,
      typeof shell === "string" && (shell === "bash" || shell.includes("-ExecutionPolicy Bypass")),
      `windows-vulkan-proof.yml step ${object(step).name ?? "<unnamed>"} must declare the bypass shell for the locked-down service account`,
    );
  }

  // The linux-vulkan-proof job's structural pin (job existence) is a rule
  // instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it.
  const linuxVulkan = object(object(release.jobs)["linux-vulkan-proof"]);
  add(violations, linuxVulkan.uses === "./.github/workflows/linux-vulkan-proof.yml", `${releaseFile} must call protected Linux Vulkan proof`);
  add(violations, sameMembers(needs(linuxVulkan), releaseChain.dependencies["linux-vulkan-proof"]), `${releaseFile} Linux Vulkan proof dependencies must match the release claim graph`);
  add(
    violations,
    object(linuxVulkan.with).candidate_installed_proof === true
      && object(linuxVulkan.with).server_behavior_only === true
      && object(linuxVulkan.with).emit_release_cells === true,
    `${releaseFile} Linux proof must close Vulkan, retrieval, and candidate-installed claims`,
  );
  // The release chain's exact --cell-id fragments for the three Linux cells
  // are a rule instance and live in release-claims.json under
  // workflow_policy.step_fragments; stepFragmentViolations evaluates them
  // beside the proof workflow's own cell-emission rows.

  // DISPOSITION: the retired inline code required the pre-publish-closeout
  // job twice. The job-existence rule lives in its structural pin; this
  // duplicate occurrence stays code so the reviewed verdict multiset is
  // unchanged. The closeout's step fragments - provenance collection, the
  // cell download chain, the evaluation, and the dev-head revalidation - are
  // rule instances and live in release-claims.json under
  // workflow_policy.step_fragments; stepFragmentViolations evaluates them.
  requireJob(violations, releaseFile, release, "pre-publish-closeout");
  const preCloseout = object(object(release.jobs)["pre-publish-closeout"]);
  add(violations, sameMembers(needs(preCloseout), releaseChain.dependencies["pre-publish-closeout"]), `${releaseFile} pre-publish closeout dependencies must match the release claim graph`);
  const preDownload = namedStep(
    preCloseout,
    "Download and verify selected pre-publish release cells",
  );
  add(
    violations,
    preDownload?.uses === undefined && preDownload?.shell === "bash",
    `${releaseFile} pre-publish closeout must materialize exact Actions artifact ids in blocking shell`,
  );
  add(
    violations,
    !list(preCloseout.steps).some(
      (step) => object(step).uses === "actions/download-artifact@v8.0.1"
        && object(step).with["artifact-ids"] !== undefined,
    ),
    `${releaseFile} pre-publish closeout must not filter mixed-run artifact ids through one Actions run`,
  );
  // The version the ledger is filed under reaches the evaluator as a variable, so the command text
  // alone no longer says which release it closed out.
  // DISPOSITION: step env bindings and step uses pins have no graph evaluator,
  // so these predicates stay code.
  requireStepEnv(violations, releaseFile, preCloseout, "Evaluate authenticated pre-publish closeout", {
    RELEASE_VERSION: "${{ needs.preflight.outputs.version }}",
  });
  const devRevalidation = namedStep(preCloseout, "Revalidate proof-only dev head");
  add(
    violations,
    devRevalidation?.if === "${{ !inputs.publish_release }}",
    `${releaseFile} proof-only ledger upload must revalidate only the dev head`,
  );
  requireStepUses(violations, releaseFile, preCloseout, "Upload accepted pre-publish closeout", "actions/upload-artifact@v7.0.1");

  // The publish job's structural pin (job existence) is a rule instance and
  // lives in release-claims.json under workflow_policy.structural_pins;
  // structuralPinViolations evaluates it. Its step fragments - the ledger-fed
  // release notes, the manifest generation, the shipped closeout summary, the
  // tag refusal, and the release creation - are rule instances and live in
  // release-claims.json under workflow_policy.step_fragments;
  // stepFragmentViolations evaluates them.
  const publish = object(object(release.jobs).publish);
  add(violations, publish.if === "inputs.publish_release", `${releaseFile} publish must require trusted publication authority`);
  add(violations, sameMembers(needs(publish), releaseChain.dependencies.publish), `${releaseFile} publish dependencies must match the release claim graph`);
  // The published platform table is a claim about this release, so it is rendered from the accepted
  // ledger. Rendering it from the static graph is how a release whose accelerator proof was
  // withheld still announced that accelerator as supported.
  requireStepUses(
    violations,
    releaseFile,
    publish,
    "Download the accepted pre-publish closeout",
    "actions/download-artifact@v8.0.1",
  );
  // The manifest generation has to happen after the checksums exist -- it
  // cross-checks against them -- and before the release is created, or the
  // launcher fetches a manifest that describes bytes nobody published; the
  // ordering pins below stay code.
  requireStepEnv(violations, releaseFile, publish, "Generate the release manifest", {
    TAG: "${{ needs.preflight.outputs.tag }}",
    VERSION: "${{ needs.preflight.outputs.version }}",
  });
  add(
    violations,
    stepIndex(publish, "Combine and verify checksums") < stepIndex(publish, "Generate the release manifest")
      && stepIndex(publish, "Generate the release manifest") < stepIndex(publish, "Create GitHub release"),
    `${releaseFile} must generate the release manifest from verified checksums before creating the release`,
  );
  const publishRun = shellLiteralNormalizedText(stepRun(
    publish,
    "Create GitHub release",
  ));
  add(
    violations,
    publishRun.includes("gh release create $TAG ${assets[@]}")
      && publishRun.includes("find target/release-assets -maxdepth 1 -type f")
      && !publishRun.includes("qualification-driver")
      && !publishRun.includes("codestory_embedding_qualification"),
    `${releaseFile} must publish only graph-declared root assets and exclude the private qualification driver`,
  );
  add(violations, !scalarStrings(release).some(value => value.includes("--generate-notes")), `${releaseFile} must use curated release notes`);

  // The marketplace-publish job's structural pin (job existence) is a rule
  // instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it.
  const marketplacePublish = object(object(release.jobs)["marketplace-publish"]);
  add(
    violations,
    marketplacePublish.if === "inputs.publish_release",
    `${releaseFile} marketplace publication must require trusted publication authority`,
  );
  add(
    violations,
    sameMembers(needs(marketplacePublish), releaseChain.dependencies["marketplace-publish"]),
    `${releaseFile} marketplace publication dependencies must match the release claim graph`,
  );
  add(
    violations,
    marketplacePublish.environment === "marketplace-publish",
    `${releaseFile} marketplace publication must hold its cross-repository credential in its own environment`,
  );
  add(
    violations,
    marketplacePublish.permissions === undefined,
    `${releaseFile} marketplace publication must not hold repository write permission`,
  );
  // The credential is minted per run and scoped to the one external repository; it must never
  // exist in a job that also runs release code.
  const tokenStep = namedStep(marketplacePublish, "Mint a scoped marketplace token");
  add(
    violations,
    String(tokenStep?.uses ?? "").startsWith("actions/create-github-app-token@")
      && fullSha.test(String(tokenStep?.uses ?? "").split("@")[1] ?? "")
      && object(tokenStep?.with).owner === "TheGreenCedar"
      && object(tokenStep?.with).repositories === "AgentPluginMarketplace",
    `${releaseFile} marketplace token must be a SHA-pinned app token scoped to the marketplace repository`,
  );
  violations.push(...catalogDeliveryOutcomeViolations(releaseFile, marketplacePublish, catalogDelivery));
  // The version the catalog is pointed at reaches the push as a variable, so the command text no
  // longer says which release it published. Both halves are pinned, as in the plugin lane: the
  // run fragment lives in release-claims.json under workflow_policy.step_fragments and the env
  // binding stays code because env bindings have no graph evaluator.
  requireStepEnv(violations, releaseFile, marketplacePublish, "Point the catalog at the published release", {
    RELEASE_VERSION: "${{ needs.preflight.outputs.version }}",
  });

  // The post-publish-smoke job's structural pin (job existence) is a rule
  // instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it.
  const post = object(object(release.jobs)["post-publish-smoke"]);
  add(violations, post.uses === "./.github/workflows/post-publish-release-smoke.yml", `${releaseFile} must call post-publish smoke`);
  add(violations, sameMembers(needs(post), releaseChain.dependencies["post-publish-smoke"]), `${releaseFile} post-publish dependencies must match the release claim graph`);
  // The smoke still needs publication authority and a real published release, but a deferred
  // catalog must not suppress proof of the assets that were actually published.
  const postIf = String(post.if ?? "");
  add(
    violations,
    postIf.includes("always()")
      && postIf.includes("inputs.publish_release")
      && postIf.includes("needs.preflight.result == 'success'")
      && postIf.includes("needs.publish.result == 'success'"),
    `${releaseFile} post-publish smoke must require trusted publication authority and a successful publish`,
  );
  // Not `.result` alone: `needs.marketplace-publish.outputs.catalog_published == 'true'` in the
  // condition would reinstate exactly the hard catalog gate this change removed, under a
  // different spelling. Nothing about the catalog job may appear in the condition at all; the
  // delivery state reaches the smoke through `with:`, where it is data rather than a gate.
  add(
    violations,
    !postIf.includes(`needs.${catalogDelivery.publish_job}`),
    `${releaseFile} post-publish smoke must not gate on ${catalogDelivery.publish_job} in any form`,
  );
  // THE anti-vacuity rule: the catalog claim may only ever be the recorded delivery state. A
  // literal, an unrelated input, or any other expression would let a release assert a catalog
  // update that never happened.
  add(
    violations,
    object(post.with).catalog_published
      === `\${{ needs.${catalogDelivery.publish_job}.outputs.catalog_published == 'true' }}`
      && object(post.with).catalog_delivery_state
        === `\${{ needs.${catalogDelivery.publish_job}.outputs.catalog_delivery_state }}`,
    `${releaseFile} post-publish smoke must derive catalog_published from the recorded ${catalogDelivery.publish_job} outcome`,
  );
  add(
    violations,
    object(post.with).emit_release_cells === true
      && object(post.with).marketplace_revision
        === `\${{ needs.${catalogDelivery.publish_job}.outputs.marketplace_revision }}`
      && String(object(post.with).pre_publish_closeout_artifact ?? "").startsWith("release-closeout-pre-publish-"),
    `${releaseFile} post-publish smoke must consume the proved marketplace revision and accepted pre-publish ledger`,
  );
  // The post-publish-closeout job's structural pin (job existence) is a rule
  // instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it. Its
  // step fragments - provenance collection, the container digest checks, and
  // the evaluation - are rule instances and live in release-claims.json under
  // workflow_policy.step_fragments; stepFragmentViolations evaluates them.
  const postCloseout = object(object(release.jobs)["post-publish-closeout"]);
  add(violations, postCloseout.if === "inputs.publish_release", `${releaseFile} post-publish closeout must require trusted publication authority`);
  add(violations, sameMembers(needs(postCloseout), releaseChain.dependencies["post-publish-closeout"]), `${releaseFile} post-publish closeout dependencies must match the release claim graph`);
  // The closeout reached marketplace-publish only through the smoke, so removing the smoke's gate
  // removed the closeout's too. Keep it that way rather than leaving it to be reintroduced here.
  add(
    violations,
    !needs(postCloseout).includes(catalogDelivery.publish_job)
      && !String(postCloseout.if ?? "").includes(`needs.${catalogDelivery.publish_job}`),
    `${releaseFile} post-publish closeout must not gate on ${catalogDelivery.publish_job} succeeding`,
  );
  const postProvenanceName = "Authenticate post-publish Actions provenance";
  const postProvenanceRun = executableRunText(stepRun(postCloseout, postProvenanceName));
  requireStepEnv(violations, releaseFile, postCloseout, postProvenanceName, {
    GH_TOKEN: "${{ github.token }}",
    REUSE_SELECTION: "${{ needs.preflight.outputs.reuse }}",
  });
  requireFlagOnInvocation(
    violations,
    `${releaseFile} post-publish producer map must consume the preflight reuse selection`,
    postProvenanceRun,
    "codestory-release-cell-manifest.mjs producer-map",
    '--reuse "$REUSE_SELECTION"',
  );
  const postDownloadName = "Download and verify selected cumulative release cells";
  const postDownload = namedStep(postCloseout, postDownloadName);
  const postDownloadRun = executableRunText(stepRun(postCloseout, postDownloadName));
  const postDigestCheck = 'test "$actual_digest" = "$expected_digest"';
  const postDestinationCreate = 'mkdir "$destination"';
  const postContainerExtract = 'unzip -q "$archive" -d "$destination"';
  const postDirectoryCheck = 'test -d "target/release-cell-manifests/$artifact_name"';
  add(
    violations,
    postDownload?.uses === undefined
      && postDownload?.shell === "bash"
      && postDownload?.["continue-on-error"] === undefined
      && object(postDownload?.env).GH_TOKEN === "${{ github.token }}",
    `${releaseFile} post-publish closeout must materialize cumulative release cells in blocking Bash`,
  );
  add(
    violations,
    postDownloadRun.includes(
      ".artifacts[] | [.id, .name, .digest] | @tsv",
    ),
    `${releaseFile} post-publish closeout must iterate every selected artifact id, name, and digest`,
  );
  add(
    violations,
    postDownloadRun.includes(
      "$GITHUB_API_URL/repos/$GITHUB_REPOSITORY/actions/artifacts/$artifact_id/zip",
    ),
    `${releaseFile} post-publish closeout must fetch every selected artifact by authenticated artifact id`,
  );
  add(
    violations,
    postDownloadRun.includes('sha256sum "$archive"')
      && postDownloadRun.includes(postDigestCheck),
    `${releaseFile} post-publish closeout must reject a selected artifact container digest mismatch`,
  );
  add(
    violations,
    postDownloadRun.includes(
      'destination="target/release-cell-manifests/$artifact_name"',
    )
      && postDownloadRun.includes(postDestinationCreate),
    `${releaseFile} post-publish closeout must create one exact artifact-name directory per selected container`,
  );
  add(
    violations,
    postDownloadRun.includes(postContainerExtract)
      && postDownloadRun.indexOf(postDigestCheck)
        < postDownloadRun.indexOf(postDestinationCreate)
      && postDownloadRun.indexOf(postDestinationCreate)
        < postDownloadRun.indexOf(postContainerExtract),
    `${releaseFile} post-publish closeout must extract each verified container into its exact artifact-name directory`,
  );
  add(
    violations,
    postDownloadRun.includes(".artifacts[].name")
      && postDownloadRun.includes(postDirectoryCheck)
      && postDownloadRun.indexOf(postContainerExtract)
        < postDownloadRun.indexOf(postDirectoryCheck),
    `${releaseFile} post-publish closeout must verify every selected artifact-name directory exists`,
  );
  requireStepEnv(violations, releaseFile, postCloseout, "Evaluate authenticated post-publish closeout", {
    RELEASE_VERSION: "${{ needs.preflight.outputs.version }}",
  });
  requireStepUses(violations, releaseFile, postCloseout, "Upload accepted post-publish closeout", "actions/upload-artifact@v7.0.1");
  for (const [jobName, job] of [
    ["Metal proof", metal],
    ["Windows Vulkan proof", vulkan],
    ["Linux Vulkan proof", linuxVulkan],
    ["post-publish proof", post],
  ]) {
    for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
      add(
        violations,
        object(job.with)[key] === undefined,
        `${releaseFile} ${jobName} must not receive ${key}`,
      );
    }
  }
}

function expectedPackageRows(graph) {
  return graph.workflow_policy.package_matrix;
}

function expectedPostPublishRows() {
  return [
    {
      asset_target: "windows-x64",
      runs_on: '["self-hosted","Windows","X64","codestory-vulkan"]',
      environment: "windows-vulkan-proof",
      backend: "Vulkan",
      extension: "zip",
    },
    {
      asset_target: "macos-arm64",
      runs_on: '["self-hosted","macOS","ARM64","codestory-metal"]',
      environment: "macos-metal-release",
      backend: "Metal",
      extension: "tar.gz",
    },
    {
      asset_target: "linux-x64",
      runs_on: '["self-hosted","Linux","X64","codestory-linux-vulkan"]',
      environment: "linux-vulkan-proof",
      backend: "Vulkan",
      extension: "tar.gz",
    },
  ];
}

// The asset targets the default (full) scope actually builds. A step gated on
// matrix.asset_target is only reachable while its target survives here.
function packageMatrixAssetTargets(expression) {
  const match = typeof expression === "string" && expression.match(
    /\|\| '([^']+)'\) \}\}$/u,
  );
  if (!match) return [];
  try {
    return list(object(JSON.parse(match[1])).include)
      .map(row => object(row).asset_target)
      .filter(target => typeof target === "string");
  } catch {
    return [];
  }
}

function validatePackageMatrixExpression(violations, expression, graph) {
  const match = typeof expression === "string" && expression.match(
    /fromJSON\(inputs\.scope == 'linux' && '([^']+)' \|\| inputs\.scope == 'windows' && '([^']+)' \|\| inputs\.scope == 'macos' && '([^']+)' \|\| '([^']+)'\)/u,
  );
  if (!match) {
    violations.push("packaged-platform-proof.yml matrix must select structural JSON by scope");
    return;
  }
  const linuxX64 = graph.workflow_policy.package_matrix.find(({ asset_target: target }) =>
    target === "linux-x64");
  const windowsX64 = graph.workflow_policy.package_matrix.find(({ asset_target: target }) =>
    target === "windows-x64");
  const macosArm64 = graph.workflow_policy.package_matrix.find(({ asset_target: target }) =>
    target === "macos-arm64");
  const expected = [
    { include: [linuxX64] },
    { include: [windowsX64] },
    { include: [macosArm64] },
    { include: expectedPackageRows(graph) },
  ];
  try {
    match.slice(1).forEach((json, index) => {
      add(violations, JSON.stringify(JSON.parse(json)) === JSON.stringify(expected[index]), "packaged-platform-proof.yml package matrix scope changed");
    });
  } catch {
    violations.push("packaged-platform-proof.yml package matrix must contain valid JSON");
  }
}

function validatePackagedProof(workflows, violations, graph) {
  const file = "packaged-platform-proof.yml";
  const workflow = workflows.get(file);
  if (!workflow) {
    violations.push(`${file} must exist`);
    return;
  }
  add(violations, trigger(workflow, "workflow_call") !== undefined, `${file} must be reusable`);
  const refInput = object(at(workflow, "on", "workflow_call", "inputs", "ref"));
  add(
    violations,
    refInput.required === true && refInput.type === "string" && refInput.default === undefined,
    `${file} must require one exact source SHA`,
  );
  add(violations, object(workflow.permissions).contents === "read", `${file} must use read-only contents permission`);
  add(violations, object(workflow.permissions).actions === "read", `${file} must read authenticated qualification artifacts`);
  for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
    requireOptionalStringInput(violations, file, workflow, "workflow_call", key);
  }
  for (const key of [
    "calibration_mode",
    "quality_evidence_artifact",
    "candidate_installed_proof",
    "candidate_installed_only",
    "server_behavior_only",
    "enforce_calibration_freeze_lineage",
  ]) {
    add(
      violations,
      at(workflow, "on", "workflow_call", "inputs", key) === undefined,
      `${file} package-only workflow must not define ${key}`,
    );
  }
  add(
    violations,
    object(workflow.env).LINUX_GLIBC_BUILD_IMAGE ===
      "rust:1.95.0-bullseye@sha256:28afaeb8445f2a2e7d878bd34ed39ba02bb517efb29986188cbd59b7cf4f2fdf",
    `${file} must pin the glibc build image`,
  );
  add(
    violations,
    object(workflow.env).LINUX_GLSLC_IMAGE ===
      "ubuntu:24.04@sha256:4fbb8e6a8395de5a7550b33509421a2bafbc0aab6c06ba2cef9ebffbc7092d90",
    `${file} must pin the glslc build image`,
  );
  const job = requireJob(violations, file, workflow, "build");
  add(
    violations,
    job["timeout-minutes"] === "${{ inputs.sign_macos && startsWith(matrix.asset_target, 'macos-') && 90 || 60 }}",
    `${file} package build timeout must cover only signed macOS packaging`,
  );
  add(
    violations,
    at(workflow, "concurrency", "group") === "packaged-platform-proof-${{ github.sha }}-${{ inputs.ref }}-${{ inputs.proof_key || github.ref }}",
    `${file} concurrency must bind caller SHA, exact package SHA, and proof identity`,
  );
  validatePackageMatrixExpression(violations, at(job, "strategy", "matrix"), graph);
  add(violations, String(job.environment ?? "").includes("macos-release-signing"), `${file} signed Mac cells must use the protected signing environment`);
  const packageSteps = list(job.steps).map(object);
  add(
    violations,
    object(workflow.env).SCCACHE_VERSION === sccacheVersion
      && object(workflow.env).SCCACHE_CACHE_SIZE === sccacheCacheSize
      && object(workflow.env).WINDOWS_SCCACHE_CACHE_SIZE === windowsSccacheCacheSize
      && object(workflow.env).CARGO_DEPENDENCY_CACHE_MAX_BYTES === "1073741824",
    `${file} must pin bounded compiler and dependency caches`,
  );
  const hermeticInput = object(at(workflow, "on", "workflow_call", "inputs", "hermetic_linux"));
  add(
    violations,
    hermeticInput.required === false
      && hermeticInput.default === false
      && hermeticInput.type === "boolean",
    `${file} frozen Linux qualification must be explicit and off by default`,
  );
  const qualificationDriverInput = object(at(
    workflow,
    "on",
    "workflow_call",
    "inputs",
    "include_qualification_driver",
  ));
  add(
    violations,
    qualificationDriverInput.required === false
      && qualificationDriverInput.default === false
      && qualificationDriverInput.type === "boolean",
    `${file} private qualification-driver retention must be explicit and off by default`,
  );
  const shortWindowsTarget = namedStep(job, "Configure short Windows Cargo target");
  const checkout = namedStep(job, "Checkout");
  add(
    violations,
    checkout?.uses === "actions/checkout@v5"
      && object(checkout?.with).ref === "${{ inputs.ref }}",
    `${file} package jobs must checkout only the requested exact SHA`,
  );
  requireStepRun(violations, file, job, "Require exact source identity", [
    '[[ "$EXACT_SHA" =~ ^[0-9a-f]{40}$ ]]',
    'head_sha="$(git rev-parse HEAD)"',
    "source_tree=\"$(git rev-parse 'HEAD^{tree}')\"",
    'test "$head_sha" = "$EXACT_SHA"',
  ]);
  add(
    violations,
    shortWindowsTarget?.if === "runner.os == 'Windows'" && shortWindowsTarget?.shell === "pwsh",
    `${file} short Cargo target must be Windows-only PowerShell setup`,
  );
  const sccacheSetup = namedStep(job, "Install pinned sccache");
  add(
    violations,
    sccacheSetup?.uses === sccacheAction
      && object(sccacheSetup?.with).version === "${{ env.SCCACHE_VERSION }}",
    `${file} must install the pinned sccache action and binary`,
  );
  const sccacheIdentity = namedStep(job, "Capture pinned sccache identity");
  add(
    violations,
    sccacheIdentity?.id === "sccache-identity"
      && sccacheIdentity?.shell === "bash"
      && sccacheIdentity?.env === undefined
      && sccacheIdentity?.["continue-on-error"] === undefined
      && stepIndex(job, "Capture pinned sccache identity")
        === stepIndex(job, "Install pinned sccache") + 1,
    `${file} must capture the pinned sccache identity immediately after installation`,
  );
  requireStepRun(violations, file, job, "Capture pinned sccache identity", [
    'sccache_path="$(command -v sccache)"',
    'if [[ "$RUNNER_OS" == "Windows" && "$sccache_path" != *.[eE][xX][eE] ]]',
    'sccache_path="${sccache_path}.exe"',
    'test -f "$sccache_path"',
    'test -x "$sccache_path"',
    'readFileSync(process.argv[1])',
    "' \"$sccache_path\"",
    'echo "path=$sccache_path"',
    'echo "sha256=$sccache_sha256"',
  ]);
  requireStepRun(violations, file, job, "Configure short Windows Cargo target", [
    '$workspaceTarget = Join-Path $env:GITHUB_WORKSPACE "target"',
    '$runnerRoot = [System.IO.Path]::GetPathRoot($workspaceTarget)',
    '[string]::IsNullOrWhiteSpace($runnerRoot)',
    'Join-Path $runnerRoot "t"',
    "New-Item -ItemType Junction -Path $shortTarget -Target $workspaceTarget",
    '"CARGO_TARGET_DIR=$shortTarget" | Out-File -FilePath $env:GITHUB_ENV',
  ]);
  requireStepRun(violations, file, job, "Configure bounded compiler cache", [
    'cache_size="$SCCACHE_CACHE_SIZE"',
    'if [[ "$RUNNER_OS" == "Windows" ]]',
    'cache_size="$WINDOWS_SCCACHE_CACHE_SIZE"',
    "CARGO_HOME=$RUNNER_TEMP/codestory-release-cargo",
    "SCCACHE_DIR=$RUNNER_TEMP/codestory-release-sccache",
    "SCCACHE_CACHE_SIZE=$cache_size",
    "RUSTC_WRAPPER=sccache",
    "CARGO_INCREMENTAL=0",
    "CMAKE_C_COMPILER_LAUNCHER=sccache",
    "CMAKE_CXX_COMPILER_LAUNCHER=sccache",
    "CMAKE_GENERATOR=Ninja",
  ]);
  const nativeIdentity = namedStep(job, "Capture reusable build cache contract");
  const nativeIdentityRun = executableRunText(String(nativeIdentity?.run ?? ""));
  add(
    violations,
    nativeIdentity?.id === "build-cache"
      && nativeIdentity?.shell === "bash"
      && object(nativeIdentity?.env).CALIBRATION_MODE === undefined
      && object(nativeIdentity?.env).QUALITY_EVIDENCE_ARTIFACT === undefined
      && nativeIdentityRun.includes(`--namespace ${graph.workflow_policy.promotion.packaged_cache_namespace}`)
      && nativeIdentityRun.includes('--exact-sha "$EXACT_SHA"')
      && nativeIdentityRun.includes('--os "$RUNNER_OS"')
      && nativeIdentityRun.includes('--target "${{ matrix.rust_target }}"')
      && nativeIdentityRun.includes('--rust-version "$rust_version"')
      && nativeIdentityRun.includes("--features codestory-cli-default-features")
      && nativeIdentityRun.includes("--native-toolchain")
      && nativeIdentityRun.includes("--generator")
      && nativeIdentityRun.includes("--cmake-version")
      && nativeIdentityRun.includes("--ninja-version")
      && nativeIdentityRun.includes("--sccache-version \"$SCCACHE_VERSION\"")
      && nativeIdentityRun.includes("--lock-file Cargo.lock")
      && nativeIdentityRun.includes("--cargo-config .cargo/config.toml")
      && nativeIdentityRun.includes("--identity cargo_incremental=0")
      && object(nativeIdentity?.env).INCLUDE_QUALIFICATION_DRIVER
        === "${{ inputs.include_qualification_driver }}"
      && nativeIdentityRun.includes(
        '--identity "qualification_driver=$INCLUDE_QUALIFICATION_DRIVER"',
      )
      && nativeIdentityRun.includes(".cargo/llama-dynamic-backends.cmake")
      && nativeIdentityRun.includes("git ls-files '*Cargo.toml'")
      && nativeIdentityRun.includes("model-contract.json")
      && nativeIdentityRun.includes("install-windows-vulkan-sdk.ps1")
      && nativeIdentityRun.includes("linux-glibc-build.Dockerfile")
      && nativeIdentityRun.includes(".github/docker/glslc")
      && nativeIdentityRun.includes("--identity cxxflags=-std=c++17")
      && nativeIdentityRun.includes("LINUX_GLIBC_BUILD_IMAGE")
      && nativeIdentityRun.includes("LINUX_GLSLC_IMAGE"),
    `${file} must compute one complete reusable compiler compatibility contract`,
  );
  const dependencyRestore = namedStep(job, "Restore Cargo dependency inputs");
  const dependencyRestoreWith = object(dependencyRestore?.with);
  add(
    violations,
    dependencyRestore?.uses === "actions/cache/restore@v5"
      && sameMembers(cachePaths(dependencyRestore), [
        "${{ runner.temp }}/codestory-release-cargo/registry",
        "${{ runner.temp }}/codestory-release-cargo/git",
      ])
      && dependencyRestoreWith.key === "${{ steps.build-cache.outputs.dependency-key }}"
      && dependencyRestoreWith["restore-keys"] === undefined,
    `${file} dependency cache must be exact-input-only and exclude compiler output`,
  );
  const compilerRestore = namedStep(job, "Restore compatible compiler objects");
  const compilerRestoreWith = object(compilerRestore?.with);
  add(
    violations,
    compilerRestore?.uses === "actions/cache/restore@v5"
      && sameMembers(cachePaths(compilerRestore), ["${{ runner.temp }}/codestory-release-sccache"])
      && compilerRestoreWith.key === "${{ steps.build-cache.outputs.compiler-key }}"
      && String(compilerRestoreWith["restore-keys"] ?? "").trim()
        === "${{ steps.build-cache.outputs.compiler-prefix }}",
    `${file} compiler cache must restore the newest compatible prior candidate`,
  );
  const dependencySave = namedStep(job, "Save Cargo dependency inputs");
  const compilerSave = namedStep(job, "Save compiler objects after compilation");
  requireStepRun(violations, file, job, "Bound Cargo dependency cache", [
    "--max-bytes \"$CARGO_DEPENDENCY_CACHE_MAX_BYTES\"",
    "--path \"$CARGO_HOME/registry\"",
    "--path \"$CARGO_HOME/git\"",
  ]);
  add(
    violations,
    dependencySave?.uses === "actions/cache/save@v5"
      && String(dependencySave?.if ?? "").includes("always()")
      && String(dependencySave?.if ?? "").includes("steps.linux-build.outcome == 'success'")
      && String(dependencySave?.if ?? "").includes("steps.package-build.outcome == 'success'")
      && String(dependencySave?.if ?? "")
        .includes("steps.cargo-dependency-cache-size.outputs.within-limit == 'true'")
      && object(dependencySave?.with).key
        === "${{ steps.cargo-dependency-cache.outputs.cache-primary-key }}"
      && sameMembers(cachePaths(dependencySave), [
        "${{ runner.temp }}/codestory-release-cargo/registry",
        "${{ runner.temp }}/codestory-release-cargo/git",
      ]),
    `${file} dependency cache must save immediately after successful compilation`,
  );
  add(
    violations,
    compilerSave?.uses === "actions/cache/save@v5"
      && String(compilerSave?.if ?? "").includes("always()")
      && String(compilerSave?.if ?? "").includes("steps.linux-build.outcome == 'success'")
      && String(compilerSave?.if ?? "").includes("steps.package-build.outcome == 'success'")
      && object(compilerSave?.with).key
        === "${{ steps.compiler-cache-restore.outputs.cache-primary-key }}"
      && sameMembers(cachePaths(compilerSave), ["${{ runner.temp }}/codestory-release-sccache"]),
    `${file} compiler cache must save a new exact-SHA suffix after successful compilation`,
  );
  add(
    violations,
    cachePathsExcludeExactOutputs(job),
    `${file} cache paths must exclude Cargo target, native seeds, models, proofs, and exact archives`,
  );
  const compilerSaveIndex = stepIndex(job, "Save compiler objects after compilation");
  for (const lateStep of [
    "Prove production feature identity",
    "Prove production feature identity on Windows",
    "Sign and notarize macOS CLI",
    "Package release asset",
    "Package release asset on Windows",
    "Smoke packaged release asset",
    "Smoke packaged release asset on Windows",
    "Upload release asset",
  ]) {
    add(
      violations,
      compilerSaveIndex >= 0 && compilerSaveIndex < stepIndex(job, lateStep),
      `${file} compiler cache must save before late ${lateStep} failure`,
    );
  }
  requireStepRun(violations, file, job, "Report compiler cache restore", [
    "--requested-key",
    "--matched-key",
    "--compatibility-prefix",
    "--cache-hit",
    "--path \"$SCCACHE_DIR\"",
  ]);
  requireStepRun(violations, file, job, "Report compiler cache save", [
    "--restored-bytes",
    "--started-ms",
    "--ended-ms",
    "--save-started-ms",
    "--save-result",
    "--path \"$SCCACHE_DIR\"",
  ]);
  add(
    violations,
    namedStep(job, "Compile immutable native staging regression on Windows") === undefined
      && namedStep(job, "Compile native workspace path regression on Windows") === undefined,
    `${file} Windows package proof must not compile either regression in a second Cargo invocation`,
  );
  const packageBuild = namedStep(job, "Build package and qualification driver");
  const linuxBuild = namedStep(job, "Build Linux x64 at the glibc 2.31 baseline");
  const expectedSccacheIdentityEnv = {
    SCCACHE_BINARY: "${{ steps.sccache-identity.outputs.path }}",
    SCCACHE_SHA256: "${{ steps.sccache-identity.outputs.sha256 }}",
  };
  requireStepRun(violations, file, job, "Build Linux x64 at the glibc 2.31 baseline", [
    'test -x "$SCCACHE_BINARY"',
    'test "$actual_sccache_sha256" = "$SCCACHE_SHA256"',
    "RUSTC_WRAPPER=/sccache/sccache",
    "SCCACHE_DIR=/sccache/cache",
    "CMAKE_C_COMPILER_LAUNCHER=/sccache/sccache",
    "CMAKE_CXX_COMPILER_LAUNCHER=/sccache/sccache",
    "$SCCACHE_BINARY:/sccache/sccache:ro",
    "$SCCACHE_DIR:/sccache/cache",
  ]);
  const expectedLinuxBuildEnv = {
    ...expectedSccacheIdentityEnv,
    INCLUDE_QUALIFICATION_DRIVER: "${{ inputs.include_qualification_driver }}",
    RELEASE_RUST_TARGET: "${{ matrix.rust_target }}",
  };
  add(
    violations,
    linuxBuild?.if === "matrix.asset_target == 'linux-x64'"
      && linuxBuild?.shell === "bash"
      && hasExactKeys(object(linuxBuild?.env), Object.keys(expectedLinuxBuildEnv))
      && Object.entries(expectedLinuxBuildEnv).every(
        ([key, value]) => object(linuxBuild?.env)[key] === value,
      )
      && linuxBuild?.["continue-on-error"] === undefined,
    `${file} Linux container must strictly report and stop its owned compiler server`,
  );
  const stopCompilationClock = namedStep(job, "Stop compilation clock");
  add(
    violations,
    stopCompilationClock?.id === "compile-clock-stop"
      && stopCompilationClock?.shell === "bash"
      && stopCompilationClock?.env === undefined
      && stopCompilationClock?.["continue-on-error"] === undefined,
    `${file} compiler clock stop must remain a strict telemetry-only boundary`,
  );
  const finalizeCompilerObjects = namedStep(job, "Finalize compiler objects");
  add(
    violations,
    String(finalizeCompilerObjects?.if ?? "").trim()
      === "always() && steps.package-build.outcome == 'success'"
      && finalizeCompilerObjects?.shell === "bash"
      && hasExactKeys(
        object(finalizeCompilerObjects?.env),
        Object.keys(expectedSccacheIdentityEnv),
      )
      && Object.entries(expectedSccacheIdentityEnv).every(
        ([key, value]) => object(finalizeCompilerObjects?.env)[key] === value,
      )
      && finalizeCompilerObjects?.["continue-on-error"] === undefined
      && packageBuild?.if === "matrix.asset_target != 'linux-x64'",
    `${file} host finalizer must strictly stop only the host package-build compiler server`,
  );
  add(
    violations,
    stepIndex(job, "Stop compilation clock")
      === stepIndex(job, "Build Linux x64 at the glibc 2.31 baseline") + 1
      && stepIndex(job, "Finalize compiler objects")
        === stepIndex(job, "Stop compilation clock") + 1,
    `${file} compiler owner build, clock stop, and finalizer must remain adjacent`,
  );
  add(
    violations,
    compilerSaveIndex > stepIndex(job, "Build package and qualification driver")
      && compilerSaveIndex > stepIndex(job, "Build Linux x64 at the glibc 2.31 baseline"),
    `${file} compiler cache must save after every selected compilation step`,
  );
  add(
    violations,
    stepIndex(job, "Build pinned Linux toolchain image")
      < stepIndex(job, "Start compilation clock")
      && stepIndex(job, "Stop compilation clock")
        > stepIndex(job, "Build package and qualification driver")
      && stepIndex(job, "Stop compilation clock") < compilerSaveIndex
      && stepIndex(job, "Start compiler cache save clock")
        > stepIndex(job, "Save Cargo dependency inputs")
      && stepIndex(job, "Start compiler cache save clock") < compilerSaveIndex,
    `${file} compile and compiler-cache-save timings must cover only their named stages`,
  );
  const linuxBuildDockerfile = fs.readFileSync(
    path.join(repositoryRoot, ".github", "docker", "linux-glibc-build.Dockerfile"),
    "utf8",
  );
  for (const fragment of [
    "clang-13=1:13.0.1-6~deb11u1",
    "libclang-13-dev=1:13.0.1-6~deb11u1",
    "CC=clang-13",
    "CXX=clang++-13",
    "LIBCLANG_PATH=/usr/lib/llvm-13/lib",
    "-mavxvnni -mavx512bf16 -mamx-tile -mamx-int8",
  ]) {
    add(
      violations,
      linuxBuildDockerfile.includes(fragment),
      `${file} Bullseye native build must preserve compiler contract ${fragment}`,
    );
  }
  add(
    violations,
    packageBuild?.shell === "bash"
      && hasExactKeys(object(packageBuild?.env), [
        "INCLUDE_QUALIFICATION_DRIVER",
        "RELEASE_RUST_TARGET",
        "SOURCE_SHA",
        "SOURCE_TREE",
      ])
      && object(packageBuild?.env).INCLUDE_QUALIFICATION_DRIVER
        === "${{ inputs.include_qualification_driver }}"
      && object(packageBuild?.env).RELEASE_RUST_TARGET
        === "${{ matrix.rust_target }}"
      && object(packageBuild?.env).SOURCE_SHA
        === "${{ steps.source-identity.outputs.sha }}"
      && object(packageBuild?.env).SOURCE_TREE
        === "${{ steps.source-identity.outputs.tree }}",
    `${file} native package build must not override the selected generator and must bind the reviewed target and qualification workload`,
  );
  requireStepRun(violations, file, job, "Smoke codestory-cli on Windows", [
    '$bin = "$env:WINDOWS_CLI"',
    "throw \"Windows CLI version smoke failed",
    "throw \"Windows CLI help smoke failed",
  ]);
  const windowsCliSmoke = namedStep(job, "Smoke codestory-cli on Windows");
  add(
    violations,
    windowsCliSmoke?.if === "runner.os == 'Windows'"
      && windowsCliSmoke?.shell === "pwsh"
      && hasExactKeys(object(windowsCliSmoke?.env), ["WINDOWS_CLI"])
      && object(windowsCliSmoke?.env).WINDOWS_CLI
        === "${{ steps.package-build.outputs.cli }}"
      && windowsCliSmoke?.["continue-on-error"] === undefined,
    `${file} Windows CLI smoke must execute only the exact release binary selected from Cargo output`,
  );
  requireStepRun(violations, file, job, "Package release asset on Windows", [
    "cargo-build-artifacts.mjs verify",
    '--manifest "$env:ARTIFACT_MANIFEST"',
    '--source-sha "$env:SOURCE_SHA"',
    '--source-tree "$env:SOURCE_TREE"',
    '--rust-target "$env:RELEASE_RUST_TARGET"',
    '$bin = "$env:WINDOWS_CLI"',
    "package-codestory-release.py",
    "--binary $bin",
  ]);
  const windowsPackage = namedStep(job, "Package release asset on Windows");
  add(
    violations,
    windowsPackage?.if === "runner.os == 'Windows'"
      && windowsPackage?.shell === "pwsh"
      && hasExactKeys(object(windowsPackage?.env), [
        "ARTIFACT_MANIFEST",
        "INPUT_VERSION",
        "RELEASE_RUST_TARGET",
        "SOURCE_SHA",
        "SOURCE_TREE",
        "WINDOWS_CLI",
      ])
      && object(windowsPackage?.env).ARTIFACT_MANIFEST
        === "${{ steps.package-build.outputs.manifest }}"
      && object(windowsPackage?.env).WINDOWS_CLI
        === "${{ steps.package-build.outputs.cli }}"
      && object(windowsPackage?.env).SOURCE_SHA
        === "${{ steps.source-identity.outputs.sha }}"
      && object(windowsPackage?.env).SOURCE_TREE
        === "${{ steps.source-identity.outputs.tree }}"
      && windowsPackage?.["continue-on-error"] === undefined,
    `${file} Windows packaging must verify and package only the exact Cargo-selected release binary`,
  );
  requireStepRun(violations, file, job, "Prepare checksum-pinned embedded model", [
    "node scripts/prepare-embedded-model.mjs",
  ]);
  requireStepRun(violations, file, job, "Install Linux Vulkan build dependencies", [
    "bash .github/scripts/install-linux-vulkan-build-deps.sh",
  ]);
  const productFeatureProbe = namedStep(job, "Prove production feature identity");
  add(
    violations,
    productFeatureProbe?.if === "runner.os != 'Windows'"
      && productFeatureProbe?.shell === "bash"
      && object(productFeatureProbe?.env).CODESTORY_EMBED_ALLOW_CPU === "0"
      && productFeatureProbe?.["continue-on-error"] === undefined,
    `${file} Unix packages must fail closed on a non-product embedding feature identity`,
  );
  requireStepRun(violations, file, job, "Prove production feature identity", [
    "retrieval status",
    '--cache-dir "$cache"',
    '.embedding_device_observation_source == "per_user_server"',
    "Production feature identity probe:",
  ]);
  const windowsProductFeatureProbe = namedStep(
    job,
    "Prove production feature identity on Windows",
  );
  add(
    violations,
    windowsProductFeatureProbe?.if === "runner.os == 'Windows'"
      && windowsProductFeatureProbe?.shell === "pwsh"
      && object(windowsProductFeatureProbe?.env).CODESTORY_EMBED_ALLOW_CPU === "0"
      && object(windowsProductFeatureProbe?.env).WINDOWS_CLI
        === "${{ steps.package-build.outputs.cli }}"
      && windowsProductFeatureProbe?.["continue-on-error"] === undefined,
    `${file} Windows package must execute the exact selected CLI for its product feature probe`,
  );
  requireStepRun(
    violations,
    file,
    job,
    "Prove production feature identity on Windows",
    [
      'retrieval status',
      '--cache-dir "$cache"',
      '$status.embedding_device_observation_source -ne "per_user_server"',
      "non-product embedding observation source",
      "Production feature identity probe:",
    ],
  );
  requireStepRun(violations, file, job, "Build pinned Linux toolchain image", [
    ".github/docker/linux-glibc-build.Dockerfile",
    "LINUX_GLIBC_BUILD_IMAGE",
    "LINUX_GLSLC_IMAGE",
  ]);
  const packageBuildRun = shellLiteralNormalizedText(stepRun(
    job,
    "Build package and qualification driver",
  ));
  const linuxBuildRun = shellLiteralNormalizedText(stepRun(
    job,
    "Build Linux x64 at the glibc 2.31 baseline",
  ));
  const cargoBuildStepNames = list(job?.steps)
    .map(object)
    .filter(step =>
      shellInvocationsContaining(
        shellLiteralNormalizedText(step.run),
        "cargo build",
      ).length > 0)
    .map(step => step.name)
    .sort();
  add(
    violations,
    shellInvocationsContaining(packageBuildRun, "cargo build").length === 1
      && packageBuildRun.includes("cargo build --release --locked")
      && packageBuildRun.includes("-p codestory-cli")
      && packageBuildRun.includes("--bin codestory-cli")
      && packageBuildRun.includes("--bin codestory-cli-runtime")
      && packageBuildRun.includes("if [ $INCLUDE_QUALIFICATION_DRIVER = true ]")
      && packageBuildRun.includes("-p codestory-bench")
      && packageBuildRun.includes("--bin codestory_embedding_qualification")
      && packageBuildRun.includes("--target $RELEASE_RUST_TARGET")
      && packageBuildRun.includes("if [ $RUNNER_OS = Windows ]")
      && packageBuildRun.includes("--message-format=json-render-diagnostics")
      && packageBuildRun.includes("--timings")
      && packageBuildRun.includes("cargo-build-artifacts.mjs select")
      && packageBuildRun.includes("cargo-build-artifacts.mjs features")
      && occurrenceCount(packageBuildRun, "--workspace-root $GITHUB_WORKSPACE") === 2
      && packageBuildRun.includes("--source-sha $SOURCE_SHA")
      && packageBuildRun.includes("--source-tree $SOURCE_TREE")
      && occurrenceCount(packageBuildRun, "build_package_graph") === 3
      && !packageBuildRun.includes("codestory_embedding_constant_calibration")
      && !packageBuildRun.includes("target/debug")
      && !/(?:^|\s)--test(?:s)?(?:\s|$)/u.test(packageBuildRun)
      && !/(?:^|\s)--bins(?:\s|$)/u.test(packageBuildRun),
    `${file} host package must build only the production bins and optional qualification driver in one exact Cargo invocation`,
  );
  // Windows linker timing was a substring count over the build log, which the
  // Cargo progress line `Compiling time v0.3.47` satisfied. The reported
  // `msvc_link` interval must instead come from the explicit link /TIME
  // boundaries in a captured trace the shell closed before reading it, and it
  // stays a separate interval from the `cargo_graph` wall clock around it.
  const linkTimingSelector = shellInvocationsContaining(
    packageBuildRun,
    "node .github/scripts/windows-link-timing.mjs select",
  );
  add(
    violations,
    linkTimingSelector.length === 1
      && linkTimingSelector[0].includes("--input $linker_log")
      && linkTimingSelector[0].includes("--out $timing_dir/windows-link-timing.json")
      && linkTimingSelector[0].includes("--build-elapsed-ms $build_elapsed_ms")
      && packageBuildRun.includes("2> $linker_log")
      && !packageBuildRun.includes(">(tee $linker_log")
      && !packageBuildRun.includes("linker_rows")
      && shellInvocationsContaining(packageBuildRun, "grep").length === 0
      && packageBuildRun.includes("- Exact release graph build (cargo_graph): ${build_elapsed_ms} ms"),
    `${file} Windows linker timing must be selected from the explicit link boundary, not a substring search`,
  );
  // The claim graph publishes what a Windows package may say about linking. It
  // is only a mirror if it matches the selector this workflow actually runs.
  const declaredLinkTiming = object(
    object(graph.workflow_policy.windows_package_graph).link_timing,
  );
  add(
    violations,
    declaredLinkTiming.phase === WINDOWS_LINK_PHASE
      && declaredLinkTiming.selector === ".github/scripts/windows-link-timing.mjs"
      && declaredLinkTiming.record_schema === WINDOWS_LINK_TIMING_SCHEMA
      && declaredLinkTiming.record_file === "windows-link-timing.json"
      && declaredLinkTiming.evidence === "explicit_link_time_boundary"
      && declaredLinkTiming.substring_match === false
      && declaredLinkTiming.observational === true
      && JSON.stringify(list(declaredLinkTiming.unavailable_reasons))
        === JSON.stringify(Object.values(WINDOWS_LINK_TIMING_REASONS).sort()),
    `${file} declared Windows linker timing must mirror the selector the workflow runs`,
  );
  add(
    violations,
    JSON.stringify(cargoBuildStepNames) === JSON.stringify([
      "Build Linux x64 at the glibc 2.31 baseline",
      "Build package and qualification driver",
    ])
      && jobShellInvocationsContaining(job, "cargo test").length === 0
      && jobShellInvocationsContaining(job, "cargo check").length === 0
      && jobShellInvocationsContaining(job, "rustc ").length === 1
      && jobShellInvocationsContaining(job, "rustc ")[0].includes("rustc -Vv"),
    `${file} package proof must not compile outside the two mutually exclusive reviewed Cargo build steps`,
  );
  add(
    violations,
    shellInvocationsContaining(linuxBuildRun, "cargo build").length === 1
      && linuxBuildRun.includes("CARGO_TARGET_DIR=/workspace/target/glibc-2.31")
      && linuxBuildRun.includes("CXXFLAGS=-std=c++17")
      && linuxBuildRun.includes("INCLUDE_QUALIFICATION_DRIVER=$INCLUDE_QUALIFICATION_DRIVER")
      && linuxBuildRun.includes("RELEASE_RUST_TARGET=$RELEASE_RUST_TARGET")
      && linuxBuildRun.includes("-p codestory-cli")
      && linuxBuildRun.includes("--bin codestory-cli")
      && linuxBuildRun.includes("--bin codestory-cli-runtime")
      && linuxBuildRun.includes("if [ $INCLUDE_QUALIFICATION_DRIVER = true ]")
      && linuxBuildRun.includes("-p codestory-bench")
      && linuxBuildRun.includes("--bin codestory_embedding_qualification")
      && linuxBuildRun.includes("--target $RELEASE_RUST_TARGET")
      && linuxBuildRun.includes("--message-format=json-render-diagnostics")
      && linuxBuildRun.includes("cargo-build-artifacts.mjs features")
      && linuxBuildRun.includes("--workspace-root $GITHUB_WORKSPACE")
      && !linuxBuildRun.includes("codestory_embedding_constant_calibration")
      && !/(?:^|\s)--bins(?:\s|$)/u.test(linuxBuildRun),
    `${file} Linux package must build CLI, runtime, and conditional qualification driver in one exact Cargo invocation`,
  );
  // The identity the smoke reads is the one `source-identity` proved against the dispatched ref,
  // and it now arrives through `env:` rather than spliced into the command. Both halves are pinned:
  // the script names the variable, and the variable names that step's output.
  const sourceIdentityBindings = {
    SOURCE_SHA: "${{ steps.source-identity.outputs.sha }}",
    SOURCE_TREE: "${{ steps.source-identity.outputs.tree }}",
  };
  for (const [smokeStep, sha, tree] of [
    ["Smoke packaged release asset", '"$SOURCE_SHA"', '"$SOURCE_TREE"'],
    ["Smoke packaged release asset on Windows", '"$env:SOURCE_SHA"', '"$env:SOURCE_TREE"'],
  ]) {
    requireStepRun(violations, file, job, smokeStep, [
      `--expected-source-sha ${sha}`,
      `--expected-source-tree ${tree}`,
    ]);
    requireStepEnv(violations, file, job, smokeStep, sourceIdentityBindings);
  }
  const driverStageName = "Stage qualification driver in package proof artifact";
  const driverStage = namedStep(job, driverStageName);
  const driverStageRun = shellLiteralNormalizedText(stepRun(job, driverStageName));
  add(
    violations,
    driverStage?.if === "inputs.include_qualification_driver"
      && driverStage?.shell === "bash"
      && driverStage?.["continue-on-error"] === undefined
      && hasExactKeys(object(driverStage?.env), [
        "INPUT_VERSION",
        "SOURCE_SHA",
        "SOURCE_TREE",
      ])
      && object(driverStage?.env).INPUT_VERSION === "${{ inputs.version }}"
      && object(driverStage?.env).SOURCE_SHA
        === "${{ steps.source-identity.outputs.sha }}"
      && object(driverStage?.env).SOURCE_TREE
        === "${{ steps.source-identity.outputs.tree }}"
      && shellInvocationsContaining(
        driverStageRun,
        "node .github/scripts/qualification-driver-artifact.mjs produce",
      ).length === 1
      && driverStageRun.includes("--asset-target ${{ matrix.asset_target }}")
      && driverStageRun.includes("--source-sha $SOURCE_SHA")
      && driverStageRun.includes("--source-tree $SOURCE_TREE")
      && driverStageRun.includes("--version $INPUT_VERSION")
      && driverStageRun.includes(
        "--archive target/release-dist/codestory-cli-v${INPUT_VERSION}-${{ matrix.asset_target }}.${{ matrix.extension }}",
      )
      && driverStageRun.includes("--trusted-root $GITHUB_WORKSPACE")
      && driverStageRun.includes("--target-dir target")
      && driverStageRun.includes(
        "--out-dir target/release-dist/qualification-driver/${{ matrix.asset_target }}",
      ),
    `${file} must retain one archive-bound private qualification driver beside each selected package`,
  );
  add(
    violations,
    stepIndex(job, driverStageName) > stepIndex(job, "Package release asset")
      && stepIndex(job, driverStageName)
        > stepIndex(job, "Package release asset on Windows")
      && stepIndex(job, driverStageName) < stepIndex(job, "Upload release asset"),
    `${file} must bind the private qualification driver after archive creation and before artifact upload`,
  );
  add(
    violations,
    [
      "Package release asset",
      "Package release asset on Windows",
      "Sign and notarize macOS CLI",
    ].every(stepName => {
      const run = shellLiteralNormalizedText(stepRun(job, stepName));
      return !run.includes("qualification-driver")
        && !run.includes("codestory_embedding_qualification");
    }),
    `${file} public archives and signing inputs must exclude the private qualification driver`,
  );
  requireStepRun(violations, file, job, "Report fresh package identity", [
    "archive_sha256=",
    "Source SHA: \\`$SOURCE_SHA\\`",
    "Source tree: \\`$SOURCE_TREE\\`",
    "Archive SHA-256:",
  ]);
  requireStepEnv(violations, file, job, "Report fresh package identity", sourceIdentityBindings);
  add(
    violations,
    stepIndex(job, "Report fresh package identity")
      > stepIndex(job, "Smoke packaged release asset")
      && stepIndex(job, "Report fresh package identity")
        > stepIndex(job, "Smoke packaged release asset on Windows")
      && stepIndex(job, "Report fresh package identity")
        < stepIndex(job, "Upload release asset"),
    `${file} must report a verified fresh archive identity before upload`,
  );
  add(
    violations,
    namedStep(job, "Prove fresh-target Node-absent network-denied Cargo release boundary") === undefined,
    `${file} matrix package jobs must not repeat the frozen Linux Cargo boundary`,
  );
  const frozenLinux = requireJob(violations, file, workflow, "frozen-linux-qualification");
  add(
    violations,
    frozenLinux.if === "inputs.hermetic_linux"
      && sameMembers(needs(frozenLinux), ["build"])
      && frozenLinux["runs-on"] === "ubuntu-latest",
    `${file} frozen Linux Cargo boundary must be one explicit post-package job`,
  );
  const frozenCheckout = namedStep(frozenLinux, "Checkout frozen candidate");
  add(
    violations,
    frozenCheckout?.uses === "actions/checkout@v5"
      && object(frozenCheckout?.with).ref === "${{ inputs.ref }}",
    `${file} frozen Linux qualification must checkout the exact candidate`,
  );
  requireStepRun(
    violations,
    file,
    frozenLinux,
    "Prove fresh-target Node-absent network-denied Cargo release boundary",
    [
      "CARGO_HOME=\"$proof_root/cargo\"",
      "cargo fetch --locked",
      "--network none",
      "--read-only",
      "command -v node",
      "test ! -e \"$CARGO_TARGET_DIR\"",
      "cargo check --release --locked --offline -p codestory-llama-sys",
      "cargo build --release --locked --offline -p codestory-llama-sys",
    ],
  );
  add(
    violations,
    cacheSteps(frozenLinux).length === 0,
    `${file} frozen Linux fresh-target qualification must not restore compiler output`,
  );
  const signing = namedStep(job, "Sign and notarize macOS CLI");
  add(violations, signing !== undefined, `${file} must sign and notarize Mac binaries`);
  violations.push(...notaryStepViolations(signing).map(message => `${file} ${message}`));
  const signingRun = executableRunText(String(signing?.run ?? ""));
  for (const fragment of [
    "umask 077",
    "chmod 600",
    "--options runtime",
    "--timestamp",
    "xcrun notarytool submit",
    "--no-wait",
    "xcrun notarytool info",
    "xcrun notarytool log",
    'jq -e \'.status == "Accepted"\'',
    "TeamIdentifier=${APPLE_DEVELOPER_TEAM_ID}",
    "certificate leaf",
  ]) {
    add(violations, signingRun.includes(fragment), `${file} signing step must include ${fragment}`);
  }
  violations.push(...macosCliDistributionViolations(
    signing,
    namedStep(job, "Execute quarantined notarized macOS CLI without signing credentials"),
    '"$work_dir/codestory-cli-quarantined"',
  ).map(message => `${file} ${message}`));
  requireStepRun(violations, file, job, "Run Windows installer ownership self-test", ["scripts/install-codestory.ps1 -SelfTest"]);
  const linuxBaseline = namedStep(job, "Prove Linux x64 glibc 2.31 baseline");
  requireStepRun(violations, file, job, "Prove Linux x64 glibc 2.31 baseline", [
    "bash .github/scripts/check-linux-glibc-baseline.sh",
  ]);
  add(
    violations,
    !executableRunText(String(linuxBaseline?.run ?? "")).includes("libvulkan"),
    `${file} Linux glibc baseline must not install a Vulkan loader`,
  );
  add(
    violations,
    namedStep(job, "Build qualification driver") === undefined
      && namedStep(job, "Packaged per-user server calibration or qualification") === undefined
      && namedStep(job, "Upload hosted Linux calibration runs") === undefined
      && namedStep(job, "Upload hosted Linux calibration failure evidence") === undefined
      && namedStep(job, "Upload packaged agent proof artifacts") === undefined,
    `${file} package workflow must not add a second driver build, calibration, or hosted qualification`,
  );
  requireCalibrationProducerAuthentication(violations, file, job);
  requireCalibrationProducerBoundary(
    violations,
    file,
    job,
    "matrix.asset_target == 'linux-x64' && inputs.calibration_bundle_artifact != ''",
  );
  // The live guard. --version-only stops before the runtime proof, so it needs
  // no release evidence and no qualification driver, but it still loads and
  // verifies the authenticated calibration bundle -- and without the
  // enforcement flag a version-only proof rejects calibration inputs outright,
  // so removing the flag breaks this step loudly instead of disabling the
  // guard. `check-packaged-agent-proof.py --self-test` proves both directions.
  const lineageStepName = "Prove frozen calibration source lineage";
  const lineageProof = namedStep(job, lineageStepName);
  requireStepRun(violations, file, job, lineageStepName, [
    'test "$(jq -r .status crates/codestory-llama-sys/per-user-embedding-server-constant-set.json)" = frozen',
    "calibration-bundle.json",
    '--calibration-bundle "$calibration_bundle"',
    "--calibration-producer-run-id",
    "--calibration-producer-artifact",
    "--proof-tier hosted_package",
    "--version-only",
    '--expected-source-sha "$SOURCE_SHA"',
    '--expected-source-tree "$SOURCE_TREE"',
    "--enforce-calibration-freeze-lineage",
  ]);
  requireFlagOnInvocation(
    violations,
    `${file} ${lineageStepName} must pass --enforce-calibration-freeze-lineage on the invocation that reads the calibration bundle`,
    stepRun(job, lineageStepName),
    "--calibration-bundle",
    "--enforce-calibration-freeze-lineage",
  );
  add(
    violations,
    lineageProof?.shell === "bash"
      && object(lineageProof?.env).SOURCE_SHA === "${{ steps.source-identity.outputs.sha }}"
      && object(lineageProof?.env).SOURCE_TREE === "${{ steps.source-identity.outputs.tree }}"
      && object(lineageProof?.env).CALIBRATION_ARTIFACT
        === "${{ inputs.calibration_bundle_artifact }}"
      && object(lineageProof?.env).CALIBRATION_RUN_ID
        === "${{ inputs.calibration_bundle_run_id }}",
    `${file} ${lineageStepName} must bind the verified source identity and the authenticated producer`,
  );
  add(
    violations,
    object(namedStep(job, "Checkout")?.with)["fetch-depth"] === 0,
    `${file} package build must keep full history for the calibration freeze lineage probe`,
  );
  // Reachability, not presence. A flag on a step no caller can reach is the
  // vacuous guard this check exists to prevent, so evaluate the step's own
  // condition against the bindings the frozen-candidate coordinator passes and
  // require some real dispatch of it to run the step.
  const coordinatorFile = "packaged-platform-pr.yml";
  const coordinator = workflows.get(coordinatorFile);
  const coordinatorPackaged = object(at(coordinator, "jobs", "packaged-proof"));
  const bindings = callerInputBindings(
    coordinatorPackaged,
    calleeInputSpecifications(workflow),
  );
  const fullScopeTargets = packageMatrixAssetTargets(
    at(workflow, "jobs", "build", "strategy", "matrix"),
  );
  for (const [stepName, stepValue] of [
    [lineageStepName, lineageProof],
    ["Authenticate calibration bundle producer", namedStep(job, "Authenticate calibration bundle producer")],
    ["Download frozen calibration bundle", namedStep(job, "Download frozen calibration bundle")],
  ]) {
    const reachability = conditionIsSatisfiable(
      String(stepValue?.if ?? "false"),
      bindings,
      { "matrix.asset_target": fullScopeTargets },
    );
    add(
      violations,
      reachability.satisfiable,
      `${file} step ${stepName} must be reachable from a ${coordinatorFile} frozen-candidate dispatch: ${reachability.reason}`,
    );
  }
  add(
    violations,
    bindings.get("calibration_bundle_artifact")?.fixed === false
      && bindings.get("calibration_bundle_run_id")?.fixed === false,
    `${coordinatorFile} packaged proof must forward the dispatched calibration bundle identity so the freeze lineage guard can run`,
  );
  add(
    violations,
    !scalarStrings(workflow).some(value =>
      value.includes("candidate_installed")
      || value.includes("candidate-installed")
      || value.includes("managed plugin handoff")
      || value.includes("scope == 'server'")
    ),
    `${file} package-only workflow must not contain installed-runtime or server-scope routing`,
  );
  requireStepUses(violations, file, job, "Upload release asset", "actions/upload-artifact@v7.0.1");
  const releaseAssetUpload = namedStep(job, "Upload release asset");
  const candidateRecordStep = namedStep(job, "Produce exact candidate archive record");
  const candidateRecordUpload = namedStep(job, "Upload exact candidate archive record");
  const qualificationDriverUpload = namedStep(job, "Upload separate qualification driver");
  requireStepRun(violations, file, job, "Produce exact candidate archive record", [
    "candidate-archive-store.mjs record",
    "--output \"$record_dir/candidate-archive-record.json\"",
    "--repository \"$GITHUB_REPOSITORY\"",
    "--source-sha \"$SOURCE_SHA\"",
    "--source-tree \"$SOURCE_TREE\"",
    "--target \"${{ matrix.asset_target }}\"",
    "--archive-name \"$archive_name\"",
    "--archive-bytes",
    "--archive-sha256",
    "--companion \"archive_checksum|",
    "--companion \"checksum_manifest|SHA256SUMS.txt|",
  ]);
  add(
    violations,
    candidateRecordStep?.shell === "bash"
      && candidateRecordStep?.["continue-on-error"] === undefined
      && hasExactKeys(object(candidateRecordStep?.env), [
        "INPUT_VERSION",
        "SOURCE_SHA",
        "SOURCE_TREE",
      ])
      && object(candidateRecordStep?.env).INPUT_VERSION === "${{ inputs.version }}"
      && object(candidateRecordStep?.env).SOURCE_SHA
        === "${{ steps.source-identity.outputs.sha }}"
      && object(candidateRecordStep?.env).SOURCE_TREE
        === "${{ steps.source-identity.outputs.tree }}"
      && stepIndex(job, "Produce exact candidate archive record")
        > stepIndex(job, "Report fresh package identity")
      && stepIndex(job, "Produce exact candidate archive record")
        < stepIndex(job, "Upload release asset"),
    `${file} package producer must derive one exact public candidate record from the authenticated package bytes`,
  );
  const expectedPublicPackagePath = [
    "target/release-dist/codestory-cli-v${{ inputs.version }}-${{ matrix.asset_target }}.${{ matrix.extension }}",
    "target/release-dist/codestory-cli-v${{ inputs.version }}-${{ matrix.asset_target }}.${{ matrix.extension }}.sha256",
    "target/release-dist/SHA256SUMS.txt",
    "",
  ].join("\n");
  add(
    violations,
    object(releaseAssetUpload?.with).name
        === "codestory-cli-${{ matrix.asset_target }}"
      && String(object(releaseAssetUpload?.with).path ?? "") === expectedPublicPackagePath
      && object(releaseAssetUpload?.with)["if-no-files-found"] === "error"
      && object(releaseAssetUpload?.with)["retention-days"] === 30
      && object(releaseAssetUpload?.with).overwrite === true,
    `${file} public package artifact must contain exactly the archive and its two candidate-local checksum files`,
  );
  add(
    violations,
    candidateRecordUpload?.uses === "actions/upload-artifact@v7.0.1"
      && object(candidateRecordUpload?.with).name
        === "codestory-candidate-archive-record-${{ matrix.asset_target }}"
      && object(candidateRecordUpload?.with).path
        === "target/candidate-archive-record/${{ matrix.asset_target }}/candidate-archive-record.json"
      && object(candidateRecordUpload?.with)["if-no-files-found"] === "error"
      && object(candidateRecordUpload?.with)["retention-days"] === 30
      && object(candidateRecordUpload?.with).overwrite === true
      && qualificationDriverUpload?.uses === "actions/upload-artifact@v7.0.1"
      && qualificationDriverUpload?.if === "inputs.include_qualification_driver"
      && object(qualificationDriverUpload?.with).name
        === "codestory-qualification-driver-${{ matrix.asset_target }}"
      && object(qualificationDriverUpload?.with).path
        === "target/release-dist/qualification-driver/${{ matrix.asset_target }}"
      && object(qualificationDriverUpload?.with)["if-no-files-found"] === "error"
      && object(qualificationDriverUpload?.with)["retention-days"] === 30
      && object(qualificationDriverUpload?.with).overwrite === true,
    `${file} candidate record and private qualification driver must be separate exact stable artifacts`,
  );
  requireStepUses(violations, file, job, "Upload macOS notarization proof", "actions/upload-artifact@v7.0.1");
  requireStepRun(violations, file, job, "Emit authenticated package release cell", [
    "codestory-release-cell-manifest.mjs produce",
    "package_identity:${{ matrix.asset_target }}",
    "--producer-job build",
    "--archive",
    '--expected-sha "$INPUT_REF"',
  ]);
  requireStepEnv(violations, file, job, "Emit authenticated package release cell", {
    INPUT_REF: "${{ inputs.ref }}",
  });
  const packageCellUpload = namedStep(job, "Upload authenticated package release cell");
  add(
    violations,
    packageCellUpload?.uses === "actions/upload-artifact@v7.0.1"
      && String(packageCellUpload?.if ?? "").includes("success()")
      && String(packageCellUpload?.if ?? "").includes("inputs.emit_release_cells"),
    `${file} package release cell must be a success-only retained artifact`,
  );
}

// The post-publish smoke runs whether or not the catalog was updated, so the one thing it must
// never do is let the deferred run look like the published one. Both states resolve a real Codex
// install of the real published assets; they differ in WHICH catalog served it, and that
// difference is carried into the release ledger as a distinct installer identity. These rules
// prove the two states stay distinguishable and that neither can be selected by accident.
function catalogDeliveryStateViolations(file, job, delivery, handoff, installStepName, checkoutRef) {
  const violations = [];
  const published = delivery.states.find(({ id }) => id === "published");
  const deferred = delivery.states.find(({ id }) => id === "deferred");
  const restored = delivery.states.find(({ id }) => id === "restored");
  // Whatever else the deferred branch does, it builds a catalog out of a tree and then verifies
  // the install back against a tree. If those may be the same tree by default, the comparison is
  // a tautology and the smoke cannot fail for any release-related reason. Both lanes therefore
  // check out the PUBLISHED tag and make GitHub confirm it before anything is pinned.
  const checkout = list(job.steps).find(
    (candidate) => String(object(candidate).uses ?? "").startsWith("actions/checkout@"),
  );
  add(
    violations,
    object(object(checkout).with).ref === checkoutRef
      && object(object(checkout).with)["fetch-depth"] === 0,
    `${file} post-publish smoke must check out the published release tag, not the run's own head`,
  );
  requireStepRun(violations, file, job, "Bind this smoke to the published release", [
    'gh release view "$TAG"',
    "--json isDraft",
    'published_commit="$(gh api "repos/$GITHUB_REPOSITORY/commits/$TAG" --jq .sha)"',
    `printf '%s' "$published_commit" | grep -Eq '^[0-9a-f]{40}$'`,
    'if [ "$published_commit" != "$(git -C "$GITHUB_WORKSPACE" rev-parse HEAD)" ]; then',
    'echo "commit=$published_commit" >> "$GITHUB_OUTPUT"',
  ]);
  const step = namedStep(job, "Record catalog delivery state");
  add(
    violations,
    object(step?.env).PUBLISHED_COMMIT === "${{ steps.published.outputs.commit }}",
    `${file} catalog delivery state must pin the commit resolved from the published release`,
  );
  add(
    violations,
    step?.if === undefined && step?.["continue-on-error"] === undefined,
    `${file} catalog delivery state must be unconditional and fail closed`,
  );
  add(
    violations,
    object(step?.env).CATALOG_PUBLISHED === handoff.published
      && object(step?.env).CATALOG_DELIVERY_STATE === handoff.state
      && object(step?.env).INPUT_MARKETPLACE_REVISION === handoff.revision,
    `${file} catalog delivery state must read the recorded publication handoff`,
  );
  requireStepRun(violations, file, job, "Record catalog delivery state", [
    // The published branch: the live catalog, its live revision, no fixture.
    'if [ "$CATALOG_DELIVERY_STATE" = "published" ] && [ "$CATALOG_PUBLISHED" = "true" ]; then',
    "marketplace_source=TheGreenCedar/AgentPluginMarketplace",
    'marketplace_revision="$INPUT_MARKETPLACE_REVISION"',
    "local_fixture=false",
    `installer=${published.installer}`,
    // The deferred branch: a catalog pinned to this published commit, and a revision that cannot
    // be a live one because the caller is required to have supplied none.
    '[ "$CATALOG_DELIVERY_STATE" = "deferred" ]',
    '[ "$CATALOG_DELIVERY_STATE" = "restored" ]',
    '[ "$CATALOG_PUBLISHED" = "false" ]',
    'if [ -n "$INPUT_MARKETPLACE_REVISION" ]; then',
    "catalog delivery must not carry the failed live catalog revision",
    "build-marketplace-fixture.mjs",
    // The fixture pins the PUBLISHED commit, never the workspace's own head. Building a catalog
    // out of the tree that then verifies the install makes the source-tree comparison a
    // tautology, which is how the plugin lane's deferred smoke became unable to fail.
    '--commit "$published_commit"',
    'marketplace_revision="$(git -C "$fixture_root" rev-parse HEAD)"',
    "local_fixture=true",
    `installer=${deferred.installer}`,
    `installer=${restored.installer}`,
    // Neither branch may fall through: an unset or unexpected handoff is a hard failure, never a
    // silent default into the published identity.
    "catalog delivery handoff is inconsistent",
    // Immutability, not length. A 40-character string is not a commit: the published branch
    // takes its revision from a `workflow_dispatch`-able input, and a length-only test admits
    // any 40 characters of anything.
    `printf '%s' "$marketplace_revision" | grep -Eq '^[0-9a-f]{40}$'`,
    'echo "installer=$installer"',
  ]);
  const deliveryRun = executableRunText(String(step?.run ?? ""));
  const publishedIndex = deliveryRun.indexOf(`installer=${published.installer}`);
  const deferredIndex = deliveryRun.indexOf(`installer=${deferred.installer}`);
  const publishedBranch = deliveryRun.indexOf('if [ "$CATALOG_DELIVERY_STATE" = "published" ]');
  const deferredBranch = deliveryRun.indexOf('[ "$CATALOG_DELIVERY_STATE" = "deferred" ]');
  add(
    violations,
    published.installer !== deferred.installer
      && publishedBranch >= 0
      && deferredBranch > publishedBranch
      && publishedIndex > publishedBranch
      && publishedIndex < deferredBranch
      && deferredIndex > deferredBranch
      && deliveryRun.indexOf(`installer=${restored.installer}`) > deferredBranch,
    `${file} the published installer identity must be reachable only from the published branch`,
  );
  // Neither state may be fabricated in a later step: the install must come from what this step
  // resolved, and the forbidden fragments that stop a faked install apply here too.
  for (const forbidden of ["git archive", "git clone", "git ls-remote", "--source-commit", "--source-tree"]) {
    add(
      violations,
      !deliveryRun.includes(forbidden),
      `${file} marketplace install must not fabricate installation with ${forbidden}`,
    );
  }
  requireStepRun(violations, file, job, installStepName, [
    '--marketplace-source "$MARKETPLACE_SOURCE"',
    '--local-fixture "$LOCAL_FIXTURE"',
    '--installation-source "$INSTALLATION_SOURCE"',
  ]);
  // The install arguments arrive as variables now, so the command text no longer says which
  // delivery state they came from. This binds each variable back to that step's own output.
  requireStepEnv(violations, file, job, installStepName, {
    MARKETPLACE_SOURCE: "${{ steps.delivery.outputs.marketplace_source }}",
    LOCAL_FIXTURE: "${{ steps.delivery.outputs.local_fixture }}",
    INSTALLATION_SOURCE: "${{ steps.delivery.outputs.installer }}",
  });
  return violations;
}

function validatePostPublish(workflows, violations, graph) {
  const file = "post-publish-release-smoke.yml";
  const workflow = workflows.get(file);
  if (!workflow) {
    violations.push(`${file} must exist`);
    return;
  }
  add(violations, trigger(workflow, "workflow_call") !== undefined, `${file} must be reusable`);
  // The smoke workflow's structural pins (job existence and its permission scope)
  // are rule instances and live in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates them.
  requireNoCalibrationReferences(violations, file, workflow);
  for (const event of ["workflow_call", "workflow_dispatch"]) {
    // The catalog revision is now empty exactly when publication was deferred, so the required
    // input is the delivery state itself: the caller must state which one it is, never omit it.
    const publishedInput = object(at(workflow, "on", event, "inputs", "catalog_published"));
    add(
      violations,
      publishedInput.required === true && publishedInput.type === "boolean",
      `${file} ${event} catalog_published must be a required boolean`,
    );
    const stateInput = object(at(workflow, "on", event, "inputs", "catalog_delivery_state"));
    add(
      violations,
      stateInput.required === true
        && (stateInput.type === "string" || stateInput.type === "choice"),
      `${file} ${event} catalog_delivery_state must be required`,
    );
    const marketplaceInput = object(
      at(workflow, "on", event, "inputs", "marketplace_revision"),
    );
    add(
      violations,
      marketplaceInput.type === "string" && marketplaceInput.default === "",
      `${file} ${event} marketplace_revision must be a string defaulting to the deferred empty revision`,
    );
    const closeoutInput = object(at(workflow, "on", event, "inputs", "pre_publish_closeout_artifact"));
    add(violations, closeoutInput.type === "string", `${file} ${event} pre_publish_closeout_artifact must be a string`);
  }
  const job = object(object(workflow.jobs).smoke);
  const pythonSetup = namedStep(job, "Install pinned Python");
  add(
    violations,
    pythonSetup?.uses === "actions/setup-python@v7.0.0"
      && pythonSetup?.if === "runner.os != 'macOS'"
      && object(pythonSetup?.with)["python-version"] === "3.13"
      && object(pythonSetup?.env).PSExecutionPolicyPreference === "Bypass",
    `${file} must install pinned Python with the protected Windows execution policy`,
  );
  const macosPythonSetup = namedStep(job, "Install pinned Python on macOS");
  add(
    violations,
    macosPythonSetup?.if === "runner.os == 'macOS'"
      && macosPythonSetup?.shell === "bash",
    `${file} must install pinned Python through the protected macOS user toolchain`,
  );
  requireStepRun(violations, file, job, "Install pinned Python on macOS", [
    "uv python install 3.13.14",
    'python_bin="$(uv python find 3.13.14)"',
    "platform.python_version()",
    'echo "$shim_dir" >> "$GITHUB_PATH"',
  ]);
  const expected = expectedPostPublishRows();
  add(
    violations,
    JSON.stringify(at(job, "strategy", "matrix", "include")) === JSON.stringify(expected),
    `${file} must run the three supported release assets on protected accelerated hosts`,
  );
  add(
    violations,
    job["runs-on"] === "${{ fromJSON(matrix.runs_on) }}"
      && job.environment === "${{ matrix.environment }}",
    `${file} smoke job must bind each asset to its protected runner and environment`,
  );
  add(
    violations,
    at(job, "strategy", "fail-fast") === false,
    `${file} protected matrix must not cancel sibling platform proof`,
  );
  add(
    violations,
    object(workflow.env).CODEX_CLI_VERSION === "0.144.5",
    `${file} must pin the Codex CLI used for marketplace installation`,
  );
  const publishedAuthentication = namedStep(
    job,
    "Authenticate published candidate assets",
  );
  add(
    violations,
    publishedAuthentication?.id === "published-assets"
      && publishedAuthentication?.shell === "bash"
      && publishedAuthentication?.if === undefined
      && publishedAuthentication?.["continue-on-error"] === undefined
      && hasExactKeys(object(publishedAuthentication?.env), [
        "ASSET_TARGET",
        "EXTENSION",
        "GH_TOKEN",
        "PUBLISHED_COMMIT",
        "TAG",
        "VERSION",
      ])
      && object(publishedAuthentication?.env).GH_TOKEN === "${{ github.token }}"
      && object(publishedAuthentication?.env).TAG === "${{ steps.release.outputs.tag }}"
      && object(publishedAuthentication?.env).VERSION
        === "${{ steps.release.outputs.version }}"
      && object(publishedAuthentication?.env).ASSET_TARGET
        === "${{ matrix.asset_target }}"
      && object(publishedAuthentication?.env).EXTENSION
        === "${{ matrix.extension }}"
      && object(publishedAuthentication?.env).PUBLISHED_COMMIT
        === "${{ steps.published.outputs.commit }}",
    `${file} published candidate authentication must bind the release tag, commit, target, and exact asset metadata`,
  );
  requireStepRun(violations, file, job, "Authenticate published candidate assets", [
    'gh api "repos/$GITHUB_REPOSITORY/releases/tags/$TAG"',
    'test "$(jq -r .tag_name <<<"$release")" = "$TAG"',
    'test "$(jq -r .draft <<<"$release")" = false',
    "expected one published release asset",
    'archive_asset="$(select_asset "$asset")"',
    'checksum_asset="$(select_asset "$checksum")"',
    "manifest_asset=\"$(select_asset SHA256SUMS.txt)\"",
    '[[ "$(jq -r .id <<<"$value")" =~ ^[0-9]+$ ]]',
    '[[ "$(jq -r .size <<<"$value")" =~ ^[0-9]+$ ]]',
    '[[ "$(jq -r .digest <<<"$value")" =~ ^sha256:[0-9a-f]{64}$ ]]',
    'validate_asset "$archive_asset"',
    'validate_asset "$checksum_asset"',
    'validate_asset "$manifest_asset"',
    "candidate-archive-store.mjs record",
    '--source-sha "$PUBLISHED_COMMIT"',
    "--source-tree \"$(git rev-parse 'HEAD^{tree}')\"",
    '--target "$ASSET_TARGET"',
    "--archive-bytes",
    "--archive-sha256",
    '--companion "archive_checksum|',
    '--companion "checksum_manifest|SHA256SUMS.txt|',
    "archive-id=",
    "archive-bytes=",
    "archive-sha256=",
    "checksum-id=",
    "checksum-bytes=",
    "checksum-sha256=",
    "manifest-id=",
    "manifest-bytes=",
    "manifest-sha256=",
  ]);
  const publishedRestore = namedStep(
    job,
    "Restore published candidate archive from protected host",
  );
  add(
    violations,
    publishedRestore?.id === "candidate-cache"
      && publishedRestore?.shell === "bash"
      && publishedRestore?.if === undefined
      && publishedRestore?.["continue-on-error"] === undefined
      && hasExactKeys(object(publishedRestore?.env), [
        "ASSET_TARGET",
        "PUBLISHED_COMMIT",
      ])
      && object(publishedRestore?.env).ASSET_TARGET
        === "${{ matrix.asset_target }}"
      && object(publishedRestore?.env).PUBLISHED_COMMIT
        === "${{ steps.published.outputs.commit }}",
    `${file} published candidate cache lookup must be unconditional and exact-source bound`,
  );
  requireStepRun(
    violations,
    file,
    job,
    "Restore published candidate archive from protected host",
    [
      "--arg repository \"$GITHUB_REPOSITORY\"",
      '--arg source_sha "$PUBLISHED_COMMIT"',
      "--arg source_tree \"$(git rev-parse 'HEAD^{tree}')\"",
      '--arg target "$ASSET_TARGET"',
      ".repository == $repository",
      ".source.commit == $source_sha",
      ".source.tree == $source_tree",
      ".target == $target",
      "$RUNNER_TOOL_CACHE/codestory/candidate-archives",
      "candidate-archive-store.mjs restore",
      "--record \"$record\"",
      "--output-dir target/post-publish-release-assets",
      'echo "hit=$hit" >> "$GITHUB_OUTPUT"',
    ],
  );
  const publishedMiss = namedStep(
    job,
    "Download, verify, and admit published candidate on miss",
  );
  add(
    violations,
    publishedMiss?.if === "steps.candidate-cache.outputs.hit != 'true'"
      && publishedMiss?.shell === "bash"
      && publishedMiss?.["continue-on-error"] === undefined
      && hasExactKeys(object(publishedMiss?.env), [
        "ARCHIVE_BYTES",
        "ARCHIVE_ID",
        "ARCHIVE_NAME",
        "ARCHIVE_SHA256",
        "ASSET_TARGET",
        "CHECKSUM_BYTES",
        "CHECKSUM_ID",
        "CHECKSUM_SHA256",
        "GH_TOKEN",
      ])
      && object(publishedMiss?.env).GH_TOKEN === "${{ github.token }}"
      && object(publishedMiss?.env).ASSET_TARGET === "${{ matrix.asset_target }}"
      && object(publishedMiss?.env).ARCHIVE_NAME
        === "${{ steps.published-assets.outputs.archive-name }}"
      && object(publishedMiss?.env).ARCHIVE_ID
        === "${{ steps.published-assets.outputs.archive-id }}"
      && object(publishedMiss?.env).ARCHIVE_BYTES
        === "${{ steps.published-assets.outputs.archive-bytes }}"
      && object(publishedMiss?.env).ARCHIVE_SHA256
        === "${{ steps.published-assets.outputs.archive-sha256 }}"
      && object(publishedMiss?.env).CHECKSUM_ID
        === "${{ steps.published-assets.outputs.checksum-id }}"
      && object(publishedMiss?.env).CHECKSUM_BYTES
        === "${{ steps.published-assets.outputs.checksum-bytes }}"
      && object(publishedMiss?.env).CHECKSUM_SHA256
        === "${{ steps.published-assets.outputs.checksum-sha256 }}",
    `${file} published archive transfer must run only on an exact cache miss`,
  );
  requireStepRun(
    violations,
    file,
    job,
    "Download, verify, and admit published candidate on miss",
    [
      "releases/assets/$id",
      "--continue-at -",
      "--max-time 120",
      'test "${actual%% *}" = "$expected_bytes"',
      'test "${actual#* }" = "$expected_sha256"',
      'download_asset "$ARCHIVE_ID" "$ARCHIVE_NAME"',
      'download_asset "$CHECKSUM_ID" "$ARCHIVE_NAME.sha256"',
      'cp "$stage/$ARCHIVE_NAME.sha256" "$stage/SHA256SUMS.txt"',
      "candidate-archive-store.mjs admit",
      "--store-root \"$RUNNER_TOOL_CACHE/codestory/candidate-archives\"",
      "--output-dir target/post-publish-release-assets",
    ],
  );
  const publishedManifest = namedStep(
    job,
    "Download authenticated published checksum manifest",
  );
  const publishedBinding = namedStep(
    job,
    "Bind materialized published asset paths",
  );
  add(
    violations,
    publishedManifest?.id === "published-checksum"
      && publishedManifest?.if === undefined
      && publishedManifest?.shell === "bash"
      && publishedManifest?.["continue-on-error"] === undefined
      && hasExactKeys(object(publishedManifest?.env), [
        "GH_TOKEN",
        "MANIFEST_BYTES",
        "MANIFEST_ID",
        "MANIFEST_SHA256",
      ])
      && object(publishedManifest?.env).GH_TOKEN === "${{ github.token }}"
      && object(publishedManifest?.env).MANIFEST_ID
        === "${{ steps.published-assets.outputs.manifest-id }}"
      && object(publishedManifest?.env).MANIFEST_BYTES
        === "${{ steps.published-assets.outputs.manifest-bytes }}"
      && object(publishedManifest?.env).MANIFEST_SHA256
        === "${{ steps.published-assets.outputs.manifest-sha256 }}"
      && publishedBinding?.id === "asset"
      && publishedBinding?.if === undefined
      && publishedBinding?.shell === "bash"
      && hasExactKeys(object(publishedBinding?.env), [
        "ASSET_NAME",
        "PUBLISHED_CHECKSUM",
      ])
      && object(publishedBinding?.env).ASSET_NAME
        === "${{ steps.published-assets.outputs.archive-name }}"
      && object(publishedBinding?.env).PUBLISHED_CHECKSUM
        === "${{ steps.published-checksum.outputs.checksum }}",
    `${file} global published checksum must stay independently authenticated and bind the materialized candidate`,
  );
  requireStepRun(
    violations,
    file,
    job,
    "Download authenticated published checksum manifest",
    [
      "releases/assets/$MANIFEST_ID",
      'test "${actual%% *}" = "$MANIFEST_BYTES"',
      'test "${actual#* }" = "$MANIFEST_SHA256"',
      'echo "checksum=$checksum" >> "$GITHUB_OUTPUT"',
    ],
  );
  requireStepRun(
    violations,
    file,
    job,
    "Bind materialized published asset paths",
    [
      'test -f "$dir/$ASSET_NAME"',
      'echo "archive=$dir/$ASSET_NAME" >> "$GITHUB_OUTPUT"',
      'echo "checksum=$PUBLISHED_CHECKSUM" >> "$GITHUB_OUTPUT"',
    ],
  );
  add(
    violations,
    stepIndex(job, "Authenticate published candidate assets")
      < stepIndex(job, "Restore published candidate archive from protected host")
      && stepIndex(job, "Restore published candidate archive from protected host")
        < stepIndex(job, "Download, verify, and admit published candidate on miss")
      && stepIndex(job, "Download, verify, and admit published candidate on miss")
        < stepIndex(job, "Download authenticated published checksum manifest")
      && stepIndex(job, "Download authenticated published checksum manifest")
        < stepIndex(job, "Bind materialized published asset paths")
      && jobShellInvocationsContaining(job, "releases/assets/$id").length === 1
      && jobShellInvocationsContaining(
        job,
        "releases/assets/$MANIFEST_ID",
      ).length === 1
      && !scalarStrings(workflow).some(value => value.includes("gh release download")),
    `${file} must resolve the protected cache before any large release-asset transfer and never use an unconditional bulk download`,
  );
  const catalogDelivery = graph.workflow_policy.catalog_delivery;
  const resolveStepName = "Resolve the published plugin through the marketplace catalog";
  violations.push(...catalogDeliveryStateViolations(
    file,
    job,
    catalogDelivery,
    {
      published: "${{ inputs.catalog_published }}",
      state: "${{ inputs.catalog_delivery_state }}",
      revision: "${{ inputs.marketplace_revision }}",
    },
    resolveStepName,
    "${{ steps.release.outputs.tag }}",
  ));
  // The one place the delivery state reaches the release ledger. It must be the resolved value and
  // never a literal, or a deferred run could sign a cell saying the public catalog served it.
  const identityRun = executableRunText(
    String(namedStep(job, "Emit authenticated post-publish release cells")?.run ?? ""),
  );
  add(
    violations,
    identityRun.includes('--arg installer "$DELIVERED_INSTALLER"')
      && object(namedStep(job, "Emit authenticated post-publish release cells")?.env)
        .DELIVERED_INSTALLER === "${{ steps.delivery.outputs.installer }}",
    `${file} post-publish cells must record the resolved delivery installer identity`,
  );
  for (const state of catalogDelivery.states) {
    add(
      violations,
      !identityRun.includes(state.installer),
      `${file} post-publish cells must not hard-code the ${state.id} installer identity`,
    );
  }
  const resolveInstalled = namedStep(job, resolveStepName);
  requireStepRun(violations, file, job, resolveStepName, [
    'marketplace_revision="$MARKETPLACE_REVISION"',
    // Re-checked here as an immutable identity, not merely as 40 characters: this job is
    // dispatchable, so the published branch's revision can arrive from a human.
    `printf '%s' "$marketplace_revision" | grep -Eq '^[0-9a-f]{40}$'`,
    '"@openai/codex@$CODEX_CLI_VERSION"',
    "install-codestory-marketplace-proof.mjs",
    '--marketplace-source "$MARKETPLACE_SOURCE"',
    '--marketplace-revision "$marketplace_revision"',
    '--local-fixture "$LOCAL_FIXTURE"',
    '--source-repository "$GITHUB_WORKSPACE"',
    "install-attestation-v2.json",
    'isolated_home="$install_root/isolated-home"',
    'HOME="$isolated_home" node',
  ]);
  // The install arguments arrive as variables, so the command text no longer says which delivery
  // state produced them. Each variable is bound back to the step that resolved it.
  requireStepEnv(violations, file, job, resolveStepName, {
    MARKETPLACE_REVISION: "${{ steps.delivery.outputs.marketplace_revision }}",
    MARKETPLACE_SOURCE: "${{ steps.delivery.outputs.marketplace_source }}",
    LOCAL_FIXTURE: "${{ steps.delivery.outputs.local_fixture }}",
  });
  add(
    violations,
    namedStep(job, "Prove packaged version, help, and stdio shape")?.shell === "bash",
    `${file} packaged Python proof must use Bash on every protected platform`,
  );
  // The published asset this proof reads now arrives through `env:`, so the command text alone no
  // longer says which archive or version it proved.
  requireStepRun(violations, file, job, "Prove packaged version, help, and stdio shape", [
    '--archive "$ASSET_ARCHIVE"',
    '--checksum-file "$ASSET_CHECKSUM"',
    '--expected-version "$RELEASE_VERSION"',
  ]);
  requireStepEnv(violations, file, job, "Prove packaged version, help, and stdio shape", {
    ASSET_ARCHIVE: "${{ steps.asset.outputs.archive }}",
    ASSET_CHECKSUM: "${{ steps.asset.outputs.checksum }}",
    RELEASE_VERSION: "${{ steps.release.outputs.version }}",
  });
  // The macOS signing proof quarantines and unpacks the same published archive.
  requireStepEnv(violations, file, job, "Prove published macOS signature, notarization, and quarantined execution", {
    ASSET_ARCHIVE: "${{ steps.asset.outputs.archive }}",
  });
  const resolveRun = executableRunText(String(resolveInstalled?.run ?? ""));
  for (const forbidden of [
    "git archive",
    "git clone",
    "git ls-remote",
    "plugin_package_sha256",
    "--source-commit",
    "--source-tree",
  ]) {
    add(
      violations,
      !resolveRun.includes(forbidden),
      `${file} marketplace install must not fabricate installation with ${forbidden}`,
    );
  }
  add(
    violations,
    resolveInstalled?.if === undefined
      && resolveInstalled?.["continue-on-error"] === undefined,
    `${file} installed plugin resolution must be unconditional and fail closed`,
  );
  const installedProofName = "Prove the catalog-resolved published runtime";
  const installed = namedStep(job, installedProofName);
  add(violations, installed !== undefined, `${file} installed runtime proof step is missing`);
  add(
    violations,
    installed?.if === undefined
      && installed?.shell === "bash"
      && installed?.["continue-on-error"] === undefined,
    `${file} installed runtime proof must be unconditional and fail closed`,
  );
  add(
    violations,
    object(installed?.env).CODESTORY_EMBED_ALLOW_CPU === "0",
    `${file} installed runtime proof must reject CPU fallback`,
  );
  const installedRun = executableRunText(String(installed?.run ?? ""));
  for (const fragment of [
    "python .github/scripts/check-packaged-agent-proof.py",
    '--archive "$ASSET_ARCHIVE"',
    "--plugin-handoff",
    "--engine-policy accelerated",
    '--expected-backend "$EXPECTED_BACKEND"',
    "--proof-tier installed_runtime",
    "--server-behavior-only",
    "--installed-plugin-attestation",
    "--installed-plugin-data",
    "--expected-source-sha",
    "--expected-source-tree",
  ]) {
    add(
      violations,
      installedRun.includes(fragment),
      `${file} installed runtime proof must run ${fragment}`,
    );
  }
  // The archive and the resolved installation now reach the proof as variables. Without these the
  // command text would read the same whether it proved the published asset or something else.
  requireStepEnv(violations, file, job, installedProofName, {
    ASSET_ARCHIVE: "${{ steps.asset.outputs.archive }}",
    ASSET_CHECKSUM: "${{ steps.asset.outputs.checksum }}",
    RELEASE_VERSION: "${{ steps.release.outputs.version }}",
    INSTALLED_PLUGIN_ROOT: "${{ steps.installed.outputs.plugin_root }}",
    INSTALLED_ATTESTATION: "${{ steps.installed.outputs.attestation }}",
    INSTALLED_PLUGIN_DATA: "${{ steps.installed.outputs.plugin_data }}",
    CATALOG_DELIVERY_STATE: "${{ steps.delivery.outputs.state }}",
    DELIVERED_INSTALLER: "${{ steps.delivery.outputs.installer }}",
    EXPECTED_BACKEND: "${{ matrix.backend }}",
  });
  add(
    violations,
    hasExactKeys(object(installed?.env), [
      "ASSET_ARCHIVE",
      "ASSET_CHECKSUM",
      "CATALOG_DELIVERY_STATE",
      "CODESTORY_EMBED_ALLOW_CPU",
      "DELIVERED_INSTALLER",
      "EXPECTED_BACKEND",
      "INSTALLED_ATTESTATION",
      "INSTALLED_PLUGIN_DATA",
      "INSTALLED_PLUGIN_ROOT",
      "RELEASE_VERSION",
    ]),
    `${file} installed runtime restart proof must bind only the reviewed package, install, delivery, and backend identities`,
  );
  const commonStart = installedRun.indexOf("common=(");
  const commonEnd = commonStart < 0 ? -1 : installedRun.indexOf("\n)", commonStart);
  const installedCommon = commonStart >= 0 && commonEnd > commonStart
    ? installedRun.slice(commonStart, commonEnd + 2)
    : "";
  const commonExpansion = '"${common[@]}"';
  add(
    violations,
    occurrenceCount(installedRun, "common=(") === 1
      && occurrenceCount(installedCommon, "python .github/scripts/check-packaged-agent-proof.py") === 1
      && [
        '--archive "$ASSET_ARCHIVE"',
        '--checksum-file "$ASSET_CHECKSUM"',
        '--expected-version "$RELEASE_VERSION"',
        '--plugin-root "$INSTALLED_PLUGIN_ROOT"',
        '--installed-plugin-attestation "$INSTALLED_ATTESTATION"',
        '--installed-plugin-data "$INSTALLED_PLUGIN_DATA"',
        '--expected-source-sha "$source_sha"',
        '--expected-source-tree "$source_tree"',
        '--expected-backend "$EXPECTED_BACKEND"',
      ].every(fragment => installedCommon.includes(fragment)),
    `${file} installed runtime restart proof must define one common exact-package and installed-runtime invocation`,
  );
  const session1Proof = `${commonExpansion} --out-dir "$proof_root/session-1"`;
  const session2Proof = `${commonExpansion} --out-dir "$proof_root/session-2"`;
  add(
    violations,
    occurrenceCount(installedRun, commonExpansion) === 2
      && occurrenceCount(installedRun, session1Proof) === 1
      && occurrenceCount(installedRun, session2Proof) === 1,
    `${file} installed runtime restart proof must execute exactly two distinct sessions through the common invocation`,
  );
  const timingAndSessionOrder = [
    'session_1_started_ms="$(now_ms)"',
    session1Proof,
    'session_1_finished_ms="$(now_ms)"',
    'session_2_started_ms="$(now_ms)"',
    session2Proof,
    'session_2_finished_ms="$(now_ms)"',
  ].map(fragment => installedRun.indexOf(fragment));
  add(
    violations,
    installedRun.includes("time.time_ns() // 1_000_000")
      && timingAndSessionOrder.every(index => index >= 0)
      && timingAndSessionOrder.every(
        (index, position) => position === 0 || timingAndSessionOrder[position - 1] < index,
      ),
    `${file} installed runtime restart proof must record two monotonically ordered session intervals`,
  );
  add(
    violations,
    installedRun.includes('hashlib.file_digest(source, "sha256").hexdigest()')
      && installedRun.includes(
        'binary_sha256="$(jq -er \'.package_contract.manifest.binary.sha256\' "$proof_root/session-1/summary.json")"',
      ),
    `${file} installed runtime restart proof must derive the actual archive and packaged binary hashes`,
  );
  const restartInvocations = shellInvocationsContaining(
    installedRun,
    "node .github/scripts/restart-survival-receipt.mjs",
  );
  const restartInvocation = restartInvocations.length === 1 ? restartInvocations[0] : "";
  add(
    violations,
    restartInvocations.length === 1
      && [
        '--session-1-summary "$proof_root/session-1/summary.json"',
        '--session-2-summary "$proof_root/session-2/summary.json"',
        '--install-attestation "$INSTALLED_ATTESTATION"',
        '--catalog-delivery-state "$CATALOG_DELIVERY_STATE"',
        '--expected-installer-identity "$DELIVERED_INSTALLER"',
        '--expected-source-commit "$source_sha"',
        '--expected-source-tree "$source_tree"',
        '--expected-version "$RELEASE_VERSION"',
        '--expected-archive-sha256 "$archive_sha256"',
        '--expected-binary-sha256 "$binary_sha256"',
        '--expected-backend "$EXPECTED_BACKEND"',
        '--session-1-start-ms "$session_1_started_ms"',
        '--session-1-finished-ms "$session_1_finished_ms"',
        '--session-2-start-ms "$session_2_started_ms"',
        '--session-2-finished-ms "$session_2_finished_ms"',
        '--out "$proof_root/restart-survival.json"',
      ].every(fragment => restartInvocation.includes(fragment))
      && !restartInvocation.includes("|| true"),
    `${file} installed runtime restart receipt must bind both sessions, one install, delivery, release, package, backend, and timing identities`,
  );
  const proofUpload = namedStep(job, "Upload post-publish proof artifacts");
  add(
    violations,
    proofUpload?.if === "always()"
      && proofUpload?.uses === "actions/upload-artifact@v7.0.1"
      && String(object(proofUpload?.with).path).split(/\r?\n/u)
        .map(value => value.trim())
        .filter(Boolean)
        .includes("target/post-publish-installed-proof")
      && object(proofUpload?.with)["if-no-files-found"] === "error",
    `${file} post-publish proof artifact must retain the complete installed-runtime restart root`,
  );
  for (const fragment of ["--engine-policy cpu_explicit", "--expected-backend CPU", "--ground-only"]) {
    add(
      violations,
      !installedRun.includes(fragment),
      `${file} installed runtime proof must not run ${fragment}`,
    );
  }
  requireStepUses(
    violations,
    file,
    job,
    "Download accepted pre-publish closeout",
    "actions/download-artifact@v8.0.1",
  );
  requireStepRun(violations, file, job, "Emit authenticated post-publish release cells", [
    "platform_support:${{ matrix.asset_target }}",
    "installed_runtime_behavior:${{ matrix.asset_target }}",
    "post_publish_bytes:${{ matrix.asset_target }}",
    "--pre-publish-ledger",
    "release-cell-postpublish-${{ matrix.asset_target }}",
  ]);
  requireStepUses(
    violations,
    file,
    job,
    "Upload authenticated post-publish release cells",
    "actions/upload-artifact@v7.0.1",
  );
  const postCellUpload = namedStep(job, "Upload authenticated post-publish release cells");
  add(
    violations,
    String(postCellUpload?.if ?? "").includes("success()")
      && String(postCellUpload?.if ?? "").includes("inputs.emit_release_cells"),
    `${file} post-publish release cells must be success-only artifacts`,
  );
  add(
    violations,
    namedStep(job, "Install pinned Rust for Windows rollback proof") === undefined
      && object(workflow.env).RELEASE_RUST_TOOLCHAIN === undefined,
    `${file} published ground proof must not install unused Rust tooling`,
  );
  add(
    violations,
    !installedRun.includes("--offline"),
    `${file} installed runtime proof must allow the managed launcher to provision the release asset`,
  );
  const macProof = namedStep(job, "Prove published macOS signature, notarization, and quarantined execution");
  requireStepRun(violations, file, job, "Prove published macOS signature, notarization, and quarantined execution", [
    "archive-quarantine.txt",
    "extracted-binary-quarantine.txt",
    "Authority=Developer ID Application:",
    "TeamIdentifier=${APPLE_DEVELOPER_TEAM_ID}",
    "certificate leaf",
  ]);
  violations.push(...macosCliDistributionViolations(macProof, macProof, '"$bin"').map(message => `${file} ${message}`));
  const windowsInstaller = namedStep(job, "Run Windows installer ownership self-test");
  add(
    violations,
    windowsInstaller?.shell
      === `powershell -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command ". '{0}'"`,
    `${file} Windows installer proof must bypass host execution policy explicitly`,
  );
  requireStepRun(violations, file, job, "Run Windows installer ownership self-test", ["scripts/install-codestory.ps1 -SelfTest"]);
  add(violations, !scalarStrings(workflow).some(value => value.includes("sha256sum")), `${file} must use the portable Python checksum gate`);
}

function validatePackagedCoordinator(workflows, violations, graph) {
  const file = "packaged-platform-pr.yml";
  const workflow = workflows.get(file);
  if (!workflow) {
    violations.push(`${file} must exist`);
    return;
  }
  const promotion = graph.workflow_policy.promotion;
  const calibrationPolicy = object(graph.workflow_policy.calibration);
  const qualificationPolicy = object(graph.workflow_policy.qualification);
  add(
    violations,
    sameMembers(Object.keys(object(workflow.jobs)), [
      "route",
      "calibration-macos",
      "calibration-assemble",
      "source-proof",
      "packaged-proof",
      "macos-metal-proof",
      "frozen-candidate-quality",
      "windows-vulkan-proof",
      "linux-vulkan-proof",
      "closeout",
    ]),
    `${file} must retain the reviewed exact job set so no hidden hardware job can block calibration or qualification`,
  );
  add(
    violations,
    calibrationPolicy.coordinator_workflow === file
      && calibrationPolicy.mode === "calibration"
      && calibrationPolicy.assembly_job === "calibration-assemble"
      && calibrationPolicy.runs_per_required_cell === 3
      && calibrationPolicy.samples_per_metric_per_run === 1
      && list(calibrationPolicy.required_cells).length === 1
      && object(list(calibrationPolicy.required_cells)[0]).id
        === "protected_macos_arm64_metal"
      && list(calibrationPolicy.optional_cells).length === 1
      && object(list(calibrationPolicy.optional_cells)[0]).id
        === "protected_linux_x64_vulkan"
      && object(list(calibrationPolicy.optional_cells)[0]).assembly_dependency === false
      && object(list(calibrationPolicy.optional_cells)[0]).feeds_constant_selection === false,
    `${file} must implement the release claim graph calibration contract`,
  );
  add(
    violations,
    qualificationPolicy.coordinator_workflow === file
      && qualificationPolicy.mode === "qualification"
      && qualificationPolicy.runs_per_available_cell === 1
      && JSON.stringify(list(qualificationPolicy.required_cells))
        === JSON.stringify([
          {
            id: "protected_macos_arm64_metal",
            workflow: "macos-metal-proof.yml",
            job: "packaged-metal",
            policy: "accelerated",
            backend: "metal",
          },
          {
            id: "protected_windows_x64_vulkan",
            workflow: "windows-vulkan-proof.yml",
            job: "packaged-vulkan",
            policy: "accelerated",
            backend: "vulkan",
          },
        ])
      && JSON.stringify(list(qualificationPolicy.optional_cells))
        === JSON.stringify([
          {
            id: "protected_linux_x64_vulkan",
            workflow: "linux-vulkan-proof.yml",
            job: "packaged-vulkan",
            trigger: "workflow_dispatch",
            policy: "accelerated",
            backend: "vulkan",
            closeout_dependency: false,
            blocking: false,
          },
        ])
      && JSON.stringify(object(qualificationPolicy.quality_contract))
        === JSON.stringify({
          producer_workflow: "packaged-platform-pr.yml",
          producer_job: "frozen-candidate-quality",
          producer_cell: "protected_macos_arm64_metal",
          scheduled_once_per_frozen_candidate: true,
          blocking: false,
          closeout_dependency: false,
          claimed: false,
          archive_cache_key_fields: [
            "source.commit",
            "target",
            "archive.sha256",
          ],
          archive_cache_contract: "candidate_archive_cache",
          archive_transfer: "authenticated_miss_only",
          evaluation_owner: "isolated_reusable_workflow",
          evaluation_owner_sha256:
            pinnedProgramRow(graph, "frozen_candidate_quality_workflow").sha256,
          evaluation_contract: "publishable-three-repeat-packet/v1",
          task_count: 1,
          repeats_per_task: 3,
          row_count: 3,
        })
      && sameMembers(list(qualificationPolicy.required_evidence), [
        "qualification_scenarios",
        "true_idle_exit",
        "total_codestory_process_memory",
        "backend_observed_accelerator_residency",
      ])
      && sameMembers(list(qualificationPolicy.required_scenarios), [
        "client_death",
        "cold_race",
        "frozen_owner",
        "incompatible_owner",
        "mixed_queue",
        "server_crash",
        "true_idle_respawn",
        "worker_stall",
      ])
      && qualificationPolicy.true_idle_timeout_ms === 60_000
      && qualificationPolicy.true_idle_observation_grace_ms === 2_500
      && sameMembers(list(qualificationPolicy.forbidden_policies), ["cpu_explicit"])
      && sameMembers(list(qualificationPolicy.forbidden_backends), ["cpu"])
      && sameMembers(
        list(qualificationPolicy.forbidden_environment),
        ["CODESTORY_EMBED_ALLOW_CPU=1"],
      )
      && JSON.stringify(object(qualificationPolicy.driver_contract))
        === JSON.stringify(expectedQualificationDriverContract()),
    `${file} must implement the release claim graph qualification contract`,
  );
  const expectedConcurrency = [
    "proof-",
    promotion.proof_run_sha_expression,
    "-${{ inputs.mode || 'platform' }}-${{ inputs.pr_number || 'dev' }}",
  ].join("");
  add(
    violations,
    trigger(workflow, "pull_request") === undefined,
    `${file} support PR labels must not trigger package or hardware proof`,
  );
  add(
    violations,
    at(workflow, "concurrency", "group") === expectedConcurrency,
    `${file} concurrency must bind the Actions SHA, mode, PR identity, and exact label`,
  );
  add(
    violations,
    String(at(workflow, "on", "workflow_dispatch", "inputs", "pr_number", "description") ?? "")
      .includes(promotion.manual_pr_ref_hint),
    `${file} manual PR input must require ${promotion.manual_pr_ref_hint}`,
  );
  add(
    violations,
    sameMembers(
      at(workflow, "on", "workflow_dispatch", "inputs", "mode", "options"),
      ["package", "platform", "qualification", "calibration", "integration"],
    ),
    `${file} dispatch modes changed`,
  );
  add(
    violations,
    sameMembers(
      at(workflow, "on", "workflow_dispatch", "inputs", "scope", "options"),
      ["auto", "none", "linux", "windows", "macos", "full"],
    ),
    `${file} dispatch scopes changed`,
  );
  const linuxQualificationInput = object(
    at(workflow, "on", "workflow_dispatch", "inputs", "qualify_linux_vulkan"),
  );
  add(
    violations,
    linuxQualificationInput.required === false
      && linuxQualificationInput.default === false
      && linuxQualificationInput.type === "boolean",
    `${file} Linux lifecycle qualification must remain an explicit non-default dispatch opt-in`,
  );
  add(violations, trigger(workflow, "pull_request_target") === undefined, `${file} must not use pull_request_target`);
  // The workflow-scoped permission grants (actions write for superseded-run
  // cancellation, read-only contents) are rule instances and live in
  // release-claims.json under workflow_policy.structural_pins;
  // structuralPinViolations evaluates them with their reviewed violation text.
  // The route job's structural pin (job existence) is a rule instance and
  // lives in the same block. Its step fragments - the trusted-head resolver
  // conjuncts, the freeze verification, the exact-head source proof, the
  // change-aware scope selection, and the calibration producer authentication
  // - are rule instances and live in release-claims.json under
  // workflow_policy.step_fragments; stepFragmentViolations evaluates them.
  const route = object(object(workflow.jobs).route);
  add(
    violations,
    route.if === undefined,
    `${file} route job must execute only explicit dispatches`,
  );
  // DISPOSITION: step env bindings have no graph evaluator, so the resolver's
  // calibration input bindings stay code.
  requireStepEnv(violations, file, route, "Resolve trusted exact head", {
    INPUT_CALIBRATION_ARTIFACT: "${{ inputs.calibration_bundle_artifact }}",
    INPUT_CALIBRATION_RUN_ID: "${{ inputs.calibration_bundle_run_id }}",
    INPUT_QUALIFY_LINUX_VULKAN: "${{ inputs.qualify_linux_vulkan }}",
  });
  add(
    violations,
    stepRun(route, "Resolve trusted exact head").includes(
      'if [ "$qualify_linux_vulkan" = true ] && [ "$mode" != qualification ]; then',
    ),
    `${file} Linux lifecycle qualification opt-in must fail outside qualification mode`,
  );
  add(
    violations,
    namedStep(route, "Require executable release freeze")?.if === undefined,
    `${file} every broad proof mode must authenticate its exact candidate head`,
  );
  // DISPOSITION: step env bindings have no graph evaluator, so the freeze
  // step's resolved-mode binding stays code.
  requireStepEnv(violations, file, route, "Require executable release freeze", {
    RESOLVED_MODE: "${{ steps.resolve.outputs.mode }}",
  });
  const exactHeadSourceProof = namedStep(route, "Require successful exact-head source proof");
  add(
    violations,
    exactHeadSourceProof?.if
      === "steps.resolve.outputs.mode != 'integration' && steps.resolve.outputs.mode != 'calibration'",
    `${file} calibration must precede the sole frozen-candidate source proof`,
  );
  // The mode the scope selector branches on now arrives as a variable, so the branch text alone no
  // longer says which mode it read. This binds the variable back to the resolver's own output.
  // DISPOSITION: step env bindings have no graph evaluator, so the binding
  // stays code.
  requireStepEnv(violations, file, route, "Select change-aware proof scope", {
    RESOLVED_MODE: "${{ steps.resolve.outputs.mode }}",
  });
  add(
    violations,
    String(namedStep(route, "Select change-aware proof scope")?.run ?? "")
      .includes('if [ "$REQUESTED_SCOPE" = none ] || [ "$REQUESTED_SCOPE" = linux ]; then'),
    `${file} integration must preserve explicit no-op and Linux scopes`,
  );
  add(
    violations,
    namedStep(route, "Read qualification constant-set state") === undefined
      && at(route, "outputs", "constants_frozen") === undefined
      && at(route, "outputs", "freeze_transition") === undefined
      && !scalarStrings(workflow).some(value =>
        value.includes("enforce_calibration_freeze_lineage")
        || value.includes("freeze_transition")
        || value.includes("base_frozen")
      ),
    `${file} standard coordinator must not gate release proof on constant-set freeze state`,
  );
  const routeSteps = list(route.steps).map(object);
  add(
    violations,
    routeSteps.findIndex(step => step.name === "Resolve trusted exact head") === 0
      && routeSteps.findIndex(step => step.uses === "actions/checkout@v5") > 0,
    `${file} must resolve exact workflow/ref identity before checkout`,
  );
  add(
    violations,
    at(workflow, "jobs", "calibration-linux") === undefined,
    `${file} calibration must not schedule hosted Linux CPU or wait for optional Linux Vulkan evidence`,
  );
  // The calibration-macos job's structural pin (job existence) is a rule
  // instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it.
  const calibrationMacos = object(object(workflow.jobs)["calibration-macos"]);
  add(
    violations,
    calibrationMacos.uses === "./.github/workflows/macos-metal-proof.yml"
      && object(calibrationMacos.with).calibration_mode === true,
    `${file} protected macOS calibration must call Metal proof in calibration mode`,
  );
  add(
    violations,
    at(workflow, "jobs", "macos-source") === undefined,
    `${file} standard coordinator must not add a macOS source hard gate`,
  );
  // The calibration-assemble job's structural pin (job existence) is a rule
  // instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it. The
  // assembler's step fragments are rule instances and live in
  // release-claims.json under workflow_policy.step_fragments;
  // stepFragmentViolations evaluates them.
  const calibrationAssemble = object(object(workflow.jobs)["calibration-assemble"]);
  add(
    violations,
    sameMembers(needs(calibrationAssemble), ["route", "calibration-macos"])
      && String(calibrationAssemble.if ?? "")
        === "always() && needs.route.result == 'success' && needs.route.outputs.mode == 'calibration' && needs.calibration-macos.result == 'success'",
    `${file} calibration assembly must wait only for required protected macOS Metal evidence`,
  );
  const calibrationAssemblySteps = list(calibrationAssemble.steps).map(object);
  const calibrationCheckout = namedStep(
    calibrationAssemble,
    "Checkout exact calibration head",
  );
  const calibrationDownload = namedStep(
    calibrationAssemble,
    "Download protected macOS calibration runs",
  );
  const calibrationAssembly = namedStep(
    calibrationAssemble,
    "Assemble frozen calibration candidate",
  );
  const calibrationUpload = namedStep(
    calibrationAssemble,
    "Upload calibration bundle and frozen constant candidate",
  );
  add(
    violations,
    JSON.stringify(calibrationAssemblySteps.map(step => step.name))
      === JSON.stringify([
        "Checkout exact calibration head",
        "Download protected macOS calibration runs",
        "Assemble frozen calibration candidate",
        "Upload calibration bundle and frozen constant candidate",
      ])
      && calibrationCheckout?.uses === "actions/checkout@v5"
      && hasExactKeys(calibrationCheckout?.with, ["ref", "fetch-depth"])
      && object(calibrationCheckout?.with).ref === "${{ needs.route.outputs.head_sha }}"
      && object(calibrationCheckout?.with)["fetch-depth"] === 0
      && calibrationDownload?.uses === "actions/download-artifact@v8.0.1"
      && hasExactKeys(calibrationDownload?.with, ["name", "path"])
      && object(calibrationDownload?.with).name
        === "embedding-calibration-macos-${{ needs.route.outputs.version }}"
      && object(calibrationDownload?.with).path === "target/calibration-inputs/macos"
      && calibrationAssembly?.shell === "bash"
      && hasExactKeys(calibrationAssembly?.env, ["EXPECTED_HEAD_SHA"])
      && object(calibrationAssembly?.env).EXPECTED_HEAD_SHA
        === "${{ needs.route.outputs.head_sha }}"
      && calibrationUpload?.uses === "actions/upload-artifact@v7.0.1"
      && hasExactKeys(
        calibrationUpload?.with,
        ["name", "path", "if-no-files-found", "retention-days"],
      )
      && object(calibrationUpload?.with).name
        === "embedding-calibration-bundle-${{ needs.route.outputs.head_sha }}"
      && object(calibrationUpload?.with).path === "target/calibration-freeze"
      && object(calibrationUpload?.with)["if-no-files-found"] === "error"
      && object(calibrationUpload?.with)["retention-days"] === 30,
    `${file} calibration assembly must keep the exact protected macOS-only step boundary`,
  );
  const calibrationAssemblyRun = stepRun(
    calibrationAssemble,
    "Assemble frozen calibration candidate",
  );
  add(
    violations,
    !scalarStrings(calibrationAssemble).some(value => value.toLowerCase().includes("linux"))
      && !calibrationAssemblyRun.includes("find target/calibration-inputs -type"),
    `${file} calibration assembly must not select, discover, or gate on Linux evidence`,
  );
  // DISPOSITION: step uses pins have no graph evaluator, so the upload action
  // pin stays code.
  requireStepUses(
    violations,
    file,
    calibrationAssemble,
    "Upload calibration bundle and frozen constant candidate",
    "actions/upload-artifact@v7.0.1",
  );
  add(
    violations,
    object(namedStep(
      calibrationAssemble,
      "Upload calibration bundle and frozen constant candidate",
    )?.with).name
      === "embedding-calibration-bundle-${{ needs.route.outputs.head_sha }}",
    `${file} calibration artifact name must bind the exact source head`,
  );
  // The packaged-proof job's structural pin (job existence) is a rule instance
  // and lives in release-claims.json under workflow_policy.structural_pins;
  // structuralPinViolations evaluates it.
  const packaged = object(object(workflow.jobs)["packaged-proof"]);
  add(violations, packaged.uses === "./.github/workflows/packaged-platform-proof.yml", `${file} must call packaged proof`);
  add(
    violations,
    String(packaged.if ?? "").includes("needs.route.outputs.mode == 'package'")
      && String(packaged.if ?? "").includes("needs.route.outputs.mode == 'platform'")
      && String(packaged.if ?? "").includes("needs.route.outputs.mode == 'qualification'")
      && object(packaged.with).hermetic_linux
        === "${{ needs.route.outputs.mode == 'qualification' }}"
      && object(packaged.with).include_qualification_driver
        === "${{ needs.route.outputs.mode == 'qualification' }}",
    `${file} package and platform modes must build fresh archives while only qualification runs the cold Linux boundary`,
  );
  add(
    violations,
    object(packaged.with).include_qualification_driver
      === "${{ needs.route.outputs.mode == 'qualification' }}",
    `${file} must retain the private qualification driver only for frozen-candidate qualification`,
  );
  for (const key of [
    "candidate_installed_proof",
    "candidate_installed_only",
    "server_behavior_only",
    "enforce_calibration_freeze_lineage",
  ]) {
    add(
      violations,
      object(packaged.with)[key] === undefined,
      `${file} package-only call must not pass ${key}`,
    );
  }
  add(
    violations,
    !String(packaged.if ?? "").includes("release-evidence")
      && !needs(packaged).includes("release-evidence")
      && object(packaged.with).quality_evidence_artifact === undefined,
    `${file} package proof must not depend on optional release evidence`,
  );
  violations.push(...packagedPrSigningViolations(workflow));
  // The macos-metal-proof job's structural pin (job existence) and its needs
  // edge are rule instances and live in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates them
  // with their reviewed violation text.
  const metal = object(object(workflow.jobs)["macos-metal-proof"]);
  add(
    violations,
    String(metal.if ?? "").includes("needs.route.outputs.mode != 'package'"),
    `${file} package-only mode must skip protected Metal proof`,
  );
  add(violations, object(metal.with).use_packaged_cli_artifact === true, `${file} Metal proof must use the packaged CLI`);
  add(
    violations,
    object(metal.with).candidate_installed_proof
      === "${{ needs.route.outputs.mode != 'qualification' }}",
    `${file} qualification must run full Metal proof rather than candidate-installed proof`,
  );
  add(
    violations,
    object(metal.with).server_behavior_only
        === "${{ needs.route.outputs.mode != 'qualification' }}"
      && object(metal.with).quality_evidence_artifact === undefined,
    `${file} qualification must run one full Metal lifecycle proof without optional quality inputs`,
  );
  // The frozen-candidate-quality job's structural pin (job existence) is a
  // rule instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it.
  // DISPOSITION: the caller's needs edge stays code because it is one conjunct
  // of the single reviewed isolated-owner message below, which a needs pin
  // could not reproduce without splitting the verdict.
  const qualityCaller = object(object(workflow.jobs)["frozen-candidate-quality"]);
  add(
    violations,
    sameMembers(needs(qualityCaller), [
      "route",
      "packaged-proof",
      "macos-metal-proof",
    ])
      && qualityCaller.if
        === "always() && needs.route.result == 'success' && needs.packaged-proof.result == 'success' && needs.macos-metal-proof.result == 'success' && needs.route.outputs.mode == 'qualification' && (needs.route.outputs.scope == 'macos' || needs.route.outputs.scope == 'full')"
      && qualityCaller.uses === frozenCandidateQualityWorkflowRef
      && hasExactKeys(qualityCaller, ["name", "if", "needs", "uses", "with"])
      && qualityCaller.secrets === undefined
      && hasExactKeys(object(qualityCaller.with), ["ref", "version"])
      && object(qualityCaller.with).ref === "${{ needs.route.outputs.head_sha }}"
      && object(qualityCaller.with).version === "${{ needs.route.outputs.version }}",
    `${file} optional quality must call its isolated owner once after protected Metal`,
  );
  const qualityFile = path.basename(frozenCandidateQualityWorkflowRef);
  const qualityWorkflow = workflows.get(qualityFile);
  if (!qualityWorkflow) {
    violations.push(`${qualityFile} must exist`);
    return;
  }
  const qualityCall = object(trigger(qualityWorkflow, "workflow_call"));
  const qualityInputs = object(qualityCall.inputs);
  add(
    violations,
    trigger(qualityWorkflow, "workflow_dispatch") === undefined
      && hasExactKeys(qualityInputs, ["ref", "version"])
      && object(qualityInputs.ref).required === true
      && object(qualityInputs.ref).type === "string"
      && object(qualityInputs.version).required === true
      && object(qualityInputs.version).type === "string"
      && JSON.stringify(qualityWorkflow.permissions)
        === JSON.stringify({ actions: "read", contents: "read" })
      && sameMembers(Object.keys(object(qualityWorkflow.jobs)), [
        "quality",
        "windows-quality",
        "linux-quality",
      ]),
    `${qualityFile} must remain a reusable-only, read-only evaluation owner`,
  );
  // The quality job's structural pin (job existence) is a rule instance and
  // lives in release-claims.json under workflow_policy.structural_pins;
  // structuralPinViolations evaluates it. Its candidate cache restore and
  // miss-only transfer step fragments are rule instances and live in
  // release-claims.json under workflow_policy.step_fragments;
  // stepFragmentViolations evaluates them.
  const quality = object(object(qualityWorkflow.jobs).quality);
  add(
    violations,
    JSON.stringify(quality["runs-on"])
        === JSON.stringify(["self-hosted", "macOS", "ARM64", "codestory-metal"])
      && quality.environment === "macos-metal-release"
      && quality["continue-on-error"] === true
      && quality["timeout-minutes"] === 60,
    `${qualityFile} optional quality must stay nonblocking on protected Metal`,
  );
  const qualitySteps = list(quality.steps).map(object);
  add(
    violations,
    qualitySteps.length === 11
      && qualitySteps.filter(step => step.id === "quality").length === 1
      && qualitySteps.filter(step => step.id === "quality-upload").length === 1
      && qualitySteps.filter(step => step.id === "ripgrep-quality").length === 1
      && qualitySteps.filter(step => step.id === "ripgrep-quality-upload").length === 1,
    `${qualityFile} must retain authenticated Axios and Ripgrep measurement boundaries`,
  );
  const qualityCheckout = namedStep(quality, "Checkout exact frozen candidate");
  add(
    violations,
    qualityCheckout?.uses === "actions/checkout@v5"
      && hasExactKeys(object(qualityCheckout?.with), ["ref", "fetch-depth"])
      && object(qualityCheckout?.with).ref === "${{ inputs.ref }}"
      && object(qualityCheckout?.with)["fetch-depth"] === 0,
    `${qualityFile} must check out the routed exact frozen candidate`,
  );
  const qualityAuthentication = namedStep(
    quality,
    "Authenticate exact candidate archive artifacts",
  );
  const qualityAuthenticationRun = shellLiteralNormalizedText(
    stepRun(quality, "Authenticate exact candidate archive artifacts"),
  );
  add(
    violations,
    qualityAuthentication?.id === "candidate-artifacts"
      && qualityAuthentication?.shell === "bash"
      && qualityAuthentication?.["continue-on-error"] === undefined
      && hasExactKeys(object(qualityAuthentication?.env), ["GH_TOKEN", "HEAD_SHA"])
      && object(qualityAuthentication?.env).GH_TOKEN === "${{ github.token }}"
      && object(qualityAuthentication?.env).HEAD_SHA === "${{ inputs.ref }}"
      && qualityAuthenticationRun.includes("git rev-parse HEAD")
      && qualityAuthenticationRun.includes(".head_repository.full_name")
      && qualityAuthenticationRun.includes(
        ".github/workflows/packaged-platform-pr.yml",
      )
      && qualityAuthenticationRun.includes(".head_sha")
      && qualityAuthenticationRun.includes(".run_attempt")
      && qualityAuthenticationRun.includes("select_artifact codestory-cli-macos-arm64")
      && qualityAuthenticationRun.includes(
        "select_artifact codestory-candidate-archive-record-macos-arm64",
      )
      && qualityAuthenticationRun.includes(".workflow_run.id == $run_id")
      && qualityAuthenticationRun.includes(".workflow_run.head_sha == $sha")
      && qualityAuthenticationRun.includes("package-id=$artifact_id")
      && qualityAuthenticationRun.includes("package-bytes=$expected_size")
      && qualityAuthenticationRun.includes(
        "package-sha256=${expected_digest#sha256:}",
      ),
    `${qualityFile} must authenticate one current-run exact-head candidate archive and record`,
  );
  const qualityRecordDownload = namedStep(
    quality,
    "Download authenticated candidate record",
  );
  const qualityCacheRestore = namedStep(
    quality,
    "Restore exact candidate archive from protected host",
  );
  const qualityCacheMiss = namedStep(
    quality,
    "Download, authenticate, and admit candidate archive on miss",
  );
  add(
    violations,
    qualityRecordDownload?.uses === "actions/download-artifact@v8.0.1"
      && hasExactKeys(object(qualityRecordDownload?.with), ["name", "path"])
      && object(qualityRecordDownload?.with).name
        === "codestory-candidate-archive-record-macos-arm64"
      && object(qualityRecordDownload?.with).path
        === "target/candidate-archive-record/macos-arm64"
      && qualityCacheRestore?.id === "candidate-cache"
      && qualityCacheRestore?.if === undefined
      && qualityCacheRestore?.shell === "bash"
      && qualityCacheRestore?.["continue-on-error"] === undefined,
    `${qualityFile} cache lookup must consume only the exact small candidate record`,
  );
  add(
    violations,
    qualityCacheMiss?.if === "steps.candidate-cache.outputs.hit != 'true'"
      && qualityCacheMiss?.shell === "bash"
      && qualityCacheMiss?.["continue-on-error"] === undefined
      && hasExactKeys(object(qualityCacheMiss?.env), [
        "ARTIFACT_ID",
        "EXPECTED_SHA256",
        "EXPECTED_SIZE",
        "GH_TOKEN",
      ])
      && object(qualityCacheMiss?.env).ARTIFACT_ID
        === "${{ steps.candidate-artifacts.outputs.package-id }}"
      && object(qualityCacheMiss?.env).EXPECTED_SIZE
        === "${{ steps.candidate-artifacts.outputs.package-bytes }}"
      && object(qualityCacheMiss?.env).EXPECTED_SHA256
        === "${{ steps.candidate-artifacts.outputs.package-sha256 }}"
      && object(qualityCacheMiss?.env).GH_TOKEN === "${{ github.token }}",
    `${qualityFile} archive transfer must be cache-miss-only and outer-digest authenticated`,
  );
  const qualityProducer = qualitySteps.find(step => step.id === "quality");
  const qualityProducerRun = shellLiteralNormalizedText(
    String(qualityProducer?.run ?? ""),
  );
  add(
    violations,
    qualityProducer?.id === "quality"
      && qualityProducer?.["continue-on-error"] === true
      && qualityProducer?.shell === "bash"
      && hasExactKeys(object(qualityProducer?.env), [
        "VERSION",
        "CODESTORY_EMBED_ALLOW_CPU",
      ])
      && object(qualityProducer?.env).VERSION === "${{ inputs.version }}"
      && object(qualityProducer?.env).CODESTORY_EMBED_ALLOW_CPU === "0"
      && qualityProducerRun.includes(
        "target/release-dist/codestory-cli-v${version}-macos-arm64.tar.gz",
      )
      && occurrenceCount(
        qualityProducerRun,
        "CODESTORY_RELEASE_EVIDENCE_CORPUS_ID=",
      ) === 1
      && occurrenceCount(
        qualityProducerRun,
        "CODESTORY_RELEASE_EVIDENCE_CORPUS_CONTRACT=",
      ) === 1
      && occurrenceCount(
        qualityProducerRun,
        "CODESTORY_RELEASE_EVIDENCE_CACHE_ID=",
      ) === 1
      && shellInvocationsContaining(qualityProducerRun, "--packet-runtime").length === 1
      && qualityProducerRun.includes("--packet-runtime")
      && qualityProducerRun.includes("--packet-runtime-mode cold-cli")
      && occurrenceCount(qualityProducerRun, "--task-manifest") === 1
      && !qualityProducerRun.includes("--task-suite")
      && !qualityProducerRun.includes("--task-ids")
      && qualityProducerRun.includes("--materialize-repos")
      && qualityProducerRun.includes("--repeats 3")
      && qualityProducerRun.includes("--publishable")
      && qualityProducerRun.includes("--max-source-reads-after-packet 0")
      && qualityProducerRun.includes("--codestory-cli $packaged_cli")
      && qualityProducerRun.includes("--timeout-ms 180000")
      && qualityProducerRun.includes("--out-dir $quality_root/packet"),
    `${qualityFile} must run exactly one pinned three-repeat publishable evaluator`,
  );
  const ripgrepQualityProducer = qualitySteps.find(step => step.id === "ripgrep-quality");
  const ripgrepQualityProducerRun = shellLiteralNormalizedText(
    String(ripgrepQualityProducer?.run ?? ""),
  );
  add(
    violations,
    ripgrepQualityProducer?.id === "ripgrep-quality"
      && ripgrepQualityProducer?.["continue-on-error"] === true
      && ripgrepQualityProducer?.shell === "bash"
      && object(ripgrepQualityProducer?.env).CODESTORY_EMBED_ALLOW_CPU === "0"
      && ripgrepQualityProducerRun.includes("--packet-runtime-mode cold-cli")
      && occurrenceCount(ripgrepQualityProducerRun, "--task-manifest") === 1
      && !ripgrepQualityProducerRun.includes("--task-suite")
      && !ripgrepQualityProducerRun.includes("--task-ids")
      && ripgrepQualityProducerRun.includes("--repeats 3")
      && ripgrepQualityProducerRun.includes("--publishable")
      && ripgrepQualityProducerRun.includes("--max-source-reads-after-packet 0")
      && ripgrepQualityProducerRun.includes("--timeout-ms 180000"),
    `${qualityFile} optional macOS Ripgrep quality must be one isolated non-gating pinned evaluator`,
  );
  const qualityUpload = qualitySteps.find(step => step.id === "quality-upload");
  const qualityOutcome = namedStep(quality, "Record optional quality outcome");
  add(
    violations,
    qualityUpload?.id === "quality-upload"
      && qualityUpload?.if === "steps.quality.outcome == 'success'"
      && qualityUpload?.["continue-on-error"] === true
      && qualityUpload?.uses === "actions/upload-artifact@v7.0.1"
      && hasExactKeys(object(qualityUpload?.with), [
        "name",
        "path",
        "if-no-files-found",
        "retention-days",
        "overwrite",
      ])
      && object(qualityUpload?.with).name
        === "frozen-candidate-quality-${{ inputs.ref }}"
      && object(qualityUpload?.with).path
        === "target/frozen-candidate-quality/evidence"
      && object(qualityUpload?.with)["if-no-files-found"] === "error"
      && object(qualityUpload?.with)["retention-days"] === 30
      && object(qualityUpload?.with).overwrite === true
      && qualityOutcome?.if === "always()"
      && qualityOutcome?.shell === "bash"
      && qualityOutcome?.["continue-on-error"] === undefined
      && hasExactKeys(object(qualityOutcome?.env), [
        "QUALITY_OUTCOME",
        "RIPGREP_QUALITY_OUTCOME",
        "RIPGREP_UPLOAD_OUTCOME",
        "UPLOAD_OUTCOME",
      ])
      && object(qualityOutcome?.env).QUALITY_OUTCOME
        === "${{ steps.quality.outcome }}"
      && object(qualityOutcome?.env).UPLOAD_OUTCOME
        === "${{ steps.quality-upload.outcome }}"
      && stepRun(quality, "Record optional quality outcome").includes(
        'echo "- Release or qualification gate: \\`false\\`"',
      ),
    `${qualityFile} must report both outcomes without becoming a qualification or release gate`,
  );
  // The windows-quality job's structural pin (job existence) is a rule
  // instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it.
  const windowsQuality = object(object(qualityWorkflow.jobs)["windows-quality"]);
  const windowsQualitySteps = list(windowsQuality.steps).map(object);
  const windowsQualityProducer = windowsQualitySteps.find(
    step => step.id === "windows-quality",
  );
  const windowsQualityRun = shellLiteralNormalizedText(
    String(windowsQualityProducer?.run ?? ""),
  );
  const windowsQualityUpload = windowsQualitySteps.find(
    step => step.id === "windows-quality-upload",
  );
  add(
    violations,
    windowsQuality.name === "Optional frozen-candidate Windows x64 Ripgrep v2 quality"
      && JSON.stringify(windowsQuality["runs-on"])
        === JSON.stringify(["self-hosted", "Windows", "X64", "codestory-vulkan"])
      && windowsQuality.environment === undefined
      && windowsQuality["continue-on-error"] === true
      && windowsQuality["timeout-minutes"] === 60
      && windowsQualitySteps.length === 6,
    `${qualityFile} optional Windows x64 quality must remain non-gating on Vulkan`,
  );
  add(
    violations,
    namedStep(windowsQuality, "Checkout exact frozen candidate")?.uses
        === "actions/checkout@v5"
      && namedStep(windowsQuality, "Download exact Windows x64 candidate archive")?.uses
        === "actions/download-artifact@v8.0.1"
      && object(
        namedStep(windowsQuality, "Download exact Windows x64 candidate archive")?.with,
      ).name === "codestory-cli-windows-x64"
      && windowsQualityProducer?.["continue-on-error"] === true
      && object(windowsQualityProducer?.env).CODESTORY_EMBED_ALLOW_CPU === "0"
      && windowsQualityRun.includes("--packet-runtime-mode cold-cli")
      && occurrenceCount(windowsQualityRun, "--task-manifest") === 1
      && windowsQualityRun.includes("--repeats 3")
      && windowsQualityRun.includes("--publishable")
      && !windowsQualityRun.includes("--task-suite")
      && !windowsQualityRun.includes("--task-ids")
      && windowsQualityUpload?.["continue-on-error"] === true
      && windowsQualityUpload?.if === "steps.windows-quality.outcome == 'success'"
      && namedStep(windowsQuality, "Record optional Windows x64 quality outcome")?.if
        === "always()",
    `${qualityFile} optional Windows x64 Ripgrep quality must retain isolated non-gating measurement and upload boundaries`,
  );
  // The linux-quality job's structural pin (job existence) is a rule instance
  // and lives in release-claims.json under workflow_policy.structural_pins;
  // structuralPinViolations evaluates it.
  const linuxQuality = object(object(qualityWorkflow.jobs)["linux-quality"]);
  const linuxQualitySteps = list(linuxQuality.steps).map(object);
  const linuxQualityProducer = linuxQualitySteps.find(step => step.id === "linux-quality");
  const linuxQualityRun = shellLiteralNormalizedText(String(linuxQualityProducer?.run ?? ""));
  const optionalLinuxQualityUpload = namedStep(
    linuxQuality,
    "Upload optional Linux x64 Axios v2 quality evidence",
  );
  add(
    violations,
    linuxQuality.name === "Optional frozen-candidate Linux x64 Axios v2 quality"
      && JSON.stringify(linuxQuality["runs-on"])
        === JSON.stringify(["self-hosted", "Linux", "X64", "codestory-linux-vulkan"])
      && linuxQuality.environment === undefined
      && linuxQuality["continue-on-error"] === true
      && linuxQuality["timeout-minutes"] === 60
      && linuxQualitySteps.length === 6,
    `${qualityFile} optional Linux x64 quality must remain a non-gating unpinned smoke`,
  );
  add(
    violations,
    namedStep(linuxQuality, "Checkout exact frozen candidate")?.uses === "actions/checkout@v5"
      && namedStep(linuxQuality, "Download exact Linux x64 candidate archive")?.uses
        === "actions/download-artifact@v8.0.1"
      && object(namedStep(linuxQuality, "Download exact Linux x64 candidate archive")?.with).name
        === "codestory-cli-linux-x64"
      && linuxQualityProducer?.["continue-on-error"] === true
      && object(linuxQualityProducer?.env).CODESTORY_EMBED_ALLOW_CPU === "0"
      && linuxQualityRun.includes("codestory-cli-v${version}-linux-x64.tar.gz")
      && linuxQualityRun.includes("--packet-runtime-mode cold-cli")
      && occurrenceCount(linuxQualityRun, "--task-manifest") === 1
      && linuxQualityRun.includes("--repeats 3")
      && linuxQualityRun.includes("--publishable")
      && !linuxQualityRun.includes("--task-suite")
      && !linuxQualityRun.includes("--task-ids")
      && optionalLinuxQualityUpload?.if === "steps.linux-quality.outcome == 'success'"
      && optionalLinuxQualityUpload?.["continue-on-error"] === true
      && object(optionalLinuxQualityUpload?.with).name
        === "frozen-candidate-linux-x64-quality-${{ inputs.ref }}-attempt-${{ github.run_attempt }}"
      && object(optionalLinuxQualityUpload?.with).overwrite === true,
    `${qualityFile} optional Linux x64 quality must run the same isolated Axios v2 smoke entrypoint`,
  );
  // The windows-vulkan-proof job's structural pin (job existence) and its
  // needs edge are rule instances and live in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates them
  // with their reviewed violation text.
  const vulkan = object(object(workflow.jobs)["windows-vulkan-proof"]);
  add(
    violations,
    String(vulkan.if ?? "").includes("needs.route.outputs.mode != 'package'")
      && !String(vulkan.if ?? "").includes("needs.macos-metal-proof"),
    `${file} package-only mode must skip Windows without serializing it behind Metal`,
  );
  add(violations, object(vulkan.with).use_packaged_cli_artifact === true, `${file} Vulkan proof must use the packaged CLI`);
  add(
    violations,
    object(vulkan.with).candidate_installed_proof
      === "${{ needs.route.outputs.mode != 'qualification' }}",
    `${file} qualification must run full Windows proof rather than candidate-installed proof`,
  );
  add(
    violations,
    object(vulkan.with).quality_evidence_artifact === undefined,
    `${file} Windows qualification must not consume optional quality evidence`,
  );
  add(
    violations,
    object(vulkan.with).server_behavior_only
      === "${{ needs.route.outputs.mode != 'qualification' }}",
    `${file} qualification must run full Windows lifecycle and fault proof`,
  );
  // The linux-vulkan-proof job's structural pin (job existence) and its needs
  // edge are rule instances and live in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates them
  // with their reviewed violation text.
  const linuxVulkan = object(object(workflow.jobs)["linux-vulkan-proof"]);
  add(
    violations,
    String(linuxVulkan.if ?? "").includes("needs.route.outputs.mode != 'package'")
      && String(linuxVulkan.if ?? "").includes(
        "needs.route.outputs.mode != 'qualification' || inputs.qualify_linux_vulkan",
      ),
    `${file} package mode must skip Linux proof; qualification must schedule Linux proof only when the explicit lifecycle opt-in is true`,
  );
  add(
    violations,
    linuxVulkan.uses === "./.github/workflows/linux-vulkan-proof.yml",
    `${file} Linux proof must use the protected Vulkan workflow`,
  );
  add(
    violations,
    object(linuxVulkan.with).calibration_bundle_artifact
        === "${{ inputs.calibration_bundle_artifact || '' }}"
      && object(linuxVulkan.with).calibration_bundle_run_id
        === "${{ inputs.calibration_bundle_run_id || '' }}"
      && object(linuxVulkan.with).candidate_installed_proof
        === "${{ needs.route.outputs.mode != 'qualification' }}"
      && object(linuxVulkan.with).server_behavior_only
        === "${{ needs.route.outputs.mode != 'qualification' }}"
      && object(linuxVulkan.with).candidate_producer_workflow_path
        === ".github/workflows/packaged-platform-pr.yml",
    `${file} Linux proof must keep platform mode candidate-installed and run full lifecycle only in qualification with the authenticated calibration bundle`,
  );
  // The closeout job's structural pin (job existence) and its needs edge are
  // rule instances and live in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates them
  // with their reviewed violation text. The closeout's mode-shape proof
  // fragments are rule instances and live in release-claims.json under
  // workflow_policy.step_fragments; stepFragmentViolations evaluates them.
  const closeout = object(object(workflow.jobs).closeout);
  add(
    violations,
    closeout.if
      === "always() && needs.route.result != 'skipped' && needs.route.outputs.mode != 'calibration'"
      && closeout["runs-on"] === "ubuntu-latest"
      && closeout["timeout-minutes"] === 65
      && closeout["continue-on-error"] === undefined,
    `${file} closeout job must retain its reviewed unconditional result-checking activation`,
  );
  add(
    violations,
    !needs(closeout).includes("frozen-candidate-quality")
      && !scalarStrings(closeout).some(value =>
        value.includes("QUALITY_RESULT")
          || value.includes("frozen-candidate-quality")
      ),
    `${file} normal closeout must not depend on optional quality evidence`,
  );
  const closeoutProofName = "Require one coherent accepted proof";
  const closeoutProof = namedStep(closeout, closeoutProofName);
  const closeoutRun = executableRunText(stepRun(closeout, closeoutProofName));
  add(
    violations,
    list(closeout.steps).length === 3
      && closeoutProof?.if === undefined
      && closeoutProof?.["continue-on-error"] === undefined
      && closeoutProof?.shell === "bash"
      && closeoutProof?.["working-directory"] === undefined,
    `${file} closeout must retain one unconditional result proof plus the two opted-in Linux quality receipt steps`,
  );
  const expectedCloseoutEnv = {
    GH_TOKEN: "${{ github.token }}",
    HEAD_SHA: "${{ needs.route.outputs.head_sha }}",
    MODE: "${{ needs.route.outputs.mode }}",
    QUALIFY_LINUX_VULKAN: "${{ inputs.qualify_linux_vulkan }}",
    SCOPE: "${{ needs.route.outputs.scope }}",
    ROUTE_RESULT: "${{ needs.route.result }}",
    SOURCE_RESULT: "${{ needs.source-proof.result }}",
    PACKAGE_RESULT: "${{ needs.packaged-proof.result }}",
    METAL_RESULT: "${{ needs.macos-metal-proof.result }}",
    WINDOWS_VULKAN_RESULT: "${{ needs.windows-vulkan-proof.result }}",
    LINUX_VULKAN_RESULT: "${{ needs.linux-vulkan-proof.result }}",
  };
  const closeoutEnv = object(closeoutProof?.env);
  add(
    violations,
    hasExactKeys(closeoutEnv, Object.keys(expectedCloseoutEnv))
      && Object.entries(expectedCloseoutEnv).every(
        ([key, value]) => closeoutEnv[key] === value,
      ),
    `${file} closeout proof must bind every route and platform result from the reviewed jobs exactly`,
  );
  add(
    violations,
    /if\s+\[\s*"\$MODE"\s*=\s*qualification\s*\];\s*then\s+if\s+\[\s*"\$QUALIFY_LINUX_VULKAN"\s*=\s*true\s*\];\s*then\s+require_result\s+"\$LINUX_VULKAN_RESULT"\s+success\s+linux-vulkan-proof\s+else\s+require_result\s+"\$LINUX_VULKAN_RESULT"\s+skipped\s+linux-vulkan-proof\s+fi\s+else\s+require_result\s+"\$LINUX_VULKAN_RESULT"\s+success\s+linux-vulkan-proof\s+fi/um
      .test(closeoutRun),
    `${file} qualification closeout must require Linux success exactly when the explicit lifecycle opt-in is true`,
  );
  const linuxQualityProofName = "Validate opted-in Linux quality evidence";
  const linuxQualityProof = namedStep(closeout, linuxQualityProofName);
  const strictLinuxQualityRun = executableRunText(stepRun(closeout, linuxQualityProofName));
  add(
    violations,
    linuxQualityProof?.if === "inputs.qualify_linux_vulkan"
      && linuxQualityProof?.shell === "bash"
      && linuxQualityProof?.["continue-on-error"] === undefined
      && hasExactKeys(object(linuxQualityProof?.env), ["GH_TOKEN", "HEAD_SHA"])
      && object(linuxQualityProof?.env).GH_TOKEN === "${{ github.token }}"
      && object(linuxQualityProof?.env).HEAD_SHA === "${{ needs.route.outputs.head_sha }}",
    `${file} opted-in Linux quality validation must fail closed on the routed frozen head`,
  );
  add(
    violations,
    [
      "frozen-candidate-linux-x64-quality-$HEAD_SHA-attempt-$GITHUB_RUN_ATTEMPT",
      "actions/runs/$GITHUB_RUN_ID/artifacts?per_page=100",
      ".workflow_run.id == $run_id",
      ".workflow_run.head_sha == $sha",
      "gh run download \"$GITHUB_RUN_ID\"",
      "packet-runtime-summary.json",
      "packet-runtime-runs.jsonl",
      "publishable-three-repeat-packet/v1",
      "optional-linux-x64-quality",
      "codestory-release-corpus-v0.16-axios-js-ts-v2",
      'and .release_evidence.quality_gate_status == "pass"',
      "and .release_evidence.publishable_blockers == []",
      "and (.release_evidence.rows | length) == 3",
      'and .task_id == "axios-request-dispatch-v2"',
      'and .sufficiency.status == "sufficient"',
      'and .sufficiency.retrieval_mode == "full"',
      "and .packet_latency.sla_missed == false",
      'and .packet_latency.retrieval_shadow.retrieval_mode == "full"',
      "and .repo_provenance.git_dirty == false",
      "and .codestory_cache_provenance.local_only == true",
      "cmp \"$evidence_root/summary-rows.json\" \"$evidence_root/raw-rows.json\"",
      "codestory.strict-linux-quality-validation/v1",
    ].every(fragment => strictLinuxQualityRun.includes(fragment)),
    `${file} opted-in Linux quality validation must authenticate and inspect every three-repeat publishable predicate`,
  );
  const linuxQualityUpload = namedStep(closeout, "Upload opted-in Linux quality validation");
  add(
    violations,
    linuxQualityUpload?.if === "inputs.qualify_linux_vulkan"
      && linuxQualityUpload?.uses === "actions/upload-artifact@v7.0.1"
      && hasExactKeys(object(linuxQualityUpload?.with), [
        "name",
        "path",
        "if-no-files-found",
        "retention-days",
      ])
      && object(linuxQualityUpload?.with).name
        === "strict-linux-quality-validation-${{ needs.route.outputs.head_sha }}-attempt-${{ github.run_attempt }}"
      && object(linuxQualityUpload?.with).path
        === "target/strict-linux-quality/receipt.json"
      && object(linuxQualityUpload?.with)["if-no-files-found"] === "error"
      && object(linuxQualityUpload?.with)["retention-days"] === 30,
    `${file} opted-in Linux quality validation must retain its exact-head receipt`,
  );
  add(violations, !scalarStrings(workflow).some(value => value === "./.github/workflows/release.yml"), `${file} must not publish releases`);
}

function validateRemainingWorkflows(workflows, violations) {
  const autoFile = "auto-release.yml";
  const auto = workflows.get(autoFile);
  if (!auto) {
    violations.push(`${autoFile} must exist`);
  } else {
    requireNoCalibrationReferences(violations, autoFile, auto);
    add(violations, includesAll(at(auto, "on", "push", "branches"), ["main"]), `${autoFile} must run on main`);
    add(violations, includesAll(at(auto, "on", "push", "paths"), [
      "package.json",
      "package-lock.json",
      "release-claims.json",
      ".github/actionlint.yaml",
      ".github/workflows/**",
      "scripts/codestory-release-*.mjs",
      "scripts/tests/codestory-release-*.test.mjs",
    ]), `${autoFile} must observe policy dependency and release-claim changes`);
    add(
      violations,
      object(auto.jobs)["workflow-policy"] === undefined,
      `${autoFile} must delegate the release policy gate to release.yml exactly once`,
    );
    // The version-detection and release jobs' structural pins - their existence,
    // the release job's needs edge, and the release caller's permission grants -
    // are rule instances and live in release-claims.json under
    // workflow_policy.structural_pins; structuralPinViolations evaluates them.
    const detectVersion = object(object(auto.jobs)["detect-version"]);
    add(
      violations,
      needs(detectVersion).length === 0,
      `${autoFile} version detection must not depend on a duplicate policy gate`,
    );
    add(
      violations,
      namedStep(detectVersion, "Validate synchronized release version") === undefined,
      `${autoFile} must delegate synchronized version validation to release.yml`,
    );
    const release = object(object(auto.jobs).release);
    add(violations, release.uses === "./.github/workflows/release.yml", `${autoFile} must call the release workflow`);
    add(violations, object(release.with).publish_release === true, `${autoFile} trusted main caller must explicitly authorize publication`);
    add(
      violations,
      release.secrets === "inherit",
      `${autoFile} release caller must inherit repository release secrets`,
    );
  }

  const metalFile = "macos-metal-proof.yml";
  const metal = workflows.get(metalFile);
  if (!metal) {
    violations.push(`${metalFile} must exist`);
  } else {
    add(
      violations,
      trigger(metal, "workflow_call") !== undefined
        && trigger(metal, "workflow_dispatch") === undefined,
      `${metalFile} must be coordinator-only and not directly dispatchable`,
    );
    for (const event of ["workflow_call"]) {
      for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
        requireOptionalStringInput(violations, metalFile, metal, event, key);
      }
      add(
        violations,
        at(metal, "on", event, "inputs", "quality_evidence_artifact") === undefined,
        `${metalFile} ${event} must not accept optional quality evidence`,
      );
    }
    const candidateInput = object(at(
      metal,
      "on",
      "workflow_call",
      "inputs",
      "candidate_installed_proof",
    ));
    add(
      violations,
      candidateInput.required === false
        && candidateInput.type === "boolean"
        && candidateInput.default === false,
      `${metalFile} candidate-installed proof must be an explicit opt-in`,
    );
    for (const event of ["workflow_call", "workflow_dispatch"]) {
      add(
        violations,
        at(metal, "on", event, "inputs", "candidate_installed_only") === undefined,
        `${metalFile} ${event} must not define candidate_installed_only`,
      );
    }
    const candidateProducerInput = object(at(
      metal,
      "on",
      "workflow_call",
      "inputs",
      "candidate_producer_workflow_path",
    ));
    add(
      violations,
      candidateProducerInput.required === false
        && candidateProducerInput.type === "string"
        && candidateProducerInput.default === ".github/workflows/packaged-platform-pr.yml",
      `${metalFile} candidate producer path must default to the exact PR coordinator`,
    );
    const serverBehaviorInput = object(at(
      metal,
      "on",
      "workflow_call",
      "inputs",
      "server_behavior_only",
    ));
    add(
      violations,
      serverBehaviorInput.required === false
        && serverBehaviorInput.type === "boolean"
        && serverBehaviorInput.default === false,
      `${metalFile} server-behavior-only claim scope must be an explicit opt-in`,
    );
    // The packaged-metal job's structural pin (job existence) is a rule instance
    // and lives in release-claims.json under workflow_policy.structural_pins;
    // structuralPinViolations evaluates it. The lane's step fragments - mode
    // validation, model preparation, host evidence, candidate authentication and
    // cache handling, calibration-producer authentication, the calibration
    // collector, the protected and candidate-installed proofs, and the release
    // cells - are rule instances and live in release-claims.json under
    // workflow_policy.step_fragments; stepFragmentViolations evaluates them.
    const job = object(object(metal.jobs)["packaged-metal"]);
    add(violations, JSON.stringify(job["runs-on"]) === JSON.stringify(["self-hosted", "macOS", "ARM64", "codestory-metal"]), `${metalFile} must use the protected Apple Silicon runner`);
    add(violations, job.environment === "macos-metal-release", `${metalFile} must use the protected Metal environment`);
    const validateCandidate = namedStep(job, "Validate candidate-installed mode");
    add(
      violations,
      validateCandidate?.if === "inputs.candidate_installed_proof"
        && validateCandidate?.shell === "bash",
      `${metalFile} candidate-installed validation must be an explicit Bash boundary`,
    );
    requireStepEnv(violations, metalFile, job, "Validate candidate-installed mode", {
      SERVER_BEHAVIOR_ONLY: "${{ inputs.server_behavior_only }}",
      CALIBRATION_MODE: "${{ inputs.calibration_mode }}",
    });
    const calibrationClock = namedStep(
      job,
      "Start Metal constant calibration clock",
    );
    const modelPreparation = namedStep(job, "Prepare checksum-pinned embedded model");
    add(
      violations,
      calibrationClock?.if === "inputs.calibration_mode"
        && calibrationClock?.id === "calibration-clock"
        && calibrationClock?.shell === "bash"
        && occurrenceCount(String(calibrationClock?.run ?? ""), "time.monotonic_ns()") === 1
        && String(calibrationClock?.run ?? "").includes(
          'echo "started-ns=',
        )
        && String(calibrationClock?.run ?? "").includes(">> \"$GITHUB_OUTPUT\"")
        && modelPreparation?.id === "model-prepare"
        && modelPreparation?.if === "${{ !inputs.use_packaged_cli_artifact }}"
        && modelPreparation?.shell === "bash"
        && occurrenceCount(
          String(modelPreparation?.run ?? ""),
          "time.monotonic_ns()",
        ) === 2
        && String(modelPreparation?.run ?? "").includes("duration-ms=")
        && String(modelPreparation?.run ?? "").includes(">> \"$GITHUB_OUTPUT\"")
        && stepIndex(job, "Start Metal constant calibration clock")
          < stepIndex(job, "Prepare checksum-pinned embedded model"),
      `${metalFile} calibration must time model preparation and total wall time from one explicit clock`,
    );
    add(
      violations,
      namedStep(job, "Install pinned Rust")?.if
        === "${{ !inputs.use_packaged_cli_artifact }}",
      `${metalFile} every packaged proof must skip Rust installation`,
    );
    add(
      violations,
      namedStep(job, "Build qualification driver") === undefined,
      `${metalFile} must not rebuild the qualification driver after package download`,
    );
    const nativeBuild = namedStep(job, "Build and package native CLI");
    const nativeBuildRun = executableRunText(String(nativeBuild?.run ?? ""));
    const normalizedNativeBuildRun = shellLiteralNormalizedText(nativeBuildRun);
    add(
      violations,
      hasExactKeys(object(nativeBuild?.env), [
        "VERSION",
        "CALIBRATION_MODE",
        "SERVER_BEHAVIOR_ONLY",
      ])
        && object(nativeBuild?.env).CALIBRATION_MODE === "${{ inputs.calibration_mode }}"
        && object(nativeBuild?.env).SERVER_BEHAVIOR_ONLY
          === "${{ inputs.server_behavior_only }}"
        && nativeBuild?.id === "native-build-package"
        && shellInvocationsContaining(normalizedNativeBuildRun, "cargo build").length === 1
        && normalizedNativeBuildRun.includes("-p codestory-cli")
        && normalizedNativeBuildRun.includes("-p codestory-bench")
        && normalizedNativeBuildRun.includes("--bin codestory-cli")
        && normalizedNativeBuildRun.includes("--bin codestory-cli-runtime")
        && normalizedNativeBuildRun.includes("--bin codestory_embedding_constant_calibration")
        && normalizedNativeBuildRun.includes(
          "--bin codestory_embedding_qualification",
        )
        && normalizedNativeBuildRun.includes("if [ $CALIBRATION_MODE = true ]")
        && normalizedNativeBuildRun.includes(
          "elif [ $SERVER_BEHAVIOR_ONLY != true ]",
        )
        && occurrenceCount(
          normalizedNativeBuildRun,
          "--bin codestory_embedding_qualification",
        ) === 1
        && occurrenceCount(
          normalizedNativeBuildRun,
          "--bin codestory_embedding_constant_calibration",
        ) === 1
        && shellInvocationsContaining(
          normalizedNativeBuildRun,
          "python3 .github/scripts/package-codestory-release.py",
        ).length === 1
        && jobShellInvocationsContaining(job, "cargo build").length === 1
        && jobShellInvocationsContaining(
          job,
          ".github/scripts/package-codestory-release.py",
        ).length === 1
        && jobShellInvocationsContaining(
          job,
          "node scripts/prepare-embedded-model.mjs",
        ).length === 1
        && jobShellInvocationsContaining(
          job,
          ".github/scripts/check-packaged-agent-proof.py",
        ).length === 4
        && occurrenceCount(normalizedNativeBuildRun, "time.monotonic_ns()") === 2
        && normalizedNativeBuildRun.includes("duration-ms=")
        && normalizedNativeBuildRun.includes(">> $GITHUB_OUTPUT"),
      `${metalFile} calibration must build CLI and constant collector once through one shared Cargo invocation and package once`,
    );
    const candidateAuthentication = namedStep(
      job,
      "Authenticate exact candidate artifacts",
    );
    const recordDownload = namedStep(
      job,
      "Download authenticated candidate record",
    );
    const cacheRestore = namedStep(
      job,
      "Restore exact candidate archive from protected host",
    );
    const cacheMiss = namedStep(
      job,
      "Download, authenticate, and admit candidate archive on miss",
    );
    const driverDownload = namedStep(
      job,
      "Download separate authenticated qualification driver",
    );
    add(
      violations,
      candidateAuthentication?.id === "candidate-artifacts"
        && candidateAuthentication?.if === "inputs.use_packaged_cli_artifact"
        && candidateAuthentication?.shell === "bash"
        && candidateAuthentication?.["continue-on-error"] === undefined
        && hasExactKeys(object(candidateAuthentication?.env), [
          "ARTIFACT_NAME",
          "CANDIDATE_PRODUCER_WORKFLOW_PATH",
          "CANDIDATE_RECORD_ARTIFACT",
          "GH_TOKEN",
          "QUALIFICATION_ARTIFACT",
          "SERVER_BEHAVIOR_ONLY",
        ])
        && object(candidateAuthentication?.env).GH_TOKEN === "${{ github.token }}"
        && object(candidateAuthentication?.env).ARTIFACT_NAME
          === "codestory-cli-macos-arm64"
        && object(candidateAuthentication?.env).CANDIDATE_RECORD_ARTIFACT
          === "codestory-candidate-archive-record-macos-arm64"
        && object(candidateAuthentication?.env).QUALIFICATION_ARTIFACT
          === "codestory-qualification-driver-macos-arm64"
        && object(candidateAuthentication?.env).CANDIDATE_PRODUCER_WORKFLOW_PATH
          === "${{ inputs.candidate_producer_workflow_path }}"
        && object(candidateAuthentication?.env).SERVER_BEHAVIOR_ONLY
          === "${{ inputs.server_behavior_only }}",
      `${metalFile} packaged candidate authentication must bind all exact artifacts before cache lookup`,
    );
    add(
      violations,
      recordDownload?.if === "inputs.use_packaged_cli_artifact"
        && recordDownload?.uses === "actions/download-artifact@v8.0.1"
        && hasExactKeys(object(recordDownload?.with), ["name", "path"])
        && object(recordDownload?.with).name
          === "codestory-candidate-archive-record-macos-arm64"
        && object(recordDownload?.with).path
          === "target/candidate-archive-record/macos-arm64"
        && cacheRestore?.id === "candidate-cache"
        && cacheRestore?.if === "inputs.use_packaged_cli_artifact"
        && cacheRestore?.shell === "bash"
        && cacheRestore?.["continue-on-error"] === undefined,
      `${metalFile} protected cache lookup must consume only the exact small candidate record`,
    );
    add(
      violations,
      cacheMiss?.if
        === "inputs.use_packaged_cli_artifact && steps.candidate-cache.outputs.hit != 'true'"
        && cacheMiss?.shell === "bash"
        && cacheMiss?.["continue-on-error"] === undefined
        && hasExactKeys(object(cacheMiss?.env), [
          "ARTIFACT_ID",
          "EXPECTED_SHA256",
          "EXPECTED_SIZE",
          "GH_TOKEN",
        ])
        && object(cacheMiss?.env).ARTIFACT_ID
          === "${{ steps.candidate-artifacts.outputs.package-id }}"
        && object(cacheMiss?.env).EXPECTED_SIZE
          === "${{ steps.candidate-artifacts.outputs.package-bytes }}"
        && object(cacheMiss?.env).EXPECTED_SHA256
          === "${{ steps.candidate-artifacts.outputs.package-sha256 }}"
        && object(cacheMiss?.env).GH_TOKEN === "${{ github.token }}",
      `${metalFile} large Actions artifact transfer must be a cache-miss-only authenticated boundary`,
    );
    add(
      violations,
      driverDownload?.if
        === "${{ inputs.use_packaged_cli_artifact && !inputs.calibration_mode && !inputs.server_behavior_only }}"
        && driverDownload?.uses === "actions/download-artifact@v8.0.1"
        && hasExactKeys(object(driverDownload?.with), ["name", "path"])
        && object(driverDownload?.with).name
          === "codestory-qualification-driver-macos-arm64"
        && object(driverDownload?.with).path
          === "target/qualification-driver-artifact/macos-arm64"
        && stepIndex(job, "Authenticate exact candidate artifacts")
          < stepIndex(job, "Download authenticated candidate record")
        && stepIndex(job, "Download authenticated candidate record")
          < stepIndex(job, "Restore exact candidate archive from protected host")
        && stepIndex(job, "Restore exact candidate archive from protected host")
          < stepIndex(job, "Download, authenticate, and admit candidate archive on miss")
        && stepIndex(job, "Download, authenticate, and admit candidate archive on miss")
          < stepIndex(job, "Download separate authenticated qualification driver"),
      `${metalFile} private driver must remain a separate authenticated artifact after candidate cache resolution`,
    );
    const metalDriverVerify = namedStep(job, "Verify packaged qualification driver");
    const metalDriverVerifyRun = shellLiteralNormalizedText(
      stepRun(job, "Verify packaged qualification driver"),
    );
    add(
      violations,
      metalDriverVerify?.id === "qualification-driver"
        && metalDriverVerify?.if
          === "${{ inputs.use_packaged_cli_artifact && !inputs.calibration_mode && !inputs.server_behavior_only }}"
        && metalDriverVerify?.shell === "bash"
        && metalDriverVerify?.["continue-on-error"] === undefined
        && hasExactKeys(object(metalDriverVerify?.env), ["INPUT_VERSION"])
        && object(metalDriverVerify?.env).INPUT_VERSION === "${{ inputs.version }}"
        && shellInvocationsContaining(
          metalDriverVerifyRun,
          "node .github/scripts/qualification-driver-artifact.mjs verify",
        ).length === 1
        && metalDriverVerifyRun.includes("--asset-target macos-arm64")
        && metalDriverVerifyRun.includes("--source-sha $(git rev-parse HEAD)")
        && metalDriverVerifyRun.includes("--source-tree $(git rev-parse HEAD^{tree})")
        && metalDriverVerifyRun.includes("--version $version")
        && metalDriverVerifyRun.includes(
          "--archive target/release-dist/codestory-cli-v${version}-macos-arm64.tar.gz",
        )
        && metalDriverVerifyRun.includes("--trusted-root $GITHUB_WORKSPACE")
        && metalDriverVerifyRun.includes(
          "--artifact-dir target/qualification-driver-artifact/macos-arm64",
        )
        && metalDriverVerifyRun.includes(
          "echo path=$(jq -r .driver <<<$verified) >> $GITHUB_OUTPUT",
        )
        && stepIndex(job, "Verify packaged qualification driver")
          === stepIndex(job, "Download separate authenticated qualification driver") + 1,
      `${metalFile} packaged qualification must verify the archive-bound private driver`,
    );
    add(
      violations,
      qualificationDriverHandoffIsSealed(
        job,
        "Verify packaged qualification driver",
        "Prove protected Metal runtime",
        [
          "Authenticate calibration bundle producer",
          "Download frozen calibration bundle",
        ],
      ),
      `${metalFile} must not replace the verified qualification driver before execution`,
    );
    requireCalibrationProducerBoundary(
      violations,
      metalFile,
      job,
      "${{ !inputs.calibration_mode && !inputs.server_behavior_only }}",
    );
    const calibrationPreflightName = "Validate unfrozen Metal calibration source";
    const calibrationPreflight = namedStep(job, calibrationPreflightName);
    add(
      violations,
      calibrationPreflight?.if === "inputs.calibration_mode"
        && calibrationPreflight?.shell === "bash"
        && calibrationPreflight?.["continue-on-error"] === undefined
        && stepRun(job, calibrationPreflightName).trim() === [
          "set -euo pipefail",
          'test "$(jq -r .status crates/codestory-llama-sys/per-user-embedding-server-constant-set.json)" = unfrozen',
          'test "$(jq -r .freeze_record crates/codestory-llama-sys/per-user-embedding-server-constant-set.json)" = null',
        ].join("\n")
        && stepIndex(job, calibrationPreflightName)
          === stepIndex(job, "Checkout") + 1,
      `${metalFile} must reject a frozen or stale calibration source immediately after checkout and before setup or compilation`,
    );
    const calibrationStepName = "Collect three independent Metal constant calibration runs";
    const calibrationRun = shellLiteralNormalizedText(
      stepRun(job, calibrationStepName),
    );
    add(
      violations,
      shellInvocationsContaining(
        calibrationRun,
        "python3 .github/scripts/check-packaged-agent-proof.py",
      ).length === 1
        && occurrenceCount(calibrationRun, "--collect-constant-calibration") === 1
        && !hasShellLoop(calibrationRun)
        && !calibrationRun.includes("--produce-qualification-evidence")
        && !calibrationRun.includes("--qualification-evidence")
        && !calibrationRun.includes("--calibration-run-index")
        && !calibrationRun.includes("--calibration-run-output")
        && !calibrationRun.includes("--retrieval-quality-evidence")
        && !calibrationRun.includes("--publication-fault-evidence")
        && !calibrationRun.includes("--qualification-scenario")
        && !calibrationRun.includes("--samples-per-metric")
        && !calibrationRun.includes("true_idle_exit")
        && !calibrationRun.includes("total_codestory_process_memory")
        && !calibrationRun.includes("backend_observed_accelerator_residency")
        && !calibrationRun.includes("--project")
        && !calibrationRun.includes("--plugin-root")
        && !calibrationRun.includes("--plugin-handoff"),
      `${metalFile} calibration must use one three-run synthetic-project constant collector without full qualification or nested sampling`,
    );
    const calibrationTimingName = "Publish Metal constant calibration timing";
    const calibrationTiming = namedStep(job, calibrationTimingName);
    const calibrationTimingRun = stepRun(job, calibrationTimingName);
    add(
      violations,
      calibrationTiming?.if === "inputs.calibration_mode"
        && calibrationTiming?.shell === "bash"
        && hasExactKeys(calibrationTiming?.env, [
          "CALIBRATION_STARTED_NS",
          "MODEL_PREPARATION_DURATION_MS",
          "BUILD_PACKAGE_DURATION_MS",
        ])
        && object(calibrationTiming?.env).CALIBRATION_STARTED_NS
          === "${{ steps.calibration-clock.outputs.started-ns }}"
        && object(calibrationTiming?.env).MODEL_PREPARATION_DURATION_MS
          === "${{ steps.model-prepare.outputs.duration-ms }}"
        && object(calibrationTiming?.env).BUILD_PACKAGE_DURATION_MS
          === "${{ steps.native-build-package.outputs.duration-ms }}"
        && calibrationTimingRun.includes("timing_path=target/calibration-runs/macos/timing.json")
        && calibrationTimingRun.includes(
          "calibration_finished_ns=\"$(python3 -c 'import time; print(time.monotonic_ns())')\"",
        )
        && calibrationTimingRun.includes(
          "calibration_total_ms=$(((calibration_finished_ns - CALIBRATION_STARTED_NS) / 1000000))",
        )
        && calibrationTimingRun.includes(
          'test "$calibration_total_ms" -lt 600000',
        )
        && calibrationTimingRun.includes(
          '[[ "$MODEL_PREPARATION_DURATION_MS" =~ ^[0-9]+$ ]]',
        )
        && calibrationTimingRun.includes("test \"$BUILD_PACKAGE_DURATION_MS\" -ge 0")
        && calibrationTimingRun.includes("archive_authentication_unpack_ms")
        && calibrationTimingRun.includes("project_and_request_setup_ms")
        && calibrationTimingRun.includes("measurement_ms")
        && calibrationTimingRun.includes("retention_validation_ms")
        && calibrationTimingRun.includes("end_to_end_ms")
        && calibrationTimingRun.includes("Model preparation")
        && calibrationTimingRun.includes("Shared CLI build and package")
        && calibrationTimingRun.includes("Total calibration wall time")
        && calibrationTimingRun.includes(">> \"$GITHUB_STEP_SUMMARY\""),
      `${metalFile} calibration must publish shared build/package and five-phase collector timing, including model preparation and an under-ten-minute total`,
    );
    const engine = namedStep(job, "Prove protected Metal runtime");
    add(violations, object(engine?.env).CODESTORY_EMBED_ALLOW_CPU === "0", `${metalFile} engine proof must reject CPU fallback`);
    const engineRun = stepRun(
      job,
      "Prove protected Metal runtime",
    );
    add(
      violations,
      engineRun.includes("calibration_args=()")
        && engineRun.includes('"${calibration_args[@]}"')
        && engineRun.includes('claim_scope_args=(--server-behavior-only)')
        && occurrenceCount(engineRun, "--calibration-bundle") === 1
        && object(engine?.env).USE_PACKAGED_CLI_ARTIFACT
          === "${{ inputs.use_packaged_cli_artifact }}"
        && object(engine?.env).VERIFIED_QUALIFICATION_DRIVER
          === "${{ steps.qualification-driver.outputs.path }}"
        && engineRun.includes(
          "qualification_driver=target/release/codestory_embedding_qualification",
        )
        && engineRun.includes(
          'if [ "$USE_PACKAGED_CLI_ARTIFACT" = true ]; then',
        )
        && engineRun.includes(
          'qualification_driver="$VERIFIED_QUALIFICATION_DRIVER"',
        )
        && occurrenceCount(engineRun, "qualification_driver=") === 2
        && engineRun.includes('test -x "$qualification_driver"')
        && engineRun.includes('--qualification-driver "$qualification_driver"')
        && !engineRun.includes(
          "--qualification-driver target/release/codestory_embedding_qualification",
        ),
      `${metalFile} server-behavior proof must omit calibration while qualification retains it`,
    );
    add(
      violations,
      engine?.if === "${{ !inputs.calibration_mode && !inputs.candidate_installed_proof }}",
      `${metalFile} protected Metal proof must yield to the candidate-installed lane`,
    );
    const candidateStage = namedStep(job, "Stage isolated candidate-managed macOS install");
    add(
      violations,
      candidateStage?.if === "${{ inputs.candidate_installed_proof && !inputs.calibration_mode }}",
      `${metalFile} candidate-managed staging must require candidate mode outside calibration`,
    );
    const candidateProof = namedStep(job, "Prove candidate-installed macOS Metal runtime");
    add(
      violations,
      candidateProof?.if === "${{ inputs.candidate_installed_proof && !inputs.calibration_mode }}",
      `${metalFile} candidate-installed Metal proof must require candidate mode outside calibration`,
    );
    const candidateProofRun = stepRun(
      job,
      "Prove candidate-installed macOS Metal runtime",
    );
    add(
      violations,
      !candidateProofRun.includes("calibration")
        && !candidateProofRun.includes("--ground-only")
        && !candidateProofRun.includes("--engine-policy cpu_explicit"),
      `${metalFile} candidate-installed Metal proof must be bounded accelerated runtime proof`,
    );
    add(
      violations,
      object(candidateProof?.env).CODESTORY_EMBED_ALLOW_CPU === "0",
      `${metalFile} candidate-installed proof must reject CPU fallback`,
    );
    for (const cell of [
      "Emit authenticated Metal release cell",
      "Emit authenticated macOS retrieval-readiness release cell",
      "Emit authenticated candidate-installed macOS release cell",
    ]) {
      requireStepEnv(violations, metalFile, job, cell, { INPUT_REF: "${{ inputs.ref }}" });
    }
    const metalCellUpload = namedStep(job, "Upload authenticated Metal release cell");
    add(
      violations,
      metalCellUpload?.uses === "actions/upload-artifact@v7.0.1"
        && String(metalCellUpload?.if ?? "").includes("success()")
        && String(metalCellUpload?.if ?? "").includes("inputs.emit_release_cells"),
      `${metalFile} Metal release cell must be a success-only retained artifact`,
    );
    requireStepUses(
      violations,
      metalFile,
      job,
      "Upload authenticated candidate-installed macOS release cell",
      "actions/upload-artifact@v7.0.1",
    );
  }

  const vulkanFile = "windows-vulkan-proof.yml";
  const vulkan = workflows.get(vulkanFile);
  if (!vulkan) {
    violations.push(`${vulkanFile} must exist`);
  } else {
    add(
      violations,
      trigger(vulkan, "workflow_call") !== undefined
        && trigger(vulkan, "workflow_dispatch") === undefined,
      `${vulkanFile} must be coordinator-only and not directly dispatchable`,
    );
    for (const event of ["workflow_call"]) {
      for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
        requireOptionalStringInput(violations, vulkanFile, vulkan, event, key);
      }
      add(
        violations,
        at(vulkan, "on", event, "inputs", "quality_evidence_artifact") === undefined,
        `${vulkanFile} ${event} must not accept optional quality evidence`,
      );
    }
    const candidateInput = object(at(
      vulkan,
      "on",
      "workflow_call",
      "inputs",
      "candidate_installed_proof",
    ));
    add(
      violations,
      candidateInput.required === false
        && candidateInput.type === "boolean"
        && candidateInput.default === false,
      `${vulkanFile} candidate-installed proof must be an explicit opt-in`,
    );
    for (const event of ["workflow_call", "workflow_dispatch"]) {
      add(
        violations,
        at(vulkan, "on", event, "inputs", "candidate_installed_only") === undefined,
        `${vulkanFile} ${event} must not define candidate_installed_only`,
      );
    }
    const candidateProducerInput = object(at(
      vulkan,
      "on",
      "workflow_call",
      "inputs",
      "candidate_producer_workflow_path",
    ));
    add(
      violations,
      candidateProducerInput.required === false
        && candidateProducerInput.type === "string"
        && candidateProducerInput.default === ".github/workflows/packaged-platform-pr.yml",
      `${vulkanFile} candidate producer path must default to the exact PR coordinator`,
    );
    const serverBehaviorInput = object(at(
      vulkan,
      "on",
      "workflow_call",
      "inputs",
      "server_behavior_only",
    ));
    add(
      violations,
      serverBehaviorInput.required === false
        && serverBehaviorInput.type === "boolean"
        && serverBehaviorInput.default === false,
      `${vulkanFile} server-behavior-only claim scope must be an explicit opt-in`,
    );
    // The packaged-vulkan job's structural pin (job existence) is a rule instance
    // and lives in release-claims.json under workflow_policy.structural_pins;
    // structuralPinViolations evaluates it. The lane's step fragments - mode
    // validation, build-tool evidence, model preparation, candidate cache
    // handling, calibration-producer authentication, the protected and
    // candidate-installed proofs, and the release cells - are rule instances and
    // live in release-claims.json under workflow_policy.step_fragments;
    // stepFragmentViolations evaluates them.
    const job = object(object(vulkan.jobs)["packaged-vulkan"]);
    add(violations, JSON.stringify(job["runs-on"]) === JSON.stringify(["self-hosted", "Windows", "X64", "codestory-vulkan"]), `${vulkanFile} must use the protected Windows Vulkan runner`);
    add(violations, job.environment === "windows-vulkan-proof", `${vulkanFile} must use the protected Vulkan environment`);
    const pythonSetup = namedStep(job, "Install pinned Python");
    requireStepUses(
      violations,
      vulkanFile,
      job,
      "Install pinned Python",
      "actions/setup-python@v7.0.0",
    );
    add(
      violations,
      hasExactKeys(object(pythonSetup), ["name", "uses", "with", "env"])
        && hasExactKeys(object(pythonSetup?.with), ["python-version"])
        && object(pythonSetup?.with)["python-version"] === "3.13"
        && hasExactKeys(object(pythonSetup?.env), ["PSExecutionPolicyPreference"])
        && object(pythonSetup?.env).PSExecutionPolicyPreference === "Bypass",
      `${vulkanFile} must pin Python 3.13 with process-scoped script policy`,
    );
    add(
      violations,
      list(job.steps).at(1) === pythonSetup,
      `${vulkanFile} pinned Python must run immediately after checkout`,
    );
    const windowsPowerShellShell = `powershell -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command ". '{0}'"`;
    for (const stepName of [
      "Capture host evidence",
      "Validate candidate-installed mode",
      "Capture source build tool evidence",
      "Install pinned Rust",
      "Build and package native CLI",
      "Authenticate exact Windows candidate artifacts",
      "Restore exact candidate archive from protected host",
      "Download, authenticate, and admit candidate archive on miss",
      "Verify packaged qualification driver",
      "Authenticate calibration bundle producer",
      "Prove protected Windows Vulkan runtime",
      "Stage isolated candidate-managed Windows install",
      "Prove candidate-installed Windows Vulkan runtime",
    ]) {
      add(
        violations,
        namedStep(job, stepName)?.shell === windowsPowerShellShell,
        `${vulkanFile} ${stepName} must use built-in Windows PowerShell with process-scoped execution-policy bypass`,
      );
    }
    const validateCandidate = namedStep(job, "Validate candidate-installed mode");
    add(
      violations,
      validateCandidate?.if === "inputs.candidate_installed_proof",
      `${vulkanFile} candidate-installed validation must require explicit candidate mode`,
    );
    requireStepEnv(violations, vulkanFile, job, "Validate candidate-installed mode", {
      SERVER_BEHAVIOR_ONLY: "${{ inputs.server_behavior_only }}",
    });
    const sourceBuildTools = namedStep(job, "Capture source build tool evidence");
    add(
      violations,
      hasExactKeys(object(sourceBuildTools), ["name", "if", "shell", "run"])
        && sourceBuildTools?.if === "${{ !inputs.use_packaged_cli_artifact }}"
        && sourceBuildTools?.shell === windowsPowerShellShell,
      `${vulkanFile} source build tool evidence must remain source-only and fail closed`,
    );
    const nativeBuild = namedStep(job, "Build and package native CLI");
    const nativeBuildRun = shellLiteralNormalizedText(String(nativeBuild?.run ?? ""));
    add(
      violations,
      hasExactKeys(object(nativeBuild?.env), [
        "VERSION",
        "CMAKE_GENERATOR",
        "SERVER_BEHAVIOR_ONLY",
      ])
        && object(nativeBuild?.env).CMAKE_GENERATOR === windowsNativeGenerator,
      `${vulkanFile} source package build must use the Ninja native generator`,
    );
    add(
      violations,
      shellInvocationsContaining(nativeBuildRun, "cargo @cargoArgs").length === 1
        && nativeBuildRun.includes("-p, codestory-cli")
        && nativeBuildRun.includes("--bin, codestory-cli")
        && nativeBuildRun.includes("--bin, codestory-cli-runtime")
        && nativeBuildRun.includes("$env:SERVER_BEHAVIOR_ONLY -ne true")
        && nativeBuildRun.includes("-p, codestory-bench")
        && nativeBuildRun.includes(
          "--bin, codestory_embedding_qualification",
        )
        && occurrenceCount(
          nativeBuildRun,
          "codestory_embedding_qualification",
        ) === 1
        && !nativeBuildRun.includes("codestory_embedding_constant_calibration")
        && shellInvocationsContaining(
          nativeBuildRun,
          "python .github/scripts/package-codestory-release.py",
        ).length === 1
        && jobShellInvocationsContaining(job, "cargo @cargoArgs").length === 1
        && jobShellInvocationsContaining(job, "cargo build").length === 0,
      `${vulkanFile} source fallback must build CLI, runtime, and qualification driver in one Cargo invocation`,
    );
    add(
      violations,
      namedStep(job, "Install pinned Rust")?.if
        === "${{ !inputs.use_packaged_cli_artifact }}",
      `${vulkanFile} every packaged proof must skip Rust installation`,
    );
    add(
      violations,
      namedStep(job, "Build qualification driver") === undefined,
      `${vulkanFile} must not rebuild the qualification driver after package download`,
    );
    const windowsPackageAuthentication = namedStep(
      job,
      "Authenticate exact Windows candidate artifacts",
    );
    const windowsPackageAuthenticationRun = shellLiteralNormalizedText(
      stepRun(job, "Authenticate exact Windows candidate artifacts"),
    );
    add(
      violations,
      windowsPackageAuthentication?.id === "candidate-artifacts"
        && windowsPackageAuthentication?.if === "inputs.use_packaged_cli_artifact"
        && windowsPackageAuthentication?.shell === windowsPowerShellShell
        && windowsPackageAuthentication?.["continue-on-error"] === undefined
        && hasExactKeys(object(windowsPackageAuthentication?.env), [
          "GH_TOKEN",
          "CANDIDATE_PRODUCER_WORKFLOW_PATH",
          "SERVER_BEHAVIOR_ONLY",
        ])
        && object(windowsPackageAuthentication?.env).GH_TOKEN
          === "${{ github.token }}"
        && object(windowsPackageAuthentication?.env).CANDIDATE_PRODUCER_WORKFLOW_PATH
          === "${{ inputs.candidate_producer_workflow_path }}"
        && object(windowsPackageAuthentication?.env).SERVER_BEHAVIOR_ONLY
          === "${{ inputs.server_behavior_only }}"
        && windowsPackageAuthenticationRun.includes(
          ".github/workflows/auto-release.yml",
        )
        && windowsPackageAuthenticationRun.includes(
          ".github/workflows/release.yml",
        )
        && windowsPackageAuthenticationRun.includes(
          ".github/workflows/packaged-platform-pr.yml",
        )
        && windowsPackageAuthenticationRun.includes(
          "$env:SERVER_BEHAVIOR_ONLY -ne true",
        )
        && windowsPackageAuthenticationRun.includes(
          "$env:CANDIDATE_PRODUCER_WORKFLOW_PATH -notin $allowedWorkflows",
        )
        && windowsPackageAuthenticationRun.includes(
          "$run.head_repository.full_name -ne $env:GITHUB_REPOSITORY",
        )
        && windowsPackageAuthenticationRun.includes(
          "$run.path -ne $env:CANDIDATE_PRODUCER_WORKFLOW_PATH",
        )
        && windowsPackageAuthenticationRun.includes(
          "$run.head_sha -ne $sourceSha",
        )
        && windowsPackageAuthenticationRun.includes(
          "[string]$run.run_attempt -ne $env:GITHUB_RUN_ATTEMPT",
        )
        && windowsPackageAuthenticationRun.includes(
          "$_.name -eq $name",
        )
        && windowsPackageAuthenticationRun.includes(
          "[string]$_.workflow_run.id -eq $env:GITHUB_RUN_ID",
        )
        && windowsPackageAuthenticationRun.includes(
          "$_.workflow_run.head_sha -eq $sourceSha",
        )
        && windowsPackageAuthenticationRun.includes(
          "expected exactly one authenticated $name artifact",
        )
        && windowsPackageAuthenticationRun.includes(
          "codestory-cli-windows-x64",
        )
        && windowsPackageAuthenticationRun.includes(
          "codestory-candidate-archive-record-windows-x64",
        )
        && windowsPackageAuthenticationRun.includes(
          "codestory-qualification-driver-windows-x64",
        )
        && windowsPackageAuthenticationRun.includes(
          "package-id=$($package.id)",
        )
        && windowsPackageAuthenticationRun.includes(
          "package-bytes=$($package.size_in_bytes)",
        )
        && windowsPackageAuthenticationRun.includes(
          "package-sha256=$($package.digest.Substring(7))",
        ),
      `${vulkanFile} packaged proof must authenticate the exact candidate record, package, and private driver from an allowlisted producer`,
    );
    const windowsRecordDownload = namedStep(
      job,
      "Download authenticated candidate record",
    );
    const windowsCacheRestore = namedStep(
      job,
      "Restore exact candidate archive from protected host",
    );
    const windowsCacheMiss = namedStep(
      job,
      "Download, authenticate, and admit candidate archive on miss",
    );
    const windowsDriverDownload = namedStep(
      job,
      "Download separate authenticated qualification driver",
    );
    add(
      violations,
      windowsRecordDownload?.if === "inputs.use_packaged_cli_artifact"
        && windowsRecordDownload?.uses === "actions/download-artifact@v8.0.1"
        && hasExactKeys(object(windowsRecordDownload?.with), ["name", "path"])
        && object(windowsRecordDownload?.with).name
          === "codestory-candidate-archive-record-windows-x64"
        && object(windowsRecordDownload?.with).path
          === "target/candidate-archive-record/windows-x64"
        && windowsCacheRestore?.id === "candidate-cache"
        && windowsCacheRestore?.if === "inputs.use_packaged_cli_artifact"
        && windowsCacheRestore?.shell === windowsPowerShellShell
        && windowsCacheRestore?.["continue-on-error"] === undefined,
      `${vulkanFile} protected cache lookup must consume only the exact small Windows candidate record`,
    );
    add(
      violations,
      windowsCacheMiss?.if
        === "inputs.use_packaged_cli_artifact && steps.candidate-cache.outputs.hit != 'true'"
        && windowsCacheMiss?.shell === windowsPowerShellShell
        && windowsCacheMiss?.["continue-on-error"] === undefined
        && hasExactKeys(object(windowsCacheMiss?.env), [
          "ARTIFACT_ID",
          "EXPECTED_SHA256",
          "EXPECTED_SIZE",
          "GH_TOKEN",
        ])
        && object(windowsCacheMiss?.env).ARTIFACT_ID
          === "${{ steps.candidate-artifacts.outputs.package-id }}"
        && object(windowsCacheMiss?.env).EXPECTED_SIZE
          === "${{ steps.candidate-artifacts.outputs.package-bytes }}"
        && object(windowsCacheMiss?.env).EXPECTED_SHA256
          === "${{ steps.candidate-artifacts.outputs.package-sha256 }}"
        && object(windowsCacheMiss?.env).GH_TOKEN === "${{ github.token }}",
      `${vulkanFile} large Windows Actions artifact transfer must be cache-miss-only and outer-digest authenticated`,
    );
    add(
      violations,
      windowsDriverDownload?.if
        === "${{ inputs.use_packaged_cli_artifact && !inputs.server_behavior_only }}"
        && windowsDriverDownload?.uses === "actions/download-artifact@v8.0.1"
        && hasExactKeys(object(windowsDriverDownload?.with), ["name", "path"])
        && object(windowsDriverDownload?.with).name
          === "codestory-qualification-driver-windows-x64"
        && object(windowsDriverDownload?.with).path
          === "target/qualification-driver-artifact/windows-x64"
        && stepIndex(job, "Authenticate exact Windows candidate artifacts")
          < stepIndex(job, "Download authenticated candidate record")
        && stepIndex(job, "Download authenticated candidate record")
          < stepIndex(job, "Restore exact candidate archive from protected host")
        && stepIndex(job, "Restore exact candidate archive from protected host")
          < stepIndex(job, "Download, authenticate, and admit candidate archive on miss")
        && stepIndex(job, "Download, authenticate, and admit candidate archive on miss")
          < stepIndex(job, "Download separate authenticated qualification driver"),
      `${vulkanFile} private Windows qualification driver must stay separate from the cached public candidate`,
    );
    const windowsDriverVerify = namedStep(job, "Verify packaged qualification driver");
    const windowsDriverVerifyRun = shellLiteralNormalizedText(
      stepRun(job, "Verify packaged qualification driver"),
    );
    add(
      violations,
      windowsDriverVerify?.id === "qualification-driver"
        && windowsDriverVerify?.if
          === "${{ inputs.use_packaged_cli_artifact && !inputs.server_behavior_only }}"
        && windowsDriverVerify?.shell === windowsPowerShellShell
        && windowsDriverVerify?.["continue-on-error"] === undefined
        && hasExactKeys(object(windowsDriverVerify?.env), ["INPUT_VERSION"])
        && object(windowsDriverVerify?.env).INPUT_VERSION
          === "${{ inputs.version }}"
        && shellInvocationsContaining(
          windowsDriverVerifyRun,
          "node .github/scripts/qualification-driver-artifact.mjs verify",
        ).length === 1
        && windowsDriverVerifyRun.includes("--asset-target windows-x64")
        && windowsDriverVerifyRun.includes("--source-sha $sourceSha")
        && windowsDriverVerifyRun.includes("--source-tree $sourceTree")
        && windowsDriverVerifyRun.includes("--version $version")
        && windowsDriverVerifyRun.includes(
          "--archive target/release-dist/codestory-cli-v$version-windows-x64.zip",
        )
        && windowsDriverVerifyRun.includes("--trusted-root $env:GITHUB_WORKSPACE")
        && windowsDriverVerifyRun.includes(
          "--artifact-dir target/qualification-driver-artifact/windows-x64",
        )
        && windowsDriverVerifyRun.includes("$result.driver")
        && windowsDriverVerifyRun.includes("$env:GITHUB_OUTPUT")
        && stepIndex(job, "Verify packaged qualification driver")
          === stepIndex(job, "Download separate authenticated qualification driver") + 1,
      `${vulkanFile} packaged qualification must verify the archive-bound private driver`,
    );
    add(
      violations,
      qualificationDriverHandoffIsSealed(
        job,
        "Verify packaged qualification driver",
        "Prove protected Windows Vulkan runtime",
        [
          "Authenticate calibration bundle producer",
          "Download frozen calibration bundle",
        ],
      ),
      `${vulkanFile} must not replace the verified qualification driver before execution`,
    );
    requireCalibrationProducerBoundary(
      violations,
      vulkanFile,
      job,
      "${{ !inputs.server_behavior_only }}",
    );
    const engine = namedStep(job, "Prove protected Windows Vulkan runtime");
    add(violations, object(engine?.env).CODESTORY_EMBED_ALLOW_CPU === "0", `${vulkanFile} engine proof must reject CPU fallback`);
    const engineRun = stepRun(job, "Prove protected Windows Vulkan runtime");
    add(
      violations,
      engineRun.includes("$calibrationArgs = @()")
        && engineRun.includes("@calibrationArgs")
        && engineRun.includes('$claimArgs = @("--server-behavior-only")')
        && engineRun.includes('"--produce-qualification-evidence"')
        && object(engine?.env).USE_PACKAGED_CLI_ARTIFACT
          === "${{ inputs.use_packaged_cli_artifact }}"
        && object(engine?.env).VERIFIED_QUALIFICATION_DRIVER
          === "${{ steps.qualification-driver.outputs.path }}"
        && engineRun.includes(
          '$qualificationDriver = "target/release/codestory_embedding_qualification.exe"',
        )
        && engineRun.includes(
          'if ($env:USE_PACKAGED_CLI_ARTIFACT -eq "true")',
        )
        && engineRun.includes(
          "$qualificationDriver = $env:VERIFIED_QUALIFICATION_DRIVER",
        )
        && occurrenceCount(engineRun, "$qualificationDriver =") === 2
        && engineRun.includes(
          'Test-Path -LiteralPath $qualificationDriver -PathType Leaf',
        )
        && engineRun.includes(
          '"--qualification-driver", $qualificationDriver',
        )
        && engineRun.includes(
          '"--qualification-evidence", "target/windows-vulkan-proof/qualification.json"',
        )
        && !engineRun.includes("--retrieval-quality-evidence")
        && occurrenceCount(engineRun, "--produce-qualification-evidence") === 1
        && occurrenceCount(engineRun, "--calibration-bundle") === 1
        && occurrenceCount(engineRun, "check-packaged-agent-proof.py") === 1,
      `${vulkanFile} server-behavior proof must omit calibration while qualification runs one lifecycle proof without optional quality`,
    );
    add(
      violations,
      engine?.if === "${{ !inputs.candidate_installed_proof }}",
      `${vulkanFile} protected Windows Vulkan proof must yield to the candidate-installed lane`,
    );
    const candidateStage = namedStep(job, "Stage isolated candidate-managed Windows install");
    add(
      violations,
      candidateStage?.if === "inputs.candidate_installed_proof",
      `${vulkanFile} candidate-managed staging must require explicit candidate mode`,
    );
    const candidateProof = namedStep(job, "Prove candidate-installed Windows Vulkan runtime");
    add(
      violations,
      candidateProof?.if === "inputs.candidate_installed_proof",
      `${vulkanFile} candidate-installed Vulkan proof must require explicit candidate mode`,
    );
    const candidateProofRun = stepRun(
      job,
      "Prove candidate-installed Windows Vulkan runtime",
    );
    add(
      violations,
      !candidateProofRun.includes("calibration")
        && !candidateProofRun.includes("--ground-only")
        && !candidateProofRun.includes("--engine-policy cpu_explicit"),
      `${vulkanFile} candidate-installed Vulkan proof must be bounded accelerated runtime proof`,
    );
    add(
      violations,
      object(candidateProof?.env).CODESTORY_EMBED_ALLOW_CPU === "0",
      `${vulkanFile} candidate-installed proof must reject CPU fallback`,
    );
    const candidateUpload = namedStep(job, "Upload candidate-installed Windows proof");
    add(
      violations,
      candidateUpload?.uses === "actions/upload-artifact@v7.0.1"
        && String(candidateUpload?.if ?? "").includes("always()")
        && String(candidateUpload?.if ?? "").includes("inputs.candidate_installed_proof")
        && object(candidateUpload?.with).name
          === "candidate-installed-windows-${{ inputs.version }}-attempt-${{ github.run_attempt }}"
        && object(candidateUpload?.with).path === "target/candidate-installed-windows"
        && object(candidateUpload?.with)["if-no-files-found"] === "error",
      `${vulkanFile} candidate-installed proof must retain one attempt-scoped artifact`,
    );
    for (const cell of [
      "Emit authenticated Vulkan release cell",
      "Emit authenticated Windows retrieval-readiness release cell",
      "Emit authenticated candidate-installed Windows release cell",
    ]) {
      requireStepEnv(violations, vulkanFile, job, cell, { INPUT_REF: "${{ inputs.ref }}" });
    }
    const releaseCell = namedStep(job, "Emit authenticated Vulkan release cell");
    add(
      violations,
      releaseCell?.if === "inputs.emit_release_cells",
      `${vulkanFile} accelerated proof must retain the authenticated Vulkan release cell`,
    );
    const vulkanCellUpload = namedStep(job, "Upload authenticated Vulkan release cell");
    add(
      violations,
      vulkanCellUpload?.uses === "actions/upload-artifact@v7.0.1"
        && String(vulkanCellUpload?.if ?? "").includes("success()")
        && String(vulkanCellUpload?.if ?? "").includes("inputs.emit_release_cells")
        && !String(vulkanCellUpload?.if ?? "").includes("!inputs.server_behavior_only"),
      `${vulkanFile} Vulkan release cell must be a success-only retained artifact`,
    );
    requireStepUses(
      violations,
      vulkanFile,
      job,
      "Upload authenticated candidate-installed Windows release cell",
      "actions/upload-artifact@v7.0.1",
    );
  }

  const linuxVulkanFile = "linux-vulkan-proof.yml";
  const linuxVulkan = workflows.get(linuxVulkanFile);
  if (!linuxVulkan) {
    violations.push(`${linuxVulkanFile} must exist`);
  } else {
    add(
      violations,
      trigger(linuxVulkan, "workflow_call") !== undefined
        && trigger(linuxVulkan, "workflow_dispatch") === undefined,
      `${linuxVulkanFile} must be coordinator-only and not directly dispatchable`,
    );
    for (const event of ["workflow_call"]) {
      for (const key of ["calibration_bundle_artifact", "calibration_bundle_run_id"]) {
        requireOptionalStringInput(violations, linuxVulkanFile, linuxVulkan, event, key);
      }
      for (const key of ["quality_evidence_artifact", "quality_evidence_run_id"]) {
        add(
          violations,
          at(linuxVulkan, "on", event, "inputs", key) === undefined,
          `${linuxVulkanFile} ${event} must not accept ${key}`,
        );
      }
      add(
        violations,
        at(linuxVulkan, "on", event, "inputs", "candidate_installed_only") === undefined,
        `${linuxVulkanFile} ${event} must not define candidate_installed_only`,
      );
    }
    const candidateInput = object(at(
      linuxVulkan,
      "on",
      "workflow_call",
      "inputs",
      "candidate_installed_proof",
    ));
    add(
      violations,
      candidateInput.required === false
        && candidateInput.type === "boolean"
        && candidateInput.default === false,
      `${linuxVulkanFile} reusable candidate-installed proof must be an explicit opt-in`,
    );
    const optionalCalibrationInput = object(at(
      linuxVulkan,
      "on",
      "workflow_call",
      "inputs",
      "constant_calibration_mode",
    ));
    add(
      violations,
      optionalCalibrationInput.required === false
        && optionalCalibrationInput.type === "boolean"
        && optionalCalibrationInput.default === false,
      `${linuxVulkanFile} optional constant calibration must be coordinator-only and off by default`,
    );
    add(
      violations,
      trigger(linuxVulkan, "workflow_dispatch") === undefined,
      `${linuxVulkanFile} standalone proof must not bypass the coordinator`,
    );
    // The route, packaged-vulkan, and optional-constant-calibration jobs'
    // structural pins (job existence) are rule instances and live in
    // release-claims.json under workflow_policy.structural_pins;
    // structuralPinViolations evaluates them. The lanes' step fragments - the
    // route gate, host evidence, mode validation, candidate cache handling,
    // calibration-producer authentication, the protected and candidate-installed
    // proofs, the release cells, and the optional calibration collector - are
    // rule instances and live in release-claims.json under
    // workflow_policy.step_fragments; stepFragmentViolations evaluates them.
    const route = object(object(linuxVulkan.jobs).route);
    add(
      violations,
      JSON.stringify(route["runs-on"]) === JSON.stringify("ubuntu-latest")
        && route["timeout-minutes"] === 5,
      `${linuxVulkanFile} standalone dispatch validation must stay on a bounded hosted route job`,
    );
    requireStepEnv(
      violations,
      linuxVulkanFile,
      route,
      "Require an upstream package for standalone protected proof",
      {
        EVENT_NAME: "${{ github.event_name }}",
        CONSTANT_CALIBRATION_MODE: "${{ inputs.constant_calibration_mode }}",
        PACKAGE_RUN_ID: "${{ inputs.package_run_id }}",
      },
    );
    const job = object(object(linuxVulkan.jobs)["packaged-vulkan"]);
    add(
      violations,
      job.if === "${{ !inputs.constant_calibration_mode }}"
        && sameMembers(needs(job), ["route"]),
      `${linuxVulkanFile} release proof job must not run during standalone optional calibration`,
    );
    requireStepUses(
      violations,
      linuxVulkanFile,
      job,
      "Install pinned Python",
      "actions/setup-python@v7.0.0",
    );
    const validateCandidate = namedStep(job, "Validate candidate-installed mode");
    add(
      violations,
      validateCandidate?.if === "inputs.candidate_installed_proof"
        && validateCandidate?.shell === "bash",
      `${linuxVulkanFile} candidate-installed validation must require explicit candidate mode`,
    );
    requireStepEnv(violations, linuxVulkanFile, job, "Validate candidate-installed mode", {
      SERVER_BEHAVIOR_ONLY: "${{ inputs.server_behavior_only }}",
    });
    const packageAuthentication = namedStep(
      job,
      "Authenticate exact Linux candidate artifacts",
    );
    const packageAuthenticationRun = shellLiteralNormalizedText(
      stepRun(job, "Authenticate exact Linux candidate artifacts"),
    );
    add(
      violations,
      packageAuthentication?.id === "candidate-artifacts"
        && packageAuthentication?.shell === "bash"
        && packageAuthentication?.if === undefined
        && packageAuthentication?.["continue-on-error"] === undefined
        && hasExactKeys(object(packageAuthentication?.env), [
          "GH_TOKEN",
          "CANDIDATE_PRODUCER_WORKFLOW_PATH",
          "PACKAGE_RUN_ID",
          "SERVER_BEHAVIOR_ONLY",
        ])
        && object(packageAuthentication?.env).GH_TOKEN === "${{ github.token }}"
        && object(packageAuthentication?.env).CANDIDATE_PRODUCER_WORKFLOW_PATH
          === "${{ inputs.candidate_producer_workflow_path }}"
        && object(packageAuthentication?.env).PACKAGE_RUN_ID
          === "${{ inputs.package_run_id || github.run_id }}"
        && object(packageAuthentication?.env).SERVER_BEHAVIOR_ONLY
          === "${{ inputs.server_behavior_only }}"
        && packageAuthenticationRun.includes(
          ".github/workflows/auto-release.yml",
        )
        && packageAuthenticationRun.includes(
          ".github/workflows/release.yml",
        )
        && packageAuthenticationRun.includes(
          ".github/workflows/packaged-platform-pr.yml",
        )
        && packageAuthenticationRun.includes(
          "case $CANDIDATE_PRODUCER_WORKFLOW_PATH in",
        )
        && packageAuthenticationRun.includes(
          "if [ $SERVER_BEHAVIOR_ONLY != true ]",
        )
        && packageAuthenticationRun.includes(
          "test $CANDIDATE_PRODUCER_WORKFLOW_PATH =",
        )
        && packageAuthenticationRun.includes(
          ".head_repository.full_name",
        )
        && packageAuthenticationRun.includes(
          "test $(jq -r .path <<<$run) = $CANDIDATE_PRODUCER_WORKFLOW_PATH",
        )
        && packageAuthenticationRun.includes(
          "test $(jq -r .head_sha <<<$run) = $(git rev-parse HEAD)",
        )
        && packageAuthenticationRun.includes(
          "if [ $PACKAGE_RUN_ID = $GITHUB_RUN_ID ]",
        )
        && packageAuthenticationRun.includes(
          "test $(jq -r .run_attempt <<<$run) = $GITHUB_RUN_ATTEMPT",
        )
        && packageAuthenticationRun.includes(
          "test $(jq -r .status <<<$run) = completed",
        )
        && packageAuthenticationRun.includes(
          "test $(jq -r .conclusion <<<$run) = success",
        )
        && packageAuthenticationRun.includes(
          "expected one exact candidate artifact",
        )
        && packageAuthenticationRun.includes(
          "select_artifact codestory-cli-linux-x64",
        )
        && packageAuthenticationRun.includes(
          "select_artifact codestory-candidate-archive-record-linux-x64",
        )
        && packageAuthenticationRun.includes(
          "select_artifact codestory-qualification-driver-linux-x64",
        )
        && packageAuthenticationRun.includes(".workflow_run.id == $run_id")
        && packageAuthenticationRun.includes(".workflow_run.head_sha == $sha")
        && packageAuthenticationRun.includes("package-id=$artifact_id")
        && packageAuthenticationRun.includes("package-bytes=$expected_size")
        && packageAuthenticationRun.includes(
          "package-sha256=${expected_digest#sha256:}",
        ),
      `${linuxVulkanFile} must authenticate one exact-head candidate record, package, and private driver from an allowlisted producer`,
    );
    const packageDownload = namedStep(
      job,
      "Download authenticated candidate record",
    );
    const linuxCacheRestore = namedStep(
      job,
      "Restore exact candidate archive from protected host",
    );
    const linuxCacheMiss = namedStep(
      job,
      "Download, authenticate, and admit candidate archive on miss",
    );
    const linuxDriverDownload = namedStep(
      job,
      "Download separate authenticated qualification driver",
    );
    add(
      violations,
      packageDownload?.uses === "actions/download-artifact@v8.0.1"
        && hasExactKeys(
          packageDownload?.with,
          ["name", "path", "run-id", "github-token"],
        )
        && object(packageDownload.with).name
          === "codestory-candidate-archive-record-linux-x64"
        && object(packageDownload.with).path
          === "target/candidate-archive-record/linux-x64"
        && object(packageDownload.with)["run-id"]
          === "${{ inputs.package_run_id || github.run_id }}"
        && object(packageDownload.with)["github-token"] === "${{ github.token }}"
        && linuxCacheRestore?.id === "candidate-cache"
        && linuxCacheRestore?.if === undefined
        && linuxCacheRestore?.shell === "bash"
        && linuxCacheRestore?.["continue-on-error"] === undefined,
      `${linuxVulkanFile} protected cache lookup must consume only the exact small Linux candidate record`,
    );
    add(
      violations,
      linuxCacheMiss?.if === "steps.candidate-cache.outputs.hit != 'true'"
        && linuxCacheMiss?.shell === "bash"
        && linuxCacheMiss?.["continue-on-error"] === undefined
        && hasExactKeys(object(linuxCacheMiss?.env), [
          "ARTIFACT_ID",
          "EXPECTED_SHA256",
          "EXPECTED_SIZE",
          "GH_TOKEN",
        ])
        && object(linuxCacheMiss?.env).ARTIFACT_ID
          === "${{ steps.candidate-artifacts.outputs.package-id }}"
        && object(linuxCacheMiss?.env).EXPECTED_SIZE
          === "${{ steps.candidate-artifacts.outputs.package-bytes }}"
        && object(linuxCacheMiss?.env).EXPECTED_SHA256
          === "${{ steps.candidate-artifacts.outputs.package-sha256 }}"
        && object(linuxCacheMiss?.env).GH_TOKEN === "${{ github.token }}",
      `${linuxVulkanFile} large Linux Actions artifact transfer must be cache-miss-only and outer-digest authenticated`,
    );
    add(
      violations,
      linuxDriverDownload?.if === "${{ !inputs.server_behavior_only }}"
        && linuxDriverDownload?.uses === "actions/download-artifact@v8.0.1"
        && hasExactKeys(
          object(linuxDriverDownload?.with),
          ["name", "path", "run-id", "github-token"],
        )
        && object(linuxDriverDownload?.with).name
          === "codestory-qualification-driver-linux-x64"
        && object(linuxDriverDownload?.with).path
          === "target/qualification-driver-artifact/linux-x64"
        && object(linuxDriverDownload?.with)["run-id"]
          === "${{ inputs.package_run_id || github.run_id }}"
        && object(linuxDriverDownload?.with)["github-token"]
          === "${{ github.token }}"
        && stepIndex(job, "Authenticate exact Linux candidate artifacts")
          < stepIndex(job, "Download authenticated candidate record")
        && stepIndex(job, "Download authenticated candidate record")
          < stepIndex(job, "Restore exact candidate archive from protected host")
        && stepIndex(job, "Restore exact candidate archive from protected host")
          < stepIndex(job, "Download, authenticate, and admit candidate archive on miss")
        && stepIndex(job, "Download, authenticate, and admit candidate archive on miss")
          < stepIndex(job, "Download separate authenticated qualification driver"),
      `${linuxVulkanFile} private Linux qualification driver must stay separate from the cached public candidate`,
    );
    requireCalibrationProducerBoundary(
      violations,
      linuxVulkanFile,
      job,
      "${{ !inputs.server_behavior_only }}",
    );
    add(
      violations,
      namedStep(job, "Install pinned Rust for frozen-candidate qualification")
          === undefined
        && namedStep(job, "Build qualification driver") === undefined
        && jobShellInvocationsContaining(job, "cargo build").length === 0
        && jobShellInvocationsContaining(
          job,
          "node scripts/prepare-embedded-model.mjs",
        ).length === 0
        && jobShellInvocationsContaining(job, "rustup toolchain install").length
          === 0,
      `${linuxVulkanFile} packaged qualification must not reinstall Rust, prepare a model, or rebuild the retained driver`,
    );
    const linuxDriverVerify = namedStep(job, "Verify packaged qualification driver");
    const linuxDriverVerifyRun = shellLiteralNormalizedText(
      stepRun(job, "Verify packaged qualification driver"),
    );
    add(
      violations,
      linuxDriverVerify?.id === "qualification-driver"
        && linuxDriverVerify?.if === "${{ !inputs.server_behavior_only }}"
        && linuxDriverVerify?.shell === "bash"
        && linuxDriverVerify?.["continue-on-error"] === undefined
        && hasExactKeys(object(linuxDriverVerify?.env), ["INPUT_VERSION"])
        && object(linuxDriverVerify?.env).INPUT_VERSION
          === "${{ inputs.version }}"
        && shellInvocationsContaining(
          linuxDriverVerifyRun,
          "node .github/scripts/qualification-driver-artifact.mjs verify",
        ).length === 1
        && linuxDriverVerifyRun.includes("--asset-target linux-x64")
        && linuxDriverVerifyRun.includes("--source-sha $(git rev-parse HEAD)")
        && linuxDriverVerifyRun.includes("--source-tree $(git rev-parse HEAD^{tree})")
        && linuxDriverVerifyRun.includes("--version $version")
        && linuxDriverVerifyRun.includes(
          "--archive target/release-dist/codestory-cli-v${version}-linux-x64.tar.gz",
        )
        && linuxDriverVerifyRun.includes("--trusted-root $GITHUB_WORKSPACE")
        && linuxDriverVerifyRun.includes(
          "--artifact-dir target/qualification-driver-artifact/linux-x64",
        )
        && linuxDriverVerifyRun.includes(
          "echo path=$(jq -r .driver <<<$verified) >> $GITHUB_OUTPUT",
        )
        && stepIndex(job, "Verify packaged qualification driver")
          === stepIndex(job, "Download separate authenticated qualification driver") + 1,
      `${linuxVulkanFile} packaged qualification must verify the archive-bound private driver`,
    );
    add(
      violations,
      qualificationDriverHandoffIsSealed(
        job,
        "Verify packaged qualification driver",
        "Prove offline Linux Vulkan retrieval",
        [
          "Authenticate calibration bundle producer",
          "Download frozen calibration bundle",
        ],
      ),
      `${linuxVulkanFile} must not replace the verified qualification driver before execution`,
    );
    const engine = namedStep(job, "Prove offline Linux Vulkan retrieval");
    add(
      violations,
      object(engine?.env).CODESTORY_EMBED_ALLOW_CPU === "0",
      `${linuxVulkanFile} protected proof must reject CPU fallback`,
    );
    const engineRun = stepRun(job, "Prove offline Linux Vulkan retrieval");
    const normalizedEngineRun = shellLiteralNormalizedText(engineRun);
    add(
      violations,
      engine?.if === "${{ !inputs.candidate_installed_proof }}",
      `${linuxVulkanFile} protected Linux Vulkan proof must yield to the candidate-installed lane`,
    );
    add(
      violations,
      engineRun.includes("calibration_args=()")
        && engineRun.includes('"${calibration_args[@]}"')
        && engineRun.includes('claim_args=(--server-behavior-only)')
        && engineRun.includes("qualification_args=()")
        && !normalizedEngineRun.includes("--retrieval-quality-evidence")
        && normalizedEngineRun.includes("--produce-qualification-evidence")
        && object(engine?.env).VERIFIED_QUALIFICATION_DRIVER
          === "${{ steps.qualification-driver.outputs.path }}"
        && normalizedEngineRun.includes(
          "qualification_driver=$VERIFIED_QUALIFICATION_DRIVER",
        )
        && occurrenceCount(normalizedEngineRun, "qualification_driver=") === 1
        && normalizedEngineRun.includes("test -n $qualification_driver")
        && normalizedEngineRun.includes("test -x $qualification_driver")
        && normalizedEngineRun.includes(
          "--qualification-driver $qualification_driver",
        )
        && normalizedEngineRun.includes(
          "--qualification-evidence target/linux-vulkan-proof/qualification.json",
        )
        && occurrenceCount(engineRun, "--produce-qualification-evidence") === 1
        && occurrenceCount(engineRun, "--calibration-bundle") === 1
        && occurrenceCount(engineRun, "check-packaged-agent-proof.py") === 1,
      `${linuxVulkanFile} server-behavior proof must omit calibration while standalone qualification runs one lifecycle proof without optional quality`,
    );
    add(
      violations,
      namedStep(job, "Stage isolated candidate-managed Linux install")?.if
        === "inputs.candidate_installed_proof",
      `${linuxVulkanFile} candidate-managed staging must require explicit candidate mode`,
    );
    const candidate = namedStep(job, "Prove candidate-installed Linux Vulkan runtime");
    add(
      violations,
      candidate?.if === "inputs.candidate_installed_proof",
      `${linuxVulkanFile} candidate-installed Vulkan proof must require explicit candidate mode`,
    );
    add(
      violations,
      object(candidate?.env).CODESTORY_EMBED_ALLOW_CPU === "0",
      `${linuxVulkanFile} candidate-installed proof must reject CPU fallback`,
    );
    requireStepEnv(violations, linuxVulkanFile, job, "Emit authenticated Linux Vulkan release cells", {
      INPUT_REF: "${{ inputs.ref }}",
    });
    const optionalCalibration = object(
      object(linuxVulkan.jobs)["optional-constant-calibration"],
    );
    add(
      violations,
      optionalCalibration.if
        === "${{ inputs.constant_calibration_mode }}"
        && JSON.stringify(optionalCalibration["runs-on"])
          === JSON.stringify(["self-hosted", "Linux", "X64", "codestory-linux-vulkan"])
        && optionalCalibration.environment === "linux-vulkan-proof",
      `${linuxVulkanFile} optional calibration must be a standalone protected coordinator-only Vulkan job`,
    );
    const optionalCollectorName = "Collect optional Linux Vulkan constant calibration";
    const optionalCollector = namedStep(optionalCalibration, optionalCollectorName);
    const optionalCollectorRun = shellLiteralNormalizedText(
      stepRun(optionalCalibration, optionalCollectorName),
    );
    add(
      violations,
      object(optionalCollector?.env).CODESTORY_EMBED_ALLOW_CPU === "0"
        && shellInvocationsContaining(
          optionalCollectorRun,
          "python .github/scripts/check-packaged-agent-proof.py",
        ).length === 1
        && !optionalCollectorRun.includes("--produce-qualification-evidence")
        && !optionalCollectorRun.includes("--qualification-evidence")
        && !optionalCollectorRun.includes("--retrieval-quality-evidence")
        && !optionalCollectorRun.includes("--publication-fault-evidence")
        && !optionalCollectorRun.includes("--qualification-scenario")
        && !optionalCollectorRun.includes("--samples-per-metric")
        && !hasShellLoop(optionalCollectorRun)
        && !optionalCollectorRun.includes("--project")
        && !optionalCollectorRun.includes("--plugin-root")
        && !optionalCollectorRun.includes("--plugin-handoff"),
      `${linuxVulkanFile} optional calibration must collect accelerated constants once from a synthetic project without qualification`,
    );
    const optionalNativeBuild = namedStep(
      optionalCalibration,
      "Build and package native CLI and constant driver",
    );
    const optionalNativeBuildRun = shellLiteralNormalizedText(
      String(optionalNativeBuild?.run ?? ""),
    );
    add(
      violations,
      object(optionalNativeBuild?.env).VERSION === "${{ inputs.version }}"
        && shellInvocationsContaining(optionalNativeBuildRun, "cargo build").length === 1
        && optionalNativeBuildRun.includes("-p codestory-cli")
        && optionalNativeBuildRun.includes("--bin codestory-cli")
        && optionalNativeBuildRun.includes("--bin codestory-cli-runtime")
        && optionalNativeBuildRun.includes("-p codestory-bench")
        && optionalNativeBuildRun.includes("--bin codestory_embedding_constant_calibration")
        && shellInvocationsContaining(
          optionalNativeBuildRun,
          "python .github/scripts/package-codestory-release.py",
        ).length === 1
        && jobShellInvocationsContaining(optionalCalibration, "cargo build").length === 1
        && jobShellInvocationsContaining(
          optionalCalibration,
          ".github/scripts/package-codestory-release.py",
        ).length === 1
        && jobShellInvocationsContaining(
          optionalCalibration,
          ".github/scripts/check-packaged-agent-proof.py",
        ).length === 1
        && optionalNativeBuildRun.includes("--target linux-x64")
        && optionalNativeBuildRun.includes("--binary target/release/codestory-cli")
        && jobShellInvocationsContaining(
          optionalCalibration,
          "node scripts/prepare-embedded-model.mjs",
        ).length === 1
        && namedStep(optionalCalibration, "Download exact Linux package") === undefined
        && namedStep(optionalCalibration, "Build constant calibration driver") === undefined
        && !scalarStrings(optionalCalibration).some(value =>
          value.includes("inputs.package_run_id")),
      `${linuxVulkanFile} optional calibration must prepare once, build CLI and collector once, and package that exact CLI once`,
    );
    const optionalUpload = namedStep(
      optionalCalibration,
      "Upload optional Linux Vulkan calibration evidence",
    );
    add(
      violations,
      optionalUpload?.uses === "actions/upload-artifact@v7.0.1"
        && String(object(optionalUpload?.with).name).includes("${{ github.run_attempt }}")
        && String(object(optionalUpload?.with).path).includes("target/calibration-runs/linux-vulkan")
        && String(object(optionalUpload?.with).path).includes("target/calibration-proof/linux-vulkan"),
      `${linuxVulkanFile} optional calibration must upload attempt-scoped non-selecting evidence`,
    );
    for (const name of [
      "Upload authenticated Linux accelerator release cell",
      "Upload authenticated Linux retrieval release cell",
      "Upload authenticated candidate-installed Linux release cell",
    ]) {
      requireStepUses(
        violations,
        linuxVulkanFile,
        job,
        name,
        "actions/upload-artifact@v7.0.1",
      );
    }
  }

  const retrieval = workflows.get(retrievalFile);
  if (!retrieval) {
    violations.push(`${retrievalFile} must exist`);
  } else {
    for (const violation of windowsManifestProofPolicyViolations(retrieval)) {
      violations.push(`${retrievalFile} ${violation}`);
    }
  }

  const guardFile = "main-branch-source-guard.yml";
  const guard = workflows.get(guardFile);
  if (!guard) {
    violations.push(`${guardFile} must exist`);
  } else {
    add(violations, includesAll(at(guard, "on", "pull_request", "branches"), ["main"]), `${guardFile} must guard main`);
    // The enforce-source-branch job's structural pin (job existence) is a rule
    // instance and lives in release-claims.json under
    // workflow_policy.structural_pins; structuralPinViolations evaluates it.
    const job = object(object(guard.jobs)["enforce-source-branch"]);
    const step = namedStep(job, "Require dev/codestory-next source branch");
    add(violations, object(step?.env).HEAD_REPO !== undefined && object(step?.env).BASE_REPO !== undefined, `${guardFile} must compare source and base repository identity`);
    add(violations, String(step?.run ?? "").includes("dev/codestory-next"), `${guardFile} must require the dev source branch`);
  }
}

function permissionMapMatches(actualValue, expectedValue) {
  const actual = object(actualValue);
  const expected = object(expectedValue);
  return sameMembers(Object.keys(actual), Object.keys(expected))
    && Object.entries(expected).every(([key, value]) => actual[key] === value);
}

function reusableWorkflowPermissionViolations(workflows) {
  const violations = [];
  const permissionRank = value => (
    value === "write" ? 2 : value === "read" ? 1 : 0
  );
  const permissionRequests = value => {
    if (value === "write-all") return [["*", "write"]];
    if (value === "read-all") return [["*", "read"]];
    return Object.entries(object(value));
  };
  const permissionGrant = (value, scope) => {
    if (value === "write-all") return { rank: 2, label: "write-all" };
    if (value === "read-all") return { rank: 1, label: "read-all" };
    const granted = object(value)[scope];
    return { rank: permissionRank(granted), label: granted ?? "none" };
  };
  const localWorkflow = /^\.\/\.github\/workflows\/([^/]+\.ya?ml)$/u;

  for (const [callerFile, callerWorkflow] of workflows) {
    for (const [jobName, jobValue] of Object.entries(object(callerWorkflow.jobs))) {
      const job = object(jobValue);
      const match = String(job.uses ?? "").match(localWorkflow);
      if (!match) continue;
      const calleeFile = match[1];
      const callee = workflows.get(calleeFile);
      if (!callee) {
        violations.push(
          `[reusable_permissions] ${callerFile} job ${jobName} calls missing local workflow ${calleeFile}`,
        );
        continue;
      }
      const callerPermissions = job.permissions === undefined
        ? callerWorkflow.permissions
        : job.permissions;
      for (const [calleeJobName, calleeJobValue] of Object.entries(object(callee.jobs))) {
        const calleeJob = object(calleeJobValue);
        const requestedPermissions = calleeJob.permissions === undefined
          ? callee.permissions
          : calleeJob.permissions;
        for (const [scope, requested] of permissionRequests(requestedPermissions)) {
          const granted = permissionGrant(callerPermissions, scope);
          add(
            violations,
            granted.rank >= permissionRank(requested),
            `[reusable_permissions] ${callerFile} job ${jobName} grants ${scope}: ${granted.label} but ${calleeFile} job ${calleeJobName} requests ${requested}`,
          );
        }
      }
    }
  }

  return violations;
}

function findNamedStep(workflow, name) {
  for (const job of Object.values(object(workflow.jobs))) {
    const found = namedStep(job, name);
    if (found) return found;
  }
  return undefined;
}

function releaseProofWorkflowFiles(workflows, graph) {
  const files = new Set([
    "auto-release.yml",
    "packaged-platform-pr.yml",
    object(graph.workflow_policy.calibration).coordinator_workflow,
  ].filter(Boolean));
  for (const evidenceType of list(graph.evidence_types)) {
    for (const lane of list(object(evidenceType).proof_lanes)) {
      files.add(path.basename(String(lane)));
    }
  }
  for (const file of list(graph.workflow_policy.artifact_workflows)) {
    files.add(path.basename(String(file)));
  }
  for (const contract of list(graph.workflow_policy.protected_jobs)) {
    files.add(path.basename(String(object(contract).workflow)));
  }
  for (const cell of [
    ...list(object(graph.workflow_policy.calibration).required_cells),
    ...list(object(graph.workflow_policy.calibration).optional_cells),
    ...list(object(graph.workflow_policy.qualification).required_cells),
    ...list(object(graph.workflow_policy.qualification).optional_cells),
  ]) {
    files.add(path.basename(String(object(cell).workflow)));
  }
  files.add(path.basename(
    String(object(graph.workflow_policy.qualification).coordinator_workflow ?? ""),
  ));
  files.delete("");
  let changed = true;
  while (changed) {
    changed = false;
    for (const file of [...files]) {
      for (const job of Object.values(object(workflows.get(file)?.jobs))) {
        const reusable = String(object(job).uses ?? "");
        const match = reusable.match(/^\.\/\.github\/workflows\/([^/]+\.yml)$/u);
        if (match && !files.has(match[1])) {
          files.add(match[1]);
          changed = true;
        }
      }
    }
  }
  return files;
}

function workflowCpuSelectorViolations(file, value) {
  const violations = [];
  function visit(current, location, key = "") {
    if (Array.isArray(current)) {
      current.forEach((item, index) => visit(item, `${location}[${index}]`));
      return;
    }
    if (current !== null && typeof current === "object") {
      for (const [childKey, childValue] of Object.entries(current)) {
        visit(childValue, `${location}.${childKey}`, childKey);
      }
      return;
    }
    const text = String(current ?? "");
    const normalizedKey = key.toLowerCase().replaceAll("-", "_");
    const normalizedText = text.trim().toLowerCase();
    if (
      normalizedKey === "codestory_embed_allow_cpu"
      && text !== "0"
    ) {
      violations.push(
        `[cpu_selector] ${file} ${location} must set CODESTORY_EMBED_ALLOW_CPU to literal 0`,
      );
    }
    if (
      ["policy", "engine_policy", "execution_policy"].includes(normalizedKey)
      && normalizedText === "cpu_explicit"
    ) {
      violations.push(`[cpu_selector] ${file} ${location} selects cpu_explicit`);
    }
    if (
      ["backend", "expected_backend"].includes(normalizedKey)
      && normalizedText === "cpu"
    ) {
      violations.push(`[cpu_selector] ${file} ${location} selects CPU backend`);
    }
    const executable = shellLiteralNormalizedText(text);
    const executableWithoutHostInventory = executable.replaceAll(
      "machdep.cpu.brand_string",
      "",
    );
    if (
      hasNonLiteralCpuAssignment(executable)
      || /\bCODESTORY_EMBED_ALLOW_CPU\b/iu.test(executable)
      || /\bcpu_explicit\b/iu.test(executable)
      || /\bcpu\b/iu.test(executableWithoutHostInventory)
      || /--expected-backend(?:\s+|=)cpu\b/iu.test(executable)
      || /(?:expected_backend|backend)\s*=\s*cpu\b/iu.test(executable)
    ) {
      violations.push(`[cpu_selector] ${file} ${location} contains a CPU proof selector`);
    }
  }
  visit(value, file);
  return violations;
}

const cpuTestSeamAllowedJobs = new Map([
  [
    "retrieval-engine-smoke.yml",
    new Set(["linux-contracts", "windows-manifest-missing"]),
  ],
]);

function workflowCpuTestSeamViolations(file, value, releaseProofFiles) {
  const violations = [];
  function visit(current, location, pathParts, jobName) {
    if (Array.isArray(current)) {
      current.forEach((item, index) =>
        visit(item, `${location}[${index}]`, [...pathParts, String(index)], jobName));
      return;
    }
    if (current !== null && typeof current === "object") {
      for (const [childKey, childValue] of Object.entries(current)) {
        const childPath = [...pathParts, childKey];
        const childLocation = `${location}.${childKey}`;
        const childJob = pathParts.length === 1 && pathParts[0] === "jobs"
          ? childKey
          : jobName;
        if (
          childKey.toLowerCase().replaceAll("-", "_")
            === "codestory_test_embed_allow_cpu"
        ) {
          const allowed = !releaseProofFiles.has(file)
            && cpuTestSeamAllowedJobs.get(file)?.has(childJob) === true
            && childPath.length === 4
            && childPath[0] === "jobs"
            && childPath[1] === childJob
            && childPath[2] === "env"
            && String(childValue) === "1";
          add(
            violations,
            allowed,
            `[cpu_test_seam] ${file} ${childLocation} may enable CPU only through the exact source-test job seam`,
          );
          continue;
        }
        visit(childValue, childLocation, childPath, childJob);
      }
      return;
    }
    if (
      /\bCODESTORY_TEST_EMBED_ALLOW_CPU\b/iu.test(
        shellLiteralNormalizedText(String(current ?? "")),
      )
    ) {
      violations.push(
        `[cpu_test_seam] ${file} ${location} may not reference the CPU test seam outside an allowlisted source-test job env`,
      );
    }
  }
  visit(value, file, [], "");
  return violations;
}

export function releaseProofCpuSelectorViolations(
  workflows,
  graph = loadReleaseClaimGraph(repositoryRoot),
  supportSources,
) {
  const violations = [];
  const releaseProofFiles = releaseProofWorkflowFiles(workflows, graph);
  for (const [file, workflow] of workflows) {
    // The product selector is never legal in a workflow. Source tests that
    // exercise CPU behavior use the separately named, policy-owned test seam.
    violations.push(...workflowCpuSelectorViolations(file, workflow));
    violations.push(...workflowCpuTestSeamViolations(
      file,
      workflow,
      releaseProofFiles,
    ));
  }
  const sources = supportSources ?? new Map([
    [
      ".github/scripts/check-linux-glibc-baseline.sh",
      fs.readFileSync(path.join(repositoryRoot, ".github/scripts/check-linux-glibc-baseline.sh"), "utf8"),
    ],
  ]);
  for (const [file, source] of sources) {
    const executable = shellLiteralNormalizedText(source);
    if (
      hasNonLiteralCpuAssignment(executable)
      || /\bcpu_explicit\b/iu.test(executable)
      || /--expected-backend(?:\s+|=)cpu\b/iu.test(executable)
    ) {
      violations.push(`[cpu_selector] ${file} contains a CPU proof selector`);
    }
  }
  return violations;
}

export function releaseFreezeBarrierWorkflowViolations(
  workflows,
  graph = loadReleaseClaimGraph(repositoryRoot),
  barrierSource = fs.readFileSync(
    path.join(repositoryRoot, ".github", "scripts", "release-freeze-barrier.mjs"),
    "utf8",
  ),
  acceptanceManifestSource = fs.readFileSync(
    path.join(
      repositoryRoot,
      ".github",
      "scripts",
      "release-freeze-acceptance-jobs.json",
    ),
    "utf8",
  ),
) {
  const violations = [];
  for (const [file, workflow] of workflows) {
    add(
      violations,
      !scalarStrings(workflow).some(value => value.includes("verify-pending")),
      `[freeze_barrier] ${file} must never trust a caller-authored pending freeze`,
    );
  }
  const freeze = object(graph.workflow_policy.release_freeze_barrier);
  const acceptance = object(freeze.acceptance);
  const acceptancePhases = object(acceptance.phases);
  const calibrationSourcePhase = object(acceptancePhases.calibration_source);
  const frozenCandidatePhase = object(acceptancePhases.frozen_candidate);
  let acceptanceManifest = {};
  try {
    acceptanceManifest = object(JSON.parse(acceptanceManifestSource));
  } catch {
    violations.push(
      "[freeze_barrier] canonical acceptance job manifest must be valid JSON",
    );
  }
  const acceptanceManifestJobs = object(acceptanceManifest.jobs);
  const acceptanceJobNames = [
    "resolve",
    "freeze-hostile-mutations",
    "freeze-windows-native-probe",
    "freeze-acceptance",
  ];
  const acceptanceManifestDigest = createHash("sha256")
    .update(acceptanceManifestSource)
    .digest("hex");
  add(
    violations,
    freeze.schema === 3
      && freeze.script === ".github/scripts/release-freeze-barrier.mjs"
      && freeze.status_context_prefix === "codestory/release-freeze"
      && sameMembers(list(freeze.allowed_future_source_changes), [
        "crates/codestory-llama-sys/per-user-embedding-server-constant-set.json",
      ])
      && freeze.invalidation_workflow === "release-freeze-invalidation.yml"
      && acceptance.producer_workflow === "source-proof.yml"
      && acceptance.receipt_authority === "github_actions"
      && acceptance.receipt_artifact
        === "release-freeze-receipt-attempt-${{ github.run_attempt }}"
      && acceptance.receipt_file === "release-freeze-receipt.json"
      && acceptance.receipt_producer_job === "resolve"
      && acceptance.status_scope === "exact_candidate_head"
      && acceptance.later_commit_revokes === true
      && acceptance.event === "workflow_dispatch"
      && acceptance.hostile_job === "freeze-hostile-mutations"
      && acceptance.hostile_step === "Execute exact-head hostile mutation matrix"
      && acceptance.windows_job === "freeze-windows-native-probe"
      && acceptance.windows_step === "Run exact-head Windows native probe"
      && sameMembers(list(acceptance.windows_runner), [
        "self-hosted",
        "Windows",
        "X64",
        "codestory-vulkan",
      ])
      && acceptance.windows_probe_max_seconds === 90
      && acceptance.publisher_job === "freeze-acceptance"
      && acceptance.publisher_step === "Publish executable release freeze"
      && acceptance.status_creator === "github-actions[bot]"
      && acceptance.job_manifest
        === ".github/scripts/release-freeze-acceptance-jobs.json"
      && /^[0-9a-f]{64}$/u.test(String(acceptance.job_manifest_sha256 ?? ""))
      && acceptance.job_manifest_sha256 === acceptanceManifestDigest
      && sameMembers(list(calibrationSourcePhase.known_future_source_changes), [
        "crates/codestory-llama-sys/per-user-embedding-server-constant-set.json",
      ])
      && JSON.stringify(list(calibrationSourcePhase.planned_actions)) === JSON.stringify([
        "calibration-source-acceptance",
        "calibration",
        "generated-constant-freeze",
        "frozen-candidate-acceptance",
        "source-proof",
        "qualification",
        "release",
      ])
      && calibrationSourcePhase.next_permitted_mutation
        === "crates/codestory-llama-sys/per-user-embedding-server-constant-set.json"
      && list(frozenCandidatePhase.known_future_source_changes).length === 0
      && JSON.stringify(list(frozenCandidatePhase.planned_actions)) === JSON.stringify([
        "frozen-candidate-acceptance",
        "source-proof",
        "qualification",
        "release",
      ])
      && frozenCandidatePhase.next_permitted_mutation === null,
    "[freeze_barrier] release claim graph must pin the executable exact-head freeze contract",
  );
  add(
    violations,
    hasExactKeys(acceptanceManifest, [
      "schema",
      "workflow",
      "workflow_context_sha256",
      "jobs",
    ])
      && acceptanceManifest.schema === "codestory.release-freeze-acceptance-jobs/v2"
      && acceptanceManifest.workflow === ".github/workflows/source-proof.yml"
      && /^[0-9a-f]{64}$/u.test(
        String(acceptanceManifest.workflow_context_sha256 ?? ""),
      )
      && sameMembers(Object.keys(acceptanceManifestJobs), acceptanceJobNames)
      && acceptanceJobNames.every(jobName =>
        /^[0-9a-f]{64}$/u.test(String(acceptanceManifestJobs[jobName] ?? ""))
      ),
    "[freeze_barrier] canonical acceptance job manifest must pin exactly the executable acceptance jobs",
  );
  add(
    violations,
    barrierSource.includes('gh(["api", `repos/${repository}/pulls/${number}`])')
      && barrierSource.includes(
        "`repos/${repository}/git/ref/heads/dev/codestory-next`",
      )
      && barrierSource.includes(
        "`repos/${repository}/compare/${liveBaseCommit}...${commit}`",
      )
      && barrierSource.includes("base_commit: liveBaseCommit")
      && barrierSource.includes("const currentReleasePr = releasePr(")
      && barrierSource.includes(
        "currentReleasePr.base_commit !== receipt?.release_pr?.base_commit",
      )
      && barrierSource.includes("release PR base advanced after freeze acceptance")
      && barrierSource.includes("git([\"merge-base\", \"--is-ancestor\", mergeCommit, commit]")
      && barrierSource.includes("support PR #${number} is not merged"),
    "[freeze_barrier] Actions receipt authority must recheck the live release PR base and integrated support PR ancestry",
  );
  add(
    violations,
    barrierSource.includes("for (const status of ACTIVE_RUN_STATES)")
      && barrierSource.includes('"api",\n      "--paginate",\n      "--slurp",')
      && barrierSource.includes(
        "`repos/${repository}/actions/runs?status=${status}&per_page=100`",
      )
      && !barrierSource.includes('"run",\n    "list",'),
    "[freeze_barrier] obsolete-run discovery must paginate every active Actions state",
  );

  const invalidationFile = freeze.invalidation_workflow;
  const invalidation = workflows.get(invalidationFile);
  add(
    violations,
    sameMembers(at(invalidation, "on", "pull_request", "branches"), [
      "dev/codestory-next",
    ])
      && sameMembers(at(invalidation, "on", "pull_request", "types"), [
        "synchronize",
      ])
      && sameMembers(at(invalidation, "on", "push", "branches"), [
        "dev/codestory-next",
      ])
      && object(invalidation.permissions).actions === "write"
      && object(invalidation.permissions).contents === "read"
      && object(invalidation.permissions).statuses === "write"
      && at(invalidation, "concurrency", "cancel-in-progress") === true,
    "[freeze_barrier] release freeze invalidation must run automatically when a candidate head is superseded",
  );
  // The invalidate job's structural pin (job existence) is a rule instance and
  // lives in release-claims.json under workflow_policy.structural_pins;
  // structuralPinViolations evaluates it. Its revocation step fragments (both
  // the required superseded-head revocation conjuncts and the forbidden
  // pending-state reuse) are rule instances and live in release-claims.json
  // under workflow_policy.step_fragments; stepFragmentViolations evaluates
  // them.
  const invalidationJob = object(object(invalidation.jobs).invalidate);
  add(
    violations,
    invalidationJob["runs-on"] === "ubuntu-latest"
      && invalidationJob["timeout-minutes"] === 5
      && sameMembers(
        list(invalidationJob.steps).map(step => step?.name ?? step?.uses),
        [
          "actions/checkout@v5",
          "Invalidate a superseded release freeze",
        ],
      ),
    "[freeze_barrier] release freeze invalidation must remain one bounded cancellation job",
  );
  // DISPOSITION: step env bindings have no graph evaluator, so the
  // invalidation step's event bindings stay code.
  requireStepEnv(
    violations,
    invalidationFile,
    invalidationJob,
    "Invalidate a superseded release freeze",
    {
      AFTER_SHA: "${{ github.event.after || github.sha }}",
      BEFORE_SHA: "${{ github.event.before }}",
      EVENT_NAME: "${{ github.event_name }}",
    },
  );
  const invalidationRun = executableRunText(stepRun(
    invalidationJob,
    "Invalidate a superseded release freeze",
  ));
  add(
    violations,
    occurrenceCount(invalidationRun, "release-freeze-barrier.mjs invalidate-superseded")
      === 2
      && occurrenceCount(invalidationRun, '--broad-workflow "Auto Release"') === 2
      && invalidationRun.indexOf('if [ "$EVENT_NAME" = push ]; then')
        < invalidationRun.indexOf("commits/$BEFORE_SHA/statuses?per_page=100")
      && invalidationRun.indexOf("release-freeze-barrier.mjs invalidate-superseded")
        < invalidationRun.indexOf("commits/$BEFORE_SHA/statuses?per_page=100"),
    "[freeze_barrier] every dev push must cancel obsolete proof before PR-status revocation logic",
  );

  for (const file of ["source-proof.yml", "packaged-platform-pr.yml"]) {
    const workflow = workflows.get(file);
    add(
      violations,
      trigger(workflow, "pull_request") === undefined,
      `[proof_identity] ${file} must not run broad proof from a support PR event`,
    );
    add(
      violations,
      String(at(workflow, "concurrency", "group") ?? "").includes("${{ github.sha }}"),
      `[proof_identity] ${file} concurrency must bind the exact Actions SHA`,
    );
    const freezeInput = object(at(
      workflow,
      "on",
      "workflow_dispatch",
      "inputs",
      "freeze_receipt_digest",
    ));
    add(
      violations,
      (
        file === "source-proof.yml"
          ? freezeInput.required === false && freezeInput.default === ""
          : freezeInput.required === true && freezeInput.default === undefined
      )
        && freezeInput.type === "string",
      file === "source-proof.yml"
        ? "[freeze_barrier] source acceptance must mint its own receipt digest"
        : "[freeze_barrier] packaged proof must require an exact-head freeze digest",
    );
    if (file === "source-proof.yml") {
      const dispatchVersionInput = object(at(
        workflow,
        "on",
        "workflow_dispatch",
        "inputs",
        "version",
      ));
      const callVersionInput = object(at(
        workflow,
        "on",
        "workflow_call",
        "inputs",
        "version",
      ));
      const acceptanceInput = object(at(
        workflow,
        "on",
        "workflow_dispatch",
        "inputs",
        "acceptance_only",
      ));
      const acceptancePhaseInput = object(at(
        workflow,
        "on",
        "workflow_dispatch",
        "inputs",
        "acceptance_phase",
      ));
      const callFreezeInput = object(at(
        workflow,
        "on",
        "workflow_call",
        "inputs",
        "freeze_receipt_digest",
      ));
      add(
        violations,
        dispatchVersionInput.required === true
          && dispatchVersionInput.type === "string"
          && callVersionInput.required === true
          && callVersionInput.type === "string"
          && callFreezeInput.required === true
          && callFreezeInput.type === "string"
          && acceptanceInput.required === false
          && acceptanceInput.type === "boolean"
          && acceptanceInput.default === false
          && acceptancePhaseInput.required === false
          && acceptancePhaseInput.type === "choice"
          && acceptancePhaseInput.default === "frozen_candidate"
          && JSON.stringify(list(acceptancePhaseInput.options))
            === JSON.stringify(["calibration_source", "frozen_candidate"])
          && at(workflow, "on", "workflow_dispatch", "inputs", "emit_release_cells")
            === undefined
          && at(workflow, "on", "workflow_call", "inputs", "emit_release_cells")
            === undefined,
        "[freeze_barrier] source-proof.yml must separate acceptance from broad proof",
      );
    }
    // The freeze family's workflow-scoped permission pins (statuses write for
    // the acceptance publisher, statuses read for the packaged authenticator,
    // actions write for superseded-run cancellation on both coordinators) are
    // rule instances and live in release-claims.json under
    // workflow_policy.structural_pins; structuralPinViolations evaluates them
    // with their reviewed violation text. The cancel-superseded step fragments
    // for both coordinator jobs are rule instances and live in
    // release-claims.json under workflow_policy.step_fragments;
    // stepFragmentViolations evaluates them.
    // DISPOSITION: the retired inline call re-required each coordinator job,
    // so a workflow without one reported that violation beside the family
    // pin's own occurrence. The job-existence rules live in their structural
    // pins; this duplicate occurrence stays code so the reviewed verdict
    // multiset is unchanged.
    const coordinatorJob = file === "source-proof.yml" ? "resolve" : "route";
    requireJob(violations, file, workflow, coordinatorJob);
  }

  const sourceWorkflow = workflows.get("source-proof.yml");
  const sourceJobNames = [
    "resolve",
    "freeze-hostile-mutations",
    "freeze-windows-native-probe",
    "freeze-acceptance",
    "full-source-gate",
    "retrieval-generalization",
    "windows-native-contracts",
  ];
  add(
    violations,
    sameMembers(Object.keys(object(sourceWorkflow.jobs)), sourceJobNames),
    "[freeze_barrier] source-proof.yml must use the closed source and acceptance job contract",
  );
  const actualWorkflowContextDigest = createHash("sha256")
    .update(canonicalJson(workflowExecutionContext(sourceWorkflow)))
    .digest("hex");
  add(
    violations,
    actualWorkflowContextDigest === acceptanceManifest.workflow_context_sha256,
    "[freeze_barrier] source-proof.yml workflow execution context must match the canonical acceptance manifest",
  );
  for (const jobName of acceptanceJobNames) {
    const actualDigest = createHash("sha256")
      .update(canonicalJson(object(at(sourceWorkflow, "jobs", jobName))))
      .digest("hex");
    add(
      violations,
      actualDigest === acceptanceManifestJobs[jobName],
      `[freeze_barrier] source-proof.yml ${jobName} must match the canonical acceptance job manifest`,
    );
  }
  // DISPOSITION: the receipt producer's job-existence rule lives in the
  // source family's structural pin; this duplicate occurrence stays code so
  // the reviewed verdict multiset is unchanged.
  const sourceResolve = requireJob(
    violations,
    "source-proof.yml",
    sourceWorkflow,
    acceptance.receipt_producer_job,
  );
  const acceptedCheckout = namedStep(sourceResolve, "Checkout accepted source head");
  add(
    violations,
    acceptedCheckout?.uses === "actions/checkout@v5"
      && object(acceptedCheckout.with).ref === "${{ steps.resolve.outputs.ref }}"
      && object(acceptedCheckout.with)["fetch-depth"] === 0,
    "[freeze_barrier] Actions receipt generation must have complete history for support PR ancestry",
  );
  const recordReceipt = namedStep(sourceResolve, "Record executable release freeze");
  add(
    violations,
    recordReceipt?.if === "${{ inputs.acceptance_only }}",
    "[freeze_barrier] Actions may generate a release freeze receipt only in acceptance mode",
  );
  // The receipt-recording step fragments are rule instances and live in
  // release-claims.json under workflow_policy.step_fragments;
  // stepFragmentViolations evaluates them.
  // DISPOSITION: step env bindings have no graph evaluator, so the receipt
  // step's input bindings stay code.
  requireStepEnv(
    violations,
    "source-proof.yml",
    sourceResolve,
    "Record executable release freeze",
    {
      CALLER_FREEZE_RECEIPT_DIGEST: "${{ inputs.freeze_receipt_digest }}",
      ACCEPTANCE_PHASE: "${{ inputs.acceptance_phase }}",
      CANCELLED_RUNS_JSON: "${{ steps.cancel.outputs.cancelled }}",
      HEAD_SHA: "${{ steps.resolve.outputs.ref }}",
      INVALIDATED_EVIDENCE_JSON: "${{ inputs.invalidated_evidence_json }}",
      PR_NUMBER: "${{ inputs.pr_number }}",
      REUSABLE_EVIDENCE_JSON: "${{ inputs.reusable_evidence_json }}",
      SUPPORT_PRS_JSON: "${{ inputs.support_prs_json }}",
    },
  );
  const receiptUpload = namedStep(
    sourceResolve,
    "Upload executable release freeze receipt",
  );
  add(
    violations,
    receiptUpload?.if === "${{ inputs.acceptance_only }}"
      && receiptUpload?.uses === "actions/upload-artifact@v7.0.1"
      && object(receiptUpload.with).name
        === "${{ steps.receipt.outputs.artifact_name }}"
      && object(receiptUpload.with).path
        === "${{ runner.temp }}/release-freeze-receipt.json"
      && object(receiptUpload.with)["if-no-files-found"] === "error"
      && object(receiptUpload.with)["retention-days"] === 30
      && object(sourceResolve.outputs).freeze_digest
        === "${{ steps.receipt.outputs.digest }}"
      && object(sourceResolve.outputs).freeze_artifact_name
        === "${{ steps.receipt.outputs.artifact_name }}",
    "[freeze_barrier] source acceptance must retain one immutable attempt-qualified Actions receipt",
  );
  const broadFreeze = namedStep(sourceResolve, "Require executable release freeze");
  add(
    violations,
    broadFreeze?.if === "${{ !inputs.acceptance_only }}",
    "[freeze_barrier] broad source proof must authenticate the accepted freeze",
  );
  // The freeze family's verify-status fragments for this step are a rule
  // instance and live in release-claims.json under
  // workflow_policy.step_fragments beside the source family's own wider pin;
  // stepFragmentViolations evaluates both.
  // DISPOSITION: step env bindings have no graph evaluator, so the freeze
  // step's digest and head bindings stay code.
  requireStepEnv(
    violations,
    "source-proof.yml",
    sourceResolve,
    "Require executable release freeze",
    {
      FREEZE_RECEIPT_DIGEST: "${{ inputs.freeze_receipt_digest }}",
      HEAD_SHA: "${{ steps.resolve.outputs.ref }}",
    },
  );
  add(
    violations,
    !scalarStrings(sourceWorkflow).some(value => value.includes("verify-pending")),
    "[freeze_barrier] source proof must never accept a caller-authored pending status",
  );
  // The hostile mutation job's structural pin (job existence) is a rule
  // instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it. Its
  // blocking suite fragments are rule instances and live in
  // release-claims.json under workflow_policy.step_fragments;
  // stepFragmentViolations evaluates them.
  const hostileJob = object(object(sourceWorkflow.jobs)[acceptance.hostile_job]);
  add(
    violations,
    hostileJob.if === "inputs.acceptance_only"
      && sameMembers(needs(hostileJob), ["resolve"])
      && hostileJob["runs-on"] === "ubuntu-latest"
      && hostileJob["timeout-minutes"] === 5
      && namedStep(hostileJob, acceptance.hostile_step)?.["continue-on-error"] !== true,
    "[freeze_barrier] source acceptance must execute the exact blocking hostile mutation job",
  );

  // The Windows probe job's structural pin (job existence) is a rule instance
  // and lives in release-claims.json under workflow_policy.structural_pins;
  // structuralPinViolations evaluates it. Its probe step fragments (both the
  // required hard-link identity legs and the forbidden inline-script variant)
  // are rule instances and live in release-claims.json under
  // workflow_policy.step_fragments; stepFragmentViolations evaluates them.
  const windowsJob = object(object(sourceWorkflow.jobs)[acceptance.windows_job]);
  const windowsProbePowerShell
    = `powershell -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command ". '{0}'"`;
  add(
    violations,
    windowsJob.if === "inputs.acceptance_only"
      && sameMembers(needs(windowsJob), ["resolve"])
      && sameMembers(list(windowsJob["runs-on"]), list(acceptance.windows_runner))
      && windowsJob["timeout-minutes"] === 5
      && namedStep(windowsJob, acceptance.windows_step)?.shell
        === windowsProbePowerShell
      && namedStep(windowsJob, acceptance.windows_step)?.["continue-on-error"] !== true,
    "[freeze_barrier] source acceptance must execute the protected blocking Windows native probe",
  );
  // The acceptance publisher's structural pin (job existence) is a rule
  // instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it. Its
  // publication step fragments are rule instances and live in
  // release-claims.json under workflow_policy.step_fragments;
  // stepFragmentViolations evaluates them.
  const publisherJob = object(object(sourceWorkflow.jobs)[acceptance.publisher_job]);
  add(
    violations,
    sameMembers(needs(publisherJob), [
      "resolve",
      acceptance.hostile_job,
      acceptance.windows_job,
    ])
      && publisherJob["runs-on"] === "ubuntu-latest"
      && publisherJob["timeout-minutes"] === 5
      && [
          "always()",
          "inputs.acceptance_only",
          `needs.${acceptance.hostile_job}.result == 'success'`,
          `needs.${acceptance.windows_job}.result == 'success'`,
        ].every(fragment => String(publisherJob.if ?? "").includes(fragment)),
    "[freeze_barrier] acceptance publisher must depend on both exact successful mutation jobs",
  );
  const receiptDownload = namedStep(
    publisherJob,
    "Download executable release freeze receipt",
  );
  add(
    violations,
    receiptDownload?.uses === "actions/download-artifact@v8.0.1"
      && object(receiptDownload.with).name
        === "${{ needs.resolve.outputs.freeze_artifact_name }}"
      && object(receiptDownload.with).path
        === "${{ runner.temp }}/release-freeze-receipt"
      && stepIndex(publisherJob, "Download executable release freeze receipt")
        < stepIndex(publisherJob, acceptance.publisher_step),
    "[freeze_barrier] acceptance publisher must download the exact Actions receipt before publication",
  );
  // DISPOSITION: step env bindings have no graph evaluator, so the
  // publisher's digest, head, and phase bindings stay code.
  requireStepEnv(
    violations,
    "source-proof.yml",
    publisherJob,
    acceptance.publisher_step,
    {
      FREEZE_RECEIPT_DIGEST: "${{ needs.resolve.outputs.freeze_digest }}",
      HEAD_SHA: "${{ needs.resolve.outputs.ref }}",
      ACCEPTANCE_PHASE: "${{ inputs.acceptance_phase }}",
    },
  );

  for (const file of list(freeze.coordinator_only_workflows)) {
    const workflow = workflows.get(file);
    add(
      violations,
      trigger(workflow, "workflow_call") !== undefined
        && trigger(workflow, "workflow_dispatch") === undefined,
      `[freeze_barrier] ${file} must be callable only through an accepted coordinator`,
    );
  }

  const coordinator = workflows.get("packaged-platform-pr.yml");
  // DISPOSITION: the route job-existence rule lives in the coordinator
  // family's structural pin; this duplicate occurrence stays code so the
  // reviewed verdict multiset is unchanged. The freeze family's own
  // verify-status and exact-head source-proof fragments for the route job are
  // rule instances and live in release-claims.json under
  // workflow_policy.step_fragments beside the coordinator family's wider
  // pins; stepFragmentViolations evaluates them all.
  const route = requireJob(violations, "packaged-platform-pr.yml", coordinator, "route");
  add(
    violations,
    namedStep(route, "Require executable release freeze")?.if === undefined,
    "[freeze_barrier] every packaged proof mode must authenticate the exact candidate head",
  );
  // DISPOSITION: step env bindings have no graph evaluator, so the freeze
  // step's resolved-mode binding stays code.
  requireStepEnv(
    violations,
    "packaged-platform-pr.yml",
    route,
    "Require executable release freeze",
    {
      RESOLVED_MODE: "${{ steps.resolve.outputs.mode }}",
    },
  );
  const packagedSourceProof = namedStep(route, "Require successful exact-head source proof");
  add(
    violations,
    packagedSourceProof?.if
      === "steps.resolve.outputs.mode != 'integration' && steps.resolve.outputs.mode != 'calibration'",
    "[freeze_barrier] calibration must precede the sole frozen-candidate source proof",
  );
  // The coordinator source-proof job's structural pin (job existence) is a
  // rule instance and lives in release-claims.json under
  // workflow_policy.structural_pins; structuralPinViolations evaluates it.
  const packagedSourceJob = object(object(coordinator.jobs)["source-proof"]);
  add(
    violations,
    permissionMapMatches(packagedSourceJob.permissions, {
      actions: "write",
      contents: "read",
      "pull-requests": "read",
      statuses: "write",
    }),
    "[freeze_barrier] packaged source-proof call must grant exactly the reusable workflow permissions",
  );

  const release = workflows.get("release.yml");
  const auto = workflows.get("auto-release.yml");
  add(
    violations,
    at(release, "concurrency", "cancel-in-progress") === true
      && at(auto, "concurrency", "cancel-in-progress") === true,
    "[freeze_barrier] release and auto-release must cancel superseded work",
  );
  add(
    violations,
    object(release.permissions).statuses === undefined
      && object(at(auto, "jobs", "release", "permissions")).statuses === undefined,
    "[freeze_barrier] publication must reuse accepted frozen-candidate proof without an active status",
  );
  // DISPOSITION: the preflight and source-proof job-existence rules live in
  // the release family's structural pins; these duplicate occurrences stay
  // code so the reviewed verdict multiset is unchanged. The freeze family's
  // reuse-resolution fragments (both the required conjuncts and the forbidden
  // freeze-status replay) are rule instances and live in release-claims.json
  // under workflow_policy.step_fragments beside the release family's own
  // pins; stepFragmentViolations evaluates them all.
  const preflight = requireJob(violations, "release.yml", release, "preflight");
  const sourceJob = requireJob(violations, "release.yml", release, "source-proof");
  add(
    violations,
    sourceJob.if === "needs.preflight.outputs.source_proof_reused != 'true'"
      && object(preflight.outputs).source_proof_reused
        === "${{ steps.reuse.outputs.source_proof_reused }}"
      && sourceJob.uses === undefined
      && sourceJob.with === undefined
      && namedStep(sourceJob, "Refuse a second source proof") !== undefined,
    "[freeze_barrier] release must make the post-calibration source-proof fallback unreachable",
  );

  const lineageSource = fs.readFileSync(
    path.join(
      repositoryRoot,
      ".github",
      "scripts",
      "packaged_agent_proof",
      "calibration_lineage.py",
    ),
    "utf8",
  );
  add(
    violations,
    lineageSource.includes("frozen_parents == [calibration_source[\"commit\"]]")
      && lineageSource.includes("Any later commit revokes acceptance")
      && lineageSource.includes("allow_promotion_commit"),
    "[freeze_barrier] calibration lineage must require one direct constant-only child with an explicit promotion exception",
  );
  return violations;
}

export function releaseWorkflowContractViolations(
  workflows,
  graph = loadReleaseClaimGraph(repositoryRoot),
) {
  const violations = [];
  const policy = graph.workflow_policy;
  for (const evidenceType of list(graph.evidence_types).map(object)) {
    for (const lane of list(evidenceType.proof_lanes)) {
      const file = path.basename(String(lane));
      add(
        violations,
        workflows.has(file),
        `[proof_lane] evidence type ${evidenceType.id} workflow ${file} must exist`,
      );
    }
  }
  for (const contract of policy.protected_jobs) {
    const workflow = workflows.get(contract.workflow);
    const job = object(at(workflow, "jobs", contract.job));
    const effectivePermissions = job.permissions === undefined ? workflow?.permissions : job.permissions;
    add(
      violations,
      JSON.stringify(job["runs-on"]) === JSON.stringify(contract.runner),
      `[runner_labels] ${contract.workflow} job ${contract.job} must use ${JSON.stringify(contract.runner)}`,
    );
    add(
      violations,
      job.environment === contract.environment,
      `[protected_environment] ${contract.workflow} job ${contract.job} must use ${contract.environment}`,
    );
    add(
      violations,
      permissionMapMatches(effectivePermissions, contract.permissions),
      `[permissions_secrets] ${contract.workflow} job ${contract.job} effective permissions must exactly match the release claim graph`,
    );
    add(
      violations,
      sameMembers(Object.keys(object(at(workflow, "on", "workflow_call", "secrets"))), contract.secrets),
      `[permissions_secrets] ${contract.workflow} callable secrets must exactly match the release claim graph`,
    );
    const reusableRef = `./.github/workflows/${contract.workflow}`;
    for (const [callerFile, callerWorkflow] of workflows) {
      for (const [callerJobName, callerJobValue] of Object.entries(object(callerWorkflow.jobs))) {
        const callerJob = object(callerJobValue);
        if (callerJob.uses !== reusableRef) continue;
        add(
          violations,
          callerJob.secrets !== "inherit",
          `[permissions_secrets] ${callerFile} job ${callerJobName} must not use secrets: inherit for ${contract.workflow}`,
        );
        if (callerJob.secrets !== undefined && callerJob.secrets !== "inherit") {
          const forwarded = Object.keys(object(callerJob.secrets));
          const undeclared = forwarded.filter((secret) => !contract.secrets.includes(secret));
          add(
            violations,
            undeclared.length === 0,
            `[permissions_secrets] ${callerFile} job ${callerJobName} forwards undeclared secrets to ${contract.workflow}: ${undeclared.join(", ")}`,
          );
        }
      }
    }
  }

  for (const file of policy.artifact_workflows) {
    const workflow = workflows.get(file);
    let uploadCount = 0;
    for (const [jobName, job] of Object.entries(object(workflow?.jobs))) {
      for (const [index, step] of list(job?.steps).entries()) {
        if (step?.uses !== "actions/upload-artifact@v7.0.1") continue;
        uploadCount += 1;
        add(
          violations,
          object(step.with)["retention-days"] === policy.artifact_retention_days,
          `[artifact_retention] ${file} jobs.${jobName}.steps.${index} must retain release evidence for ${policy.artifact_retention_days} days`,
        );
      }
    }
    add(violations, uploadCount > 0, `[artifact_retention] ${file} must upload its release-significant evidence`);
  }

  const release = workflows.get("release.yml");
  for (const jobName of policy.release_chain.exact_sha_jobs) {
    const job = object(at(release, "jobs", jobName));
    const exactSha = jobName === "source-proof" && job.uses === undefined
      ? object(job.env).SOURCE_SHA
      : object(job.with).ref;
    add(
      violations,
      exactSha === policy.promotion.exact_sha_expression,
      `[exact_sha] release.yml job ${jobName} must receive ${policy.promotion.exact_sha_expression}`,
    );
  }
  for (const [jobName, expectedNeeds] of Object.entries(policy.release_chain.dependencies)) {
    add(
      violations,
      sameMembers(needs(at(release, "jobs", jobName)), expectedNeeds),
      `[promotion_boundary] release.yml job ${jobName} dependencies must match the release claim graph`,
    );
  }
  const sourceGuard = workflows.get("main-branch-source-guard.yml");
  const sourceGuardRun = executableRunText(String(findNamedStep(sourceGuard, "Require dev/codestory-next source branch")?.run ?? ""));
  add(
    violations,
    includesAll(at(sourceGuard, "on", "pull_request", "branches"), [policy.promotion.release_branch])
      && sourceGuardRun.includes(policy.promotion.source_branch),
    `[promotion_boundary] main-branch source guard must require ${policy.promotion.source_branch} into ${policy.promotion.release_branch}`,
  );
  add(
    violations,
    includesAll(at(workflows.get("auto-release.yml"), "on", "push", "branches"), [policy.promotion.release_branch]),
    `[promotion_boundary] auto-release.yml must run from ${policy.promotion.release_branch}`,
  );
  const matrixViolations = [];
  validatePackageMatrixExpression(
    matrixViolations,
    at(workflows.get("packaged-platform-proof.yml"), "jobs", "build", "strategy", "matrix"),
    graph,
  );
  violations.push(...matrixViolations.map((message) => `[target_matrix] ${message}`));

  for (const file of policy.promotion.label_routed_workflows) {
    const workflow = workflows.get(file);
    add(
      violations,
      sameMembers(at(workflow, "on", "pull_request", "types"), policy.promotion.required_events),
      `[proof_identity] ${file} must use only ${policy.promotion.required_events.join(" and ")} pull-request events`,
    );
    add(
      violations,
      String(at(workflow, "concurrency", "group") ?? "").includes(policy.promotion.proof_run_sha_expression),
      `[proof_identity] ${file} concurrency must bind ${policy.promotion.proof_run_sha_expression}`,
    );
    const resolver = findNamedStep(workflow, "Resolve trusted exact head");
    const run = executableRunText(String(resolver?.run ?? ""));
    add(
      violations,
      run.includes("current_head") && run.includes("EVENT_HEAD_SHA"),
      `[proof_identity] ${file} must resolve the current head and compare its exact SHA before executing labeled work`,
    );
  }
  violations.push(...releaseFreezeBarrierWorkflowViolations(workflows, graph));
  return violations;
}

function validateReleaseCellUploadOwnership(workflows, violations) {
  const actual = [];
  for (const [file, workflow] of workflows) {
    for (const [jobId, jobValue] of Object.entries(object(workflow.jobs))) {
      for (const step of Array.isArray(jobValue.steps) ? jobValue.steps : []) {
        if (step?.uses !== "actions/upload-artifact@v7.0.1") continue;
        const artifactName = String(object(step.with).name ?? "");
        if (artifactName.startsWith("release-cell-")) {
          actual.push(`${file}/${jobId}/${artifactName}`);
        }
      }
    }
  }
  const expected = [
    "source-proof.yml/full-source-gate/release-cell-prepublish-source-attempt-${{ github.run_attempt }}",
    "packaged-platform-proof.yml/build/release-cell-prepublish-package-${{ matrix.asset_target }}-attempt-${{ github.run_attempt }}",
    "macos-metal-proof.yml/packaged-metal/release-cell-prepublish-macos-arm64-metal-attempt-${{ github.run_attempt }}",
    "macos-metal-proof.yml/packaged-metal/release-cell-postpublish-retrieval-macos-arm64-attempt-${{ github.run_attempt }}",
    "macos-metal-proof.yml/packaged-metal/release-cell-prepublish-candidate-installed-macos-arm64-attempt-${{ github.run_attempt }}",
    "windows-vulkan-proof.yml/packaged-vulkan/release-cell-prepublish-windows-x64-vulkan-attempt-${{ github.run_attempt }}",
    "windows-vulkan-proof.yml/packaged-vulkan/release-cell-postpublish-retrieval-windows-x64-attempt-${{ github.run_attempt }}",
    "windows-vulkan-proof.yml/packaged-vulkan/release-cell-prepublish-candidate-installed-windows-x64-attempt-${{ github.run_attempt }}",
    "linux-vulkan-proof.yml/packaged-vulkan/release-cell-prepublish-linux-x64-vulkan-attempt-${{ github.run_attempt }}",
    "linux-vulkan-proof.yml/packaged-vulkan/release-cell-postpublish-retrieval-linux-x64-attempt-${{ github.run_attempt }}",
    "linux-vulkan-proof.yml/packaged-vulkan/release-cell-prepublish-candidate-installed-linux-x64-attempt-${{ github.run_attempt }}",
    "post-publish-release-smoke.yml/smoke/release-cell-postpublish-${{ matrix.asset_target }}-attempt-${{ github.run_attempt }}",
    // The withheld-claim producer is the one job allowed to write a cell it did not prove, and it
    // owns exactly one attempt-qualified artifact per protected host and closeout phase.
    "release.yml/accelerator-non-claim/release-cell-nonclaim-prepublish-macos-arm64-metal-attempt-${{ github.run_attempt }}",
    "release.yml/accelerator-non-claim/release-cell-nonclaim-postpublish-macos-arm64-metal-attempt-${{ github.run_attempt }}",
    "release.yml/accelerator-non-claim/release-cell-nonclaim-prepublish-windows-x64-vulkan-attempt-${{ github.run_attempt }}",
    "release.yml/accelerator-non-claim/release-cell-nonclaim-postpublish-windows-x64-vulkan-attempt-${{ github.run_attempt }}",
    "release.yml/accelerator-non-claim/release-cell-nonclaim-prepublish-linux-x64-vulkan-attempt-${{ github.run_attempt }}",
    "release.yml/accelerator-non-claim/release-cell-nonclaim-postpublish-linux-x64-vulkan-attempt-${{ github.run_attempt }}",
  ];
  add(
    violations,
    JSON.stringify(actual.sort()) === JSON.stringify(expected.sort()),
    "release-cell Actions artifact names must have one graph-owned producer job and attempt suffix",
  );
}

/// Every `${{ ... }}` in a piece of text, bounded by the `}}` that actually closes it.
///
/// A non-greedy `/\$\{\{[\s\S]*?\}\}/` stops at the first `}}` it sees, which is not always the
/// terminator. GitHub's expression grammar puts braces inside expressions -- `fromJSON('{"a":1}')`
/// carries one, and `format('{{Hello {0}}}', ...)`, the brace escape from GitHub's own expression
/// documentation, carries a run of them. Against `${{ format('{{Hello {0}}}', inputs.ref) }}` the
/// non-greedy form returned `${{ format('{{Hello {0}}`, which names no context at all, so a rule
/// reading these spans saw a clean file and GitHub still spliced the input. Braces are counted
/// here and the span ends at the `}}` that closes the expression itself.
///
/// Single-quoted literals are skipped whole, with `''` read as GitHub's escape for one quote, so a
/// brace inside a string cannot move the count in either direction. Text that opens an expression
/// and never closes it yields the rest of the text rather than nothing: an unreadable expression is
/// not evidence that it is harmless.
export function interpolationSpans(text) {
  const source = String(text);
  const spans = [];
  let cursor = 0;
  for (;;) {
    const start = source.indexOf("${{", cursor);
    if (start === -1) return spans;
    let depth = 0;
    let quoted = false;
    let end = -1;
    for (let index = start + 3; index < source.length; index += 1) {
      const character = source[index];
      if (quoted) {
        if (character !== "'") continue;
        if (source[index + 1] === "'") index += 1;
        else quoted = false;
        continue;
      }
      if (character === "'") quoted = true;
      else if (character === "{") depth += 1;
      else if (character === "}") {
        if (depth > 0) depth -= 1;
        else if (source[index + 1] === "}") {
          end = index + 2;
          break;
        }
      }
    }
    if (end === -1) {
      spans.push(source.slice(start));
      return spans;
    }
    spans.push(source.slice(start, end));
    cursor = end;
  }
}

/// Any mention of the `inputs` context, however it is spelled. GitHub serves the same dispatched
/// value under `inputs.version`, `github.event.inputs.version`, and `inputs['version']`, and an
/// expression can bury it in a function call, so this matches the context name itself rather than
/// any one path through it. `outputs` does not contain `inputs`, and a word character before it
/// (`my_inputs`) is not the context.
const namesADispatchInput = /\binputs\b/u;

/// The contexts a dispatched value can be standing in when a script reads it one hop later. Each
/// one is a channel, not a value: nothing at the reading site says what was put into it.
///
/// `env` -- a workflow-, job-, or step-level `env:` entry may be bound to `${{ inputs.x }}`, and
///   `${{ env.NAME }}` in a script is then the input, spliced as text. This PR alone created 117
///   step-level `env:` bindings carrying inputs, so this is the shape the next author reaches for.
/// `steps.*.outputs.*` -- a step that receives an input can write it to `$GITHUB_OUTPUT`, and the
///   consuming `${{ steps.x.outputs.y }}` is again text.
/// `needs.*.outputs.*` -- a job output is a step output that crossed a job boundary.
///
/// The remedy is the same one #1566 applied 117 times: bind the value in `env:` and read `$NAME`.
/// For `env` specifically it costs nothing at all -- a workflow- or job-level `env:` entry is
/// already exported into the shell, so `$NAME` is available with no new binding.
///
/// Not claimed here: `github.*` can carry attacker-authored text (a pull request title), which is a
/// different surface with a different argument. `matrix.*` can be built from an input, which is
/// pinned where the matrix is built (`fromJSON` over a fixed set of literals) rather than here.
const launderingContexts = [
  [/\benv\b/u, "env"],
  [/\bsteps\b[\s\S]*\boutputs\b/u, "a step output"],
  [/\bneeds\b[\s\S]*\boutputs\b/u, "a job output"],
  // For `workflow_dispatch`, `github.event` *is* the inputs container, so serialising the event
  // carries every dispatched value into script text without the word `inputs` ever appearing --
  // `toJSON` preserves `$(` and backticks intact. `\bevent\b` does not match inside
  // `github.event_name`, because `_` is a word character, so the ordinary trigger read is untouched.
  [/\bgithub\b[\s\S]*\bevent\b/u, "the event payload"],
];

export function interpolatedDispatchInputs(run) {
  return interpolationSpans(run).filter(expression => namesADispatchInput.test(expression));
}

/// Every interpolation in `run` that reaches a dispatched value, paired with why it can.
export function interpolatedInputChannels(run) {
  const found = [];
  for (const expression of interpolationSpans(run)) {
    if (namesADispatchInput.test(expression)) {
      found.push([expression, "a dispatch input"]);
      continue;
    }
    const laundering = launderingContexts.find(([pattern]) => pattern.test(expression));
    if (laundering !== undefined) found.push([expression, laundering[1]]);
  }
  return found;
}

/// Dispatched values must reach a script through `env:`, never through the script's own text.
///
/// Expression interpolation happens before any shell exists: GitHub splices the value into the
/// `run:` body as characters, and the shell then parses the result. Double quotes do not stop
/// `$(...)` or backticks, so a dispatcher who can name the value can run commands on the runner --
/// beside whatever `GH_TOKEN`, environment secret, or self-hosted host state that step carries.
/// `env:` is not textual: the value arrives as a variable and `"$VAR"` is inert.
///
/// #1554 fixed this in marketplace-sync.yml and pinned the fix with `validateMarketplaceSync`, a
/// validator named after one file. That shape cannot fail on a second file no matter how many
/// times the same splice is written, and eight other workflows carried it. This rule is driven by
/// the loaded workflow set instead, so it reads whatever workflows exist at the time it runs and a
/// workflow added tomorrow is covered without anyone editing this function.
///
/// The rule reads `run:` only. A dispatched value in an action input (`with.ref`) or an `if:` is a
/// different surface with a different argument, pinned separately where it belongs.
///
/// Naming the `inputs` context alone was not enough. The context is only where the value is at the
/// moment the rule looks: an author who binds it into `env:` and reads `${{ env.NAME }}` one line
/// later, or writes it to `$GITHUB_OUTPUT` and reads `${{ steps.x.outputs.y }}` one step later, has
/// rebuilt #1566 with the gate green. The channels a dispatched value can be sitting in are refused
/// with it, so closing the surface does not depend on spotting where the value came from.
export function dispatchInputInterpolationViolations(workflows) {
  const violations = [];
  for (const [file, workflow] of workflows) {
    for (const [jobId, job] of Object.entries(object(workflow.jobs))) {
      for (const [index, rawStep] of list(object(job).steps).entries()) {
        const step = object(rawStep);
        if (typeof step.run !== "string") continue;
        const named = step.name ? ` (${step.name})` : "";
        const seen = new Set();
        for (const [expression, channel] of interpolatedInputChannels(step.run)) {
          if (seen.has(expression)) continue;
          seen.add(expression);
          violations.push(
            `${file} jobs.${jobId}.steps.${index}${named} must read ${expression}`
              + ` from step env, not interpolated script text: it carries ${channel}`,
          );
        }
      }
    }
  }
  return violations;
}

/// Routing a value through `env:` moves the read from GitHub's interpolator into the shell, so the
/// script stops being shell-independent the moment it does.
///
/// `${{ env.NAME }}` is spliced before any shell exists and reads the same everywhere. `"$NAME"` is
/// a bash read; under pwsh -- the runner default on Windows -- it is the literal `$NAME` if it
/// resolves to anything at all, and the correct read is `$env:NAME`. So a step that consumes a
/// binding on a job that can land on a Windows runner has to say which shell it was written for.
/// Every affected step in this repository declares one; this keeps that true, because the failure
/// mode is a proof that silently compares against an empty string rather than an error.
export function shellDependentBindingViolations(workflows) {
  const violations = [];
  const bashRead = name => new RegExp(`\\$\\{?${name}\\b`, "u");
  for (const [file, workflow] of workflows) {
    const workflowShell = at(workflow, "defaults", "run", "shell");
    for (const [jobId, rawJob] of Object.entries(object(workflow.jobs))) {
      const job = object(rawJob);
      // `runs-on` is often an expression, so the platform is not always readable here. Anything
      // that is not a literal non-Windows label is treated as reaching Windows.
      const label = JSON.stringify(job["runs-on"] ?? "");
      const known = /^"(ubuntu|macos)[\w.-]*"$/u.test(label);
      if (known) continue;
      const jobShell = at(job, "defaults", "run", "shell") ?? workflowShell;
      for (const [index, rawStep] of list(job.steps).entries()) {
        const step = object(rawStep);
        if (typeof step.run !== "string") continue;
        if ((step.shell ?? jobShell) !== undefined) continue;
        const bound = Object.keys(object(step.env))
          .concat(Object.keys(object(job.env)), Object.keys(object(workflow.env)))
          .filter(name => bashRead(name).test(step.run)
            && !new RegExp(`\\$env:${name}\\b`, "u").test(step.run));
        if (bound.length === 0) continue;
        const named = step.name ? ` (${step.name})` : "";
        violations.push(
          `${file} jobs.${jobId}.steps.${index}${named} reads ${bound.sort().join(", ")}`
            + " as a shell variable on a job that can run on Windows and must declare its shell",
        );
      }
    }
  }
  return violations;
}

/// A script that absorbs its own failure has to hand that failure to something that does not.
///
/// `continue-on-error` lives outside the script, so nothing the script's own text asserts can see
/// it, and it turns a gate's `exit 1` into advice. Putting it on plugin-static.yml's
/// `Check workflow policy` step would silence this file and its whole test suite while the run
/// still reported green -- the commands that step runs are pinned, its blocking-ness was not.
///
/// The rule is not "gates must be blocking", because the repository has scripts that deliberately
/// are not: source-proof compiles and lints under `continue-on-error` so a later step can save the
/// cache before failing the job, and both release lanes push the marketplace catalog that way so a
/// credential problem cannot strand an already-published release. What those have and a silenced
/// gate does not is a *successor*: an `id:`, and another step that reads `steps.<id>.outcome` and
/// fails on it. So absorbing a failure is allowed exactly when the failure is still required
/// somewhere, and a step that absorbs its failure into nothing is refused.
///
/// Scoped to `run:` steps. The optional cache restores are `uses:` steps whose miss is the normal
/// path and carries no outcome to require -- their non-blocking-ness is separately required, and
/// this rule must not contradict that.
/// Every `if:` removed, at any depth. A condition decides whether something
/// runs; it can never make a run fail, which is why neither the job-level nor
/// the step-level successor may be found in one.
function withoutConditions(value) {
  if (Array.isArray(value)) return value.map(withoutConditions);
  if (value === null || typeof value !== "object") return value;
  return Object.fromEntries(
    Object.entries(value)
      .filter(([key]) => key !== "if")
      .map(([key, nested]) => [key, withoutConditions(nested)]),
  );
}

export function absorbedFailureViolations(workflows) {
  const violations = [];
  const absorbs = value => value !== undefined && value !== false;
  for (const [file, workflow] of workflows) {
    for (const [jobId, rawJob] of Object.entries(object(workflow.jobs))) {
      const job = object(rawJob);
      // A job-level `continue-on-error` downgrades every step it contains at once, and the only
      // thing that can still require the failure is a downstream job reading `needs.<id>.result`.
      if (absorbs(job["continue-on-error"])) {
        // The separately validated frozen-candidate adjunct is intentionally unclaimed and non-gating,
        // including runner loss and timeout. It cannot appear in a downstream `needs` edge:
        // doing so would turn optional evidence back into a closeout dependency. Its exact job
        // structure, activation, protected host, cache boundary, evaluator, and outcome recorder
        // are pinned by validatePackagedCoordinator and the whole-workflow digest.
        const isOptionalFrozenCandidateQuality =
          file === frozenCandidateQualityWorkflowRef.slice(
            frozenCandidateQualityWorkflowRef.lastIndexOf("/") + 1,
          )
          && (
            jobId === "quality"
            || jobId === "windows-quality"
            || jobId === "linux-quality"
          );
        // Reading the outcome is not requiring it one level up either, and the
        // job-level rule used to accept exactly the `if:`-only read the
        // step-level rule below deliberately refuses -- it scanned every string
        // under `jobs`, including the absorbing job's own text and every
        // condition. A successor is a *downstream, blocking* job that receives
        // `needs.<id>.result` somewhere other than a condition, where a script
        // can still test it and exit non-zero.
        const requiredDownstream = Object.entries(object(workflow.jobs)).some(
          ([otherId, rawOther]) => {
            if (otherId === jobId) return false;
            const other = object(rawOther);
            if (absorbs(other["continue-on-error"])) return false;
            return scalarStrings(withoutConditions(other)).some(
              text => text.includes(`needs.${jobId}.result`),
            );
          },
        );
        add(
          violations,
          isOptionalFrozenCandidateQuality || requiredDownstream,
          `${file} jobs.${jobId} absorbs its own failure and must have needs.${jobId}.result required`,
        );
      }
      const steps = list(job.steps).map(step => object(step));
      // Reading the outcome is not requiring it. `if: steps.x.outcome == 'success'` only decides
      // whether the reader runs, and a skipped step is not a failed job; a reader that absorbs its
      // own failure cannot fail the job on what it read either, so it just moves the same question
      // one step along. A successor is therefore a blocking step that receives the outcome
      // somewhere other than its own `if:` -- where a script can still `test` it and exit non-zero.
      const requires = outcome => steps.some(other => {
        if (absorbs(other["continue-on-error"])) return false;
        const consumed = { ...other };
        delete consumed.if;
        return scalarStrings(consumed).some(text => text.includes(outcome));
      });
      for (const [index, step] of steps.entries()) {
        if (typeof step.run !== "string") continue;
        if (!absorbs(step["continue-on-error"])) continue;
        const named = step.name ? ` (${step.name})` : "";
        add(
          violations,
          typeof step.id === "string" && requires(`steps.${step.id}.outcome`),
          `${file} jobs.${jobId}.steps.${index}${named} absorbs its own failure and must have`
            + " an id whose outcome a later blocking step requires",
        );
      }
    }
  }
  return violations;
}

const JOB_EVIDENCE_COLLECTOR = ".github/scripts/collect-actions-job-evidence.sh";

/// `checks: read` is the token scope that makes the lost-runner signature readable at all.
///
/// The signature's first part is a job annotation, and GET /repos/{o}/{r}/check-runs/{id}/annotations
/// is gated on that scope. A workflow that runs the collector without it gets a 403, which the
/// collector now refuses rather than reporting as "no annotations" -- so the missing scope stops a
/// release instead of quietly making recovery impossible. This rule catches it before the release,
/// in every workflow that reaches the collector, including the reusable-workflow callers whose own
/// grant is the ceiling for everything they call.
export function annotationScopeViolations(workflows) {
  const violations = [];
  const grantsChecksRead = permissions => object(permissions).checks === "read";
  const collectorWorkflows = new Set();
  for (const [file, workflow] of workflows) {
    for (const [jobId, job] of Object.entries(object(workflow.jobs))) {
      const runsCollector = list(object(job).steps)
        .some(step => String(object(step).run ?? "").includes(JOB_EVIDENCE_COLLECTOR));
      if (!runsCollector) continue;
      collectorWorkflows.add(file);
      // A job-level `permissions:` block replaces the workflow-level one outright, so the effective
      // grant is whichever of the two the job actually has.
      const effective = object(job).permissions !== undefined
        ? object(job).permissions
        : object(workflow).permissions;
      add(
        violations,
        grantsChecksRead(effective),
        `${file} job ${jobId} reads Actions job annotations and must grant checks: read`,
      );
    }
  }
  for (const [file, workflow] of workflows) {
    for (const [jobId, job] of Object.entries(object(workflow.jobs))) {
      const uses = String(object(job).uses ?? "");
      if (!uses.startsWith("./.github/workflows/")) continue;
      if (!collectorWorkflows.has(uses.slice(uses.lastIndexOf("/") + 1))) continue;
      add(
        violations,
        grantsChecksRead(object(job).permissions),
        `${file} job ${jobId} calls a workflow that reads job annotations and must pass checks: read`,
      );
    }
  }
  add(
    violations,
    collectorWorkflows.size > 0,
    `no workflow runs ${JOB_EVIDENCE_COLLECTOR}, so the lost-runner signature is never collected`,
  );
  return violations;
}

/// The two halves of the lost-runner contract: a bounded automatic re-dispatch, and a withheld
/// claim once that bound is spent.
///
/// Both are places where a gate is being relaxed, so the policy pins the shapes that keep the
/// relaxation honest: the rerun names individual lost jobs instead of asking Actions to rerun every
/// failure, the recovery never waits on a human, and the withheld-claim producer decides from the
/// shared classifier rather than from "the proof job went red".
export function lostRunnerRecoveryViolations(workflows, graph) {
  const violations = [];
  const policy = graph.non_claim_policy;
  const rerunPolicy = object(graph.workflow_policy.lost_runner_rerun);
  const rerunFile = "lost-runner-rerun.yml";
  const rerun = workflows.get(rerunFile);
  add(
    violations,
    MAXIMUM_RUN_ATTEMPTS === policy.maximum_run_attempts,
    `${rerunFile} recovery bound must equal the release claim graph maximum_run_attempts`,
  );
  add(
    violations,
    rerunPolicy.workflow === rerunFile
      && rerunPolicy.job === "rerun-lost-jobs"
      && rerunPolicy.recovery_contract === policy.recovery_contract,
    "the release claim graph lost_runner_rerun block must name the recovery workflow and contract",
  );
  add(
    violations,
    rerunPolicy.plan_schema === RERUN_PLAN_SCHEMA
      && rerunPolicy.dispatch_schema === RERUN_DISPATCH_SCHEMA,
    "the release claim graph must record the recovery plan and dispatch receipt schemas the contract emits",
  );
  // Two facts the recovery cannot be reasoned about without: which execution of a job the plan
  // reads, and what one refused re-dispatch does to the others.
  add(
    violations,
    rerunPolicy.selection === "latest_execution_per_job_name"
      && rerunPolicy.dispatch_tolerance === "per_job_id"
      && rerunPolicy.dispatch_failure === "fail_only_when_no_job_was_accepted",
    "the release claim graph must declare latest-execution rerun selection and per-id dispatch tolerance",
  );
  add(
    violations,
    LOST_RUNNER_ANNOTATION === policy.annotation,
    `${rerunFile} recovery contract must key on the annotation the release claim graph records`,
  );
  if (!rerun) {
    violations.push(`${rerunFile} must exist`);
  } else {
    const trigger = object(at(rerun, "on", "workflow_run"));
    add(
      violations,
      includesAll(trigger.workflows, ["Auto Release", "Release"])
        && includesAll(trigger.types, ["completed"]),
      `${rerunFile} must observe completed release runs`,
    );
    add(
      violations,
      JSON.stringify(Object.entries(object(rerun.permissions)).sort())
        === JSON.stringify([["actions", "write"], ["checks", "read"], ["contents", "read"]]),
      `${rerunFile} must hold only the Actions write and annotation read scopes its recovery needs`,
    );
    const job = requireJob(violations, rerunFile, rerun, "rerun-lost-jobs");
    // The repository requires machine recovery: an environment on this job would put a human click
    // between a dropped connection and the retry, which is the failure this workflow exists to fix.
    add(
      violations,
      object(job).environment === undefined,
      `${rerunFile} recovery must not wait on an approval environment`,
    );
    add(
      violations,
      String(object(job).if ?? "").includes("github.event.workflow_run.conclusion == 'failure'"),
      `${rerunFile} must act only on a failed release run`,
    );
    requireStepRun(violations, rerunFile, job, "Collect Actions failure evidence", [
      "bash .github/scripts/collect-actions-job-evidence.sh",
    ]);
    requireStepRun(violations, rerunFile, job, "Plan the bounded rerun", [
      "node .github/scripts/lost-runner-recovery.mjs plan-rerun",
      `--out ${rerunPolicy.plan_file}`,
    ]);
    const dispatchStep = String(rerunPolicy.dispatch_step ?? "");
    const dispatch = namedStep(job, dispatchStep);
    add(
      violations,
      dispatch?.if === "steps.plan.outputs.rerun == 'true'",
      `${rerunFile} re-dispatch must be gated on the classified recovery plan`,
    );
    requireStepRun(violations, rerunFile, job, dispatchStep, [
      String(rerunPolicy.dispatch_command ?? ""),
      `--plan ${rerunPolicy.plan_file}`,
      `--out ${rerunPolicy.dispatch_file}`,
    ]);
    // The hosts share one machine, so a single incident loses two of them at once. A shell loop
    // calling the API directly stops at the first refusal under `set -e` and abandons the recovery
    // the remaining ids are still owed, so the step must delegate to the contract that records a
    // per-id outcome and fails only when nothing at all was accepted.
    forbidStepRun(violations, rerunFile, job, dispatchStep, ["gh api", "for job_id in"]);
    // Re-running every failed job would sweep an assertion failure back into the queue alongside
    // the lost one; the plan names ids, so the API call must be the per-job endpoint.
    add(
      violations,
      !scalarStrings(rerun).some(value => value.includes("rerun-failed-jobs")),
      `${rerunFile} must re-dispatch named lost jobs, never every failed job`,
    );
  }

  const releaseFile = "release.yml";
  const release = workflows.get(releaseFile);
  if (!release) return violations;
  const job = requireJob(violations, releaseFile, release, "accelerator-non-claim");
  add(
    violations,
    sameMembers(needs(job), graph.workflow_policy.release_chain.dependencies["accelerator-non-claim"]),
    `${releaseFile} non-claim dependencies must match the release claim graph`,
  );
  add(
    violations,
    job.name === policy.producer_job_name,
    `${releaseFile} non-claim job name must equal the release claim graph producer_job_name`,
  );
  add(
    violations,
    object(job).environment === undefined,
    `${releaseFile} non-claim producer must not wait on an approval environment`,
  );
  add(
    violations,
    String(object(job).if ?? "").startsWith("always()"),
    `${releaseFile} non-claim producer must observe every accelerator outcome`,
  );
  requireStepRun(violations, releaseFile, job, "Collect protected accelerator job evidence", [
    "bash .github/scripts/collect-actions-job-evidence.sh",
    "non_claim_policy.hosts",
  ]);
  requireStepRun(violations, releaseFile, job, "Decide withheld accelerator hosts", [
    "node .github/scripts/lost-runner-recovery.mjs plan-non-claim",
  ]);
  const recordDownload = namedStep(
    job,
    "Download authenticated candidate records for withheld identity",
  );
  add(
    violations,
    recordDownload?.if === "steps.non-claim.outputs.withheld_hosts != ''"
      && recordDownload?.uses === "actions/download-artifact@v8.0.1"
      && hasExactKeys(object(recordDownload?.with), [
        "merge-multiple",
        "path",
        "pattern",
      ])
      && object(recordDownload?.with).pattern
        === "codestory-candidate-archive-record-*"
      && object(recordDownload?.with).path
        === "target/release-non-claim/candidate-records"
      && object(recordDownload?.with)["merge-multiple"] === false,
    `${releaseFile} non-claim producer must download only tiny authenticated candidate records`,
  );
  const record = namedStep(job, "Record populated accelerator non-claims");
  add(
    violations,
    record?.if === "steps.non-claim.outputs.withheld_hosts != ''",
    `${releaseFile} non-claim cells must be written only for hosts the classifier withheld`,
  );
  requireStepRun(violations, releaseFile, job, "Record populated accelerator non-claims", [
    "scripts/codestory-release-cell-manifest.mjs withhold",
    '--producer-run-attempt "$GITHUB_RUN_ATTEMPT"',
    '--candidate-record "target/release-non-claim/candidate-records/codestory-candidate-archive-record-$target/candidate-archive-record.json"',
  ]);
  add(
    violations,
    !shellLiteralNormalizedText(stepRun(
      job,
      "Record populated accelerator non-claims",
    )).includes("--archive ")
      && !scalarStrings(recordDownload).some(value =>
        value.includes("codestory-cli-")),
    `${releaseFile} non-claim producer must never transfer or read a large package archive`,
  );
  // A non-claim producer that emitted evidence for a host that reported would overwrite a real
  // proof, so every upload is bound to the classifier's own withheld list. Each closeout phase gets
  // its own container: a phase authorizes every manifest inside the container it downloads, so a
  // container mixing phases would carry a manifest that phase's producer map never selected.
  for (const host of policy.hosts) {
    for (const [phase, artifact] of Object.entries(host.producer_artifacts)) {
      const prefix = artifact.replace("-attempt-{attempt}", "");
      const upload = [...list(job.steps)].find(step =>
        String(object(object(step).with).name ?? "").startsWith(prefix));
      add(
        violations,
        upload?.if === `contains(steps.non-claim.outputs.withheld_hosts, '${host.id}')`
          && String(object(object(upload).with).path ?? "").endsWith(`/${host.id}/${phase}`),
        `${releaseFile} withheld ${host.id} ${phase} cells must upload only that phase for a withheld host`,
      );
    }
  }
  const closeout = requireJob(violations, releaseFile, release, "pre-publish-closeout");
  add(
    violations,
    String(object(closeout).if ?? "").includes("needs.accelerator-non-claim.result == 'success'"),
    `${releaseFile} pre-publish closeout must require a decided non-claim outcome`,
  );
  return violations;
}

/// The pre-dispatch reservation contract: the heartbeat workflow whose tiny jobs run ON the three
/// protected hosts, the release gate that types each host before any proof is dispatched, the
/// per-host dispatch conditions on the proof calls, the receipt feed into the non-claim producer,
/// and the closeouts' own receipt collection.
///
/// Every pin here is a place where a gate is being relaxed -- an absent proof may now be withheld
/// instead of blocking -- so the shapes that keep the relaxation honest are pinned: the gate never
/// waits on a human, the receipt has exactly one producer, the proof jobs dispatch only for hosts
/// the receipt typed alive, and the closeouts download the receipt themselves rather than
/// inheriting the non-claim producer's verdict.
export function protectedRunnerReservationViolations(workflows, graph) {
  const violations = [];
  const policy = graph.non_claim_policy;
  const reservation = object(policy.reservation ?? {});
  const heartbeat = object(reservation.heartbeat ?? {});
  const heartbeatFile = String(heartbeat.workflow ?? "").split("/").at(-1);
  const hostFlag = host => `dispatch_${host.id.replaceAll(/[^A-Za-z0-9_]/gu, "_")}`;

  // The receipt schema and the recheck bound are the same facts the recovery contract exports,
  // so a graph that drifts from the code is a violation even with the workflows untouched.
  add(
    violations,
    reservation.schema === RESERVATION_RECEIPT_SCHEMA,
    "the release claim graph reservation schema must match the recovery contract",
  );
  add(
    violations,
    reservation.maximum_observation_attempts === MAXIMUM_RUN_ATTEMPTS,
    "the release claim graph reservation recheck bound must mirror MAXIMUM_RUN_ATTEMPTS",
  );

  const heartbeatWorkflow = workflows.get(heartbeatFile);
  if (!heartbeatWorkflow) {
    violations.push(`${heartbeatFile} must exist`);
  } else {
    const schedule = list(at(heartbeatWorkflow, "on", "schedule") ?? []);
    add(
      violations,
      schedule.length === 1 && object(schedule[0]).cron === heartbeat.cron,
      `${heartbeatFile} must run on the graph-declared ${String(heartbeat.cron)} schedule`,
    );
    add(
      violations,
      trigger(heartbeatWorkflow, "workflow_dispatch") !== undefined,
      `${heartbeatFile} must accept the bounded recheck dispatch`,
    );
    // Queued heartbeats behind a busy or dead host must never pile up: each run supersedes the
    // one before it.
    add(
      violations,
      concurrencyCancels(heartbeatWorkflow),
      `${heartbeatFile} must cancel superseded heartbeats`,
    );
    add(
      violations,
      JSON.stringify(object(heartbeatWorkflow.permissions)) === "{}",
      `${heartbeatFile} must hold no token scopes at all`,
    );
    add(
      violations,
      !scalarStrings(heartbeatWorkflow).some(value => value.includes("secrets.")),
      `${heartbeatFile} must reference no secrets`,
    );
    const jobs = Object.entries(object(heartbeatWorkflow.jobs));
    add(
      violations,
      jobs.length === list(policy.hosts).length,
      `${heartbeatFile} must define exactly one heartbeat job per protected host`,
    );
    for (const host of policy.hosts) {
      const matches = jobs.filter(([, job]) => object(job).name === host.heartbeat_job_name);
      if (matches.length !== 1) {
        violations.push(`${heartbeatFile} must define one job named ${host.heartbeat_job_name}`);
        continue;
      }
      const [jobId, job] = matches[0];
      add(
        violations,
        JSON.stringify(job["runs-on"]) === JSON.stringify(host.runner_labels),
        `${heartbeatFile} jobs.${jobId} must run on ${JSON.stringify(host.runner_labels)}`,
      );
      add(
        violations,
        job["timeout-minutes"] === 5,
        `${heartbeatFile} jobs.${jobId} must keep the tiny 5-minute heartbeat budget`,
      );
      add(
        violations,
        job.environment === undefined,
        `${heartbeatFile} jobs.${jobId} must not wait on an approval environment`,
      );
      // The only fact a heartbeat proves is that the runner completed a job; checking anything
      // out or downloading anything onto the protected host proves nothing more and widens the
      // host's exposure.
      add(
        violations,
        list(job.steps).every(step => object(step).uses === undefined),
        `${heartbeatFile} jobs.${jobId} must run no actions on the protected host`,
      );
    }
  }

  const releaseFile = "release.yml";
  const release = workflows.get(releaseFile);
  if (!release) return violations;
  const gateJobId = String(reservation.producer_job ?? "");
  const gate = requireJob(violations, releaseFile, release, gateJobId);
  // The gate sits between the packaged proof and every protected dispatch. A human approval here
  // would put a click between a finished build and its proofs on every release; an `if:` here
  // would make "the hosts were never typed" reachable, which is the invisible-queue state this
  // gate exists to remove.
  add(
    violations,
    object(gate).environment === undefined,
    `${releaseFile} ${gateJobId} must not wait on an approval environment`,
  );
  add(
    violations,
    object(gate).if === undefined,
    `${releaseFile} ${gateJobId} must type the hosts unconditionally`,
  );
  const reserveStepName = "Type each protected host from its heartbeat evidence";
  requireStepRun(violations, releaseFile, gate, reserveStepName, [
    "node .github/scripts/lost-runner-recovery.mjs plan-reservation",
    "--out target/runner-reservation/receipt.json",
    ".non_claim_policy.reservation.heartbeat.workflow",
    ".non_claim_policy.reservation.heartbeat.tolerance_minutes",
    // The bounded recheck is a fresh heartbeat dispatch, not a longer wait on stale evidence.
    "gh workflow run",
  ]);
  for (const host of policy.hosts) {
    add(
      violations,
      object(gate.outputs)[hostFlag(host)]
        === `\${{ steps.reserve.outputs.${hostFlag(host)} }}`,
      `${releaseFile} ${gateJobId} must publish the ${hostFlag(host)} receipt flag`,
    );
  }
  const receiptArtifactName = String(reservation.receipt_artifact ?? "")
    .replaceAll("{attempt}", "${{ github.run_attempt }}");
  const receiptUpload = list(object(gate).steps ?? []).find(step =>
    object(object(step).with).name === receiptArtifactName);
  add(
    violations,
    receiptUpload !== undefined
      && receiptUpload.if === "always()"
      && String(object(receiptUpload?.with).path ?? "").endsWith("receipt.json"),
    `${releaseFile} ${gateJobId} must upload the ${receiptArtifactName} receipt even when it fails red`,
  );
  // Exactly one producer for the receipt artifact, anywhere in the repository: a second uploader
  // could hand the closeouts a receipt no reservation ever wrote.
  for (const [file, workflow] of workflows) {
    for (const [jobId, jobValue] of Object.entries(object(workflow.jobs))) {
      for (const step of Array.isArray(object(jobValue).steps) ? jobValue.steps : []) {
        const uploadName = String(object(object(step).with).name ?? "");
        if (!uploadName.startsWith("protected-runner-reservation-")) continue;
        if (typeof object(step).uses !== "string" || !object(step).uses.startsWith("actions/upload-artifact")) continue;
        add(
          violations,
          file === releaseFile && jobId === gateJobId,
          `${file} job ${jobId} must not upload the reservation receipt ${uploadName}`,
        );
      }
    }
  }

  // Proof jobs dispatch only for hosts the receipt typed alive, host by host, derived from the
  // graph rather than listed here.
  for (const host of policy.hosts) {
    const proofRef = `./${host.unavailable_producer_workflow}`;
    const entry = Object.entries(object(release.jobs))
      .find(([, jobValue]) => object(jobValue).uses === proofRef);
    add(
      violations,
      entry !== undefined
        && object(entry[1]).if
          === `needs.${gateJobId}.outputs.${hostFlag(host)} == 'true'`
        && needs(object(entry[1])).includes(gateJobId),
      `${releaseFile} proof calling ${host.unavailable_producer_workflow} must dispatch only on ${hostFlag(host)}`,
    );
  }

  // The non-claim producer feeds the receipt into the shared classifier and records the
  // pre-assignment reason only for hosts the classifier withheld under it.
  const nonClaim = requireJob(violations, releaseFile, release, "accelerator-non-claim");
  const nonClaimDownload = namedStep(nonClaim, "Download this run's reservation receipt");
  add(
    violations,
    nonClaimDownload?.uses === "actions/download-artifact@v8.0.1"
      && object(nonClaimDownload?.with).pattern === "protected-runner-reservation-attempt-*"
      && object(nonClaimDownload?.with)["merge-multiple"] === false,
    `${releaseFile} non-claim producer must download this run's reservation receipt`,
  );
  requireStepRun(violations, releaseFile, nonClaim, "Decide withheld accelerator hosts", [
    "protected-runner-reservation-attempt-",
    "--reservation",
  ]);
  requireStepRun(violations, releaseFile, nonClaim, "Record populated accelerator non-claims", [
    "runner_unproven_pre_assignment",
    "--reason accelerator_host_offline_pre_assignment",
  ]);

  // The closeouts re-confirm the pre-assignment route from a receipt they downloaded themselves:
  // the trust boundary that already refuses to inherit the lost-runner verdict refuses to
  // inherit this one the same way.
  for (const [jobId, stepName] of [
    ["pre-publish-closeout", "Authenticate pre-publish Actions provenance"],
    ["post-publish-closeout", "Authenticate post-publish Actions provenance"],
  ]) {
    const job = requireJob(violations, releaseFile, release, jobId);
    const download = namedStep(job, "Collect this run's reservation receipt");
    add(
      violations,
      download?.uses === "actions/download-artifact@v8.0.1"
        && object(download?.with).pattern === "protected-runner-reservation-attempt-*"
        && object(download?.with)["merge-multiple"] === false,
      `${releaseFile} ${jobId} must collect this run's reservation receipt itself`,
    );
    requireStepRun(violations, releaseFile, job, stepName, [
      "--reservation-receipt",
    ]);
  }
  return violations;
}

function validateReleaseArtifactRerunSafety(workflows, violations) {
  const releaseChainWorkflows = new Set([
    "source-proof.yml",
    "packaged-platform-proof.yml",
    "macos-metal-proof.yml",
    "windows-vulkan-proof.yml",
    "linux-vulkan-proof.yml",
    "post-publish-release-smoke.yml",
    "release.yml",
  ]);
  const replaceableStableIntermediates = new Map([
    ["packaged-platform-proof.yml/build/Upload release asset", {
      name: "codestory-cli-${{ matrix.asset_target }}",
      path: "target/release-dist/codestory-cli-v${{ inputs.version }}-${{ matrix.asset_target }}.${{ matrix.extension }}\ntarget/release-dist/codestory-cli-v${{ inputs.version }}-${{ matrix.asset_target }}.${{ matrix.extension }}.sha256\ntarget/release-dist/SHA256SUMS.txt\n",
    }],
    ["packaged-platform-proof.yml/build/Upload exact candidate archive record", {
      name: "codestory-candidate-archive-record-${{ matrix.asset_target }}",
      path: "target/candidate-archive-record/${{ matrix.asset_target }}/candidate-archive-record.json",
    }],
    ["packaged-platform-proof.yml/build/Upload separate qualification driver", {
      name: "codestory-qualification-driver-${{ matrix.asset_target }}",
      path: "target/release-dist/qualification-driver/${{ matrix.asset_target }}",
    }],
    ["macos-metal-proof.yml/packaged-metal/Upload Metal calibration runs", {
      name: "embedding-calibration-macos-${{ inputs.version }}",
      path: "target/calibration-runs/macos",
    }],
    ["release.yml/pre-publish-closeout/Upload accepted pre-publish closeout", {
      name: "release-closeout-pre-publish-${{ needs.preflight.outputs.version }}-${{ github.sha }}",
      path: "target/release-closeout/trusted-pre-publish-producers.json\ntarget/release-closeout/pre_publish\n",
    }],
  ]);
  const observedStableIntermediates = new Set();
  for (const [file, workflow] of workflows) {
    if (!releaseChainWorkflows.has(file)) continue;
    for (const [jobId, jobValue] of Object.entries(object(workflow.jobs))) {
      for (const step of list(jobValue?.steps)) {
        if (step?.uses !== "actions/upload-artifact@v7.0.1") continue;
        const upload = object(step.with);
        const artifactName = String(upload.name ?? "");
        const uploadKey = `${file}/${jobId}/${step.name ?? ""}`;
        const attemptQualified = artifactName.includes("${{ github.run_attempt }}")
          || (
            uploadKey === "source-proof.yml/resolve/Upload executable release freeze receipt"
            && artifactName === "${{ steps.receipt.outputs.artifact_name }}"
          );
        const expectedStable = replaceableStableIntermediates.get(uploadKey);
        const stableIntermediateMatches = expectedStable !== undefined
          && !observedStableIntermediates.has(uploadKey)
          && artifactName === expectedStable.name
          && String(upload.path ?? "") === expectedStable.path
          && upload.overwrite === true;
        if (expectedStable !== undefined) {
          observedStableIntermediates.add(uploadKey);
        }
        add(
          violations,
          expectedStable !== undefined
            ? stableIntermediateMatches
            : attemptQualified && upload.overwrite !== true,
          `${file} job ${jobId} upload ${step.name ?? artifactName} must be immutable and attempt-qualified unless it is an explicitly allowlisted stable intermediate`,
        );
      }
    }
  }
  for (const uploadKey of replaceableStableIntermediates.keys()) {
    add(
      violations,
      observedStableIntermediates.has(uploadKey),
      `${uploadKey} stable intermediate upload must exist exactly once with its policy-owned artifact name and path`,
    );
  }
}

// Cargo test-name filters are substring matches: a filter that names nothing selects zero tests and
// still exits 0, so a renamed test turns its proof lane green without running anything. `--exact`
// does not help — libtest also exits 0 when an exact filter matches nothing. These names are
// therefore checked statically against the crate sources they claim to select.
const cargoValueOptions = new Set([
  "-p",
  "--package",
  "--test",
  "--bench",
  "--example",
  "--bin",
  "--features",
  "--target",
  "--target-dir",
  "--manifest-path",
  "--profile",
  "--jobs",
  "-j",
]);

const expressionPlaceholder = "__CODESTORY_GITHUB_EXPRESSION__";
const harnessValueOptions = new Set(["--color", "--format", "--skip", "--test-threads"]);

function cargoTestFilterNames(line) {
  // GitHub expressions expand at run time; treat each as one opaque token so an option that takes a
  // value consumes it instead of leaving `matrix.foo` behind as a bare positional.
  const tokens = line
    .replace(/\$\{\{.*?\}\}/gu, expressionPlaceholder)
    .trim()
    .split(/\s+/u)
    .filter(token => token !== "\\");
  const start = tokens.findIndex(token => token === "test");
  if (start < 0) return { package: null, filters: [], exact: false };
  const separator = tokens.indexOf("--", start + 1);
  const cargoTokens = tokens.slice(start + 1, separator < 0 ? undefined : separator);
  const harnessTokens = separator < 0 ? [] : tokens.slice(separator + 1);

  let packageName = null;
  const filters = [];
  for (let index = 0; index < cargoTokens.length; index += 1) {
    const token = cargoTokens[index];
    if (cargoValueOptions.has(token)) {
      if (token === "-p" || token === "--package") packageName = cargoTokens[index + 1] ?? null;
      index += 1;
      continue;
    }
    if (token.startsWith("-")) {
      const [name, value] = token.split("=");
      if ((name === "-p" || name === "--package") && value) packageName = value;
      continue;
    }
    if (token.includes(expressionPlaceholder)) continue;
    // Cargo accepts at most one positional TESTNAME filter.
    filters.push(token);
    break;
  }
  for (let index = 0; index < harnessTokens.length; index += 1) {
    const token = harnessTokens[index];
    if (harnessValueOptions.has(token)) {
      index += 1;
      continue;
    }
    if (token.startsWith("-") || token.includes(expressionPlaceholder)) continue;
    filters.push(token);
  }
  return { package: packageName, filters, exact: harnessTokens.includes("--exact") };
}

function crateDirectories() {
  const crateRoot = path.join(repositoryRoot, "crates");
  const directories = new Map();
  if (!fs.existsSync(crateRoot)) return directories;
  for (const entry of fs.readdirSync(crateRoot, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const manifest = path.join(crateRoot, entry.name, "Cargo.toml");
    if (!fs.existsSync(manifest)) continue;
    const name = /^\s*name\s*=\s*"([^"]+)"/mu.exec(fs.readFileSync(manifest, "utf8"))?.[1];
    if (name) directories.set(name, path.join(crateRoot, entry.name));
  }
  return directories;
}

function rustSourceIdentifiers(directory) {
  const identifiers = new Set();
  const stack = [directory];
  while (stack.length > 0) {
    const current = stack.pop();
    let entries;
    try {
      entries = fs.readdirSync(current, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      const entryPath = path.join(current, entry.name);
      if (entry.isDirectory()) {
        if (entry.name !== "target") stack.push(entryPath);
        continue;
      }
      if (!entry.name.endsWith(".rs")) continue;
      const source = fs.readFileSync(entryPath, "utf8");
      for (const match of source.matchAll(/\b(?:fn|mod)\s+([A-Za-z_][A-Za-z0-9_]*)/gu)) {
        identifiers.add(match[1]);
      }
    }
  }
  return identifiers;
}

export function validateCargoTestFilters(
  workflows,
  violations,
  directories = crateDirectories(),
  readIdentifiers = rustSourceIdentifiers,
) {
  const identifierCache = new Map();
  const identifiersFor = packageName => {
    if (!identifierCache.has(packageName)) {
      const directory = directories.get(packageName);
      identifierCache.set(packageName, directory ? readIdentifiers(directory) : null);
    }
    return identifierCache.get(packageName);
  };

  for (const [file, workflow] of workflows) {
    for (const [jobName, rawJob] of Object.entries(object(workflow.jobs))) {
      for (const [stepIndex, step] of list(object(rawJob).steps).entries()) {
        if (typeof step?.run !== "string") continue;
        for (const { line, number } of executableCargoLines(step.run)) {
          if (!/^\s*(?:[A-Z_][A-Z0-9_]*=\S+\s+)*(?:sudo\s+)?cargo\s+test\b/u.test(line)) continue;
          const { package: packageName, filters, exact } = cargoTestFilterNames(line);
          if (filters.length === 0) continue;
          // A workspace-wide run resolves names across every crate, which this guard does not model.
          if (!packageName) continue;
          const identifiers = identifiersFor(packageName);
          const location = `${file} jobs.${jobName}.steps.${stepIndex}.run:${number}`;
          if (!identifiers) {
            violations.push(`${location} names unknown package ${packageName}`);
            continue;
          }
          for (const filter of filters) {
            for (const segment of filter.split("::").filter(Boolean)) {
              // Without `--exact` cargo matches substrings, so mirror that rather than demanding a
              // whole identifier: `publication_transitions_...` legitimately selects both the
              // `full_` and `incremental_` variants.
              const resolved = exact
                ? identifiers.has(segment)
                : [...identifiers].some(identifier => identifier.includes(segment));
              add(
                violations,
                resolved,
                `${location} cargo test filter "${filter}" selects no test: ${segment} matches no fn or mod in ${packageName}`,
              );
            }
          }
        }
      }
    }
  }
}

// The only secret read the plugin lane is allowed is the marketplace app identity, and only in the
// step that mints the scoped token. Return a copy of the workflow with exactly that read removed,
// so whatever still names the secrets context afterwards is a read nobody sanctioned. Both the key
// and the expression must match exactly: swapping either value for a different secret leaves the
// mention in place rather than inheriting the exemption.
const MARKETPLACE_IDENTITY_READS = new Map([
  ["app-id", "${{ secrets.MARKETPLACE_APP_ID }}"],
  ["private-key", "${{ secrets.MARKETPLACE_APP_PRIVATE_KEY }}"],
]);

function withoutMarketplaceIdentity(workflow) {
  const redacted = JSON.parse(JSON.stringify(workflow));
  const tokenStep = namedStep(
    object(object(redacted.jobs)["marketplace-publish"]),
    "Mint a scoped marketplace token",
  );
  const inputs = object(tokenStep?.with);
  for (const [key, expression] of MARKETPLACE_IDENTITY_READS) {
    if (inputs[key] === expression) delete inputs[key];
  }
  return redacted;
}

export function validatePluginRelease(workflows, violations, graph) {
  const file = "plugin-release.yml";
  const workflow = workflows.get(file);
  if (!workflow) {
    violations.push(`${file} must exist`);
    return;
  }
  const pluginChain = object(object(at(graph, "workflow_policy", "plugin_chain")).dependencies);
  const scalars = scalarStrings(workflow);
  add(violations, hasExactKeys(object(workflow.on), ["workflow_call"]), `${file} must be callable only`);
  // Nothing is built or signed on the plugin lane, so it declares no callable secret surface and
  // its caller forwards none. The one credential it may read is the marketplace app identity, and
  // only where the scoped token is minted.
  //
  // The rule stays a whole-workflow substring scan, no weaker than the blanket ban it replaces,
  // because "secrets." is not the only way to reach the context: `toJSON(secrets)`,
  // `secrets['NAME']`, a `secrets:` key, a secret smuggled through a bare array element, and
  // `SECRETS.NAME` (contexts are case-insensitive) all name it without that substring. Instead of
  // pattern-matching the smuggling shapes, redact the two permitted identity reads at their exact
  // position and require the remainder to mention secrets nowhere at all.
  add(
    violations,
    !/secrets/iu.test(JSON.stringify(withoutMarketplaceIdentity(workflow))),
    `${file} must not receive or forward secrets beyond the minted marketplace app identity: nothing is built or signed on the plugin lane`,
  );
  walk(workflow, (key, value) => {
    if (/^APPLE_/u.test(key) || (typeof value === "string" && /\bAPPLE_[A-Z0-9_]+\b/u.test(value))) {
      violations.push(`${file} must never reference Apple signing material`);
    }
  });
  const jobs = object(workflow.jobs);
  add(
    violations,
    hasExactKeys(jobs, ["workflow-policy", ...Object.keys(pluginChain)]),
    `${file} must keep exactly the plugin lane the release claim graph declares`,
  );
  for (const [name, dependencies] of Object.entries(pluginChain)) {
    add(
      violations,
      sameMembers(needs(object(jobs[name])), dependencies),
      `${file} ${name} dependencies must match the release claim graph`,
    );
  }
  for (const [name, job] of Object.entries(jobs)) {
    const permissions = object(job).permissions;
    add(
      violations,
      name === "publish" ? object(permissions).contents === "write" : permissions === undefined,
      `${file} only the publish job may hold write permission`,
    );
  }
  const preflight = object(jobs.preflight);
  requireStepRun(violations, file, preflight, "Validate release authority", [
    "auto-release.yml@refs/heads/main",
    "repos/$GITHUB_REPOSITORY/git/ref/heads/main",
  ]);
  requireStepRun(violations, file, preflight, "Validate plugin-lane version synchronization", [
    "check-codestory-release.py",
    "--lane plugin",
  ]);
  requireStepRun(violations, file, preflight, "Bind the pin to the published CLI release", [
    "cli-version.json",
    "SHA256SUMS.txt",
  ]);
  requireStepRun(violations, file, preflight, "Refuse a changed tool surface", [
    "generated-mcp-catalog.json",
  ]);
  requireStepRun(violations, file, object(jobs["plugin-proof"]), "Check the pinned provision proof", [
    "node --test scripts/tests/prove-plugin-pinned-provision.test.mjs",
  ]);
  // The lane is stated, never defaulted. The native lane's pin lawfully carries no archive
  // digests, so a plugin-lane gate that stopped naming its lane could silently become one that
  // proves nothing about the source pin it exists to enforce.
  requireStepRun(violations, file, object(jobs["plugin-proof"]), "Provision the pinned CLI end to end", [
    "scripts/prove-plugin-pinned-provision.mjs",
    "--lane plugin",
  ]);
  requireStepRun(violations, file, object(jobs.publish), "Re-verify main before tagging", [
    "repos/$GITHUB_REPOSITORY/git/ref/heads/main",
  ]);
  add(
    violations,
    !scalars.some((value) => /cargo\s+(?:build|test)/u.test(value)),
    `${file} must not build native code`,
  );

  // The catalog a host installs from is only correct once it names this release, so the plugin
  // lane owns the same publication step the native lane does.
  const marketplacePublish = object(jobs["marketplace-publish"]);
  add(
    violations,
    marketplacePublish.environment === "marketplace-publish",
    `${file} marketplace publication must hold its cross-repository credential in its own environment`,
  );
  const tokenStep = namedStep(marketplacePublish, "Mint a scoped marketplace token");
  add(
    violations,
    String(tokenStep?.uses ?? "").startsWith("actions/create-github-app-token@")
      && fullSha.test(String(tokenStep?.uses ?? "").split("@")[1] ?? "")
      && object(tokenStep?.with).owner === "TheGreenCedar"
      && object(tokenStep?.with).repositories === "AgentPluginMarketplace",
    `${file} marketplace token must be a SHA-pinned app token scoped to the marketplace repository`,
  );
  requireStepRun(violations, file, marketplacePublish, "Point the catalog at the published release", [
    "publish-marketplace-catalog.mjs",
    '--version "$INPUT_VERSION"',
  ]);
  requireStepEnv(violations, file, marketplacePublish, "Point the catalog at the published release", {
    INPUT_VERSION: "${{ inputs.version }}",
  });
  // Same contract as the native lane: the catalog push is delivery after an irreversible tag, so
  // it may not fail the release, and the run must record which state it ended in.
  const catalogDelivery = object(at(graph, "workflow_policy", "catalog_delivery"));
  violations.push(...catalogDeliveryOutcomeViolations(file, marketplacePublish, catalogDelivery));

  // Preflight runs before the release exists, so a revision captured there names the *previous*
  // release. Smoke must install from the revision this run published or it proves nothing.
  const smoke = object(jobs["post-publish-smoke"]);
  add(
    violations,
    object(preflight.outputs).marketplace_revision === undefined,
    `${file} preflight must not capture a marketplace revision that predates publication`,
  );
  const candidateCatalogProof = namedStep(
    preflight,
    "Prove the candidate-pinned marketplace install path",
  );
  add(
    violations,
    candidateCatalogProof?.["continue-on-error"] === undefined,
    `${file} W8.4 candidate-pinned marketplace proof must fail closed before publication`,
  );
  requireStepRun(
    violations,
    file,
    preflight,
    "Prove the candidate-pinned marketplace install path",
    [
      "build-marketplace-fixture.mjs",
      '--commit "$GITHUB_SHA"',
      'fixture_revision="$(git -C "$fixture_root" rev-parse HEAD)"',
      "install-codestory-marketplace-proof.mjs",
      '--marketplace-source "$fixture_root"',
      '--marketplace-revision "$fixture_revision"',
      "--local-fixture true",
    ],
  );
  const installStepName = "Prove the public marketplace install path";
  add(
    violations,
    object(namedStep(smoke, installStepName)?.env).MARKETPLACE_REVISION
      === "${{ steps.delivery.outputs.marketplace_revision }}",
    `${file} post-publish smoke must install from the marketplace revision this release published`,
  );
  violations.push(...catalogDeliveryStateViolations(
    file,
    smoke,
    catalogDelivery,
    {
      published: "${{ needs.marketplace-publish.outputs.catalog_published == 'true' }}",
      state: "${{ needs.marketplace-publish.outputs.catalog_delivery_state }}",
      revision: "${{ needs.marketplace-publish.outputs.marketplace_revision }}",
    },
    installStepName,
    "v${{ inputs.version }}",
  ));
  const smokeIf = String(smoke.if ?? "");
  add(
    violations,
    smokeIf.includes("always()")
      && smokeIf.includes("needs.preflight.result == 'success'")
      && smokeIf.includes("needs.publish.result == 'success'")
      // Any reference at all, not just `.result`: an `outputs.catalog_published == 'true'`
      // conjunct here is the same hard gate wearing a different name.
      && !smokeIf.includes("needs.marketplace-publish"),
    `${file} post-publish smoke must require a successful publish without gating on marketplace-publish in any form`,
  );

  const auto = workflows.get("auto-release.yml");
  const pluginCaller = object(at(auto, "jobs", "plugin-release"));
  add(
    violations,
    pluginCaller.uses === "./.github/workflows/plugin-release.yml"
      && String(pluginCaller.if ?? "").includes("release_lane == 'plugin'")
      && pluginCaller.secrets === undefined,
    "auto-release.yml must route the plugin lane without forwarding secrets",
  );
  add(
    violations,
    String(object(at(auto, "jobs", "release")).if ?? "").includes("release_lane == 'native'"),
    "auto-release.yml native release must be gated on the native lane",
  );
}

export function validateMarketplaceSync(workflows, violations, graph) {
  const file = "marketplace-sync.yml";
  const workflow = workflows.get(file);
  if (!workflow) {
    violations.push(`${file} must exist`);
    return;
  }
  // Pinning the dispatch input names while leaving the trigger set open closes one door and
  // leaves another: `workflow_call` carries its own inputs, which `on.workflow_dispatch.inputs`
  // says nothing about, and a caller-supplied value would reach the same steps.
  add(
    violations,
    hasExactKeys(object(workflow.on), ["workflow_dispatch"]),
    `${file} must be reachable only by manual dispatch`,
  );
  add(
    violations,
    hasExactKeys(at(workflow, "on", "workflow_dispatch", "inputs"), ["version", "commit"]),
    `${file} must dispatch on exactly a version and a commit`,
  );
  const job = requireJob(violations, file, workflow, "sync");
  const bindings = {
    INPUT_COMMIT: "${{ inputs.commit }}",
    INPUT_VERSION: "${{ inputs.version }}",
  };
  const checkout = "Checkout the published commit";
  // GitHub serves the same dispatched value under a second name, `github.event.inputs.commit`, and
  // the guard validates only what arrives as `inputs.commit`. The checkout already refuses the
  // other spelling for its own `ref`; this refuses it everywhere in the file, including the job
  // level, where a step's own binding check cannot see it.
  add(
    violations,
    scalarStrings(workflow)
      .flatMap(text => interpolatedDispatchInputs(text))
      .every(expression => Object.values(bindings).includes(expression)),
    `${file} must name a dispatch input only as ${bindings.INPUT_COMMIT} or ${bindings.INPUT_VERSION}`,
  );
  // The ban is a property of the file, not of one job. A second job added beside `sync` runs on a
  // runner with the same repository token and the same marketplace environment, so a scan scoped
  // to `jobs.sync` would exempt exactly the code an attacker would add.
  for (const [jobName, rawJob] of Object.entries(object(workflow.jobs))) {
    // `continue-on-error` is the same class of blind spot as `shell:`: it lives outside the script,
    // so nothing the guard's own text asserts can see it, and it converts the guard's `exit 1` into
    // advice. A job carrying it downgrades every step it contains at once.
    add(
      violations,
      object(rawJob)["continue-on-error"] === undefined,
      `${file} jobs.${jobName} must not declare continue-on-error, which would make its guards advisory`,
    );
    for (const [index, rawStep] of list(object(rawJob).steps).entries()) {
      const step = object(rawStep);
      const where = `${file} jobs.${jobName}.steps.${index}`;
      add(
        violations,
        step["continue-on-error"] === undefined,
        `${where} must not declare continue-on-error, which would make its refusal advisory`,
      );
      if (typeof step.run === "string") {
        // Interpolation is textual and quoting does not stop command substitution, so a dispatched
        // value spliced into script text executes on the runner -- here beside repository tokens.
        add(
          violations,
          !step.run.includes("${{"),
          `${where} must read dispatch inputs from env, not interpolated script text`,
        );
        // A `run:` body is executed by the shell the step declares, so the script and its
        // interpreter are one artifact. The guard's whole-value test is `[[ =~ ]]`, which POSIX
        // shells do not have: under `shell: sh` the condition is a missing command, `set -e` does
        // not fire inside an `if`, the refusal branch never runs, and the guard exits 0 on the very
        // value it exists to reject. Nothing in the script's own text can see that, so the shell is
        // pinned here.
        add(
          violations,
          step.shell === "bash",
          `${where} must declare shell: bash so its script runs under the shell it was reviewed under`,
        );
      }
      // `env:` is the sanctioned channel into a step. Every other scalar is an action input or
      // script text, and an action can evaluate what it is handed -- `actions/github-script` runs
      // its `script:` input. The checkout `ref` is the single exception: it is not an executable
      // surface and is separately pinned below to the value the guard validated. That exemption is
      // scoped to `sync`, the only job the guard runs in; a like-named step elsewhere is not covered
      // by it and so is not exempt either.
      const surfaces = { ...step };
      delete surfaces.env;
      if (jobName === "sync" && step.name === checkout) {
        surfaces.with = { ...object(step.with) };
        delete surfaces.with.ref;
      }
      add(
        violations,
        !scalarStrings(surfaces).some(text => interpolatedDispatchInputs(text).length > 0),
        `${where} must not splice a dispatch input into an action input`,
      );
      for (const [name, expected] of Object.entries(bindings)) {
        // `$NAME` and `${NAME}` are the same read; gating on the bare form alone let a step consume
        // `${INPUT_COMMIT}` with no binding at all. Checking the declaration too closes the other
        // direction: a binding of the unvalidated `github.event.inputs` spelling is a violation
        // whether or not this step is the one that reads it.
        const consumed = typeof step.run === "string"
          && new RegExp(`\\$\\{?${name}\\b`, "u").test(step.run);
        const declared = Object.hasOwn(object(step.env), name);
        if (!consumed && !declared) continue;
        add(
          violations,
          object(step.env)[name] === expected,
          `${where} must bind ${name} to ${expected}`,
        );
      }
    }
  }
  // Shape is proven before the checkout resolves the ref and before any marketplace token exists.
  const guard = "Validate the dispatched release coordinates";
  // The guard's step fragments - the anchored shapes with their consuming tests, and
  // the grep ban that keeps the match whole-value - are rule instances and live in
  // release-claims.json under workflow_policy.step_fragments; stepFragmentViolations
  // evaluates them.
  add(
    violations,
    stepIndex(job, guard) === 0,
    `${file} must validate the dispatched coordinates before any other step`,
  );
  // Ordering only buys something if the guard covers what the next step consumes. Without this the
  // checkout could resolve `github.ref` and the validated commit would gate nothing.
  add(
    violations,
    object(object(namedStep(job, checkout)).with).ref === bindings.INPUT_COMMIT,
    `${file} ${checkout} must resolve the validated ${bindings.INPUT_COMMIT}`,
  );
  add(
    violations,
    stepIndex(job, checkout) > stepIndex(job, guard),
    `${file} must validate the dispatched commit before checking it out`,
  );
  // A catalog moved by this workflow is a RECOVERY, not the publication that shipped the plugin.
  // The two stay distinguishable only if this lane records its own identity, and the rollback it
  // performs is only undoable if the pin it replaced is recorded with it.
  const recovery = object(object(graph?.workflow_policy?.catalog_delivery).recovery);
  const recoveryStep = "Record catalog recovery identity";
  // The catalog push's step fragments are rule instances and live in
  // release-claims.json under workflow_policy.step_fragments;
  // stepFragmentViolations evaluates them.
  add(
    violations,
    namedStep(job, "Point the catalog at the published release")?.id === "publish",
    `${file} catalog push must publish its recorded pin under the id the identity step reads`,
  );
  add(
    violations,
    object(namedStep(job, recoveryStep)?.env).CATALOG_DELIVERY === recovery.id,
    `${file} must record the ${recovery.id} delivery identity the release claim graph declares`,
  );
  for (const name of list(recovery.previous_pin_outputs)) {
    add(
      violations,
      scalarStrings(object(namedStep(job, recoveryStep)?.env)).includes(
        `\${{ steps.publish.outputs.${name} }}`,
      ),
      `${file} ${recoveryStep} must carry the recorded ${name}`,
    );
  }
  // The recovery identity's step fragments are rule instances and live in
  // release-claims.json under workflow_policy.step_fragments;
  // stepFragmentViolations evaluates them.
  add(
    violations,
    stepIndex(job, recoveryStep) > stepIndex(job, "Point the catalog at the published release"),
    `${file} must record the recovery identity from the catalog move it actually performed`,
  );
  for (const state of list(object(graph?.workflow_policy?.catalog_delivery).states)) {
    add(
      violations,
      object(state).id !== recovery.id,
      `${file} recovery identity must not reuse the ${object(state).id} delivery state`,
    );
  }
}

export function validateWorkflows(workflows, graph = loadReleaseClaimGraph(repositoryRoot)) {
  const violations = [];
  violations.push(...proofFloorPolicyViolations(workflows, graph));
  violations.push(...benchmarkDependencyIsolationViolations(
    fs.readFileSync(
      path.join(repositoryRoot, "crates", "codestory-bench", "Cargo.toml"),
      "utf8",
    ),
  ));
  violations.push(...retrievalGeneralizationSuitePolicyViolations(
    fs.readFileSync(
      path.join(repositoryRoot, retrievalGeneralizationSuiteFile),
      "utf8",
    ),
    {
      legacyWrapperPresent:
        fs.existsSync(path.join(repositoryRoot, legacyRetrievalGeneralizationWrapper))
        || serializedRustRetrievalWrapperPresent(),
    },
  ));
  violations.push(...qualificationDriverArtifactViolations(
    fs.readFileSync(
      path.join(
        repositoryRoot,
        ".github",
        "scripts",
        "qualification-driver-artifact.mjs",
      ),
      "utf8",
    ),
    graph,
  ));
  for (const [file, workflow] of workflows) {
    violations.push(...basicWorkflowViolations(file, workflow));
  }
  violations.push(...reusableWorkflowPermissionViolations(workflows));
  validateCargoTestFilters(workflows, violations);
  validatePluginRelease(workflows, violations, graph);
  validateMarketplaceSync(workflows, violations, graph);
  // The locked-setup constant mirrors are rule instances and live in
  // release-claims.json under workflow_policy.constant_mirrors;
  // constantMirrorViolations evaluates them.
  validateIssueWorkflows(workflows, violations);
  validatePluginAndDraftWorkflows(workflows, violations, graph);
  validateReleaseCoordinator(workflows, violations, graph);
  validatePackagedProof(workflows, violations, graph);
  validatePostPublish(workflows, violations, graph);
  validatePackagedCoordinator(workflows, violations, graph);
  validateRemainingWorkflows(workflows, violations);
  violations.push(...releaseProofCpuSelectorViolations(workflows, graph));
  validateReleaseCellUploadOwnership(workflows, violations);
  validateReleaseArtifactRerunSafety(workflows, violations);
  violations.push(...dispatchInputInterpolationViolations(workflows));
  violations.push(...shellDependentBindingViolations(workflows));
  violations.push(...absorbedFailureViolations(workflows));
  violations.push(...annotationScopeViolations(workflows));
  violations.push(...lostRunnerRecoveryViolations(workflows, graph));
  violations.push(...protectedRunnerReservationViolations(workflows, graph));
  violations.push(...releaseWorkflowContractViolations(workflows, graph));
  violations.push(...pinnedProgramViolations(workflows, graph));
  violations.push(...stepFragmentViolations(workflows, graph));
  violations.push(...structuralPinViolations(workflows, graph));
  violations.push(...constantMirrorViolations(graph));
  return violations;
}

function main() {
  let workflows;
  try {
    workflows = loadWorkflows();
  } catch (error) {
    console.error(`Workflow YAML parse failed: ${error.message}`);
    process.exit(1);
  }
  const violations = validateWorkflows(workflows);
  if (violations.length > 0) {
    console.error(violations.join("\n"));
    process.exit(1);
  }
  console.log("Workflow policy passed: parsed workflow structure satisfies repository contracts.");
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main();
}
