// The bounded wait behind scripts/prove-plugin-pinned-provision.mjs.
//
// This module holds no top-level side effects so a test can import the wait without running the
// proof. The proof itself stays an entry point that always executes its body: an entry-point
// guard on a release gate can only fail open, and a gate that exits 0 without proving anything
// is worse than one that fails.

import fs from "node:fs";

// Wait for the launcher to publish managed runtime metadata.
//
// Order matters. The read comes first so a launcher that published managed metadata and then
// exited cleanly is reported as the success it is. The liveness guards then run on every tick
// rather than only when the read throws: a launcher that resolves to a non-managed source (pin
// or archive digest drift) keeps writing a readable file, so guards nested in the read's catch
// would never fire again and the proof would hang until the CI job timeout.
export function waitForManagedRuntime({ child, runtimeMetadata, timeoutMs, intervalMs = 250 }) {
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    throw new TypeError(`waitForManagedRuntime needs a positive finite timeoutMs, got ${timeoutMs}.`);
  }
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolve, reject) => {
    const poll = setInterval(() => {
      let metadata;
      try {
        metadata = JSON.parse(fs.readFileSync(runtimeMetadata, "utf8"));
      } catch {
        // Not written yet, or caught mid-write. Fall through to the guards and read again.
        metadata = undefined;
      }
      if (metadata !== null && typeof metadata === "object" && metadata.source === "managed") {
        clearInterval(poll);
        resolve(metadata);
        return;
      }
      if (child.exitCode !== null) {
        clearInterval(poll);
        reject(new Error(`launcher exited ${child.exitCode} before provisioning finished.`));
        return;
      }
      if (Date.now() > deadline) {
        clearInterval(poll);
        child.kill();
        reject(new Error(`provisioning did not finish within ${timeoutMs}ms.`));
      }
    }, intervalMs);
  });
}
