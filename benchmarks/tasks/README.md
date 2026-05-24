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
- `expected_symbols`: stable symbols that should be found when applicable.
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
pin, optional workspace root, languages, and lightweight setup notes. The
benchmark harness must still know how to map each `repo.name` to a local clone
before it can execute the task. Until that mapping exists, the manifest remains
valid corpus data but is not runnable through the harness.

Expected setup is intentionally simple:

- Clone the public repository URL at the manifest `repo.ref`.
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
