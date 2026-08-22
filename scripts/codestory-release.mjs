#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import process from "node:process";
import {
  initReceipt,
  invalidateReceipt,
  recordGroup,
} from "../.github/scripts/release-driver-receipt.mjs";

export const COORDINATOR_SCHEMA = "codestory.release-coordinator/v1";
export const SOURCE_PROOF_WORKFLOW = ".github/workflows/source-proof.yml";
export const PACKAGED_WORKFLOW = ".github/workflows/packaged-platform-pr.yml";
export const RELEASE_WORKFLOW = ".github/workflows/release.yml";
export const BROAD_WORKFLOWS = Object.freeze([
  SOURCE_PROOF_WORKFLOW,
  PACKAGED_WORKFLOW,
  RELEASE_WORKFLOW,
  ".github/workflows/macos-metal-proof.yml",
  ".github/workflows/windows-vulkan-proof.yml",
]);

const MARKER = `<!-- ${COORDINATOR_SCHEMA} -->`;
const DIGEST = "a".repeat(64);
const PHASES = Object.freeze([
  "preflight",
  "source_stabilization",
  "calibration",
  "freeze",
  "frozen_candidate_acceptance",
  "package",
  "hardware",
  "installed_candidate",
  "qualification",
  "awaiting_approval",
  "promotion",
  "publication",
  "closeout",
  "complete",
]);
const EXPENSIVE = new Set([
  "source_stabilization",
  "calibration",
  "frozen_candidate_acceptance",
  "package",
  "hardware",
  "installed_candidate",
  "qualification",
  "publication",
]);
const NATIVE_ASSETS = Object.freeze(["linux-x64", "macos-arm64", "windows-x64"]);
const PHASE_ESTIMATE_MINUTES = Object.freeze({
  preflight: 2,
  source_stabilization: 45,
  calibration: 25,
  freeze: 5,
  frozen_candidate_acceptance: 15,
  package: 20,
  hardware: 15,
  installed_candidate: 10,
  qualification: 15,
  awaiting_approval: 5,
  promotion: 5,
  publication: 15,
  closeout: 10,
  complete: 0,
});

function fail(message) {
  throw new Error(message);
}

function present(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function jsonFence(value) {
  return `${MARKER}\n\`\`\`json\n${JSON.stringify(value, null, 2)}\n\`\`\`\n`;
}

function parseRecord(body) {
  if (!present(body) || !body.includes(MARKER)) return null;
  const match = body.match(/```json\n([\s\S]*?)\n```/);
  if (!match) return null;
  const parsed = JSON.parse(match[1]);
  if (parsed?.schema !== COORDINATOR_SCHEMA) return null;
  return parsed;
}

function formatDuration(ms) {
  const total = Math.max(0, Math.round(Number(ms) / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

function remainingPath(phase) {
  const index = PHASES.indexOf(phase);
  const minutes = PHASES.slice(Math.max(0, index))
    .reduce((sum, name) => sum + (PHASE_ESTIMATE_MINUTES[name] ?? 0), 0);
  return formatDuration(minutes * 60 * 1000);
}

function parseArgs(argv) {
  const options = {
    command: argv[0],
    version: undefined,
    lane: "native",
    rehearse: false,
    issue: undefined,
    recordApproval: false,
    approver: undefined,
    repo: undefined,
    injectFailure: undefined,
    operatorSuppliedRunIds: [],
  };
  for (let index = 1; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--rehearse") {
      options.rehearse = true;
      continue;
    }
    if (argument === "--record-approval") {
      options.recordApproval = true;
      continue;
    }
    const value = argv[index + 1];
    if (["--version", "--lane", "--issue", "--approver", "--repo", "--inject-failure"].includes(argument)) {
      if (!present(value) || value.startsWith("--")) fail(`${argument} requires a value`);
      const key = argument.slice(2).replace(/-([a-z])/gu, (_, letter) => letter.toUpperCase());
      options[key] = value;
      index += 1;
      continue;
    }
    if (argument === "--run-id" || argument === "--run-ids") {
      options.operatorSuppliedRunIds.push(value);
      index += 1;
      continue;
    }
    fail(`unknown argument ${argument}`);
  }
  if (!["start", "status", "advance", "resume"].includes(options.command)) {
    fail("usage: node scripts/codestory-release.mjs <start|status|advance|resume> ...");
  }
  return options;
}

function runEvidence(group, run, commit, tree) {
  return {
    lane: group,
    run_id: run.id,
    attempt: run.attempt ?? 1,
    artifact: `${group}-run-${run.id}-attempt-${run.attempt ?? 1}`,
    digest: DIGEST,
    identity: `${group}@${commit.slice(0, 8)}`,
    conclusion: run.conclusion ?? "success",
    commit,
    tree,
  };
}

function ensureBaseGroups(record, host) {
  const head = host.heads.next;
  let receipt = record.receipt;
  if (!receipt.groups["calibration-source"]) {
    receipt = recordGroup(receipt, "calibration-source", {
      commit: head.commit,
      tree: head.tree,
    });
  }
  if (!receipt.groups["pull-requests"]) {
    receipt = recordGroup(receipt, "pull-requests", {
      release_pr: null,
      bind: "next_head",
      integrated_support_prs: [],
    });
  }
  if (!receipt.groups.evidence) {
    receipt = recordGroup(receipt, "evidence", { reusable: [], invalidated: [] });
  }
  if (!receipt.groups["next-action"]) {
    receipt = recordGroup(receipt, "next-action", {
      action: "run preflight then source stabilization",
      owner: "codestory-release",
    });
  }
  record.receipt = receipt;
}

function nextActionFor(phase, rehearse) {
  const actions = {
    preflight: "run preflight then source stabilization",
    source_stabilization: "wait for source-stabilization acceptance",
    calibration: "dispatch protected Metal calibration",
    freeze: "apply the generated constant-set freeze commit",
    frozen_candidate_acceptance: "dispatch frozen-candidate acceptance",
    package: "dispatch package proof",
    hardware: "dispatch protected hardware proof",
    installed_candidate: "dispatch installed-candidate proof",
    qualification: "dispatch qualification",
    awaiting_approval: "record combined maintainer approval",
    promotion: rehearse
      ? "rehearse promotion without merging to main"
      : "fast-forward next to F and open the tree-preserving next→main PR",
    publication: rehearse
      ? "authenticate release.yml with publish_release=false"
      : "publish after recorded approval",
    closeout: "verify tag, assets, catalog-delivery, and live ground",
    complete: "release coordinator complete",
  };
  return actions[phase] ?? phase;
}

function compact(record, extras = {}) {
  const started = Date.parse(record.started_at);
  const now = Date.parse(extras.now ?? record.started_at);
  return {
    command: extras.command ?? "status",
    phase: record.phase,
    sha: extras.sha ?? record.heads?.C ?? record.receipt.groups["calibration-source"]?.value?.commit,
    tree: extras.tree ?? record.heads?.C_tree ?? record.receipt.groups["calibration-source"]?.value?.tree,
    active_runs: extras.active_runs ?? [],
    blocker: extras.blocker ?? null,
    next_action: extras.next_action ?? nextActionFor(record.phase, record.rehearse),
    elapsed: formatDuration(now - started),
    estimated_critical_path: remainingPath(record.phase),
    rehearse: Boolean(record.rehearse),
    dispatched: extras.dispatched ?? null,
    record,
  };
}

function runnerBlocker(probes) {
  const runners = probes.runners ?? {};
  for (const [hostName, state] of Object.entries(runners)) {
    if (state !== "alive") {
      return {
        blocker: `protected runner ${hostName} is ${state}; heartbeat preflight is unproven`,
        next_action: `Restore ${hostName} heartbeat via reserve-protected-runners before dispatch`,
      };
    }
  }
  return null;
}

function preflight(record, host, intendedPhase) {
  const probes = host.probes ?? {};
  if (probes.macosZombie === "zombie" || probes.macosIdentity === "unreadable") {
    return {
      blocker: "macOS zombie PID remains while exact runtime identity is unreadable",
      next_action: "Clear the macOS zombie PID and re-read the installed runtime identity",
    };
  }
  if (
    (intendedPhase === "package" || intendedPhase === "hardware" || record.phase === "package")
    && probes.windowsStaging === "fail"
  ) {
    return {
      blocker: "Windows path or native staging probe failed",
      next_action: "Reproduce the Windows staging path in a sub-90s probe before another package dispatch",
    };
  }
  if (EXPENSIVE.has(intendedPhase) || intendedPhase === "preflight") {
    const runners = runnerBlocker(probes);
    if (runners) return runners;
  }
  if (
    intendedPhase === "source_stabilization"
    && probes.acceptanceMode
    && probes.acceptanceMode !== "source_stabilization"
  ) {
    return {
      blocker: `wrong source-proof acceptance mode ${probes.acceptanceMode}; required acceptance_phase=source_stabilization`,
      next_action: "Dispatch source-proof.yml with acceptance_only=true and acceptance_phase=source_stabilization",
    };
  }
  return null;
}

function activeRuns(host, commit) {
  return (host.workflowRuns?.() ?? host.runs ?? []).filter((run) =>
    ["queued", "waiting", "requested", "pending", "in_progress"].includes(run.status)
    && (!commit || run.headSha === commit)
    && !run.optional
  );
}

function optionalRuns(host) {
  return (host.runs ?? []).filter((run) => run.optional && run.status === "in_progress");
}

function persist(host, record) {
  const body = jsonFence(record);
  const existing = (host.listIssueComments(record.issue_number) ?? [])
    .find((comment) => comment.body?.includes(MARKER));
  if (existing) host.updateIssueComment(existing.id, body);
  else host.createIssueComment(record.issue_number, body);
  if (typeof host.persistRecord === "function") host.persistRecord(record);
}

function loadRecord(host, options) {
  const issueNumber = options.issue
    ? Number(options.issue)
    : host.listOpenCoordinatorIssues?.()?.[0]?.number;
  if (!issueNumber) fail("no coordinator issue found; pass --issue or run start");
  const comments = host.listIssueComments(issueNumber) ?? [];
  const record = comments.map((comment) => parseRecord(comment.body)).find(Boolean);
  if (!record) fail(`coordinator record not found on issue #${issueNumber}`);
  return record;
}

function observeCompleted(record, host) {
  const source = record.receipt.groups["calibration-source"]?.value;
  if (!source) return record;
  const completed = (host.runs ?? []).filter((run) =>
    run.status === "completed" && run.conclusion === "success"
  );
  const commit = source.commit;
  const tree = source.tree;
  const frozen = record.receipt.groups["frozen-candidate"]?.value;
  for (const run of completed) {
    if (
      run.workflow === SOURCE_PROOF_WORKFLOW
      && run.inputs?.acceptance_phase === "source_stabilization"
      && !record.receipt.groups["source-stabilization"]
    ) {
      record.receipt = recordGroup(
        record.receipt,
        "source-stabilization",
        runEvidence("source-stabilization", run, commit, tree),
      );
      record.receipt = recordGroup(
        record.receipt,
        "source-proof",
        runEvidence("source-proof", run, commit, tree),
      );
    }
    if (run.workflow === PACKAGED_WORKFLOW && run.inputs?.mode === "calibration" && !record.receipt.groups.calibration) {
      record.receipt = recordGroup(record.receipt, "calibration", [
        runEvidence("metal-1", { ...run, id: run.id, attempt: 1 }, commit, tree),
        runEvidence("metal-2", { ...run, id: run.id + 1, attempt: 1 }, commit, tree),
        runEvidence("metal-3", { ...run, id: run.id + 2, attempt: 1 }, commit, tree),
      ]);
    }
    if (
      run.workflow === SOURCE_PROOF_WORKFLOW
      && run.inputs?.acceptance_phase === "frozen_candidate"
      && frozen
      && !record.receipt.groups["frozen-candidate-acceptance"]
    ) {
      record.receipt = recordGroup(
        record.receipt,
        "frozen-candidate-acceptance",
        runEvidence("frozen-candidate-acceptance", run, frozen.commit, frozen.tree),
      );
    }
    if (run.workflow === PACKAGED_WORKFLOW && run.inputs?.mode === "package" && frozen && !record.receipt.groups.package) {
      record.receipt = recordGroup(
        record.receipt,
        "package",
        runEvidence("package", run, frozen.commit, frozen.tree),
      );
    }
    if (run.workflow === PACKAGED_WORKFLOW && run.inputs?.mode === "platform" && frozen && !record.receipt.groups.hardware) {
      record.receipt = recordGroup(
        record.receipt,
        "hardware",
        runEvidence("hardware", run, frozen.commit, frozen.tree),
      );
    }
    if (
      run.workflow === PACKAGED_WORKFLOW
      && run.inputs?.mode === "installed"
      && frozen
      && !record.receipt.groups["installed-candidate"]
    ) {
      record.receipt = recordGroup(
        record.receipt,
        "installed-candidate",
        runEvidence("installed-candidate", run, frozen.commit, frozen.tree),
      );
    }
    if (
      run.workflow === PACKAGED_WORKFLOW
      && run.inputs?.mode === "qualification"
      && frozen
      && !record.receipt.groups.qualification
    ) {
      record.receipt = recordGroup(
        record.receipt,
        "qualification",
        runEvidence("qualification", run, frozen.commit, frozen.tree),
      );
    }
  }
  return record;
}

function activeGroup(record, name) {
  const entry = record.receipt.groups[name];
  return entry?.status === "active" ? entry : undefined;
}

function derivePhase(record) {
  if (activeGroup(record, "catalog-delivery") && (activeGroup(record, "publication") || record.rehearse)) {
    return "complete";
  }
  if (activeGroup(record, "publication")) return "closeout";
  if (record.approval && ["promotion", "publication", "closeout"].includes(record.phase)) {
    return record.phase === "awaiting_approval" ? "promotion" : record.phase;
  }
  if (activeGroup(record, "qualification")) return record.approval ? "promotion" : "awaiting_approval";
  if (activeGroup(record, "installed-candidate")) return "qualification";
  if (activeGroup(record, "hardware")) return "installed_candidate";
  if (activeGroup(record, "package")) return "hardware";
  if (activeGroup(record, "frozen-candidate-acceptance")) return "package";
  if (activeGroup(record, "frozen-candidate")) return "frozen_candidate_acceptance";
  if (activeGroup(record, "calibration")) return "freeze";
  if (activeGroup(record, "source-stabilization")) return "calibration";
  if (record.phase && PHASES.includes(record.phase)) return record.phase;
  return "preflight";
}

function laterPhase(left, right) {
  return PHASES.indexOf(left) >= PHASES.indexOf(right) ? left : right;
}

function handleDrift(record, host) {
  const source = record.receipt.groups["calibration-source"]?.value;
  const live = host.heads.next;
  if (!source || source.commit === live.commit) return { record, drift: false, residual: [] };
  const superseded = (host.runs ?? []).filter((run) =>
    ["queued", "in_progress", "waiting", "requested", "pending"].includes(run.status)
    && run.headSha !== live.commit
  );
  const cancelled = [];
  for (const run of superseded) {
    host.cancelRun?.(run.id);
    cancelled.push(run.id);
    host.freezeBarrier?.("cancel-superseded", {
      commit: live.commit,
      workflows: BROAD_WORKFLOWS,
    });
  }
  const residual = (host.runs ?? []).filter((run) =>
    ["queued", "in_progress", "waiting", "requested", "pending"].includes(run.status)
    && run.headSha !== live.commit
  );
  if (residual.length > 0) {
    return {
      record,
      drift: true,
      residual,
      blocker: "superseded run ignored ordinary cancellation",
      next_action: "force-cancel via release-freeze-barrier invalidate-superseded",
    };
  }
  record.receipt = invalidateReceipt(record.receipt, {
    event: "evidence",
    groups: Object.keys(record.receipt.groups),
    reason: "source head moved after coordinator start",
    replacingSha: live.commit,
  });
  record.heads = { ...record.heads, C: live.commit, C_tree: live.tree };
  record.phase = "preflight";
  record.receipt = recordGroup(record.receipt, "calibration-source", {
    commit: live.commit,
    tree: live.tree,
  });
  record.receipt = recordGroup(record.receipt, "evidence", {
    reusable: [],
    invalidated: [{
      identity: `superseded@${source.commit.slice(0, 8)}`,
      reason: "source head moved",
      replacing_sha: live.commit,
    }],
  });
  record.receipt = recordGroup(record.receipt, "next-action", {
    action: "run preflight then source stabilization",
    owner: "codestory-release",
  });
  return { record, drift: true, residual: [], cancelled };
}

function dispatchSourceStabilization(record, host) {
  return host.dispatch({
    workflow: SOURCE_PROOF_WORKFLOW,
    ref: "dev/codestory-next",
    inputs: {
      expected_head_sha: host.heads.next.commit,
      version: record.receipt.version,
      acceptance_only: "true",
      acceptance_phase: "source_stabilization",
      freeze_receipt_digest: "",
      support_prs_json: "[]",
      reusable_evidence_json: "[]",
      invalidated_evidence_json: "[]",
    },
  });
}

function dispatchPackaged(record, host, mode) {
  const head = record.receipt.groups["frozen-candidate"]?.value ?? host.heads.next;
  return host.dispatch({
    workflow: PACKAGED_WORKFLOW,
    ref: "dev/codestory-next",
    inputs: {
      mode,
      version: record.receipt.version,
      expected_head_sha: head.commit,
    },
  });
}

function assertAssetLane(record, host) {
  const assets = host.assets ?? [];
  if (record.lane === "native") {
    const missing = NATIVE_ASSETS.filter((name) => !assets.includes(name));
    if (assets.includes("plugin-only") || missing.length === NATIVE_ASSETS.length) {
      fail("native lane requires native archives, not plugin-only assets");
    }
  }
  if (record.lane === "plugin") {
    const nativeHit = assets.some((name) => NATIVE_ASSETS.includes(name));
    if (nativeHit) {
      return {
        blocker: "plugin-only lane must not publish native archive assets",
        next_action: "Use --lane native for native archives or drop native assets from the plugin publication",
      };
    }
  }
  return null;
}

function applyFreeze(record, host) {
  const generated = host.generatedFreezeHead ?? {
    commit: "f".repeat(40),
    tree: "2".repeat(40),
  };
  if (!record.receipt.groups["frozen-candidate"]) {
    record.receipt = recordGroup(record.receipt, "frozen-candidate", generated);
  }
  record.heads = { ...record.heads, F: generated.commit, F_tree: generated.tree };
  record.phase = "frozen_candidate_acceptance";
  return record;
}

function recordMarketplace(record, host) {
  const marketplace = host.marketplace ?? { state: "deferred", installer_identity: "codex_marketplace_deferred_fixture" };
  const state = marketplace.state === "published" ? "published" : (marketplace.state === "unpublished" ? "deferred" : marketplace.state);
  record.receipt = recordGroup(record.receipt, "catalog-delivery", {
    state,
    installer_identity: marketplace.installer_identity
      ?? (state === "published" ? "codex_marketplace" : "codex_marketplace_deferred_fixture"),
  });
  return record;
}

function dispatchFor(record, host) {
  const phase = record.phase;
  if (phase === "preflight" || phase === "source_stabilization") {
    return { record, dispatched: dispatchSourceStabilization(record, host), phase: "source_stabilization" };
  }
  if (phase === "calibration") {
    return { record, dispatched: dispatchPackaged(record, host, "calibration"), phase: "calibration" };
  }
  if (phase === "freeze") {
    return { record: applyFreeze(record, host), dispatched: null, phase: "frozen_candidate_acceptance" };
  }
  if (phase === "frozen_candidate_acceptance") {
    const frozen = record.receipt.groups["frozen-candidate"].value;
    const dispatched = host.dispatch({
      workflow: SOURCE_PROOF_WORKFLOW,
      ref: "dev/codestory-next",
      inputs: {
        expected_head_sha: frozen.commit,
        version: record.receipt.version,
        acceptance_only: "true",
        acceptance_phase: "frozen_candidate",
      },
    });
    return { record, dispatched, phase: "frozen_candidate_acceptance" };
  }
  if (phase === "package") {
    return { record, dispatched: dispatchPackaged(record, host, "package"), phase: "package" };
  }
  if (phase === "hardware") {
    return { record, dispatched: dispatchPackaged(record, host, "platform"), phase: "hardware" };
  }
  if (phase === "installed_candidate") {
    return { record, dispatched: dispatchPackaged(record, host, "installed"), phase: "installed_candidate" };
  }
  if (phase === "qualification") {
    return { record, dispatched: dispatchPackaged(record, host, "qualification"), phase: "qualification" };
  }
  if (phase === "promotion") {
    if (!record.rehearse) host.promoteToMain?.();
    record.phase = "publication";
    return { record, dispatched: null, phase: "publication" };
  }
  if (phase === "publication") {
    const published = !record.rehearse && Boolean(record.approval);
    const dispatched = host.dispatch({
      workflow: RELEASE_WORKFLOW,
      ref: record.rehearse ? "dev/codestory-next" : "main",
      inputs: {
        version: record.receipt.version,
        expected_head_sha: record.heads?.F ?? host.heads.next.commit,
        publish_release: published ? "true" : "false",
      },
    });
    if (published) host.createTag?.(`v${record.receipt.version}`);
    record.phase = "closeout";
    return { record, dispatched, phase: "closeout" };
  }
  if (phase === "closeout") {
    record = recordMarketplace(record, host);
    record.phase = "complete";
    return { record, dispatched: null, phase: "complete" };
  }
  return { record, dispatched: null, phase };
}

async function start(options, host) {
  if (!present(options.version)) fail("--version is required");
  if (!["native", "plugin"].includes(options.lane)) fail("--lane must be native or plugin");
  const issue = options.issue
    ? { number: Number(options.issue) }
    : host.createIssue({
      title: `Release ${options.version} coordinator`,
      body: `Canonical coordinator record for ${options.version}.`,
    });
  const head = host.heads.next;
  const now = host.now().toISOString();
  const record = {
    schema: COORDINATOR_SCHEMA,
    lane: options.lane,
    rehearse: Boolean(options.rehearse),
    phase: "preflight",
    started_at: now,
    issue_number: issue.number,
    approval: null,
    heads: { C: head.commit, C_tree: head.tree },
    receipt: initReceipt(options.version),
  };
  ensureBaseGroups(record, host);
  persist(host, record);
  return compact(record, { command: "start", sha: head.commit, tree: head.tree, now });
}

async function observe(options, host, command) {
  if (options.operatorSuppliedRunIds.length > 0) {
    host.operatorSuppliedRunIds.push(...options.operatorSuppliedRunIds);
  }
  let record = loadRecord(host, options);
  ensureBaseGroups(record, host);
  const drift = handleDrift(record, host);
  record = drift.record;
  if (drift.residual?.length) {
    persist(host, record);
    return compact(record, {
      command,
      sha: host.heads.next.commit,
      tree: host.heads.next.tree,
      now: host.now().toISOString(),
      active_runs: drift.residual,
      blocker: drift.blocker,
      next_action: drift.next_action,
    });
  }
  record = observeCompleted(record, host);
  const liveRuns = activeRuns(host, host.heads.next.commit);
  if (liveRuns.length > 0 && !drift.drift) {
    const current = liveRuns[0];
    if (current.workflow === SOURCE_PROOF_WORKFLOW) {
      record.phase = current.inputs?.acceptance_phase === "frozen_candidate"
        ? "frozen_candidate_acceptance"
        : "source_stabilization";
    }
  } else if (!drift.drift) {
    record.phase = laterPhase(record.phase, derivePhase(record));
  }
  if (record.phase === "closeout" || command === "status" && host.marketplace) {
    const groups = record.receipt.groups;
    if (record.phase === "closeout" && !groups["catalog-delivery"] && host.marketplace) {
      record = recordMarketplace(record, host);
    }
  }
  const intended = record.phase === "preflight" ? "source_stabilization" : record.phase;
  const blocked = preflight(record, host, intended);
  const assetBlock = record.phase === "publication" || record.lane === "plugin"
    ? assertAssetLane(record, host)
    : null;
  persist(host, record);
  return compact(record, {
    command,
    sha: host.heads.next.commit,
    tree: host.heads.next.tree,
    now: host.now().toISOString(),
    active_runs: liveRuns.map((run) => ({
      id: run.id,
      workflow: run.workflow,
      headSha: run.headSha,
      status: run.status,
    })),
    blocker: drift.blocker ?? blocked?.blocker ?? assetBlock?.blocker ?? null,
    next_action: liveRuns.length > 0 && !drift.residual?.length
      ? "wait for the in-flight permitted workflow"
      : (drift.next_action ?? blocked?.next_action ?? assetBlock?.next_action ?? nextActionFor(record.phase, record.rehearse)),
  });
}

async function advance(options, host) {
  const incoming = loadRecord(host, options).phase;
  const current = await observe(options, host, "advance");
  let record = current.record;
  if (current.blocker && /cancel|zombie|identity|runner|heartbeat|unproven|windows|staging|acceptance_phase/i.test(current.blocker)) {
    fail(current.blocker);
  }
  if (options.recordApproval) {
    if (!present(options.approver)) fail("--approver is required with --record-approval");
    record.approval = {
      approver: options.approver,
      at: host.now().toISOString(),
      recorded_on: `issue:${record.issue_number}`,
    };
    record.phase = "promotion";
    persist(host, record);
    return compact(record, {
      command: "advance",
      sha: host.heads.next.commit,
      tree: host.heads.next.tree,
      now: host.now().toISOString(),
    });
  }
  if (record.phase === "awaiting_approval") {
    if (incoming === "awaiting_approval" && !options.recordApproval) {
      fail("promotion and publication require recorded combined maintainer approval");
    }
    persist(host, record);
    return compact(record, {
      command: "advance",
      sha: host.heads.next.commit,
      tree: host.heads.next.tree,
      now: host.now().toISOString(),
      dispatched: null,
      next_action: nextActionFor("awaiting_approval", record.rehearse),
    });
  }
  if (record.phase === "publication" && record.lane === "native") {
    const assets = assertAssetLane(record, host);
    if (assets?.blocker) fail(assets.blocker);
  }
  const liveRuns = activeRuns(host, host.heads.next.commit);
  if (liveRuns.length > 0) {
    return compact(record, {
      command: "advance",
      sha: host.heads.next.commit,
      tree: host.heads.next.tree,
      now: host.now().toISOString(),
      active_runs: liveRuns,
      dispatched: null,
      next_action: "wait for the in-flight permitted workflow",
    });
  }
  if (optionalRuns(host).length > 0 && record.receipt.groups.qualification) {
    record.phase = record.approval ? "promotion" : "awaiting_approval";
    persist(host, record);
    return compact(record, {
      command: "advance",
      sha: host.heads.next.commit,
      tree: host.heads.next.tree,
      now: host.now().toISOString(),
      blocker: null,
    });
  }
  const intended = record.phase === "preflight" ? "source_stabilization" : record.phase;
  const blocked = preflight(record, host, intended);
  if (blocked) fail(blocked.blocker);
  if (record.phase === "complete") {
    return compact(record, { command: "advance", dispatched: null, now: host.now().toISOString() });
  }
  const result = dispatchFor(record, host);
  record = result.record;
  record.phase = result.phase;
  persist(host, record);
  return compact(record, {
    command: "advance",
    sha: host.heads.next.commit,
    tree: host.heads.next.tree,
    now: host.now().toISOString(),
    dispatched: result.dispatched,
    active_runs: activeRuns(host, host.heads.next.commit),
  });
}

export async function execute(argv, host) {
  const options = parseArgs(argv);
  host.operatorSuppliedRunIds ??= [];
  if (options.injectFailure) applyInjectedFailure(host, options.injectFailure);
  if (options.command === "start") return start(options, host);
  if (options.command === "status") return observe(options, host, "status");
  if (options.command === "advance") return advance(options, host);
  const status = await observe(options, host, "resume");
  if (status.blocker || status.active_runs.length > 0) return status;
  return advance(options, host);
}

function applyInjectedFailure(host, kind) {
  host.probes ??= {};
  if (kind === "runner-offline") host.probes.runners = { "macos-arm64-metal": "unproven" };
  if (kind === "macos-zombie") {
    host.probes.macosZombie = "zombie";
    host.probes.macosIdentity = "unreadable";
  }
  if (kind === "windows-staging") host.probes.windowsStaging = "fail";
  if (kind === "wrong-acceptance-mode") host.probes.acceptanceMode = "frozen_candidate";
  if (kind === "ignored-cancellation") host.cancelHonored = false;
}

function defaultProbes() {
  return {
    runners: {
      "macos-arm64-metal": "alive",
      "windows-x64-vulkan": "alive",
      "linux-x64-vulkan": "alive",
    },
    acceptanceMode: "source_stabilization",
    macosZombie: "clear",
    macosIdentity: "readable",
    windowsStaging: "ok",
    packaging: "ok",
    calibrationLineage: "ok",
  };
}

function fillReceipt(record, phase, host) {
  const C = host.heads.next.commit;
  const C_TREE = host.heads.next.tree;
  const generated = host.generatedFreezeHead ?? { commit: "f".repeat(40), tree: "2".repeat(40) };
  const F = generated.commit;
  const F_TREE = generated.tree;
  const success = (group, id, commit, tree) => runEvidence(group, { id, attempt: 1, conclusion: "success" }, commit, tree);
  if (!record.receipt.groups["pull-requests"]) {
    record.receipt = recordGroup(record.receipt, "pull-requests", {
      release_pr: null,
      bind: "next_head",
      integrated_support_prs: [],
    });
  }
  const completeWhenForced = new Set([
    "qualification",
    "awaiting_approval",
    "promotion",
    "publication",
    "closeout",
    "complete",
  ]);
  const need = (name) => {
    const target = PHASES.indexOf(phase);
    const current = PHASES.indexOf(name);
    return completeWhenForced.has(phase) ? current <= target : current < target;
  };
  if (need("source_stabilization") && !record.receipt.groups["source-stabilization"]) {
    record.receipt = recordGroup(record.receipt, "source-stabilization", success("source-stabilization", 100, C, C_TREE));
    record.receipt = recordGroup(record.receipt, "source-proof", success("source-proof", 105, C, C_TREE));
  }
  if (need("calibration") && !record.receipt.groups.calibration) {
    record.receipt = recordGroup(record.receipt, "calibration", [
      success("metal-1", 101, C, C_TREE),
      success("metal-2", 102, C, C_TREE),
      success("metal-3", 103, C, C_TREE),
    ]);
  }
  if (need("freeze") && !record.receipt.groups["frozen-candidate"]) {
    record.receipt = recordGroup(record.receipt, "frozen-candidate", { commit: F, tree: F_TREE });
    record.heads.F = F;
    record.heads.F_tree = F_TREE;
  }
  if (need("frozen_candidate_acceptance") && !record.receipt.groups["frozen-candidate-acceptance"]) {
    record.receipt = recordGroup(
      record.receipt,
      "frozen-candidate-acceptance",
      success("frozen-candidate-acceptance", 104, F, F_TREE),
    );
  }
  if (need("package") && !record.receipt.groups.package) {
    record.receipt = recordGroup(record.receipt, "package", success("package", 106, F, F_TREE));
  }
  if (need("hardware") && !record.receipt.groups.hardware) {
    record.receipt = recordGroup(record.receipt, "hardware", success("hardware", 107, F, F_TREE));
  }
  if (need("installed_candidate") && !record.receipt.groups["installed-candidate"]) {
    record.receipt = recordGroup(
      record.receipt,
      "installed-candidate",
      success("installed-candidate", 108, F, F_TREE),
    );
  }
  if (need("qualification") && !record.receipt.groups.qualification) {
    record.receipt = recordGroup(record.receipt, "qualification", success("qualification", 109, F, F_TREE));
  }
  if (need("publication") && !record.receipt.groups.promotion) {
    record.receipt = recordGroup(record.receipt, "promotion", {
      pull_request: 1999,
      approver: "rehearsal",
      approved_at: host.now().toISOString(),
    });
  }
  if (need("closeout") && !record.receipt.groups.publication && !record.rehearse) {
    record.receipt = recordGroup(record.receipt, "publication", {
      commit: F,
      tree: F_TREE,
      tag: `v${record.receipt.version}`,
      release_url: `https://github.com/TheGreenCedar/CodeStory/releases/tag/v${record.receipt.version}`,
      release_run: success("publication", 110, F, F_TREE),
    });
  }
  if (need("closeout") && !record.receipt.groups["pre-publish-ledger"]) {
    record.receipt = recordGroup(record.receipt, "pre-publish-ledger", {
      artifact: "pre-publish-ledger-attempt-1",
      digest: DIGEST,
    });
  }
}

export function createTestHost(overrides = {}) {
  const cloned = overrides.cloneFrom
    ? structuredClone({
      issues: overrides.cloneFrom.issues,
      comments: overrides.cloneFrom.comments,
      runs: overrides.cloneFrom.runs,
      dispatches: overrides.cloneFrom.dispatches,
      heads: overrides.cloneFrom.heads,
      probes: overrides.cloneFrom.probes,
      marketplace: overrides.cloneFrom.marketplace,
      assets: overrides.cloneFrom.assets,
      _record: overrides.cloneFrom._record,
      freezeBarrierCalls: overrides.cloneFrom.freezeBarrierCalls,
      cancelled: overrides.cloneFrom.cancelled,
      tags: overrides.cloneFrom.tags,
      cancelHonored: overrides.cloneFrom.cancelHonored,
      generatedFreezeHead: overrides.cloneFrom.generatedFreezeHead,
    })
    : null;
  const host = {
    now: () => new Date("2026-08-21T18:00:00Z"),
    heads: cloned?.heads ?? overrides.heads ?? {
      next: { commit: "c".repeat(40), tree: "1".repeat(40) },
      main: { commit: "0".repeat(40), tree: "9".repeat(40) },
    },
    probes: { ...defaultProbes(), ...(cloned?.probes ?? {}), ...(overrides.probes ?? {}) },
    issues: cloned?.issues ?? [],
    comments: cloned?.comments ?? [],
    runs: cloned?.runs ?? [],
    dispatches: cloned?.dispatches ?? [],
    cancelled: cloned?.cancelled ?? [],
    freezeBarrierCalls: cloned?.freezeBarrierCalls ?? [],
    tags: cloned?.tags ?? [],
    marketplace: cloned?.marketplace ?? overrides.marketplace ?? { state: "unpublished" },
    assets: cloned?.assets ?? overrides.assets ?? [...NATIVE_ASSETS],
    cancelHonored: cloned?.cancelHonored ?? overrides.cancelHonored ?? true,
    promotedToMain: false,
    ciGreen: false,
    operatorSuppliedRunIds: [],
    generatedFreezeHead: cloned?.generatedFreezeHead ?? overrides.generatedFreezeHead ?? {
      commit: "f".repeat(40),
      tree: "2".repeat(40),
    },
    _record: cloned?._record ?? null,
    _nextId: 500,
    _commentId: 1,
  };
  Object.defineProperty(host, "record", {
    get() {
      return host._record;
    },
    set(value) {
      host._record = value;
    },
  });
  host.workflowRuns = () => host.runs;
  host.createIssue = ({ title, body }) => {
    const issue = { number: 1997, title, body, state: "open" };
    host.issues.push(issue);
    return issue;
  };
  host.listOpenCoordinatorIssues = () => host.issues.filter((issue) =>
    issue.state === "open" && /coordinator/i.test(issue.title)
  );
  host.listIssueComments = () => host.comments;
  host.createIssueComment = (_number, body) => {
    const comment = { id: host._commentId, body };
    host._commentId += 1;
    host.comments.push(comment);
    return comment;
  };
  host.updateIssueComment = (id, body) => {
    const comment = host.comments.find((entry) => entry.id === id);
    if (comment) comment.body = body;
  };
  host.persist = () => {
    if (host._record) persist(host, host._record);
  };
  host.persistRecord = (record) => {
    host._record = record;
  };
  host.dispatch = ({ workflow, ref, inputs }) => {
    host._nextId += 1;
    const dispatched = {
      id: host._nextId,
      workflow,
      ref,
      inputs,
      headSha: inputs.expected_head_sha ?? host.heads.next.commit,
      status: "in_progress",
      conclusion: null,
      event: "workflow_dispatch",
      attempt: 1,
    };
    host.dispatches.push(dispatched);
    host.runs.push({ ...dispatched });
    return dispatched;
  };
  host.cancelRun = (id) => {
    if (host.cancelHonored) {
      host.runs = host.runs.filter((run) => run.id !== id);
      host.cancelled.push(id);
    }
  };
  host.freezeBarrier = (command, args) => {
    host.freezeBarrierCalls.push([command, args]);
  };
  host.completeActiveRuns = () => {
    for (const run of host.runs) {
      if (run.status === "in_progress") {
        run.status = "completed";
        run.conclusion = "success";
      }
    }
  };
  host.forcePhase = (phase) => {
    if (!host._record) fail("forcePhase requires a started coordinator record");
    fillReceipt(host._record, phase, host);
    host._record.phase = phase;
    persist(host, host._record);
  };
  host.promoteToMain = () => {
    host.promotedToMain = true;
  };
  host.createTag = (name) => {
    host.tags.push(name);
  };
  if (overrides.lane) host.lane = overrides.lane;
  return host;
}

function gh(args, options = {}) {
  return execFileSync("gh", args, {
    encoding: "utf8",
    input: options.input,
    stdio: options.input ? ["pipe", "pipe", "pipe"] : ["ignore", "pipe", "pipe"],
  }).trim();
}

export function createDefaultHost({ repository = "TheGreenCedar/CodeStory" } = {}) {
  return {
    now: () => new Date(),
    repository,
    probes: defaultProbes(),
    operatorSuppliedRunIds: [],
    get heads() {
      const next = gh(["api", `repos/${repository}/git/ref/heads/dev%2Fcodestory-next`]);
      const main = gh(["api", `repos/${repository}/git/ref/heads/main`]);
      const nextSha = JSON.parse(next).object.sha;
      const mainSha = JSON.parse(main).object.sha;
      const nextCommit = JSON.parse(gh(["api", `repos/${repository}/git/commits/${nextSha}`]));
      const mainCommit = JSON.parse(gh(["api", `repos/${repository}/git/commits/${mainSha}`]));
      return {
        next: { commit: nextSha, tree: nextCommit.tree.sha },
        main: { commit: mainSha, tree: mainCommit.tree.sha },
      };
    },
    runs: [],
    marketplace: { state: "unpublished" },
    assets: [...NATIVE_ASSETS],
    createIssue({ title, body }) {
      const created = JSON.parse(gh(
        ["api", "--method", "POST", `repos/${repository}/issues`, "--input", "-"],
        { input: JSON.stringify({ title, body }) },
      ));
      return { number: created.number, title: created.title };
    },
    listOpenCoordinatorIssues() {
      const rows = JSON.parse(gh([
        "issue",
        "list",
        "--repo",
        repository,
        "--state",
        "open",
        "--search",
        "coordinator in:title",
        "--json",
        "number,title,state",
      ]));
      return rows;
    },
    listIssueComments(number) {
      return JSON.parse(gh([
        "api",
        `repos/${repository}/issues/${number}/comments`,
      ]));
    },
    createIssueComment(number, body) {
      return JSON.parse(gh(
        ["api", "--method", "POST", `repos/${repository}/issues/${number}/comments`, "--input", "-"],
        { input: JSON.stringify({ body }) },
      ));
    },
    updateIssueComment(id, body) {
      return JSON.parse(gh(
        ["api", "--method", "PATCH", `repos/${repository}/issues/comments/${id}`, "--input", "-"],
        { input: JSON.stringify({ body }) },
      ));
    },
    dispatch({ workflow, ref, inputs }) {
      const args = ["workflow", "run", workflow, "--repo", repository, "--ref", ref];
      for (const [key, value] of Object.entries(inputs ?? {})) {
        if (value === undefined || value === null) continue;
        args.push("-f", `${key}=${value}`);
      }
      gh(args);
      return { id: Date.now(), workflow, ref, inputs, status: "queued" };
    },
    workflowRuns() {
      const rows = JSON.parse(gh([
        "run",
        "list",
        "--repo",
        repository,
        "--limit",
        "30",
        "--json",
        "databaseId,headSha,status,conclusion,event,workflowName,attempt",
      ]));
      const workflowPath = (name) => {
        if (name === "Exact-head source proof") return SOURCE_PROOF_WORKFLOW;
        if (name === "Platform and integration proof") return PACKAGED_WORKFLOW;
        if (name === "Release") return RELEASE_WORKFLOW;
        return name;
      };
      return (rows ?? []).map((row) => ({
        id: row.databaseId,
        workflow: workflowPath(row.workflowName),
        headSha: row.headSha,
        status: row.status,
        conclusion: row.conclusion,
        event: row.event,
        attempt: row.attempt ?? 1,
      }));
    },
    cancelRun(id) {
      gh(["run", "cancel", String(id), "--repo", repository]);
    },
  };
}

function printStatus(result) {
  const lines = [
    `phase: ${result.phase}`,
    `sha: ${result.sha}`,
    `tree: ${result.tree}`,
    `active_runs: ${result.active_runs.length}`,
    `blocker: ${result.blocker ?? "none"}`,
    `next: ${result.next_action}`,
    `elapsed: ${result.elapsed}`,
    `critical_path: ${result.estimated_critical_path}`,
    `rehearse: ${result.rehearse}`,
  ];
  process.stdout.write(`${lines.join("\n")}\n`);
}

async function main() {
  const result = await execute(process.argv.slice(2), createDefaultHost());
  printStatus(result);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((error) => {
    process.stderr.write(`codestory-release: ${error.message}\n`);
    process.exitCode = 1;
  });
}
