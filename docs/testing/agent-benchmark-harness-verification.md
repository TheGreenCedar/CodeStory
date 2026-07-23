# Agent Benchmark Harness Verification

**Audience:** Evidence record — not an install guide.

Scope: transcript analysis and manifest-backed quality scoring for
`scripts/codestory-agent-ab-benchmark.mjs`.

The harness exposes pure analyzer/scorer functions and keeps a built-in
fixture smoke test:

```sh
node ./scripts/codestory-agent-ab-benchmark.mjs --self-test
```

The focused Node fixture lives at
`scripts/tests/codestory-agent-ab-analyzer.test.mjs`:

```sh
node --test ./scripts/tests/codestory-agent-ab-analyzer.test.mjs
```

The fixture verifies:

- command category counts for CodeStory CLI, shell search, direct file reads,
  git, and build/test commands;
- runner JSONL tool category counts for web search, MCP tool calls,
  command execution, function calls, and other tool calls;
- direct source-read accounting across the supported language extension set,
  including Dart, Bash, HTML, CSS, and SQL;
- ordinary source reads after the first successful packet command;
- harness-run no-CodeStory local-context preludes and CodeStory packet preludes
  as measured first-context commands;
- duplicate file reads by normalized path;
- expected file, symbol, claim, and citation recall;
- missed anchors as quality evidence, separate from operational run status;
- forbidden-claim scoring that avoids contradicted positive-claim false
  positives, including hyphenated terms such as `whitespace-only`;
- publishable blockers when the `without_codestory` arm either calls CodeStory
  or never inspects the local repository.

Drill-suite answer-quality ledgers are the repo-grounded counterpart to this
transcript scorer. Use the transcript harness to check how an agent behaved;
then run `node scripts/score-drill-ledger.mjs <suite-report.json> <ledger.json> [scored-report.json]`
to merge focused source-truth classifications outside the
product runtime. Ledger claim classifications are `correct`, `partial`,
`misleading`, and `unsupported`; the scored artifact keeps the final
answer-quality verdict separate from green index/build mechanics.

For source-truth recall, `drill` maps its question and seed anchors through the
runtime packet planner, then feeds the broad question search and bounded planned
subqueries into the verification target list. Treat those targets as candidate
files for verification, not as final answer support. Drill still owns the
compatibility report; the versioned script owns source-truth scoring, and packet
owns query planning.

Keep `node ./scripts/codestory-agent-ab-benchmark.mjs --list` as the cheapest
configuration smoke check.

The language-support promotion packet-runtime suite is:

```powershell
$env:CODESTORY_EMBED_MODEL_SOURCE = (node scripts/prepare-embedded-model.mjs).Trim()
cargo build --release -p codestory-cli
node scripts/codestory-agent-ab-benchmark.mjs `
  --packet-runtime `
  --packet-runtime-mode both `
  --task-suite language-expansion-holdout `
  --repeats 3 `
  --materialize-repos `
  --jobs 4 `
  --prepare-codestory-jobs 2 `
  --codestory-cli target/release/codestory-cli.exe `
  --out-dir target/agent-benchmark/language-expansion-publishable-full-form-command-shapes `
  --timeout-ms 180000 `
  --max-source-reads-after-packet 0 `
  --publishable
```

The run ledger records per-run `wall_ms`, token usage, estimated cost when
`CODESTORY_BENCH_INPUT_COST_PER_MTOK` and
`CODESTORY_BENCH_OUTPUT_COST_PER_MTOK` are configured, observed tool calls, tool
categories, web searches, command counts, command categories, direct source
reads, ordinary source reads after the first CodeStory command, ordinary source
reads after the first packet, duplicate file reads, and manifest quality scores.
For the `without_codestory` arm, the harness mechanically runs a strictly
no-CodeStory local-context prelude before starting the nested agent. It derives
plain `rg` search terms from the prompt, reads bounded snippets from selected
source files, records those as measured shell-search/file-read commands, and
injects the snippets into the prompt. For the `with_codestory` arm, the harness
mechanically runs the required `codestory-cli packet` prelude before starting
the nested agent, injects a lean packet excerpt into the prompt, and records
that prelude as a measured CodeStory command with its own wall time. This makes
both local-baseline inspection and packet-first evidence harness facts instead
of prompt-compliance hopes.
When the harness can also score the packet itself against the task manifest and
that packet-level manifest quality passes, the nested CodeStory prompt treats
the packet as complete for that benchmark row. That row-specific stop rule is
not based on generic `sufficiency.status`; it is based on the same expected
file, symbol, claim, and citation evidence used by the row quality gate.
When packet-level manifest quality is incomplete, the CodeStory arm remains
CodeStory-first but is not packet-only by default. The nested agent must use
listed CodeStory follow-ups before ordinary source reads, and any source reads
after the first packet are counted as post-packet overhead. Use
`--max-source-reads-after-packet 0` only for stricter packet-only promotion
evidence.
Each run row also includes a normalized `resource_accounting` object with the
same wall-clock, agent-runner wall-clock, baseline-prelude wall-clock,
CodeStory-prelude wall-clock, token, tool-call, command-count, and source-read
evidence in one place.

`summary.json` and `reanalyzed-summary.json` include a top-level
`cost_accounting` block. It totals time spent, input/output/total tokens spent,
estimated cost, tool calls, command counts, web searches, and source reads per
arm across all observed rows, including failed or timed-out rows when their
measurements are present, then emits a `with_vs_without` comparison for runner
wall time, all-in wall time, tokens, tool calls, commands, and estimated cost.
The Markdown summary prints the same totals before the per-task median table,
so a human report can compare aggregate cost and time before looking at quality
medians.
`scripts/codestory-agent-ab-score.mjs` reuses that accounting and emits
`METRIC` lines for the raw per-arm wall time, tokens, tool calls,
commands, CodeStory commands, shell searches, file-read commands, web searches,
post-packet reads, quality pass counts, packet-first pass counts,
packet-manifest quality pass counts, partial packet counts, and ratios.
The score wrapper streams the lower-level benchmark progress while still
capturing stdout/stderr for failure reporting, and it forwards
`--prepare-codestory-timeout-ms` to the benchmark so long CodeStory cache
preparation is visible and explicitly bounded.
The primary `agent_ab_gap` penalizes with-CodeStory quality failures,
packet-first failures, post-packet source reads, and external web/search
leakage. The no-CodeStory quality result is emitted separately as
`without_quality_passes` and `quality_pass_delta` so baseline failure remains
visible without being misattributed as a CodeStory-side regression.

For faster iteration on runtime packet fixes, use packet probes before nested
agents:

```powershell
node scripts\codestory-agent-ab-score.mjs `
  --packet-gate --packet-probe-jobs 4 `
  --prepare-codestory-jobs 2 `
  --task-ids <comma-separated-task-ids> `
  --out-dir target\agent-benchmark\<run-name>
```

The packet gate runs cold `codestory-cli packet` probes first, with independent
rows parallelized by `--packet-probe-jobs`. Only tasks whose packet manifest
quality passes are sent to the nested A/B harness. If no task passes the packet
gate, the wrapper emits `packet_gate_*` metrics and exits non-zero before
nested agents run. Pass `--allow-empty-packet-gate` only for exploratory
diagnostics where an empty nested A/B run is intentional.
Rows that fail because the packet process temporarily cannot reach mandatory
retrievals are retried once, serially, in `packet-probes-retry`; the wrapper
emits `packet_gate_retry_tasks` plus retry artifact paths and uses the merged
quality-debug rows for A/B selection. Content-quality failures are not retried.
Use `--packet-gate-improved-from <previous-run-dir>` when iterating on runtime
packet fixes; then a task must pass the current packet gate and improve over
the previous packet-probe `quality-debug.json` or A/B `reanalyzed-runs.jsonl`
packet-prelude manifest score before nested agents are launched.

For anti-overfit language work, run packet probes with production defaults and
keep exact benchmark probes behind manifests, explicit request probes, or
`CODESTORY_EVAL_PROBES=1` diagnostics only. Do not treat general
framework/domain semantics as overfit when they apply to real projects.
Keep run-specific packet-runtime results in ignored `target/` artifacts and the
reviewing PR, issue, or release note. This page documents the harness contract
and commands; it should not become a historical benchmark ledger.

The lower-level packet runtime mode can also be run directly with row-level
parallelism:

```powershell
node scripts/codestory-agent-ab-benchmark.mjs `
  --packet-runtime --packet-runtime-mode both `
  --task-suite language-expansion-holdout `
  --repeats 3 `
  --materialize-repos `
  --jobs 4 --prepare-codestory-jobs 2 `
  --codestory-cli target/release/codestory-cli.exe `
  --out-dir target/agent-benchmark/<run-name> `
  --timeout-ms 180000 `
  --max-source-reads-after-packet 0 `
  --publishable
```

This mode runs only CodeStory packet probes and does not start nested agents.
For the language-expansion eval lane, `--jobs 4` is valid row concurrency.
Keep `--prepare-codestory-jobs` lower than packet row concurrency; use `2` for
examples unless a file already has `1` for serial prep.
Add `--task-ids <comma-separated-task-ids>` only for targeted diagnostics; a
task-filtered run is not the full-suite promotion shape.

Nested A/B runs can use `--jobs N` too, but the harness parallelizes only
independent repo groups. Arms, repeats, and multiple tasks for the same repo
stay serial so both arms do not mutate the same checkout at the same time.

When only CodeStory runtime packet behavior changed, reuse matching baselines:

```powershell
node scripts/codestory-agent-ab-score.mjs `
  --packet-gate --packet-probe-jobs 4 `
  --packet-gate-improved-from target/agent-benchmark/<previous-run> `
  --task-ids <comma-separated-task-ids> `
  --reuse-baseline-from target/agent-benchmark/<previous-run> `
  --out-dir target/agent-benchmark/<run-name>
```

Baseline reuse is strict. The benchmark reuses only `without_codestory` rows
whose repo, task id, repeat, and task manifest snapshot match the current run.
It reanalyzes the old raw row with the current analyzer, copies stdout/stderr
and baseline-context artifacts into the new output directory, and annotates the
row with `reused_from`. Stale `--reuse-baseline-from` or fixed no-CodeStory
comparisons are development diagnostics unless the current harness accepts
matching fingerprints, and they are never enough for packet-runtime promotion
by themselves. Do not reuse baselines across manifest or scorer changes; rerun
the no-CodeStory arm in those cases.

Web search, browser tools, remote URLs, and upstream mirrors are not allowed in
local pinned-repo A/B runs. Publishable gating reports external web/search tool
calls as blockers instead of treating them as local repository exploration.
Publishable gating also rejects rows that are missing wall time, total token
usage, observed tool-call count, or command-count accounting. A publishable
`without_codestory` row must inspect the local repository without CodeStory; a
model-prior answer with zero local commands is not valid baseline evidence.

On Windows, nested sandboxed runner commands can fail before local commands
launch with `CreateProcessWithLogonW failed: 1326`. Treat those rows as invalid
local-repo evidence. For local smoke verification on a trusted checkout, rerun
with the harness's trusted-checkout mode and confirm the summary shows local
command/tool counts and zero web searches.

Do not make public savings claims from these fixtures. They only prove
transcript analyzer/parser and scorer behavior. Promotion evidence still
requires real benchmark runs with raw transcripts, repeated medians, and quality
thresholds.

## README with/without row

The [README evaluation section](../../README.md#evaluation) keeps
two recorded tiers: the focused `readme-with-without` task and a suite-total row
for the 18-task `language-expansion-holdout` manifest
[`language-support-ab.task.json`](../../benchmarks/tasks/language-expansion-holdout/language-support-ab.task.json).
Latest recorded medians, ranges, and per-task rows:
[language-expansion-holdout stats](language-expansion-holdout-stats.md).

**Key concepts for benchmark harness verification:**

- **Transcript analyzer/parser and scorer**: The harness exposes pure analyzer/scorer functions and keeps a built-in fixture smoke test.
- **Runner JSONL tool category counts**: The harness counts runner JSONL tool categories for web search, MCP tool calls, command execution, function calls, and other tool calls.
- **Direct source-read accounting**: The harness accounts for direct source reads across the supported language extension set, including Dart, Bash, HTML, CSS, and SQL.
- **Ordinary source reads after the first successful packet command**: The harness counts source reads that occur after the first successful packet command.
- **Harness-run no-CodeStory local-context preludes and CodeStory packet preludes**: The harness measures both no-CodeStory local-context preludes and CodeStory packet preludes as measured first-context commands.
- **Duplicate file reads by normalized path**: The harness counts duplicate file reads by normalized path.
- **Expected file, symbol, claim, and citation recall**: The harness verifies expected file, symbol, claim, and citation recall.
- **Missed anchors as quality evidence**: The harness treats missed anchors as quality evidence, separate from operational run status.
- **Forbidden-claim scoring**: The harness avoids contradicted positive-claim false positives, including hyphenated terms such as `whitespace-only`.
- **Publishable blockers**: The harness reports external web/search tool calls as blockers and rejects rows missing wall time, total token usage, observed tool-call count, or command-count accounting.

Historical comparison artifact:
`target/agent-benchmark/language-expansion-holdout-20260617-post-quality-hardening-j2`
(without baseline reused from `language-expansion-holdout-20260617-baseline-j4`).
