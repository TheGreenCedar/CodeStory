# Research Synthesis: Reduce CodeStory real-repo agent drill friction across Sourcetrail, CodeStory, and rootandruntime. Track current-run quality gaps from source-verified drill evidence, implement improvements, rerun the drill, and repeat until gaps stop falling.

## Project Essence
- CodeStory is trying to become a trustworthy grounding surface for agents:
  not just an indexer, but a tool that helps an agent answer realistic
  codebase questions from evidence and then survive source-truth verification.

## High-Impact Findings
- Exact symbol search works well enough to start investigations in all three
  repos, but bridge claims are still fragile.
- Snippets are the highest-value evidence surface. The first patch moved drill
  and Search Plan handoffs to body-aware snippets, then tightened promotion so
  class/config anchors do not jump to unrelated functions.
- Trail output is useful for local structure. The first patch removed the unsafe
  global method-tail resolution class that produced the Sourcetrail Java/C++
  false edge.
- Active-path discovery is the largest product gap for web/app repos: exact
  definitions are not the same thing as runtime or render participation.
- The drill itself needs a compact machine-readable summary so iterations can
  compare quality without rereading every artifact.
- The second packet improved active-path ranking and uncertainty labeling:
  rootandruntime social/feed searches now prefer callable anchors with visible
  callers over a definition-only `getElsewhereFeed`, and trail story output
  warns when the focus has no visible incoming call edges.
- The second packet also added `drill-summary.json` for each repo drill, making
  mechanical status, anchor resolution, bridge gaps, source-truth prompts, and
  open-gap status easy to compare across iterations.
- The third packet added decision-grade verdicts and execution-boundary evidence:
  each real-repo drill summary now reports `degraded` explicitly, and drill
  reports identify CLI/runtime/store flows for `drill`, `trail`, and
  `search/snippet`.
- The third packet added bounded caller/consumer summaries. They are useful where
  graph evidence exists (`getCommentAuth` shows two source-verified callers), but
  they still do not explain Sourcetrail `StorageAccess` or rootandruntime Payload
  collection/config consumers, so the caller/consumer gap stays open.
- Trail JSON now suppresses self-edges and duplicate edge keys in the packet-3
  `getElsewhereFeed` artifact (`self_edges=0`, `duplicate_keys=0`).
- Packet 4 extends drill caller/consumer summaries beyond the literal selected
  anchor for Payload collections. In the real rootandruntime drill, `Posts` now
  reports 9 consumers through the related `framework::payload::collection::posts`
  node, including frontend comment route, admin widgets, content-data helpers,
  and migration/import scripts. Source grep confirms the named files contain
  `payload.find` or `req.payload.find` calls with `collection: "posts"`.
- The caller/consumer gap still remains open because this packet proves the
  Payload collection side, not the Sourcetrail `StorageAccess` side.
- Packet 5 closes the caller/consumer gap by adding an honest fallback for
  graph-empty anchors: `StorageAccess` still reports zero typed graph consumers,
  but now also reports 24 repo-text consumer hints with source files and lines.
  This gives the agent source-truth pointers without pretending text hits are
  typed graph edges.
- Packet 6 closes the remaining trail-noise gap for the current drill: strict
  trail filtering now hides weak runtime bridge edges while preserving probable
  non-call data/config usage edges, and broad Search Plan output suppresses
  low-confidence bridge rows into explicit source-truth prompts. The final
  cached all-repo drill has `low_search_plan_bridges=0` for Sourcetrail,
  CodeStory, and rootandruntime; CodeStory and rootandruntime each carry one
  suppression prompt, and rootandruntime `Posts` still reports 9 Payload
  collection consumers.
- Packet 7 adds a canonical one-command runner: `codestory-cli drill-suite`.
  It derives the three fixed real-repo drill cases from the CodeStory checkout,
  runs each repo drill, writes per-repo artifacts, and emits an aggregate
  suite report with verdicts, anchor counts, bridge counts, source-truth checks,
  and next actions.
- Packet 8 closes the regression-coverage gap. The ignored real-repo e2e drill
  harness now runs `drill-suite` once for the fixed three-repo matrix and checks
  question search, anchor order, source-truth checks, symbol/trail/snippet/explore
  artifacts, non-blocked verdicts, and the specific consumer/text-hint signals
  that made the agent answers usable. The real sibling-repo test passed in
  122.62s.
- Fresh Round 2 found and closed a suite-isolation gap: `drill-suite` accepted
  `--cache-dir` through shared project args but did not propagate it to per-repo
  drills. Packet 9 now maps an explicit suite cache root to per-repo sub-caches,
  and removes the drill double pre-open before refresh that exposed a Windows
  search-writer permission failure on a fresh isolated cache.
- Fresh Round 3 discovery from the latest explicit-cache suite found a new
  agent-UX gap cluster: the suite is mechanically healthy and resolves every
  seed anchor, but every repo is still degraded because bridge evidence is
  `0/3` and source-truth is required. The reports already contain useful
  consumer/text evidence, but it is not promoted into bridge follow-ups,
  source-truth checks, or repo-specific next actions.
- Packet 10 closes most of that Round 3 agent-UX gap without pretending the
  graph is better than it is: consumer/text evidence is promoted into required
  source-truth checks, bridge-pair repo-text follow-up searches are emitted, and
  degraded next actions name repo-specific verification files.
- Packet 11 closes the remaining Round 3 bridge-row gap: bridge evidence now
  emits `evidence_hint_only` plus evidence files when consumer/text hints exist
  but no graph path or shared file does. The cached suite still reports
  `graph_path=0`, but all three repos now report `partial=3` and
  `unresolved_or_error=0`.
- Fresh Round 4 found two new agent-UX issues in the Packet 11 artifacts. First,
  freshness is visible in detailed search artifacts but not in drill-suite
  verdicts or follow-up commands, so a stale `--refresh none` run can look
  healthier than it is. Second, `evidence_hint_only` bridge files are useful but
  need role-aware ordering so production/runtime boundary files come before
  benches, tests, scripts, and migrations.
- Packet 12 closes the Round 4 gaps. Drill summaries now include compact
  freshness status/count/samples, stale freshness changes the verdict next
  action to refresh-first guidance, generated follow-up commands use
  incremental refresh when the index is stale, full markdown reports render
  `evidence_files`, and bridge evidence files are ranked runtime/source before
  auxiliary test/bench/script files.
- Fresh Round 5 found parity gaps after a full fresh suite. The suite was
  mechanically clean and fresh, but source-truth/claim-ledger lists could still
  reintroduce auxiliary-file noise, anchor consumer examples could bury public
  runtime consumers behind scripts, bridge rows could require looking elsewhere
  for endpoint definition files, suite markdown hid symbolic-only retrieval,
  and repo-local skill docs lagged the new drill/drill-suite contracts.
- Packet 18 closes those Round 5 handoff gaps. Source-truth and claim-ledger
  files now use the same runtime/source-first ranking as bridge evidence;
  consumer summaries rank public/runtime files before scripts/tests/benches;
  bridge rows emit `endpoint_files`; suite markdown shows retrieval as
  `symbolic-only` or hybrid-ready; and the repo-local grounding skill/reference
  docs cover `drill-suite`, `drill-summary.json`, endpoint files, and consumer
  summaries.
- Fresh Round 6 found one target-truth gap after Packet 18. Related Payload
  collection consumer rows were ordered well, but `target_file_path` could still
  point at an import script for the synthetic related collection target instead
  of the selected collection source file.
- Packet 19 closes the Round 6 gap by carrying the preferred collection source
  file into related Payload consumer targets. The full-refresh suite at
  `20260522-164127-round6-packet19` shows rootandruntime `Posts` consumer rows
  now target `src/collections/Posts.ts`, including the public comments route
  consumer.
- Fresh Round 7 found a P1 consistency bug in `trail --hide-speculative`.
  `response.edges` used the stricter `is_speculative_trail_edge` predicate, but
  `canonical_layout.edges` only checked speculative certainty labels and
  reachability, so probable or low-confidence runtime bridge edges could remain
  visible through the layout even after being hidden from the main edge list.
- Packet 20 closes the Round 7 layout suppression gap. Canonical layout edges
  now must reference retained `source_edge_ids`, and the regression keeps a
  suppressed probable edge out of `canonical_layout.edges` even when its
  endpoints remain reachable through other retained edges.
- Fresh Round 8 found that Sourcetrail's broad real-agent question still had a
  weak starting surface. The question explicitly named `SourceGroupCxxCdb`,
  `IndexerJava`, and `StorageAccess`, but `question-search.json` reported no
  Search Plan, `exact_symbol_hit_count=0`, `weak_top_hit=true`, and a generic
  repo-text rerun recommendation.
- Packet 21 closes the broad-question anchor-planning gap. Explain-how
  architecture questions are now plan-eligible, compound named anchors are
  ranked ahead of project/filler terms, and per-anchor typed subqueries are
  emitted. Fresh suite `20260522-171439-round8-packet21` shows Sourcetrail
  named-anchor subqueries for all three requested anchors and anchor groups
  containing `SourceGroupCxxCdb`, `IndexerJava`, and `StorageAccess`.

## Quality-Gap Translation
- `quality-gaps.md` started with 12 open current-run gaps, added 1 fresh
  suite-isolation gap in Round 2, closed all 13, then added 4 Fresh Round 3
  bridge-to-verification usability gaps after inspecting the latest explicit
  cache suite. Packet 10 closed 3 of those 4, and Packet 11 closes the final
  one. Fresh Round 4 added 2 new accepted gaps, and Packet 12 closes them.
  Fresh Round 5 added 5 handoff-surface parity gaps, and Packet 18 closes them.
  Fresh Round 6 added 1 related target-truth gap, and Packet 19 closes it.
  Fresh Round 7 added 1 trail layout suppression gap, and Packet 20 closes it,
  Fresh Round 8 added 1 broad-question anchor-planning gap, and Packet 21 closes
  it. Fresh Round 9 added 1 progress-visibility gap and Packet 22 closes it.
  Fresh Round 10 added 1 checklist-compression gap and Packet 23 closes it.
  The checklist state is now 0 open of 29 accepted gaps.
- Closed items cover
  closing the false-edge class, body-aware evidence path, active-path ranking,
  no-caller labeling, claim/source-truth prompts, compact drill summaries,
  execution-boundary evidence, ready/degraded/blocked verdict reporting, and
  caller/consumer summaries for both Payload collection and graph-empty storage
  anchors, trail/Search Plan low-confidence bridge suppression, and the
  canonical drill-suite command/report path, plus real-repo golden regression
  coverage for the fixed questions, explicit suite cache isolation, the
  Round 3 bridge-to-verification UX gaps, Round 4 freshness/evidence ranking
  gaps, and Round 5 source-truth ordering, consumer ordering, endpoint-file,
  retrieval visibility, skill-doc parity gaps, the Round 6 related collection
  target-truth gap, and the Round 7 `hide-speculative` canonical-layout
  suppression gap, plus the Round 8 broad-question named-anchor planning gap.
- No accepted Fresh Round 10 quality gaps remain open. A new round should start
  from fresh evidence instead of reusing historical synthesis candidates.

## Segment 5 Metric Interpretation
- Segment 5 must not use flat `quality_gap=0` as proof that continued packets
  are useful. `quality_gap` is only the current open accepted-gap count.
- Packets 18-23 still changed the product while the old chart was flat because
  each packet accepted and closed a fresh source-backed gap before logging:
  `quality_total` and `quality_closed` increased from 24 to 29 while
  `quality_gap` stayed at 0.
- The dashboard-facing progress signal should therefore focus on
  `quality_closed / quality_total`, per-packet `quality_newly_accepted` and
  `quality_newly_closed`, and the `quality_stagnating` / `quality_plateau`
  indicator.
- With `quality_gap=0` and no newly accepted candidate, the correct next action
  is to stop product iteration and either run a promotion/holdout gate or open a
  fresh source-backed candidate before fixing it.

## Confidence And Gaps
- High confidence: the baseline report is source-verified and identifies real
  agent-UX failures.
- High confidence: the false-edge class caught by the Sourcetrail drill is fixed
  by refusing unsafe owner-qualified method-tail global resolution.
- High confidence: drill/search-plan body-aware snippets now preserve selected
  anchors and can include decisive route operations.
- High confidence: active-path search ranking and no-visible-caller labels are
  visible in the packet-2 real-repo probe artifacts.
- High confidence: compact drill summaries exist for Sourcetrail, CodeStory, and
  rootandruntime in the packet-2 probe output.
- High confidence: packet-3 drill artifacts expose execution boundaries and
  explicit degraded verdicts for all three repos.
- High confidence: caller/consumer summaries now expose useful evidence for
  rootandruntime Payload collection config anchors and Sourcetrail graph-empty
  storage anchors. The StorageAccess hints are repo-text/source-truth pointers,
  not typed graph edges.
- High confidence: Packet 6 suppresses low-confidence Search Plan bridge rows in
  the final cached all-repo drill while preserving the related Payload consumer
  graph evidence that Packet 4 introduced.
- High confidence: Packet 7 provides the requested one-command drill/report
  path. The cached real-repo suite run produced three degraded-but-unblocked
  repo verdicts with all anchors resolved and source-truth checks explicit.
- High confidence: Packet 8 protects the current agent-UX improvements with an
  ignored real-repo gate. It passed against Sourcetrail, CodeStory, and
  rootandruntime using the release binary.
- High confidence: Packet 9 fixed and verified explicit cache isolation for
  `drill-suite`; the clean full-refresh suite run produced separate cache dirs
  for all three target repos and preserved all anchor/source-truth summaries.
- High confidence: Fresh Round 3 gaps are real product friction. The latest
  suite reports all anchors resolved and all indexes healthy, while all three
  repos remain degraded because every bridge pair is unresolved and the next
  actions/checklists are too generic for the asked architecture flows.
- High confidence: Packet 10 improves the agent-facing degraded path. The
  cached suite rerun at `20260522-153948-round3-packet13b` reports expanded
  source-truth checks and repo-specific next actions while preserving the honest
  degraded verdicts.
- High confidence: Packet 11 makes bridge rows actionable without overclaiming.
  The cached suite rerun at `20260522-155014-round3-packet14` reports
  `evidence_hint_only` for every pair, `partial=3`, and
  `unresolved_or_error=0` in all three target repos.
- High confidence: Fresh Round 4 gaps are real and not historical repeats.
  Packet 11 artifacts show stale CodeStory freshness in detailed search output
  but not the aggregate verdict, and bridge hint files include non-production
  files before stronger runtime boundaries.
- High confidence: Packet 12 fixes the Round 4 gaps. The cached suite rerun at
  `20260522-161057-round4-packet16` shows the CodeStory aggregate row as stale
  with refresh-first guidance, and bridge evidence files put runtime/source
  paths before benches/scripts.
- High confidence: Fresh Round 5 gaps were current and not historical repeats.
  Full-refresh suite `20260522-162027-fresh-iteration` was fresh and
  mechanically clean, but still showed auxiliary-file ordering, consumer
  ordering, endpoint-file, and suite-level retrieval visibility friction.
- High confidence: Packet 18 fixes the Round 5 handoff gaps. Full-refresh suite
  `20260522-163139-round5-packet18` shows source-truth lists with runtime/source
  files first, rootandruntime `Posts` consumers led by the public comments
  route, bridge `endpoint_files` for `Posts -> getElsewhereFeed`, and suite
  markdown `retrieval=symbolic-only`.
- High confidence: Fresh Round 6 gap was current and specific. Packet 18
  rootandruntime evidence still showed `related_payload_collection:posts`
  consumer rows with a script-like target path even when the consumer file was
  the public comments route.
- High confidence: Packet 19 fixes the Round 6 target-truth gap. Full-refresh
  suite `20260522-164127-round6-packet19` shows `Posts` consumer rows targeting
  `src/collections/Posts.ts`, and the focused drill tests/check/build passed.
- High confidence: Fresh Round 7 gap was current and independently found by
  subagent review. Source truth confirmed `canonical_layout.edges` did not use
  the same retained-edge contract as `response.edges`.
- High confidence: Packet 20 fixes the Round 7 layout suppression gap.
  `cargo test -p codestory-runtime graph_builders -- --nocapture`,
  `cargo test -p codestory-cli drill_`, multi-crate `cargo check`, release
  build, and full-refresh suite `20260522-165409-round7-packet20` all passed.
- High confidence: Fresh Round 8 gap was current. Packet 20 Sourcetrail
  question-search had no Search Plan despite explicit named anchors.
- High confidence: Packet 21 fixes the broad-question planning gap. The runtime
  Sourcetrail regression, search-plan tests, CLI drill tests, release build, and
  full-refresh suite `20260522-171439-round8-packet21` passed, and the fresh
  Sourcetrail question-search artifact contains per-anchor typed subqueries and
  anchor groups for the three requested anchors.
- High confidence: Fresh Round 9 progress visibility gap was current. Full
  drill-suite verification could be silent long enough that the agent/user had
  no live evidence of whether the suite was indexing, drilling, or writing
  reports.
- High confidence: Packet 22 fixes the progress visibility gap. Focused
  drill-suite tests, `cargo check -p codestory-cli`, release build, and the
  smoke run at `20260522-172548-round9-packet22-smoke` passed; captured stderr
  includes suite and per-repo progress while captured stdout remains valid JSON.
- High confidence: Fresh Round 10 gap was current. The latest suite had
  actionable target files, but source-truth verification was still noisy at
  25/28/25 checks for only 10/9/6 files.
- High confidence: Packet 23 fixes the checklist verbosity gap without hiding
  evidence roles. Focused grouping coverage, `cargo test -p codestory-cli
  drill_`, `cargo check -p codestory-cli`, release build, and suite rerun
  `20260522-175104-round10-packet23` passed; source-truth checks now equal
  target-file counts at 10/9/6 while role summaries are preserved in reasons.
- High confidence: Segment 5 fixed the misleading dashboard contract. Run 24
  switched the primary metric to `quality_closed`, run 25 logged a fresh accepted
  bridge-visibility gap before implementation, and Packet 26 closed it with
  `quality_closed` expected to advance from 29 to 30.
- High confidence: Packet 26 fixes the suite bridge visibility gap. The cached
  suite rerun at `20260522-183036-segment5-bridge-label` shows all three repos
  as `0 graph / 3 partial / 0 unresolved-error` in markdown, matching the JSON
  bridge counts and making the degraded verdict understandable.
- High confidence: Fresh Segment 6 scout found current reporting friction after
  the bridge-column fix. Full suite `20260522-183954-fresh-scout` was fresh and
  hybrid-ready with all anchors resolved, but reasons still hid
  `graph_path=0/partial=3`, source target counts could read as completed
  verification, and `needs_verification_count=0` hid pending source-truth work.
- High confidence: Packet 28 fixes the accepted reporting and metric gaps. The
  cached suite rerun at `20260522-185830-segment6-report-clarity` shows
  `source targets/files`, explicit pending/verified source-truth JSON counts,
  and degraded reasons such as `graph_bridges=0/3 partial_bridges=3
  unresolved_or_error_bridges=0 pending_source_truth_checks=10`.
- High confidence: Packet 30 fixes the ID-stable handoff gap. The cached suite
  rerun at `20260522-191603-segment7-id-handoff` exposes selected node metadata
  in `drill-summary.json` and emits `symbol`, function-body `snippet`, and
  bridge `trail` follow-ups with `--id` for selected anchors instead of
  re-querying ambiguous display names.
- High confidence: Packet 32 fixes pending claim scoring/scope wording. The
  cached suite rerun at `20260522-192649-segment7-claim-scope` renders
  `score_status=pending_source_verification pending=6` instead of zero scoring,
  and degraded next actions say to verify 6 pending claims starting with 3
  degraded bridges.
- High confidence: Packet 34 closes the Fresh Round 17 source-verification
  handoff gaps. Fresh full suite `20260522-200425-round17-handoff` keeps the
  three repos honestly degraded, but now shows why: all three are symbolic-only,
  all bridge links are partial, and all source-truth checks remain pending.
- High confidence: Packet 34 makes bridge degradation actionable. Every
  `evidence_hint_only` bridge artifact now carries bridge-local follow-up
  commands, and drill reports render those commands next to the degraded bridge
  evidence instead of only pointing at aggregate next actions.
- High confidence: Packet 34 fixes the verification truth language. Suite
  markdown renders source truth as target/verified/pending state, and claim
  ledgers describe candidate claims requiring source-truth verification instead
  of claiming CodeStory evidence is already sufficient.
- High confidence: Fresh Round 18 found an isolated-cache semantic setup gap.
  `setup embeddings` installed managed ONNX assets and default-cache doctor
  reports hybrid retrieval ready, but isolated `drill-suite --cache-dir` still
  reports `MissingEmbeddingRuntime` for all three repos because runtime looks
  under the isolated cache for managed assets.
- High confidence: Fresh Round 18 found regression coverage debt around the
  newest agent handoff contracts. The artifacts are correct, but deterministic
  tests do not yet protect bridge-local follow-up commands, retrieval blocker
  rendering, pending-safe claim wording, and real-repo pending source-truth
  counts.
- High confidence: Packet 36 fixes the isolated-cache semantic setup gap.
  Explicit-cache doctor now reports available managed assets with only
  `missing_semantic_docs` before indexing, and the fresh isolated full suite
  `20260522-203922-semantic-fallback` reports `hybrid:semantic_ready` for all
  three repos.
- High confidence: Packet 36 fixes the newest regression coverage debt.
  Managed embedding fallback, bridge-local follow-up commands, retrieval
  blocker rendering, pending-safe claim wording, and real-repo pending
  source-truth counts now have focused tests or compile-time ignored-test
  coverage.
- High confidence: Segment 5 looked flat at zero because its primary signal was
  open accepted gaps. Packets still changed the product by accepting fresh gaps
  before fixes, closing them, and increasing cumulative accepted closed gaps;
  the benchmark wrapper now emits `quality_closed` first while retaining open,
  total, newly accepted, newly closed, and plateau metrics. Run 39 confirms the
  stop condition after Packet 38: `quality_closed=45`, `quality_total=45`,
  `quality_gap=0`, `quality_newly_accepted=0`, `quality_newly_closed=0`,
  `quality_stagnating=1`, and `quality_plateau=1`.
- High confidence: Fresh Round 19 found no new source-truth UX gap, but did
  find two credible smaller gaps before implementation. First, `forward_no_path`
  bridge graph payloads can still report `truncated=true` with zero edges and
  zero omitted edges. Second, repo-configured `.codestory.toml cache_dir`
  behavior needs deterministic coverage so storage isolation cannot accidentally
  select repo-controlled managed executable assets or pass only because this
  workstation has global assets installed.
- High confidence: Packet 38 closes the Fresh Round 19 gaps. Representative
  CodeStory no-path bridge artifacts now render `truncated=false` when no edges
  were omitted, and repo-configured `cache_dir` is explicitly storage-only for
  managed asset root selection. Focused tests, drill tests, golden-path checks,
  release build, and a CodeStory drill spot check passed.
- High confidence: Fresh Round 20 found current post-plateau handoff and
  coverage friction, not another partial-bridge duplicate. Fresh suite
  `20260522-211810-fresh-after-run39` was fresh/hybrid-ready for the sibling
  repos and still degraded only because bridge/source-truth work is pending, but
  the aggregate report told agents to use emitted follow-up commands without
  linking the per-repo artifacts; active Autoresearch docs still led with stale
  baseline/Round 8 state; and tests did not guard stale `--refresh none`
  follow-ups, broad-question named anchors, or compact bridge status rows.
- High confidence: Packet 41 closes the Fresh Round 20 gaps. Cached suite
  `20260522-214000-round20-final` renders report/artifact links in the suite
  markdown, rootandruntime's `question-search.json` now includes `Posts` as a
  `named_anchor` subquery, and compact suite JSON bridge statuses are complete
  with `command_status=ok` for all three repos. Focused runtime/CLI tests,
  ignored real-repo harness compilation, release build, and cached drill-suite
  verification passed.
- High confidence: Segment 10 run 42 is a plateau marker after Packet 41, not
  another product improvement. It records `quality_closed=50`,
  `quality_gap=0`, `quality_newly_accepted=0`, `quality_newly_closed=0`,
  `quality_stagnating=1`, and `quality_plateau=1`; the next product packet
  should not run unless a fresh candidate is logged open first.
- High confidence: Fresh Round 21 was the only new accepted gap from the final
  stagnation audit, and it was logged open before the fix. Code/test scouts
  rejected the remaining candidates as duplicates or weak near-misses, while
  the docs scout found that `tasks.md` still told the next packet to close
  Fresh Round 20 from the blockers section even though all Round 20 gaps were
  closed. Packet 44 replaces that stale blocker with the plateau guard, moving
  `quality_closed` to 51 while returning `quality_gap` to 0.
- High confidence: Packet 45 is a measurement-only stop marker after Fresh
  Round 21 closure. It should be read as stagnation (`quality_stagnating=1`,
  `quality_plateau=1`) with 51 accepted gaps closed, not as proof that another
  flat-zero product iteration is useful.
- Lower confidence: full receiver/type-aware resolution remains a deeper design
  gap because the current unresolved-call row carries the terminal call name, not
  a receiver expression/type.
