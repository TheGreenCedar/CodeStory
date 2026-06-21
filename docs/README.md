# CodeStory docs

Start from the job you need to do. CodeStory docs are split by trust boundary:
operator path, repair path, architecture, verification, and research evidence.

## First stop

| Reader job | Canonical doc | Use it to decide | Trust boundary |
| --- | --- | --- | --- |
| Operator using an agent | [Usage](usage.md) | How to install, ground a repo, and keep the plugin-first path straight. | The plugin is the normal path; the CLI is for setup, repair, debugging, and transcripts. |
| Maintainer debugging readiness | [Retrieval sidecars operations](ops/retrieval-sidecars.md) | How to repair local navigation versus packet/search readiness. | Packet/search is proof-bearing only when `retrieval_mode` is `full`. |
| Contributor changing code | [Contributor setup](contributors/getting-started.md) | Which crate owns the change and which verification lane is enough. | Pick the smallest lane that covers the behavior; run Cargo checks serially. |
| Reviewer verifying claims | [Testing matrix](contributors/testing-matrix.md) | Which command or evidence tier supports a PR claim. | Logs and benchmark pages are evidence records or playbooks, not product promises by themselves. |
| Researcher comparing retrieval choices | [Research handbook](research.md) | Which retrieval, embedding, or sidecar decision is current enough to build on. | Research rows must stay tied to the comparison matrix and proof tier that produced them. |

## Common paths

| Question | Start here | Then read |
| --- | --- | --- |
| Where do I start as a first-time user? | [README - Use It With An Agent](../README.md#use-it-with-an-agent) | [Usage - Operator Journey](usage.md#operator-journey) |
| How do I repair readiness? | [Usage - Stale Local Cache](usage.md#stale-local-cache) | [Retrieval sidecars operations - Operator repair path](ops/retrieval-sidecars.md#operator-repair-path) |
| How does CodeStory work internally? | [How CodeStory Works](concepts/how-codestory-works.md) | [Architecture overview](architecture/overview.md) and [runtime execution path](architecture/runtime-execution-path.md) |
| Which docs define sidecar architecture? | [Retrieval design](architecture/retrieval-design.md) | [Retrieval architecture and promotion guide](testing/retrieval-architecture.md) |
| Which language support claims are safe? | [Language support](architecture/language-support.md) | [Testing matrix - Indexer And Graph Fidelity](contributors/testing-matrix.md#indexer-and-graph-fidelity) |
| Which test proves my docs-only change? | [Testing matrix - Docs-Only Fast Path](contributors/testing-matrix.md#docs-only-fast-path) | [Contributor setup - Choose The Verification Lane First](contributors/getting-started.md#choose-the-verification-lane-first) |
| Where are timing and benchmark records? | [E2E stats log](testing/codestory-e2e-stats-log.md) | [Performance review playbook](testing/performance-review-playbook.md) and [embedding backend benchmarks](testing/embedding-backend-benchmarks.md) |

## Evidence surfaces

| Surface | Treat as | Do not treat as |
| --- | --- | --- |
| [codestory-e2e-stats-log.md](testing/codestory-e2e-stats-log.md) | Rolling release-style timing and readiness record. | A promise that current packet/search is ready on your machine. |
| [retrieval-architecture.md](testing/retrieval-architecture.md) | Promotion gates and proof tiers for sidecar packet/search work. | A substitute for fresh benchmark or drill evidence. |
| [embedding-backend-benchmarks.md](testing/embedding-backend-benchmarks.md) | Comparison matrix for embedding and retrieval experiments. | A shortcut around rerunning the relevant quality gates. |
| [performance-review-playbook.md](testing/performance-review-playbook.md) | Review playbook for performance claims. | A generated ledger or release artifact. |

## Maintenance rule

When a doc repeats an entry path, keep the durable version here or in the owning
task doc, not both. Use this page for routing. Use the target page for commands,
contracts, and recovery details.
