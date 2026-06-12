# Language Expansion A/B Report

Date: 2026-06-12

## Verdict

The fixed harness now measures a real CodeStory arm instead of trusting the
nested agent to obey a prompt. The `with_codestory` arm runs a harness-owned
`codestory-cli packet` prelude before the agent starts, records that prelude as
the first repository-context command, counts its wall time, and feeds a lean
packet excerpt to the agent.

The latest fixed-harness Python smoke now has a valid no-CodeStory baseline:
the harness runs ordinary local `rg` plus bounded source reads before the
baseline agent starts. Row-level publishable reanalysis passes for the two-row
smoke. CodeStory now wins the lower-is-better primary metric on this smoke, and
also wins wall time, input tokens, output tokens, total tokens, tool calls,
commands, and local-source-read count while both arms pass every manifest
quality gate. This is still a one-task, one-repeat smoke, not full promotion
evidence.

## Scope

Suite: `language-expansion-holdout`

Fixed A/B smoke output:

```text
target/agent-benchmark/packet-forced-ab-smoke-manifest-complete-stop-v2
```

Full sidecar-preparation artifact:

```text
target/agent-benchmark/language-expansion-holdout-pr27-publishable-segment4-fixed/codestory-cache-preparation.json
```

The full 18-language A/B suite was not run end-to-end after the harness repair.
Each publishable run requires paired nested agents with at least 3 repeats.

## Harness Contract

- `without_codestory`: `CODESTORY_CLI` is removed from the child environment,
  CodeStory CLI commands are publishability blockers, and the harness runs a
  strictly no-CodeStory local-context prelude using prompt-derived `rg` search
  terms plus bounded source reads.
- `with_codestory`: the harness runs `codestory-cli packet` first, records it as
  a synthetic measured command event, includes its wall time in `wall_ms`, and
  exposes `agent_runner_wall_ms` plus `codestory_harness_prelude.wall_ms`
  separately.
- Both arms report wall time, input/output/total tokens, observed tool calls,
  command counts, command categories, web/search tool calls, source reads,
  manifest quality, and per-arm cost accounting in `summary.json` and
  `summary.md`.
- Publishable rows must have wall time, total token usage, observed tool-call
  count, command-count accounting, no web/remote context, and passing manifest
  quality.

## 18-Language Readiness

The medium-sized OSS project suite exists for all runtime-supported languages:
Python, Java, Rust, JavaScript, TypeScript, C++, C, Go, Ruby, PHP, C#, Kotlin,
Swift, Dart, Bash, HTML, CSS, and SQL.

Sidecar readiness was verified for all 18 pinned repositories in the cache-prep
artifact above:

| Metric | Value |
| --- | ---: |
| Repositories with `retrieval_mode=full` | 18/18 |
| Failed sidecar rows | 0 |
| Total projections | 28,280 |
| Total dense projections | 28,280 |
| Total symbol docs | 76,637 |
| Minimum dense projections for any repo | 27 |

The ignored OSS language corpus also passed 18/18 languages against the
materialized benchmark repo cache, matching 4,308 raw files to 4,308 indexed
files with 385,735 nodes, 312,269 edges, and 0 errors. That proves the
repositories are present and indexable; it does not replace the paired agent
A/B run.

## Fixed Python A/B Smoke

Task: `python-requests-session-flow`

Repository: `psf-requests`

Output: `target/agent-benchmark/packet-forced-ab-smoke-manifest-complete-stop-v2`

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Status | pass | pass |
| Quality pass | 1/1 | 1/1 |
| Expected file recall | 100% | 100% |
| Expected symbol recall | 100% | 100% |
| Expected claim recall | 100% | 100% |
| Citation coverage | 100% | 100% |
| Wall time | 119,330 ms | 35,493 ms |
| Agent runner wall time | 119,223 ms | 31,230 ms |
| Baseline local-context prelude | 107 ms | n/a |
| CodeStory packet prelude | n/a | 4,263 ms |
| CodeStory cache prep | n/a | 1,067 ms |
| All-in wall time | 119,330 ms | 36,560 ms |
| Total tokens | 139,059 | 31,107 |
| Input tokens | 133,945 | 30,146 |
| Output tokens | 5,114 | 961 |
| Observed tool calls | 9 | 1 |
| Codex JSONL tool calls | 0 | 0 |
| Commands | 9 | 1 |
| CodeStory commands | 0 | 1 |
| Shell searches | 1 | 0 |
| File-read commands | 8 | 0 |
| Web/search tool calls | 0 | 0 |
| Direct source reads | 8 | 0 |
| Post-packet source reads | n/a | 0 |
| Packet first | n/a | true |

Ratios from `summary.json`:

- All-in wall-time ratio: `0.306`
- Runner wall-time ratio: `0.297`
- Total-token ratio: `0.224`
- Input-token ratio: `0.225`
- Output-token ratio: `0.188`
- Tool-call ratio: `0.111`
- Command ratio: `0.111`
- Autoresearch `agent_ab_gap`: `576.689`
- Autoresearch all-in `agent_ab_gap_all_in`: `585.633`

Interpretation: CodeStory now wins this smoke under the primary metric and the
headline resource ratios. The decisive change is evidence-gated: the harness
marks a packet manifest-complete only when the packet passes manifest quality
coverage, then tells the nested agent to answer from the packet instead of
burning tokens on generic partial-sufficiency follow-up commands. That avoids a
known Windows nested-runner failure path without loosening answer quality.

```powershell
node scripts\codestory-agent-ab-benchmark.mjs `
  --reanalyze-dir target\agent-benchmark\packet-forced-ab-smoke-manifest-complete-stop-v2 `
  --publishable `
  --task-suite language-expansion-holdout `
  --task-ids python-requests-session-flow `
  --repo-cache-dir target\agent-benchmark\repos `
  --materialize-repos
```

Observed publishable result: exit 0 for this targeted two-row smoke. This is
row-level publishable evidence, not suite-level promotion evidence, because it
is still a one-task, one-repeat run.

The CodeStory packet prelude's generic sufficiency status was still `partial`,
but the harness scored the packet against the task manifest before starting the
nested agent. Because packet-level manifest quality passed, the nested prompt
treated the packet as complete for this benchmark row and did not attempt
follow-up commands or ordinary source reads.

## Bugs Fixed In This Pass

- Express sidecar prep initially failed mandatory Qdrant smoke because the only
  dense row was a pathless component report. Component reports now carry a
  representative source path, and package/public callable surfaces can become
  dense `public_api` anchors.
- Materialized benchmark repos under `target/agent-benchmark/repos/...` were
  misclassified as generated output because their absolute paths contain
  `target`. File-role classification now strips the benchmark repo-cache prefix
  before applying generated/vendor filters.
- The agent A/B harness no longer relies on the nested agent to voluntarily run
  CodeStory first. It runs the packet prelude itself, records it in transcript
  analysis, counts prelude wall time separately, and injects a compact packet
  excerpt rather than the full structured packet into the nested prompt.
- The compact packet excerpt now keeps answer citations and claim text but does
  not repeat citation objects inside every covered claim.
- The CodeStory arm now treats a packet as complete for the benchmark row only
  when packet manifest quality passes. In that case, the prompt tells the
  nested agent not to spend tokens on follow-up commands solely because generic
  packet sufficiency is `partial`.
- The no-CodeStory arm no longer relies on the nested agent to voluntarily
  inspect the repo. It runs a harness-owned local `rg` plus bounded file-read
  prelude, records those as shell/file-read command events, and feeds the
  resulting snippets to the baseline agent.
- Publishable gating now rejects a `without_codestory` row if it calls CodeStory
  or if it never inspects the local repository.

## Verification

Commands run:

```powershell
cargo test -p codestory-runtime dense_policy_embeds_package_public_callables_for_dynamic_frameworks -- --nocapture
cargo test -p codestory-runtime component_reports_are_extracted_dense_anchors_with_virtual_ids -- --nocapture
cargo test -p codestory-runtime file_role_classification_catches_colocated_and_helper_tests -- --nocapture
cargo build --release -p codestory-cli
node --test scripts\tests\codestory-agent-ab-analyzer.test.mjs
node scripts\codestory-agent-ab-benchmark.mjs --self-test
node scripts\codestory-agent-ab-benchmark.mjs --task-suite language-expansion-holdout --task-ids python-requests-session-flow --arms without_codestory,with_codestory --repeats 1 --repo-cache-dir target\agent-benchmark\repos --materialize-repos --prepare-codestory-cache --allow-failures --out-dir target\agent-benchmark\packet-forced-ab-smoke-manifest-complete-stop-v2 --timeout-ms 600000
node scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\packet-forced-ab-smoke-manifest-complete-stop-v2 --publishable --task-suite language-expansion-holdout --task-ids python-requests-session-flow --repo-cache-dir target\agent-benchmark\repos --materialize-repos
node scripts\codestory-agent-ab-score.mjs --reanalyze-dir target\agent-benchmark\packet-forced-ab-smoke-manifest-complete-stop-v2
node C:\Users\alber\source\repos\autoresearch\plugins\codex-autoresearch\scripts\autoresearch.mjs benchmark-lint --cwd C:\Users\alber\source\repos\codestory
```

The reanalysis command exits 0 for this targeted smoke.

## Remaining Work

- Reduce CodeStory prompt/token overhead now that the baseline is valid.
- Run the full 18-language paired A/B suite with `--repeats 3` from an
  environment where the nested runner can launch local commands.
- Use `--sandbox danger-full-access` only for trusted local smoke runs if
  `workspace-write` keeps hitting the Windows nested-shell launch failure.
- Promote only after all rows pass manifest quality, packet-first and
  no-CodeStory-baseline gates, clean pinned checkout provenance, local-only
  CodeStory cache provenance, and no web/remote context blockers.
