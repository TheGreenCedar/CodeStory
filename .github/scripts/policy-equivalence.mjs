#!/usr/bin/env node
// Differential oracle for the S1 policy-graph migration (#1670): relocating a rule
// between checker code and release-claims.json must not change a single verdict.
// The merge-base checker (and its policy inputs) is materialized from git and run
// beside the working-tree checker over identical workflow structures: once over the
// pristine tree, then over a deterministic seeded hostile corpus that deletes every
// node and perturbs every scalar of every workflow. The two violation multisets must
// be verbatim-identical for every input; the corpus is bounded so a full run stays
// under roughly twenty minutes.
//
// Each side reads its own non-workflow inputs from its own tree (the merge-base side
// from the materialized base tree, the working-tree side from the repository), so a
// digest moved from code into the graph is exercised against the data that shipped
// with that side's checker.
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

export const DEFAULT_BASE_BRANCH = "origin/dev/codestory-next";
// Bounds the corpus: full enumeration today is ~8k mutants at ~120ms per mutant.
// When the enumerated corpus outgrows the bound, a seeded deterministic sample keeps
// the run inside the budget instead of silently truncating coverage from one end.
export const DEFAULT_MAX_MUTANTS = 9000;
const CORPUS_SEED = 0xc0de57a1;
// The base tree pathspecs cover every file the checker reads at run time: workflow
// sources, its own module closure, the release-claim graph, the crate tree walked by
// the cargo-test-filter and retrieval-wrapper lints, and the locked setup surfaces.
const BASE_TREE_PATHS = [
  ".cargo",
  ".github",
  "crates",
  "plugins",
  "scripts",
  "release-claims.json",
];

function git(args, options = {}) {
  const result = spawnSync("git", args, {
    cwd: repositoryRoot,
    encoding: "utf8",
    maxBuffer: 1 << 28,
    ...options,
  });
  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} failed: ${String(result.stderr).trim()}`);
  }
  return result;
}

export function resolveBaseSha(baseRef = process.env.POLICY_EQUIVALENCE_BASE_REF) {
  if (baseRef) {
    return git(["rev-parse", "--verify", `${baseRef}^{commit}`]).stdout.trim();
  }
  return git(["merge-base", "HEAD", DEFAULT_BASE_BRANCH]).stdout.trim();
}

// The base tree lives under node_modules/.cache so Node resolves the `yaml`
// dependency through the repository's installed node_modules and git never sees the
// materialized files. The tree is content-addressed by commit and reused when it is
// already complete.
export function materializeBaseTree(baseSha) {
  const root = path.join(
    repositoryRoot,
    "node_modules",
    ".cache",
    "codestory-policy-equivalence",
    baseSha,
  );
  const marker = path.join(root, ".complete");
  if (fs.existsSync(marker)) return root;
  fs.rmSync(root, { recursive: true, force: true });
  fs.mkdirSync(root, { recursive: true });
  const archive = spawnSync("git", ["archive", baseSha, "--", ...BASE_TREE_PATHS], {
    cwd: repositoryRoot,
    maxBuffer: 1 << 30,
  });
  if (archive.status !== 0) {
    throw new Error(`git archive ${baseSha} failed: ${String(archive.stderr).trim()}`);
  }
  const extract = spawnSync("tar", ["-x", "-C", root], {
    input: archive.stdout,
    maxBuffer: 1 << 30,
  });
  if (extract.status !== 0) {
    throw new Error(`extracting base tree ${baseSha} failed: ${String(extract.stderr).trim()}`);
  }
  fs.writeFileSync(marker, `${baseSha}\n`);
  return root;
}

function mulberry32(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// Every node of every parsed workflow yields a deletion mutant; every scalar leaf
// additionally yields a perturbation mutant. Enumeration order is the parse order of
// the sorted workflow files, so the corpus is deterministic for a given tree.
export function enumerateMutations(workflows) {
  const mutations = [];
  for (const [file, workflow] of workflows) {
    const walk = (value, trail) => {
      const entries = Array.isArray(value)
        ? value.map((item, index) => [index, item])
        : Object.entries(value);
      for (const [key, child] of entries) {
        const childTrail = [...trail, key];
        mutations.push({ file, path: childTrail, op: "delete" });
        if (child !== null && typeof child === "object") {
          walk(child, childTrail);
        } else {
          mutations.push({ file, path: childTrail, op: "perturb" });
        }
      }
    };
    if (workflow !== null && typeof workflow === "object") walk(workflow, []);
  }
  return mutations;
}

function perturbScalar(value) {
  if (typeof value === "string") return `${value} hostile-mutant`;
  if (typeof value === "number") return value + 1;
  if (typeof value === "boolean") return !value;
  return "hostile-mutant";
}

export function applyMutation(workflow, mutation) {
  const mutated = structuredClone(workflow);
  let parent = mutated;
  for (const key of mutation.path.slice(0, -1)) parent = parent[key];
  const leaf = mutation.path[mutation.path.length - 1];
  if (mutation.op === "delete") {
    if (Array.isArray(parent)) parent.splice(Number(leaf), 1);
    else delete parent[leaf];
  } else {
    parent[leaf] = perturbScalar(parent[leaf]);
  }
  return mutated;
}

function describeMutation(mutation) {
  return `${mutation.file} ${mutation.op} ${mutation.path.join(".")}`;
}

// A verdict is the sorted violation multiset, or the thrown error when a hostile
// structure escapes a validator; both sides must agree on whichever occurs. Each
// side runs with its own working directory because the locked-setup lint reads its
// contract files relative to the process cwd.
function verdict(validate, workflows, graph, cwd) {
  const previous = process.cwd();
  process.chdir(cwd);
  try {
    return { violations: validate(workflows, graph).map(String).sort() };
  } catch (error) {
    return { thrown: String((error && error.message) || error) };
  } finally {
    process.chdir(previous);
  }
}

export async function runEquivalence({
  baseRef,
  maxMutants = Number(process.env.POLICY_EQUIVALENCE_MAX_MUTANTS ?? DEFAULT_MAX_MUTANTS),
  log = () => {},
} = {}) {
  const started = Date.now();
  const baseSha = resolveBaseSha(baseRef);
  const baseRoot = materializeBaseTree(baseSha);
  const importFrom = (root, file) => import(pathToFileURL(path.join(root, file)).href);
  const current = await importFrom(repositoryRoot, ".github/scripts/check-workflow-policy.mjs");
  const currentClaims = await importFrom(repositoryRoot, "scripts/codestory-release-claims.mjs");
  const base = await importFrom(baseRoot, ".github/scripts/check-workflow-policy.mjs");
  const baseClaims = await importFrom(baseRoot, "scripts/codestory-release-claims.mjs");
  const currentGraph = currentClaims.loadReleaseClaimGraph(repositoryRoot);
  const baseGraph = baseClaims.loadReleaseClaimGraph(baseRoot);
  const workflows = current.loadWorkflows(path.join(repositoryRoot, ".github", "workflows"));

  const mismatches = [];
  const compare = (label, mutatedWorkflows) => {
    const baseVerdict = verdict(base.validateWorkflows, mutatedWorkflows, baseGraph, baseRoot);
    const currentVerdict = verdict(
      current.validateWorkflows,
      mutatedWorkflows,
      currentGraph,
      repositoryRoot,
    );
    const equal = JSON.stringify(baseVerdict) === JSON.stringify(currentVerdict);
    if (!equal && mismatches.length < 25) {
      mismatches.push({ input: label, base: baseVerdict, current: currentVerdict });
    }
    return { equal, baseVerdict, currentVerdict };
  };

  const pristine = compare("pristine tree", workflows);
  const enumerated = enumerateMutations(workflows);
  let corpus = enumerated;
  if (corpus.length > maxMutants) {
    const random = mulberry32(CORPUS_SEED);
    corpus = [...corpus]
      .map(mutation => ({ mutation, rank: random() }))
      .sort((left, right) => left.rank - right.rank || 0)
      .slice(0, maxMutants)
      .map(({ mutation }) => mutation);
  }
  for (const [index, mutation] of corpus.entries()) {
    const mutatedWorkflows = new Map(workflows);
    mutatedWorkflows.set(mutation.file, applyMutation(workflows.get(mutation.file), mutation));
    compare(describeMutation(mutation), mutatedWorkflows);
    if ((index + 1) % 1000 === 0) {
      log(`policy-equivalence: ${index + 1}/${corpus.length} mutants compared, ${mismatches.length} mismatches`);
    }
  }
  return {
    baseSha,
    enumeratedMutations: enumerated.length,
    corpusSize: corpus.length,
    pristine: {
      equal: pristine.equal,
      base: pristine.baseVerdict,
      current: pristine.currentVerdict,
    },
    mismatches,
    durationMs: Date.now() - started,
  };
}

function main() {
  const args = process.argv.slice(2);
  const readFlag = name => {
    const index = args.indexOf(name);
    return index >= 0 ? args[index + 1] : undefined;
  };
  const maxFlag = readFlag("--max");
  runEquivalence({
    baseRef: readFlag("--base"),
    ...(maxFlag === undefined ? {} : { maxMutants: Number(maxFlag) }),
    log: message => console.log(message),
  }).then(result => {
    console.log(
      `policy-equivalence: base ${result.baseSha}, pristine tree ${result.pristine.equal ? "identical" : "MISMATCHED"}, `
      + `${result.corpusSize} hostile mutants (${result.enumeratedMutations} enumerated), `
      + `${result.mismatches.length} mismatches, ${Math.round(result.durationMs / 1000)}s`,
    );
    for (const mismatch of result.mismatches) {
      console.error(JSON.stringify(mismatch, null, 2));
    }
    if (!result.pristine.equal || result.mismatches.length > 0) process.exitCode = 1;
  }).catch(error => {
    console.error(error);
    process.exitCode = 1;
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main();
}
