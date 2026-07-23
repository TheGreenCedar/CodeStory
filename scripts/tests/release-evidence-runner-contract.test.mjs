import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../../", import.meta.url);

async function source(path) {
  return readFile(new URL(path, root), "utf8");
}

test("release-evidence runner provisions and verifies its XDG runtime authority", async () => {
  const [contractSource, baselineSource, provision, runner, verify] = await Promise.all([
    source("scripts/release-evidence/machine-contract.json"),
    source("benchmarks/release-evidence/approved-baselines.json"),
    source("scripts/release-evidence/guest-provision.sh"),
    source("scripts/release-evidence/guest-runner.sh"),
    source("scripts/release-evidence/guest-verify.sh"),
  ]);
  const contract = JSON.parse(contractSource);
  const baselines = JSON.parse(baselineSource);
  const runtimeDir = `${contract.runner.root}/runtime`;
  const contractSha = createHash("sha256").update(contractSource).digest("hex");

  assert.match(provision, /runtime_dir="\$runner_root\/runtime"/);
  assert.match(runner, /runtime_dir="\$runner_root\/runtime"/);
  assert.match(verify, /runtime_dir="\$runner_root\/runtime"/);
  assert.equal(
    baselines.profiles[contract.profile_id].identity.machine_fingerprint,
    `${contract.profile_id}/${contractSha}`,
  );
  assert.match(provision, /install -d -o codestory-runner -g codestory-runner -m 0700 "\$runtime_dir"/);
  assert.match(runner, /Environment=XDG_RUNTIME_DIR=\$runtime_dir/);
  assert.match(verify, /stat -c '%u:%g:%a' "\$runtime_dir"/);
  assert.match(verify, /grep -qxF "XDG_RUNTIME_DIR=\$runtime_dir"/);
});
