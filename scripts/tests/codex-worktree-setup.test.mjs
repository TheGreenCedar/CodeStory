import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  agentRepairArguments,
  cliCandidates,
  doctorSummaryLines,
  findCli,
  parseArguments,
  proofTarget,
  remoteHeadName,
  resolveSccache,
  runSetup,
} from "../codex-worktree-setup.mjs";

const scriptsDirectory = dirname(dirname(fileURLToPath(import.meta.url)));
const temporaryRoots = [];

function temporaryRoot() {
  const root = mkdtempSync(join(tmpdir(), "codestory-worktree-setup-test-"));
  temporaryRoots.push(root);
  return root;
}

function writeExecutable(path, contents = "") {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, contents);
  chmodSync(path, 0o755);
  return realpathSync(path);
}

function createProject(version = "0.15.0") {
  const root = temporaryRoot();
  mkdirSync(join(root, "crates", "codestory-cli"), { recursive: true });
  writeFileSync(
    join(root, "crates", "codestory-cli", "Cargo.toml"),
    `[package]\nname = "codestory-cli"\nversion = "${version}"\n`,
  );
  return root;
}

function result(status = 0, stdout = "", stderr = "") {
  return { status, stdout, stderr, signal: null };
}

test.after(() => {
  for (const root of temporaryRoots) rmSync(root, { recursive: true, force: true });
});

test("portable and PowerShell argument spellings share one parser", () => {
  assert.deepEqual(
    parseArguments([
      "-Project",
      "C:\\repo",
      "--pr-head-ref",
      "origin/topic",
      "-BranchHeadProof",
      "--resolve-cli-only",
    ], {}),
    {
      project: "C:\\repo",
      intendedBaseRef: "origin/dev/codestory-next",
      prHeadRef: "origin/topic",
      branchHeadProof: true,
      resolveCliOnly: true,
      selfTest: false,
      help: false,
    },
  );
  assert.equal(parseArguments([], { CODESTORY_BRANCH_HEAD_PROOF: "yes" }).branchHeadProof, true);
});

test("only a hexadecimal 40-character ref is detached", () => {
  assert.equal(remoteHeadName("a".repeat(40)), null);
  assert.equal(remoteHeadName("A1".repeat(20)), null);
  assert.equal(remoteHeadName("g".repeat(40)), `refs/heads/${"g".repeat(40)}`);
  assert.equal(remoteHeadName("origin/dev/codestory-next"), "refs/heads/dev/codestory-next");
  assert.equal(remoteHeadName("refs/heads/topic"), "refs/heads/topic");
  assert.equal(remoteHeadName("refs/tags/v1"), null);
});

test("proof target keeps base-plus-head as the default", () => {
  assert.equal(
    proofTarget("origin/dev/codestory-next", "origin/topic", false),
    "base:origin/dev/codestory-next + pr-head:origin/topic",
  );
  assert.equal(
    proofTarget("origin/dev/codestory-next", "origin/topic", true),
    "branch-head:origin/topic",
  );
});

test("CLI candidates retain explicit, install, versioned, worktree order", () => {
  const project = createProject();
  const home = temporaryRoot();
  const explicit = join(home, "explicit-cli");
  const candidates = cliCandidates(project, "0.15.0", {
    env: { CODESTORY_CLI: explicit, CODESTORY_HOME: home, PATH: "" },
    spawnSync: () => result(1),
  });
  const binary = process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli";
  assert.equal(candidates[0], explicit);
  assert.equal(candidates.indexOf(join(home, "bin", binary)), 1);
  assert.ok(
    candidates.indexOf(join(home, "bin", "releases", "0.15.0", binary))
      > candidates.indexOf(join(home, "bin", binary)),
  );
  assert.ok(
    candidates.indexOf(join(project, "target", "release", binary))
      > candidates.indexOf(join(home, "bin", "releases", "0.15.0", binary)),
  );
});

test("CLI resolution rejects a stale explicit binary and accepts the versioned install", () => {
  const project = createProject();
  const home = temporaryRoot();
  const binary = process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli";
  const stale = writeExecutable(join(home, "stale", binary));
  const current = writeExecutable(join(home, "bin", "releases", "0.15.0", binary));
  const versions = new Map([
    [stale, "0.14.3"],
    [current, "0.15.0"],
  ]);
  const selected = findCli(project, "0.15.0", {
    env: { CODESTORY_CLI: stale, CODESTORY_HOME: home, PATH: "" },
    spawnSync(command, args) {
      if (command === "git") return result(1);
      const version = versions.get(realpathSync(command));
      return args[0] === "--version" && version
        ? result(0, `codestory-cli ${version}\n`)
        : result(1);
    },
  });
  assert.equal(selected, current);
});

test("sccache prefers the user Cargo installation", () => {
  const home = temporaryRoot();
  const name = process.platform === "win32" ? "sccache.exe" : "sccache";
  const expected = writeExecutable(join(home, ".cargo", "bin", name));
  assert.equal(resolveSccache({ env: { HOME: home, USERPROFILE: home, PATH: "" } }), expected);
});

test("shared orchestration rehydrates, indexes, repairs, and reports status in order", () => {
  const project = createProject();
  const source = temporaryRoot();
  writeFileSync(join(source, "Cargo.toml"), "[workspace]\n");
  const cli = writeExecutable(join(temporaryRoot(), "codestory-cli"));
  const calls = [];
  const logs = [];
  const doctor = {
    retrieval_mode: "full",
    degraded_reason: null,
    readiness: [
      { goal: "local_navigation", status: "ready", summary: "Local navigation ready." },
      { goal: "agent_packet_search", status: "ready", summary: "Agent packet/search ready." },
    ],
    next_commands: [],
  };
  runSetup(parseArguments(["--project", project]), {
    env: {
      CODESTORY_CLI: cli,
      CODESTORY_REHYDRATE_FROM: source,
      HOME: temporaryRoot(),
      PATH: "",
    },
    log: line => logs.push(line),
    warn: line => logs.push(`warning: ${line}`),
    spawnSync(command, args) {
      if (command === cli && args[0] === "--version") return result(0, "codestory-cli 0.15.0\n");
      if (command === cli) {
        calls.push(args);
        return args[0] === "doctor" ? result(0, JSON.stringify(doctor)) : result();
      }
      return result(1);
    },
  });
  assert.deepEqual(calls.map(args => args[0]), ["cache", "index", "ready", "doctor"]);
  assert.deepEqual(calls[0], [
    "cache",
    "rehydrate",
    "--from-project",
    realpathSync(source),
    "--project",
    realpathSync(project),
  ]);
  assert.ok(logs.includes("  agent_packet_search: ready"));
  assert.ok(logs.includes("  retrieval_mode: full"));
});

test("Cargo fallback is locked and still feeds the shared setup path", () => {
  const project = createProject();
  const home = temporaryRoot();
  const stale = writeExecutable(join(home, "stale-codestory-cli"));
  const binaryName = process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli";
  const built = join(project, "target", "release", binaryName);
  let cargoArguments;
  runSetup(parseArguments(["--project", project]), {
    env: { CODESTORY_CLI: stale, HOME: home, PATH: "" },
    log: () => {},
    warn: () => {},
    installRelease() {
      throw new Error("release unavailable");
    },
    spawnSync(command, args) {
      if (command === "git") return result(1);
      if (command === stale && args[0] === "--version") {
        return result(0, "codestory-cli 0.14.3\n");
      }
      if (command === "cargo") {
        cargoArguments = args;
        writeExecutable(built);
        return result();
      }
      if (command === realpathSync(built) && args[0] === "--version") {
        return result(0, "codestory-cli 0.15.0\n");
      }
      if (command === realpathSync(built) && args[0] === "doctor") {
        return result(0, JSON.stringify({ readiness: [], next_commands: [] }));
      }
      if (command === realpathSync(built)) return result();
      return result(1);
    },
  });
  assert.deepEqual(cargoArguments, ["build", "--release", "--locked", "-p", "codestory-cli"]);
});

test("readiness helpers preserve the agent repair and degraded handoff contracts", () => {
  assert.deepEqual(agentRepairArguments("/repo"), [
    "ready",
    "--goal",
    "agent",
    "--repair",
    "--project",
    "/repo",
    "--format",
    "json",
    "--run-id",
    "shared-agent",
  ]);
  const summary = doctorSummaryLines({
    retrieval_mode: "lexical",
    degraded_reason: "embedding unavailable",
    readiness: [
      { goal: "local_navigation", status: "ready" },
      {
        goal: "agent_packet_search",
        status: "repair_retrieval",
        minimum_next: ["codestory-cli ready --goal agent --repair"],
      },
    ],
  });
  assert.ok(summary.includes("  agent_packet_search: repair_retrieval"));
  assert.ok(summary.includes("  degraded_reason: embedding unavailable"));
  assert.ok(summary.some(line => line.startsWith("  minimum_next:")));
  assert.ok(summary.some(line => line.startsWith("  handoff:")));
});

test(`${process.platform === "win32" ? "PowerShell" : "POSIX"} adapter forwards to the Node help surface`, () => {
  const environment = { ...process.env, CODESTORY_NODE: process.execPath };
  const invocation = process.platform === "win32"
    ? {
        command: "powershell",
        args: [
          "-NoProfile",
          "-ExecutionPolicy",
          "Bypass",
          "-File",
          join(scriptsDirectory, "codex-worktree-setup.ps1"),
          "-Help",
        ],
      }
    : {
        command: join(scriptsDirectory, "codex-worktree-setup.sh"),
        args: ["--help"],
      };
  const adapter = spawnSync(invocation.command, invocation.args, {
    encoding: "utf8",
    env: environment,
  });
  assert.equal(adapter.status, 0, `${adapter.stdout}\n${adapter.stderr}`);
  assert.match(adapter.stdout, /Usage: node scripts\/codex-worktree-setup\.mjs/);
});
