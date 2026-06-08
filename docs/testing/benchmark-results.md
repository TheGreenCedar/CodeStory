# CodeStory Benchmark Results

This is the short, decision-grade scorecard linked from the README. It keeps
current claims cautious and points detailed history to the
[benchmark ledger](benchmark-ledger.md).

Runs recorded before the 2026-05-24 harness tightening are historical unless
they are reanalyzed or rerun with answer-level expected-file/symbol recall,
immutable manifest refs, and CodeStory cache provenance. The harness now keeps
transcript-observed anchors separate from anchors actually present in the final
answer, so tool output alone cannot make a row quality-pass.

## Current Scorecard

| Lane | Current status | Public claim status |
| --- | --- | --- |
| Agent A/B quick check | The 2026-05-23 CodeStory-only quick run passed both arms, but the CodeStory arm used more tokens, more wall time, and more tool starts. | No agent savings claim. |
| Local-real Codex probe | On 2026-05-25, the narrowed `codex-exec-json-flow` live A/B repeated with a quality-passing CodeStory arm against a failing no-CodeStory arm. Latest corrected-wrapper repeat: `114,510` vs `2,209,856` tokens, `2` vs `39` observed tool calls, `117.37s` vs `262.39s`, and overhead ratio `0.183466`. | Strong exploratory evidence; no promotion claim from this task alone. |
| Local-real Sourcetrail probe | On 2026-05-25, the `sourcetrail-indexing-to-storage` live A/B passed with CodeStory after source-group/indexing/storage packet fixes. CodeStory used `269,363` vs `5,697,852` tokens, `2` vs `105` observed tool calls, `138.92s` vs `532.68s`, `0` vs `87` source reads, and overhead ratio `0.10904`. | Strong second-repo exploratory evidence; still not promotion-grade because it is one repeat using a local existing cache. |
| Local-real VS Code probe | On 2026-05-25, the `vscode-workbench-extension-host` packet holdout moved from partial coverage to a sufficient packet, then the live A/B passed with CodeStory after workbench/extension-host packet fixes. CodeStory used `1,070,153` vs `7,296,578` tokens, `2` vs `115` observed tool calls, `329.69s` vs `626.08s`, `0` vs `71` source reads, and overhead ratio `0.230215`. A follow-up release incremental refresh repaired the stale cache provenance, moving VS Code freshness from `74` new files to `0`. | Strong third-repo exploratory evidence; still not promotion-grade because it is one repeat and the no-CodeStory arm failed quality. |
| Local-real drill-suite probe | On 2026-05-25, a four-repo `drill-suite` matrix exposed a real CodeStory cache-reuse blocker, stale Codex anchor selections, VS Code indexing-error blockage, and Sourcetrail source-truth-only bridges. After the CodeStory cache fix and Rust receiver/return-chain graph pass, this repo's one-case drill is still degraded but now resolves `11/11` anchors with `28/55` graph bridges, `27` partial bridges, and `0` unresolved bridges. | Diagnostic product evidence only; the remaining target is store/workspace execution-plan and snapshot/projection bridge coverage. |
| Strict packet-first rows | Several with-CodeStory public-checkout rows passed quality, packet-first, and zero ordinary source reads after packet. | Behavior evidence only; paired savings still needs broader quality-passing baselines. |
| Packet runtime | Public-core warm stdio and cold CLI packet rows passed repeated publishable quality gates. | Runtime evidence, not agent-token savings. |
| Repo-scale cold index/read timing | The current timing source is the latest row in [codestory-e2e-stats-log.md](codestory-e2e-stats-log.md). | Current only after a fresh row is logged for the relevant change. |
| Warm stdio smoke | The current warm-loop timing source is [codestory-stdio-warm-loop-stats.md](codestory-stdio-warm-loop-stats.md). | Smoke evidence for the persistent read surface. |

## What Is Solid

- CodeStory can produce quality-passing packet-first answers on selected public
  tasks while avoiding ordinary source reads after the answer packet.
- Repeated packet-runtime rows show `packet` can fit inside an agent workflow
  budget in both cold CLI and warm stdio modes.
- The local-real harness now separates first-index setup cost from timed
  cache-reuse agent work, blocks stale or semantic-empty caches from
  publishable evidence, and records useful-context density for final-answer
  context instead of raw packet volume alone.
- The quality-passing local-real Codex live A/B has now repeated on the same
  task with the corrected wrapper.
- Sourcetrail now adds a second realistic repo where the CodeStory arm passed
  quality and avoided source reads while the no-CodeStory arm failed quality
  after broad exploration.
- VS Code now adds a large TypeScript repo where the packet planner can find the
  workbench startup, extension service, extension host manager, extension-host
  activation, and command execution anchors without follow-up commands, and the
  live CodeStory arm passed quality while using far fewer tools and source
  reads than the no-CodeStory arm.
- The VS Code cache freshness issue behind the first local-real row is now
  understood and fixed: TypeScript/TSX factory-call superclass extraction no
  longer crashes on `extends mock<T>()`, failed attempts are recorded as
  incomplete files with attached errors, and `../vscode` now reports
  `10,491/10,491` indexed files as fresh after incremental refresh.
- CodeStory's own active cache can now recover from stale incremental
  projection cleanup where cross-file callable state points at a deleted node;
  release incremental refresh reports fresh inventory with `150/150` indexed
  files, `0` errors, and `7,794` semantic docs.
- The tightened CodeStory drill now exposes the CLI-to-runtime-to-indexer path
  mostly as graph evidence: Rust receiver and return-chain resolution moved the
  case from `3/55` graph bridges to `28/55`, while preserving explicit
  source-truth-only status for the remaining unproven bridge pairs.
- Repo-scale timing history is tracked in the stats log instead of copied into
  prose that silently drifts.

## What Is Not Claimed

This page does not claim that CodeStory generally reduces agent cost, token
count, wall time, or tool calls. General savings claims require repeated
controlled with/without-agent measurements from the benchmark harness, not one
exploratory row or representative estimates.

The 2026-05-25 Codex, Sourcetrail, and VS Code local-real rows are explicitly
non-promotional. They show a CodeStory advantage on three realistic tasks, but
they are still single-run or same-task exploratory measurements using local
cache state. The VS Code cache now has fresh provenance after the follow-up
repair, but public savings language still needs repeated controlled rows, clean
pinned checkout provenance, and at least one holdout that was not tuned during
the implementation loop.

The 2026-05-25 `drill-suite` rows are also non-promotional. They are designed to
find grounding failures before an agent A/B run, and the current CodeStory case
still falls back to source-truth-only evidence for `27/55` bridge pairs.

## Proof Tier Ladder

Use the highest tier actually reached when describing a row. Do not promote a
lower tier into a broader claim just because the command exited successfully.

| Proof tier | Required evidence | Can claim | Cannot claim |
| --- | --- | --- | --- |
| Stats-only local regression signal | `codestory_repo_e2e_stats` completed with skip allowances or without prepared full sidecars. | Local timing, indexing, and cache-shape regression signal for the current checkout. | Full sidecar readiness, agent packet/search readiness, real-repo release coverage, or performance promotion. |
| Full sidecar readiness proof | Zoekt, Qdrant, SCIP, and llama.cpp are running; `retrieval index --refresh full` succeeds; `retrieval status --format json` reports `retrieval_mode: "full"` and product backend fields. | Agent-facing packet/search readiness for the verified workspace and cache state. | General quality, cross-repo coverage, or benchmark savings. |
| Real-repo drill proof | Prepared real-repo drill manifests run without skip allowances and produce expected evidence packets, source-truth checks, and verdicts. | The release path was exercised beyond the CodeStory checkout on the named drill cases. | Generalized agent savings or promotion-grade performance. |
| Promotion-grade benchmark proof | Controlled baseline and candidate benchmark rows use pinned refs, comparable cache state, sidecar status, answer-level quality gates, and no-regression thresholds. | Cautious performance or retrieval-quality promotion for the measured scope. | Universal savings, untested repos, or environments outside the recorded setup. |

## Promotion Rules

- Use the same project, cache state, semantic backend, command flags, runner,
  model, and sample shape when comparing before/after results.
- Do not promote a speed win if expected anchors, answer-level quality, protocol
  cleanliness, or semantic-doc reuse regress.
- Treat small-fixture warm-loop numbers as smoke evidence, not repo-scale
  product proof.
- Append current repo-scale timing rows to
  [codestory-e2e-stats-log.md](codestory-e2e-stats-log.md) when default
  indexing, semantic persistence, embedding reuse, or cold-start behavior
  changes.

## Detailed History

- Detailed agent A/B rows, packet-runtime history, methodology, and commands:
  [benchmark-ledger.md](benchmark-ledger.md)
- Repo-scale timing history:
  [codestory-e2e-stats-log.md](codestory-e2e-stats-log.md)
- Warm stdio loop history:
  [codestory-stdio-warm-loop-stats.md](codestory-stdio-warm-loop-stats.md)
