#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";

export function implementationForPlatform(platform, scriptDirectory) {
  if (platform === "win32") {
    return {
      command: "powershell",
      arguments: [
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        join(scriptDirectory, "codex-worktree-setup.ps1"),
      ],
    };
  }

  return {
    command: join(scriptDirectory, "codex-worktree-setup.sh"),
    arguments: [],
  };
}

export function argumentsForPlatform(platform, argumentsToForward) {
  if (platform !== "win32") return argumentsToForward;

  const windowsNames = new Map([
    ["--project", "-Project"],
    ["--intended-base-ref", "-IntendedBaseRef"],
    ["--pr-head-ref", "-PrHeadRef"],
    ["--branch-head-proof", "-BranchHeadProof"],
    ["--resolve-cli-only", "-ResolveCliOnly"],
    ["--self-test", "-SelfTest"],
  ]);
  return argumentsToForward.map(argument => windowsNames.get(argument) ?? argument);
}

function main() {
  const scriptDirectory = dirname(fileURLToPath(import.meta.url));
  const implementation = implementationForPlatform(process.platform, scriptDirectory);
  const forwardedArguments = argumentsForPlatform(process.platform, process.argv.slice(2));
  const result = spawnSync(
    implementation.command,
    [...implementation.arguments, ...forwardedArguments],
    {
      stdio: "inherit",
      env: { ...process.env, CODESTORY_NODE: process.execPath },
    },
  );

  if (result.error) {
    console.error(`Unable to start CodeStory worktree setup: ${result.error.message}`);
    process.exitCode = 1;
    return;
  }
  if (result.signal) {
    console.error(`CodeStory worktree setup stopped by ${result.signal}.`);
    process.exitCode = 1;
    return;
  }
  process.exitCode = result.status ?? 1;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  main();
}
