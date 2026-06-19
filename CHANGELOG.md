# Changelog

## 0.10.0

CodeStory 0.10.0 turns the post-0.9.0 research wave into releaseable
contracts, proof/provenance plumbing, cache-reuse primitives, release evidence,
and smaller maintenance surfaces. It is not a packet-runtime SLA clearance
release: #78 remains open and is carried as accepted/deferred release risk.

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
| Release evidence and review surface | Promotion audit, final pre-release report package, CSV evidence table, SVG visual summary, and a reviewer branch rooted at `v0.9.0` before version-bump noise. | PR #77, #145, #146, #151 |

### Evidence and Comparison

| Gate | 0.9.0 baseline / previous state | 0.10.0 result | Evidence |
| --- | --- | --- | --- |
| Reviewer diff | Baseline tag `v0.9.0` at `2feb60990c6e`. | Review branch `review/codestory-saga-from-v0.9.0-f4f6d3d6` preserves the saga diff before the version bump. | Compare URL above; #74 |
| Workspace release version | Workspace crates were synchronized at `0.9.0`. | All eight `codestory-*` workspace crates and `Cargo.lock` are synchronized at `0.10.0`. | PR #151; `check-codestory-release.py --version 0.10.0` |
| Repo-scale e2e after sidecar repair | No release claim based only on `retrieval_mode=full`. | E2E passed after repair with 14,041 symbol docs, 760 dense docs, 0 index errors, 83.31s full index, 28.42s repeat refresh, and 8.70s retrieval index. | `docs/testing/issue-72-promotion-audit-2026-06-18-f61e6717cbbf.md` |
| Focused packet quality | Publishable packet-runtime evidence was blocked. | Focused Apache and Redis rows had quality `3/3` and sufficiency `sufficient:3`. | `docs/testing/pre-release-review-2026-06-19-ce1047249fe5.md`; #143 |
| Packet-runtime SLA | Not cleared. | Redis focused cold row cleared `0/3` SLA misses; Apache focused cold still missed `2/3`. Warm SLA remains accepted residual risk. | #78; #143 |
| Cache reuse | Cache identity was path/root-bound and expensive for parallel agent worktrees. | SQLite graph/search/doc rows and portable v2 artifact-cache rows can be reused across compatible clean worktrees; retrieval sidecars revalidate/rebuild fail-closed instead of being blindly trusted. | #82; PR #84, #114, #118, #123 |
| Release notes / review package | No final package for the saga diff. | Report package includes Markdown, CSV, and SVG visual summary. | `docs/testing/pre-release-review-2026-06-19-ce1047249fe5.*` |

### Packet-Runtime Release Risk

| Evidence row | Quality | Sufficiency | SLA result | Retrieval median | Decision |
| --- | ---: | --- | ---: | ---: | --- |
| Apache Commons Lang cold focused row | 3/3 | `sufficient:3` | 2/3 misses | 15,528 ms | Accepted/deferred risk; keep #78 open. |
| Redis cold focused row | 3/3 | `sufficient:3` | 0/3 misses | 7,722 ms | Clear for the focused cold row. |

The full publishable packet-runtime gate is not claimed as cleared. Earlier draft
and diagnostic PRs remain evidence surfaces, not shipped SLA fixes, unless their
specific code changes landed in the PR list above.

### Still Not Shipped

- #78 packet-runtime SLA clearance and publishable promotion evidence.
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
