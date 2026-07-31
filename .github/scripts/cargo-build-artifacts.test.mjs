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
  for (const packageName of ["codestory-retrieval", "codestory-runtime"]) {
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

  return {
    artifacts,
    expectations: artifacts.map(
      ({ alias, packageName, kind, targetName }) =>
        `${alias}=${packageName}:${kind}:${targetName}`,
    ),
    jsonLines: messages.map((message) => JSON.stringify(message)).join("\n"),
    messages,
    releaseDir,
    root,
    targetDir,
  };
}

function refreshCargoJson(input) {
  input.jsonLines = input.messages
    .map((message) => JSON.stringify(message))
    .join("\n");
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

test("binds each requested executable to the exact Windows release graph", () => {
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

test("rejects retrieval test support in the shipping graph", () => {
  const input = fixture();
  featureMessage(input, "codestory-retrieval").features = ["test-support"];
  refreshCargoJson(input);

  assert.throws(
    () =>
      assertShippingFeatureContract({
        jsonLines: input.jsonLines,
        workspaceRoot: input.root,
      }),
    /forbidden feature codestory-retrieval\/test-support/u,
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
