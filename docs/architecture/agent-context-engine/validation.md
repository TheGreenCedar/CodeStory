# Agent Context Engine Validation

## Validation Strategy

The validation goal is to prove that CodeStory changes agent behavior. Passing means agents open fewer files and run fewer broad search commands after receiving a packet, while still producing correct, cited answers.

## Gates

| Gate | Metric | Initial target | Promotion target |
| --- | --- | ---: | ---: |
| Quality | Expected-anchor recall | >= 0.80 | >= 0.90 |
| Quality | False critical claims | 0 | 0 |
| Evidence | Final answer citation coverage | >= 0.80 | >= 0.90 |
| Behavior | Ordinary source reads after packet | <= no-CodeStory median | >= 40% reduction |
| Behavior | Duplicate source reads | <= no-CodeStory median | >= 50% reduction |
| Cost | Total tokens | no hard gate | lower than no-CodeStory with quality passing |
| Time | Wall time | no hard gate | no slower than no-CodeStory in warm mode |
| Tooling | Tool starts | no hard gate | lower than no-CodeStory in warm mode |

## Test Matrix

### Unit Tests

- Packet planner classifies task prompts and produces expected subqueries.
- Packet budgeter enforces hard shape limits.
- Sufficiency contract marks known complete packets as sufficient.
- Transcript analyzer counts CodeStory CLI, shell search, direct file reads, and duplicate reads.
- Task manifest parser rejects missing expected anchors.

### Integration Tests

- `codestory-cli packet --format json` returns a schema-stable packet.
- `codestory-cli packet --budget tiny` remains under budget limits.
- `serve --stdio` exposes read-only `packet` metadata.
- Packet output handles stale cache, missing cache, semantic fallback, and ambiguous query states.
- Benchmark harness emits quality and behavior telemetry into `summary.json`.

### Benchmark Tests

- Run at least three repeats for each task arm, with four repeats preferred for public headline rows.
- Run both cold CLI and warm stdio modes.
- Record repo, language, task class, model, runner, sandbox, cache policy, semantic backend, and pricing settings.
- Keep raw transcripts under ignored output paths.

## Public Benchmark Corpus

The public suite should include at least:

| Repo shape | Task examples |
| --- | --- |
| Rust workspace | indexing flow, storage/trail ownership, CLI command path |
| TypeScript/Next | route-to-handler tracing, component ownership, API/client flow |
| Python service | request/data flow, module ownership, test selection |
| Go service | package flow, handler/service/storage tracing |
| Mixed monorepo | cross-package impact and config-to-code relationships |

Each task manifest must include:

- prompt;
- task class;
- expected files;
- expected symbols when stable;
- expected claims;
- optional forbidden claims;
- quality threshold.

## Required Commands

Focused validation:

```powershell
cargo fmt --check
cargo test -p codestory-cli --test onboarding_contracts
cargo test -p codestory-cli --test stdio_protocol_contracts
node .\scripts\codestory-agent-ab-benchmark.mjs --quick --repos codestory --repeats 3 --timeout-ms 900000 --sandbox danger-full-access --publishable
```

Future quality-gated validation:

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --task-suite public-core --list --publishable --repeats 3 --materialize-repos
node .\scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --task-suite public-core --repeats 3 --packet-runtime-mode both --codestory-cli .\target\release\codestory-cli.exe --publishable
node .\scripts\codestory-agent-ab-benchmark.mjs --task-suite public-core --repeats 3 --timeout-ms 900000 --sandbox danger-full-access --publishable --materialize-repos
```

## Traceability

| Requirement | Validation |
| --- | --- |
| R1 Packet-first agent entry | CLI packet integration tests, stdio packet tests, task-class benchmark runs |
| R2 Budgeted output | packet budget unit tests, JSON budget fields, tiny/compact golden tests |
| R3 Stop condition | sufficiency golden tests, post-packet ordinary source-read telemetry |
| R4 Benchmark quality scoring | manifest quality tests, expected-anchor recall reports |
| R5 Public multi-repo corpus | public-core suite coverage report |
| R6 Warm read integration | stdio packet tests and warm benchmark mode |
| R7 Skill router simplification | skill validation and benchmark transcript reduction in broad file reads |

## Baseline Interpretation

The current three-repeat CodeStory benchmark is a negative baseline for agent savings: the with-CodeStory arm used more median tokens, wall time, and tool starts. That result should remain visible until the packet workflow and skill stop rules produce a quality-passing improvement.

The packet-first work has produced five historical paired diagnostics: Express
response-helper bug localization, mux router matching-flow architecture,
Express response symbol ownership, mux CORS middleware edit planning, and
Express application route tracing. Those rows predate the stricter 2026-05-24
answer-level quality and cache-provenance gates, so treat them as promotion
seeds that require rerun or reanalysis before they support a public aggregate.

## Exit Criteria For First Promotion

The first public "agent savings" claim is allowed only when all are true:

- at least five public repositories;
- at least six task classes;
- at least three repeats per arm, with four repeats preferred for headline rows;
- quality thresholds pass;
- with-CodeStory reduces ordinary source reads by at least 40%;
- token or wall-time savings are positive on the reported aggregate;
- all pricing assumptions are explicit when cost is reported.
