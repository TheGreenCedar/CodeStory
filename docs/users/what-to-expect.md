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

- **Shell lookups** read the current files even on an unindexed checkout. An
  index can add symbol names and source ranges when it matches the file.
- **First index** on a large checkout can take minutes; the agent retries the
  intended CodeStory call while preparation runs.
- **First broad search in a process** also initializes the embedded model. Later
  repositories in that process reuse the warm engine.
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

The released executable contains its model and embedding backend. There is no
separate model download or service startup. A verified content-addressed model
copy may be materialized in the CodeStory cache for memory mapping and reused
after restart.

## When output looks weak

Blocked or degraded broad-search infrastructure returns no packet evidence. A
packet with `continuation_available` or `no_useful_evidence` is a **lead to
inspect**, not authority for a complete answer. Local graph tools may still be
reliable meanwhile.

Plain-language trust boundaries and good vs blocked sessions:
[Trust and readiness](trust-and-readiness.md).

Recovery steps: [Troubleshooting](troubleshooting.md).

## Benchmarks

In a controlled Codex study, agents used shell lookups on their own when the
commands appeared in standing instructions. Graded correctness and token use
were similar to plain Codex. The study did not establish token savings.

See the sample sizes, results, and probe timings in
[Evaluation](../../README.md#evaluation).

Better prompts help every session: [Prompt patterns](prompt-patterns.md).
