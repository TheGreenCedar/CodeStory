import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repositoryRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);

/// A minimal tree carrying every surface the script owns.
function fixtureRoot(version) {
  const root = mkdtempSync(path.join(tmpdir(), "codestory-bump-"));
  const crates = [
    "codestory-llama-sys",
    "codestory-contracts",
    "codestory-workspace",
    "codestory-store",
    "codestory-indexer",
    "codestory-retrieval",
    "codestory-runtime",
    "codestory-cli",
    "codestory-bench",
  ];
  for (const crate of crates) {
    const directory = path.join(root, "crates", crate);
    execFileSync("mkdir", ["-p", directory]);
    writeFileSync(
      path.join(directory, "Cargo.toml"),
      `[package]\nname = "${crate}"\nversion = "${version}"\nedition = "2024"\n\n` +
        `[dependencies]\nserde = { version = "1.0", features = ["derive"] }\n`,
    );
  }
  for (const manifest of [
    "plugins/codestory/.codex-plugin/plugin.json",
    "plugins/codestory/.claude-plugin/plugin.json",
    "plugins/codestory/.github/plugin/plugin.json",
  ]) {
    const absolute = path.join(root, manifest);
    execFileSync("mkdir", ["-p", path.dirname(absolute)]);
    writeFileSync(
      absolute,
      `${JSON.stringify({ name: "codestory", version, description: "fixture" }, null, 2)}\n`,
    );
  }
  writeFileSync(
    path.join(root, "crates/codestory-llama-sys/model-contract.json"),
    `${JSON.stringify(
      { model: { file_name: "m.gguf" }, producer: { name: "codestory-llama-sys", version } },
      null,
      2,
    )}\n`,
  );
  writeFileSync(
    path.join(root, "CHANGELOG.md"),
    `# Changelog\n\n## Unreleased\n\n- something user visible\n\n## ${version}\n\nolder notes\n`,
  );
  execFileSync("mkdir", ["-p", path.join(root, "scripts")]);
  cpSync(
    path.join(repositoryRoot, "scripts/bump-version.mjs"),
    path.join(root, "scripts/bump-version.mjs"),
  );
  return root;
}

function readVersions(root) {
  const crateVersion = (crate) =>
    /^version\s*=\s*"([^"]+)"/mu.exec(
      readFileSync(path.join(root, "crates", crate, "Cargo.toml"), "utf8"),
    )?.[1];
  const jsonVersion = (relative, pointer) => {
    let value = JSON.parse(readFileSync(path.join(root, relative), "utf8"));
    for (const key of pointer) value = value[key];
    return value;
  };
  return {
    cli: crateVersion("codestory-cli"),
    bench: crateVersion("codestory-bench"),
    codexPlugin: jsonVersion("plugins/codestory/.codex-plugin/plugin.json", ["version"]),
    claudePlugin: jsonVersion("plugins/codestory/.claude-plugin/plugin.json", ["version"]),
    githubPlugin: jsonVersion("plugins/codestory/.github/plugin/plugin.json", ["version"]),
    producer: jsonVersion("crates/codestory-llama-sys/model-contract.json", [
      "producer",
      "version",
    ]),
    changelog: readFileSync(path.join(root, "CHANGELOG.md"), "utf8"),
  };
}

/// Run the script without cargo/python, which the fixture tree cannot satisfy.
function bump(root, args) {
  return execFileSync(
    process.execPath,
    [path.join(root, "scripts/bump-version.mjs"), ...args],
    { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
  );
}

test("--check reports every surface that has not been bumped", () => {
  const root = fixtureRoot("0.16.1");
  try {
    let failure;
    try {
      bump(root, ["--version", "0.17.0", "--check"]);
    } catch (error) {
      failure = error;
    }
    assert.ok(failure, "--check must fail when surfaces are stale");
    const message = `${failure.stdout}${failure.stderr}`;
    for (const surface of [
      "crates/codestory-cli/Cargo.toml",
      "plugins/codestory/.codex-plugin/plugin.json",
      "crates/codestory-llama-sys/model-contract.json",
      "CHANGELOG.md",
    ]) {
      assert.match(message, new RegExp(surface.replaceAll(".", "\\.")));
    }

    // Nothing may be written in check mode.
    assert.equal(readVersions(root).cli, "0.16.1");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("--check passes once every surface already carries the version", () => {
  const root = fixtureRoot("0.16.1");
  try {
    const output = bump(root, ["--version", "0.16.1", "--check"]);
    assert.match(output, /already carries 0\.16\.1/u);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a bad version is rejected before anything is written", () => {
  const root = fixtureRoot("0.16.1");
  try {
    assert.throws(() => bump(root, ["--version", "0.17"]), /strict semver/u);
    assert.throws(() => bump(root, ["--version", "not-a-version"]), /strict semver/u);
    assert.equal(readVersions(root).cli, "0.16.1");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
