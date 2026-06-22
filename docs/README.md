# CodeStory docs

Coding agents rediscover a repository on every question: file hunts, snippet
reads, import chasing, and context spent rebuilding the same map. CodeStory
indexes once and serves read-only evidence from that map.

Start from the job you need to do. Docs are split by trust boundary: operator
path, repair path, architecture, verification, and research evidence.

## First stop

| Reader job | Canonical doc | Use it to decide | Trust boundary |
| --- | --- | --- | --- |
| Operator using an agent | [Usage](usage.md) | How to install, ground a repo, and keep the plugin-first path straight. | The agent plugin is the normal path; the CLI is for setup, repair, debugging, and transcripts. |
| Maintainer debugging readiness | [Retrieval sidecars operations](ops/retrieval-sidecars.md) | How to repair local navigation versus packet/search readiness. | Packet/search is proof-bearing only when `retrieval_mode` is `full`. |
| Contributor changing code | [Contributor setup](contributors/getting-started.md) | Which crate owns the change and which verification lane is enough. | Pick the smallest lane that covers the behavior; run Cargo checks serially. |
| Reviewer verifying claims | [Testing matrix](contributors/testing-matrix.md) | Which command or evidence tier supports a PR claim. | Logs and benchmark pages are evidence records or playbooks, not product promises by themselves. |
| Researcher comparing retrieval choices | [Research handbook](research.md) | Which retrieval, embedding, or sidecar decision is current enough to build on. | Research rows must stay tied to the comparison matrix and proof tier that produced them. |

## Common paths

| Question | Start here | Then read |
| --- | --- | --- |
| Where do I start as a first-time user? | [README - Quick start](../README.md#quick-start) | [Usage - Operator Journey](usage.md#operator-journey) |
| How do I repair readiness? | [Usage - Stale Local Cache](usage.md#stale-local-cache) | [Retrieval sidecars operations - Operator repair path](ops/retrieval-sidecars.md#operator-repair-path) |
| How does CodeStory work internally? | [Architecture overview](architecture/overview.md) | [Runtime execution path](architecture/runtime-execution-path.md) and subsystem pages |
| Which docs define sidecar architecture? | [Retrieval design](architecture/retrieval-design.md) | [Retrieval architecture and promotion guide](testing/retrieval-architecture.md) |
| Which language support claims are safe? | [Language support](architecture/language-support.md) | [Testing matrix - Indexer And Graph Fidelity](contributors/testing-matrix.md#indexer-and-graph-fidelity) |
| Which test proves my docs-only change? | [Testing matrix - Docs-Only Fast Path](contributors/testing-matrix.md#docs-only-fast-path) | [Contributor setup - Choose The Verification Lane First](contributors/getting-started.md#choose-the-verification-lane-first) |
| Where are timing and benchmark records? | [E2E stats log](testing/codestory-e2e-stats-log.md) | [Performance review playbook](testing/performance-review-playbook.md), [embedding backend benchmarks](testing/embedding-backend-benchmarks.md), and [language-expansion holdout stats](testing/language-expansion-holdout-stats.md) |
| What does a term mean? | [Glossary](glossary.md) | [Usage - Readiness Lanes](usage.md#readiness-lanes) |
| Where is the with/without comparison? | [README - With vs without CodeStory](../README.md#with-vs-without-codestory) | [Agent benchmark harness verification](testing/agent-benchmark-harness-verification.md) |

Use the [verification lane picker](contributors/getting-started.md#choose-the-verification-lane-first) when a change needs a proof path beyond reading docs back.

## Evidence surfaces

| Surface | Treat as | Do not treat as |
| --- | --- | --- |
| [codestory-e2e-stats-log.md](testing/codestory-e2e-stats-log.md) | Rolling release-style timing and readiness record. | A promise that current packet/search is ready on your machine. |
| [retrieval-architecture.md](testing/retrieval-architecture.md) | Promotion gates and proof tiers for sidecar packet/search work. | A substitute for fresh benchmark or drill evidence. |
| [embedding-backend-benchmarks.md](testing/embedding-backend-benchmarks.md) | Comparison matrix for embedding and retrieval experiments. | A shortcut around rerunning the relevant quality gates. |
| [performance-review-playbook.md](testing/performance-review-playbook.md) | Review playbook for performance claims. | A generated ledger or release artifact. |
| [language-expansion-holdout-stats.md](testing/language-expansion-holdout-stats.md) | Scoped 18-repo with/without benchmark record. | A generalization promise for every repo question. |

## Canonical owners

Use this page for routing. Use the target page for commands, contracts, and recovery details.

| Topic | Canonical doc |
| --- | --- |
| Operator install, prompts, commands | [usage.md](usage.md) |
| Terminology | [glossary.md](glossary.md) |
| Verification lanes and proof tiers | [contributors/testing-matrix.md](contributors/testing-matrix.md) |
| With/without benchmark summary | [README - With vs without CodeStory](../README.md#with-vs-without-codestory) |

## Documentation maintenance

| Need | Start here |
| --- | --- |
| Search this doc set by topic | [search-index.md](search-index.md) |
| Checklist before committing docs | [documentation-maintenance-checklist.md](contributors/documentation-maintenance-checklist.md) |
| Templates for new docs | [templates/](templates/) |
