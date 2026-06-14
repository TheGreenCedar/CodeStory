# CodeStory Benchmark Ledger

Decision-grade scorecard and benchmark history - too dense for the README.
Treat every row as machine-, cache-, runner-, and date-specific. Do not quote a
row as a universal savings claim without checking harness tier and setup.

Runs recorded before the 2026-05-24 harness tightening are historical unless
they are reanalyzed or rerun with answer-level expected-file/symbol recall,
immutable manifest refs, and CodeStory cache provenance. The harness now keeps
transcript-observed anchors separate from anchors actually present in the final
answer, so tool output alone cannot make a row quality-pass.

## Current Scorecard

| Lane | Current status | Public claim status |
| --- | --- | --- |
| Agent A/B quick check | The 2026-05-23 CodeStory-only quick run passed both arms, but the CodeStory arm used more tokens, more wall time, and more tool starts. | No agent savings claim. |
| Local-real Codex probe | On 2026-05-25, the narrowed `codex-exec-json-flow` live A/B repeated with a quality-passing CodeStory arm against a failing no-CodeStory arm. Latest corrected-wrapper repeat: `114,510` vs `2,209,856` tokens, `2` vs `39` observed tool calls, `117.37s` vs `262.39s`, and overhead ratio `0.183466`. | Strong exploratory evidence; no promotion claim from this task alone. |
| Local-real Sourcetrail probe | On 2026-05-25, the `sourcetrail-indexing-to-storage` live A/B passed with CodeStory after source-group/indexing/storage packet fixes. CodeStory used `269,363` vs `5,697,852` tokens, `2` vs `105` observed tool calls, `138.92s` vs `532.68s`, `0` vs `87` source reads, and overhead ratio `0.10904`. | Strong second-repo exploratory evidence; still not promotion-grade because it is one repeat using a local existing cache. |
| Local-real VS Code probe | On 2026-05-25, the `vscode-workbench-extension-host` packet holdout moved from partial coverage to a sufficient packet, then the live A/B passed with CodeStory after workbench/extension-host packet fixes. CodeStory used `1,070,153` vs `7,296,578` tokens, `2` vs `115` observed tool calls, `329.69s` vs `626.08s`, `0` vs `71` source reads, and overhead ratio `0.230215`. A follow-up release incremental refresh repaired the stale cache provenance, moving VS Code freshness from `74` new files to `0`. | Strong third-repo exploratory evidence; still not promotion-grade because it is one repeat and the no-CodeStory arm failed quality. |
| Local-real drill-suite probe | On 2026-05-25, a four-repo `drill-suite` matrix exposed a real CodeStory cache-reuse blocker, stale Codex anchor selections, VS Code indexing-error blockage, and Sourcetrail source-truth-only bridges. After the CodeStory cache fix and Rust receiver/return-chain graph pass, this repo's one-case drill is still degraded but now resolves `11/11` anchors with `28/55` graph bridges, `27` partial bridges, and `0` unresolved bridges. | Diagnostic product evidence only; the remaining target is store/workspace execution-plan and snapshot/projection bridge coverage. |
| Strict packet-first rows | Several with-CodeStory public-checkout rows passed quality, packet-first, and zero ordinary source reads after packet. | Behavior evidence only; paired savings still needs broader quality-passing baselines. |
| Packet runtime | Public-core warm stdio and cold CLI packet rows passed repeated publishable quality gates. | Runtime evidence, not agent-token savings. |
| Repo-scale cold index/read timing | The current timing source is the latest row in [codestory-e2e-stats-log.md](codestory-e2e-stats-log.md). | Current only after a fresh row is logged for the relevant change. |
| Warm stdio smoke | The current warm-loop timing source is [codestory-stdio-warm-loop-stats.md](codestory-stdio-warm-loop-stats.md). | Smoke evidence for the persistent read surface. |

## What Is Solid

- CodeStory can produce quality-passing packet-first answers on selected public
  tasks while avoiding ordinary source reads after the answer packet.
- Repeated packet-runtime rows show `packet` can fit inside an agent workflow
  budget in both cold CLI and warm stdio modes.
- The local-real harness now separates first-index setup cost from timed
  cache-reuse agent work, blocks stale or semantic-empty caches from
  publishable evidence, and records useful-context density for final-answer
  context instead of raw packet volume alone.
- The quality-passing local-real Codex live A/B has now repeated on the same
  task with the corrected wrapper.
- Sourcetrail now adds a second realistic repo where the CodeStory arm passed
  quality and avoided source reads while the no-CodeStory arm failed quality
  after broad exploration.
- VS Code now adds a large TypeScript repo where the packet planner can find the
  workbench startup, extension service, extension host manager, extension-host
  activation, and command execution anchors without follow-up commands, and the
  live CodeStory arm passed quality while using far fewer tools and source
  reads than the no-CodeStory arm.
- The VS Code cache freshness issue behind the first local-real row is now
  understood and fixed: TypeScript/TSX factory-call superclass extraction no
  longer crashes on `extends mock<T>()`, failed attempts are recorded as
  incomplete files with attached errors, and `../vscode` now reports
  `10,491/10,491` indexed files as fresh after incremental refresh.
- CodeStory's own active cache can now recover from stale incremental
  projection cleanup where cross-file callable state points at a deleted node;
  release incremental refresh reports fresh inventory with `150/150` indexed
  files, `0` errors, and `7,794` semantic docs.
- The tightened CodeStory drill now exposes the CLI-to-runtime-to-indexer path
  mostly as graph evidence: Rust receiver and return-chain resolution moved the
  case from `3/55` graph bridges to `28/55`, while preserving explicit
  source-truth-only status for the remaining unproven bridge pairs.
- Repo-scale timing history is tracked in the stats log instead of copied into
  prose that silently drifts.

## What Is Not Claimed

This page does not claim that CodeStory generally reduces agent cost, token
count, wall time, or tool calls. General savings claims require repeated
controlled with/without-agent measurements from the benchmark harness, not one
exploratory row or representative estimates.

The 2026-05-25 Codex, Sourcetrail, and VS Code local-real rows are explicitly
non-promotional. They show a CodeStory advantage on three realistic tasks, but
they are still single-run or same-task exploratory measurements using local
cache state. The VS Code cache now has fresh provenance after the follow-up
repair, but public savings language still needs repeated controlled rows, clean
pinned checkout provenance, and at least one holdout that was not tuned during
the implementation loop.

The 2026-05-25 `drill-suite` rows are also non-promotional. They are designed to
find grounding failures before an agent A/B run, and the current CodeStory case
still falls back to source-truth-only evidence for `27/55` bridge pairs.

## Proof Tier Ladder

Use the highest tier actually reached when describing a row. Do not promote a
lower tier into a broader claim just because the command exited successfully.

| Proof tier | Required evidence | Can claim | Cannot claim |
| --- | --- | --- | --- |
| Stats-only local regression signal | `codestory_repo_e2e_stats` completed with skip allowances or without prepared full sidecars. | Local timing, indexing, and cache-shape regression signal for the current checkout. | Full sidecar readiness, agent packet/search readiness, real-repo release coverage, or performance promotion. |
| Full sidecar readiness proof | Zoekt, Qdrant, SCIP, and llama.cpp are running; `retrieval index --refresh full` succeeds; `retrieval status --format json` reports `retrieval_mode: "full"` and product backend fields. | Agent-facing packet/search readiness for the verified workspace and cache state. | General quality, cross-repo coverage, or benchmark savings. |
| Real-repo drill proof | Prepared real-repo drill manifests run without skip allowances and produce expected evidence packets, source-truth checks, and verdicts. | The release path was exercised beyond the CodeStory checkout on the named drill cases. | Generalized agent savings or promotion-grade performance. |
| Promotion-grade benchmark proof | Controlled baseline and candidate benchmark rows use pinned refs, comparable cache state, sidecar status, answer-level quality gates, and no-regression thresholds. | Cautious performance or retrieval-quality promotion for the measured scope. | Universal savings, untested repos, or environments outside the recorded setup. |

## Promotion Rules

- Use the same project, cache state, semantic backend, command flags, runner,
  model, and sample shape when comparing before/after results.
- Do not promote a speed win if expected anchors, answer-level quality, protocol
  cleanliness, or semantic-doc reuse regress.
- Treat small-fixture warm-loop numbers as smoke evidence, not repo-scale
  product proof.
- Append current repo-scale timing rows to
  [codestory-e2e-stats-log.md](codestory-e2e-stats-log.md) when default
  indexing, semantic persistence, embedding reuse, or cold-start behavior
  changes.

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

## Harness Contract

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
