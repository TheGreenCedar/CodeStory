# What to expect in your repo

CodeStory builds a **local map** of your checkout — files, symbols, imports,
call paths, and bounded answer packets when sidecars are ready. Quality depends
on your **language mix**, **repo size**, and **readiness lane**, not on a single
global score.

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

**Mixed repos** — a monorepo with Rust services and YAML configs gets graph
navigation in Rust and structural anchors in config files. Ask concrete questions
per area rather than expecting one uniform depth everywhere.

**Depth and claim tiers** — contributor-grade definitions, evidence floors, and
what each claim does *not* mean:
[Language support contract](../architecture/language-support.md).

## Repo size and freshness

- **First index** on a large checkout can take minutes; hooks and status report
  progress and blocked surfaces.
- **Incremental refresh** after small edits is usually fast; stale local
  navigation is a repair problem, not a prompt problem.
- **Very large or unusual layouts** (generated trees, vendored giants) may index
  partially; the agent should cite gaps instead of guessing.

## When output looks weak

Degraded or partial packet/search output is a **lead to inspect**, not proof
that the answer is complete. Local graph tools may still be reliable while broad
search is blocked.

Plain-language trust boundaries and good vs blocked sessions:
[Trust and readiness](trust-and-readiness.md).

Repair steps: [Troubleshooting](troubleshooting.md).

## Benchmarks are not your repo

Public holdout suites measure token and time reduction on **pinned OSS packages**
under controlled tasks. They do not guarantee the same numbers in your checkout.

What the holdout proves and does not prove:
[Evaluation scope in the README](../../README.md#evaluation).

Better prompts help every session: [Prompt patterns](prompt-patterns.md).
