#!/usr/bin/env node
import { accessSync, constants, existsSync } from "node:fs";
import { delimiter, dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { spawn } from "node:child_process";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const pluginRoot = dirname(scriptDir);

function findRepoRoot() {
  let current = pluginRoot;
  for (;;) {
    if (existsSync(join(current, "scripts", "install-codestory.ps1"))) {
      return current;
    }
    const parent = dirname(current);
    if (parent === current) {
      return dirname(dirname(pluginRoot));
    }
    current = parent;
  }
}

const repoRoot = findRepoRoot();
const binaryName = process.platform === "win32" ? "codestory-cli.exe" : "codestory-cli";

function canRun(file) {
  try {
    accessSync(file, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

export function pathCandidates(name) {
  const extensions = process.platform === "win32" ? [".exe"] : [""];
  return (process.env.PATH ?? "")
    .split(delimiter)
    .filter(Boolean)
    .flatMap((entry) => extensions.map((extension) => join(entry, `${name}${extension}`)));
}

export function resolveCli() {
  const explicit = process.env.CODESTORY_CLI;
  if (explicit && canRun(explicit)) {
    return explicit;
  }
  for (const candidate of pathCandidates("codestory-cli")) {
    if (canRun(candidate)) {
      return candidate;
    }
  }
  for (const candidate of [
    join(repoRoot, "target", "release", binaryName),
    join(process.cwd(), "target", "release", binaryName),
  ]) {
    if (canRun(candidate)) {
      return candidate;
    }
  }
  return null;
}

export function resolveProject() {
  return (
    process.env.CODESTORY_PROJECT ||
    process.env.CODESTORY_WORKSPACE ||
    process.env.CODEX_WORKSPACE ||
    repoRoot ||
    process.cwd()
  );
}

function printMissingCli(project) {
  const setup = join(repoRoot, "scripts", "install-codestory.ps1");
  const worktreeSetup = join(repoRoot, "scripts", "codex-worktree-setup.ps1");
  process.stderr.write(
    [
      "codestory-cli was not found.",
      "Set CODESTORY_CLI to a ready codestory-cli binary or add codestory-cli to PATH.",
      `Setup action: powershell -ExecutionPolicy Bypass -File "${setup}" -Project "${project}"`,
      `PowerShell 7: pwsh -File "${setup}" -Project "${project}"`,
      `Worktree fallback: powershell -ExecutionPolicy Bypass -File "${worktreeSetup}" -Project "${project}" -ResolveCliOnly`,
      "",
    ].join("\n"),
  );
}

export function main() {
  const project = resolveProject();
  const cli = resolveCli();
  if (!cli) {
    printMissingCli(project);
    process.exitCode = 127;
    return;
  }

  const child = spawn(cli, ["serve", "--project", project, "--stdio", "--refresh", "none"], {
    stdio: "inherit",
    windowsHide: true,
  });
  child.on("exit", (code, signal) => {
    process.exitCode = code ?? (signal ? 1 : 0);
  });
  child.on("error", (error) => {
    process.stderr.write(`failed to launch codestory-cli serve --stdio: ${error.message}\n`);
    process.exitCode = 1;
  });
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
