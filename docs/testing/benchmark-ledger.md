# CodeStory Benchmark Ledger

This ledger keeps detailed benchmark history that is too dense for the README
scorecard. Treat every row as machine-, cache-, runner-, and date-specific.
Promote only rows that pass the current harness gates documented in
[benchmark-results.md](benchmark-results.md).

## Agent A/B History

The 2026-05-23 quick CodeStory repo run used:

```sh
node ./scripts/codestory-agent-ab-benchmark.mjs --quick --repos codestory --repeats 3 --timeout-ms 900000 --sandbox danger-full-access --publishable --out-dir target/agent-benchmark/codestory-quick-2026-05-23-r3
```

It was a real baseline, not a savings claim. The without-CodeStory arm passed
`3/3` with median `214.90s`, `1,605,030` total tokens, and `29` tool starts.
The with-CodeStory arm passed `3/3` with median `306.24s`, `2,724,490` total
tokens, and `43` tool starts. It showed no token, wall-time, or tool-count
savings.

The packet-first diagnostic series then tightened the harness around answer
packets, `CODESTORY_CLI` injection, post-packet source-read budgets, final-answer
quality scoring, and repository/cache provenance. The first strict
with-CodeStory row for the CodeStory indexing-flow task was r20:

| Arm | Quality | Answer packet first | Wall time | Total tokens | Tool starts | Direct source reads | Reads after packet |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| With CodeStory r20 | `3/3` | `3/3` | `71.92s` | `167,102` | `2` | `0` | `0` |

Strict with-CodeStory public-checkout rows followed:

| Repo | Task class | Quality | Answer packet first | Wall time | Total tokens | Tool starts | Direct source reads |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| vite | architecture_explanation | `3/3` | `3/3` | `66.78s` | `163,815` | `2` | `0` |
| express | bug_localization | `3/3` | `3/3` | `67.39s` | `164,291` | `2` | `0` |
| mux | architecture_explanation | `3/3` | `3/3` | `65.12s` | `163,935` | `2` | `0` |
| express | symbol_ownership | `3/3` | `3/3` | `63.28s` | `165,715` | `2` | `0` |
| mux | edit_planning | `3/3` | `3/3` | `59.08s` | `100,833` | `2` | `0` |
| express | route_tracing | `3/3` | `3/3` | `62.80s` | `102,137` | `2` | `0` |

Historical paired diagnostics remain useful for context, but rows produced
before the 2026-05-24 answer-level quality and cache-provenance gates should be
rerun or reanalyzed before promotion:

| Task | Without CodeStory | With CodeStory | Historical note |
| --- | --- | --- | --- |
| express response send bug localization | `3/3`, `136.57s`, `635,793` tokens, `18` tool starts | `3/3`, `67.39s`, `164,291` tokens, `2` tool starts | Pre-gate scorer showed fewer tokens, wall time, and tool starts. |
| mux router matching flow | `3/3`, `123.37s`, `321,908` tokens, `16` tool starts | `3/3`, `65.12s`, `163,935` tokens, `2` tool starts | Pre-gate scorer showed fewer tokens, wall time, and tool starts. |
| express response symbol ownership | `3/3`, `125.10s`, `397,110` tokens, `15` tool starts | `3/3`, `63.28s`, `165,715` tokens, `2` tool starts | Covers `symbol_ownership`. |
| mux CORS middleware edit plan | `3/3`, `115.12s`, `364,175` tokens, `14` tool starts | `3/3`, `59.08s`, `100,833` tokens, `2` tool starts | Covers edit planning without broad file reads. |
| express application routing flow | `3/3`, `127.46s`, `334,231` tokens, `15` tool starts | `3/3`, `62.80s`, `102,137` tokens, `2` tool starts | Covers route tracing. |

The public-core subset with CodeStory quality-passed for CodeStory and Vite, but
strict reanalysis showed the answer packet was not the first repository-context
command. Keep those rows diagnostic until rerun with the stricter prompt and
publishable gate.

## Packet Runtime History

On 2026-05-23, the release CLI completed three-repeat packet runtime runs
against the full public-core manifest suite in both warm stdio and cold CLI
modes:

```sh
node ./scripts/codestory-agent-ab-benchmark.mjs --packet-runtime --task-suite public-core --repeats 3 --packet-runtime-mode warm-stdio --codestory-cli ./target/release/codestory-cli --out-dir target/agent-benchmark/packet-runtime-public-core-warm-r8 --timeout-ms 120000 --publishable
node ./scripts/codestory-agent-ab-benchmark.mjs --packet-runtime --task-suite public-core --repeats 3 --packet-runtime-mode cold-cli --codestory-cli ./target/release/codestory-cli --out-dir target/agent-benchmark/packet-runtime-public-core-cold-r9 --timeout-ms 120000 --publishable
```

Across both modes, all `108` packet rows passed operationally and quality gates.
Every row reported `sufficient` packet coverage with `0` sufficiency/quality
mismatches. Warm stdio task medians ranged from `2.69s` to `3.60s`, with an
aggregate task median of `3.13s`; cold CLI task medians ranged from `4.22s` to
`5.76s`, with an aggregate task median of `4.86s`.

## Methodology

The agent A/B harness runs the same repository prompt in two arms:

- `without_codestory`: avoid CodeStory and use normal repository exploration.
- `with_codestory`: use CodeStory grounding first, run `packet` for broad
  repository questions, then ordinary source reads only for named gaps.

The harness writes raw stdout/stderr per run, a JSONL run log, transcript
analysis, a machine summary, and a Markdown summary under
`target/agent-benchmark/<timestamp>`. Compare medians across successful repeats
for the same runner, repository set, prompt set, cache policy, semantic backend,
and model.

Use `--publishable` only when the selected runner reports token usage and every
run succeeds. For agent A/B rows, `--publishable` also requires with-CodeStory
runs to execute an answer packet with `--question` first and stay within the
explicit post-packet ordinary source-read budget supplied through
`--max-source-reads-after-packet <n>`. Use `0` for packet-only promotion
evidence; use a larger number only when the row is intentionally CodeStory-first
but not packet-only. Publishable rows must carry clean repository provenance
pinned to a full 40-character Git commit SHA plus CodeStory cache provenance
from `doctor --format json`. Tags are not accepted for publishable
materialized-repo rows because they can be moved after the benchmark is
published.

Packet runtime runs compare cold CLI `packet` invocations with warm
`serve --stdio` packet calls. They are runtime rows, not agent-token rows, and
still use manifest quality gates before promotion.

## Commands

```sh
node ./scripts/codestory-agent-ab-benchmark.mjs --list
node ./scripts/codestory-agent-ab-benchmark.mjs --quick --repos codestory --repeats 3 --timeout-ms 600000 --publishable --max-source-reads-after-packet 0
node ./scripts/codestory-agent-ab-benchmark.mjs --task-suite public-core --list
node ./scripts/codestory-agent-ab-benchmark.mjs --task-suite public-core --task-ids codestory-indexing-flow,vite-dev-server-architecture --arms with_codestory --repeats 3 --max-source-reads-after-packet 0 --allow-failures
node ./scripts/codestory-agent-ab-benchmark.mjs --reanalyze-dir target/agent-benchmark/<run-dir>
node ./scripts/codestory-agent-ab-benchmark.mjs --task-suite public-core --materialize-repos --list
node ./scripts/codestory-agent-ab-benchmark.mjs --packet-runtime --task-suite public-core --repeats 3
```

Cold repo-scale timings are owned by
[codestory-e2e-stats-log.md](codestory-e2e-stats-log.md). Warm stdio loop
timings are owned by
[codestory-stdio-warm-loop-stats.md](codestory-stdio-warm-loop-stats.md).
