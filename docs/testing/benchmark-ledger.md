# CodeStory Benchmark Ledger

This page is the current benchmark scorecard. It should answer a reader's first
question quickly: did CodeStory help, hurt, or still need proof?

Short answer: the June 17, 2026 fixed-baseline comparison is a strong
development win, but it is still not a general public answer-quality claim.
CodeStory is quality-equal or better on every measured language task, reduces
wall time, tokens, commands, and direct source reads, and keeps the
no-CodeStory arm as a fixed control artifact instead of rerunning it.

## Plain English

| Term in raw artifacts | Reader meaning |
| --- | --- |
| Answer bundle | A CodeStory response with cited files, likely owners, and the explanation it can support. |
| Quality pass | The answer covered the expected files and explanation points for that task. |
| Files found | How many expected files CodeStory found or cited. |
| Explanation points | How many expected claims were actually present in the answer. |
| Follow-up needed | Extra commands CodeStory said a user should run because the answer bundle was incomplete. |
| Comparison run | The same task run twice: once without CodeStory and once with CodeStory. |

## Current Answer

| Question | Answer |
| --- | --- |
| Is there benchmark data from this week? | Yes: a fixed no-CodeStory baseline, a full CodeStory-only confirmation, and an offline reused-baseline composite with three repeats per language task. |
| Does the fresh data show CodeStory can be useful? | Yes. In the current composite, CodeStory succeeded on `54/54` rows, quality-passed `54/54`, used `0.426x` all-in wall time, `0.221x` tokens, `54` commands instead of `471`, and `0` source reads instead of `417`. |
| Can we claim general answer-quality superiority yet? | Not as a public promotion claim. The current evidence is an offline composite over the fixed control and still needs repeat, freshness, breadth, and promotion metadata before broad language/framework claims. The fixed control does not need to be rerun for those loops. |

## Current Evidence At A Glance

```mermaid
flowchart LR
    run["June 17 reused-baseline composite<br/>18 tasks x 3 repeats"]
    cs["with CodeStory<br/>54/54 quality"]
    base["fixed no-CodeStory<br/>24/54 quality"]
    ops["lower wall, tokens,<br/>commands, source reads"]
    quality["quality equal or better<br/>on every measured row"]
    run --> cs
    run --> base
    cs --> ops
    cs --> quality
```

| Lane | Fresh result | What it means | Claim status |
| --- | --- | --- | --- |
| Fixed-baseline language comparison | With CodeStory: `54/54` success, `54/54` quality, `3,383,687 ms` all-in wall, `2,141,124` tokens, `54` commands, `0` source reads. Without CodeStory: `54/54` success, `24/54` quality, `7,943,578 ms`, `9,692,559` tokens, `471` commands, `417` source reads. | CodeStory materially reduced operating cost, stayed packet-first, and is quality-equal or better on every measured language task. | Current development comparison; offline reused-baseline composite, not final public promotion proof. |
| 18-language packet-runtime diagnostic | `18/18` answer bundles completed; `12/18` passed full quality; `17/18` found all expected files; median elapsed time was `11.14s`. | CodeStory usually found the right files, but explanation quality was uneven even before the paired run. | Runtime and coverage evidence, not a comparison-run savings claim. |
| TypeScript React library comparison | With CodeStory: `32,168` tokens, `42.67s` elapsed including setup check, `1` command, `0` source files opened, quality `1/1`. Without CodeStory: `535,632` tokens, `201.38s`, `35` commands, `30` source files opened, quality `0/1`. | CodeStory was clearly useful on this task. | Strong historical single-task evidence, not a general savings claim. |

## Current Full A/B By Language

Source: `target/agent-benchmark/language-expansion-holdout-20260617-fixed-baseline-vs-round24-codeonly-offline/reanalyzed-summary.md`

| Language | Example project | With CodeStory quality | Baseline quality | Read |
| --- | --- | ---: | ---: | --- |
| Python | requests | `3/3` | `3/3` | Quality tie; CodeStory uses `3` commands and `0` source reads versus baseline `27` commands and `24` source reads. |
| Java | Commons Lang | `3/3` | `0/3` | CodeStory quality win with lower command and source-read cost. |
| Rust | ripgrep | `3/3` | `0/3` | CodeStory quality win with lower command and source-read cost. |
| JavaScript | Express | `3/3` | `3/3` | Quality tie; CodeStory wins operationally and stays packet-first. |
| TypeScript | SWR | `3/3` | `0/3` | CodeStory quality win with lower command and source-read cost. |
| C++ | fmt | `3/3` | `3/3` | Quality tie; CodeStory wins operationally and stays packet-first. |
| C | Redis | `3/3` | `3/3` | Quality tie; CodeStory wins operationally and stays packet-first. |
| Go | Gin | `3/3` | `3/3` | Quality tie; CodeStory wins operationally and stays packet-first. |
| Ruby | Jekyll | `3/3` | `2/3` | CodeStory quality win with lower command and source-read cost. |
| PHP | Monolog | `3/3` | `2/3` | CodeStory quality win with lower command and source-read cost. |
| C# | AutoMapper | `3/3` | `0/3` | CodeStory quality win with lower command and source-read cost. |
| Kotlin | Okio | `3/3` | `0/3` | CodeStory quality win with lower command and source-read cost. |
| Swift | Alamofire | `3/3` | `1/3` | CodeStory quality win with lower command and source-read cost. |
| Dart | http | `3/3` | `1/3` | CodeStory quality win with lower command and source-read cost. |
| Bash | nvm | `3/3` | `3/3` | Quality tie; CodeStory wins operationally and stays packet-first. |
| HTML | MDN forms | `3/3` | `0/3` | CodeStory quality win with lower command and source-read cost. |
| CSS | animate.css | `3/3` | `0/3` | CodeStory quality win with lower command and source-read cost. |
| SQL | Chinook | `3/3` | `0/3` | CodeStory quality win with lower command and source-read cost. |

## Packet Runtime Performance By Language

This older packet-runtime diagnostic remains useful for packet composition and
claim-quality work, but the June 17 fixed-baseline composite is the current
development comparison source.

Source: `target/agent-benchmark/language-expansion-packet-runtime-current-after-claim-fixes/packet-runtime-summary.json`

Generated: `2026-06-14T01:10:57.739Z`

Sorted best to worst: quality pass first, then higher explanation coverage,
higher file coverage, lower elapsed time, and fewer follow-up commands.

| Language | Example project | Task kind | Result | Elapsed | Files found | Explanation points | Follow-up needed |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| Bash | nvm | install dispatch | pass | `5.04s` | `100%` | `100%` | `0` |
| SQL | Chinook | schema relations | pass | `5.12s` | `100%` | `100%` | `8` |
| CSS | animate.css | animation base/keyframes | pass | `5.60s` | `100%` | `100%` | `8` |
| Go | Gin | route dispatch | pass | `6.50s` | `100%` | `100%` | `0` |
| JavaScript | Express | routing flow | pass | `7.71s` | `100%` | `100%` | `8` |
| TypeScript | SWR | React data-fetching hook flow | pass | `7.78s` | `100%` | `100%` | `0` |
| C++ | fmt | formatting flow | pass | `11.86s` | `100%` | `100%` | `0` |
| Swift | Alamofire | request flow | pass | `12.89s` | `100%` | `100%` | `8` |
| Rust | ripgrep | search pipeline | pass | `13.07s` | `100%` | `100%` | `3` |
| Java | Commons Lang | string utility flow | pass | `33.03s` | `100%` | `100%` | `8` |
| C | Redis | command loop | pass | `33.10s` | `75%` | `100%` | `8` |
| Dart | http | HTTP client flow | pass | `18.63s` | `100%` | `75%` | `0` |
| Ruby | Jekyll | site build | miss | `10.42s` | `100%` | `50%` | `0` |
| Kotlin | Okio | buffer flow | miss | `22.30s` | `100%` | `50%` | `8` |
| HTML | MDN forms | form validation | miss | `14.59s` | `100%` | `25%` | `0` |
| PHP | Monolog | log record flow | miss | `5.19s` | `100%` | `0%` | `0` |
| Python | requests | HTTP client flow | miss | `8.19s` | `100%` | `0%` | `0` |
| C# | AutoMapper | mapping flow | miss | `12.01s` | `100%` | `0%` | `3` |

What this says:

- File discovery is mostly working: only Redis missed expected file coverage.
- Explanation quality is the real gap: Python, Ruby, PHP, C#, Kotlin, and HTML
  found the right files but failed or partially failed the expected claims.
- Slow passing rows are Java and C at about `33s`.

## Performance By Framework Or Domain Kind

This groups the same 18 rows by the kind of code a user might recognize. It is
more useful than a repo-name-only table when deciding where CodeStory is ready.

Sorted best to worst: higher quality-pass rate first, then lower median elapsed
time.

| Framework / domain kind | Examples | Quality pass | Median elapsed | What it tells us |
| --- | --- | ---: | ---: | --- |
| Web routing and data fetching | Express, Gin, SWR | `3/3` | `7.71s` | Strong current lane: fast, complete file coverage, complete explanation coverage. |
| Systems and data engines | ripgrep, Redis, Chinook SQL | `3/3` | `13.07s` | Quality passed; Redis still missed one expected file and was slow. |
| Text and formatting utilities | Commons Lang, fmt | `2/2` | `22.45s` | Quality passed, but Java was one of the slow rows. |
| HTTP client libraries | requests, Alamofire, Dart http | `2/3` | `12.89s` | Swift and Dart passed; Python found files but failed explanation points. |
| Build, install, and site tooling | nvm, Jekyll | `1/2` | `7.73s` | Bash passed; Jekyll found files but only half the expected explanation points. |
| Web documents and styling | MDN forms, animate.css | `1/2` | `10.09s` | CSS passed; HTML found files but missed explanation points. |
| App support libraries | Monolog, AutoMapper, Okio | `0/3` | `12.01s` | Main weak lane: PHP, C#, and Kotlin need claim-quality work. |

## TypeScript React Library Example

Source: `target/agent-benchmark/segment9-current-ab-swr-generic-final/summary.json`

Generated: `2026-06-13T11:15:27.190Z`

SWR is Vercel's TypeScript/React data-fetching library. This row asks the same
repository question with and without CodeStory.

| Run | Quality | Tokens | Elapsed | Commands | Source files opened | Files found | Explanation points |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Without CodeStory | `0/1` | `535,632` | `201.38s` | `35` | `30` | `66.7%` | `25%` |
| With CodeStory | `1/1` | `32,168` | `42.67s` | `1` | `0` | `100%` | `100%` |

Why this matters:

- It is a real example where CodeStory changed the work from a source-file crawl
  into one cited answer bundle.
- It is not enough for a broad savings claim. The raw run reports
  `publishable: false`, `allow_failures: true`, and `reused_baseline_runs: 1`.

## What Is Solid

- The full 18-language with-CodeStory arm completed `54/54` attempted rows.
- The current fixed-baseline composite lowered all-in wall time, total tokens, commands, and
  direct source reads overall.
- The current with-CodeStory arm stayed packet-first: `54` commands and `0`
  direct source reads across the full suite.
- CodeStory quality-passed every measured language task in the current
  composite. Rows where baseline also passed are still operational wins because
  CodeStory used fewer commands, fewer tokens, lower wall time, and zero source
  reads.

## What Is Not Claimed

- No general public savings claim beyond the measured fixed-baseline artifacts.
- No universal language-support or answer-quality claim yet.
- No publishable public benchmark claim yet: the June 17 composite is an
  offline reused-baseline artifact, and the June 13 TypeScript row is a
  non-publishable single task.
- No claim that the holdout suite alone proves every supported framework or
  domain surface. Broader language/framework claims still need repeat,
  freshness, breadth, and promotion metadata.

## Next Runs Needed

Run these before promoting the current story beyond "promising current
evidence":

1. Keep the no-CodeStory arm as the fixed control artifact for this harness
   context. Do not rerun it for freshness or promotion loops; generate a new
   control artifact only if the task suite, pinned repo state, harness contract,
   or scorer boundary changes with explicit approval.
2. Add breadth evidence beyond the current holdout repos: at minimum, run
   CodeStory-only packet/runtime slices against additional supported
   language/framework surfaces and compare them to the fixed control where
   applicable.
3. Record promotion metadata for repeat/freshness/breadth before turning the
   development comparison into a public claim.

## How To Rerun

List available tasks:

```sh
node ./scripts/codestory-agent-ab-benchmark.mjs --list
node ./scripts/codestory-agent-ab-benchmark.mjs --task-suite language-expansion-holdout --list
```

Language answer-bundle run:

```sh
node ./scripts/codestory-agent-ab-benchmark.mjs --packet-runtime --task-suite language-expansion-holdout --repeats 1 --packet-runtime-mode cold-cli --codestory-cli ./target/release/codestory-cli --out-dir target/agent-benchmark/language-expansion-packet-runtime-current --timeout-ms 120000
```

Full language comparison with fixed baseline reuse:

```sh
node ./scripts/codestory-agent-ab-benchmark.mjs \
  --task-suite language-expansion-holdout \
  --arms without_codestory,with_codestory \
  --repeats 3 \
  --materialize-repos \
  --prepare-codestory-cache \
  --reuse-baseline-from target/agent-benchmark/language-expansion-holdout-20260617-baseline-j4 \
  --out-dir target/agent-benchmark/language-expansion-holdout \
  --timeout-ms 600000
```

Do not rerun the no-CodeStory arm for the current harness context. With
`--reuse-baseline-from`, matching without-CodeStory rows are reused from the
fixed artifact rather than executed. A new no-CodeStory control artifact is
only justified when the task suite, pinned repo state, harness contract, or
scorer boundary changes with explicit approval.

Publishable TypeScript React comparison target:

```sh
node ./scripts/codestory-agent-ab-benchmark.mjs --task-suite language-expansion-holdout --task-ids typescript-swr-hook-flow --repeats 3 --publishable --max-source-reads-after-packet 0 --codestory-cli ./target/release/codestory-cli --out-dir target/agent-benchmark/swr-publishable-r3
```

## Harness Contract

The agent benchmark harness runs the same repository prompt in two arms:

- `without_codestory`: avoid CodeStory and use normal repository exploration.
- `with_codestory`: use CodeStory grounding first, run an answer bundle for
  broad repository questions, then ordinary source reads only for named gaps.

The harness writes raw stdout/stderr per run, a JSONL run log, transcript
analysis, a machine summary, and a Markdown summary under
`target/agent-benchmark/<run-dir>`.

Use `--publishable` only when:

- both arms have token usage,
- every required run succeeds,
- repository provenance is pinned,
- CodeStory cache provenance is recorded,
- the with-CodeStory arm runs an answer bundle first, and
- post-bundle source reads stay inside the explicit budget.

Cold repo-scale timings are owned by
[codestory-e2e-stats-log.md](codestory-e2e-stats-log.md). Warm stdio loop
timings are owned by
[codestory-stdio-warm-loop-stats.md](codestory-stdio-warm-loop-stats.md).
