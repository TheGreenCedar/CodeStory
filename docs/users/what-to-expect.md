# What to expect in your repo

CodeStory builds a **local map** of your checkout — files, symbols, imports,
call paths, and bounded answer packets when broad search is ready. Quality depends
on your **language mix**, **repo size**, and **readiness lane**, not on a single
global score.

The map and retrieval generations are stored per repository. Compatible host
processes for one OS user can reuse one warm embedding server, but it does not
receive repository paths or mix indexes, publications, or readiness state.

## Language coverage (plain language)

**Strong day-to-day navigation** — typical Python, Java, Rust, JavaScript,
TypeScript, C/C++, Go, Ruby, PHP, C#, Kotlin, Swift, Dart, and Bash in normal
project layouts. The indexer extracts a source graph; fidelity suites gate the
core symbol, import, and call shapes.

**Structural anchors, not full code graphs** — HTML, CSS, SQL, GitHub Actions
workflows (under `.github/workflows/`), Docker Compose manifests, `Cargo.toml`
(basename-scoped), and OpenAPI endpoint schema anchors. You get exact-source
pointers for those files; they are not the same as parser-backed navigation
through application code.

Docker Compose here is a file format CodeStory can index. CodeStory itself does
not run Docker or a Compose-managed retrieval service.

**Mixed repos** — a monorepo with Rust services and YAML configs gets graph
navigation in Rust and structural anchors in config files. Ask concrete questions
per area rather than expecting one uniform depth everywhere.

**Depth and claim tiers** — contributor-grade definitions, evidence floors, and
what each claim does *not* mean:
[Language support contract](../architecture/language-support.md).

## Repo size and freshness

- **Finding an implementation.** Try `cs find --question "Where are retry delays calculated?"`. The reply shows likely functions and the source that connects them. Put an exact location in the question, for example ``cs find --question 'Explain `src/retry.go:42` and its callers'``. The reply leads with source it can verify at the supplied location, including the matching old side of a proposed diff. These excerpts follow the normal ignore rules. If a relevant function appears as a short excerpt or a text mention, copy its `path:start-end` into `cs read`. An unindexed checkout can return live text matches. Run `codestory-cli index --core-only` to prepare the index for verified relationships.
- **Reviewing edits.** Run `cs refs --changed` to see which functions a tracked edit touches, where the index found callers, and which tests mention a changed name. Run those tests to check behavior. `--file-glob` narrows both the changed files and the search for tests. Use `--no-bodies` for a compact list.
- **Broad investigation**. Run `cs packet --question "How do retries work across this service?"` for the canonical bounded packet. This route requires full retrieval readiness and uses the repository you run it from.
- **Shell lookups** read the current files even on an unindexed checkout. An
  index can add symbol names and source ranges when it matches the file.
  To check several known symbols together, pass them to one command, such as
  `cs refs api.go::Start worker.go::Run`. Call links come from the pinned index.
  CodeStory checks the source at both ends. A factory or import in a third file
  may have changed since indexing. For a rename, use `cs find NAME` to inspect
  other mentions and run the project’s normal checks.
  When declaration bodies would push later matches out of the byte budget,
  `find` and single-name `refs` switch to a compact location list. Pass
  `--no-bodies` to request that list directly, then `read` the relevant sites.
- **First index** on a large checkout can take minutes; the agent retries the
  intended CodeStory call while preparation runs.
- **First broad search** can load the bundled model. Projects using the same
  per-user embedding server can reuse it until the server goes idle.
- **Incremental refresh** after small edits is usually fast; stale local
  navigation is a freshness problem, not a prompt problem.
- **Very large or unusual layouts** (generated trees, vendored giants) may index
  partially; the agent should cite gaps instead of guessing.
- **Supported scale envelope** — the freshness scan that keeps incremental
  refresh honest is bounded at 25,000 indexed files. Beyond that bound the
  index itself stays complete and usable, but `index` reports a warning and
  `doctor` reports the inventory check at `warn` because drift past the bound
  is not observed. Narrow the indexed scope (exclude generated or vendored
  trees) to get full freshness checking back.

The released executable contains its model and embedding backend. CodeStory
starts the per-user embedding server automatically and can reuse a verified
copy of the bundled model from its cache after restart.

For example, in the codex-autoresearch dashboard, asking how a long chart is
sampled located `chartPointBudget` and the call from `sampleTrendChartData` to
`sampleTimeline`. After editing the sampler, `cs refs --changed` identified
that same caller. A regression test then exposed what a source lookup alone
could miss. The old sampler met the point limit while dropping early and
middle runs. [Upstream change and regression test](https://github.com/TheGreenCedar/codex-autoresearch/commit/218b294678d24435be6ba4f20b306d73b70566cf)

## When output looks weak

Blocked or degraded broad-search infrastructure returns no packet evidence. A
packet with `continuation_available` or `no_useful_evidence` is a **lead to
inspect**, not authority for a complete answer. Local graph tools may still be
reliable meanwhile.

Plain-language trust boundaries and good vs blocked sessions:
[Trust and readiness](trust-and-readiness.md).

Recovery steps: [Troubleshooting](troubleshooting.md).

## Benchmarks

In the `codestory-cli setup codex` trial, Codex used shell lookups on its own.
On large repositories, raw token use was nearly flat and the setup missed the
correctness target. It met 72 of 75 requirements, compared with 74 of 75 for
plain Codex. On small repositories, both groups met all 45 requirements, and the
setup group used fewer raw tokens in that cohort. General token savings and
equal correctness across repository sizes remain unestablished.

See the setup results, earlier comparisons, and command timings in
[Evaluation](../../README.md#evaluation).

Better prompts help every session: [Prompt patterns](prompt-patterns.md).
