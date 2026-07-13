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
const macosMetalProof = path.join(workflowRoot, "macos-metal-proof.yml");
const mainBranchSourceGuard = path.join(workflowRoot, "main-branch-source-guard.yml");
const sourceProof = path.join(workflowRoot, "source-proof.yml");
const repoScaleStats = path.join(workflowRoot, "repo-scale-stats.yml");
const retrievalSidecarSmoke = path.join(workflowRoot, "retrieval-sidecar-smoke.yml");

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

function yamlJobWithValue(job, key) {
  const withStart = job.indexOf("    with:");
  if (withStart < 0) return undefined;
  const relativeEnd = job.slice(withStart + 1).findIndex((line) => /^    \S/u.test(line));
  const withLines = job.slice(
    withStart + 1,
    relativeEnd < 0 ? undefined : withStart + 1 + relativeEnd,
  );
  const prefix = `      ${key}:`;
  const values = withLines
    .filter((line) => line.startsWith(prefix))
    .map((line) => line.slice(prefix.length).trim());
  return values.length === 1 ? values[0] : undefined;
}

function packagedPrSigningPolicyViolations(content) {
  const found = [];
  const packagedProofJob = yamlJob(content, "packaged-proof");
  if (yamlJobWithValue(packagedProofJob, "sign_macos") !== "false") {
    found.push("named packaged-proof job must set with.sign_macos to false");
  }
  if (packagedProofJob.some((line) => /^    secrets:/u.test(line))) {
    found.push("named packaged-proof job must not receive caller secrets");
  }
  if (/\bAPPLE_[A-Z0-9_]+\b/u.test(content)) {
    found.push("must not reference Apple secret identifiers");
  }
  if (content.includes("macos-release-signing")) {
    found.push("must not reference the release signing environment");
  }
  return found;
}

function requireContent(content, requirements, violationFor) {
  for (const requirement of requirements) {
    if (!content.includes(requirement)) {
      violations.push(violationFor(requirement));
    }
  }
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
    job.some(
      (line) =>
        /^\s+exclude:/u.test(line) &&
        !line.includes("fromJSON(inputs.scope == 'macos'"),
    ) ||
    step.length === 0 ||
    step.some((line) => /^        (?:if|continue-on-error):/u.test(line)) ||
    required.some((line) => !step.includes(line))
  );
}

function requireManagedPluginStep(content, jobName, workflowName, archiveLine) {
  if (!managedPluginMatrixIsRequired(content, jobName, archiveLine)) {
    violations.push(`${workflowName} must run the managed plugin handoff unconditionally in every matrix cell`);
  }
  const matrixBypass = content.includes("      matrix: ${{ fromJSON")
    ? content.replace(
        /^      matrix: \$\{\{ fromJSON.*$/mu,
        "      matrix:\n        exclude:\n          - os: never",
      )
    : content.replace("      matrix:\n", "      matrix:\n        exclude:\n          - os: never\n");
  const policyBypasses = [
    ["fail-fast", content.replace("      fail-fast: false\n", "")],
    ["matrix exclude", matrixBypass],
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

  if (/^  pull_request(?:_target)?:/mu.test(content)) {
    if (!/^concurrency:/mu.test(content) || !/^  cancel-in-progress: true$/mu.test(content)) {
      violations.push(`${file} pull-request runs must cancel stale work`);
    }
  }

  content.split(/\r?\n/).forEach((line, index) => {
    if (/^\s+key:/u.test(line) && line.includes("github.sha")) {
      violations.push(`${file}:${index + 1} Cargo cache keys must not include commit SHA`);
    }
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

  requireContent(content, requiredSnippets, snippet => `close-dev-issues.yml must include ${snippet}`);

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
    "node .github/scripts/route-ci-proof.mjs --self-test",
    "python .github/scripts/check-packaged-agent-proof.py --self-test",
    "python .github/scripts/package-codestory-release.py --self-test",
    "python .github/scripts/check-codestory-release.py --version",
    ".github/scripts/check-packaged-agent-proof.py",
    ".github/scripts/package-codestory-release.py",
    ".github/scripts/route-ci-proof.mjs",
    ".github/workflows/release.yml",
    ".github/workflows/post-publish-release-smoke.yml",
    ".github/workflows/packaged-platform-pr.yml",
    ".github/workflows/packaged-platform-proof.yml",
    ".github/workflows/macos-metal-proof.yml",
    ".github/workflows/source-proof.yml",
    ".github/workflows/repo-scale-stats.yml",
    "scripts/codex-worktree-setup.*",
    "scripts/tests/codex-worktree-setup.test.mjs",
    "scripts/setup-retrieval-env.*",
    "scripts/install-codestory.ps1",
  ];

  requireContent(content, requiredSnippets, snippet => `plugin-static.yml must include ${snippet}`);
}

if (!fs.existsSync(rustCi)) {
  violations.push("rust-ci.yml must run default Rust workspace checks for routine code changes");
} else {
  const content = fs.readFileSync(rustCi, "utf8");
  const requiredSnippets = [
    "Cargo.lock",
    "Cargo.toml",
    "crates/**",
    "cargo fmt --check",
    "cargo check --workspace --locked",
    "cargo clippy --workspace --lib --locked -- -D warnings",
    "Prove focused publication contracts",
    "workspace-default-features",
    "cancel-in-progress: true",
  ];

  requireContent(content, requiredSnippets, snippet => `rust-ci.yml must include ${snippet}`);
  if (content.includes("macos-") || content.includes("windows-") || content.includes("push:")) {
    violations.push("rust-ci.yml draft pushes must use one Ubuntu-only lane");
  }
}

if (!fs.existsSync(sourceProof)) {
  violations.push("source-proof.yml must own exact-head full source proof");
} else {
  const content = fs.readFileSync(sourceProof, "utf8");
  requireContent(content, [
    "types: [labeled, synchronize]",
    "review-accepted",
    'test "$EVENT_HEAD_REPO" = "$GITHUB_REPOSITORY"',
    'test "$current_head" = "$EVENT_HEAD_SHA"',
    "name: full-source-gate",
    "cargo test --workspace --locked",
    "cargo clippy --workspace --all-targets --all-features -- -D warnings",
    "cancel-in-progress: true",
  ], snippet => `source-proof.yml must include ${snippet}`);
  if (content.includes("pull_request_target:")) {
    violations.push("source-proof.yml must not execute pull-request code through pull_request_target");
  }
}

if (!fs.existsSync(releaseWorkflow)) {
  violations.push("release.yml must exist for release automation");
} else {
  const content = fs.readFileSync(releaseWorkflow, "utf8");
  if (content.includes("rustup toolchain install stable")) {
    violations.push("release.yml release builds must not install floating stable Rust");
  }
  requireContent(content, [
    "uses: ./.github/workflows/packaged-platform-proof.yml",
    "sign_macos: true",
    "APPLE_DEVELOPER_ID_P12_BASE64: ${{ secrets.APPLE_DEVELOPER_ID_P12_BASE64 }}",
    "APPLE_DEVELOPER_ID_P12_PASSWORD: ${{ secrets.APPLE_DEVELOPER_ID_P12_PASSWORD }}",
    "APPLE_SIGNING_IDENTITY: ${{ secrets.APPLE_SIGNING_IDENTITY }}",
    "APPLE_NOTARY_KEY_P8_BASE64: ${{ secrets.APPLE_NOTARY_KEY_P8_BASE64 }}",
    "APPLE_NOTARY_KEY_ID: ${{ secrets.APPLE_NOTARY_KEY_ID }}",
    "APPLE_NOTARY_ISSUER_ID: ${{ secrets.APPLE_NOTARY_ISSUER_ID }}",
    "uses: ./.github/workflows/macos-metal-proof.yml",
    "use_packaged_cli_artifact: true",
    "version: ${{ needs.preflight.outputs.version }}",
    "uses: ./.github/workflows/post-publish-release-smoke.yml",
    "--notes-file target/release-assets/proof-boundaries.md",
    'if [ "${#assets[@]}" -ne 7 ]; then',
    "Expected six binary archives plus SHA256SUMS.txt",
    "macOS x64/arm64",
    "gated on packaged managed-Metal cold/warm reuse, dead-endpoint blocking, recovery, packet, and search proof",
    "Linux x64 is packaged and executed at the glibc 2.31 baseline; this does not claim musl support or Linux arm64 baseline parity.",
    "--generate-notes",
  ], snippet => `release.yml must include ${snippet}`);
  requireContent(content, [
    "macos-metal-proof:\n    needs:\n      - preflight\n      - packaged-proof\n    uses: ./.github/workflows/macos-metal-proof.yml",
    "needs:\n      - preflight\n      - packaged-proof\n      - macos-metal-proof\n    runs-on: ubuntu-latest",
    "needs:\n      - preflight\n      - publish\n    uses: ./.github/workflows/post-publish-release-smoke.yml",
  ], row => `release.yml must preserve release proof block ${row.split("\n")[0]}`);
}

if (!fs.existsSync(packagedPlatformProof)) {
  violations.push("packaged-platform-proof.yml must own the reusable native package matrix");
} else {
  const content = fs.readFileSync(packagedPlatformProof, "utf8");
  requireContent(content, [
    "workflow_call:",
    "contents: read",
    'RELEASE_RUST_TOOLCHAIN: "1.95.0"',
    'LINUX_GLIBC_BUILD_IMAGE: "rust:1.95.0-bullseye@sha256:28afaeb8445f2a2e7d878bd34ed39ba02bb517efb29986188cbd59b7cf4f2fdf"',
    'LINUX_GLIBC_BASELINE_IMAGE: "ubuntu:20.04@sha256:8feb4d8ca5354def3d8fce243717141ce31e2c428701f6682bd2fafe15388214"',
    'APPLE_DEVELOPER_TEAM_ID: "PKUJNR8D6F"',
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
    "sign_macos:",
    "environment: ${{ inputs.sign_macos && startsWith(matrix.asset_target, 'macos-') && 'macos-release-signing' || null }}",
    "timeout-minutes: ${{ inputs.sign_macos && startsWith(matrix.asset_target, 'macos-') && 90 || 60 }}",
    "APPLE_DEVELOPER_ID_P12_BASE64",
    "APPLE_NOTARY_KEY_P8_BASE64",
    "umask 077",
    "chmod 600 \"$work_dir/developer-id.p12\" \"$work_dir/notary-key.p8\"",
    "security list-keychains -d user -s \"$keychain\"",
    "--sign \"$signing_hash\"",
    "--options runtime",
    "--timestamp",
    "xcrun notarytool submit",
    "--no-wait",
    "notarytool-submission-id.txt",
    'xcrun notarytool info "$submission_id"',
    "max_notary_attempts=120",
    "notary_poll_seconds=30",
    "Invalid|Rejected)",
    'xcrun notarytool log "$submission_id"',
    "Notarization timed out after",
    "jq -e '.status == \"Accepted\"'",
    "source=Notarized Developer ID",
    "TeamIdentifier=${APPLE_DEVELOPER_TEAM_ID}",
    '> "$proof_dir/designated-requirement.txt" 2>&1',
    "subject\\\\.OU",
    "macos-notarization-proof-${{ matrix.asset_target }}",
    "--intel-runtime-policy",
    "packaged-intel-policy-proof-${{ matrix.asset_target }}",
    "if: matrix.asset_target == 'macos-x64'",
    "scripts/install-codestory.ps1 -SelfTest",
    "--checksum-file target/release-dist/SHA256SUMS.txt",
    "scope:",
    "proof_key:",
    "ref:",
    "cancel-in-progress: true",
    "codestory-cli-default-features",
    "matrix: ${{ fromJSON(inputs.scope == 'macos'",
  ], snippet => `packaged-platform-proof.yml must include ${snippet}`);
  const macosSigningStep = namedStep(yamlJob(content, "build"), "Sign and notarize macOS CLI").join("\n");
  const blockingNotaryWait = /^\s+--wait(?:\s|\\|$)/mu;
  if (blockingNotaryWait.test(macosSigningStep)) {
    violations.push("packaged-platform-proof.yml must poll notarization explicitly instead of using notarytool --wait");
  }
  const blockingWaitBypass = namedStep(
    yamlJob(content.replace("--no-wait", "--wait"), "build"),
    "Sign and notarize macOS CLI",
  ).join("\n");
  if (!blockingNotaryWait.test(blockingWaitBypass)) {
    violations.push("packaged-platform-proof.yml blocking notary wait policy did not detect its bypass fixture");
  }
  const releaseAssetStart = content.indexOf("- name: Upload release asset");
  const notarizationProofStart = content.indexOf("- name: Upload macOS notarization proof");
  const releaseAssetBlock = releaseAssetStart >= 0
    ? content.slice(releaseAssetStart, notarizationProofStart >= 0 ? notarizationProofStart : undefined)
    : "";
  if (releaseAssetBlock.includes("target/notarization-proof")) {
    violations.push("packaged-platform-proof.yml must keep notarization evidence out of the flat binary release artifact");
  }
  if (!releaseAssetBlock.includes("target/release-dist/SHA256SUMS.txt")) {
    violations.push("packaged-platform-proof.yml must include SHA256SUMS.txt in each reusable package artifact");
  }
  const matrixMatch = content.match(
    /^      matrix: \$\{\{ fromJSON\(inputs\.scope == 'macos' && '([^']+)' \|\| '([^']+)'\) \}\}$/mu,
  );
  const expectedFullMatrix = {
    include: [
      { os: "ubuntu-latest", rust_target: "x86_64-unknown-linux-gnu", asset_target: "linux-x64", exe_suffix: "", extension: "tar.gz" },
      { os: "ubuntu-24.04-arm", rust_target: "aarch64-unknown-linux-gnu", asset_target: "linux-arm64", exe_suffix: "", extension: "tar.gz" },
      { os: "windows-latest", rust_target: "x86_64-pc-windows-msvc", asset_target: "windows-x64", exe_suffix: ".exe", extension: "zip" },
      { os: "windows-11-arm", rust_target: "aarch64-pc-windows-msvc", asset_target: "windows-arm64", exe_suffix: ".exe", extension: "zip" },
      { os: "macos-15-intel", rust_target: "x86_64-apple-darwin", asset_target: "macos-x64", exe_suffix: "", extension: "tar.gz" },
      { os: "macos-15", rust_target: "aarch64-apple-darwin", asset_target: "macos-arm64", exe_suffix: "", extension: "tar.gz" },
    ],
  };
  const expectedMacMatrix = {
    include: expectedFullMatrix.include.filter(row => row.asset_target.startsWith("macos-")),
  };
  try {
    const macMatrix = matrixMatch && JSON.parse(matrixMatch[1]);
    const fullMatrix = matrixMatch && JSON.parse(matrixMatch[2]);
    if (JSON.stringify(macMatrix) !== JSON.stringify(expectedMacMatrix)) {
      violations.push("packaged-platform-proof.yml macos scope must contain exactly the two Mac package rows");
    }
    if (JSON.stringify(fullMatrix) !== JSON.stringify(expectedFullMatrix)) {
      violations.push("packaged-platform-proof.yml full scope must contain exactly all six native package rows");
    }
  } catch {
    violations.push("packaged-platform-proof.yml package matrices must be valid JSON objects");
  }
  requireContent(content, [
    "if: matrix.asset_target == 'windows-x64'\n        shell: pwsh\n        run: pwsh -File scripts/install-codestory.ps1 -SelfTest",
  ], row => `packaged-platform-proof.yml must preserve native proof block ${row.split("\n")[0]}`);
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
  requireContent(content, [
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
    "Prove published macOS signature, notarization, and Gatekeeper acceptance",
    "archive-quarantine.txt",
    "extracted-binary-quarantine.txt",
    "Authority=Developer ID Application:",
    "source=Notarized Developer ID",
    'APPLE_DEVELOPER_TEAM_ID: "PKUJNR8D6F"',
    "TeamIdentifier=${APPLE_DEVELOPER_TEAM_ID}",
    '> "$proof_dir/designated-requirement.txt" 2>&1',
    "subject\\\\.OU",
    "--intel-runtime-policy",
    "target/post-publish-intel-policy-proof",
    "if: matrix.asset_target == 'macos-x64'",
    "scripts/install-codestory.ps1 -SelfTest",
    "--checksum-file \"${{ steps.asset.outputs.checksum }}\"",
  ], snippet => `post-publish-release-smoke.yml must include ${snippet}`);
  requireContent(content, [
    "- os: ubuntu-latest\n            asset_target: linux-x64\n            extension: tar.gz",
    "- os: ubuntu-24.04-arm\n            asset_target: linux-arm64\n            extension: tar.gz",
    "- os: windows-latest\n            asset_target: windows-x64\n            extension: zip",
    "- os: windows-11-arm\n            asset_target: windows-arm64\n            extension: zip",
    "- os: macos-15-intel\n            asset_target: macos-x64\n            extension: tar.gz",
    "- os: macos-15\n            asset_target: macos-arm64\n            extension: tar.gz",
    "if: matrix.asset_target == 'windows-x64'\n        shell: pwsh\n        run: pwsh -File scripts/install-codestory.ps1 -SelfTest",
  ], row => `post-publish-release-smoke.yml must preserve native proof block ${row.split("\n")[0]}`);
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
  violations.push("packaged-platform-pr.yml must orchestrate explicitly promoted platform proof");
} else {
  const content = fs.readFileSync(packagedPlatformPr, "utf8");
  requireContent(content, [
    "types: [labeled, synchronize]",
    "platform-proof",
    "workflow_dispatch:",
    "options: [platform, integration]",
    "expected_head_sha:",
    "actions: read",
    'test "$head_repo" = "$GITHUB_REPOSITORY"',
    'test "$current_head" = "$expected_head"',
    "actions/runs?head_sha=$HEAD_SHA",
    '.path == ".github/workflows/source-proof.yml"',
    ".head_repository.full_name == $repo",
    '.name == "full-source-gate" and .conclusion == "success"',
    'test "$INPUT_HEAD_SHA" = "$dev_head"',
    "node .github/scripts/route-ci-proof.mjs --stdin",
    "scope=full",
    "uses: ./.github/workflows/source-proof.yml",
    "uses: ./.github/workflows/repo-scale-stats.yml",
    "uses: ./.github/workflows/packaged-platform-proof.yml",
    "scope: ${{ needs.route.outputs.scope }}",
    "always() &&\n      needs.route.result == 'success' &&\n      needs.packaged-proof.result == 'success' &&",
    "uses: ./.github/workflows/macos-metal-proof.yml",
    "use_packaged_cli_artifact: true",
    "dev/codestory-next moved from proved head",
  ], snippet => `packaged-platform-pr.yml must include ${snippet}`);
  for (const violation of packagedPrSigningPolicyViolations(content)) {
    violations.push(`packaged-platform-pr.yml ${violation}`);
  }
  const signingPolicyMutations = [
    [
      "sign_macos true",
      content.replace("      sign_macos: false", "      sign_macos: true"),
    ],
    [
      "explicit secrets mapping",
      content.replace(
        "      sign_macos: false",
        "      sign_macos: false\n    secrets:\n      PACKAGE_PROOF_TOKEN: ${{ secrets.PACKAGE_PROOF_TOKEN }}",
      ),
    ],
    [
      "inherited secrets",
      content.replace("      sign_macos: false", "      sign_macos: false\n    secrets: inherit"),
    ],
    [
      "Apple secret identifier",
      content.replace("      sign_macos: false", "      sign_macos: false\n    # APPLE_NOTARY_KEY_ID"),
    ],
    [
      "release signing environment",
      content.replace(
        "  packaged-proof:\n",
        "  packaged-proof:\n    environment: macos-release-signing\n",
      ),
    ],
  ];
  for (const [name, candidate] of signingPolicyMutations) {
    if (candidate === content) {
      violations.push(`packaged-platform-pr.yml could not apply ${name} policy mutation`);
    } else if (packagedPrSigningPolicyViolations(candidate).length === 0) {
      violations.push(`packaged-platform-pr.yml signing policy accepted ${name} mutation`);
    }
  }
  if ((content.match(/branches\/dev\/codestory-next/gu) ?? []).length < 2) {
    violations.push("packaged-platform-pr.yml must verify the dev head before and after integration proof");
  }
  if (
    content.includes("contents: write") ||
    content.includes("uses: ./.github/workflows/release.yml") ||
    content.includes("pull_request_target:")
  ) {
    violations.push("packaged-platform-pr.yml must not request release permissions or call the publishing workflow");
  }
}

if (!fs.existsSync(macosMetalProof)) {
  violations.push("macos-metal-proof.yml must gate release assets on protected Apple Silicon hardware");
} else {
  const content = fs.readFileSync(macosMetalProof, "utf8");
  requireContent(content, [
    "workflow_call:",
    "workflow_dispatch:",
    "use_packaged_cli_artifact:",
    "runs-on: [self-hosted, macOS, ARM64, codestory-metal]",
    "environment: macos-metal-release",
    "actions/download-artifact@v8.0.1",
    "name: codestory-cli-macos-arm64",
    "node scripts/setup-retrieval-env.mjs --self-test",
    "python3 --version",
    "test \"$macos_major\" -ge 15",
    "--native-accelerator-lifecycle",
    "--managed-plugin-grounding-convergence",
    "CODESTORY_PROOF_TEMP_ROOT:",
    "Clean and assert proof-owned hardware state",
    "--cleanup-proof-temp-root",
    "CODESTORY_EMBED_DEVICE_PROVIDER: metal",
    "CODESTORY_EMBED_ALLOW_CPU: \"0\"",
    "macos-arm64-metal-proof-${{ inputs.version }}",
    "proof_key:",
    "cancel-in-progress: true",
  ], snippet => `macos-metal-proof.yml must include ${snippet}`);
}

if (!fs.existsSync(repoScaleStats)) {
  violations.push("repo-scale-stats.yml must own promoted-head stats proof");
} else {
  const content = fs.readFileSync(repoScaleStats, "utf8");
  requireContent(content, [
    "workflow_call:",
    "cargo build --release --locked -p codestory-cli",
    "cargo test --locked -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture",
    "repo-scale-stats-${{ inputs.proof_key }}",
    "actions/upload-artifact@v7.0.1",
    "codestory-cli-default-features",
    "cancel-in-progress: true",
  ], snippet => `repo-scale-stats.yml must include ${snippet}`);
}

if (!fs.existsSync(retrievalSidecarSmoke)) {
  violations.push("retrieval-sidecar-smoke.yml must exist");
} else {
  const windowsJob = yamlJob(fs.readFileSync(retrievalSidecarSmoke, "utf8"), "windows-manifest-missing");
  if (
    !windowsJob.includes("    if: github.event_name == 'workflow_dispatch'") ||
    windowsJob.some(line => line.includes("labels"))
  ) {
    violations.push("retrieval-sidecar-smoke.yml Windows proof must be workflow_dispatch-only");
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

  requireContent(content, requiredSnippets, snippet => `main-branch-source-guard.yml must include ${snippet}`);
}

if (violations.length > 0) {
  console.error(violations.join("\n"));
  process.exit(1);
}

console.log("Workflow policy passed: third-party actions and saga issue-link guard are valid.");
