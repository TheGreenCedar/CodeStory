# Natural-Language Search Tasks

These tasks are ordered for execution, not scoped as optional slices. The whole
plan is required to close the natural-language search gap.

## 1. Establish Guardrails

- Add exact-symbol regression cases for the drill anchors:
  `WorkspaceIndexer`, `SearchService`, `TrailResult`, `SourceGroupCxxCdb`,
  `IndexerJava`, `StorageAccess`, `Posts`, `getElsewhereFeed`, and
  `getCommentAuth`.
- Add negative/noisy queries that must not produce exact anchors.
- Record baseline MRR, recall, latency, anchor buckets, and top-hit reasons.

Requirements: NLS-REQ-1, NLS-REQ-11, NLS-REQ-12.

## 2. Implement Query Term Extraction

- Add runtime query-term extraction for identifiers, compounds, CamelCase,
  snake_case, dotted names, meaningful nouns, relation verbs, and dropped terms.
- Cover stop-word, stemming/variant, and compound preservation behavior with
  unit tests.
- Expose extracted and dropped terms in JSON and `--why`.

Requirements: NLS-REQ-2, NLS-REQ-3, NLS-REQ-10.

## 3. Add The SearchPlan DTO

- Add `SearchPlanDto` and child DTOs in `codestory-contracts`.
- Thread optional search-plan data through runtime service responses.
- Keep existing search output backward-compatible for callers that ignore the
  plan.

Requirements: NLS-REQ-4, NLS-REQ-9, NLS-REQ-10.

## 4. Build The Planner

- Add `ArchitectureQueryPlanner` in runtime.
- Reuse existing architecture intent detection.
- Decompose broad questions into bounded subqueries.
- Mark exact-symbol queries as ineligible for broad planning unless the query
  also contains relationship/architecture terms.

Requirements: NLS-REQ-1, NLS-REQ-2, NLS-REQ-3, NLS-REQ-9.

## 5. Collect Candidate Windows

- Collect exact typed symbols, broader lexical/text hits, semantic suggestions,
  and repo-text leads independently.
- Preserve window limits, returned counts, truncation, and score reasons.
- Over-fetch enough candidates for multi-term reranking before final truncation.

Requirements: NLS-REQ-4, NLS-REQ-7, NLS-REQ-12.

## 6. Promote Repo-Text Leads

- Extract identifiers from repo-text excerpts.
- Resolve same-file identifiers to indexed symbols.
- Add file-qualified retries and nearest-symbol fallback where supported.
- Mark ambiguous or unpromoted leads as requiring source reads.

Requirements: NLS-REQ-5, NLS-REQ-10.

## 7. Rank Anchor Groups

- Group candidates by canonical symbol/path anchor.
- Add exact-name, typed-symbol, source-role, multi-term, co-location, and
  production-path ranking factors.
- Keep semantic score visible as one component.
- Record rejected or weakened hits.

Requirements: NLS-REQ-1, NLS-REQ-6, NLS-REQ-7, NLS-REQ-10.

## 8. Add Bridge Evidence

- Expand bounded graph neighborhoods only after anchor selection.
- Recover edges between selected nodes after node trimming.
- Label forward, reverse, shared-file, and isolated bridge states.
- Surface bridge confidence in search plan and drill output.

Requirements: NLS-REQ-8, NLS-REQ-9, NLS-REQ-12.

## 9. Wire CLI And Drill Output

- Render the search plan in Markdown when `--why` is set.
- Emit full plan JSON for `search --format json`.
- Let `drill --question` include suggested anchors and plan evidence while
  preserving partial-discovery labels.
- Add next commands and source-truth checks to the visible report.

Requirements: NLS-REQ-9, NLS-REQ-10.

## 10. Expand Quality Gates

- Add CodeGraph-inspired query classes: subsystem, symbol location,
  callers/usage, impact, interaction, flow, polymorphism, route/page, and bug
  investigation.
- Add the three drill questions as quality fixtures.
- Run real-repo drill after narrow tests pass.
- Reject regressions in exact anchor ranking, broad-query recall, bridge
  completeness, unsupported high-confidence claims, and latency.

Requirements: NLS-REQ-1, NLS-REQ-8, NLS-REQ-11, NLS-REQ-12.

## 11. Update Operator Guidance

- Update `.agents/skills/codestory-grounding/SKILL.md` if the command flow or
  drill interpretation changes.
- Update `docs/testing/search-quality-eval.md` with new metrics and thresholds.
- Update CLI subsystem docs if new output contract fields are promoted.

Requirements: NLS-REQ-9, NLS-REQ-10, NLS-REQ-11.
