import assert from "node:assert/strict";
import test from "node:test";

import { buildCacheContract } from "./cargo-cache-contract.mjs";

const SHA_A = "a".repeat(40);
const SHA_B = "b".repeat(40);
const DIGEST_A = "1".repeat(64);
const DIGEST_B = "2".repeat(64);

function contract(overrides = {}) {
  return buildCacheContract({
    namespace: "codestory-cli-native-v4",
    exactSha: SHA_A,
    os: "Windows",
    target: "x86_64-pc-windows-msvc",
    rustVersion: "1.95.0",
    features: "codestory-cli-default-features",
    nativeToolchain: "msvc-14.50.35717",
    generator: "ninja",
    cmakeVersion: "4.1.2",
    ninjaVersion: "1.13.1",
    sccacheVersion: "v0.16.0",
    cargoLockSha256: DIGEST_A,
    cargoConfigSha256: DIGEST_A,
    relevantInputs: {
      ".github/docker/linux-glibc-build.Dockerfile": DIGEST_A,
      ".github/docker/glslc": DIGEST_A,
    },
    extraIdentity: {
      linux_glibc_build_image: "rust@sha256:one",
    },
    ...overrides,
  });
}

test("a new exact SHA restores the same compatible compiler namespace", () => {
  const seeded = contract();
  const next = contract({ exactSha: SHA_B });

  assert.equal(next.compilerPrefix, seeded.compilerPrefix);
  assert.notEqual(next.compilerKey, seeded.compilerKey);
  assert.equal(seeded.compilerKey, `${seeded.compilerPrefix}${SHA_A}`);
  assert.equal(next.compilerKey, `${next.compilerPrefix}${SHA_B}`);
  assert.equal(seeded.dependencyKey, next.dependencyKey);
  assert.equal(seeded.compilerKey.endsWith(SHA_A), true);
  assert.equal(seeded.compilerKey.slice(0, -SHA_A.length).includes(SHA_A), false);
});

test("every compiler compatibility boundary invalidates the restore prefix", async (t) => {
  const baseline = contract();
  const cases = [
    ["operating system", { os: "Linux" }],
    ["target", { target: "x86_64-unknown-linux-gnu" }],
    ["Rust version", { rustVersion: "1.95.1" }],
    ["Cargo.lock", { cargoLockSha256: DIGEST_B }],
    ["Cargo config", { cargoConfigSha256: DIGEST_B }],
    ["features", { features: "workspace-all-features" }],
    ["native toolchain", { nativeToolchain: "msvc-14.51" }],
    ["generator", { generator: "visual-studio" }],
    ["CMake", { cmakeVersion: "4.2.0" }],
    ["Ninja", { ninjaVersion: "1.14.0" }],
    ["sccache format", { sccacheVersion: "v0.17.0" }],
    [
      "relevant Docker input",
      {
        relevantInputs: {
          ".github/docker/linux-glibc-build.Dockerfile": DIGEST_B,
          ".github/docker/glslc": DIGEST_A,
        },
      },
    ],
    [
      "pinned Docker image",
      {
        extraIdentity: {
          linux_glibc_build_image: "rust@sha256:two",
        },
      },
    ],
  ];

  for (const [name, override] of cases) {
    await t.test(name, () => {
      const changed = contract(override);
      assert.notEqual(changed.compatibilityHash, baseline.compatibilityHash);
      assert.notEqual(changed.compilerPrefix, baseline.compilerPrefix);
    });
  }
});

test("dependency inputs use an exact content key without candidate duplication", () => {
  const baseline = contract();
  assert.equal(contract({ exactSha: SHA_B }).dependencyKey, baseline.dependencyKey);
  assert.notEqual(contract({ cargoLockSha256: DIGEST_B }).dependencyKey, baseline.dependencyKey);
  assert.notEqual(contract({ cargoConfigSha256: DIGEST_B }).dependencyKey, baseline.dependencyKey);
  assert.notEqual(contract({ rustVersion: "1.95.1" }).dependencyKey, baseline.dependencyKey);
  assert.notEqual(contract({ target: "aarch64-apple-darwin" }).dependencyKey, baseline.dependencyKey);
  assert.notEqual(contract({ os: "macOS" }).dependencyKey, baseline.dependencyKey);
});

test("compiler save keys require one full exact-SHA suffix", () => {
  assert.throws(
    () => contract({ exactSha: "abc" }),
    /full lowercase commit SHA/u,
  );
  assert.throws(
    () => contract({ exactSha: SHA_A.toUpperCase() }),
    /full lowercase commit SHA/u,
  );
});
