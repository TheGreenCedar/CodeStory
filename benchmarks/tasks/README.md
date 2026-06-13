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
immutable commit or tag ref, optional workspace root, languages, and lightweight setup notes. The
benchmark harness must still know how to map each `repo.name` to a local clone
before it can execute the task. Until that mapping exists, the manifest remains
valid corpus data but is not runnable through the harness.

Expected setup is intentionally simple:

- Clone the public repository URL at the manifest `repo.ref`; branch-like refs
  such as `main` are allowed for local diagnostics only and fail publishable
  provenance gates.
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
runtime-supported languages. It is separate from the OSS language corpus:

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

The suite currently has one medium-sized open source project per supported
language: Python, Java, Rust, JavaScript, TypeScript, C++, C, Go, Ruby, PHP,
C#, Kotlin, Swift, Dart, Bash, HTML, CSS, and SQL.

Materialize the pinned repos:

```powershell
node scripts/codestory-agent-ab-benchmark.mjs `
  --list --task-suite language-expansion-holdout --materialize-repos
```

Run a strict paired comparison:

```powershell
node scripts/codestory-agent-ab-benchmark.mjs `
  --task-suite language-expansion-holdout `
  --arms without_codestory,with_codestory `
  --repeats 3 --materialize-repos --prepare-codestory-cache `
  --out-dir target/agent-benchmark/language-expansion-holdout `
  --timeout-ms 600000
```

Use `--task-ids <id>` for a cheaper targeted run. The Markdown summary table
includes the human-readable A/B columns; `runs.jsonl` remains the source of
truth for per-run metrics.

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
Baseline reuse is valid only when the task manifest and scorer boundary are
unchanged.

For anti-overfit language checks, set
`CODESTORY_PACKET_EXACT_FAMILY_STEERING=0` before running the packet gate. The
current clean serial full gate is:

```text
target/agent-benchmark/segment8-no-family-steering-full-packets-java-css-generic-shapes-serial
```

It quality-passes `9/18` rows. The corresponding current packet-gated A/B slice
is:

```text
target/agent-benchmark/segment8-no-family-steering-current9-ab-java-css-generic-shapes
```

That slice compares `9/9` CodeStory quality against `6/9` baseline quality and
records time, tokens, commands, tool calls, post-packet source reads, and web
leakage. Treat it as packet-eligible-slice evidence, not broad promotion proof
for all supported languages.

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
