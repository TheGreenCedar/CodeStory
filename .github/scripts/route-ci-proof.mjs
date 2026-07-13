#!/usr/bin/env node
import fs from "node:fs";
import { pathToFileURL } from "node:url";

const scopeRank = new Map([
  ["none", 0],
  ["macos", 1],
  ["full", 2],
]);

const macosSurfaces = [
  /^\.github\/workflows\/macos-/u,
  /^backend\/.*(?:darwin|macos|metal)/iu,
  /^scripts\/setup-retrieval-env\./u,
  /^scripts\/codex-worktree-setup\.(?:mjs|sh)$/u,
  /^scripts\/tests\/codex-worktree-setup\.test\.mjs$/u,
  /^plugins\/codestory\/.*(?:darwin|macos|metal)/iu,
];

const crossPlatformRuntimeSurfaces = [
  /^(?:Cargo\.toml|Cargo\.lock)$/u,
  /^crates\/[^/]+\/Cargo\.toml$/u,
  /^crates\/codestory-cli\/src\/readiness_broker\//u,
  /^crates\/codestory-retrieval\/src\/(?:config|index|inventory|lib|query|sidecar)\.rs$/u,
  /^scripts\/install-codestory\.ps1$/u,
];

const proofNeutralSurfaces = [
  /^CHANGELOG\.md$/u,
  /^docs\//u,
  /^\.github\/workflows\/retrieval-sidecar-smoke\.yml$/u,
  /^crates\/codestory-runtime\/tests\/retrieval_generalization_guard\.rs$/u,
  /^scripts\/(?:codestory-agent-ab-benchmark|codestory-evidence-provenance|codestory-release-evidence-gate|lint-retrieval-generalization)\.mjs$/u,
];

function cleanPaths(paths) {
  return [...new Set(paths.map(value => value.trim().replaceAll("\\", "/")).filter(Boolean))];
}

export function classifyProofScope(paths) {
  const changed = cleanPaths(paths);
  if (changed.length === 0) return "none";
  if (changed.some(file => crossPlatformRuntimeSurfaces.some(pattern => pattern.test(file)))) {
    return "full";
  }
  if (changed.some(file => macosSurfaces.some(pattern => pattern.test(file)))) {
    return "macos";
  }
  if (changed.every(file => proofNeutralSurfaces.some(pattern => pattern.test(file)))) {
    return "none";
  }
  return "full";
}

export function selectProofScope(paths, requested = "auto") {
  const inferred = classifyProofScope(paths);
  if (requested === "auto" || requested === "") return inferred;
  if (!scopeRank.has(requested)) {
    throw new Error(`unsupported proof scope: ${requested}`);
  }
  return scopeRank.get(requested) > scopeRank.get(inferred) ? requested : inferred;
}

function selfTest() {
  const fixtures = [
    {
      name: "script and guard tests do not package",
      expected: "none",
      paths: [
        ".github/workflows/retrieval-sidecar-smoke.yml",
        "crates/codestory-runtime/tests/retrieval_generalization_guard.rs",
        "scripts/lint-retrieval-generalization.mjs",
        "docs/testing/retrieval-architecture.md",
        "CHANGELOG.md",
      ],
    },
    {
      name: "Mac lifecycle changes stay on Mac",
      expected: "macos",
      paths: [
        ".github/scripts/check-packaged-agent-proof.py",
        ".github/workflows/macos-metal-proof.yml",
        "crates/codestory-cli/src/ready_repair_status.rs",
        "crates/codestory-cli/src/stdio_transport.rs",
        "crates/codestory-retrieval/src/embeddings.rs",
      ],
    },
    {
      name: "runtime identity changes use every platform",
      expected: "full",
      paths: [
        "Cargo.lock",
        "crates/codestory-cli/src/readiness_broker/native_lease.rs",
        "crates/codestory-retrieval/src/inventory.rs",
        "crates/codestory-retrieval/src/sidecar.rs",
      ],
    },
  ];
  for (const fixture of fixtures) {
    const actual = classifyProofScope(fixture.paths);
    if (actual !== fixture.expected) {
      throw new Error(`${fixture.name}: expected ${fixture.expected}, got ${actual}`);
    }
  }
  if (selectProofScope(fixtures[0].paths, "macos") !== "macos") {
    throw new Error("explicit promotion must be able to widen an inferred scope");
  }
  if (selectProofScope(fixtures[2].paths, "macos") !== "full") {
    throw new Error("explicit promotion must not narrow an inferred scope");
  }
}

function main(argv) {
  const args = [...argv];
  if (args.includes("--self-test")) {
    selfTest();
    console.log("CI proof routing fixtures passed.");
    return;
  }
  const requestedIndex = args.indexOf("--requested");
  const requested = requestedIndex >= 0 ? args.splice(requestedIndex, 2)[1] : "auto";
  const stdin = args.includes("--stdin") ? fs.readFileSync(0, "utf8").split(/\r?\n/u) : [];
  const paths = args.filter(arg => arg !== "--stdin");
  console.log(selectProofScope([...stdin, ...paths], requested));
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
