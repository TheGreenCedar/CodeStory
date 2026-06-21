# Changelog

## 0.11.2

CodeStory 0.11.2 carries the post-0.11.1 documentation and MCP stdio work from
`dev/codestory-next` into a synchronized patch release. The release version is
now aligned across all `codestory-*` workspace crates and `Cargo.lock`.

The user-facing docs were tightened around the way people actually install,
operate, and review CodeStory. The README and usage docs now separate source
state from runtime proof, keep readiness checks visible, and avoid implying that
docs alone prove packet/search health. Plugin install guidance now points at the
latest-release flow where this repository owns the plugin package, while the
external marketplace catalog remains owned by
`TheGreenCedar/AgentPluginMarketplace`.

The plugin MCP path is intentionally direct: `.mcp.json` runs
`codestory-cli serve --stdio --refresh none` instead of carrying a duplicate
adapter runtime. The stdio catalog also exposes a read-only `ground` tool for
grounding snapshots, alongside the existing resource and packet/search safety
boundaries.

This release does not promote packet/search readiness, sidecar readiness,
benchmark results, or query quality. It also does not claim live installed
plugin runtime proof unless that surface is dogfooded separately from this
source release lane.

## 0.11.1

CodeStory 0.11.1 was published from `main` at
`9dc3a20e7de84b7955579e6ad8dd44945a47d47a`. It ships the Codex plugin
packaging work that landed after `v0.11.0`: install/readiness stays in the CLI
wrapper, the plugin package owns only Codex metadata and skill text, and
`.mcp.json` launches `codestory-cli serve --stdio` directly instead of carrying
a Node adapter.

Release evidence:

- GitHub release: https://github.com/TheGreenCedar/CodeStory/releases/tag/v0.11.1
- Full comparison: https://github.com/TheGreenCedar/CodeStory/compare/v0.11.0...v0.11.1
- Version and packaging lane: #267

The marketplace catalog is still outside this repository. Issue #264 closed the
separate `TheGreenCedar/AgentPluginMarketplace` catalog lane, while PR #262 left
CodeStory owning only the plugin package source under `plugins/codestory`. This
release does not claim packet/search readiness, sidecar promotion, or benchmark
improvement.

### Shipped Since 0.11.0

| Area | Delivered in 0.11.1 | Evidence |
| --- | --- | --- |
| Release version | All `codestory-*` workspace crates and `Cargo.lock` are synchronized at `0.11.1`. | Issue #267; tag `v0.11.1` |
| Plugin packaging | `plugins/codestory` now contains the Codex plugin manifest, MCP metadata, package README, grounding skill, and static package tests. | PR #262 |
| Direct CLI MCP launch | The plugin `.mcp.json` launches `codestory-cli serve --stdio --refresh none` directly, with no in-package Node adapter or duplicated retrieval/runtime logic. | PR #262 |
| Install and readiness wrapper | `scripts/install-codestory.ps1` added the Windows x64 happy path for finding or installing `codestory-cli`, then reporting binary, local-navigation, and packet/search readiness from `doctor`. | PR #261 |
| Cross-platform plugin readiness | Plugin README, skill guidance, and static tests now cover Windows, macOS, and Linux install/readiness paths without adding an adapter runtime or changing Rust product behavior. | PR #269 |
| Release-note hygiene | Stale generated 0.11 pre-release docs and ledger-style artifacts were removed from committed docs before this release. | PR #260 |

Binary release assets are packaging evidence only. In this release, the plugin
docs and installer defaults kept archive names release-bound to `v0.11.1`; the
marketplace catalog remains outside this repository.

## 0.11.0

CodeStory 0.11.0 was published from `main` at
`d793965b11e526449f66b1eb1166b137a0d3839f`. It carries the post-0.10.1
development branch into a synchronized release without changing the rule that
packet/search readiness needs fresh sidecar evidence.

Release evidence:

- GitHub release: https://github.com/TheGreenCedar/CodeStory/releases/tag/v0.11.0
- Full comparison: https://github.com/TheGreenCedar/CodeStory/compare/v0.10.1...v0.11.0
- Version bump PR: #256

### Shipped Since 0.10.1

| Area | Delivered in 0.11.0 | Evidence |
| --- | --- | --- |
| Release version | All `codestory-*` workspace crates and `Cargo.lock` are synchronized at `0.11.0`. | PR #256; tag `v0.11.0` |
| Rustdoc and API docs | Rustdoc baseline guidance, public API documentation passes across contracts, workspace/store/indexer, retrieval/runtime, and CLI integration surfaces, plus a rustdoc warning gate. | PR #221, #225, #230, #234, #237, #239 |
| Sidecar and packet diagnostics | Sidecar status repair hints, vector timing diagnostics, Turbovec diagnostic gates, lexical/rerank probes, and embedding identity probes. | PR #224, #227, #236, #241, #242, #247, #251, #252 |
| Workflow and reliability | Dev PR flow documentation, worktree setup bootstrap, stale rehydrate-env source hardening, manifest schema repair, workspace dependency cleanup, compact proof anchors, and dependency audit repair. | PR #195, #201, #204, #206, #207, #217, #219 |

Binary release assets are packaging evidence only. Use the sidecar and
promotion tiers in `docs/contributors/testing-matrix.md` before claiming
packet/search readiness, answer quality, or performance promotion.

## 0.10.1

CodeStory 0.10.1 was published from `main` at
`02ae23d23519e6ee63a0824ecc96fcfc0a3bb45a`.

Release evidence:

- GitHub release: https://github.com/TheGreenCedar/CodeStory/releases/tag/v0.10.1
- Full comparison: https://github.com/TheGreenCedar/CodeStory/compare/v0.10.0...v0.10.1
- Version bump PR: #192

### Shipped Since 0.10.0

| Area | Delivered in 0.10.1 | Evidence |
| --- | --- | --- |
| Release version | All `codestory-*` workspace crates and `Cargo.lock` are synchronized at `0.10.1`. | PR #192; tag `v0.10.1` |
| Structural source proof | GitHub Actions workflow routing, Docker Compose structural collectors, Cargo manifest structural anchors, and OpenAPI endpoint evidence demotion. | PR #162, #177, #180, #182 |
| Retrieval and packet correctness | Cache rehydrate freshness guard, stdio packet budget timing, retrieval shadow fixture repairs, release evidence docs repair, retrieval mode override removal, and precise semantic SCIP diagnostics. | PR #163, #164, #165, #166, #167, #169, #183 |
| Durable docs hygiene | Stale generated pre-release review docs were removed from committed documentation. | PR #185 |

## 0.10.0

CodeStory 0.10.0 turns the post-0.9.0 research wave into releaseable
contracts, proof/provenance plumbing, cache-reuse primitives, release evidence,
and smaller maintenance surfaces. It is not a packet-runtime SLA clearance
release: #78 was carried as accepted/deferred release risk and later closed as
stale before `v0.11.0`.

Reviewer comparison branch:
`https://github.com/TheGreenCedar/CodeStory/compare/v0.9.0...review/codestory-saga-from-v0.9.0-f4f6d3d6`

### Shipped Since 0.9.0

| Area | Delivered in 0.10.0 | Evidence |
| --- | --- | --- |
| Language/support claims | Tiered language claim definitions, sidecar manifest contract, anti-overfit claim profile gates, product agent workflow contract, and explicit performance/ops gates. | PR #43, #44, #45, #46, #56, #57, #58 |
| Retrieval proof/provenance | Compact packet provenance counts, SCIP proof adapter contract slice, structural workflow source-proof pilot, unresolved-candidate diagnostics, publishable blocker buckets, and packet artifact UX improvements. | PR #66, #68, #70, #71, #80, #81, #130, #131 |
| Cache reuse across worktrees | `cache rehydrate` command, SQLite graph/search/doc rebasing, portable v2 artifact-cache keys, canonical repository identity, canonical sidecar generation identity, and fail-closed sidecar revalidation semantics. | PR #84, #92, #114, #118, #123 |
| Cross-platform operator docs | Cache recovery and release-review support documented for Windows, macOS, and Linux operator flows. | PR #146 |
| Packet-runtime diagnostics | Batch setup reuse, search timing, batch overhead attribution, final-output/residual-wall timing, strict batch bounds, compact probe tapering, and artifact/reporting cleanup. | PR #86, #88, #93, #97, #101, #110, #116, #125, #127, #130 |
| Code reduction and abstraction cleanup | `enum_dispatch` resolver slice, shared language registry routing, mirrored enum conversion cleanup, retrieval manifest fixture helper, CLI DTO fixture cleanup, and retrieval stage metadata centralization. | PR #94, #102, #103, #108, #109, #113 |
| Release evidence and review surface | Promotion audit evidence, cross-platform release-review support, and a reviewer branch rooted at `v0.9.0` before version-bump noise. Generated report packages belong in PRs, issues, or external artifacts, not durable repo docs. | PR #77, #145, #146, #151 |

### Evidence and Comparison

| Gate | 0.9.0 baseline / previous state | 0.10.0 result | Evidence |
| --- | --- | --- | --- |
| Reviewer diff | Baseline tag `v0.9.0` at `2feb60990c6e`. | Review branch `review/codestory-saga-from-v0.9.0-f4f6d3d6` preserves the saga diff before the version bump. | Compare URL above; #74 |
| Workspace release version | Workspace crates were synchronized at `0.9.0`. | All eight `codestory-*` workspace crates and `Cargo.lock` are synchronized at `0.10.0`. | PR #151; `check-codestory-release.py --version 0.10.0` |
| Repo-scale e2e after sidecar repair | No release claim based only on `retrieval_mode=full`. | E2E passed after repair with 14,041 symbol docs, 760 dense docs, 0 index errors, 83.31s full index, 28.42s repeat refresh, and 8.70s retrieval index. | #72 and associated target artifacts |
| Focused packet quality | Publishable packet-runtime evidence was blocked. | Focused Apache and Redis rows had quality `3/3` and sufficiency `sufficient:3`. | #143 |
| Packet-runtime SLA | Not cleared. | Redis focused cold row cleared `0/3` SLA misses; Apache focused cold still missed `2/3`. Warm SLA remains accepted residual risk. | #78; #143 |
| Cache reuse | Cache identity was path/root-bound and expensive for parallel agent worktrees. | SQLite graph/search/doc rows and portable v2 artifact-cache rows can be reused across compatible clean worktrees; retrieval sidecars revalidate/rebuild fail-closed instead of being blindly trusted. | #82; PR #84, #114, #118, #123 |
| Release notes / review package | No final package for the saga diff. | Report package was produced for review outside committed docs. | #143 |

### Packet-Runtime Release Risk

| Evidence row | Quality | Sufficiency | SLA result | Retrieval median | Decision |
| --- | ---: | --- | ---: | ---: | --- |
| Apache Commons Lang cold focused row | 3/3 | `sufficient:3` | 2/3 misses | 15,528 ms | Accepted/deferred risk; #78 was later closed as stale. |
| Redis cold focused row | 3/3 | `sufficient:3` | 0/3 misses | 7,722 ms | Clear for the focused cold row. |

The full publishable packet-runtime gate is not claimed as cleared. Earlier draft
and diagnostic PRs remain evidence surfaces, not shipped SLA fixes, unless their
specific code changes landed in the PR list above.

### Still Not Shipped

- Packet-runtime SLA clearance and publishable promotion evidence.
- Full precise semantic import implementation beyond the contract/proof slices.
- Broad structural collector rollout beyond the workflow-source pilot.
- True offline retrieval sidecar preservation during `cache rehydrate`; current
  behavior is fail-closed revalidation/rebuild under canonical sidecar identity.
- Any manually created release tag. Tags and binary assets remain owned by the
  repository release workflow.

## 0.7.0

- Current synchronized workspace release baseline.
- Future synchronized CodeStory workspace version bumps on `main` create GitHub
  releases with cross-platform `codestory-cli` binary assets and `SHA256SUMS.txt`.

## Release Notes

- Add concise human-facing notes under the bumped version before merging a
  release version change to `main`.
- Keep release notes focused on user-visible CLI, grounding, retrieval,
  packaging, and documentation changes.
