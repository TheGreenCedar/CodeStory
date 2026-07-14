#!/usr/bin/env node

import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import {
  accessSync,
  chmodSync,
  constants as fsConstants,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import {
  delimiter,
  dirname,
  extname,
  isAbsolute,
  join,
  resolve,
} from "node:path";
import { fileURLToPath } from "node:url";

const DEFAULT_BASE_REF = "origin/dev/codestory-next";
const DETACHED_COMMIT = /^[0-9a-f]{40}$/i;
const CLI_VERSION = /^codestory-cli\s+([0-9][0-9A-Za-z.+-]*)$/;

const usage = `Usage: node scripts/codex-worktree-setup.mjs [options]

  --project <path>             Worktree to prepare (default: .)
  --intended-base-ref <ref>    Base ref used in the handoff proof summary
  --pr-head-ref <ref>          Optional PR head used in the proof summary
  --branch-head-proof          Prove only the PR branch head
  --resolve-cli-only           Resolve/install the CLI without indexing
  --full-retrieval-proof       Prepare and verify full retrieval (maintainers only)
  --self-test                  Run the shared Node setup test suite
  --help                       Show this help`;

function truthy(value) {
  return /^(?:1|true|yes)$/i.test(value ?? "");
}

export function parseArguments(argv, env = process.env) {
  const options = {
    project: ".",
    intendedBaseRef: env.CODESTORY_INTENDED_BASE_REF || DEFAULT_BASE_REF,
    prHeadRef: env.CODESTORY_PR_HEAD_REF || "",
    branchHeadProof: truthy(env.CODESTORY_BRANCH_HEAD_PROOF),
    resolveCliOnly: false,
    fullRetrievalProof: false,
    selfTest: false,
    help: false,
  };
  const aliases = new Map([
    ["-Project", "--project"],
    ["-IntendedBaseRef", "--intended-base-ref"],
    ["-PrHeadRef", "--pr-head-ref"],
    ["-BranchHeadProof", "--branch-head-proof"],
    ["-ResolveCliOnly", "--resolve-cli-only"],
    ["-FullRetrievalProof", "--full-retrieval-proof"],
    ["-SelfTest", "--self-test"],
    ["-Help", "--help"],
  ]);

  for (let index = 0; index < argv.length; index += 1) {
    const argument = aliases.get(argv[index]) ?? argv[index];
    if (["--project", "--intended-base-ref", "--pr-head-ref"].includes(argument)) {
      const value = argv[index + 1];
      if (value === undefined) {
        const error = new Error(`Missing value for ${argv[index]}`);
        error.exitCode = 2;
        throw error;
      }
      index += 1;
      if (argument === "--project") options.project = value;
      if (argument === "--intended-base-ref") options.intendedBaseRef = value;
      if (argument === "--pr-head-ref") options.prHeadRef = value;
      continue;
    }
    if (argument === "--branch-head-proof") options.branchHeadProof = true;
    else if (argument === "--resolve-cli-only") options.resolveCliOnly = true;
    else if (argument === "--full-retrieval-proof") options.fullRetrievalProof = true;
    else if (argument === "--self-test") options.selfTest = true;
    else if (["--help", "-h"].includes(argument)) options.help = true;
    else {
      const error = new Error(`Unknown option: ${argv[index]}`);
      error.exitCode = 2;
      throw error;
    }
  }
  return options;
}

function createContext(overrides = {}) {
  const context = {
    platform: process.platform,
    arch: process.arch,
    env: { ...process.env },
    scriptDirectory: dirname(fileURLToPath(import.meta.url)),
    spawnSync,
    log: console.log,
    warn: message => console.error(`warning: ${message}`),
    ...overrides,
  };
  context.env = { ...context.env };
  return context;
}

function outputText(value) {
  if (value === undefined || value === null) return "";
  return Buffer.isBuffer(value) ? value.toString("utf8") : String(value);
}

function invoke(context, command, args, { cwd, capture = false } = {}) {
  const shell = context.platform === "win32" && /\.(?:cmd|bat)$/i.test(command);
  return context.spawnSync(command, args, {
    cwd,
    env: context.env,
    encoding: "utf8",
    shell,
    stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
  });
}

function commandFailure(command, args, result) {
  if (result.error) return `${command} could not start: ${result.error.message}`;
  const detail = outputText(result.stderr).trim() || outputText(result.stdout).trim();
  return `${command} ${args.join(" ")} failed with exit code ${result.status ?? 1}${detail ? `\n${detail}` : ""}`;
}

function runRequired(context, command, args, options = {}) {
  const result = invoke(context, command, args, options);
  if (result.error || result.signal || result.status !== 0) {
    throw new Error(commandFailure(command, args, result));
  }
  return outputText(result.stdout);
}

function runCaptured(context, command, args, cwd) {
  const result = invoke(context, command, args, { cwd, capture: true });
  if (result.error || result.signal || result.status !== 0) return null;
  return outputText(result.stdout).trim();
}

function step(context, label, callback, optional = false) {
  context.log("");
  context.log(`==> ${label}`);
  try {
    return callback();
  } catch (error) {
    if (!optional) throw error;
    context.warn(`${label} failed: ${error.message}`);
    return undefined;
  }
}

function pathKey(value, platform) {
  const normalized = resolve(value);
  return platform === "win32" ? normalized.toLowerCase() : normalized;
}

function isRunnable(file, platform) {
  try {
    accessSync(file, platform === "win32" ? fsConstants.F_OK : fsConstants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function pathExtensions(env) {
  return (env.PATHEXT || ".COM;.EXE;.BAT;.CMD")
    .split(";")
    .filter(Boolean)
    .map(value => value.toLowerCase());
}

export function findExecutableOnPath(name, { platform = process.platform, env = process.env } = {}) {
  const directories = (env.PATH || "").split(delimiter).filter(Boolean);
  const suffixes = platform === "win32" && !extname(name)
    ? ["", ...pathExtensions(env)]
    : [""];
  for (const directory of directories) {
    for (const suffix of suffixes) {
      const candidate = join(directory, `${name}${suffix}`);
      if (isRunnable(candidate, platform)) return realpathSync(candidate);
    }
  }
  return null;
}

function candidatePath(candidate, context) {
  if (!candidate) return null;
  const hasPath = isAbsolute(candidate) || candidate.includes("/") || candidate.includes("\\");
  if (!hasPath) {
    return findExecutableOnPath(candidate, context);
  }
  if (!isRunnable(candidate, context.platform)) return null;
  return realpathSync(candidate);
}

function canonicalDirectory(candidate) {
  try {
    return realpathSync(candidate);
  } catch {
    throw new Error(`Project path does not exist: ${candidate}`);
  }
}

function samePath(left, right, platform) {
  try {
    return pathKey(realpathSync(left), platform) === pathKey(realpathSync(right), platform);
  } catch {
    return false;
  }
}

function gitText(context, cwd, args) {
  return runCaptured(context, "git", args, cwd);
}

function gitCommit(context, cwd, ref) {
  if (!ref) return null;
  const output = gitText(context, cwd, ["rev-parse", "--verify", `${ref}^{commit}`]);
  return output?.split(/\r?\n/u)[0] || null;
}

export function remoteHeadName(ref) {
  if (!ref) return null;
  if (ref.startsWith("origin/")) return `refs/heads/${ref.slice("origin/".length)}`;
  if (ref.startsWith("refs/heads/")) return ref;
  if (ref.startsWith("refs/") || DETACHED_COMMIT.test(ref)) return null;
  return `refs/heads/${ref}`;
}

function remoteHeadVerification(context, cwd, ref) {
  const remoteRef = remoteHeadName(ref);
  if (!remoteRef) return null;
  const result = gitText(context, cwd, ["ls-remote", "origin", remoteRef]);
  return {
    command: `git ls-remote origin ${remoteRef}`,
    result: result || "<no remote tip>",
    commit: result?.split(/\s+/u)[0] || null,
  };
}

export function proofTarget(baseRef, headRef, branchHeadProof) {
  if (!headRef) return baseRef;
  return branchHeadProof ? `branch-head:${headRef}` : `base:${baseRef} + pr-head:${headRef}`;
}

function writeHandoffSummary(context, projectPath, options) {
  if (!findExecutableOnPath("git", context)) {
    context.warn("Git is unavailable; skipping CodeStory handoff proof-target status.");
    return;
  }
  const childHead = gitCommit(context, projectPath, "HEAD");
  if (!childHead) {
    context.warn("Current directory is not a Git worktree; skipping CodeStory handoff proof-target status.");
    return;
  }

  const baseCommit = gitCommit(context, projectPath, options.intendedBaseRef);
  const headCommit = gitCommit(context, projectPath, options.prHeadRef);
  const branch = gitText(context, projectPath, ["symbolic-ref", "--quiet", "--short", "HEAD"])
    || `detached:${childHead}`;
  const baseRemote = remoteHeadVerification(context, projectPath, options.intendedBaseRef);
  const headRemote = remoteHeadVerification(context, projectPath, options.prHeadRef);
  const warnings = [];

  const compareRemote = (label, localCommit, remote) => {
    if (localCommit && remote?.commit && localCommit !== remote.commit) {
      warnings.push(`${label} stale: local=${localCommit} remote=${remote.commit}`);
    }
  };
  if (!baseCommit) warnings.push(`intended_base_ref unresolved: ${options.intendedBaseRef}`);
  compareRemote("intended_base_ref", baseCommit, baseRemote);
  compareRemote(
    "main",
    gitCommit(context, projectPath, "main"),
    remoteHeadVerification(context, projectPath, "origin/main"),
  );
  compareRemote(
    "dev/codestory-next",
    gitCommit(context, projectPath, "dev/codestory-next"),
    remoteHeadVerification(context, projectPath, "origin/dev/codestory-next"),
  );
  if (options.prHeadRef) {
    if (!headCommit) warnings.push(`pr_head_ref unresolved: ${options.prHeadRef}`);
    compareRemote("pr_head_ref", headCommit, headRemote);
    if (options.branchHeadProof) {
      warnings.push("branch-head proof requested; default PR proof is current base plus PR head.");
    }
  }

  context.log("CodeStory handoff proof target");
  context.log(`  intended_base_ref: ${options.intendedBaseRef}`);
  context.log(`  resolved_base_commit: ${baseCommit || "unresolved"}`);
  context.log(`  child_start_head: ${childHead}`);
  context.log(`  child_branch_or_detached: ${branch}`);
  context.log(`  proof_target: ${proofTarget(options.intendedBaseRef, options.prHeadRef, options.branchHeadProof)}`);
  context.log(`  pr_head_ref: ${options.prHeadRef || "none"}`);
  context.log(`  pr_head_commit: ${headCommit || "none"}`);
  if (baseRemote) {
    context.log(`  remote_tip_verification.intended_base.command: ${baseRemote.command}`);
    context.log(`  remote_tip_verification.intended_base.result: ${baseRemote.result}`);
  }
  if (headRemote) {
    context.log(`  remote_tip_verification.pr_head.command: ${headRemote.command}`);
    context.log(`  remote_tip_verification.pr_head.result: ${headRemote.result}`);
  }
  for (const warning of warnings) {
    context.warn(`CodeStory handoff proof target: ${warning}`);
  }
}

export function expectedCliVersion(projectPath) {
  const manifest = join(projectPath, "crates", "codestory-cli", "Cargo.toml");
  const match = readFileSync(manifest, "utf8").match(/^\s*version\s*=\s*"([^"]+)"/mu);
  if (!match) throw new Error(`Unable to read expected codestory-cli version from ${manifest}.`);
  return match[1];
}

function homeDirectory(context) {
  return context.env.HOME || context.env.USERPROFILE || homedir();
}

export function installDirectory(context) {
  if (context.env.CODESTORY_HOME) return join(context.env.CODESTORY_HOME, "bin");
  if (context.platform === "win32" && context.env.LOCALAPPDATA) {
    return join(context.env.LOCALAPPDATA, "CodeStory", "bin");
  }
  return join(homeDirectory(context), ".codestory", "bin");
}

function cliNames(platform) {
  return platform === "win32"
    ? ["codestory-cli.exe", "codestory-cli.cmd", "codestory-cli"]
    : ["codestory-cli"];
}

function worktreePaths(context, projectPath) {
  const output = gitText(context, projectPath, ["worktree", "list", "--porcelain"]);
  if (!output) return [];
  return output
    .split(/\r?\n/u)
    .filter(line => line.startsWith("worktree "))
    .map(line => line.slice("worktree ".length));
}

export function cliCandidates(projectPath, expectedVersion, contextOverrides = {}) {
  const context = createContext(contextOverrides);
  const candidates = [];
  if (context.env.CODESTORY_CLI) candidates.push(context.env.CODESTORY_CLI);
  const pathCli = findExecutableOnPath("codestory-cli", context);
  if (pathCli) candidates.push(pathCli);

  const installRoot = installDirectory(context);
  for (const name of cliNames(context.platform)) candidates.push(join(installRoot, name));
  for (const name of cliNames(context.platform)) {
    candidates.push(join(installRoot, "releases", expectedVersion, name));
  }
  for (const name of cliNames(context.platform)) {
    candidates.push(join(projectPath, "target", "release", name));
  }
  for (const sibling of worktreePaths(context, projectPath)) {
    if (samePath(sibling, projectPath, context.platform)) continue;
    for (const name of cliNames(context.platform)) {
      candidates.push(join(sibling, "target", "release", name));
    }
  }

  const seen = new Set();
  return candidates.filter(candidate => {
    const key = pathKey(candidate, context.platform);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function cliVersion(context, candidate) {
  const resolved = candidatePath(candidate, context);
  if (!resolved) return null;
  const output = runCaptured(context, resolved, ["--version"]);
  if (!output) return { path: resolved, version: null };
  const match = output.split(/\r?\n/u)[0].match(CLI_VERSION);
  return { path: resolved, version: match?.[1] || null };
}

export function findCli(projectPath, expectedVersion, contextOverrides = {}) {
  const context = createContext(contextOverrides);
  const stale = [];
  for (const candidate of cliCandidates(projectPath, expectedVersion, context)) {
    const inspected = cliVersion(context, candidate);
    if (!inspected) continue;
    if (inspected.version === expectedVersion) return inspected.path;
    if (inspected.version) stale.push(`${candidate} reported ${inspected.version}`);
  }
  let message = [
    `No ready codestory-cli ${expectedVersion} found via CODESTORY_CLI, PATH,`,
    "the CodeStory install directory, this worktree's target/release,",
    "or sibling worktree target/release directories.",
  ].join(" ");
  if (stale.length) message += ` Stale candidates: ${stale.join("; ")}.`;
  throw new Error(message);
}

export function resolveSccache(contextOverrides = {}) {
  const context = createContext(contextOverrides);
  const homeCandidate = join(
    homeDirectory(context),
    ".cargo",
    "bin",
    context.platform === "win32" ? "sccache.exe" : "sccache",
  );
  if (isRunnable(homeCandidate, context.platform)) return realpathSync(homeCandidate);
  return findExecutableOnPath("sccache", context);
}

function releaseTarget(platform, arch) {
  const architecture = arch === "x64" ? "x64" : arch === "arm64" ? "arm64" : null;
  if (!architecture) return null;
  if (platform === "darwin") return `macos-${architecture}`;
  if (platform === "linux") return `linux-${architecture}`;
  if (platform === "win32") return `windows-${architecture}`;
  return null;
}

function checksum(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function findNamedFile(root, name) {
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const candidate = join(root, entry.name);
    if (entry.isFile() && entry.name === name) return candidate;
    if (entry.isDirectory()) {
      const nested = findNamedFile(candidate, name);
      if (nested) return nested;
    }
  }
  return null;
}

function installPosixRelease(context, projectPath, version) {
  const target = releaseTarget(context.platform, context.arch);
  if (!target) throw new Error(`No release asset is available for ${context.platform}/${context.arch}.`);
  const archiveName = `codestory-cli-v${version}-${target}.tar.gz`;
  const baseUrl = context.env.CODESTORY_RELEASE_BASE_URL
    || `https://github.com/TheGreenCedar/CodeStory/releases/download/v${version}`;
  const temporary = mkdtempSync(join(tmpdir(), "codestory-install-"));
  const archive = join(temporary, archiveName);
  const sums = join(temporary, "SHA256SUMS.txt");
  try {
    runRequired(context, "curl", ["-fsSL", `${baseUrl}/SHA256SUMS.txt`, "-o", sums]);
    runRequired(context, "curl", ["-fsSL", `${baseUrl}/${archiveName}`, "-o", archive]);
    const expected = readFileSync(sums, "utf8")
      .split(/\r?\n/u)
      .map(line => line.match(/^([0-9a-fA-F]{64})\s+\*?(.+)$/u))
      .find(match => match?.[2] === archiveName)?.[1]
      ?.toLowerCase();
    if (!expected) throw new Error(`SHA256SUMS.txt has no valid entry for ${archiveName}.`);
    const actual = checksum(archive);
    if (actual !== expected) {
      throw new Error(`Downloaded archive checksum mismatch for ${archiveName}: expected ${expected}, got ${actual}`);
    }

    const extracted = join(temporary, "extract");
    mkdirSync(extracted, { recursive: true });
    runRequired(context, "tar", ["-xzf", archive, "-C", extracted]);
    const binary = findNamedFile(extracted, "codestory-cli");
    if (!binary) throw new Error("Downloaded archive did not contain codestory-cli.");

    const installRoot = installDirectory(context);
    mkdirSync(installRoot, { recursive: true });
    let destination = join(installRoot, "codestory-cli");
    try {
      copyFileSync(binary, destination);
    } catch {
      destination = join(installRoot, "releases", version, "codestory-cli");
      mkdirSync(dirname(destination), { recursive: true });
      copyFileSync(binary, destination);
    }
    chmodSync(destination, 0o755);
    if (cliVersion(context, destination)?.version !== version) {
      throw new Error(`Installed codestory-cli did not report expected version ${version}.`);
    }
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

function installCurrentReleaseCli(context, projectPath, version) {
  context.log("");
  context.log("==> Install current release CLI");
  context.log(`Trying codestory-cli ${version} release install before Cargo build.`);
  if (context.platform === "win32") {
    const installer = join(context.scriptDirectory, "install-codestory.ps1");
    runRequired(context, "powershell", [
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-File",
      installer,
      "-Project",
      projectPath,
      "-Version",
      version,
    ]);
    return;
  }
  installPosixRelease(context, projectPath, version);
}

function resolveCli(context, projectPath, expectedVersion, resolveCliOnly) {
  let resolutionError;
  try {
    return findCli(projectPath, expectedVersion, context);
  } catch (error) {
    resolutionError = error.message;
  }

  let installError;
  try {
    const installer = context.installRelease ?? installCurrentReleaseCli;
    installer(context, projectPath, expectedVersion);
    return findCli(projectPath, expectedVersion, context);
  } catch (error) {
    installError = error.message;
  }
  if (resolveCliOnly) {
    throw new Error([
      resolutionError,
      `Current-release install failed: ${installError}.`,
      "Set CODESTORY_CLI to a ready binary.",
    ].join(" "));
  }

  step(context, "Build release CLI", () => {
    context.warn([
      resolutionError,
      `Current-release install failed: ${installError}.`,
      "Building release CLI with cargo.",
    ].join(" "));
    runRequired(context, "cargo", ["build", "--release", "--locked", "-p", "codestory-cli"], {
      cwd: projectPath,
    });
  });
  return findCli(projectPath, expectedVersion, context);
}

function findRehydrateSource(context, projectPath) {
  const configured = context.env.CODESTORY_REHYDRATE_FROM;
  if (configured) {
    try {
      const source = canonicalDirectory(configured);
      if (!samePath(source, projectPath, context.platform)) return source;
    } catch (error) {
      context.warn(`Ignoring CODESTORY_REHYDRATE_FROM='${configured}': ${error.message}`);
    }
  }
  return worktreePaths(context, projectPath)
    .filter(candidate => !samePath(candidate, projectPath, context.platform))
    .find(candidate => existsSync(join(candidate, "Cargo.toml"))) || null;
}

function fullRetrievalProofArguments(projectPath) {
  return [
    "ready",
    "--goal",
    "agent",
    "--repair",
    "--project",
    projectPath,
    "--format",
    "json",
    "--run-id",
    "shared-agent",
  ];
}

export function setupSummaryLines(doctor, cliVersion, fullRetrievalProof) {
  const verdict = goal => (doctor.readiness || []).find(item => item.goal === goal);
  const local = verdict("local_navigation");
  const agent = verdict("agent_packet_search");
  const repositoryMap = local?.status === "ready" ? "ready" : "needs_attention";
  const activePreparation = (doctor.readiness_broker?.operations || []).length > 0;
  const backgroundPreparation = agent?.status === "ready" && doctor.retrieval_mode === "full"
    ? "ready"
    : activePreparation
      ? "in_progress"
      : fullRetrievalProof
        ? "needs_attention"
        : "not_requested";
  const lines = [
    "CodeStory worktree setup complete",
    `  cli_version: ${cliVersion}`,
    `  repository_map: ${repositoryMap}`,
    `  background_preparation: ${backgroundPreparation}`,
  ];
  return lines;
}

function setupSummary(context, cli, cliVersion, projectPath, fullRetrievalProof) {
  const result = invoke(context, cli, ["doctor", "--project", projectPath, "--format", "json"], {
    cwd: projectPath,
    capture: true,
  });
  if (result.error || result.signal || result.status !== 0) {
    throw new Error(commandFailure(cli, ["doctor", "--project", projectPath, "--format", "json"], result));
  }
  const doctor = JSON.parse(outputText(result.stdout));
  const agent = (doctor.readiness || []).find(item => item.goal === "agent_packet_search");
  if (fullRetrievalProof && (agent?.status !== "ready" || doctor.retrieval_mode !== "full")) {
    throw new Error("Full retrieval proof did not reach the ready state.");
  }
  for (const line of setupSummaryLines(doctor, cliVersion, fullRetrievalProof)) context.log(line);
}

export function runSetup(options, contextOverrides = {}) {
  const context = createContext(contextOverrides);
  const projectPath = canonicalDirectory(options.project);
  const expectedVersion = expectedCliVersion(projectPath);

  writeHandoffSummary(context, projectPath, options);
  const sccache = resolveSccache(context);
  if (sccache) {
    context.env.RUSTC_WRAPPER = sccache;
    context.log(`Using RUSTC_WRAPPER=${sccache}`);
  }

  const cli = resolveCli(context, projectPath, expectedVersion, options.resolveCliOnly);
  if (options.resolveCliOnly) return { cli, projectPath };

  const source = findRehydrateSource(context, projectPath);
  if (source) {
    step(
      context,
      `Reuse repository map from ${source}`,
      () => runRequired(context, cli, ["cache", "rehydrate", "--from-project", source, "--project", projectPath], {
        cwd: projectPath,
        capture: true,
      }),
      true,
    );
  } else {
    context.log("");
    context.log("==> Reuse repository map");
    context.log("No sibling repository map found; preparing this worktree directly.");
  }
  step(context, "Prepare repository map", () => runRequired(
    context,
    cli,
    ["index", "--project", projectPath, "--refresh", "auto"],
    { cwd: projectPath, capture: true },
  ));
  if (options.fullRetrievalProof) {
    step(context, "Prepare full retrieval proof", () => runRequired(
      context,
      cli,
      fullRetrievalProofArguments(projectPath),
      { cwd: projectPath, capture: true },
    ));
  }
  step(
    context,
    "Check setup state",
    () => setupSummary(context, cli, expectedVersion, projectPath, options.fullRetrievalProof),
  );
  return { cli, projectPath };
}

function runSelfTest(context) {
  runRequired(context, process.execPath, [
    "--test",
    join(context.scriptDirectory, "tests", "codex-worktree-setup.test.mjs"),
  ]);
}

export function main(argv = process.argv.slice(2), contextOverrides = {}) {
  const context = createContext(contextOverrides);
  try {
    const options = parseArguments(argv, context.env);
    if (options.help) {
      context.log(usage);
      return 0;
    }
    if (options.selfTest) {
      runSelfTest(context);
      return 0;
    }
    runSetup(options, context);
    return 0;
  } catch (error) {
    console.error(error.message);
    if (error.exitCode === 2) console.error(usage);
    return error.exitCode ?? 1;
  }
}

const entrypoint = process.argv[1] ? resolve(process.argv[1]) : "";
const modulePath = fileURLToPath(import.meta.url);
const sameModule = process.platform === "win32"
  ? entrypoint.toLowerCase() === modulePath.toLowerCase()
  : entrypoint === modulePath;
if (sameModule) process.exitCode = main();
