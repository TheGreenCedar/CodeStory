# Agent Context Engine Tasks

## Current Implementation Status

Implemented in this branch:

- Milestone 1 analyzer, task manifest loading, and manifest-backed quality
  scoring.
- Packet DTOs, heuristic planner trace, budget enforcement, sufficiency
  contract, CLI `packet`, read-only stdio `packet`, and skill routing updates.
- Public-core seed corpus across five public repositories, all six task
  classes, and direct packet runtime rows for cold CLI vs warm stdio.
- Public-core task shape now has 18 manifests: three tasks for each of the six
  task classes.
- Packet broad-prompt expansion now emits ranked symbol probes, an evidence
  ledger, and claim-led flow notes. The CodeStory indexing-flow warm stdio
  packet smoke quality-passed with 100% expected-anchor, file, symbol, claim,
  and citation coverage.
- Packet anchor probes now use an internal symbolic search path, and stdio
  packet tool text is a compact digest while full packet data remains in
  `structuredContent`. The current warm stdio smoke row is `5.16s` and `80,588`
  bytes on the debug CLI.
- Public-core repositories are available and indexed locally; external
  checkouts live under `target/agent-benchmark/repos` while CodeStory uses the
  active workspace. The latest full-suite packet diagnostics ran
  across all 18 public-core manifests with three repeats in both warm stdio and
  cold CLI modes: `108/108` operational success, `108/108` quality-pass rows,
  `sufficient` packet coverage on every row, and no sufficiency/quality
  mismatches after task-specific packet seeds, cited claims, Go text-only
  symbols, graph-node pruning, and ranking fixes.
- Sufficiency golden tests now cover stop behavior across the six task classes,
  partial/insufficient targeted follow-ups, citation-budget truncation, and
  graph budget pruning.
- Strict packet-first telemetry now distinguishes an answer packet from
  `packet --help` and requires the answer packet to be the first successful
  repository-context command. Reanalysis invalidated the older packet-first
  claim for the public-core subset (`0/3` packet-first), while corrected
  with-CodeStory rows now pass `3/3` packet-first and `3/3` quality on the
  CodeStory indexing-flow task, Vite dev-server architecture task, Express
  response-helper bug-localization task, mux router matching-flow task, Express
  response symbol-ownership task, mux CORS middleware edit-planning task, and
  Express application route-tracing task.
- Five historical paired diagnostics exist. Under the pre-2026-05-24 quality
  scorer, both arms quality-passed `3/3` on Express bug localization (`74.2%`
  fewer median tokens, `50.7%` lower median wall time, `88.9%` fewer median
  tool starts with CodeStory), mux router matching-flow (`49.1%`, `47.2%`,
  `87.5%`), Express symbol ownership (`58.3%`, `49.4%`, `86.7%`), mux CORS
  edit planning (`72.3%`, `48.7%`, `85.7%`), and Express application route
  tracing (`69.4%`, `50.7%`, `86.7%`). Rerun or reanalyze them under the
  answer-level quality and cache-provenance gates before using them as savings
  evidence.
- The harness can reanalyze existing run directories from raw stdout JSONL after
  analyzer fixes, so quality-scoring corrections do not require a fresh model
  run.
- Agent A/B `--publishable` now fails with-CodeStory rows that perform
  ordinary source reads after a successful packet beyond the configured
  `--max-source-reads-after-packet` budget, which defaults to zero.
- Agent A/B rows now record repository provenance: resolved path, manifest
  URL/ref, actual git HEAD, dirty status, and whether a built-in local checkout
  overrode a manifest-defined public checkout.

Still open before public savings claims:

- More strict paired rows across additional public repositories and language
  families after the answer-level quality, cache-provenance, packet-first, and
  post-packet ordinary source-read gates show stable stop behavior.
- Baseline comparability for savings claims: Express and mux have historical
  paired diagnostics across five task rows, but those rows need strict
  reanalysis or rerun. CodeStory indexing-flow and Vite dev-server architecture
  still have quality-uneven no-CodeStory baselines (`1/3`), so they support
  quality-rescue evidence rather than aggregate savings. A Python/Flask row is
  the next best breadth target.

## Milestone 1: Benchmark Truth

### Task 1.1: Add Run Analyzer

Requirements: R3, R4

- Parse JSONL transcripts into command categories: CodeStory CLI, shell search, direct file read, git, build/test, other.
- Count ordinary source reads after first successful CodeStory packet.
- Count duplicate file reads and duplicate command patterns.
- Emit analyzer results into `summary.json` and `summary.md`.
- Add fixture tests for transcript parsing and category counts.

### Task 1.2: Add Task Manifest Support

Requirements: R4, R5

- Define benchmark task manifest schema.
- Add expected files, symbols, claims, task class, and quality thresholds.
- Load manifests from a public `benchmarks/tasks/` folder.
- Fail `--publishable` when quality scoring is unavailable for a manifest-backed run.

### Task 1.3: Add Quality Scoring

Requirements: R4

- Score expected-anchor recall from final answer and command transcript.
- Score citation coverage.
- Record missed anchors and unsupported claims.
- Keep operational pass/fail separate from quality pass/fail.

## Milestone 2: Packet Workflow

### Task 2.1: Define Packet DTOs

Requirements: R1, R2, R3

- Add request/response DTOs for packet, budget, sufficiency, covered claims, gaps, and benchmark trace.
- Add serialization tests.
- Keep citations compatible with existing context packet output.

### Task 2.2: Implement Packet Planner

Requirements: R1

- Add task classification and query decomposition.
- Start with heuristic classes: architecture flow, bug localization, edit planning, route tracing, symbol ownership, data flow.
- Add unit tests for query plans from task prompts.

### Task 2.3: Implement Evidence Orchestrator

Requirements: R1, R2

- Compose search, symbol, trail, snippet, affected, and context calls behind one service function.
- Deduplicate anchors and snippets.
- Prefer graph-backed hits over repo-text-only hits.
- Preserve fallback reasons and confidence.

### Task 2.4: Implement Packet Budgeter

Requirements: R2

- Add `tiny`, `compact`, `standard`, and `deep` budgets.
- Enforce max files, snippets, trail edges, citations, and output bytes.
- Return explicit truncation metadata and deeper follow-up commands.

### Task 2.5: Implement Sufficiency Contract

Requirements: R3

- Add `sufficient`, `partial`, and `insufficient` status.
- Emit covered claims, gaps, open-next files, avoid-opening files, and follow-up commands.
- Add golden tests where sufficient packets should not recommend broad exploration.

## Milestone 3: Product Surfaces

### Task 3.1: Add CLI Command

Requirements: R1, R2, R3

- Add `codestory-cli packet --project <repo> --question <task> --budget compact`.
- Support `--format markdown|json`.
- Include `--bundle` only after the core command is stable.

### Task 3.2: Add Warm Stdio Tool

Requirements: R6

- Add read-only `packet` tool to the stdio catalog.
- Expose the same DTO schema as CLI JSON output.
- Add protocol contract tests for read-only metadata and output schema.

### Task 3.3: Update Skill Router

Requirements: R7

- Make packet-first the broad-task default.
- Move primitive command choreography into references.
- Add explicit stop rule: do not run broad ordinary file reads after a sufficient packet.

## Milestone 4: Public Benchmark Corpus

### Task 4.1: Add Public Repos

Requirements: R5

- Add at least five public repos across Rust, TypeScript/JavaScript, Python, Go, and mixed monorepo shapes.
- Keep private sibling repos opt-in.
- Document clone/setup expectations.

### Task 4.2: Add Task Classes

Requirements: R4, R5

- Add at least six task classes: architecture explanation, bug localization, change impact, route tracing, symbol ownership, and edit planning.
- Add at least three tasks per class.
- Include expected anchors and quality thresholds.

### Task 4.3: Publish Baseline And Target Rows

Requirements: R4, R5

- Run cold CLI and warm stdio modes.
- Report medians across at least three repeats.
- Prefer four repeats per arm for public headline rows when runtime cost is acceptable.
- Publish quality-gated rows only.
- Keep negative rows visible until fixed.

## Milestone 5: Optimization

### Task 5.1: Reduce Post-Packet Ordinary Reads

Requirements: R3, R4

- Use analyzer output to identify why agents still run broad `rg` or file reads.
- Update packet content and skill stop rules.
- Track ordinary source-read reduction as a release gate.

### Task 5.2: Tune Packet Ranking

Requirements: R1, R2, R4

- Compare lexical, semantic, and graph ranking contributions by task class.
- Add regression fixtures for expected anchors.
- Tune default packet budgets around quality and token use.

### Task 5.3: Warm Transport Benchmark

Requirements: R6

- Add benchmark mode that starts `serve --stdio` once per repo and runs packet calls over the warm connection.
- Compare cold CLI and warm stdio rows.
- Promote warm mode as the expected agent integration when it wins.
