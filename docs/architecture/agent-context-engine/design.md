# Agent Context Engine Design

## Design Summary

Add a packet-first workflow on top of the existing graph/search/context surfaces. The workflow should consume a natural-language task, decompose it into anchored evidence requests, return a budgeted context packet, and explicitly tell the agent whether it has enough evidence to answer or plan an edit. Existing primitive commands remain available, but the skill and benchmark should treat them as follow-up tools, not the default first move.

## Current System Fit

CodeStory already has most low-level pieces:

- `context` accepts exact targets and returns a structured context packet with citation controls.
- `search --why` and repo text fallback can find candidate anchors when indexed symbol search is weak.
- `trail`, `snippet`, `symbol`, `definition`, `references`, and `symbols` expose code-intelligence primitives.
- `serve --stdio` publishes read-only tool metadata and packet schemas.
- The benchmark harness can run with/without arms and record medians, token usage, sandbox, and tool starts.

The missing layer is orchestration: one high-level packet that makes those primitives feel like one decision-ready context engine.

## Component Design

### Packet Planner

Inputs:

- `project`
- `question`
- `budget`
- optional `task_class`
- optional `expected_anchor_manifest` for benchmarks

Responsibilities:

- Classify the task as architecture explanation, bug localization, change impact, route tracing, symbol ownership, data flow, or edit planning.
- Extract candidate query terms from the question.
- Decide which retrieval primitives to call.
- Keep a small trace of why each subquery exists.

Initial implementation can be heuristic:

| Task signal | Planned subqueries |
| --- | --- |
| "flow", "through", "pipeline" | entrypoint search, runtime search, trail around core anchors, snippets for handoff points |
| "where to edit", "change", "fix" | search, affected, likely tests, snippets around edit candidates |
| "route", "endpoint", "handler" | route/file search, definition, references, trail |
| "owner", "symbol", "who calls" | symbol, references, trail, snippet |

### Evidence Orchestrator

Responsibilities:

- Run planned retrieval calls against the existing runtime services.
- Deduplicate anchors and file snippets.
- Prefer graph-backed evidence over repo-text-only hits when both exist.
- Keep repo-text-only hits as candidate evidence with lower confidence.
- Avoid ordinary file reads; use CodeStory storage and snippet APIs.

The CLI command can be implemented as `packet` or as a broad mode of `context`. A separate `packet` command is clearer for agents because `context` currently means "deep evidence around one concrete target."

### Packet Budgeter

Budget modes:

| Budget | Intended use | Hard shape |
| --- | --- | --- |
| `tiny` | benchmark and chat triage | 3 anchors, 3 files, 6 snippets, 12 trail edges |
| `compact` | default agent answer | 10 anchors, 10 files, 12 snippets, 30 trail edges |
| `standard` | edit planning | 10 anchors, 10 files, 24 snippets, 60 trail edges |
| `deep` | manual investigation | bounded by explicit byte cap |

Every packet should report:

- `budget.requested`
- `budget.used`
- `budget.truncated`
- `budget.omitted_sections`
- `next_deeper_command`

### Sufficiency Contract

The packet should end with a decision block:

```text
sufficiency: sufficient | partial | insufficient
covered_claims:
- claim -> citations
open_next:
- file/path.rs:line because ...
avoid_opening:
- file/path.rs because packet already includes the relevant snippet
gaps:
- missing edge or ambiguous symbol
```

This is the part that should reduce benchmark cost. It gives the agent permission to stop broad discovery.

### Benchmark Harness

Extend `scripts/codestory-agent-ab-benchmark.mjs` with a post-run analyzer:

- parse command executions into CodeStory CLI, shell search, direct file read, git, test/build, and other;
- count ordinary source reads after the first successful packet;
- count duplicate file reads by path;
- measure command output characters by category;
- score final answer against task manifests.

Add task manifests:

```json
{
  "repo": "codestory",
  "task_class": "architecture_explanation",
  "prompt": "Explain how full indexing flows through CLI, runtime, workspace, indexer, and store.",
  "expected_files": [
    "crates/codestory-cli/src/main.rs",
    "crates/codestory-runtime/src/lib.rs",
    "crates/codestory-workspace/src/lib.rs",
    "crates/codestory-indexer/src/lib.rs",
    "crates/codestory-store/src/snapshot_store.rs"
  ],
  "expected_claims": [
    "CLI delegates indexing to runtime",
    "workspace computes the refresh plan",
    "indexer extracts symbols and edges",
    "store persists graph/search/snapshot state"
  ]
}
```

### Warm Transport

Expose the packet workflow through `serve --stdio` with read-only safety metadata. The benchmark should run both cold CLI and warm stdio modes because a context engine should not pay process startup per retrieval call in normal agent loops.

### Skill Router

Restructure the skill workflow:

1. Resolve binary and target workspace.
2. For broad tasks, run packet first.
3. Read the sufficiency contract.
4. Use primitive commands only for named gaps.
5. Open source files only when editing or when the packet says a claim needs verification.

## Data Model Additions

New DTOs should be added under contracts rather than CLI-only structs:

- `AgentPacketRequestDto`
- `AgentPacketDto`
- `PacketBudgetDto`
- `PacketSufficiencyDto`
- `PacketClaimDto`
- `PacketBenchmarkTraceDto`

These DTOs should preserve existing citation, retrieval, trail, snippet, and route metadata instead of flattening everything into prose.

## Failure Modes

| Failure | Expected behavior |
| --- | --- |
| No index | CLI and stdio packet entrypoints fail at the command preflight with an exact `index --refresh full` command before packet construction. |
| Stale index | Packet includes stale warning and decides whether evidence remains usable. |
| Ambiguous query | Packet reports ambiguity and suggested anchors. |
| Budget exceeded | Packet truncates low-confidence sections and reports deeper command. |
| Semantic backend unavailable | Packet falls back to lexical/graph evidence and reports mode. |
| Task out of scope | Packet returns unsupported task class and recommends primitive commands. |

## Non-Goals

- Natural-language answer generation inside CodeStory as a hosted model dependency.
- Cloud indexing.
- Hiding evidence behind a black-box score.
- Replacing existing primitive commands.
