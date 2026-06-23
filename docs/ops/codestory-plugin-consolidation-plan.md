# Plan: CodeStory Plugin Consolidation

**Generated**: 2026-06-23
**Estimated Complexity**: Medium

## Overview

Session `019ef232-1fc0-72a1-a0e4-8e2ea03e8835` used the CodeStory plugin in
`C:\Users\alber\OneDrive\Documents\TEC_TechnicalTest`. The plugin did provide
usable local graph evidence: `ground` and `files` returned an indexed 23-file
repo. The failure was the surrounding workflow:

- The agent treated "deep audit" as permission to edit, applied two patches, and
  then had to revert them after the user clarified that audit did not mean edit.
- The agent called `ground` and `files` before reading `codestory://status`.
  The skill says to read status first, but the runtime did not make the trust
  boundary hard to miss.
- CodeStory status later showed split readiness: local navigation was usable
  after an index refresh, but packet/search remained blocked because sidecar
  retrieval was unavailable or stale. That was correct, but noisy for an audit
  that only needed local navigation.
- The accidental edits dirtied the index and forced an avoidable
  `codestory-cli index --refresh incremental` repair.
- The forensic pass started from a release branch, but implementation moved to
  `dev/codestory-next`, where `codestory-cli` and the plugin package are
  `0.11.6`. On this machine, `where.exe codestory-cli` currently resolves
  `C:\Users\alber\.local\bin\codestory-cli.exe` first and that binary also
  reports `0.11.6`; a `target\release\codestory-cli.exe` exists too. The
  plugin's `.mcp.json` launches
  raw `codestory-cli` from `PATH`, so runtime truth can diverge from source and
  plugin package truth.

The consolidation goal is not "more instructions". It is one runtime truth
surface that tells an agent what is safe to use now, plus small guardrails so a
local-only audit does not fall into packet/search repair work or apply-mode
churn.

## Prerequisites

- Keep `local_navigation` and `agent_packet_search` as separate readiness lanes.
- Do not weaken the `retrieval_mode=full` requirement for packet/search.
- Keep MCP tools read-only.
- Treat CLI version/currentness, MCP registration, index freshness, and sidecar
  readiness as separate gates.

## Sprint 1: Pin The Bad Loop

**Goal**: Add tests that fail when the plugin presents stale or over-broad next
steps to agents.

**Demo/Validation**:

- Run focused stdio/readiness tests.
- Confirm degraded sidecars still allow local navigation guidance without
  recommending packet/search as usable.

### Task 1.1: Add A Stdio Status Contract For Allowed Surfaces

- **Location**:
  - `crates/codestory-cli/src/stdio_transport.rs`
  - `crates/codestory-cli/tests/stdio_protocol_contracts.rs`
- **Description**: Extend `codestory://status` with a compact
  `allowed_surfaces` object derived from existing readiness verdicts:
  local graph surfaces such as
  `ground/files/symbol/definition/trail/snippet/references/affected/get_node/neighbors/shortest_path/query_subgraph/symbols`
  allowed when `local_navigation=ready`; `packet/search/context` allowed only
  when `agent_packet_search=ready`.
- **Dependencies**: None.
- **Acceptance Criteria**:
  - Fresh index plus unavailable sidecar reports local navigation allowed and
    packet/search/context blocked.
  - Full sidecar reports packet/search/context allowed.
  - The field is generated from `crate::readiness`, not hand-coded in the stdio
    resource.
- **Validation**:
  - `cargo test -p codestory-cli --test stdio_protocol_contracts`

### Task 1.2: Stop The Static Agent Guide From Listing Packet/Search As A Normal Next Step

- **Location**:
  - `crates/codestory-cli/src/stdio_transport.rs`
  - `crates/codestory-cli/tests/stdio_protocol_contracts.rs`
- **Description**: Change `codestory://agent-guide` from one unconditional
  sequence to two lanes: "always read status and ground when local navigation is
  ready" and "only use packet/search when status allows them".
- **Dependencies**: Task 1.1.
- **Acceptance Criteria**:
  - Agent guide still names `packet` and `search`, but only under a readiness
    condition.
  - Recommended calls in `codestory://status` are the dynamic source of truth.
- **Validation**:
  - `cargo test -p codestory-cli --test stdio_protocol_contracts`

### Task 1.3: Add Trust Summary To Local Navigation Outputs

- **Location**:
  - `crates/codestory-cli/src/stdio_transport.rs`
  - `crates/codestory-cli/tests/stdio_protocol_contracts.rs`
- **Description**: Add a small `trust` or `readiness_summary` field to `ground`
  and `files` outputs so an agent that calls them before status still sees:
  local navigation status, packet/search status, and the first minimum repair
  command.
- **Dependencies**: Task 1.1.
- **Acceptance Criteria**:
  - `ground` and `files` remain useful as compact read-only data.
  - Degraded sidecars are visible without implying local navigation is unusable.
- **Validation**:
  - `cargo test -p codestory-cli --test stdio_protocol_contracts`

## Sprint 2: Make Runtime Version Truth Visible

**Goal**: Make the active MCP binary self-report enough evidence that agents do
not have to guess which `codestory-cli` they are using.

**Demo/Validation**:

- Start `codestory-cli serve --stdio --refresh none`.
- Read `codestory://status`.
- Confirm it reports the serving binary version and executable path.

### Task 2.1: Add Server Version And Executable Path To Status

- **Location**:
  - `crates/codestory-cli/src/stdio_transport.rs`
  - `crates/codestory-cli/tests/stdio_protocol_contracts.rs`
- **Description**: Add `server_version` and `server_executable` to
  `codestory://status` using the current package version and
  `std::env::current_exe()`.
- **Dependencies**: None.
- **Acceptance Criteria**:
  - Status identifies the exact server binary used by MCP.
  - Path failures degrade to an explicit warning field instead of omitting the
    evidence.
- **Validation**:
  - `cargo test -p codestory-cli --test stdio_protocol_contracts`

### Task 2.2: Keep The Plugin Path Rule, But Make Drift Obvious

- **Location**:
  - `plugins/codestory/.mcp.json`
  - `plugins/codestory/README.md`
  - `plugins/codestory/skills/codestory-grounding/SKILL.md`
  - `plugins/codestory/tests/plugin-static.test.mjs`
- **Description**: Keep `.mcp.json` launching `codestory-cli` from `PATH`, but
  update docs and skill text to tell agents to trust `server_version` and
  `server_executable` from `codestory://status` once MCP is live. Use
  `where.exe codestory-cli` and `codestory-cli --version` only when MCP is
  missing or needs repair.
- **Dependencies**: Task 2.1.
- **Acceptance Criteria**:
  - Plugin docs no longer make every normal run walk the full latest-release
    install path.
  - Static tests preserve the key facts: PATH launch, MCP restart boundary, and
    stale binary diagnosis.
- **Validation**:
  - `node plugins/codestory/tests/plugin-static.test.mjs`

## Sprint 3: Cut Duplicate Guidance

**Goal**: Make the skill, README, docs, and status resource agree without
repeating a long repair playbook in every place.

**Demo/Validation**:

- A reviewer can start from the README, the skill, or `codestory://agent-guide`
  and see the same order: status, ground, then conditional packet/search.

### Task 3.1: Shorten The CodeStory Grounding Skill

- **Location**:
  - `plugins/codestory/skills/codestory-grounding/SKILL.md`
  - `plugins/codestory/skills/codestory-grounding/references/doctor.md`
  - `plugins/codestory/skills/codestory-grounding/references/serve.md`
- **Description**: Move detailed install and sidecar repair prose into the
  specific references. Keep `SKILL.md` to the normal loop and the fallback rule:
  if MCP is live, read status and obey `allowed_surfaces`; if MCP is missing,
  use CLI repair.
- **Dependencies**: Sprint 1 and Sprint 2.
- **Acceptance Criteria**:
  - Main skill is shorter and operational.
  - It does not duplicate every release asset name unless the setup reference is
    being used.
- **Validation**:
  - `node plugins/codestory/tests/plugin-static.test.mjs`

### Task 3.2: Align Public Docs With The Same Contract

- **Location**:
  - `plugins/codestory/README.md`
  - `docs/usage.md`
  - `docs/ops/retrieval-sidecars.md`
- **Description**: Replace repeated readiness explanations with one table:
  `local_navigation` permits local browse tools; `agent_packet_search` permits
  packet/search only with `retrieval_mode=full`; status tells the current truth.
- **Dependencies**: Task 3.1.
- **Acceptance Criteria**:
  - No doc implies packet/search readiness from successful `ground`.
  - No doc tells agents to rebuild sidecars for a task that only needs local
    navigation.
- **Validation**:
  - `rg -n "retrieval_mode=full|local_navigation|agent_packet_search" plugins docs`
  - `node plugins/codestory/tests/plugin-static.test.mjs`

## Sprint 4: Dogfood The Fixed Loop

**Goal**: Prove the new loop avoids the session failure mode.

**Demo/Validation**:

- Use a repo with a fresh local index and unavailable sidecars.
- Ask for a read-only audit using CodeStory.
- Expected behavior: status first, local graph surfaces allowed,
  packet/search/context blocked, no sidecar rebuild unless the user asks for a
  sidecar-backed surface, no edits.

### Task 4.1: Add A Maintainer Dogfood Checklist

- **Location**:
  - `docs/ops/codestory-plugin-dogfood-checklist.md`
- **Description**: Add a short checklist for a plugin smoke run:
  active binary, status, allowed surfaces, local-only audit behavior, and final
  git status.
- **Dependencies**: Sprints 1-3.
- **Acceptance Criteria**:
  - Checklist is under one page.
  - It includes exact commands and expected readiness outcomes.
- **Validation**:
  - Manual checklist execution in a target repo.

### Task 4.2: Run Focused CodeStory Verification

- **Location**: Repository root.
- **Description**: Run the smallest useful command set after implementation.
- **Dependencies**: Sprints 1-3.
- **Acceptance Criteria**:
  - Stdio/readiness tests pass.
  - Plugin static docs contract passes.
  - Release metadata check still passes for the active version.
- **Validation**:
  - `cargo test -p codestory-cli --test ready_command`
  - `cargo test -p codestory-cli --test stdio_protocol_contracts`
  - `node plugins/codestory/tests/plugin-static.test.mjs`
  - `python .github/scripts/check-codestory-release.py --version 0.11.6`
  - `git diff --check`

## Out Of Repo Follow-Up

- Update `C:\Users\alber\.codex\skills\code-simplifier\SKILL.md` so "audit",
  "audit pass", "deep audit", and "review" are audit-only unless the user
  explicitly says to edit, apply, implement, commit, or open a PR.
- Keep this outside the CodeStory repo unless the user asks to update global
  skills or memory.

## Risks

- Adding readiness fields can become another duplicated schema if they are not
  derived from `crate::readiness`.
- Over-warning on sidecars can still make local-only tasks feel broken. The UI
  copy must say "packet/search/context blocked" instead of "CodeStory blocked"
  when local navigation is ready.
- Server executable paths are local machine evidence. They must not be committed
  into public docs or golden strings.

## Rollback Plan

- Revert stdio status/guide changes and docs together.
- Keep any tests that describe the desired trust boundary if they still fail for
  a useful reason.
- If runtime version reporting proves flaky on one platform, keep
  `server_version` and downgrade `server_executable` to best-effort warning
  evidence.
