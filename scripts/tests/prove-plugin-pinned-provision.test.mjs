import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

import { requirePinnedArchiveDigest } from "../lib/pinned-archive-digests.mjs";
import {
  assertProvisionedArchiveDigest,
  parseProvisionProofArguments,
} from "../lib/provision-proof-lanes.mjs";
import {
  RELEASE_MANIFEST_ASSET,
  buildReleaseManifest,
  parseReleaseManifest,
  requireManifestArchiveDigest,
} from "../lib/release-manifest.mjs";
import { buildReleaseManifestFromAssets, parseBuildArguments } from "../build-release-manifest.mjs";

const scriptsDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repositoryRoot = path.resolve(scriptsDir, "..");
const proofScript = path.join(scriptsDir, "prove-plugin-pinned-provision.mjs");
const waitModuleUrl = pathToFileURL(path.join(scriptsDir, "lib/wait-for-managed-runtime.mjs")).href;

// The wait must bound itself even when the launcher keeps a readable non-managed metadata file
// on disk, so drive it in its own process and kill it if it outlives the bound. A hang here is a
// reported failure, not a wedged test run.
const KILL_AFTER_MS = 5_000;
const VALID_DIGEST = "a".repeat(64);
const COMMIT = "9".repeat(40);

function sourcePin(overrides = {}) {
  return { schema_version: 1, cli_version: "1.2.3", release_tag: "v1.2.3", ...overrides };
}

function repositoryPin() {
  return JSON.parse(
    fs.readFileSync(path.join(repositoryRoot, "plugins/codestory/cli-version.json"), "utf8"),
  );
}

function manifestFor(version, entries = {}) {
  const archives = {
    "macos-arm64": {
      filename: `codestory-cli-v${version}-macos-arm64.tar.gz`,
      bytes: 111,
      sha256: "1".repeat(64),
    },
    "linux-x64": {
      filename: `codestory-cli-v${version}-linux-x64.tar.gz`,
      bytes: 222,
      sha256: "2".repeat(64),
    },
    "windows-x64": {
      filename: `codestory-cli-v${version}-windows-x64.zip`,
      bytes: 333,
      sha256: "3".repeat(64),
    },
  };
  for (const [target, entry] of Object.entries(entries)) {
    archives[target] = { ...archives[target], ...entry };
  }
  return buildReleaseManifest({ version, tag: `v${version}`, commit: COMMIT, archives });
}

test("the provision gate requires a source-pinned digest", () => {
  assert.throws(
    () =>
      requirePinnedArchiveDigest({
        pin: sourcePin(),
        target: "macos-arm64",
        observedDigest: VALID_DIGEST,
        lane: "plugin",
      }),
    /no valid macos-arm64 archive digest/u,
  );
  assert.throws(
    () =>
      requirePinnedArchiveDigest({
        pin: sourcePin({ archives: { "macos-arm64": "not-a-digest" } }),
        target: "macos-arm64",
        observedDigest: VALID_DIGEST,
        lane: "plugin",
      }),
    /no valid macos-arm64 archive digest/u,
  );
});

test("the provision gate rejects a hostile archive digest", () => {
  assert.throws(
    () =>
      requirePinnedArchiveDigest({
        pin: sourcePin({ archives: { "linux-x64": VALID_DIGEST } }),
        target: "linux-x64",
        observedDigest: "b".repeat(64),
        lane: "plugin",
      }),
    /does not match the pin's linux-x64 digest/u,
  );
});

test("the provision gate accepts the exact source-pinned digest", () => {
  assert.equal(
    requirePinnedArchiveDigest({
      pin: sourcePin({ archives: { "windows-x64": VALID_DIGEST } }),
      target: "windows-x64",
      observedDigest: VALID_DIGEST,
      lane: "plugin",
    }),
    VALID_DIGEST,
  );
});

// The lane is a required argument with no default, so a caller that omits it or misspells it is
// refused rather than quietly granted the plugin lane's authority over a native pin.
test("only the plugin lane may assert a source-pinned archive digest", () => {
  const pin = sourcePin({ archives: { "linux-x64": VALID_DIGEST } });
  for (const lane of ["native", undefined, "", "Plugin"]) {
    assert.throws(
      () => requirePinnedArchiveDigest({ pin, target: "linux-x64", observedDigest: VALID_DIGEST, lane }),
      /only the plugin lane may assert a source-pinned archive digest/u,
      `lane ${JSON.stringify(lane)} reached the source-pin assertion`,
    );
  }
});

// The freeze-critical case, held against the pin this repository actually carries rather than a
// hand-made one. The frozen native head's pin is lawfully archive-less -- bump-version.mjs deletes
// `archives` on a native bump because the archives are built FROM this tree -- and the proof used
// to run the source-pin assertion on it unconditionally, which can only fail.
test("the frozen source carries no circular native archive digest", () => {
  const pin = repositoryPin();
  assert.equal(
    Object.hasOwn(pin, "archives"),
    false,
    "a native pin naming digests of archives built from its own tree is the circularity REL-MAN removes",
  );
});

test("a lawful archive-less native pin fails the plugin lane and passes the native one", () => {
  const pin = repositoryPin();
  const target = "linux-x64";
  const provisioned = { sha256: VALID_DIGEST, bytes: 4096 };

  // What the frozen head hit before the split, reproduced on the real pin.
  assert.throws(
    () => requirePinnedArchiveDigest({ pin, target, observedDigest: provisioned.sha256, lane: "plugin" }),
    /no valid linux-x64 archive digest/u,
  );

  // The native lane never reaches that assertion. Without a staged manifest it records an explicit
  // deferral instead of claiming a digest it did not prove.
  const deferred = assertProvisionedArchiveDigest({
    lane: "native",
    pin,
    target,
    provisioned,
    deferArchiveDigest: true,
  });
  assert.equal(deferred.asserted, false);
  assert.equal(deferred.authority, "deferred");
  assert.match(deferred.claim, /NOT proven here/u);

  // With the staged manifest it proves the digest the release itself owns.
  const manifest = manifestFor(pin.cli_version, {
    [target]: { bytes: provisioned.bytes, sha256: provisioned.sha256 },
  });
  const proven = assertProvisionedArchiveDigest({
    lane: "native",
    pin,
    target,
    provisioned,
    releaseManifest: manifest,
  });
  assert.equal(proven.asserted, true);
  assert.equal(proven.authority, "release_manifest");

  // The dispatcher must be doing the assertion, not reporting that one happened. A native run
  // whose provisioned archive disagrees with the staged manifest is refused here, in the same
  // call that reported success a moment ago.
  for (const hostile of [
    { sha256: "f".repeat(64), bytes: provisioned.bytes },
    { sha256: provisioned.sha256, bytes: provisioned.bytes + 1 },
  ]) {
    assert.throws(
      () =>
        assertProvisionedArchiveDigest({
          lane: "native",
          pin,
          target,
          provisioned: hostile,
          releaseManifest: manifest,
        }),
      /release manifest linux-x64 (?:digest|length)/u,
      JSON.stringify(hostile),
    );
  }
  // And the plugin lane's own assertion is real: a pin that does carry a digest still refuses a
  // provision that does not match it.
  assert.throws(
    () =>
      assertProvisionedArchiveDigest({
        lane: "plugin",
        pin: { ...pin, archives: { [target]: VALID_DIGEST } },
        target,
        provisioned: { sha256: "f".repeat(64), bytes: 4096 },
      }),
    /does not match the pin's linux-x64 digest/u,
  );
});

// The dispatcher is the single reader of the source-pin assertion. If the proof could import it
// directly, a native branch could reacquire the plugin lane's authority without any lane check.
test("the proof reaches the source-pin assertion only through the lane dispatcher", () => {
  const source = fs.readFileSync(proofScript, "utf8");
  assert.equal(
    source.includes("pinned-archive-digests.mjs"),
    false,
    "prove-plugin-pinned-provision.mjs must not import the source-pin module directly",
  );
  assert.equal(source.includes("requirePinnedArchiveDigest"), false);
  assert.equal(source.includes("provision-proof-lanes.mjs"), true);
});

test("the plugin lane refuses the native lane's digest authorities", () => {
  const plugin = parseProvisionProofArguments([]);
  assert.deepEqual(plugin, {
    lane: "plugin",
    timeoutMs: 600_000,
    expectBuildSource: "github_release",
    releaseManifestPath: null,
    deferArchiveDigest: false,
  });
  for (const argv of [
    ["--release-manifest", "manifest.json"],
    ["--defer-archive-digest"],
    ["--expect-build-source", "explicit_package"],
  ]) {
    assert.throws(
      () => parseProvisionProofArguments(argv),
      /belongs to the native lane/u,
      argv.join(" "),
    );
  }
  assert.throws(
    () =>
      assertProvisionedArchiveDigest({
        lane: "plugin",
        pin: sourcePin({ archives: { "linux-x64": VALID_DIGEST } }),
        target: "linux-x64",
        provisioned: { sha256: VALID_DIGEST, bytes: 10 },
        deferArchiveDigest: true,
      }),
    /proves its source pin and nothing else/u,
  );
});

test("the native lane refuses to run without a stated digest authority", () => {
  assert.throws(
    () => parseProvisionProofArguments(["--lane", "native", "--expect-build-source", "explicit_package"]),
    /needs either --release-manifest PATH or an explicit --defer-archive-digest/u,
  );
  assert.throws(
    () =>
      parseProvisionProofArguments([
        "--lane",
        "native",
        "--expect-build-source",
        "explicit_package",
        "--release-manifest",
        "m.json",
        "--defer-archive-digest",
      ]),
    /mutually exclusive/u,
  );
  assert.throws(
    () => parseProvisionProofArguments(["--lane", "native", "--defer-archive-digest"]),
    /needs --expect-build-source github_release\|explicit_package/u,
  );
  assert.throws(
    () =>
      parseProvisionProofArguments([
        "--lane",
        "native",
        "--expect-build-source",
        "candidate_archive",
        "--defer-archive-digest",
      ]),
    /needs --expect-build-source github_release\|explicit_package/u,
  );
  assert.deepEqual(
    parseProvisionProofArguments([
      "--lane",
      "native",
      "--expect-build-source",
      "explicit_package",
      "--release-manifest",
      "staged/manifest.json",
    ]),
    {
      lane: "native",
      timeoutMs: 600_000,
      expectBuildSource: "explicit_package",
      releaseManifestPath: "staged/manifest.json",
      deferArchiveDigest: false,
    },
  );
});

// A misspelled or duplicated flag used to be ignored, which turns `--lane natvie` into a silent
// plugin-lane run against a native pin -- the exact failure this package removes.
test("the proof refuses unknown, repeated, and malformed arguments", () => {
  assert.throws(() => parseProvisionProofArguments(["--lane", "natvie"]), /--lane must be one of/u);
  assert.throws(() => parseProvisionProofArguments(["--wat", "1"]), /unknown argument --wat/u);
  assert.throws(() => parseProvisionProofArguments(["positional"]), /unexpected argument/u);
  assert.throws(
    () => parseProvisionProofArguments(["--timeout-ms", "1", "--timeout-ms", "2"]),
    /--timeout-ms was given more than once/u,
  );
  assert.throws(
    () => parseProvisionProofArguments(["--timeout-ms"]),
    /--timeout-ms needs a positive number/u,
  );
});

test("the release manifest binds its digests to one release identity", () => {
  const manifest = manifestFor("1.2.3");
  const target = "linux-x64";
  const provisioned = { sha256: "2".repeat(64), bytes: 222 };
  assert.deepEqual(
    requireManifestArchiveDigest(manifest, { version: "1.2.3", tag: "v1.2.3" }, target, provisioned),
    { filename: "codestory-cli-v1.2.3-linux-x64.tar.gz", bytes: 222, sha256: "2".repeat(64) },
  );
  // A perfectly valid manifest for a DIFFERENT release describes intact bytes of the wrong
  // release. Without the identity binding it would satisfy this assertion.
  assert.throws(
    () =>
      requireManifestArchiveDigest(
        manifestFor("1.2.4"),
        { version: "1.2.3", tag: "v1.2.3" },
        target,
        provisioned,
      ),
    /describes v1\.2\.4 \(1\.2\.4\), not the v1\.2\.3 \(1\.2\.3\) being provisioned/u,
  );
  assert.throws(
    () =>
      requireManifestArchiveDigest(manifest, { version: "1.2.3", tag: "v1.2.3" }, target, {
        sha256: "4".repeat(64),
        bytes: 222,
      }),
    /linux-x64 digest .* does not match the provisioned archive digest/u,
  );
  // The length is recorded by the release and read here; a manifest field nothing checks cannot
  // fail, so a byte-length drift is a refusal like any other.
  assert.throws(
    () =>
      requireManifestArchiveDigest(manifest, { version: "1.2.3", tag: "v1.2.3" }, target, {
        sha256: "2".repeat(64),
        bytes: 223,
      }),
    /linux-x64 length 222 does not match the provisioned archive length 223/u,
  );
  assert.throws(
    () =>
      requireManifestArchiveDigest(manifest, { version: "1.2.3", tag: "v1.2.3" }, target, {
        sha256: "2".repeat(64),
      }),
    /needs the provisioned linux-x64 archive byte length/u,
  );
  assert.throws(
    () =>
      requireManifestArchiveDigest(manifest, { version: "1.2.3", tag: "v1.2.3" }, "solaris-sparc", provisioned),
    /has no solaris-sparc target/u,
  );
});

test("the release manifest refuses every malformed shape", () => {
  const good = manifestFor("1.2.3");
  for (const [mutate, expected] of [
    [(m) => ({ ...m, domain: "codestory.other" }), /domain must be codestory\.release-manifest/u],
    [(m) => ({ ...m, schema_version: 2 }), /schema_version must be 1/u],
    [(m) => ({ ...m, version: "not-semver" }), /version must be semver/u],
    [(m) => ({ ...m, tag: "1.2.3" }), /tag must be v1\.2\.3/u],
    [(m) => ({ ...m, commit: "ABC" }), /commit must be a 40-character lowercase hexadecimal id/u],
    [
      (m) => ({ ...m, archives: { "linux-x64": m.archives["linux-x64"] } }),
      /archives must name exactly linux-x64, macos-arm64, windows-x64/u,
    ],
    [
      (m) => ({ ...m, archives: { ...m.archives, extra: m.archives["linux-x64"] } }),
      /archives must name exactly/u,
    ],
    [
      (m) => ({
        ...m,
        archives: { ...m.archives, "linux-x64": { ...m.archives["linux-x64"], filename: "other.tar.gz" } },
      }),
      /linux-x64 filename must be codestory-cli-v1\.2\.3-linux-x64\.tar\.gz/u,
    ],
    [
      (m) => ({
        ...m,
        archives: { ...m.archives, "linux-x64": { ...m.archives["linux-x64"], bytes: 0 } },
      }),
      /linux-x64 bytes must be a positive integer/u,
    ],
    [
      (m) => ({
        ...m,
        archives: { ...m.archives, "linux-x64": { ...m.archives["linux-x64"], sha256: "AB" } },
      }),
      /linux-x64 sha256 must be 64 lowercase hexadecimal characters/u,
    ],
  ]) {
    assert.throws(() => parseReleaseManifest(JSON.stringify(mutate(good))), expected);
  }
  assert.throws(() => parseReleaseManifest("{"), /is not valid JSON/u);
});

function stagedAssets(version, { corruptSums = false, omitTarget = null } = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "codestory-release-assets-"));
  const lines = [];
  const expected = {};
  for (const [target, filename] of [
    ["macos-arm64", `codestory-cli-v${version}-macos-arm64.tar.gz`],
    ["linux-x64", `codestory-cli-v${version}-linux-x64.tar.gz`],
    ["windows-x64", `codestory-cli-v${version}-windows-x64.zip`],
  ]) {
    const body = Buffer.from(`${target} archive for ${version}\n`, "utf8");
    if (target !== omitTarget) fs.writeFileSync(path.join(dir, filename), body);
    const digest = createHash("sha256").update(body).digest("hex");
    expected[target] = { filename, bytes: body.length, sha256: digest };
    lines.push(`${corruptSums && target === "linux-x64" ? "0".repeat(64) : digest}  ${filename}`);
  }
  fs.writeFileSync(path.join(dir, "SHA256SUMS.txt"), `${lines.join("\n")}\n`, "utf8");
  return { dir, expected };
}

test("the release manifest is generated from the archives the release built", () => {
  const { dir, expected } = stagedAssets("1.2.3");
  try {
    const manifest = buildReleaseManifestFromAssets({
      version: "1.2.3",
      tag: "v1.2.3",
      commit: COMMIT,
      assets: dir,
    });
    assert.deepEqual(manifest.archives, expected);
    assert.equal(manifest.commit, COMMIT);
    // Round-trips through the reader the launcher and the native proof use.
    assert.deepEqual(parseReleaseManifest(JSON.stringify(manifest)), manifest);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("the generator refuses archives its own checksum file does not describe", () => {
  const drifted = stagedAssets("1.2.3", { corruptSums: true });
  try {
    assert.throws(
      () =>
        buildReleaseManifestFromAssets({
          version: "1.2.3",
          tag: "v1.2.3",
          commit: COMMIT,
          assets: drifted.dir,
        }),
      /the built archives and the release checksum file disagree/u,
    );
  } finally {
    fs.rmSync(drifted.dir, { recursive: true, force: true });
  }
  const incomplete = stagedAssets("1.2.3", { omitTarget: "windows-x64" });
  try {
    assert.throws(
      () =>
        buildReleaseManifestFromAssets({
          version: "1.2.3",
          tag: "v1.2.3",
          commit: COMMIT,
          assets: incomplete.dir,
        }),
      /could not read the built windows-x64 archive/u,
    );
  } finally {
    fs.rmSync(incomplete.dir, { recursive: true, force: true });
  }
  assert.throws(() => parseBuildArguments(["--version", "1.2.3"]), /--tag is required/u);
  assert.throws(() => parseBuildArguments(["--nope", "1"]), /unknown argument --nope/u);
});

test("the release manifest asset name is the one the launcher fetches", () => {
  assert.equal(RELEASE_MANIFEST_ASSET, "codestory-release-manifest.json");
  const launcher = fs.readFileSync(
    path.join(repositoryRoot, "plugins/codestory/scripts/codestory-mcp.cjs"),
    "utf8",
  );
  assert.equal(launcher.includes(`'${RELEASE_MANIFEST_ASSET}'`), true);
});

function runNode(args, options = {}) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, args, { stdio: ["ignore", "pipe", "pipe"], ...options });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    const killer = setTimeout(() => child.kill("SIGKILL"), KILL_AFTER_MS);
    child.on("close", (code, signal) => {
      clearTimeout(killer);
      resolve({ code, signal, stdout, stderr });
    });
  });
}

function driveWait({ runtimeMetadata, timeoutMs, exitCode }) {
  const source = `
    import { waitForManagedRuntime } from ${JSON.stringify(waitModuleUrl)};
    const exitCode = ${JSON.stringify(exitCode)};
    let killed = false;
    const child = { exitCode, kill() { killed = true; } };
    const started = Date.now();
    let outcome;
    try {
      const metadata = await waitForManagedRuntime({
        child,
        runtimeMetadata: ${JSON.stringify(runtimeMetadata)},
        timeoutMs: ${JSON.stringify(timeoutMs)},
        intervalMs: 10,
      });
      outcome = { settled: "resolved", metadata };
    } catch (error) {
      outcome = { settled: "rejected", message: error.message };
    }
    console.log(JSON.stringify({ ...outcome, killed, elapsedMs: Date.now() - started }));
  `;
  return runNode(["--input-type=module", "-e", source]).then((result) => {
    if (result.signal || result.stdout.trim() === "") {
      return { settled: "hung", code: result.code, signal: result.signal, stderr: result.stderr };
    }
    return JSON.parse(result.stdout.trim());
  });
}

function scratchDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "codestory-pin-proof-test-"));
}

function driftedRuntimeMetadata() {
  const runtimeMetadata = path.join(scratchDir(), ".codestory-mcp-runtime.json");
  // What the launcher writes when the pin's archive digest no longer resolves: readable
  // metadata that never reaches the managed source the proof is waiting for.
  fs.writeFileSync(
    runtimeMetadata,
    JSON.stringify({ source: "managed_unavailable", path: null, cliVersion: null }),
  );
  return runtimeMetadata;
}

function managedRuntimeMetadata() {
  const runtimeMetadata = path.join(scratchDir(), ".codestory-mcp-runtime.json");
  fs.writeFileSync(runtimeMetadata, JSON.stringify({ source: "managed", cliVersion: "9.9.9" }));
  return runtimeMetadata;
}

test("pin drift that keeps runtime metadata readable still times out within the bound", async () => {
  const outcome = await driveWait({
    runtimeMetadata: driftedRuntimeMetadata(),
    timeoutMs: 300,
    exitCode: null,
  });
  assert.equal(outcome.settled, "rejected", `wait did not bound itself: ${JSON.stringify(outcome)}`);
  assert.match(outcome.message, /provisioning did not finish within 300ms/u);
  assert.equal(outcome.killed, true, "the timed-out wait must kill the launcher");
  assert.ok(outcome.elapsedMs < KILL_AFTER_MS, `waited ${outcome.elapsedMs}ms`);
});

test("a launcher that exits while runtime metadata stays readable fails fast", async () => {
  const outcome = await driveWait({
    runtimeMetadata: driftedRuntimeMetadata(),
    timeoutMs: 600_000,
    exitCode: 3,
  });
  assert.equal(outcome.settled, "rejected", `wait did not notice the exit: ${JSON.stringify(outcome)}`);
  assert.match(outcome.message, /launcher exited 3 before provisioning finished/u);
  assert.ok(outcome.elapsedMs < KILL_AFTER_MS, `waited ${outcome.elapsedMs}ms`);
});

test("managed runtime metadata resolves the wait with the published metadata", async () => {
  const outcome = await driveWait({
    runtimeMetadata: managedRuntimeMetadata(),
    timeoutMs: 600_000,
    exitCode: null,
  });
  assert.equal(outcome.settled, "resolved", JSON.stringify(outcome));
  assert.equal(outcome.metadata.cliVersion, "9.9.9");
  assert.equal(outcome.killed, false, "a successful wait must not kill the launcher itself");
});

// The launcher hands off and exits 0 once it has published its manifest, so the exit guard must
// never outrank a completed provision: the metadata read has to settle the tick first.
test("a launcher that published managed metadata and then exited 0 is a success", async () => {
  const outcome = await driveWait({
    runtimeMetadata: managedRuntimeMetadata(),
    timeoutMs: 600_000,
    exitCode: 0,
  });
  assert.equal(
    outcome.settled,
    "resolved",
    `a completed provision was reported as a failure: ${JSON.stringify(outcome)}`,
  );
  assert.equal(outcome.metadata.source, "managed");
});

test("the wait refuses a timeout that is not a positive number instead of never expiring", async () => {
  const outcome = await driveWait({
    runtimeMetadata: driftedRuntimeMetadata(),
    timeoutMs: null,
    exitCode: null,
  });
  assert.equal(outcome.settled, "rejected", `an unbounded wait was accepted: ${JSON.stringify(outcome)}`);
  assert.match(outcome.message, /positive finite timeoutMs/u);
});

// plugin-release.yml gates the `v*` tag on this script, so every way of invoking it has to reach
// the proof. A wrong entry-point comparison used to exit 0 with no output when the path reached
// the file through a symlink, which is a vacuous pass on the release gate.
for (const invocation of ["direct", "symlinked"]) {
  test(`the gate refuses an unusable --timeout-ms when invoked ${invocation}`, async (t) => {
    let entry = proofScript;
    if (invocation === "symlinked") {
      const link = path.join(scratchDir(), "scripts");
      try {
        fs.symlinkSync(scriptsDir, link, "dir");
      } catch (error) {
        t.skip(`this platform refuses directory symlinks: ${error.code}`);
        return;
      }
      entry = path.join(link, "prove-plugin-pinned-provision.mjs");
    }
    const outcome = await runNode([entry, "--timeout-ms", "not-a-number"]);
    assert.equal(
      outcome.code,
      1,
      `the gate did not fail closed: ${JSON.stringify({ ...outcome, entry })}`,
    );
    assert.match(outcome.stderr, /::error::--timeout-ms needs a positive number/u);
  });
}

test("the gate refuses --timeout-ms with no value at all", async () => {
  const outcome = await runNode([proofScript, "--timeout-ms"]);
  assert.equal(outcome.code, 1, `the gate did not fail closed: ${JSON.stringify(outcome)}`);
  assert.match(outcome.stderr, /::error::--timeout-ms needs a positive number/u);
});

// The lane arguments have to be refused by the SCRIPT, not just by the parser the script imports:
// the release lanes invoke this file, and an argument error that only the unit test sees would
// still let a badly invoked native dispatch run the plugin lane's assertion.
for (const [name, argv, expected] of [
  [
    "a native lane with no digest authority",
    ["--lane", "native", "--expect-build-source", "explicit_package"],
    /::error::--lane native needs either --release-manifest PATH or an explicit --defer-archive-digest/u,
  ],
  [
    "a native lane with no build source",
    ["--lane", "native", "--defer-archive-digest"],
    /::error::--lane native needs --expect-build-source/u,
  ],
  [
    "a plugin lane reaching for the manifest",
    ["--release-manifest", "staged.json"],
    /::error::--release-manifest belongs to the native lane/u,
  ],
  ["a misspelled lane", ["--lane", "natvie"], /::error::--lane must be one of/u],
  ["an unknown argument", ["--nope", "1"], /::error::unknown argument --nope/u],
]) {
  test(`the gate refuses ${name} before it provisions anything`, async () => {
    const outcome = await runNode([proofScript, ...argv]);
    assert.equal(outcome.code, 1, `the gate did not fail closed: ${JSON.stringify(outcome)}`);
    assert.match(outcome.stderr, expected);
  });
}

test("the gate refuses a staged release manifest it cannot use", async () => {
  const manifestPath = path.join(scratchDir(), "release-manifest.json");
  fs.writeFileSync(manifestPath, JSON.stringify({ domain: "codestory.release-manifest" }), "utf8");
  const outcome = await runNode([
    proofScript,
    "--lane",
    "native",
    "--expect-build-source",
    "explicit_package",
    "--release-manifest",
    manifestPath,
  ]);
  assert.equal(outcome.code, 1, `the gate did not fail closed: ${JSON.stringify(outcome)}`);
  assert.match(outcome.stderr, /::error::could not use .*release-manifest\.json: release manifest schema_version/u);
});
