# Agent Context Engine Requirements

## Introduction

These requirements turn the benchmark friction into testable product behavior. The central measure is not whether CodeStory can return evidence. It already can. The measure is whether its evidence reduces redundant agent exploration while preserving answer quality.

## Glossary

| Term | Meaning |
| --- | --- |
| Packet | A bounded, cited context bundle generated for one agent task or question. |
| Primitive command | A focused command such as `search`, `symbol`, `trail`, `snippet`, `context`, or `affected`. |
| Ordinary source read | Non-CodeStory shell/file exploration such as `rg`, `Get-Content`, `cat`, or equivalent direct file reads. |
| Expected anchor | A file, symbol, or claim that the task corpus marks as necessary evidence. |
| Sufficiency contract | Packet section that states covered claims, uncertainty, gaps, and exact next files/commands. |

## Requirements

### Requirement 1: Packet-First Agent Entry

The system shall provide one agent-native packet entrypoint for broad tasks.

Acceptance Criteria:

1. WHEN an agent has a broad repository question, THE Packet Planner SHALL accept `--question` text and derive concrete retrieval subqueries without requiring the agent to manually run `ground`, multiple `search` calls, `trail`, and `snippet`.
2. WHEN a packet is generated, THE Evidence Orchestrator SHALL include cited files, symbols, trails, snippets, confidence notes, and gaps in one bounded response.
3. WHEN a packet cannot resolve enough evidence, THE Sufficiency Contract SHALL return exact follow-up primitive commands instead of silently inviting broad source exploration.

Traceability: baseline friction shows the with-CodeStory arm used CodeStory first and then continued normal broad exploration; see [benchmark-results.md](../../testing/benchmark-results.md).

### Requirement 2: Budgeted Output

The system shall make packet size predictable for agent context windows.

Acceptance Criteria:

1. WHEN `--budget tiny|compact|standard|deep` is passed, THE Packet Budgeter SHALL enforce maximum files, snippets, trail edges, citations, and output bytes for that mode.
2. WHEN the packet truncates evidence, THE packet SHALL expose omitted sections in budget metadata and SHALL add follow-up commands only when truncation changes the sufficiency status.
3. WHEN `--format json` is used, THE packet SHALL expose budget usage fields so harnesses can compare actual output sizes.

Traceability: benchmark transcripts showed large command-output volumes and repeated manual reads after CodeStory output.

### Requirement 3: Stop Condition For Agents

The system shall tell agents when the packet is sufficient.

Acceptance Criteria:

1. WHEN expected evidence coverage is high, THE Sufficiency Contract SHALL say which claims are covered and which files do not need further opening for an answer.
2. WHEN expected evidence coverage is low or ambiguous, THE Sufficiency Contract SHALL name the missing anchors and recommend targeted follow-ups.
3. WHEN the agent opens broad files after a sufficient packet, THE Benchmark Harness SHALL count that as avoidable exploration.

Traceability: current skill guidance defines packet-first routing and a stopping rule for sufficient packets; the remaining risk is keeping sufficiency tied to task coverage rather than citation existence alone.

### Requirement 4: Benchmark Quality Scoring

The benchmark suite shall score correctness and evidence coverage, not only tokens and time.

Acceptance Criteria:

1. WHEN a benchmark task completes, THE Benchmark Harness SHALL report expected-anchor recall, false-claim count, citation coverage, ordinary source-read count, and duplicate-read count.
2. WHEN an agent answer omits required anchors, THE run SHALL remain successful operationally but fail the quality threshold.
3. WHEN token or wall-time savings appear, THE benchmark report SHALL only promote them when quality thresholds pass.

Traceability: the current harness records medians, tool starts, answer-level quality gates, duplicate direct file reads, and ordinary source reads after packet. Remaining benchmark work should broaden strict paired rows and unsupported-claim detection rather than re-adding basic quality telemetry.

### Requirement 5: Public Multi-Repo Corpus

The benchmark corpus shall be publicly reproducible.

Acceptance Criteria:

1. WHEN the `public-core` public benchmark runs, THE Task Corpus SHALL use at least five public repositories across at least four language families, with seven repositories and seven language families preferred for headline rows.
2. WHEN private sibling repositories are configured, THE harness SHALL keep them opt-in and exclude them from public README claims.
3. WHEN benchmark rows are promoted, THE report SHALL identify repo, language, task class, repeats, cache policy, semantic backend, runner, model, sandbox, pricing assumptions, and raw medians for cost, tokens, wall time, and tool starts.

Traceability: the no-suite default is a CodeStory smoke path; `--task-suite public-core` is the publishable public corpus path, and local sibling repos are opt-in.

### Requirement 6: Warm Read Integration

The system shall avoid per-command startup cost for repeated agent reads.

Acceptance Criteria:

1. WHEN an agent uses `serve --stdio`, THE Warm Transport SHALL expose the packet entrypoint and primitive follow-ups as read-only tools.
2. WHEN a benchmark uses the warm transport, THE Benchmark Harness SHALL record transport mode separately from cold CLI mode.
3. WHEN stdio tools are listed, THE catalog SHALL preserve read-only safety metadata.

Traceability: the stdio catalog exposes the read-only `packet` task surface plus primitive navigation tools; warm-transport validation should keep that path tested without making stdio the default for ordinary CLI navigation work.

### Requirement 7: Skill Router Simplification

The installed skill shall make the packet workflow the default.

Acceptance Criteria:

1. WHEN the skill triggers for a broad task, THE Skill Router SHALL instruct the agent to run packet first.
2. WHEN packet output reports gaps, THE Skill Router SHALL select follow-up primitives from those gaps.
3. WHEN packet output reports sufficient coverage, THE Skill Router SHALL discourage broad ordinary file sweeps unless editing or verifying a specific claim.

Traceability: current skill body documents many primitives and template workflows, which increases agent command choreography.

## Requirement-To-Component Map

| Requirement | Components |
| --- | --- |
| R1 | Packet Planner, Evidence Orchestrator, Sufficiency Contract |
| R2 | Packet Budgeter, Evidence Orchestrator |
| R3 | Sufficiency Contract, Benchmark Harness |
| R4 | Benchmark Harness, Task Corpus |
| R5 | Task Corpus, Benchmark Harness |
| R6 | Warm Transport, Evidence Orchestrator |
| R7 | Skill Router, Sufficiency Contract |
