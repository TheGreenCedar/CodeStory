# Quality Gaps: CodeStory Real-Repo Agent Drill

Current-run baseline: `target/codestory-cross-repo-test/20260522-100457/final-report.md`.

The dashboard metric is the number of unchecked items below. Close an item only
after a rerun of the real-repo drill shows the gap is gone or deliberately
accepted with source-verified evidence.

- [x] Receiver-aware call resolution prevents false method edges such as the Sourcetrail `IndexerJava::doIndex -> CxxParser::buildIndex` trail.
- [x] Search ranks active production/render paths above definition-only anchors, for example rootandruntime public writing paths over an unused `getElsewhereFeed` definition.
- [x] Search and trail label definition-only or no-caller anchors so agents do not overstate runtime participation.
- [x] `snippet` and drill evidence include whole relevant function or route bodies by default when line windows would omit the decisive operation.
- [x] Caller and consumer summaries expose who actually uses an anchor, especially storage access and Payload collections.
  - Packet 4 exposes related Payload collection consumers for `Posts`; Packet 5 exposes repo-text consumer hints for Sourcetrail `StorageAccess` while clearly labeling that graph consumers are still missing.
- [x] Trail output suppresses self-edges, duplicate loops, and low-confidence bridges unless explicitly requested.
  - Packet 6 keeps default self/duplicate suppression, makes strict trail filtering hide weak runtime bridge edges while preserving probable data/config usage edges, and suppresses low-confidence Search Plan bridge rows into source-truth prompts.
- [x] CodeStory surfaces execution-boundary paths for CLI commands instead of leaving agents to infer CLI/runtime/store relationships from symbol names alone.
- [x] Agent-facing commands produce machine-checkable claim ledgers or source-truth prompts for claims that need verification.
- [x] The repeatable drill can be run as one canonical command that indexes, asks, verifies, and writes a comparison report.
  - Packet 7 adds `codestory-cli drill-suite`, which derives the fixed Sourcetrail/CodeStory/rootandruntime drill matrix from the owning CodeStory checkout, runs each per-repo drill, writes per-repo reports, and emits aggregate `suite-report.md` / `suite-report.json` verdicts.
- [x] The drill emits a compact `drill-summary.json` with mechanical status, anchor status, source-truth prompts, and open gaps.
- [x] Golden regression coverage protects the three real-repo questions from ranking, trail, snippet, and verification regressions.
  - Packet 8 updates the ignored real-repo e2e drill harness to execute `drill-suite` once for the fixed three-repo matrix and assert question search, per-anchor symbol/trail/snippet/explore artifacts, source-truth checks, verdict usability, and the key caller/consumer/text-hint signals. The ignored test passed against the real sibling repos in 122.62s.
- [x] The final cross-repo report clearly separates ready, degraded, and blocked states without burying the agent-UX verdict in raw command output.

## Fresh Round 2: Suite Isolation

- [x] `drill-suite --cache-dir` isolates per-target caches under the explicit suite cache root instead of silently discarding the caller's cache argument.
  - Packet 9 propagates explicit suite cache roots into per-repo cache subdirectories, avoids the duplicate pre-open path before drill refresh, and verifies a full explicit-cache suite run with separate `sourcetrail`, `codestory`, and `rootandruntime` cache directories.

## Fresh Round 3: Bridge-To-Verification Usability

- [x] Bridge evidence uses existing consumer/text/shared-file evidence to produce actionable partial paths instead of reporting `no_bridge_found` for every anchor pair when nearby evidence exists.
  - Baseline symptom: the explicit-cache suite reported `graph_path=0` and `unresolved_or_error=3` for every repo, even though the same reports included useful consumer and text-hint evidence.
  - Packet 11 rerun `20260522-155014-round3-packet14` preserves `graph_path=0` but converts every pair to `evidence_hint_only`; each repo now reports `partial=3` and `unresolved_or_error=0`.
- [x] Consumer summaries promote their best evidence into source-truth checks and bridge follow-up targets.
  - Baseline examples: Sourcetrail `StorageAccess` had text consumer hints, CodeStory `WorkspaceIndexer` had text hints, and rootandruntime `Posts` had Payload collection consumers, but those signals were not promoted into the verification checklist.
  - Packet 10 promotes caller/consumer/text-hint evidence into required source-truth checks and emits bridge-pair follow-up searches with `--repo-text on`.
- [x] Drill verdict next actions are repo-specific and evidence-specific.
  - Baseline symptom: the suite repeated the same generic next action for all three repos instead of pointing agents to the strongest consumer hints, source files, or bridge-verification commands for that repo.
  - Packet 10 rerun `20260522-153948-round3-packet13b` now produces per-repo next actions: Sourcetrail points at `SourceGroupCxxCdb.h`, `IndexerJava.h`, and `StorageAccess.h`; CodeStory points at indexer/runtime/contracts files; rootandruntime points at `Posts.ts`, `social-feed.ts`, and `comment-auth.ts`.
- [x] Source-truth checklists include behavior-boundary files needed to verify the asked flow, not only selected symbol definitions.
  - Baseline symptom: checks identified anchor definition files but under-covered route, command, indexing, storage, and public-surface boundaries that the questions actually ask about.
  - Packet 10 expands source-truth checks to include bridge endpoints/shared files plus caller, consumer, and repo-text hint files; the suite now reports 16/19/17 checks across Sourcetrail/CodeStory/rootandruntime instead of only selected definitions.

## Fresh Round 4: Freshness And Evidence Ranking

- [x] Drill and drill-suite summaries surface stale index/repo freshness as an agent-UX blocker before claiming evidence is ready.
  - Packet 11's CodeStory drill used `--refresh none` while the CodeStory worktree had changed files; `question-search.json` reported stale freshness, but the aggregate suite summary still emphasized `index_ready=true` and generated `--refresh none` follow-up commands.
  - Packet 12 rerun `20260522-161057-round4-packet16` shows the suite markdown/JSON CodeStory row as `freshness=stale` and the next action starts with `codestory-cli index --refresh incremental`.
- [x] Bridge evidence ranks production/runtime boundary files ahead of bench, test, migration, and script hints.
  - Packet 11 correctly emitted `evidence_hint_only`, but some `evidence_files` lists put benches or migration/import scripts ahead of files that better match the requested architecture flow.
  - Packet 12 rerun `20260522-161057-round4-packet16` ranks CodeStory runtime files before bench files and rootandruntime `src/lib/comment-auth.ts` before import/migration scripts in bridge `evidence_files`.

## Fresh Round 5: Handoff Surface Parity

- [x] Source-truth and claim-ledger file lists preserve the same runtime/source-first ordering as bridge evidence.
  - Fresh full suite `20260522-162027-fresh-iteration` showed bridge `evidence_files` were ranked, but CodeStory summary `source_truth.target_files` still led with bench files and rootandruntime led with scripts before public/runtime files.
  - Packet 18 rerun `20260522-163139-round5-packet18` ranks CodeStory source-truth files with CLI/contracts/indexer/runtime files first and rootandruntime files with public route/collection/admin/lib files before tests/scripts.
- [x] Anchor consumer summaries show public/runtime consumers before scripts, benches, migrations, and tests.
  - Fresh Round 5 discovery showed rootandruntime `Posts` had public route evidence, but visible consumers could start with import/migration scripts.
  - Packet 18 ranks consumer examples by source role; rootandruntime `Posts` now lists `src/app/(frontend)/posts/[slug]/comments/route.ts` first.
- [x] Bridge evidence rows include endpoint definition files separately from hint files.
  - Fresh Round 5 discovery showed `Posts -> getElsewhereFeed` bridge rows could list hint files without directly naming both endpoint source files.
  - Packet 18 emits `endpoint_files`; `Posts -> getElsewhereFeed` now names `src/collections/Posts.ts` and `src/lib/social-feed.ts` on the bridge row.
- [x] Suite markdown surfaces symbolic-only retrieval at the top level.
  - Fresh Round 5 discovery showed detailed reports exposed `semantic_unavailable:fallback=MissingEmbeddingRuntime`, while `suite-report.md` only showed verdict/freshness/anchors/bridges.
  - Packet 18 adds a `retrieval` column; all three fresh suite rows show `symbolic-only`.
- [x] Repo-local grounding skill documentation covers the current drill and drill-suite contracts.
  - Subagent review found `.agents/skills/codestory-grounding` still described only old `drill-report` outputs and did not mention `drill-suite`, `drill-summary`, endpoint files, or consumer summaries.
  - Packet 18 updates `SKILL.md`, `references/drill.md`, and adds `references/drill-suite.md`.

## Fresh Round 6: Related Target Truth

- [x] Related Payload collection consumer rows point their target file at the collection source, not a synthetic usage/script file.
  - Packet 18 fixed consumer ordering, but rootandruntime `Posts` consumer rows still reported `target_file_path=scripts/import-wordpress-rich-content.ts` for `related_payload_collection:posts`, making the target look like an import script instead of the collection definition.
  - Packet 19 carries the selected anchor's preferred source-truth file into related Payload consumer targets; fixture coverage now asserts `target_file_path` ends with `src/Posts.ts`.

## Fresh Round 7: Trail Layout Suppression

- [x] `trail --hide-speculative` removes suppressed probable/low-confidence runtime bridge edges from both `edges` and `canonical_layout.edges`.
  - Subagent review found `hide_speculative_trail_edges` filtered `response.edges` with `is_speculative_trail_edge`, but filtered `canonical_layout.edges` only by speculative certainty labels and reachability, allowing probable/low-confidence `CALL` or `MACRO_USAGE` edges to remain visible through the canonical layout.
  - Packet 20 filters canonical layout edges by retained `source_edge_ids`; runtime regression coverage now keeps a suppressed probable edge out of `canonical_layout.edges` even when its endpoints remain reachable through retained edges.

## Fresh Round 8: Broad Question Anchor Planning

- [x] Broad real-agent questions that explicitly name anchors produce Search Plan named-anchor subqueries instead of leaving agents with generic semantic suggestions.
  - Fresh Sourcetrail Packet 20 evidence showed the real question had `exact_symbol_hit_count=0`, `weak_top_hit=true`, no Search Plan, and a generic repo-text rerun recommendation even though it named `SourceGroupCxxCdb`, `IndexerJava`, and `StorageAccess`.
  - Packet 21 treats explain-how architecture questions as plan-eligible, ranks compound named anchors ahead of project/filler terms, emits per-anchor typed subqueries, and the fresh suite shows Sourcetrail question-search anchor groups containing all three named anchors.

## Fresh Round 9: Drill-Suite Progress Visibility

- [x] Long `drill-suite` runs emit stderr progress without corrupting stdout JSON.
  - Fresh Packet 20 and Packet 21 verification showed full-refresh drill-suite runs could be silent for minutes, leaving agents and the dashboard observer unable to tell whether indexing, per-repo drill work, or report writing was still alive.
  - Packet 22 emits suite start, per-repo start, per-repo done, report-writing, and final summary progress to stderr; the smoke run proves stdout remains valid JSON while stderr contains progress heartbeats.

## Fresh Round 10: Source-Truth Checklist Compression

- [x] Drill source-truth checks group repeated file evidence instead of making agents work through duplicate per-role rows.
  - Fresh Packet 22 evidence still showed verbose source-truth check counts: Sourcetrail 25 checks over 10 files, CodeStory 28 checks over 9 files, and rootandruntime 25 checks over 6 files.
  - Packet 23 groups checks by file while preserving roles in the reason text; the rerun reports Sourcetrail 10/10 checks/files, CodeStory 9/9, and rootandruntime 6/6.

## Fresh Round 11: Suite Bridge Degradation Visibility

- [x] Suite markdown bridge summaries distinguish graph paths, partial evidence, and unresolved/error bridges instead of showing only total bridges and unresolved/error counts.
  - Fresh Segment 5 suite `20260522-181557-segment5-fresh-drill` is degraded for all three repos because every bridge is partial (`graph_path=0`, `partial=3`, `unresolved_or_error=0`), but `suite-report.md` renders each row as `3 total, 0 unresolved/error`, hiding the partial/no-graph reason behind the degraded verdict.
  - Packet 26 changes the suite markdown bridge column to show graph/partial/unresolved-error counts directly; the rerun at `20260522-183036-segment5-bridge-label` shows all three repos as `0 graph / 3 partial / 0 unresolved-error`.

## Fresh Round 12: Verdict Reason Bridge Specificity

- [x] Degraded verdict reasons include graph/partial/unresolved bridge counts instead of mentioning only `unresolved_or_error_bridges`.
  - Fresh full suite `20260522-183954-fresh-scout` correctly renders each markdown bridge cell as `0 graph / 3 partial / 0 unresolved-error`, but `suite-report.json` still reports reasons like `source_truth_required=true unresolved_or_error_bridges=0` for every degraded repo. That wording hides the actual degradation condition (`graph_path=0`, `partial=3`) and can make a degraded verdict look contradictory.
  - Packet 28 changes degraded verdict reasons to include `graph_bridges`, `partial_bridges`, `unresolved_or_error_bridges`, and `pending_source_truth_checks`; the suite smoke at `20260522-185830-segment6-report-clarity` shows all three repo reasons with the bridge-specific counts.

## Fresh Round 13: Newly Closed Metric Accuracy

- [x] `quality_newly_closed` uses the previous Segment 5 primary metric as a fallback when historical run `metrics` omit `quality_closed`.
  - After accepting the Round 12 open gap, `autoresearch.ps1` reported `quality_newly_closed=1` even though no checklist item had just closed. Segment 5 runs log `quality_closed` as the primary `metric`, but the secondary `metrics` object omits `quality_closed`, so the wrapper can fall back to stale pre-segment closed counts.
  - Packet 28 makes the wrapper fall back to the Segment 5 primary `metric` when `metrics.quality_closed` is absent; Segment 6 baseline run 27 records `quality_newly_closed=0` with the accepted gaps open.

## Fresh Round 14: Verification Debt Wording

- [x] Drill-suite and drill summaries distinguish emitted source-truth targets from completed source verification.
  - Fresh full suite `20260522-183954-fresh-scout` shows aggregate `source checks/files` as `10/10`, `9/9`, and `6/6`, which can read like completed verification even though the run has only emitted verification targets. The same artifacts have `source_truth.required=true`, nonzero check counts, and no source verification performed.
  - Packet 28 renames the suite markdown column to `source targets/files` and adds `pending_check_count`, `verified_check_count`, `pending_claim_count`, and `verified_claim_count` to summary JSON.

- [x] `needs_verification_count` no longer looks like zero remaining work when source-truth checks are required.
  - Fresh full suite `20260522-183954-fresh-scout` reports `source_truth.required=true`, `overall_status=partial`, and nonzero `source_truth.check_count`, while `open_gaps.needs_verification_count=0`. That counter is only explicit readiness messages, not pending source-truth checks.
  - Packet 28 keeps the legacy counter for compatibility but adds `needs_verification_claim_count` and `pending_source_truth_check_count`; the suite smoke reports pending source-truth checks as 10, 9, and 6.

## Fresh Round 15: ID-Stable Follow-Up Commands

- [x] Drill follow-up commands use selected node IDs when a concrete anchor was chosen, instead of re-querying ambiguous display text.
  - Fresh full suite `20260522-183954-fresh-scout` shows CodeStory anchors with `typed_hit_count=10`, and `WorkspaceIndexer-search.json` includes a concrete `node_id`/`node_ref`, but drill next commands in `codestory-drill/drill-report.md` still use `--query "WorkspaceIndexer"`. CodeStory already selected a target, yet the handoff reintroduces ambiguity before source verification.
  - Packet 30 adds selected node id/ref/kind/file/line to drill summary anchor statuses and emits `symbol`, `snippet`, and bridge `trail` follow-ups with `--id` when a chosen anchor is available. The suite smoke at `20260522-191603-segment7-id-handoff` shows CodeStory `WorkspaceIndexer` selected as node `568779309516800935` and follow-ups using that id.

## Fresh Round 16: Pending Claim Scoring And Scope

- [x] Claim ledger scoring renders as pending until source-truth classifications exist, instead of showing all-zero correct/partial/misleading/unsupported counts.
  - Fresh full suite `20260522-183954-fresh-scout` reports all claim classifications as pending, then renders `scoring: correct=0 partial=0 misleading=0 unsupported=0`, which can look like no misleading or unsupported claims were found before verification happened.
  - Packet 32 adds `status=pending_source_verification` and `pending_claim_count` to claim-ledger scoring and renders markdown as `score_status=pending_source_verification pending=6` until classifications exist.

- [x] Degraded next actions mention all pending claims, not only the three low-confidence bridges.
  - Fresh full suite `20260522-183954-fresh-scout` says to verify `3 degraded bridge(s)`, but each per-repo claim ledger has 6 pending claims: 3 anchor claims and 3 bridge claims.
  - Packet 32 changes degraded next actions to say `verify 6 pending claim(s), starting with 3 degraded bridge(s)` while preserving the file preview.

## Fresh Round 17: Source-Verification Handoff And Suite Truth

- [x] Bridge artifacts and next commands directly open source-truth targets for `evidence_hint_only` bridges instead of leaving bridge `next_commands` empty.
  - Fresh full suite `20260522-194211-fresh-after-run32` still has all nine bridges as `evidence_hint_only` with `graph_path.edge_count=0`; representative bridge JSON such as `codestory-drill/WorkspaceIndexer-to-SearchService-bridge.json` leaves `next_commands` empty while the suite says to use emitted bridge/consumer follow-up commands.
  - Sourcetrail and CodeStory reviewers both found that the report points to evidence files but does not emit concrete snippet/context commands for pending source-truth files and bridge evidence files.
  - Packet 34 emits bridge-specific `next_commands` in each `*-bridge.json` and renders them in `drill-report.md`; the fresh suite `20260522-200425-round17-handoff` shows 8-10 follow-up commands per bridge artifact.

- [x] Suite source-truth progress renders pending verification explicitly instead of showing `source targets/files` as `10/10`, `9/9`, and `6/6`.
  - Fresh full suite `20260522-194211-fresh-after-run32/suite-report.md` shows `source targets/files` as `10/10`, `9/9`, and `6/6`; the same summaries report `verified_check_count=0` and all checks pending.
  - Render this as target/verified/pending state, for example `10 targets / 0 verified / 10 pending`.
  - Packet 34 changes the suite table to `source truth`; fresh suite `20260522-200425-round17-handoff/suite-report.md` renders `10 targets / 0 verified / 10 pending`, `9 targets / 0 verified / 9 pending`, and `6 targets / 0 verified / 6 pending`.

- [x] Pending claim-ledger text no longer says "CodeStory evidence is sufficient" before source verification.
  - Fresh full suite `20260522-194211-fresh-after-run32/*-drill/drill-report.md` renders `score_status=pending_source_verification pending=6`, but the pending anchor claims still say `CodeStory evidence is sufficient to make the architecture claim`.
  - The ledger should describe candidate architecture/bridge claims that require source-truth verification, not evidence sufficiency that has not been proven.
  - Packet 34 rewrites pending ledger entries as candidate claims requiring source-truth verification; fresh suite `20260522-200425-round17-handoff` contains no `CodeStory evidence is sufficient` claim text.

- [x] Suite reports the shared semantic retrieval/runtime blocker at the top level when every repo is symbolic-only.
  - Fresh full suite `20260522-194211-fresh-after-run32` reports `symbolic:semantic_unavailable:fallback=MissingEmbeddingRuntime` for all three repos, while the aggregate markdown only shows `symbolic-only` per row.
  - Add a top-level retrieval blocker/semantic runtime status so agents can distinguish environment degradation from repo-specific graph/search friction.
  - Packet 34 adds `retrieval_blockers` to the suite JSON and a `## Retrieval Blockers` markdown section; the fresh suite groups all three repos under `symbolic:semantic_unavailable:fallback=MissingEmbeddingRuntime`.

## Fresh Round 18: Isolated Semantic Setup And Regression Coverage

- [x] Explicit `--cache-dir` drill-suite runs reuse globally installed managed embeddings when the isolated cache does not have local embedding assets.
  - `codestory-cli setup embeddings --project C:\Users\alber\source\repos\codestory` installed managed ONNX assets successfully, and default-cache `doctor` reported hybrid retrieval ready for Sourcetrail, CodeStory, and rootandruntime.
  - Fresh isolated suite `20260522-202502-semantic-scout` still reported `symbolic:semantic_unavailable:fallback=MissingEmbeddingRuntime` for all three repos because `--cache-dir target/.../cache` made runtime look for managed assets under the isolated cache.
  - `doctor --project C:\Users\alber\source\repos\codestory --cache-dir target/codestory-cross-repo-test/20260522-202502-semantic-scout/cache` reported `CODESTORY_EMBED_ONNX_MODEL is not set` and `Managed ONNX assets are not installed`, proving the setup success did not carry into isolated-cache runs.
  - Packet 36 changes runtime managed-embedding selection so explicit cache dirs still isolate index storage but fall back to globally installed managed assets when the isolated cache lacks local assets. The fresh suite `20260522-203922-semantic-fallback` reports `hybrid:semantic_ready` for Sourcetrail, CodeStory, and rootandruntime.

- [x] Fresh drill/report improvements have deterministic regression coverage for bridge-local follow-up commands, retrieval blocker rendering, pending-safe claim wording, and real-repo source-truth pending counts.
  - The latest artifacts show these behaviors working, but subagent coverage review found missing focused assertions for non-empty `evidence_hint_only.next_commands`, `## Retrieval Blockers`, absence of the old `CodeStory evidence is sufficient` wording, and real-repo `targets / 0 verified / ... pending` contracts.
  - Without these tests, the dashboard/report can regress back to stale zero-gap or overconfident handoff behavior while artifacts still superficially exist.
  - Packet 36 adds targeted unit coverage for managed fallback, bridge-local follow-up commands, suite retrieval blocker rendering, source-verification-safe claim wording, and pending source-truth counts in the ignored real-repo drill contract. Verified with `cargo test -p codestory-cli managed_embeddings`, `cargo test -p codestory-cli drill_`, and `cargo test -p codestory-cli --test codestory_repo_e2e_stats --no-run`.

## Fresh Round 19: Plateau Scout Findings

- [x] `forward_no_path` bridge graph payloads do not report `truncated=true` when they contain only the origin node, zero edges, and zero omitted edges.
  - Fresh isolated suite `20260522-203922-semantic-fallback` reports CodeStory as fresh and `hybrid:semantic_ready`, but `codestory-drill/WorkspaceIndexer-to-SearchService-bridge.json` and `WorkspaceIndexer-to-TrailResult-bridge.json` both have `graph_path.mode=forward_no_path`, `edge_count=0`, `omitted_edge_count=0`, and `truncated=true`.
  - This is distinct from the already-closed partial-bridge and bridge-follow-up gaps: the artifacts now have next commands and correct partial/degraded labels, but the graph payload still makes "no visible path" look like clipped path evidence.
  - Packet 38 suppresses truncation on zero-edge no-path bridge payloads when no edges were omitted while preserving truncation when omitted edges are present. The CodeStory spot drill `20260522-211200-round19-spot` shows both representative bridges with `truncated=false`.

- [x] Repo-configured `.codestory.toml cache_dir` behavior is deterministic and cannot select a repo-controlled managed executable asset root.
  - Packet 36 fixed explicit CLI `--cache-dir` fallback to global managed embeddings, but the runtime test for project config `cache_dir` currently depends on this workstation having global managed embeddings installed.
  - A repo-controlled config cache should isolate storage without selecting `cache_dir/managed-embeddings` as the managed executable asset root; tests should prove both the global-assets-present and no-global-assets cases instead of inheriting machine state.
  - Packet 38 keeps repo-configured cache dirs scoped to storage and uses only explicit CLI `--cache-dir` for managed asset-root selection. Tests now assert repo config cache dirs resolve storage to the configured cache while managed embeddings use the global root, and managed-root selection has a no-global-assets case.

## Fresh Round 20: Suite Handoff And Resume Surfaces

- [x] Suite markdown points agents to the per-repo drill reports and bridge artifacts that contain the emitted follow-up commands.
  - Fresh full suite `20260522-211810-fresh-after-run39/suite-report.md` says to `use emitted bridge/consumer follow-up commands before finalizing`, but the aggregate report only shows the suite `output_dir` and does not name the per-repo `*-drill/drill-report.md`, `drill-report.json`, or bridge JSON files where those commands live.
  - This is distinct from Fresh Round 17, which made bridge artifacts emit `next_commands`; the current gap is that the suite-level handoff tells agents to use emitted commands without linking the artifacts that contain them.
  - Packet 41 adds a `reports` column and a `## Repo Artifacts` section that names each per-repo markdown report, JSON report, and `*-bridge.json` artifact glob. Cached suite `20260522-214000-round20-final` shows the links in the aggregate report.

- [x] Active Autoresearch handoff docs show the current Round 19 plateau/scout state at the top instead of stale baseline/Round 8 state.
  - `autoresearch.md` still reports `Baseline: pending` near the top even though runs 37-39 are logged and Round 19 is closed with a plateau marker.
  - `tasks.md` still starts with a queued note saying accepted Round 8 gaps are closed and later says baseline logging was not forced, while the tail records the current Round 19 closure and `quality_plateau=1` stop/scout guard.
  - Packet 41 updates the active handoff docs to lead with the current Segment 9 / Round 20 state, the Round 19 plateau marker, and the log-open-before-fix guard.

- [x] Stale drill regression coverage forbids `--refresh none` readiness follow-ups when freshness is stale.
  - `drill_summary_surfaces_stale_freshness_and_refresh_followups` asserts incremental refresh commands exist, but it does not assert stale readiness commands no longer contain `--refresh none`.
  - A regression could leave stale `--refresh none` searches next to refresh-first commands and still pass the current test.
  - Packet 41 adds negative JSON and markdown assertions so stale readiness follow-ups cannot leave `--refresh none` beside refresh-first commands.

- [x] The real-repo drill-suite regression protects broad-question Search Plan named-anchor planning, not only explicit seed anchor resolution.
  - `codestory_repo_e2e_stats` asserts `question_search.status == "ok"` and later checks explicit `--anchors`, but it does not inspect `question-search.json`/`question_search.search_plan` to prove the natural-language question itself surfaces the named anchors.
  - The drill should remain protected from the broad question forward; otherwise explicit seed anchors can mask weak question decomposition.
  - Packet 41 adds real-repo harness assertions for named-anchor Search Plan subqueries and fixes short PascalCase anchor planning so rootandruntime `Posts` appears as a `named_anchor` subquery. Cached suite `20260522-214000-round20-final` shows `Posts`, `getElsewhereFeed`, and `getCommentAuth`.

- [x] The real-repo drill-suite regression checks compact bridge status handoff entries, not only full bridge artifacts.
  - `codestory_repo_e2e_stats` checks the full `drill-report.json["bridges"]` count, but does not assert `summary.bridges.statuses` contains complete pair entries with anchor names, status, strategy, and command health.
  - Agents and dashboards read the compact suite JSON first, so disappearing bridge status details would hide which degraded bridge pair needs source verification even if full artifacts still exist.
  - Packet 41 adds real-repo harness assertions for compact bridge status count, pair labels, strategy, and command status. Cached suite `20260522-214000-round20-final` reports three compact bridge statuses for each repo with `command_status=ok`.

## Fresh Round 21: Plateau Handoff Consistency

- [x] Active Autoresearch blocker handoff matches the post-Round-20 plateau guard instead of telling the next packet to close Fresh Round 20 again.
  - Segment 10 run 42 records a measurement-only plateau after Fresh Round 20 closure: `quality_closed=50`, `quality_gap=0`, `quality_newly_accepted=0`, `quality_newly_closed=0`, `quality_stagnating=1`, and `quality_plateau=1`.
  - `tasks.md` correctly leads with the guard that run 42 is the current plateau marker, but its `## blockers` section still says the next packet should close Fresh Round 20 or discard a candidate.
  - This can send the next agent into a nonexistent Round 20 close/discard step even though all Round 20 gaps are already closed; the handoff should instead say to stop product iteration unless a fresh source-backed candidate is logged open first.
  - Packet 44 updates the blockers section to the plateau guard directly: product iteration stops while `quality_gap=0` unless a fresh source-backed candidate is logged open before it is fixed, or the work is a promotion / holdout gate.
