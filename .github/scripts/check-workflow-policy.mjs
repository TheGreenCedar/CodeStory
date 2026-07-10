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
const mainBranchSourceGuard = path.join(workflowRoot, "main-branch-source-guard.yml");

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
    "python .github/scripts/check-codestory-release.py --version",
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
  if (!content.includes('RELEASE_RUST_TOOLCHAIN: "1.95.0"')) {
    violations.push("release.yml must pin RELEASE_RUST_TOOLCHAIN to 1.95.0");
  }
  if (content.includes("rustup toolchain install stable")) {
    violations.push("release.yml release builds must not install floating stable Rust");
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
