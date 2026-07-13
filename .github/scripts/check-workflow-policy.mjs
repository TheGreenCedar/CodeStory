#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";

const workflowRoot = path.join(".github", "workflows");
const trustedOwners = new Set(["actions", "github"]);
const shaPattern = /^[0-9a-f]{40}$/i;
const violations = [];
const sagaIssueLinkGuard = path.join(workflowRoot, "saga-issue-link-guard.yml");
const closeDevIssues = path.join(workflowRoot, "close-dev-issues.yml");
const pluginStatic = path.join(workflowRoot, "plugin-static.yml");
const rustCi = path.join(workflowRoot, "rust-ci.yml");
const releaseWorkflow = path.join(workflowRoot, "release.yml");
const postPublishReleaseSmoke = path.join(workflowRoot, "post-publish-release-smoke.yml");
const packagedPlatformPr = path.join(workflowRoot, "packaged-platform-pr.yml");
const packagedPlatformProof = path.join(workflowRoot, "packaged-platform-proof.yml");
const mainBranchSourceGuard = path.join(workflowRoot, "main-branch-source-guard.yml");

function yamlJob(content, name) {
  const lines = content.split(/\r?\n/u);
  const start = lines.indexOf(`  ${name}:`);
  if (start < 0) return [];
  const end = lines.findIndex((line, index) => index > start && /^  \S/iu.test(line));
  return lines.slice(start, end < 0 ? undefined : end);
}

function namedStep(job, name) {
  const start = job.indexOf(`      - name: ${name}`);
  if (start < 0) return [];
  const relativeEnd = job.slice(start + 1).findIndex((line) => /^      - /u.test(line));
  return job.slice(start, relativeEnd < 0 ? undefined : start + 1 + relativeEnd);
}

function managedPluginMatrixIsRequired(content, jobName, archiveLine) {
  const job = yamlJob(content, jobName);
  const step = namedStep(job, "Prove managed plugin handoff");
  const required = [
    "        run: >-",
    "          python .github/scripts/check-packaged-agent-proof.py",
    archiveLine,
    "          --managed-plugin-handoff",
  ];
  return !(
    job.length === 0 ||
    job.some((line) => /^    (?:if|continue-on-error):/u.test(line)) ||
    !job.includes("      fail-fast: false") ||
    job.some((line) => /^\s+exclude:/u.test(line)) ||
    step.length === 0 ||
    step.some((line) => /^        (?:if|continue-on-error):/u.test(line)) ||
    required.some((line) => !step.includes(line))
  );
}

function requireManagedPluginStep(content, jobName, workflowName, archiveLine) {
  if (!managedPluginMatrixIsRequired(content, jobName, archiveLine)) {
    violations.push(`${workflowName} must run the managed plugin handoff unconditionally in every matrix cell`);
  }
  const policyBypasses = [
    ["fail-fast", content.replace("      fail-fast: false\n", "")],
    ["matrix exclude", content.replace("      matrix:\n", "      matrix:\n        exclude:\n          - os: never\n")],
    ["job if", content.replace(`  ${jobName}:\n`, `  ${jobName}:\n    if: always()\n`)],
    [
      "job continue-on-error",
      content.replace(`  ${jobName}:\n`, `  ${jobName}:\n    continue-on-error: true\n`),
    ],
    [
      "step if",
      content.replace(
        "      - name: Prove managed plugin handoff\n",
        "      - name: Prove managed plugin handoff\n        if: always()\n",
      ),
    ],
    [
      "step continue-on-error",
      content.replace(
        "      - name: Prove managed plugin handoff\n",
        "      - name: Prove managed plugin handoff\n        continue-on-error: true\n",
      ),
    ],
  ];
  const acceptedBypasses = policyBypasses
    .filter(([, candidate]) => managedPluginMatrixIsRequired(candidate, jobName, archiveLine))
    .map(([name]) => name);
  if (acceptedBypasses.length > 0) {
    violations.push(`${workflowName} managed matrix policy accepted bypasses: ${acceptedBypasses.join(", ")}`);
  }
}

for (const file of fs
  .readdirSync(workflowRoot)
  .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))) {
  const workflowPath = path.join(workflowRoot, file);
  const content = fs.readFileSync(workflowPath, "utf8");

  content.split(/\r?\n/).forEach((line, index) => {
    const match = line.match(/\buses:\s*['"]?([^'"\s#]+)['"]?/);
    if (!match) return;

    const spec = match[1];
    if (spec.startsWith("./")) return;

    const at = spec.lastIndexOf("@");
    if (at === -1) {
      violations.push(`${file}:${index + 1} ${spec} is missing a ref`);
      return;
    }

    const action = spec.slice(0, at);
    const ref = spec.slice(at + 1);
    const owner = action.split("/")[0];

    if (!trustedOwners.has(owner) && !shaPattern.test(ref)) {
      violations.push(
        `${file}:${index + 1} ${spec} must pin third-party actions to a full-length SHA`,
      );
    }
  });
}

if (fs.existsSync(sagaIssueLinkGuard)) {
  const content = fs.readFileSync(sagaIssueLinkGuard, "utf8");
  const closingRef =
    /\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+(?:#\d+|https:\/\/github\.com\/TheGreenCedar\/CodeStory\/issues\/\d+)\b/im;

  if (!content.includes("review/codestory-saga-")) {
    violations.push("saga-issue-link-guard.yml must guard review/codestory-saga-* branches");
  }

  if (
    !content.includes(
      'r"(?:#\\d+|https://github\\.com/TheGreenCedar/CodeStory/issues/\\d+)\\b"',
    )
  ) {
    violations.push("saga-issue-link-guard.yml closing refs must require # or a full issue URL");
  }

  if (
    closingRef.test("Closes 123") ||
    !closingRef.test("Closes #123") ||
    !closingRef.test("Closes https://github.com/TheGreenCedar/CodeStory/issues/123")
  ) {
    violations.push("saga-issue-link-guard.yml closing ref policy must reject bare numbers and accept # or full issue URLs");
  }
}

if (!fs.existsSync(closeDevIssues)) {
  violations.push("close-dev-issues.yml must close linked issues for merged dev PRs");
} else {
  const content = fs.readFileSync(closeDevIssues, "utf8");
  const requiredSnippets = [
    "push:",
    "dev/codestory-next",
    'commit = event["after"]',
    'pull_request.get("merged_at")',
    'pull_request.get("merge_commit_sha") == commit',
    "issues: write",
    'if "pull_request" in issue:',
    '"state_reason=completed"',
  ];

  for (const snippet of requiredSnippets) {
    if (!content.includes(snippet)) {
      violations.push(`close-dev-issues.yml must include ${snippet}`);
    }
  }

  if (
    !content.includes(
      'r"(?:#(\\d+)|https://github\\.com/TheGreenCedar/CodeStory/issues/(\\d+))\\b"',
    )
  ) {
    violations.push("close-dev-issues.yml must accept only # or same-repository issue URLs");
  }
}

if (!fs.existsSync(pluginStatic)) {
  violations.push("plugin-static.yml must run plugin static tests for plugin changes");
} else {
  const content = fs.readFileSync(pluginStatic, "utf8");
  const requiredSnippets = [
    "plugins/codestory/**",
    "dev/codestory-next",
    "node --test plugins/codestory/tests/plugin-static.test.mjs",
    "node .github/scripts/check-workflow-policy.mjs",
    "python .github/scripts/check-packaged-agent-proof.py --self-test",
    "python .github/scripts/package-codestory-release.py --self-test",
    "python .github/scripts/check-codestory-release.py --version",
    ".github/scripts/check-packaged-agent-proof.py",
    ".github/scripts/package-codestory-release.py",
    ".github/workflows/release.yml",
    ".github/workflows/post-publish-release-smoke.yml",
    ".github/workflows/packaged-platform-pr.yml",
    ".github/workflows/packaged-platform-proof.yml",
    "scripts/install-codestory.ps1",
  ];

  for (const snippet of requiredSnippets) {
    if (!content.includes(snippet)) {
      violations.push(`plugin-static.yml must include ${snippet}`);
    }
  }
}

if (!fs.existsSync(rustCi)) {
  violations.push("rust-ci.yml must run default Rust workspace checks for routine code changes");
} else {
  const content = fs.readFileSync(rustCi, "utf8");
  const requiredSnippets = [
    "Cargo.lock",
    "Cargo.toml",
    "crates/**",
    "dev/codestory-next",
    "cargo fmt --check",
    "cargo check --workspace --locked",
    "cargo test --locked",
    "cargo clippy --workspace --all-targets --all-features -- -D warnings",
  ];

  for (const snippet of requiredSnippets) {
    if (!content.includes(snippet)) {
      violations.push(`rust-ci.yml must include ${snippet}`);
    }
  }
}

if (!fs.existsSync(releaseWorkflow)) {
  violations.push("release.yml must exist for release automation");
} else {
  const content = fs.readFileSync(releaseWorkflow, "utf8");
  if (content.includes("rustup toolchain install stable")) {
    violations.push("release.yml release builds must not install floating stable Rust");
  }
  for (const snippet of [
    "uses: ./.github/workflows/packaged-platform-proof.yml",
    "version: ${{ needs.preflight.outputs.version }}",
    "uses: ./.github/workflows/post-publish-release-smoke.yml",
    "--notes-file target/release-assets/proof-boundaries.md",
    'if [ "${#assets[@]}" -ne 7 ]; then',
    "Expected six binary archives plus SHA256SUMS.txt",
    "macOS x64/arm64",
    "live managed Metal endpoint survival remains open in #887",
    "Older-glibc compatibility is unproven",
    "--generate-notes",
  ]) {
    if (!content.includes(snippet)) {
      violations.push(`release.yml must include ${snippet}`);
    }
  }
  for (const row of [
    "needs:\n      - preflight\n      - publish\n    uses: ./.github/workflows/post-publish-release-smoke.yml",
  ]) {
    if (!content.includes(row)) {
      violations.push(`release.yml must preserve release proof block ${row.split("\n")[0]}`);
    }
  }
}

if (!fs.existsSync(packagedPlatformProof)) {
  violations.push("packaged-platform-proof.yml must own the reusable native package matrix");
} else {
  const content = fs.readFileSync(packagedPlatformProof, "utf8");
  for (const snippet of [
    "workflow_call:",
    "contents: read",
    'RELEASE_RUST_TOOLCHAIN: "1.95.0"',
    'LINUX_GLIBC_BUILD_IMAGE: "rust:1.95.0-bullseye@sha256:28afaeb8445f2a2e7d878bd34ed39ba02bb517efb29986188cbd59b7cf4f2fdf"',
    'LINUX_GLIBC_BASELINE_IMAGE: "ubuntu:20.04@sha256:8feb4d8ca5354def3d8fce243717141ce31e2c428701f6682bd2fafe15388214"',
    "Build Linux x64 at the glibc 2.31 baseline",
    "CARGO_TARGET_DIR=/workspace/target/glibc-2.31",
    'cp "target/glibc-2.31/${{ matrix.rust_target }}/release/codestory-cli"',
    "bash .github/scripts/check-linux-glibc-baseline.sh",
    '"glibc 2.31"',
    "linux-glibc-2.31-baseline-proof",
    "packaged-version-proof-${{ matrix.asset_target }}",
    "packaged-managed-proof-${{ matrix.asset_target }}",
    "- name: Prove managed plugin handoff",
    "--archive \"target/release-dist/codestory-cli-v${{ inputs.version }}-${{ matrix.asset_target }}.${{ matrix.extension }}\"",
    "--managed-plugin-handoff",
    "scripts/install-codestory.ps1 -SelfTest",
    "--checksum-file target/release-dist/SHA256SUMS.txt",
  ]) {
    if (!content.includes(snippet)) {
      violations.push(`packaged-platform-proof.yml must include ${snippet}`);
    }
  }
  for (const row of [
    "- os: ubuntu-latest\n            rust_target: x86_64-unknown-linux-gnu\n            asset_target: linux-x64\n            exe_suffix: \"\"\n            extension: tar.gz",
    "- os: ubuntu-24.04-arm\n            rust_target: aarch64-unknown-linux-gnu\n            asset_target: linux-arm64\n            exe_suffix: \"\"\n            extension: tar.gz",
    "- os: windows-latest\n            rust_target: x86_64-pc-windows-msvc\n            asset_target: windows-x64\n            exe_suffix: \".exe\"\n            extension: zip",
    "- os: windows-11-arm\n            rust_target: aarch64-pc-windows-msvc\n            asset_target: windows-arm64\n            exe_suffix: \".exe\"\n            extension: zip",
    "- os: macos-15-intel\n            rust_target: x86_64-apple-darwin\n            asset_target: macos-x64\n            exe_suffix: \"\"\n            extension: tar.gz",
    "- os: macos-15\n            rust_target: aarch64-apple-darwin\n            asset_target: macos-arm64\n            exe_suffix: \"\"\n            extension: tar.gz",
    "if: matrix.asset_target == 'windows-x64'\n        shell: pwsh\n        run: pwsh -File scripts/install-codestory.ps1 -SelfTest",
  ]) {
    if (!content.includes(row)) {
      violations.push(`packaged-platform-proof.yml must preserve native proof block ${row.split("\n")[0]}`);
    }
  }
  requireManagedPluginStep(
    content,
    "build",
    "packaged-platform-proof.yml",
    '          --archive "target/release-dist/codestory-cli-v${{ inputs.version }}-${{ matrix.asset_target }}.${{ matrix.extension }}"',
  );
}

if (!fs.existsSync(postPublishReleaseSmoke)) {
  violations.push("post-publish-release-smoke.yml must exist for native published-asset proof");
} else {
  const content = fs.readFileSync(postPublishReleaseSmoke, "utf8");
  for (const snippet of [
    "workflow_call:",
    "ubuntu-latest",
    "ubuntu-24.04-arm",
    "windows-latest",
    "windows-11-arm",
    "macos-15",
    "linux-x64",
    "linux-arm64",
    "windows-x64",
    "windows-arm64",
    "macos-x64",
    "macos-arm64",
    "--version-only",
    "- name: Prove managed plugin handoff",
    "--managed-plugin-handoff",
    "scripts/install-codestory.ps1 -SelfTest",
    "--checksum-file \"${{ steps.asset.outputs.checksum }}\"",
  ]) {
    if (!content.includes(snippet)) {
      violations.push(`post-publish-release-smoke.yml must include ${snippet}`);
    }
  }
  for (const row of [
    "- os: ubuntu-latest\n            asset_target: linux-x64\n            extension: tar.gz",
    "- os: ubuntu-24.04-arm\n            asset_target: linux-arm64\n            extension: tar.gz",
    "- os: windows-latest\n            asset_target: windows-x64\n            extension: zip",
    "- os: windows-11-arm\n            asset_target: windows-arm64\n            extension: zip",
    "- os: macos-15-intel\n            asset_target: macos-x64\n            extension: tar.gz",
    "- os: macos-15\n            asset_target: macos-arm64\n            extension: tar.gz",
    "if: matrix.asset_target == 'windows-x64'\n        shell: pwsh\n        run: pwsh -File scripts/install-codestory.ps1 -SelfTest",
  ]) {
    if (!content.includes(row)) {
      violations.push(`post-publish-release-smoke.yml must preserve native proof block ${row.split("\n")[0]}`);
    }
  }
  if (content.includes("sha256sum")) {
    violations.push("post-publish-release-smoke.yml must use the portable Python checksum gate");
  }
  requireManagedPluginStep(
    content,
    "smoke",
    "post-publish-release-smoke.yml",
    '          --archive "${{ steps.asset.outputs.archive }}"',
  );
}

if (!fs.existsSync(packagedPlatformPr)) {
  violations.push("packaged-platform-pr.yml must run the native package matrix on implementation PRs");
} else {
  const content = fs.readFileSync(packagedPlatformPr, "utf8");
  for (const snippet of [
    "pull_request:",
    "contents: read",
    "uses: ./.github/workflows/packaged-platform-proof.yml",
    "version: ${{ needs.prepare.outputs.version }}",
    ".github/scripts/check-packaged-agent-proof.py",
    ".github/scripts/check-linux-glibc-baseline.sh",
    ".github/workflows/post-publish-release-smoke.yml",
    ".github/workflows/packaged-platform-proof.yml",
  ]) {
    if (!content.includes(snippet)) {
      violations.push(`packaged-platform-pr.yml must include ${snippet}`);
    }
  }
  if (content.includes("contents: write") || content.includes("uses: ./.github/workflows/release.yml")) {
    violations.push("packaged-platform-pr.yml must not request release permissions or call the publishing workflow");
  }
}

if (!fs.existsSync(mainBranchSourceGuard)) {
  violations.push("main-branch-source-guard.yml must guard PRs into main");
} else {
  const content = fs.readFileSync(mainBranchSourceGuard, "utf8");
  const requiredSnippets = [
    "pull_request:",
    "- main",
    "dev/codestory-next",
    "HEAD_REPO",
    "BASE_REPO",
  ];

  for (const snippet of requiredSnippets) {
    if (!content.includes(snippet)) {
      violations.push(`main-branch-source-guard.yml must include ${snippet}`);
    }
  }
}

if (violations.length > 0) {
  console.error(violations.join("\n"));
  process.exit(1);
}

console.log("Workflow policy passed: third-party actions and saga issue-link guard are valid.");
