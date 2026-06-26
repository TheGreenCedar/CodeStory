# Language expansion holdout stats

**Audience:** Evidence record — not an install guide.

Scoped with/without evidence for the [README evaluation section](../../README.md#evaluation).

Task manifest:
[`language-support-ab.task.json`](../../benchmarks/tasks/language-expansion-holdout/language-support-ab.task.json)
— 18 public OSS packages, one architecture question each, parser-backed and
structural language profiles.

## Recorded paired run

| Field | Value |
| --- | --- |
| Comparison artifact | `target/agent-benchmark/language-expansion-holdout-20260617-post-quality-hardening-j2` |
| Without baseline | `target/agent-benchmark/language-expansion-holdout-20260617-baseline-j4` (reused, not rerun) |
| Date | 2026-06-17 |
| Tasks | 18 |
| Repeats | 3 per arm |
| Sidecars | `retrieval_mode: full` |

Reproduction shape:

```powershell
node scripts/codestory-agent-ab-benchmark.mjs `
  --task-suite language-expansion-holdout `
  --arms without_codestory,with_codestory `
  --repeats 3 `
  --materialize-repos `
  --prepare-codestory-cache `
  --reuse-baseline-from target/agent-benchmark/language-expansion-holdout-20260617-baseline-j4 `
  --codestory-cli target/release/codestory-cli.exe `
  --out-dir target/agent-benchmark/language-expansion-holdout-<stamp> `
  --timeout-ms 600000
```

## Suite totals

| Metric | Without | With | Change |
| --- | ---: | ---: | --- |
| Context tokens | 9,692,559 | 5,514,580 | −43% |
| Repeat-task wall time | 7,943s | 4,343s | −45% |
| Tool calls | 475 | 60 | −87% |
| Commands | 471 | 54 | −89% |
| Direct source reads | 417 | 0 | −100% |
| All-in wall time | 7,944s | 4,554s | −43% |

All-in wall time includes CodeStory cache prep and packet preludes on the with arm.

## Per-task medians

Across all 18 tasks (median of each task's 3-run median):

| Metric | Without | With | Median change | Range |
| --- | ---: | ---: | ---: | --- |
| Context tokens | 182k | 33k | −77% | −43% to +87% |
| Repeat-task wall time | 146s | 63s | −54% | −16% to +79% |
| Tool calls | 9 | 1 | −89% | 1 vs 9–10 |

Most tasks dropped into the low-30k token band with one `packet` call. Outliers in
this run include `cpp-fmt-formatting-flow` (more tokens with CodeStory) and a few
tasks where packet manifest quality still failed even when the run completed.

## Per-task rows

Median tokens / repeat-task seconds / tool calls per task:

| Task | Package | Without | With |
| --- | --- | --- | --- |
| `python-requests-session-flow` | requests | 145k / 109s / 9 | 33k / 43s / 1 |
| `java-commons-lang-string-utils` | commons-lang | 184k / 155s / 9 | 33k / 62s / 1 |
| `rust-ripgrep-search-pipeline` | ripgrep | 181k / 144s / 9 | 33k / 49s / 1 |
| `javascript-express-routing-flow` | express | 149k / 119s / 9 | 33k / 42s / 1 |
| `typescript-swr-hook-flow` | swr | 152k / 150s / 9 | 33k / 43s / 1 |
| `cpp-fmt-formatting-flow` | fmt | 194k / 183s / 9 | 278k / 136s / 2 |
| `c-redis-command-loop` | redis | 151k / 125s / 9 | 33k / 64s / 1 |
| `go-gin-route-dispatch` | gin | 199k / 160s / 9 | 33k / 41s / 1 |
| `ruby-jekyll-site-build` | jekyll | 255k / 198s / 10 | 33k / 42s / 1 |
| `php-monolog-record-flow` | monolog | 151k / 129s / 9 | 33k / 40s / 1 |
| `csharp-automapper-map-flow` | AutoMapper | 142k / 97s / 9 | 170k / 107s / 1 |
| `kotlin-okio-buffer-flow` | okio | 188k / 149s / 9 | 100k / 92s / 1 |
| `swift-alamofire-request-flow` | Alamofire | 199k / 173s / 9 | 246k / 171s / 2 |
| `dart-http-client-flow` | http | 185k / 224s / 9 | 176k / 146s / 1 |
| `bash-nvm-install-dispatch` | nvm | 209k / 148s / 9 | 189k / 76s / 1 |
| `html-mdn-form-validation` | MDN learning area | 179k / 101s / 4 | 209k / 117s / 2 |
| `css-animate-base-and-keyframes` | animate.css | 184k / 134s / 9 | 169k / 95s / 1 |
| `sql-chinook-schema-relations` | Chinook | 140k / 135s / 9 | 33k / 40s / 1 |

Source of truth: `summary.md` and `runs.jsonl` in the comparison artifact directory.

## Boundary

This is scoped A/B evidence on 18 pinned public repos, not proof that every
future repo question saves tokens or time. Do not rerun the no-CodeStory baseline
unless the harness contract changes; reuse the recorded baseline artifact when
comparing new CodeStory arms.
