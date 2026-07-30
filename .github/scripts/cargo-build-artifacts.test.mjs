import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  buildCargoArtifactManifest,
  verifyCargoArtifactManifest,
} from "./cargo-build-artifacts.mjs";

const SOURCE_SHA = "a".repeat(40);
const SOURCE_TREE = "b".repeat(40);
const RUST_TARGET = "x86_64-pc-windows-msvc";

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
    {
      alias: "native_staging",
      packageName: "codestory-llama-sys",
      kind: "test",
      targetName: "native_staging",
      executable: path.join(depsDir, "native_staging-123456.exe"),
      contents: "native staging",
    },
    {
      alias: "windows_path_identity",
      packageName: "codestory-workspace",
      kind: "test",
      targetName: "windows_path_identity",
      executable: path.join(depsDir, "windows_path_identity-123456.exe"),
      contents: "path identity",
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
    const manifest = path.join(root, "crates", artifact.packageName, "Cargo.toml");
    if (!fs.existsSync(manifest)) {
      fs.mkdirSync(path.dirname(manifest), { recursive: true });
      fs.writeFileSync(
        manifest,
        `[package]\nname = "${artifact.packageName}"\nversion = "0.16.3"\n`,
      );
    }
    artifact.manifest = manifest;
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
      test: artifact.kind === "test" ? true : binaryTargetTest,
    },
    profile: {
      opt_level: "3",
      debuginfo: 0,
      debug_assertions: false,
      overflow_checks: false,
      test: artifact.kind === "test",
    },
    features: [],
    filenames: [artifact.executable],
    executable: artifact.executable,
    fresh: false,
  }));
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

test("binds each requested executable to the exact Windows release graph", () => {
  const { input, manifest } = build();

  assert.equal(manifest.schema, "codestory.cargo-build-artifacts/v1");
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
    assert.equal(selected.profile.test, artifact.kind === "test");
  }

  assert.deepEqual(
    verifyCargoArtifactManifest({
      exactSha: SOURCE_SHA,
      exactTree: SOURCE_TREE,
      manifest,
      rustTarget: RUST_TARGET,
      workspaceRoot: input.root,
    }),
    Object.fromEntries(
      input.artifacts.map((artifact) => [
        artifact.alias,
        path.resolve(artifact.executable),
      ]),
    ),
  );
});

test("accepts the release graph without the optional qualification driver", () => {
  const input = fixture({ includeQualificationDriver: false });
  const { manifest } = build(input);

  assert.deepEqual(
    Object.keys(manifest.artifacts).sort(),
    ["cli", "native_staging", "runtime", "windows_path_identity"],
  );
});

test("does not confuse a binary target's test capability with its active profile", () => {
  const input = fixture({ binaryTargetTest: false });
  const { manifest } = build(input);

  assert.equal(input.messages[1].target.test, false);
  assert.equal(manifest.artifacts.cli.profile.test, false);
});

test("rejects duplicate compiler artifacts instead of choosing one by path order", () => {
  const input = fixture();
  input.jsonLines = [...input.messages, input.messages[1]]
    .map((message) => JSON.stringify(message))
    .join("\n");

  assert.throws(
    () => build(input),
    /expected exactly one Cargo artifact for cli, found 2/u,
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
  input.jsonLines = input.messages.map((message) => JSON.stringify(message)).join("\n");

  assert.throws(() => build(input), /profile\.test did not match bin/u);
});

test("rejects an expanded Cargo target kind instead of accepting a partial match", () => {
  const input = fixture();
  input.messages[1].target.kind = ["bin", "test"];
  input.jsonLines = input.messages.map((message) => JSON.stringify(message)).join("\n");

  assert.throws(
    () => build(input),
    /Cargo artifact target contract changed for cli/u,
  );
});

test("rejects a renamed Cargo target instead of substituting another binary", () => {
  const input = fixture();
  input.messages[1].target.name = "codestory-cli-shadow";
  input.jsonLines = input.messages.map((message) => JSON.stringify(message)).join("\n");

  assert.throws(
    () => build(input),
    /expected exactly one Cargo artifact for cli, found 0/u,
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
    /regular, non-symlink, singly linked file/u,
  );
});

test("rejects a mixed test and production profile", () => {
  const input = fixture();
  const native = input.messages.find(
    (message) => message.target?.name === "native_staging",
  );
  native.profile.test = false;
  input.jsonLines = input.messages.map((message) => JSON.stringify(message)).join("\n");

  assert.throws(
    () => build(input),
    /profile\.test did not match test/u,
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
      verifyCargoArtifactManifest({
        exactSha: SOURCE_SHA,
        exactTree: SOURCE_TREE,
        manifest,
        rustTarget: RUST_TARGET,
        workspaceRoot: input.root,
      }),
    /regular, non-symlink, singly linked file/u,
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
