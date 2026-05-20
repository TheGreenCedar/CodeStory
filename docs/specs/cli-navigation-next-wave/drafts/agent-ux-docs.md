# Agent UX and Documentation Draft

Draft owner lane: CLI agent UX and documentation only.

## Workflow Boundary

This draft covers three next-wave ideas from the CLI-first navigation suite:

- (4) Explore Packet Deepening
- (5) Affected Analysis 2.0
- (9) README Refresh

The implementation boundary stays inside the existing CLI/runtime/docs surfaces:

- Keep `codestory-cli explore` as the one-call investigation packet.
- Keep `codestory-cli affected` as the changed-file impact entrypoint.
- Keep README and repo-local skill references aligned with actual CLI behavior.
- Do not add MCP tools, server routes, `projectPath`, or `serve --stdio` contract changes.
- Do not promote mock or partial graph evidence as complete support; outputs must expose coverage, freshness, and uncertainty.

Current evidence in the repo:

- `docs/specs/cli-navigation-next-wave/blueprint.md` already names `ExploreInvestigationPacket`, `AffectedImpactAnalyzer`, and `DocumentationSurface`.
- `.agents/skills/codestory-grounding/references/explore.md` documents status, retrieval/freshness, target resolution, navigation, trail, source packet, related files, budget notes, and snippet output.
- `.agents/skills/codestory-grounding/references/affected.md` documents changed path inputs, `--stdin`, `--depth`, `--filter`, Markdown/JSON output, impacted symbols, likely tests, and notes.
- `README.md` already frames CodeStory as CLI-first and lists `explore`, `affected`, `drill`, `doctor`, `context`, and related grounding workflows.

## Blueprint Components

| Component | Responsibility | CLI surface | Primary crate/files |
|---|---|---|---|
| `ExploreInvestigationPacket` | Build a deeper, deterministic packet around one resolved target, including route-aware relationships, source slices, related files, gaps, and next commands. | `codestory-cli explore` | `crates/codestory-cli/src/explore.rs`, runtime read APIs, CLI golden-path tests |
| `AffectedImpactAnalyzer` | Convert changed files into impacted symbols, routes/endpoints, public API surfaces, likely tests, and blind-spot notes. | `codestory-cli affected` | `crates/codestory-runtime/src/lib.rs`, `crates/codestory-contracts/src/api/dto.rs`, `crates/codestory-cli/src/main.rs` |
| `DocumentationSurface` | Keep README, repo-local skill refs, and validation docs synchronized with the actual CLI command model and example workflows. | README and `.agents/skills/codestory-grounding/references/*.md` | `README.md`, `.agents/skills/codestory-grounding/SKILL.md`, command reference files, onboarding docs/tests |

## Requirements

### R-EPD-01: Explore Packet Deepening

`ExploreInvestigationPacket` shall make `explore` a richer agent handoff packet without turning it into question answering or a server-backed cockpit.

Acceptance criteria:

- **AC-EPD-01**: Given `explore --project <workspace> --id <node-id> --format json`, the output includes stable sections for status, resolution, navigation results, symbol/detail context, trail, source packet, related files, budget notes, freshness, and next commands.
- **AC-EPD-02**: Given an ambiguous `--query`, `explore` fails without silently choosing a target and preserves enough resolution metadata for the agent to retry with `search --why`, `--id`, or `--file`.
- **AC-EPD-03**: Given a partial, stale, or fallback retrieval state, `explore` reports that condition in both Markdown and JSON instead of implying complete evidence.
- **AC-EPD-04**: Given route or endpoint nodes from the route model, `explore` labels route/endpoint relationships and certainty in the packet when available, while omitting the section cleanly when the index has no route evidence.
- **AC-EPD-05**: Given source packets that hit size limits, `explore` reports truncation and budget details, and still returns usable file/slice records for downstream agent review.

### R-AFA-01: Affected Analysis 2.0

`AffectedImpactAnalyzer` shall move from symbol-only impact hints toward review-ready change impact without claiming to replace the test suite.

Acceptance criteria:

- **AC-AFA-01**: Given explicit paths, stdin paths, or omitted paths, `affected` normalizes changed paths and reports which paths matched indexed files and which did not.
- **AC-AFA-02**: Given changed files with contained symbols, `affected` reports impacted symbols with graph depth, relationship reason, and certainty or confidence where the runtime can determine it.
- **AC-AFA-03**: Given indexed route/endpoint evidence, `affected` reports impacted routes/endpoints or explicitly states that no route/endpoint evidence was found for the matched files.
- **AC-AFA-04**: Given test-like files reached by the graph, `affected` ranks likely tests and labels the recommendation as a focused hint, not a proof that other tests are unnecessary.
- **AC-AFA-05**: Given unmatched paths, generated/vendor files, partial indexes, or stale caches, `affected` emits blind-spot notes and next commands such as `files`, `doctor`, or `index --refresh full`.
- **AC-AFA-06**: Given `--format json`, all impact sections are machine-readable without parsing Markdown headings.

### R-DOC-01: README Refresh

`DocumentationSurface` shall make the CLI-first workflow obvious to a new agent or maintainer and keep examples aligned with real command behavior.

Acceptance criteria:

- **AC-DOC-01**: README opens with the normal path from build, index, ground/search, explore/context, affected, and validation without leading readers into MCP/server work.
- **AC-DOC-02**: README examples use `--project <workspace>` consistently and avoid `projectPath` terminology.
- **AC-DOC-03**: README explains the difference between broad orientation (`ground --why`), candidate discovery (`search --why`), focused packets (`explore`/`context`), and changed-file impact (`affected`).
- **AC-DOC-04**: README and skill references document failure paths: stale/partial retrieval, ambiguous targets, unmatched changed paths, and missing route evidence.
- **AC-DOC-05**: Docs mention validation expectations for CLI-facing changes, including serialized Cargo checks and repo-scale e2e stats when the change touches workflow gates.

## Design Notes

### Explore Packet Deepening

- Preserve `explore` as a CLI packet builder: it may compose runtime reads, but it should not interpret broad natural-language questions.
- Prefer extending existing output structs over inventing a parallel command. Keep Markdown readable and JSON stable.
- Treat route/endpoint evidence as optional and confidence-labeled. Absence of route evidence should be a clear packet note, not an empty success.
- Keep source packet budgeting explicit: include selected files, omitted files when known, merge/truncation notes, and the command that can fetch sharper evidence.
- Keep TUI behavior secondary to non-interactive agent output. `--no-tui` and `--format json` must remain the stable automation paths.

### Affected Analysis 2.0

- Keep `affected` input behavior compatible: positional paths, `--stdin`, and git-diff default.
- Add impact categories without hiding the old symbol/test sections. A practical shape is `matched_files`, `unmatched_paths`, `impacted_symbols`, `impacted_routes`, `impacted_tests`, `blind_spots`, and `next_commands`.
- Rank results by proximity, certainty, and test/public-surface relevance. Do not overfit to one language or framework.
- Use existing file role and graph data first. If route-aware work lands in another lane, consume the route model through runtime DTOs rather than adding CLI-side parsing.
- Keep failure output useful: no matched files, no index, stale index, and partial coverage should all return actionable next commands.

### README Refresh

- Update docs after the command behavior exists or in the same implementation slice. The README should not promise route-aware impact before CLI output and tests prove it.
- Keep README as the product workflow entrypoint. Put long command tables and edge cases in repo-local skill references or contributor docs.
- Prefer copy-paste-safe PowerShell examples for this workstation, while keeping generic `codestory-cli` examples where the README is platform-neutral.
- Cross-link to the nearest detailed docs rather than duplicating every option table.

## Tasks

### T-EPD: Explore Packet Deepening

- **T-EPD-01**: Inventory current `ExploreOutput` JSON fields and Markdown sections; record the exact before shape in a focused test.
- **T-EPD-02**: Add route/endpoint-aware optional fields to runtime/CLI output only after the route model DTO is available from the route lane.
- **T-EPD-03**: Add packet notes for missing route evidence, stale/partial retrieval, and source budget truncation.
- **T-EPD-04**: Add or update golden-path tests for JSON and Markdown packet sections.
- **T-EPD-05**: Update `.agents/skills/codestory-grounding/references/explore.md` only after behavior is implemented.

### T-AFA: Affected Analysis 2.0

- **T-AFA-01**: Extend affected DTOs with matched/unmatched path detail, blind spots, and optional route/endpoint impact records.
- **T-AFA-02**: Add runtime impact classification for symbols, tests, and routes/endpoints using existing graph/read-model data.
- **T-AFA-03**: Preserve existing CLI path handling and add failure-path notes for no diff, no matched files, stale cache, and partial index coverage.
- **T-AFA-04**: Add JSON and Markdown rendering tests for normal path, unmatched path, and route-evidence-absent edge.
- **T-AFA-05**: Update `.agents/skills/codestory-grounding/references/affected.md` only after behavior is implemented.

### T-DOC: README Refresh

- **T-DOC-01**: Compare README command examples against `codestory-cli --help` and command-specific help.
- **T-DOC-02**: Rewrite the main workflow around build, index, ground/search, explore/context, affected, and validation.
- **T-DOC-03**: Add concise recovery notes for stale retrieval, ambiguous target resolution, and unmatched affected paths.
- **T-DOC-04**: Cross-link to `explore.md`, `affected.md`, contributor testing docs, and architecture docs instead of duplicating long option tables.
- **T-DOC-05**: Run onboarding/docs tests or equivalent Markdown checks that prove README command names and references stay current.

## Validation Points

### Explore Packet Deepening

- Normal path: build the CLI, index a fixture workspace, run `explore --project <workspace> --id <node-id> --format json`, and assert all packet sections required by **AC-EPD-01** exist.
- Failure path: run `explore --project <workspace> --query <ambiguous-name> --format json` and assert it fails with resolution-layer metadata and retry guidance required by **AC-EPD-02**.
- Integration edge: run `search --project <workspace> --query <target> --why --format json`, feed the selected `node_id` into `explore`, then feed the same id into `context`, `trail`, or `snippet` to confirm command handoff remains stable.

### Affected Analysis 2.0

- Normal path: run `affected --project <workspace> src/runtime.rs --depth 3 --format json` against a prepared fixture and assert matched files, impacted symbols, likely tests, and next commands are populated.
- Failure path: run `affected --project <workspace> missing/file.rs --format json` and assert unmatched path detail plus recovery guidance are present.
- Integration edge: pipe `git diff --name-only HEAD` into `affected --project <workspace> --stdin --format json` after an incremental refresh and verify stale/partial coverage notes remain accurate.

### README Refresh

- Normal path: execute the README quick-start commands through help/build/index/read-command smoke checks where practical.
- Failure path: verify README recovery guidance maps to real commands: `doctor`, `index --refresh full`, `search --why`, `files`, and focused `explore`/`context` retry paths.
- Integration edge: run onboarding or docs contract tests that scan README for current command names and required links; then run `git diff --check`.

## Traceability Matrix

| Requirement | Acceptance criteria | Tasks | Validation |
|---|---|---|---|
| R-EPD-01 | AC-EPD-01..AC-EPD-05 | T-EPD-01..T-EPD-05 | Explore normal, failure, integration edge |
| R-AFA-01 | AC-AFA-01..AC-AFA-06 | T-AFA-01..T-AFA-05 | Affected normal, failure, integration edge |
| R-DOC-01 | AC-DOC-01..AC-DOC-05 | T-DOC-01..T-DOC-05 | README normal, failure, integration edge |

## Residual Risks

- Route/endpoint impact sections depend on the route model lane; until that lands, this lane should expose absence of route evidence rather than fabricating impact.
- README can drift if command behavior changes without updating repo-local skill references and onboarding checks.
- `affected` remains a focused graph hint. It should help choose tests, not narrow required verification when shared runtime/indexer behavior changes.
