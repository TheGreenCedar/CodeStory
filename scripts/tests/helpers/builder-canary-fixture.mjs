import { CONTRACT, REQUIRED, canaryExecutionContext, digestObject } from "../../lib/builder-operation-canary.mjs";

export function passingCanaryFixture() {
  const project = "/canary/project";
  const context = canaryExecutionContext({
    cli: process.execPath, cliSha256: "a".repeat(64), sandbox: "workspace-write", timeoutMs: 60_000,
    envForArm: (arm) => ({ CODEX_HOME: `/host/${arm}`, CODESTORY_CACHE_ROOT: "/cache/root" }),
  });
  const receipt = {
    contract: CONTRACT, status: "pass", project, cli_sha256: context.cli_sha256,
    sandbox: context.sandbox, context_sha256: digestObject(context),
    operations: Object.entries(REQUIRED).flatMap(([arm, operations]) => operations.map((operation) => ({
      arm, operation, exit_code: 0, shape_valid: true, sandboxed: true,
      wall_ms: 1, stdout_sha256: "b".repeat(64), stderr_sha256: "c".repeat(64),
      command: "codex",
      arguments: ["sandbox", "--permission-profile", ":workspace", "--cd", project, "--",
        ...(operation === "native_search" ? ["rg", "seed", "src/lib.rs"] : operation === "native_source" ? ["sed", "-n", "1,2p", "src/lib.rs"] : [context.cli_path, operation, "--project", project])],
      codex_home: context.arms[arm].codex_home, environment: { ...context.arms[arm] }, process: { ...context.process },
    }))),
  };
  return { context, receipt };
}

export const contextMutations = [
  (receipt) => { receipt.sandbox = "danger-full-access"; },
  (receipt) => { delete receipt.operations[0].environment; },
  (receipt) => { receipt.operations[0].codex_home = "/different/host"; },
  (receipt) => { receipt.operations[0].command = "/bin/true"; receipt.operations[0].arguments = []; },
  (receipt) => { receipt.operations[0].process = { timeout_ms: 1 }; },
  (receipt) => { receipt.operations[0].environment.cache_configuration_sha256 = "0".repeat(64); },
];
