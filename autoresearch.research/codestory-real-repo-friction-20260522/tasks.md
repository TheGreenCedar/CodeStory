# Research Tasks: Reduce CodeStory real-repo agent drill friction across Sourcetrail, CodeStory, and rootandruntime. Track current-run quality gaps from source-verified drill evidence, implement improvements, rerun the drill, and repeat until gaps stop falling.

## queued
- Do not run another product packet while `quality_gap=0` unless a fresh,
  source-backed candidate is first logged as open or the work is a promotion /
  holdout gate. Run 42 is the current plateau marker after Fresh Round 20.

## in_progress
- None.

## done
- Scratchpad initialized.
- Current-run quality gaps replaced generic scaffold items.
- Dashboard server started on `http://127.0.0.1:8787/`; `/health` returned 200.
- Baseline metric validated as `quality_gap=12`, `quality_total=12`,
  `quality_closed=0`.
- Implemented and verified the Sourcetrail Java/C++ false-edge fix.
- Implemented and verified body-aware drill/search-plan snippets, including a
  route snippet that reaches `payload.create`.
- Reran doctor/index/ground/drill across all three repos with the updated
  indexer, then reran drill after the snippet promotion correction.
- Implemented and verified active-path ranking plus definition-only/no-caller
  labels against rootandruntime real-repo search/trail artifacts.
- Implemented and verified compact `drill-summary.json` artifacts across
  Sourcetrail, CodeStory, and rootandruntime drill reruns.
- Updated the visual quality-gap metric from 10 open gaps to 6 open gaps.
- Logged dashboard data points for the current-run baseline (`quality_gap=12`)
  and current verified state (`quality_gap=6`); the live server now serves 2
  runs from `autoresearch.jsonl`.
- Implemented and verified packet-3 drill caller/consumer summaries, execution
  boundary evidence, and ready/degraded/blocked verdict summaries.
- Reran drill for Sourcetrail, CodeStory, and rootandruntime into
  `target/codestory-cross-repo-test/20260522-131641-packet3-drill`.
- Wrote `packet3-comparison-summary.md` and updated the visual quality-gap metric
  from 6 open gaps to 4 open gaps.
- Refreshed the live dashboard through a normal packet/log path; run 4 confirms
  `quality_gap=4`, `quality_total=12`, and `quality_closed=8` with
  `cli_golden_path` passing.
- Implemented and verified Packet 4 related Payload collection consumer
  summaries. A real rootandruntime `Posts` drill now reports 9 consumers via
  `related_payload_collection:posts`, and source grep confirms the named
  `collection: "posts"` calls.
- Implemented and verified Packet 5 repo-text consumer hints for graph-empty
  anchors. A real Sourcetrail `StorageAccess` drill now reports 24 text-backed
  consumer hints while preserving the zero graph-consumer warning.
- Started a new Autoresearch segment for continued work after the first segment
  reached its packet limit.
- Implemented and verified Packet 6 trail/Search Plan noise suppression. Strict
  trail filtering hides weak runtime bridge edges without hiding probable
  Payload/data usage edges, and broad Search Plan output suppresses
  low-confidence bridge rows into source-truth prompts.
- Reran cached drill for Sourcetrail, CodeStory, and rootandruntime into
  `target/codestory-cross-repo-test/20260522-142000-packet6-final`; all three
  question searches report zero low-confidence Search Plan bridge rows, and
  rootandruntime `Posts` still reports 9 related Payload consumers.
- Implemented and verified Packet 7 canonical `drill-suite` command. The
  release CLI derives the fixed cross-repo matrix from the CodeStory checkout,
  runs each repo drill, writes per-repo artifacts, and emits aggregate
  `suite-report.md` / `suite-report.json` files.
- Reran the cached suite into
  `target/codestory-cross-repo-test/20260522-144309-suite`; all three repos are
  degraded but unblocked, all anchors resolve, and source-truth checks remain
  explicit in the aggregate report.
- Implemented and verified Packet 8 real-repo golden coverage. The ignored
  `codestory_repo_e2e_stats` drill test now runs `drill-suite` once for the
  fixed three-repo matrix and asserts question search, per-anchor
  symbol/trail/snippet/explore artifacts, source-truth checks, non-blocked
  verdicts, and the key caller/consumer/text-hint signals.
- Ran
  `cargo test -p codestory-cli --test codestory_repo_e2e_stats real_repo_agent_grounding_drill_emits_verification_packets -- --ignored --nocapture`;
  it passed against the real sibling repos in 122.62s.
- Started Fresh Round 2 after closing the initial 12-gap checklist. The new
  accepted gap was explicit `drill-suite --cache-dir` isolation.
- Implemented and verified Packet 9 suite cache isolation. Explicit suite cache
  roots now become per-repo cache subdirectories, and `drill` avoids a duplicate
  summary pre-open before refresh.
- Reran a clean full-refresh explicit-cache suite into
  `target/codestory-cross-repo-test/20260522-150711-suite-cache2`; it passed and
  created separate `sourcetrail`, `codestory`, and `rootandruntime` cache dirs.
- Refreshed the live dashboard and verified the rendered page at
  `http://127.0.0.1:8787/` shows 2 plotted runs in the current segment, latest
  run `#11` at `0gaps`, and `0 open / 13 total` accepted gaps.
- Previewed `gap-candidates`; the helper reports 30 historical synthesis
  candidates and correctly requires a fresh research round before declaring the
  domain exhausted.
- Accepted Fresh Round 3 bridge-to-verification gaps and logged the Round 3
  baseline as run 12 (`quality_gap=4`, `quality_total=17`).
- Implemented Packet 10 drill report UX improvements: source-truth checks now
  include caller/consumer/text-hint evidence and bridge endpoints/shared files,
  next commands include bridge-pair `--repo-text on` searches, and degraded
  verdicts name repo-specific evidence files.
- Verified Packet 10 with
  `cargo test -p codestory-cli --test search_json_output drill`,
  `cargo check -p codestory-cli`, `cargo build --release -p codestory-cli`, and
  cached `drill-suite --refresh none` into
  `target/codestory-cross-repo-test/20260522-153948-round3-packet13b`.
- Implemented Packet 11 bridge evidence classification. Bridge fallback now
  emits `evidence_hint_only` plus `evidence_files` when consumer/text hints
  exist but no graph path/shared file does.
- Verified Packet 11 with
  `cargo test -p codestory-cli drill_bridge_constructors_preserve_status_contract`,
  `cargo test -p codestory-cli --test search_json_output drill`,
  `cargo check -p codestory-cli`, `cargo build --release -p codestory-cli`, and
  cached `drill-suite --refresh none` into
  `target/codestory-cross-repo-test/20260522-155014-round3-packet14`.
- Logged Packet 11 as run 14 (`quality_gap=0`, `quality_total=17`,
  `quality_closed=17`) and refreshed both static and live dashboards. A live
  HTTP check of `http://127.0.0.1:8787/` returned 200 and contained run `#14`
  plus the zero-gap metric.
- Accepted two Fresh Round 4 gaps from subagent review: stale freshness is not
  visible enough in suite verdicts/follow-ups, and bridge `evidence_files` need
  production/runtime-first ranking.
- Logged Fresh Round 4 baseline as run 15 (`quality_gap=2`,
  `quality_total=19`, `quality_closed=17`).
- Implemented Packet 12 freshness and evidence-ranking UX fixes. Drill and
  suite summaries now surface freshness, stale runs get refresh-first next
  actions and incremental follow-up commands, full markdown reports render
  `evidence_files`, shared-file bridges preserve consumer/text hint files, and
  bridge evidence files rank runtime/source paths ahead of auxiliary files.
- Verified Packet 12 with `cargo test -p codestory-cli drill_`,
  `cargo check -p codestory-cli`, `cargo build --release -p codestory-cli`, and
  cached `drill-suite --refresh none` into
  `target/codestory-cross-repo-test/20260522-161057-round4-packet16`.
- Accepted Fresh Round 5 handoff-surface parity gaps from fresh suite/subagent
  review: source-truth ordering, consumer ordering, bridge endpoint files,
  suite-level retrieval visibility, and repo-local skill docs.
- Implemented Packet 18 UX parity fixes. Source-truth/claim-ledger files now
  rank runtime/source files before auxiliary files, consumer summaries rank
  public/runtime consumers first, bridge rows expose `endpoint_files`, suite
  markdown shows retrieval state, and the codestory-grounding skill docs cover
  the new drill/drill-suite contracts.
- Verified Packet 18 with `cargo test -p codestory-cli drill_`,
  `cargo check -p codestory-cli`, `cargo build --release -p codestory-cli`, and
  full-refresh `drill-suite` into
  `target/codestory-cross-repo-test/20260522-163139-round5-packet18`.
- Accepted Fresh Round 6 related target-truth gap: related Payload collection
  consumer rows could still point `target_file_path` at a synthetic
  usage/import script instead of the selected collection source file.
- Implemented Packet 19 related Payload target truth. Related consumer targets
  now carry the selected anchor's preferred collection source file, so
  rootandruntime `Posts` consumer rows target `src/collections/Posts.ts`.
- Verified Packet 19 with `cargo test -p codestory-cli drill_`,
  `cargo check -p codestory-cli`, `cargo build --release -p codestory-cli`, and
  full-refresh `drill-suite` into
  `target/codestory-cross-repo-test/20260522-164127-round6-packet19`.
- Accepted Fresh Round 7 trail layout suppression gap from subagent review:
  `trail --hide-speculative` could filter suppressed runtime bridge edges from
  `response.edges` while leaving them visible in `canonical_layout.edges`.
- Implemented Packet 20 layout suppression fix. `hide_speculative_trail_edges`
  now filters canonical layout edges by retained `source_edge_ids`, matching the
  graph edge contract even when suppressed edge endpoints remain reachable.
- Verified Packet 20 with
  `cargo test -p codestory-runtime graph_builders -- --nocapture`,
  `cargo check -p codestory-cli -p codestory-runtime -p codestory-indexer`,
  `cargo test -p codestory-cli drill_`,
  `cargo build --release -p codestory-cli`, and full-refresh `drill-suite` into
  `target/codestory-cross-repo-test/20260522-165409-round7-packet20`.
- Accepted Fresh Round 8 broad-question anchor-planning gap: Sourcetrail's real
  question explicitly named anchors but started from weak semantic suggestions
  with no Search Plan.
- Implemented Packet 21 broad-question planning fixes. Explain-how architecture
  questions now trigger planning, compound named anchors are ranked ahead of
  project/filler terms, and per-anchor typed subqueries are emitted.
- Verified Packet 21 with
  `cargo test -p codestory-runtime sourcetrail_agent_question_prioritizes_named_anchor_subquery_terms -- --nocapture`,
  `cargo test -p codestory-runtime search_plan -- --nocapture`,
  `cargo check -p codestory-runtime -p codestory-cli`,
  `cargo build --release -p codestory-cli`,
  `cargo test -p codestory-cli drill_`, and full-refresh `drill-suite` into
  `target/codestory-cross-repo-test/20260522-171439-round8-packet21`.
- Accepted Fresh Round 9 progress visibility gap: long `drill-suite` runs had
  no live progress output, so the agent/user could not tell whether the suite was
  alive during indexing, per-repo drill work, or report writing.
- Implemented Packet 22 progress visibility. `drill-suite` now emits start,
  per-repo start/done, report-writing, and final summary progress to stderr
  while preserving stdout for structured JSON output.
- Verified Packet 22 with
  `cargo test -p codestory-cli drill_suite_progress_messages_include_repo_index_and_verdict -- --nocapture`,
  `cargo test -p codestory-cli drill_suite`, `cargo check -p codestory-cli`,
  `cargo build --release -p codestory-cli`, and smoke `drill-suite --refresh
  none` into
  `target/codestory-cross-repo-test/20260522-172548-round9-packet22-smoke`.
- Accepted Fresh Round 10 checklist verbosity gap after subagent counter-scout:
  bridge graph-path extraction remains a later research lane, while source-truth
  verification was currently making agents inspect 25/28/25 checks for only
  10/9/6 files.
- Implemented Packet 23 source-truth grouping. Checks are now compacted by file
  and preserve evidence roles in the reason text; suite markdown reports
  `source checks/files`.
- Verified Packet 23 with
  `cargo test -p codestory-cli source_truth_checks_group_repeated_files_without_dropping_roles -- --nocapture`,
  `cargo test -p codestory-cli drill_`, `cargo check -p codestory-cli`,
  `cargo build --release -p codestory-cli`, and cached `drill-suite --refresh
  none` into
  `target/codestory-cross-repo-test/20260522-175104-round10-packet23`.
- Corrected the measurement surface after Packet 23: `quality_gap=0` is now
  treated as closed-checklist state, while the dashboard-facing progress signal
  is `quality_closed` with `quality_total`, `quality_newly_accepted`,
  `quality_newly_closed`, `quality_stagnating`, and `quality_plateau` as
  supporting metrics.
- Segment 5 is a metric-contract segment, not another product iteration. It
  should explain that packets 18-23 changed the product while the old open-gap
  metric stayed flat because each fresh accepted gap was closed before logging.
- Logged Segment 5 run 24 as a measurement-only baseline:
  `quality_closed=29`, `quality_total=29`, `quality_gap=0`,
  `quality_newly_accepted=0`, and `quality_stagnating=1`.
- Accepted Fresh Round 11 bridge-visibility gap as Segment 5 run 25 before
  fixing it: the fresh full suite was degraded because every repo had
  `graph_path=0`, `partial=3`, and `unresolved_or_error=0`, but the suite
  markdown rendered that as `3 total, 0 unresolved/error`.
- Implemented Packet 26 suite bridge-label visibility. The suite markdown now
  renders bridge quality as `graph / partial / unresolved-error` counts instead
  of hiding hint-only bridges behind a total.
- Verified Packet 26 with
  `cargo test -p codestory-cli drill_suite_markdown_summarizes_verdicts_and_source_truth`,
  `cargo build --release -p codestory-cli`, and cached `drill-suite --refresh
  none` into
  `target/codestory-cross-repo-test/20260522-183036-segment5-bridge-label`.
  The report shows all three repos as `0 graph / 3 partial / 0
  unresolved-error`.
- Ran a fresh full scout suite into
  `target/codestory-cross-repo-test/20260522-183954-fresh-scout`. It stayed
  mechanically healthy (`fresh`, `hybrid-ready`, all anchors resolved), but all
  repos remained degraded with `graph=0`, `partial=3`, `unresolved=0`.
- Accepted Segment 6 reporting/metric gaps: verdict reasons hid
  graph/partial bridge counts, `quality_newly_closed` could be inflated by old
  Segment 5 records, source target counts looked like completed verification,
  and `needs_verification_count=0` hid required source-truth checks.
- Implemented Packet 28 report clarity. The suite markdown now says `source
  targets/files`; summary JSON exposes pending/verified source-truth and claim
  counts; degraded verdict reasons include graph/partial/unresolved bridge
  counts plus pending source-truth checks; `autoresearch.ps1` falls back to the
  Segment 5 primary metric when old records omit `metrics.quality_closed`.
- Verified Packet 28 with `cargo test -p codestory-cli drill_`,
  `cargo build --release -p codestory-cli`, and cached `drill-suite --refresh
  none` into
  `target/codestory-cross-repo-test/20260522-185830-segment6-report-clarity`.
- Accepted Fresh Round 15 ID-stable handoff gap from subagent review:
  CodeStory had selected concrete anchors with `typed_hit_count=10`, but drill
  next commands still re-used ambiguous `--query` text for follow-ups.
- Implemented Packet 30 ID-stable follow-ups. Drill summary anchor statuses now
  include selected node id/ref/kind/file/line, and next commands use `--id` for
  selected-anchor `symbol`, function-body `snippet`, and bridge `trail`
  follow-ups while retaining query search commands for rediscovery.
- Verified Packet 30 with `cargo test -p codestory-cli drill_`,
  `cargo build --release -p codestory-cli`, and cached `drill-suite --refresh
  none` into
  `target/codestory-cross-repo-test/20260522-191603-segment7-id-handoff`.
- Accepted Fresh Round 16 pending-claim scoring/scope gaps: claim ledgers
  rendered all-zero scoring while classifications were pending, and degraded
  next actions named only the 3 bridge claims while each report had 6 pending
  claims.
- Implemented Packet 32 pending claim clarity. Claim-ledger scoring now carries
  `status=pending_source_verification` and `pending_claim_count`, markdown
  renders `score_status=pending_source_verification pending=6`, and degraded
  next actions tell the operator to verify all pending claims while starting
  with degraded bridges.
- Verified Packet 32 with `cargo fmt`, `cargo test -p codestory-cli drill_`,
  `cargo build --release -p codestory-cli`, and cached `drill-suite --refresh
  none` into
  `target/codestory-cross-repo-test/20260522-192649-segment7-claim-scope`.
- Ran a fresh full post-Packet-32 drill-suite into
  `target/codestory-cross-repo-test/20260522-194211-fresh-after-run32`.
  The run is fresh and all anchors resolve, but all three repos remain
  degraded with `0 graph / 3 partial / 0 unresolved-error`, symbolic-only
  retrieval, and pending source-truth verification.
- Accepted Fresh Round 17 source-verification handoff gaps before fixing them:
  bridge artifacts still have empty bridge-specific follow-up commands,
  aggregate `source targets/files` reads like completed coverage even with
  `0` verified checks, pending claim text still says CodeStory evidence is
  sufficient, and the shared missing semantic runtime is not promoted as a
  suite-level blocker.
- Closed Fresh Round 17 in Packet 34. Verified with `cargo fmt`,
  `cargo test -p codestory-cli drill_`, `cargo build --release -p
  codestory-cli`, and a fresh full suite at
  `target/codestory-cross-repo-test/20260522-200425-round17-handoff`.
- Packet 34 output now gives the dashboard/report an actual progress signal:
  bridge artifacts carry concrete follow-up commands, suite rows render
  target/verified/pending source-truth state, pending claim text stays honest,
  and the aggregate report surfaces the shared missing embedding runtime.
- Ran a Fresh Round 18 semantic precondition check. `setup embeddings` installed
  managed ONNX assets and default-cache `doctor` reported hybrid retrieval ready,
  but isolated `drill-suite --cache-dir` still fell back to
  `MissingEmbeddingRuntime` in
  `target/codestory-cross-repo-test/20260522-202502-semantic-scout`.
- Accepted Fresh Round 18 gaps before fixing them: explicit cache dirs should
  reuse global managed embeddings when local isolated assets are absent, and the
  newest drill/report handoff behavior needs deterministic regression coverage.
- Closed Fresh Round 18 in Packet 36. The runtime now selects global managed
  embedding assets for explicit-cache runs when the isolated cache lacks local
  assets, while keeping index storage isolated. Regression coverage now guards
  managed fallback, bridge-local follow-up commands, suite retrieval blocker
  rendering, source-verification-safe claim text, and pending source-truth
  counts.
- Verified Packet 36 with `cargo fmt`,
  `cargo test -p codestory-cli managed_embeddings`,
  `cargo test -p codestory-cli drill_`,
  `cargo test -p codestory-cli --test codestory_repo_e2e_stats --no-run`,
  `cargo build --release -p codestory-cli`, and a fresh isolated full suite at
  `target/codestory-cross-repo-test/20260522-203922-semantic-fallback`.
  The suite reports `hybrid:semantic_ready` for Sourcetrail, CodeStory, and
  rootandruntime, with no retrieval blockers.
- Ran a Fresh Round 19 plateau scout. Source-truth UX review found no new
  high-confidence gap beyond the known partial bridge/pending source-truth
  state. Bridge artifact review found a narrow payload clarity gap:
  `forward_no_path` graph payloads can still show `truncated=true` with zero
  edges and zero omitted edges. Coverage review found a deterministic test gap
  around repo-configured `.codestory.toml cache_dir` and managed executable
  asset root selection.
- Accepted the two Fresh Round 19 gaps before fixing them.
- Closed Fresh Round 19 in Packet 38. No-path bridge graph payloads now clear
  `truncated` when there are zero edges and zero omitted edges, while retaining
  truncation when omitted edges exist. Repo-configured `.codestory.toml
  cache_dir` now controls storage only; managed executable asset roots are
  selected from explicit CLI cache overrides or the global managed root.
- Verified Packet 38 with `cargo fmt`, focused no-path graph coverage,
  `cargo test -p codestory-cli managed_embeddings`,
  `cargo test -p codestory-cli
  runtime::tests::project_config_cache_dir_does_not_select_managed_executable_root`,
  `cargo test -p codestory-cli drill_`,
  `cargo test -p codestory-cli --test cli_golden_path`,
  `cargo build --release -p codestory-cli`, `autoresearch.checks.ps1`, and a
  cached CodeStory drill spot check at
  `target/codestory-cross-repo-test/20260522-211200-round19-spot`.
- Logged Packet 39 as a measurement-only plateau marker after Round 19 closure:
  `quality_closed=45`, `quality_total=45`, `quality_gap=0`,
  `quality_newly_accepted=0`, `quality_newly_closed=0`,
  `quality_stagnating=1`, and `quality_plateau=1`. No more product iteration
  should run unless a fresh candidate is logged open before it is fixed.
- Ran a fresh post-plateau full drill-suite at
  `target/codestory-cross-repo-test/20260522-211810-fresh-after-run39`.
  The suite is fresh and hybrid-ready for all three repos, with the familiar
  degraded bridge state (`0 graph / 3 partial / 0 unresolved-error`) and
  pending source-truth checks.
- Accepted Fresh Round 20 in Packet 40 before fixing it:
  `quality_closed=45`, `quality_total=50`, `quality_gap=5`,
  `quality_newly_accepted=5`, `quality_newly_closed=0`,
  `quality_stagnating=0`, and `quality_plateau=0`. The accepted gaps are
  suite-level artifact handoff, stale active resume docs, stale-refresh
  follow-up coverage, broad-question Search Plan coverage, and compact
  bridge-status coverage.
- Closed Fresh Round 20 in Packet 41. Suite markdown now has a `reports`
  column plus a `## Repo Artifacts` section with per-repo report and bridge
  artifact paths; active Autoresearch docs now lead with the current Round 20
  / post-plateau guard state; stale drill coverage forbids `--refresh none`;
  real-repo coverage protects broad-question named-anchor Search Plan subqueries
  and compact bridge status handoff rows.
- Verified Packet 41 with `cargo fmt`, focused runtime Search Plan tests,
  focused CLI stale-refresh and suite-markdown tests,
  `cargo test -p codestory-cli --test codestory_repo_e2e_stats --no-run`,
  `cargo build --release -p codestory-cli`, and cached drill-suite
  `target/codestory-cross-repo-test/20260522-214000-round20-final`.
- Started Segment 10 for the post-Round-20 plateau measurement after the
  Segment 9 iteration cap and contract drift guard. Logged Packet 42 as a
  measurement-only plateau:
  `quality_closed=50`, `quality_total=50`, `quality_gap=0`,
  `quality_newly_accepted=0`, `quality_newly_closed=0`,
  `quality_stagnating=1`, and `quality_plateau=1`.
- Ran a fresh stagnation audit. Fresh full suite
  `target/codestory-cross-repo-test/20260522-214838-final-stagnation-audit`
  preserved the current suite handoff behavior: fresh/hybrid-ready repos,
  per-repo report links, bridge artifact globs, compact bridge statuses, and
  pending source-truth wording. Two independent code/test scouts found no new
  non-duplicate product gap, but the docs handoff scout found one fresh gap:
  the `## blockers` section still told the next packet to close Fresh Round 20
  even though Round 20 was already closed and run 42 was the plateau marker.
- Accepted Fresh Round 21 in Packet 43 before fixing it:
  `quality_closed=50`, `quality_total=51`, `quality_gap=1`,
  `quality_newly_accepted=1`, `quality_newly_closed=0`,
  `quality_stagnating=0`, and `quality_plateau=0`.
- Closed Fresh Round 21 in Packet 44 by replacing the stale Round 20 blocker
  handoff with the current plateau guard:
  `quality_closed=51`, `quality_total=51`, `quality_gap=0`,
  `quality_newly_accepted=0`, `quality_newly_closed=1`,
  `quality_stagnating=0`, and `quality_plateau=0`.
- Logged Packet 45 as the post-Round-21 measurement-only plateau:
  `quality_closed=51`, `quality_total=51`, `quality_gap=0`,
  `quality_newly_accepted=0`, `quality_newly_closed=0`,
  `quality_stagnating=1`, and `quality_plateau=1`. Product iteration stops
  here unless a fresh candidate is logged open first or the next work is a
  promotion / holdout gate.

## blockers
- No current blocker. Product iteration should stop while `quality_gap=0`
  unless a fresh source-backed candidate is logged open before it is fixed, or
  the work is a promotion / holdout gate.
