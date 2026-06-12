# Agent Benchmark Harness Verification

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
- modern Codex JSONL tool category counts for web search, MCP tool calls,
  command execution, function calls, and other tool calls;
- direct source-read accounting across the supported language extension set,
  including Dart, Bash, HTML, CSS, and SQL;
- ordinary source reads after the first successful packet command;
- harness-run no-CodeStory local-context preludes and CodeStory packet preludes
  as measured first-context commands;
- duplicate file reads by normalized path;
- expected file, symbol, claim, and citation recall;
- missed anchors as quality evidence, separate from operational run status;
- publishable blockers when the `without_codestory` arm either calls CodeStory
  or never inspects the local repository.

`drill-suite` answer-quality ledgers are the repo-grounded counterpart to this
transcript scorer. Use the transcript harness to check how an agent behaved; use
`drill-suite --ledger <file>` to merge focused source-truth classifications back
into a real-repo evidence packet. Ledger claim classifications are `correct`,
`partial`, `misleading`, and `unsupported`, and the suite keeps the final
answer-quality verdict separate from green index/build mechanics.

For source-truth recall, `drill` now feeds the broad question search and bounded
supplemental searches into the verification target list. Treat those targets as
candidate files for verification, not as final answer support.

Keep `node ./scripts/codestory-agent-ab-benchmark.mjs --list` as the cheapest
configuration smoke check.

The language-support A/B suite is:

```powershell
node scripts/codestory-agent-ab-benchmark.mjs `
  --task-suite language-expansion-holdout `
  --arms without_codestory,with_codestory `
  --repeats 3 --materialize-repos --prepare-codestory-cache `
  --out-dir target/agent-benchmark/language-expansion-holdout `
  --timeout-ms 600000
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
`scripts/codestory-agent-ab-score.mjs` reuses that ledger for Autoresearch and
emits `METRIC` lines for the raw per-arm wall time, tokens, tool calls,
commands, CodeStory commands, shell searches, file-read commands, web searches,
post-packet reads, quality pass counts, packet-first pass counts, and ratios.
The primary `agent_ab_gap` penalizes with-CodeStory quality failures,
packet-first failures, post-packet source reads, and external web/search
leakage. The no-CodeStory quality result is emitted separately as
`without_quality_passes` and `quality_pass_delta` so baseline failure remains
visible without being misattributed as a CodeStory-side regression.

Web search, browser tools, remote URLs, and upstream mirrors are not allowed in
local pinned-repo A/B runs. Publishable gating reports external web/search tool
calls as blockers instead of treating them as local repository exploration.
Publishable gating also rejects rows that are missing wall time, total token
usage, observed tool-call count, or command-count accounting. A publishable
`without_codestory` row must inspect the local repository without CodeStory; a
model-prior answer with zero local commands is not valid baseline evidence.

On Windows, nested `codex exec --sandbox workspace-write` can fail before local
commands launch with `CreateProcessWithLogonW failed: 1326`. Treat those rows as
invalid local-repo evidence. For local smoke verification on a trusted checkout,
rerun with `--sandbox danger-full-access` and confirm the summary shows local
command/tool counts and zero web searches.

Do not make public savings claims from these fixtures. They only prove parser
and scorer behavior. Promotion evidence still requires real benchmark runs with
raw transcripts, repeated medians, and quality thresholds.
