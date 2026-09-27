import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  assertShippingFeatureContract,
  buildCargoArtifactManifest,
  verifyCargoArtifactManifest,
} from "./cargo-build-artifacts.mjs";

const SOURCE_SHA = "a".repeat(40);
const SOURCE_TREE = "b".repeat(40);
const RUST_TARGET = "x86_64-pc-windows-msvc";
const SCRIPT = fileURLToPath(new URL("./cargo-build-artifacts.mjs", import.meta.url));
const WORKSPACE_ROOT = fileURLToPath(new URL("../..", import.meta.url));

test("real shipping dependency graph excludes qualification features", async (t) => {
  // Cargo's resolved package features expose default dependency edges that a
  // hand-built compiler-artifact fixture can never prove absent. Both shipped
  // bins belong to codestory-cli and share this package-level feature graph;
  // the artifact gate separately checks that Cargo emitted each bin.
  for (const defaultMode of ["defaults", "no-default-features"]) {
    await t.test(defaultMode, () => {
      const args = [
        "tree", "--locked", "-p", "codestory-cli", "--edges", "normal,build",
        "--prefix", "none", "--format", "{p} {f}",
      ];
      if (defaultMode === "no-default-features") args.push("--no-default-features");
      const result = spawnSync("cargo", args, {
        cwd: WORKSPACE_ROOT,
        encoding: "utf8",
        env: { ...process.env, RUSTC_WRAPPER: "" },
      });
      assert.equal(result.status, 0, result.error?.message ?? result.stderr);
      for (const [packageName, forbidden] of [
        ["codestory-cli", ["proof-qualification-support"]],
        ["codestory-runtime", ["benchmark-support", "proof-qualification-support", "test-support"]],
        ["codestory-retrieval", ["benchmark-support", "test-support"]],
        ["codestory-agent", ["test-support"]],
      ]) {
        const rows = result.stdout.split("\n").filter((line) =>
          line.startsWith(`${packageName} v`)
          && line.replaceAll("\\", "/").includes(`/crates/${packageName})`)
        );
        assert.ok(rows.length > 0, `missing ${packageName} in resolved graph`);
        for (const row of rows) {
          const enabled = row.slice(row.lastIndexOf(")") + 1).trim().split(",");
          for (const feature of forbidden) {
            assert.ok(!enabled.includes(feature), `${defaultMode}: ${packageName}/${feature} in ${row}`);
          }
        }
      }
    });
  }
});

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function fixture({
  includeQualificationDriver = true,
  binaryTargetTest = true,
} = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-cargo-artifacts-"));
  const targetDir = path.join(root, "target");
  const releaseDir = path.join(targetDir, RUST_TARGET, "release");
  const depsDir = path.join(releaseDir, "deps");
  fs.mkdirSync(depsDir, { recursive: true });

  function writePackageManifest(packageName) {
    const manifest = path.join(root, "crates", packageName, "Cargo.toml");
    if (!fs.existsSync(manifest)) {
      fs.mkdirSync(path.dirname(manifest), { recursive: true });
      fs.writeFileSync(
        manifest,
        `[package]\nname = "${packageName}"\nversion = "0.16.3"\n`,
      );
    }
    return manifest;
  }

  const artifacts = [
    {
      alias: "cli",
      packageName: "codestory-cli",
      kind: "bin",
      targetName: "codestory-cli",
      executable: path.join(releaseDir, "codestory-cli.exe"),
      contents: "cli",
    },
    {
      alias: "runtime",
      packageName: "codestory-cli",
      kind: "bin",
      targetName: "codestory-cli-runtime",
      executable: path.join(releaseDir, "codestory-cli-runtime.exe"),
      contents: "runtime",
    },
  ];
  if (includeQualificationDriver) {
    artifacts.push({
      alias: "qualification_driver",
      packageName: "codestory-bench",
      kind: "bin",
      targetName: "codestory_embedding_qualification",
      executable: path.join(releaseDir, "codestory_embedding_qualification.exe"),
      contents: "qualification driver",
    });
  }
  for (const artifact of artifacts) {
    fs.writeFileSync(artifact.executable, artifact.contents);
    artifact.manifest = writePackageManifest(artifact.packageName);
  }

  const messages = artifacts.map((artifact) => ({
    reason: "compiler-artifact",
    package_id: `path+file://${path.dirname(artifact.manifest)}#0.16.3`,
    manifest_path: artifact.manifest,
    target: {
      kind: [artifact.kind],
      crate_types: ["bin"],
      name: artifact.targetName,
      src_path: `/checkout/crates/${artifact.packageName}/target.rs`,
      edition: "2024",
      doc: false,
      doctest: false,
      // Cargo reports whether a target is test-capable here. Release binaries
      // commonly report true; profile.test below identifies the active build.
      test: binaryTargetTest,
    },
    profile: {
      opt_level: "3",
      debuginfo: 0,
      debug_assertions: false,
      overflow_checks: false,
      test: false,
    },
    features: [],
    filenames: [artifact.executable],
    executable: artifact.executable,
    fresh: false,
  }));
  // Every crate the shipping feature gate speaks for has to be in the graph it inspects.
  for (const packageName of [
    "codestory-agent",
    "codestory-retrieval",
    "codestory-runtime",
  ]) {
    const manifest = writePackageManifest(packageName);
    messages.push({
      reason: "compiler-artifact",
      package_id:
        `path+file://${path.dirname(manifest)}#${packageName}@0.16.3`,
      manifest_path: manifest,
      target: {
        kind: ["lib"],
        crate_types: ["lib"],
        name: packageName.replaceAll("-", "_"),
        src_path: `/checkout/crates/${packageName}/src/lib.rs`,
        edition: "2024",
        doc: true,
        doctest: true,
        test: true,
      },
      profile: {
        opt_level: "3",
        debuginfo: 0,
        debug_assertions: false,
        overflow_checks: false,
        test: false,
      },
      features: [],
      filenames: [
        path.join(releaseDir, "deps", `lib${packageName.replaceAll("-", "_")}.rlib`),
      ],
      executable: null,
      fresh: false,
    });
  }
  messages.unshift({
    reason: "compiler-artifact",
    package_id: "registry+https://example.invalid/index#serde@1.0.0",
    target: {
      kind: ["lib"],
      crate_types: ["lib"],
      name: "serde",
      src_path: "/registry/serde/src/lib.rs",
      edition: "2021",
      doc: true,
      doctest: true,
      test: true,
    },
    profile: {
      opt_level: "3",
      debuginfo: 0,
      debug_assertions: false,
      overflow_checks: false,
      test: false,
    },
    filenames: [path.join(releaseDir, "deps", "libserde.rlib")],
    executable: null,
  });
  messages.push({
    reason: "build-finished",
    success: true,
  });
  const qualificationMessages = includeQualificationDriver
    ? [
      messages.splice(messages.findIndex(
        (message) => message.target?.name === "codestory_embedding_qualification",
      ), 1)[0],
      { reason: "build-finished", success: true },
    ]
    : [];

  return {
    artifacts,
    expectations: artifacts.map(
      ({ alias, packageName, kind, targetName }) =>
        `${alias}=${packageName}:${kind}:${targetName}`,
    ),
    jsonLines: messages.map((message) => JSON.stringify(message)).join("\n"),
    messages,
    qualificationJsonLines: includeQualificationDriver
      ? qualificationMessages.map((message) => JSON.stringify(message)).join("\n")
      : undefined,
    qualificationMessages,
    releaseDir,
    root,
    targetDir,
  };
}

function refreshCargoJson(input) {
  input.jsonLines = input.messages
    .map((message) => JSON.stringify(message))
    .join("\n");
  if (input.qualificationMessages.length > 0) {
    input.qualificationJsonLines = input.qualificationMessages
      .map((message) => JSON.stringify(message))
      .join("\n");
  }
}

function featureMessage(input, packageName) {
  const message = input.messages.find(
    (entry) => entry.target?.name === packageName.replaceAll("-", "_"),
  );
  assert.ok(message, `missing fixture message for ${packageName}`);
  return message;
}

function build(input = fixture()) {
  const manifest = buildCargoArtifactManifest({
    exactSha: SOURCE_SHA,
    exactTree: SOURCE_TREE,
    expectations: input.expectations,
    jsonLines: input.jsonLines,
    qualificationJsonLines: input.qualificationJsonLines,
    rustTarget: RUST_TARGET,
    targetDir: input.targetDir,
    workspaceRoot: input.root,
  });
  return { input, manifest };
}

function addCargoBinPeer(input, alias = "cli", peerName) {
  const artifact = input.artifacts.find((entry) => entry.alias === alias);
  assert.ok(artifact);
  assert.equal(artifact.kind, "bin");
  const peer = path.join(
    input.releaseDir,
    "deps",
    peerName ?? `${artifact.targetName.replaceAll("-", "_")}.exe`,
  );
  fs.linkSync(artifact.executable, peer);
  return peer;
}

function verify(input, manifest, exactSha = SOURCE_SHA) {
  return verifyCargoArtifactManifest({
    exactSha,
    exactTree: SOURCE_TREE,
    manifest,
    rustTarget: RUST_TARGET,
    workspaceRoot: input.root,
  });
}

test("binds each requested executable to its exact Windows Cargo graph", () => {
  const { input, manifest } = build();

  assert.equal(manifest.schema, "codestory.cargo-build-artifacts/v2");
  assert.deepEqual(manifest.source, {
    commit: SOURCE_SHA,
    tree: SOURCE_TREE,
  });
  assert.equal(manifest.build.profile, "release");
  assert.equal(manifest.build.rust_target, RUST_TARGET);
  for (const artifact of input.artifacts) {
    const selected = manifest.artifacts[artifact.alias];
    assert.equal(selected.path, path.resolve(artifact.executable));
    assert.equal(selected.bytes, Buffer.byteLength(artifact.contents));
    assert.equal(selected.sha256, sha256(artifact.contents));
    assert.equal(selected.profile.test, false);
    assert.equal(selected.native_links.count, 1);
    assert.deepEqual(selected.native_links.paths, [selected.relative_path]);
    assert.match(selected.native_links.device, /^(?:0|[1-9][0-9]*)$/u);
    assert.match(selected.native_links.inode, /^[1-9][0-9]*$/u);
  }

  assert.deepEqual(
    verify(input, manifest),
    Object.fromEntries(
      input.artifacts.map((artifact) => [
        artifact.alias,
        path.resolve(artifact.executable),
      ]),
    ),
  );
});

test("accepts and records Cargo's release-root hardlink to release/deps", () => {
  const input = fixture();
  const peer = addCargoBinPeer(input, "cli");
  const { manifest } = build(input);
  const selected = manifest.artifacts.cli;

  assert.equal(selected.native_links.count, 2);
  assert.deepEqual(selected.native_links.paths, [
    "codestory-cli.exe",
    "deps/codestory_cli.exe",
  ]);
  const rootIdentity = fs.lstatSync(input.artifacts[0].executable, { bigint: true });
  const peerIdentity = fs.lstatSync(peer, { bigint: true });
  assert.equal(rootIdentity.dev, peerIdentity.dev);
  assert.equal(rootIdentity.ino, peerIdentity.ino);
  assert.equal(rootIdentity.nlink, 2n);
  assert.equal(selected.native_links.device, rootIdentity.dev.toString());
  assert.equal(selected.native_links.inode, rootIdentity.ino.toString());

  assert.equal(verify(input, manifest).cli, path.resolve(input.artifacts[0].executable));
});

test("accepts Cargo's copied release-root fallback when native hardlinking is unavailable", () => {
  const { input, manifest } = build();

  assert.equal(manifest.artifacts.cli.native_links.count, 1);
  assert.deepEqual(manifest.artifacts.cli.native_links.paths, [
    "codestory-cli.exe",
  ]);
  assert.doesNotThrow(() => verify(input, manifest));
});

test("accepts the release graph without the optional qualification driver", () => {
  const input = fixture({ includeQualificationDriver: false });
  const { manifest } = build(input);

  assert.deepEqual(
    Object.keys(manifest.artifacts).sort(),
    ["cli", "runtime"],
  );
});

test("selects the private driver from a separate Cargo graph", () => {
  const input = fixture();
  const driverMessage = input.qualificationMessages[0];

  const manifest = buildCargoArtifactManifest({
    exactSha: SOURCE_SHA,
    exactTree: SOURCE_TREE,
    expectations: input.expectations,
    jsonLines: input.jsonLines,
    qualificationJsonLines: input.qualificationJsonLines,
    rustTarget: RUST_TARGET,
    targetDir: input.targetDir,
    workspaceRoot: input.root,
  });
  assert.equal(
    manifest.artifacts.qualification_driver.path,
    path.resolve(driverMessage.executable),
  );
  assert.doesNotThrow(() => verify(input, manifest));
});

test("Windows selector binds production and private Cargo receipts independently", () => {
  const input = fixture();
  const productionFile = path.join(input.root, "production.jsonl");
  const driverFile = path.join(input.root, "qualification.jsonl");
  const manifestFile = path.join(input.root, "manifest.json");
  fs.writeFileSync(productionFile, input.jsonLines);
  fs.writeFileSync(driverFile, input.qualificationJsonLines);
  const args = [
    SCRIPT,
    "select",
    "--input", productionFile,
    "--qualification-input", driverFile,
    "--out", manifestFile,
    "--target-dir", input.targetDir,
    "--workspace-root", input.root,
    "--rust-target", RUST_TARGET,
    "--source-sha", SOURCE_SHA,
    "--source-tree", SOURCE_TREE,
    ...input.expectations.flatMap((expectation) => ["--expect", expectation]),
  ];
  const selected = spawnSync(process.execPath, args, { encoding: "utf8" });
  assert.equal(selected.status, 0, selected.stderr);
  const manifest = JSON.parse(fs.readFileSync(manifestFile, "utf8"));
  assert.deepEqual(Object.keys(manifest.artifacts).sort(), [
    "cli", "qualification_driver", "runtime",
  ]);

  fs.rmSync(manifestFile);
  const wrongStream = spawnSync(
    process.execPath,
    args.map((arg) => arg === driverFile ? productionFile : arg),
    { encoding: "utf8" },
  );
  assert.equal(wrongStream.status, 1);
  assert.match(wrongStream.stderr, /qualification Cargo graph emitted a production binary/u);
});

test("refuses qualification feature unification in the shipping stream", () => {
  for (const packageName of ["codestory-cli", "codestory-runtime"]) {
    const input = fixture();
    const message = packageName === "codestory-cli"
      ? input.messages.find((entry) => entry.target?.name === "codestory-cli")
      : featureMessage(input, packageName);
    message.features = ["proof-qualification-support"];
    refreshCargoJson(input);
    assert.throws(
      () => build(input),
      new RegExp(`forbidden feature ${packageName}/proof-qualification-support`, "u"),
    );
  }
});

test("rejects mixed or unverified qualification artifact streams", () => {
  const mixed = fixture();
  mixed.messages.splice(-1, 0, mixed.qualificationMessages[0]);
  refreshCargoJson(mixed);
  assert.throws(
    () => build(mixed),
    /shipping Cargo graph included codestory-bench/u,
  );

  const unfinished = fixture();
  unfinished.qualificationMessages.at(-1).success = false;
  refreshCargoJson(unfinished);
  assert.throws(
    () => build(unfinished),
    /qualification Cargo message stream did not finish successfully/u,
  );

  const testTarget = fixture();
  testTarget.qualificationMessages[0].profile.test = true;
  refreshCargoJson(testTarget);
  assert.throws(
    () => build(testTarget),
    /qualification Cargo graph emitted a test or benchmark target/u,
  );

  const missing = fixture();
  missing.qualificationJsonLines = undefined;
  assert.throws(
    () => build(missing),
    /qualification driver requires its separate Cargo message stream/u,
  );
});

test("does not confuse a binary target's test capability with its active profile", () => {
  const input = fixture({ binaryTargetTest: false });
  const { manifest } = build(input);

  assert.equal(input.messages[1].target.test, false);
  assert.equal(manifest.artifacts.cli.profile.test, false);
});

test("accepts an isolated shipping feature graph", () => {
  const input = fixture();

  assert.doesNotThrow(() =>
    assertShippingFeatureContract({
      jsonLines: input.jsonLines,
      workspaceRoot: input.root,
    })
  );
});

test("rejects retrieval benchmark or test support in the shipping graph", async (t) => {
  // `benchmark-support` exposes the vector-backend bake-off measurement
  // surface. It reaches the shipping graph only through a feature-unification
  // mistake, which is exactly the mistake this contract exists to catch.
  for (const feature of ["benchmark-support", "test-support"]) {
    await t.test(feature, () => {
      const input = fixture();
      featureMessage(input, "codestory-retrieval").features = [feature];
      refreshCargoJson(input);

      assert.throws(
        () =>
          assertShippingFeatureContract({
            jsonLines: input.jsonLines,
            workspaceRoot: input.root,
          }),
        new RegExp(`forbidden feature codestory-retrieval/${feature}`, "u"),
      );
    });
  }
});

test("rejects agent test support in the shipping graph", () => {
  // `codestory-agent/test-support` compiles the eval/holdout probe hooks that used to ride
  // `#[cfg(test)]` inside codestory-runtime. Product builds never enable it, and the gate is
  // what makes "never" checkable rather than asserted (#1673).
  const input = fixture();
  featureMessage(input, "codestory-agent").features = ["test-support"];
  refreshCargoJson(input);

  assert.throws(
    () =>
      assertShippingFeatureContract({
        jsonLines: input.jsonLines,
        workspaceRoot: input.root,
      }),
    /forbidden feature codestory-agent\/test-support/u,
  );
});

test("refuses a shipping graph that never mentions a gated crate", () => {
  // Absence is the failure the gate has to notice: a crate that vanishes from the graph produces
  // no feature evidence, and "no evidence of the feature" must not read as "feature off".
  const input = fixture();
  const agent = featureMessage(input, "codestory-agent");
  input.messages.splice(input.messages.indexOf(agent), 1);
  refreshCargoJson(input);

  assert.throws(
    () =>
      assertShippingFeatureContract({
        jsonLines: input.jsonLines,
        workspaceRoot: input.root,
      }),
    /shipping Cargo graph omitted feature evidence for codestory-agent/u,
  );
});

test("rejects runtime benchmark or test support in the shipping graph", async (t) => {
  for (const feature of ["benchmark-support", "test-support"]) {
    await t.test(feature, () => {
      const input = fixture();
      featureMessage(input, "codestory-runtime").features = [feature];
      refreshCargoJson(input);

      assert.throws(
        () =>
          assertShippingFeatureContract({
            jsonLines: input.jsonLines,
            workspaceRoot: input.root,
          }),
        new RegExp(`forbidden feature codestory-runtime/${feature}`, "u"),
      );
    });
  }
});

test("rejects any test or benchmark target mixed into the shipping build", () => {
  const input = fixture();
  input.messages.splice(-1, 0, {
    reason: "compiler-artifact",
    package_id: "registry+https://example.invalid/index#probe@1.0.0",
    target: {
      kind: ["test"],
      name: "probe",
    },
    profile: {
      test: true,
    },
    features: [],
  });
  refreshCargoJson(input);

  assert.throws(
    () =>
      assertShippingFeatureContract({
        jsonLines: input.jsonLines,
        workspaceRoot: input.root,
      }),
    /shipping Cargo graph emitted a test or benchmark target: probe:probe/u,
  );
});

test("requires one successful Cargo build completion", () => {
  const input = fixture();
  input.messages.at(-1).success = false;
  refreshCargoJson(input);

  assert.throws(
    () =>
      assertShippingFeatureContract({
        jsonLines: input.jsonLines,
        workspaceRoot: input.root,
      }),
    /shipping Cargo message stream did not finish successfully/u,
  );
});

test("exposes the feature contract through the command-line helper", () => {
  const input = fixture();
  const jsonFile = path.join(input.root, "cargo.jsonl");
  fs.writeFileSync(jsonFile, input.jsonLines);

  const accepted = spawnSync(
    process.execPath,
    [
      SCRIPT,
      "features",
      "--input",
      jsonFile,
      "--workspace-root",
      input.root,
    ],
    { encoding: "utf8" },
  );
  assert.equal(accepted.status, 0, accepted.stderr);

  featureMessage(input, "codestory-runtime").features = ["benchmark-support"];
  refreshCargoJson(input);
  fs.writeFileSync(jsonFile, input.jsonLines);
  const rejected = spawnSync(
    process.execPath,
    [
      SCRIPT,
      "features",
      "--input",
      jsonFile,
      "--workspace-root",
      input.root,
    ],
    { encoding: "utf8" },
  );
  assert.equal(rejected.status, 1);
  assert.match(
    rejected.stderr,
    /forbidden feature codestory-runtime\/benchmark-support/u,
  );
});

test("rejects duplicate compiler artifacts instead of choosing one by path order", () => {
  const input = fixture();
  input.jsonLines = [...input.messages, input.messages[1]]
    .map((message) => JSON.stringify(message))
    .join("\n");

  assert.throws(
    () => build(input),
    /must emit exactly one production binary codestory-cli:codestory-cli; found 2/u,
  );
});

test("rejects a fresh Cargo artifact from a prior build invocation", () => {
  const input = fixture();
  input.messages[1].fresh = true;
  input.jsonLines = input.messages.map((message) => JSON.stringify(message)).join("\n");

  assert.throws(
    () => build(input),
    /not produced by the exact build invocation/u,
  );
});

test("rejects debug-profile output even when the target name matches", () => {
  const input = fixture();
  input.messages[1].profile.debug_assertions = true;
  input.messages[1].profile.opt_level = "0";
  input.jsonLines = input.messages.map((message) => JSON.stringify(message)).join("\n");

  assert.throws(() => build(input), /not built with the release profile/u);
});

test("rejects a production binary actually built with the test profile", () => {
  const input = fixture();
  input.messages[1].profile.test = true;
  refreshCargoJson(input);

  assert.throws(
    () => build(input),
    /shipping Cargo graph emitted a test or benchmark target/u,
  );
});

test("rejects an expanded Cargo target kind instead of accepting a partial match", () => {
  const input = fixture();
  input.messages[1].target.kind = ["bin", "test"];
  refreshCargoJson(input);

  assert.throws(
    () => build(input),
    /shipping Cargo graph emitted a test or benchmark target/u,
  );
});

test("rejects a renamed Cargo target instead of substituting another binary", () => {
  const input = fixture();
  input.messages[1].target.name = "codestory-cli-shadow";
  refreshCargoJson(input);

  assert.throws(
    () => build(input),
    /must emit exactly one production binary codestory-cli:codestory-cli; found 0/u,
  );
});

test("rejects an executable emitted outside the exact target release directory", () => {
  const input = fixture();
  const stale = path.join(input.root, "debug", "codestory-cli.exe");
  fs.mkdirSync(path.dirname(stale), { recursive: true });
  fs.writeFileSync(stale, "stale debug cli");
  input.messages[1].executable = stale;
  input.messages[1].filenames = [stale];
  input.jsonLines = input.messages.map((message) => JSON.stringify(message)).join("\n");

  assert.throws(
    () => build(input),
    /escaped the exact target release directory/u,
  );
});

test("rejects a release-path executable hardlinked to another build graph", () => {
  const input = fixture();
  const releaseCli = input.artifacts[0].executable;
  const debugCli = path.join(input.targetDir, RUST_TARGET, "debug", "codestory-cli.exe");
  fs.mkdirSync(path.dirname(debugCli), { recursive: true });
  fs.writeFileSync(debugCli, input.artifacts[0].contents);
  fs.unlinkSync(releaseCli);
  fs.linkSync(debugCli, releaseCli);

  assert.throws(
    () => build(input),
    /not exactly the release-root executable and one release\/deps peer/u,
  );
});

test("rejects a release-root executable hardlinked outside the release graph", () => {
  const input = fixture();
  const external = path.join(input.root, "foreign", "codestory-cli.exe");
  fs.mkdirSync(path.dirname(external), { recursive: true });
  fs.linkSync(input.artifacts[0].executable, external);

  assert.throws(
    () => build(input),
    /not exactly the release-root executable and one release\/deps peer/u,
  );
});

test("rejects a release-root executable hardlinked into another target graph", () => {
  const input = fixture();
  const otherTarget = path.join(
    input.targetDir,
    "aarch64-pc-windows-msvc",
    "release",
    "deps",
    "codestory-cli.exe",
  );
  fs.mkdirSync(path.dirname(otherTarget), { recursive: true });
  fs.linkSync(input.artifacts[0].executable, otherTarget);

  assert.throws(
    () => build(input),
    /not exactly the release-root executable and one release\/deps peer/u,
  );
});

test("rejects a non-executable hardlink posing as Cargo's release/deps peer", () => {
  const input = fixture();
  addCargoBinPeer(input, "cli", "codestory-cli.pdb");

  assert.throws(
    () => build(input),
    /not exactly the release-root executable and one release\/deps peer/u,
  );
});

test("rejects an arbitrary executable name posing as Cargo's release/deps peer", () => {
  const input = fixture();
  addCargoBinPeer(input, "cli", "evil.exe");

  assert.throws(
    () => build(input),
    /not exactly the release-root executable and one release\/deps peer/u,
  );
});

test("rejects the release-root spelling where Cargo uses a normalized deps peer", () => {
  const input = fixture();
  addCargoBinPeer(input, "cli", "codestory-cli.exe");

  assert.throws(
    () => build(input),
    /not exactly the release-root executable and one release\/deps peer/u,
  );
});

test("rejects bytes changed after Cargo emitted the authenticated executable", () => {
  const { input, manifest } = build();
  fs.appendFileSync(input.artifacts[0].executable, "mutated");

  assert.throws(
    () =>
      verifyCargoArtifactManifest({
        exactSha: SOURCE_SHA,
        exactTree: SOURCE_TREE,
        manifest,
        rustTarget: RUST_TARGET,
        workspaceRoot: input.root,
      }),
    /no longer matches its authenticated build output/u,
  );
});

test("rejects a hardlink added after Cargo artifact selection", () => {
  const { input, manifest } = build();
  const alias = path.join(input.root, "cross-graph-cli.exe");
  fs.linkSync(input.artifacts[0].executable, alias);

  assert.throws(
    () =>
      verify(input, manifest),
    /not exactly the release-root executable and one release\/deps peer/u,
  );
});

test("rejects a third hardlink added after selecting Cargo's root and deps pair", () => {
  const input = fixture();
  addCargoBinPeer(input, "cli");
  const { manifest } = build(input);
  fs.linkSync(
    input.artifacts[0].executable,
    path.join(input.root, "cross-graph-cli.exe"),
  );

  assert.throws(
    () => verify(input, manifest),
    /unsupported native hardlink count 3/u,
  );
});

test("rejects an identical-byte replacement of Cargo's recorded deps peer", () => {
  const input = fixture();
  const peer = addCargoBinPeer(input, "cli");
  const { manifest } = build(input);
  fs.unlinkSync(peer);
  fs.writeFileSync(peer, input.artifacts[0].contents);

  assert.throws(
    () => verify(input, manifest),
    /no longer matches its authenticated native links/u,
  );
});

test("rejects an identical-byte replacement of the selected release-root executable", () => {
  const input = fixture();
  addCargoBinPeer(input, "cli");
  const { manifest } = build(input);
  fs.unlinkSync(input.artifacts[0].executable);
  fs.writeFileSync(input.artifacts[0].executable, input.artifacts[0].contents);

  assert.throws(
    () => verify(input, manifest),
    /no longer matches its authenticated native links/u,
  );
});

test("rejects an identical-byte replacement of both recorded hardlink paths", () => {
  const input = fixture();
  const selected = input.artifacts[0].executable;
  const peer = addCargoBinPeer(input, "cli");
  const { manifest } = build(input);
  const replacementRoot = path.join(input.releaseDir, "replacement.exe");
  const replacementPeer = path.join(input.releaseDir, "deps", "replacement.exe");
  fs.writeFileSync(replacementRoot, input.artifacts[0].contents);
  fs.linkSync(replacementRoot, replacementPeer);
  const replacementIdentity = fs.lstatSync(replacementRoot, { bigint: true });
  assert.notEqual(
    replacementIdentity.ino.toString(),
    manifest.artifacts.cli.native_links.inode,
  );
  fs.unlinkSync(selected);
  fs.unlinkSync(peer);
  fs.renameSync(replacementRoot, selected);
  fs.renameSync(replacementPeer, peer);

  assert.throws(
    () => verify(input, manifest),
    /no longer matches its authenticated native links/u,
  );
});

test("rejects manifest native-link topology changed from deps to another graph", () => {
  const input = fixture();
  addCargoBinPeer(input, "cli");
  const { manifest } = build(input);
  manifest.artifacts.cli.native_links.paths[1] = "../debug/codestory-cli.exe";

  assert.throws(
    () => verify(input, manifest),
    /native links cli values changed/u,
  );
});

test("rejects a manifest from another exact source SHA", () => {
  const { input, manifest } = build();

  assert.throws(
    () =>
      verifyCargoArtifactManifest({
        exactSha: "c".repeat(40),
        exactTree: SOURCE_TREE,
        manifest,
        rustTarget: RUST_TARGET,
        workspaceRoot: input.root,
      }),
    /source identity does not match the exact checkout/u,
  );
});

test("rejects a manifest that drops one required release-graph artifact", () => {
  const { input, manifest } = build();
  delete manifest.artifacts.cli;

  assert.throws(
    () =>
      verifyCargoArtifactManifest({
        exactSha: SOURCE_SHA,
        exactTree: SOURCE_TREE,
        manifest,
        rustTarget: RUST_TARGET,
        workspaceRoot: input.root,
      }),
    /artifact manifest release graph changed/u,
  );
});

test("rejects malformed Cargo JSON rather than silently dropping an artifact line", () => {
  const input = fixture();
  input.jsonLines = `${input.jsonLines}\nnot-json`;

  assert.throws(
    () => build(input),
    /Cargo JSON output line .* is not valid JSON/u,
  );
});
