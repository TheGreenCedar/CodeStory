# Benchmark Task Manifests

Benchmark task manifests describe repeatable repository questions and the
anchors needed to score answer quality. Manifest files use the
`*.task.json` suffix and live in this folder so public benchmark suites can
load them without private workspace data.

Each manifest should include:

- `id`: stable lowercase identifier for reports.
- `suite`: suite name, such as `public-core`.
- `task_class`: one of the task classes listed in `manifest.schema.json`.
- `repo`: public repository metadata and optional setup notes.
- `prompt`: the task text to run.
- `expected_files`: repository-relative files that should be cited or used.
- `expected_verification_files`: optional test, benchmark, or verification
  files that are useful secondary evidence but should not count against primary
  source recall unless the prompt asks for verification.
- `expected_symbols`: stable symbols that should be found when applicable.
  Symbol entries may include an optional `query` when the canonical symbol name
  is intentionally ambiguous without task context; scoring still checks the
  `name` and `path`.
- `expected_claims`: claims a correct answer should make.
- `forbidden_claims`: claims that should fail quality scoring when present.
- `quality_thresholds`: pass/fail thresholds for files, symbols, claims,
  citations, and forbidden claims.

Keep manifests small and reviewable. Prefer stable files, exported/public
symbols, and claims that can be checked from repository source.

## Public Seed Corpus

The `public-core` suite is a source-quality gate for benchmark runs, not a
published benchmark result. A task passing means the answer covered the required
files, symbols, claims, citations, and forbidden-claim checks for that manifest.
It does not by itself establish speed, cost, or product headline claims.

Repository metadata in each manifest records the intended public clone target,
a full 40-character immutable Git commit SHA, optional workspace root,
languages, and lightweight setup notes. The benchmark harness must still know
how to map each `repo.name` to a local clone before it can execute the task.
Until that mapping exists, the manifest remains valid corpus data but is not
runnable through the harness.

Expected setup is intentionally simple:

- Clone the public repository URL at the manifest `repo.ref`. Branches, tags,
  and short SHAs are intentionally excluded so benchmark provenance is stable.
- Run the listed setup commands only when the benchmark runner needs local
  dependency metadata or tests.
- Treat `repo.workspace_root` as the benchmark working directory when it is
  present.
- Do not report public benchmark rows from a task unless the run is repeated,
  quality-gated, and clearly tied to the pinned repository state.

Public repos are not cloned automatically during ordinary `--list` or agent
A/B runs. Use `--materialize-repos` to clone or fetch manifest repos into
`target/agent-benchmark/repos`, or pass `--repo-cache-dir <path>` to use a
different local cache. Setup entries in manifests are documentation for humans;
the harness does not run arbitrary setup commands.

Direct packet runtime rows are available with `--packet-runtime`. They compare
cold CLI packet calls with warm `serve --stdio` packet calls while reusing the
same expected-anchor quality gates.

## Language Expansion Holdout

The `language-expansion-holdout` suite is the triggerable agent A/B suite for
public language-support profiles. It is separate from the OSS language corpus:

- The OSS corpus checks whether CodeStory can index pinned real projects.
- This suite runs paired `without_codestory` and `with_codestory` agent arms
  against those pinned projects and records elapsed time, token usage, estimated
  cost, observed tool calls, command counts, command categories, source reads,
  source reads after the first CodeStory packet, and manifest quality gates.
- The `without_codestory` arm mechanically runs a harness-owned local `rg` plus
  bounded source-read prelude. The `with_codestory` arm mechanically runs a
  harness-owned `codestory-cli packet` prelude. Both preludes count their wall
  time and command/tool accounting. The CodeStory arm is packet-first, not
  packet-only by default: if the packet and CodeStory follow-ups are partial,
  ordinary local source reads are allowed after CodeStory and counted as
  post-packet overhead. Pass `--max-source-reads-after-packet 0` only when you
  want stricter packet-only promotion evidence. The `without_codestory` arm is
  invalid for publishable evidence if it calls CodeStory or never inspects the
  local repository.

The suite currently has one medium-sized open source project per public
language-support profile: parser-backed graph languages (Python, Java, Rust,
JavaScript, TypeScript, C++, C, Go, Ruby, PHP, C#, Kotlin, Swift, Dart, Bash)
plus structural collectors (HTML, CSS, SQL).

Materialize the pinned repos:

```powershell
node scripts/codestory-agent-ab-benchmark.mjs `
  --list --task-suite language-expansion-holdout --materialize-repos
```

Run the promotion-eligible packet-runtime shape:

```powershell
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
  --publishable
```

Use `--task-ids <id>` for a cheaper targeted run. The Markdown summary table
includes the human-readable A/B columns; `runs.jsonl` remains the source of
truth for per-run metrics.

Promotion requires full `language-expansion-holdout` packet-runtime coverage,
cold and warm modes, `--repeats 3`, `--jobs 4`, prepared sidecars,
`--publishable`, no `--allow-failures`, full sidecar provenance, no quality
misses, no sufficiency gaps, and no SLA misses. `--jobs 4` is valid row
concurrency for this eval lane. Keep `--prepare-codestory-jobs` lower or capped;
use `2` for examples unless intentionally running serial prep.

Fixed no-CodeStory controls and `--reuse-baseline-from` are development
diagnostics unless the benchmark contract accepts matching fingerprints. They
are never enough for packet-runtime promotion by themselves. Generate a new
control artifact only when the task suite, pinned repo state, harness contract,
or scorer boundary changes with explicit approval.

Run a diagnostic comparison with a compatible fixed no-CodeStory control:

```powershell
node scripts/codestory-agent-ab-benchmark.mjs `
  --task-suite language-expansion-holdout `
  --arms without_codestory,with_codestory `
  --repeats 3 `
  --materialize-repos `
  --prepare-codestory-cache `
  --reuse-baseline-from target/agent-benchmark/language-expansion-holdout-20260617-baseline-j4 `
  --out-dir target/agent-benchmark/language-expansion-holdout `
  --timeout-ms 600000
```

For runtime packet fixes, prefer a packet-first gated loop before launching
nested agents:

```powershell
node scripts/codestory-agent-ab-score.mjs `
  --packet-gate --packet-probe-jobs 4 `
  --packet-gate-improved-from target/agent-benchmark/<previous-run> `
  --reuse-baseline-from target/agent-benchmark/<previous-run> `
  --prepare-codestory-jobs 2 `
  --task-ids <comma-separated-task-ids> `
  --out-dir target/agent-benchmark/<run-name>
```

`--packet-probe-jobs` controls cheap packet probes, `--jobs` controls
independent nested A/B repo groups, and `--prepare-codestory-jobs` caps cache
prep across repos. If a packet probe fails from transient sidecar
unavailability, the score wrapper reruns just those task ids serially in a
`packet-probes-retry` artifact before deciding which rows enter the A/B phase.
Baseline reuse is valid only when the task manifest, scorer, harness, model,
CLI identity, retrieval contract, and packet threshold fingerprints match.

For anti-overfit language checks, run promotion-oriented packet gates with
production defaults. Exact benchmark probes belong in benchmark manifests,
explicit `--extra-probe` inputs, or eval-only diagnostics; they are benchmark
fixture behavior, not production steering. Framework/domain semantics belong in
product code when they generalize to real projects.

Write fresh outputs under `target/agent-benchmark/<run-name>` and summarize the
durable result in [language-expansion-ab-report.md](../../docs/testing/language-expansion-ab-report.md)
instead of preserving local run directory catalogs here. The current June 18
diagnostic full form+command packet-runtime artifact at
`target/agent-benchmark/language-expansion-proof-full-form-command-shapes`
passes `108/108` success, quality, and sufficiency gates, but has `9` cold SLA
misses and is non-publishable development proof only. The current publishable
artifact at
`target/agent-benchmark/language-expansion-publishable-full-form-command-shapes`
passes `108/108` success, `106/108` quality, and `107/108` sufficiency, with
`1` partial row and `8` cold SLA misses. Promotion is blocked by
apache-commons-lang cold SLA `3/3`, redis cold SLA `3/3`, AutoMapper cold SLA
`1/3`, dart-http cold SLA `1/3`, square-okio cold quality `2/3`, and Alamofire
cold quality `2/3` plus `1` partial sufficiency.

## Local Real-Repo Corpus

The `local-real` suite targets sibling checkouts under the parent directory of
this CodeStory repo. It is meant for exploratory A/B runs against the user's
real local workspaces before promotion-grade public rows exist.

Current slots:

- `codestory`: this repo.
- `sourcetrail`: `../Sourcetrail`.
- `codex`: `../codex`.
- `vscode`: `../vscode`; currently verified against pinned commit
  `20ed2bc21d4d73a029b52d3ee6db382ee85c3cca` when that checkout is present.

Local-real rows are not public savings claims by themselves. They must be
repeated, quality-gated, tied to clean pinned checkouts, and compared against
the no-CodeStory arm before they support promotion language.

For CodeStory-assisted rows, use `--prepare-codestory-cache` unless the intent is
explicitly to measure degraded cache behavior. This refreshes stale or
semantic-empty local caches before the timed agent run and records the index
cost as setup evidence, so cache-reuse savings are not confused with indexing
cost.

## Holdout retrieval

The `holdout-retrieval` suite measures **generalization** on pinned public OSS
libraries. It is required for sidecar retrieval promotion (pass at least 2/3 repos)
and must **not** be used to tune planner or ranker heuristics.

| Repo key | Upstream | Pin |
|----------|----------|-----|
| `ripgrep` | [BurntSushi/ripgrep](https://github.com/BurntSushi/ripgrep) | `14.1.0` |
| `axios` | [axios/axios](https://github.com/axios/axios) | `v1.6.8` |
| `redis` | [redis/redis](https://github.com/redis/redis) | `7.2.4` |

Manifests live under `benchmarks/tasks/holdout-retrieval/`. Each task uses
`task_class: architecture_explanation` and records immutable `repo.ref` values.

### Materializing repos

Holdout repos are cloned only under `target/agent-benchmark/repos/` (under
`target/`, so they stay out of version control). No sibling checkout is required.

Prefetch all three holdout checkouts:

```powershell
node scripts/fetch-holdout-repos.mjs
```

Or use the benchmark harness directly:

```powershell
node scripts/codestory-agent-ab-benchmark.mjs `
  --list --task-suite holdout-retrieval --materialize-repos
```

Optional `--repo-cache-dir <path>` overrides the default cache directory on both
commands.

### Smoke run (packet runtime)

```powershell
node scripts/codestory-agent-ab-benchmark.mjs `
  --packet-runtime --packet-runtime-mode cold-cli `
  --task-suite holdout-retrieval --materialize-repos `
  --repeats 1 --out-dir target/agent-benchmark/holdout-retrieval-smoke `
  --codestory-cli target/release/codestory-cli.exe --timeout-ms 180000
```

Build `codestory-cli` in release mode before the smoke run. `redis` is L-tier and
may exceed the default timeout on cold index; increase `--timeout-ms` when needed.

### Forbidden tuning policy

- Do **not** add repo-name, path, or display-name literals for `ripgrep`, `axios`,
  or `redis` in v2 planner or ranker code.
- Keep holdout-specific probes and claim templates in manifests, benchmark
  harnesses, tests, or `crates/codestory-runtime/src/agent/eval_probes.rs`
  behind `CODESTORY_EVAL_PROBES`; do not put them in product packet/search
  planning or ranking paths.
- Do **not** iterate KPI fixes against holdout manifests; use `local-real` for
  in-scope tuning and treat holdout rows as promotion-only evidence.
- Legacy sibling apps (`freelancer`, `traderotate`) are removed from default
  benchmark repos; use this suite instead for generalization gates.
