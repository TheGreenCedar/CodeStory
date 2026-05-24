# Internal Plan: Make CodeStory An LLM's Default Codebase Browser

**Generated**: 2026-05-06
**Estimated complexity**: High
**Status**: historical planning artifact; status reviewed on 2026-05-24

## Overview

CodeStory already has the core substrate for an agent-facing codebase browser:
local indexing, a SQLite-backed symbol/edge graph, semantic docs, grounding
snapshots, search, symbol inspection, trails, snippets, DB-first `context`, a TUI
`explore` path, HTTP routes, and MCP-style stdio serving.

The next product step is not another isolated command. It is making those
primitives act like one browsing layer that an LLM can use
before reaching for ad hoc file reads.

The reviewed direction is:

1. Fix unsafe or drifting contracts first.
2. Add fast always-on browser-path tests.
3. Cleanly separate read-only browser services from CLI transports.
4. Make stdio/MCP compatibility explicit and testable.
5. Improve retrieval quality with a bounded target-context mode.
6. Add freshness, setup, and performance trust signals.
7. Improve the existing `explore` and evidence UX before creating a new UI surface.

## Current State

CodeStory's durable promise is strong:

- `codestory-cli index` builds graph state, snapshots, lexical search state, and semantic docs.
- Read commands default to `--refresh none`, which is the right posture for agent loops over a known cache.
- `ground`, `search --why`, `context`, `symbol`, `trail`, `snippet`, `query`, `explore`, `doctor`, and `serve` already cover most browser primitives.
- `serve --stdio` exposes tools, resources, resource templates, and prompts.
- The architecture docs and contract tests preserve the intended crate split.
- The repo-scale e2e stats gate already measures index/search/symbol/trail/snippet behavior.

This plan is retained as design history, not as the current delivery backlog.
The status of the original limitations is:

| Area | Status | Current note |
| --- | --- | --- |
| DB-first browser contract | Completed, guarded | High-level retrieval no longer carries local external-agent execution controls; architecture and onboarding contracts protect the read-only boundary. |
| `.codestory.toml` embedding mapping | Completed | `embedding_profile` and `embedding_model_id` map to runtime env names; legacy `embedding_model` remains a deprecated alias. |
| Repo-local grounding skill refs | Completed, guarded | Command refs exist for the browser surfaces and onboarding tests check required reference shape. |
| Trail-only DOT output | Completed, guarded | CLI help and command contracts keep DOT scoped to trail output. |
| Fast browser golden path | Completed | `cli_golden_path` covers the small always-on index-then-browse loop. |
| HTTP/stdio schema generation | Still open | Tool schemas and prompts are still handwritten in the CLI. |
| `context` packet quality | Superseded by packet/search-plan work | `packet`, `search --why`, and structured follow-up commands now carry more of the agent handoff path. |
| Freshness/profile mismatch signals | Partly complete | `doctor` and read outputs report retrieval/freshness state; continue improving where review evidence shows ambiguity. |
| Large-repo performance evidence | Still open | Repo-scale and public-core rows exist, but 10k-100k file agent-loop evidence remains future work. |

## Sprint 0: Safety, Drift, And Fast Tripwires

**Goal**: make the current browser surface safer and harder to regress before adding new product behavior.

**Demo/validation**

- `cargo test -p codestory-contracts`
- `cargo test -p codestory-cli --test onboarding_contracts`
- `cargo test -p codestory-cli --test architecture_contracts`
- `cargo test -p codestory-cli --test cli_golden_path`
- `cargo test -p codestory-cli --test cli_error_contracts`

### Task 0.1: Keep Context DB-First Everywhere

- **Location**: `crates/codestory-contracts/src/api/dto.rs`, `crates/codestory-runtime/src/agent/orchestrator.rs`, CLI context tests.
- **Description**: keep `AgentAskRequest` as a retrieval-only contract with no external local-agent execution controls.
- **Acceptance criteria**:
  - CLI `context` exposes no local-agent flags.
  - `serve --stdio` context remains read-only and DB-first.
  - Retrieval trace contains no local-agent execution step.
- **Validation**:
  - Add CLI/stdin tests proving context output has only retrieval-owned trace steps.

### Task 0.2: Fix `.codestory.toml` Embedding Config Mapping

- **Location**: `crates/codestory-cli/src/config.rs`, `README.md`, `docs/architecture/subsystems/cli.md`, `docs/contributors/getting-started.md`.
- **Description**: make config keys map to runtime env names that actually control embeddings.
- **Recommended shape**:
  - Add `embedding_profile` -> `CODESTORY_EMBED_PROFILE`.
  - Add `embedding_model_id` -> `CODESTORY_EMBED_MODEL_ID`.
  - Keep legacy `embedding_model` as a deprecated alias for `embedding_model_id`.
  - Stop setting only `CODESTORY_EMBEDDING_MODEL` unless runtime starts reading it.
- **Acceptance criteria**:
  - A copy-paste `.codestory.toml` example changes `doctor` output predictably.
  - Docs explain precedence: explicit env vars win over config defaults; project config overrides home config.
  - A config test covers profile, model id, legacy alias, and explicit env override behavior.
- **Validation**:
  - `cargo test -p codestory-cli config`
  - `codestory-cli setup embeddings --project . --dry-run --format json`
  - `codestory-cli doctor --project . --format json` in hash-mode, missing-managed-assets, and missing-llama modes.

### Task 0.3: Repair Agent-Facing Skill Docs

- **Location**: `.agents/skills/codestory-grounding/SKILL.md`, `.agents/skills/codestory-grounding/references/`.
- **Description**: make the repo-local skill the canonical operational guide for agents.
- **Acceptance criteria**:
  - Remove stale crate names: `codestory-app`, `codestory-index`, `codestory-storage`.
  - Add command refs for `context.md`, `doctor.md`, `explore.md`, and `serve.md`.
  - Add a short "LLM default browser loop":
    `doctor` -> `index` when needed -> `ground` -> `search --why` -> `symbol/trail/snippet/explore` -> `context` with citations.
  - Each command ref includes one normal path, one failure path, and one integration edge.
- **Validation**:
  - Skill metadata validation with `quick_validate.py`.
  - Add or extend CLI docs contract tests so command refs cannot drift silently.

### Task 0.4: Add Fast Browser Golden Path

- **Location**: `crates/codestory-cli/tests/cli_golden_path.rs`.
- **Description**: add a tiny always-on temp Rust workspace test that proves the core browser loop without indexing the full repo.
- **Fixture**:
  - `src/lib.rs` with `AppController`, `open_project`, `run_indexing`.
  - `src/runtime.rs` with a cross-file call.
  - isolated `--cache-dir`.
  - deterministic `CODESTORY_EMBED_RUNTIME_MODE=hash`.
- **Acceptance criteria**:
  - `index --refresh full --format json` succeeds.
  - `doctor`, `ground`, `search`, `symbol`, `trail`, `snippet`, and `query` work with `--refresh none`.
  - Read commands do not mutate the search directory.
- **Validation**:
  - `cargo test -p codestory-cli --test cli_golden_path`.

### Task 0.5: Add Error Contract Tests

- **Location**: `crates/codestory-cli/tests/cli_error_contracts.rs`.
- **Description**: make common agent-facing failures actionable and stable.
- **Acceptance criteria**:
  - Read command without cache exits nonzero and includes a recovery command.
  - Ambiguous query lists ranked alternatives or exact next steps.
  - Missing output parent fails before runtime mutation.
  - Non-`trail` `--format dot` is either absent from help or rejected by a tested pre-runtime error.
- **Validation**:
  - `cargo test -p codestory-cli --test cli_error_contracts`.

## Sprint 1: Architecture Boundary Cleanup

**Goal**: create a safe read-only browser boundary before publishing richer protocol metadata or UI surfaces.

**Demo/validation**

- `cargo test -p codestory-cli --test architecture_contracts`
- `cargo test -p codestory-store`
- targeted full/incremental refresh tests
- `cargo check --all-targets`

### Task 1.1: Move Refresh/Inventory Contracts Out Of Workspace Coupling

- **Location**: `crates/codestory-contracts`, `crates/codestory-workspace`, `crates/codestory-store`, `crates/codestory-indexer`, `crates/codestory-runtime`.
- **Description**: stop `codestory-store` from depending on `codestory-workspace` types.
- **Acceptance criteria**:
  - `codestory-store` no longer depends on `codestory-workspace`.
  - Neutral refresh/inventory value types live in `codestory-contracts` or a tiny internal planning module.
  - `index --dry-run`, full index, and incremental index preserve current `files_to_index` / `files_to_remove` behavior.
- **Validation**:
  - Add architecture test asserting store does not depend on workspace.
  - Run existing workspace, store, indexer, and runtime incremental tests.

### Task 1.2: Introduce A Runtime Read-Only Browser Service

- **Location**: `crates/codestory-runtime/src/services.rs`, new runtime module such as `crates/codestory-runtime/src/browser.rs`.
- **Description**: gather read-only browser operations behind a runtime-owned service used by CLI, HTTP, stdio, and future UI.
- **Initial operations**:
  - `search`
  - `symbol`
  - `definition`
  - `references`
  - `symbols`
  - `trail`
  - `snippet`
  - `query`
  - DB-first `context` packet
- **Non-goals**:
  - Do not include file writes, opening IDEs, opening folders, or OS actions.
  - Do not move socket/stdin transport loops into runtime.
- **Acceptance criteria**:
  - CLI transport stays thin.
  - Read-only capability boundary is explicit.
  - Existing response shapes remain compatible.
- **Validation**:
  - Architecture tests that CLI does not construct browser business logic.
  - HTTP/stdin regression tests for route/tool names and core JSON shapes.

## Sprint 2: Protocol And Agent Integration Contracts

**Goal**: make CodeStory's agent integration stable, discoverable, and safe for automatic use.

**Demo/validation**

- JSON-lines transcript tests for `serve --stdio`
- HTTP parity smoke for `/search`, `/definition`, `/references`, `/symbols`, `/trail`
- `cargo test -p codestory-cli`

### Task 2.1: Add Stdio Transcript Compatibility Tests First

- **Location**: `crates/codestory-cli/tests/stdio_protocol_contracts.rs`.
- **Description**: test current and intended JSON-RPC/MCP-style behavior before changing metadata.
- **Acceptance criteria**:
  - `initialize` preserves request `id` and reports server info/capabilities.
  - Unknown method, invalid JSON, bad args, and not-found errors return stable JSON-RPC-shaped errors.
  - `tools/list`, `resources/list`, `resources/templates/list`, `prompts/list`, `resources/read`, and `tools/call` have transcript fixtures.
- **Validation**:
  - `cargo test -p codestory-cli --test stdio_protocol_contracts`.

### Task 2.2: Create A Typed Tool/Resource/Prompt Catalog

- **Location**: runtime read-only service or a small transport-neutral module; CLI renders it.
- **Description**: replace handwritten loose schema generation with a single manifest/catalog.
- **Acceptance criteria**:
  - Tool names remain stable.
  - Input schemas include required fields, enum values, defaults, and bounds.
  - Output schemas exist for core tools where stable DTOs already exist.
  - Future write/system tools cannot appear in the read-only catalog without explicit safety metadata.
- **Validation**:
  - Snapshot tests for tool/resource/prompt catalog.
  - Tests comparing catalog command list to browser service operations.

### Task 2.3: Add Safety Metadata And Resource Links

- **Location**: catalog/rendering module, stdio result wrappers.
- **Description**: make safe automatic use easy for agents.
- **Acceptance criteria**:
  - All read-only tools include annotations such as read-only, non-destructive, idempotent, and local-only/open-world false where supported.
  - `search` and `definition` results expose `codestory://symbol/{node_id}`, snippet, references, and trail links.
  - `codestory://status` reports project root, cache path, retrieval mode, semantic readiness, fallback reason, and recommended next calls.
  - `codestory://agent-guide` describes the default browser loop.
- **Validation**:
  - `tools/list` snapshot asserts annotations.
  - Resource read tests for `status` and `agent-guide`.
  - Tool call tests assert continuation links and payload-size limits.

### Task 2.4: Keep HTTP And Stdio Aligned

- **Location**: CLI transport layer, shared route/tool descriptors.
- **Description**: prevent route defaults from diverging between HTTP and stdio.
- **Acceptance criteria**:
  - `/definition`, `/references`, `/symbols`, `/trail` share default limits/depth semantics with stdio tools.
  - Existing HTTP paths remain stable.
- **Validation**:
  - Handler descriptor tests.
  - One HTTP smoke against an indexed temp repo.

## Sprint 3: Retrieval Quality And Target Context

**Goal**: make `context` gather deep evidence around concrete integration and architecture anchors instead of hoping a single search query hits.

**Demo/validation**

- `cargo test -p codestory-runtime --test retrieval_eval`
- new retrieval golden tests
- CLI `context` JSON/Markdown snapshot tests

### Task 3.1: Build Retrieval Golden Fixtures Before Changing Context

- **Location**: `crates/codestory-runtime/tests/retrieval_browser_contracts.rs`.
- **Description**: create deterministic fixtures for the browser investigations CodeStory must ground.
- **Cases**:
  - exact symbol query
  - exact file/literal query
  - broad integration question decomposed into concrete search anchors
  - ambiguous symbol requiring alternatives
  - graph/snippet expansion
  - stale index warning
  - no-hit query with suggestions and explicit gaps
- **Acceptance criteria**:
  - Tests assert citations, selected focus, trace steps, and gap reporting.
  - Hash embedding mode gives deterministic results.
- **Validation**:
  - `cargo test -p codestory-runtime --test retrieval_browser_contracts`.

### Task 3.2: Make Bounded Context The Default

- **Location**: `crates/codestory-cli/src/args.rs`, `crates/codestory-contracts/src/api/dto.rs`, `crates/codestory-runtime/src/agent/orchestrator.rs`, `crates/codestory-runtime/src/agent/profiles.rs`.
- **Description**: make the deep retrieval path the default for `context`, with no public lightweight/deep split.
- **Behavior**:
  - Initial search with current ranking.
  - Query expansion or exact-symbol/file fallback when first hits are weak.
  - Bounded graph expansion.
  - Bounded snippet/source reads.
  - Citations and "what I checked" trace.
  - Explicit gaps when confidence is low.
- **Hard limits**:
  - Respect latency budget before expensive trail/source phases.
  - Cap default trail nodes and source bytes.
  - Keep investigation inside CodeStory's indexed retrieval layer.
- **Acceptance criteria**:
  - Integration targets that currently miss relevant symbols return cited hits.
  - Trace proves multiple retrieval steps only when needed.
  - `context` stays target-first and does not accept broad question prompts.
- **Validation**:
  - Golden target tests.
  - `context --format json` trace assertions.
  - Warm latency checks under the performance thresholds.

### Task 3.3: Improve Target Resolution UX

- **Location**: `crates/codestory-cli/src/runtime.rs`, target selection DTOs, CLI renderers.
- **Description**: reduce ambiguous-query dead ends.
- **Acceptance criteria**:
  - Ambiguous results include numbered alternatives and stable node refs.
  - Add `--choose <N>` or equivalent only if it can be made deterministic without hidden session state.
  - JSON includes enough data for agents to resolve by id on the next call.
- **Validation**:
  - Ambiguous symbol CLI tests.
  - No silent auto-pick when ranks tie.

### Task 3.4: Redesign Evidence Packet Output

- **Location**: `crates/codestory-cli/src/output.rs`, context renderers, search/ground explanations.
- **Description**: make Markdown outputs easier for humans and LLMs to consume.
- **Suggested structure**:
  - context summary or short finding
  - confidence
  - what was checked
  - gaps/uncertainty
  - citations
  - next useful commands
- **Acceptance criteria**:
  - Full trace remains in JSON/bundles.
  - Markdown never hides fallback reasons or low-confidence state.
- **Validation**:
  - Snapshot tests for `context`, `search --why`, and `ground --why`.

## Sprint 4: Operational Trust And Performance Evidence

**Goal**: expose the state that agents need to know: freshness, retrieval readiness, semantic profile, and warm-loop performance.

**Demo/validation**

- `doctor` reports useful cache/profile/fallback/freshness state.
- warm stdio benchmark produces p50/p95/p99.
- repo-scale e2e stats remain the promotion gate.

### Task 4.1: Add Embedding Profile Contract And Doctor Warnings

- **Location**: `crates/codestory-runtime/src/search/engine.rs`, semantic doc metadata, `doctor` DTO/output.
- **Description**: represent embedding profile/backend/doc-shape as a stable runtime contract.
- **Acceptance criteria**:
  - Stored semantic docs report profile/model/backend/dimension/doc-shape enough to explain reuse or rebuild.
  - `doctor` warns when stored docs and current env/config disagree.
  - Missing managed ONNX assets and external legacy llama.cpp endpoint failures remain clear fallbacks, not silent degradation.
- **Validation**:
  - hash backend normal path
  - fake llama.cpp path
  - missing endpoint failure path
  - profile mismatch warning

### Task 4.2: Add Index Freshness Signal

- **Location**: runtime project/search/context DTOs, workspace inventory check, `doctor`, `serve --stdio` status resource.
- **Description**: make stale caches visible without mutating read commands.
- **Acceptance criteria**:
  - Freshness check is bounded and read-only.
  - It reports changed/new/removed counts or "not checked" with reason.
  - Read commands do not refresh implicitly.
- **Validation**:
  - Temp fixture where a file changes after indexing.
  - Freshness p95 under 250 ms for small repos.

### Task 4.3: Measure Warm `serve --stdio` Agent Loop

- **Location**: CLI stdio test harness or bench, `docs/testing/codestory-e2e-stats-log.md` or a new warm-loop stats doc.
- **Description**: measure the actual persistent-session shape agents should use.
- **Metrics**:
  - startup ms
  - first tool ms
  - warm p50/p95/p99 per tool
  - response bytes
  - semantic reload ms
  - fallback reason
  - search dir unchanged
- **Acceptance criteria**:
  - Metrics do not pollute stdout protocol.
  - Initial report compares warm stdio to cold one-shot CLI timings.
- **Validation**:
  - transcript: initialize -> tools/list -> search -> symbol -> trail -> snippet -> resources/read.

### Task 4.4: Add Hard Caps Before Bigger Bundles

- **Location**: repo-text search, context retrieval, future bundle/context tools.
- **Description**: reduce large-repo footguns before introducing higher-level bundle tools.
- **Caps**:
  - repo-text scanned files/bytes/time
  - bundle output bytes
  - default context trail nodes
  - source snippet bytes
- **Acceptance criteria**:
  - Truncation is explicit and actionable.
  - Caps are visible in `--why`, JSON, or retrieval trace.
- **Validation**:
  - Large low-match repo-text fixture.
  - High-fanout trail fixture.

### Task 4.5: Add Stress Lanes Only After Metrics Exist

- **Location**: `crates/codestory-bench`.
- **Description**: create large-repo stress benches after the warm-loop counters are stable.
- **Scenarios**:
  - 1k, 10k, 100k synthetic file sets
  - high-degree graph nodes
  - repo-text `auto/on/off`
  - trail depths 2/4/6
  - stdio/HTTP concurrency 1/4/16
- **Acceptance criteria**:
  - Promotion thresholds documented.
  - Synthetic results are not treated as real-world proof without at least one real repo run.

## Sprint 5: Delight UX On The Existing Surface

**Goal**: improve the existing browser flow without creating duplicate UI surfaces prematurely.

**Demo/validation**

- `explore` flow improves for keyboard-first navigation.
- No new `browse` command until its distinction from `explore` is clear.
- Accessibility and text-equivalent review for any graph-heavy UI.

### Task 5.1: Improve `explore` Before Adding `browse`

- **Location**: `crates/codestory-cli/src/main.rs`, explore rendering/TUI modules if split.
- **Description**: evolve the current TUI into the default browser path.
- **Acceptance criteria**:
  - Project/status pane shows retrieval mode, fallback, freshness, and next useful command.
  - Search/results/detail/trail/snippet panes are keyboard reachable.
  - Empty/error states preserve the failed layer: cache, index, semantic runtime, query resolution, output write.
- **Validation**:
  - Keyboard-only TUI pass.
  - JSON/Markdown fallback pass with `--no-tui`.

### Task 5.2: Add Bookmarks As Investigation State

- **Location**: existing bookmark store/runtime surfaces, CLI commands or explore actions.
- **Description**: expose saved focus sets for repeated investigations.
- **Acceptance criteria**:
  - Add/list/remove bookmarks.
  - `context` or `trail` can use bookmark context if explicitly requested.
  - Stale bookmarks after reindex degrade gracefully.
- **Validation**:
  - CRUD tests.
  - Reindex stale-node behavior.

### Task 5.3: Add Trail Story Mode

- **Location**: trail renderers and runtime trail DTOs.
- **Description**: provide a readable narrative of graph paths.
- **Acceptance criteria**:
  - Entry points, core flow, side effects, uncertain edges, and tests included/excluded are explicit.
  - Uncertainty is textual, not only color or graph styling.
- **Validation**:
  - Trail fixtures with certain/probable/speculative edges.
  - Markdown snapshot tests.

### Task 5.4: Defer Web Cockpit Until Contracts Are Stable

- **Description**: only add a separate web UI after read-only service, protocol catalog, status/freshness, and warm-loop telemetry are stable.
- **Acceptance criteria for starting web work**:
  - Tool/resource manifest stable.
  - Warm p95 thresholds are met.
  - Existing `explore` experience proves the browser workflow.
  - Screenshot-visible review loop is planned before implementation.

## Suggested First Three PRs

### PR 1: Trust Foundations

- Fix the high-level retrieval request default.
- Add serde omission tests.
- Fix `.codestory.toml` embedding config mapping.
- Add config precedence tests.
- Update README/CLI docs for config mapping.

### PR 2: Agent Docs And Fast Browser Tests

- Repair `codestory-grounding` skill freshness rules.
- Add `context`, `doctor`, `explore`, `serve` refs.
- Add command-reference drift tests.
- Add `cli_golden_path.rs`.
- Add `cli_error_contracts.rs`.

### PR 3: Read-Only Browser Boundary

- Move refresh/inventory shared types out of workspace-store coupling.
- Add architecture guard for store not depending on workspace.
- Introduce `ReadOnlyBrowserService`.
- Keep CLI transport loops in CLI.
- Preserve route/tool names and response shapes.

## Review Risks

- **Protocol overreach**: do not freeze a rich manifest until service boundaries are clean.
- **UI duplication**: improve `explore` first; defer `browse` and a separate web UI.
- **Latency waterfall**: deep `context` must be budgeted before graph/source phases.
- **Repo-text I/O**: add global caps before repo-text participates in high-level bundles.
- **Config churn**: support legacy `embedding_model` while introducing precise `embedding_profile` and `embedding_model_id`.
- **Telemetry sprawl**: retrieval state already reports several useful fields; add only counters that explain current blind spots.
- **Large-repo claims**: CodeStory repo stats are useful but small; do not claim large-monorepo readiness until stress lanes exist.

## Completion Definition

CodeStory is credibly acting as an LLM's default codebase browser when:

- an agent can discover and use the read-only browser loop from the repo-local skill or stdio resources;
- missing cache, stale cache, semantic fallback, ambiguous symbols, and unsupported format cases produce actionable output;
- `context` can gather evidence for real integration anchors with cited symbols, snippets, trails, and explicit gaps;
- MCP/stdio clients receive stable schemas, read-only annotations, JSON-RPC-shaped errors, and continuation resource links;
- warm stdio/browser-loop p95 timings are measured and bounded;
- repo-scale and stress-lane gates protect index/search/trail/snippet behavior before releases;
- `explore` provides a useful browser-style flow without requiring a separate web app.
