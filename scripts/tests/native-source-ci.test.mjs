import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const action = './.github/actions/native-source';
const consumers = {
  'rust-ci.yml': ['linux-draft'],
  'crate-durability.yml': ['linux-durability'],
  'retrieval-engine-smoke.yml': ['linux-contracts', 'windows-manifest-missing'],
  'rustdoc.yml': ['warnings'],
  'source-proof.yml': ['full-source-gate', 'windows-native-contracts', 'freeze-hostile-mutations', 'freeze-windows-native-probe'],
  'packaged-platform-proof.yml': ['build', 'frozen-linux-qualification'],
  'macos-metal-proof.yml': ['packaged-metal'],
  'windows-vulkan-proof.yml': ['packaged-vulkan'],
  'linux-vulkan-proof.yml': ['optional-constant-calibration'],
  'notifier-parity.yml': ['parity-macos-fsevents', 'parity-windows-rdcw', 'parity-linux-wsl-inotify'],
};

test('native source reconstruction provisions non-Mac Python and uses the existing Mac interpreter', () => {
  const { runs } = parse(fs.readFileSync(path.join(root, action, 'action.yml'), 'utf8'));
  const setup = runs.steps.findIndex(step => step.uses?.startsWith('actions/setup-python@'));
  const bootstrap = runs.steps.findIndex(step => step.run?.includes('bootstrap-research-native-vendor.py'));
  assert.ok(setup >= 0 && setup < bootstrap, 'Python must be available before the bootstrap shell starts');
  assert.equal(runs.steps[setup].with['python-version'], '3.13');
  assert.equal(runs.steps[setup].if, "runner.os != 'macOS'");
  assert.deepEqual(runs.steps[setup].env, { PSExecutionPolicyPreference: 'Bypass' });
  assert.equal(runs.steps[setup]['continue-on-error'], undefined);
  assert.equal(runs.steps[bootstrap].if, undefined);
  assert.equal(runs.steps[bootstrap]['continue-on-error'], undefined);
});

test('every workspace Cargo job prepares pinned native source before its first Cargo consumer', () => {
  for (const [file, jobs] of Object.entries(consumers)) {
    const workflow = parse(fs.readFileSync(path.join(root, '.github/workflows', file), 'utf8'));
    for (const name of jobs) {
      const steps = workflow.jobs[name]?.steps;
      assert.ok(steps, `${file}:${name} consumer must still exist`);
      const prepare = steps.findIndex(step => step.uses === action);
      assert.ok(prepare >= 0, `${file}:${name} is missing native source reconstruction`);
      const cargo = steps.findIndex(step => /\bcargo(?:\s|\s*@)|cargo-build-artifacts\.test\.mjs/u.test(step.run ?? ''));
      assert.ok(cargo > prepare, `${file}:${name} invokes Cargo before native source reconstruction`);
      const condition = ['macos-metal-proof.yml', 'windows-vulkan-proof.yml'].includes(file)
        ? '${{ !inputs.use_packaged_cli_artifact }}' : undefined;
      assert.equal(steps[prepare].if, condition, `${file}:${name} must reconstruct on the source-build path`);
      assert.equal(steps[prepare]['continue-on-error'], undefined);
      for (const step of steps) {
        const key = step.with?.key ?? '';
        if (key.includes("hashFiles('Cargo.toml'")) {
          assert.ok(key.includes('vendor/llama-cpp-sys-2-patches/**'), `${file}:${name} cache must bind native patch inputs`);
        }
        if (step.name === 'Capture reusable build cache contract') {
          assert.ok(step.run.includes('--relevant-input vendor/llama-cpp-sys-2-patches/manifest.json'));
          assert.ok(step.run.includes('--relevant-input vendor/llama-cpp-sys-2-patches/0.1.151.patch'));
        }
      }
    }
  }
});

test('Windows static CI reconstructs, verifies and rechecks source without compiling it', () => {
  const workflow = parse(fs.readFileSync(path.join(root, '.github/workflows/plugin-static.yml'), 'utf8'));
  const job = workflow.jobs['windows-native-source'];
  assert.ok(job, 'Windows native source reconstruction job is missing');
  assert.equal(job['runs-on'], 'windows-latest');
  assert.ok(job['timeout-minutes'] > 0 && job['timeout-minutes'] <= 5);
  assert.equal(job.if, undefined);
  assert.equal(job['continue-on-error'], undefined);
  assert.equal(job.steps.length, 4);
  assert.equal(job.steps[0].uses, 'actions/checkout@v5');
  assert.equal(job.steps[1].uses, 'actions/setup-python@v7.0.0');
  assert.equal(job.steps[1].with['python-version'], '3.13');
  assert.equal(job.steps[2].uses, action);
  assert.ok(job.steps.every(step => step.if === undefined && step['continue-on-error'] === undefined));
  const verification = job.steps[3];
  assert.equal(verification.shell, 'bash');
  assert.deepEqual(verification.run.trim().split('\n'), [
    'set -euo pipefail',
    'python scripts/bootstrap-research-native-vendor.py --verify',
    'python scripts/bootstrap-research-native-vendor.py --ensure',
  ]);
});
