# What to expect in your repo

CodeStory builds a **local map** of your checkout — files, symbols, imports,
call paths, and bounded answer packets when broad search is ready. Quality depends
on your **language mix**, **repo size**, and **readiness lane**, not on a single
global score.

The map and retrieval generations are stored per repository. One host process
can serve several repositories and reuse one warm embedding engine, but it does
not mix their indexes, publications, or readiness state.

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

- **First index** on a large checkout can take minutes; the agent retries the
  intended CodeStory call while preparation runs.
- **First broad search in a process** also initializes the embedded model. Later
  repositories in that process reuse the warm engine.
- **Incremental refresh** after small edits is usually fast; stale local
  navigation is a freshness problem, not a prompt problem.
- **Very large or unusual layouts** (generated trees, vendored giants) may index
  partially; the agent should cite gaps instead of guessing.

The released executable contains its model and embedding backend. There is no
separate model download or service startup. A verified content-addressed model
copy may be materialized in the CodeStory cache for memory mapping and reused
after restart.

## When output looks weak

Blocked or degraded broad-search infrastructure returns no packet evidence. A
returned packet with partial sufficiency is a **lead to inspect**, not proof
that the answer is complete. Local graph tools may still be reliable meanwhile.

Plain-language trust boundaries and good vs blocked sessions:
[Trust and readiness](trust-and-readiness.md).

Recovery steps: [Troubleshooting](troubleshooting.md).

## Benchmarks are not your repo

Public holdout suites measure token and time reduction on **pinned OSS packages**
under controlled tasks. They do not guarantee the same numbers in your checkout.

What the holdout proves and does not prove:
[Evaluation scope in the README](../../README.md#evaluation).

Better prompts help every session: [Prompt patterns](prompt-patterns.md).
