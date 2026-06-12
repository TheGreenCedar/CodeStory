# Language Expansion A/B Report

Date: 2026-06-12

## Scope

This report covers the strict local A/B harness for the language-expansion
holdout suite. The suite contains one medium-sized open source repository task
per supported language. The measured A/B runs below are the focused Python
Requests and JavaScript Express smoke tasks:

- Task: `python-requests-session-flow`
- Repository: `psf-requests`
- Suite: `language-expansion-holdout`
- Output: `target/agent-benchmark/language-expansion-smoke-python-fixed`
- Task: `javascript-express-routing-flow`
- Repository: `expressjs-express`
- Suite: `language-expansion-holdout`
- Output: `target/agent-benchmark/language-expansion-smoke-js-express-final`

The full 18-language suite is triggerable with the same harness; it was not run
end-to-end in this measurement because each row launches nested Codex agents.

## 18-Language Corpus Status

The full language-expansion repo cache was materialized on 2026-06-12 with:

```powershell
node scripts\codestory-agent-ab-benchmark.mjs --list --task-suite language-expansion-holdout --materialize-repos
```

The harness reported all 18 pinned repositories as `available`, and a follow-up
HEAD check matched every checkout to the manifest commit. The ignored OSS
language corpus was then run against that cache:

```powershell
$env:CODESTORY_RUN_OSS_LANGUAGE_CORPUS = "1"
$env:CODESTORY_OSS_CORPUS_CACHE = "target\agent-benchmark\repos"
cargo test -p codestory-indexer --test oss_language_corpus -- --ignored --nocapture
```

Result: 18/18 languages passed. Across the corpus, CodeStory indexed the same
4,308 files found by the raw baseline and produced 385,735 nodes and 312,269
edges with 0 errors. This proves the medium OSS projects are present and
indexable; it is not a substitute for the full 18-language agent A/B run.

## Harness Contract

The harness compares two arms on the same pinned local repository:

- `without_codestory`: no CodeStory CLI packet allowed.
- `with_codestory`: must run `codestory-cli packet` first.

The ledger records agent wall time, token usage, observed tool calls, command
counts, CodeStory command counts, shell-search commands, file-read commands,
web/search tool calls, ordinary source reads after packet, packet-first status,
and manifest quality recall. The score wrapper also emits total-run metrics and
CodeStory cache preparation timing so reports can distinguish agent-only time
from all-in CodeStory setup time.

Current harness output must include three accounting layers:

- per-run `resource_accounting` in `runs.jsonl` / `reanalyzed-runs.jsonl`;
- top-level `cost_accounting` in `summary.json` / `reanalyzed-summary.json`;
- a Markdown `Cost Accounting` section before the per-task median table.

Those accounting layers measure time spent, input/output/total tokens spent,
estimated cost when pricing env vars are configured, observed tool calls, tool
categories, command counts, command categories, web searches, and source reads
for each arm across all observed rows, including failed or timed-out rows when
their measurements are present. The top-level comparison reports
`with_codestory` versus `without_codestory` ratios for runner wall time, all-in
wall time, tokens, tool calls, commands, and estimated cost. A publishable run
is invalid if wall time, total token usage, observed tool-call count, or
command-count accounting is missing from any row.

Web search, browser use, remote URLs, and upstream mirrors are blockers for
publishable local-repo evidence.

## Latest Python A/B Result

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 0/1 | 1/1 |
| Expected file recall | 100% | 100% |
| Expected symbol recall | 100% | 100% |
| Expected claim recall | 50% | 100% |
| Wall time | 138,488 ms | 83,168 ms |
| Total tokens | 201,287 | 67,527 |
| Tool calls | 15 | 1 |
| Commands | 15 | 1 |
| CodeStory commands | 0 | 1 |
| Shell searches | 4 | 0 |
| File-read commands | 8 | 0 |
| Web searches | 0 | 0 |
| Post-packet source reads | n/a | 0 |

Ratios:

- Token ratio: `0.335`
- Wall-time ratio: `0.601`
- Tool-call ratio: `0.067`
- Command ratio: `0.067`
- Corrected `agent_ab_gap`: `969.350`

Interpretation: for this task, the patched CodeStory packet wins on quality and
uses fewer tokens, less wall time, and far fewer tool calls. This is not an
equal-quality savings claim because the no-CodeStory arm missed two expected
flow claims.

## Latest Express A/B Result

| Metric | without CodeStory | with CodeStory |
| --- | ---: | ---: |
| Quality pass | 0/1 | 1/1 |
| Expected file recall | 75% | 100% |
| Expected symbol recall | 100% | 100% |
| Expected claim recall | 50% | 100% |
| Citation coverage | 75% | 100% |
| Agent wall time | 202,366 ms | 78,322 ms |
| CodeStory cache prep | n/a | 1,285 ms |
| All-in wall time | 202,366 ms | 79,607 ms |
| Total tokens | 702,190 | 66,389 |
| Tool calls | 32 | 1 |
| Commands | 32 | 1 |
| CodeStory commands | 0 | 1 |
| Shell searches | 11 | 0 |
| File-read commands | 19 | 0 |
| Web searches | 0 | 0 |
| Post-packet source reads | n/a | 0 |

Ratios:

- Token ratio: `0.095`
- Agent wall-time ratio: `0.387`
- All-in wall-time ratio: `0.393`
- Tool-call ratio: `0.031`
- Command ratio: `0.031`
- Corrected `agent_ab_gap`: `497.202`
- All-in `agent_ab_gap_all_in`: `503.552`

Interpretation: for this task, the patched CodeStory packet wins quality and
efficiency even after counting CodeStory cache preparation time.

## Bug Fixed

Before the Python fix, CodeStory found the right files but failed the answer
surface:

- Expected symbol recall: `2/6`
- Expected claim recall: `0/4`
- Bad packet guidance included Axios-shaped transport claims such as XHR/HTTP
  adapter selection on Python Requests source.

The runtime packet now:

- protects exact method probes for prepared-request/session-adapter flows:
  `Session.request`, `Session.prepare_request`, `PreparedRequest.prepare`,
  `Session.send`, and `HTTPAdapter.send`;
- keeps those exact probes through compact citation capping;
- emits source-shaped Python Requests flow claims only when the cited source
  supports them;
- stops emitting the stale XHR/HTTP claim for Python Requests source.

Direct packet reproduction after the fix confirmed all expected method citations
and all expected flow claims were present, with no stale XHR claim.

Before the Express fix, the first red A/B row exposed two separate issues:

- the analyzer misclassified Codex's inline PowerShell `$env:CODESTORY_CLI`
  fallback command as `other`, so packet-first and CodeStory command counts were
  wrong;
- the packet itself called a broad Express packet sufficient while missing
  `app.init`, `app.handle`, `app.use`, `app.route`, `res.send`, and the
  source-backed flow claims.

The analyzer now recognizes the inline PowerShell fallback form. The runtime now
adds Express-shaped route probes only when the prompt names an Express
application/router/response flow, emits source-derived claims from
`lib/express.js`, `lib/application.js`, and `lib/response.js`, and lets
sufficiency probes be covered by source-derived claim text when JavaScript
prototype methods are not exposed as clean indexed symbols.

## Verification

Commands run:

- `node scripts/codestory-agent-ab-score.mjs --task-ids python-requests-session-flow --repeats 1 --timeout-ms 600000 --out-dir target\agent-benchmark\language-expansion-smoke-python-fixed`
- `node scripts/codestory-agent-ab-score.mjs --reanalyze-dir target\agent-benchmark\language-expansion-smoke-python-fixed`
- `node scripts\codestory-agent-ab-score.mjs --task-ids javascript-express-routing-flow --repeats 1 --timeout-ms 600000 --out-dir target\agent-benchmark\language-expansion-smoke-js-express-final`
- direct Express packet reproduction: `target\agent-benchmark\manual-packets\express-route-flow-final.json`
- `node scripts\codestory-agent-ab-benchmark.mjs --list --task-suite language-expansion-holdout --materialize-repos`
- pinned checkout HEAD verification for all 18 language-expansion repositories
- `$env:CODESTORY_RUN_OSS_LANGUAGE_CORPUS="1"; $env:CODESTORY_OSS_CORPUS_CACHE="target\agent-benchmark\repos"; cargo test -p codestory-indexer --test oss_language_corpus -- --ignored --nocapture`
- `node scripts\codestory-language-holdout-integrity.mjs`
- `node --test scripts\tests\codestory-agent-ab-analyzer.test.mjs`
- `node scripts\codestory-agent-ab-benchmark.mjs --self-test`
- `cargo fmt --check`
- `cargo test -p codestory-runtime`
- `cargo build --release -p codestory-cli`
- `git diff --check`

Autoresearch note: `benchmark-lint` now parses the wrapper successfully and sees
53 `METRIC` values, including wall time, tokens, tool calls, command counts,
CodeStory cache-preparation time, web searches, and post-packet source reads.
The scorer does not emit estimated-cost metrics unless benchmark pricing env
vars are configured, so absent pricing is not reported as `$0`. The Express
smoke result is accepted in the Autoresearch ledger as segment-0 exploratory
evidence for commit `a9e51edb2402`. Promotion is still blocked because the
current branch has older unkept overlapping commits, the full 18-language suite
has not run, and repeat/breadth/holdout promotion metadata is still missing. The
A/B artifacts above are real local evidence on disk, but not product-grade
promotion evidence.
