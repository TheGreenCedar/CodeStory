# Natural-Language Search Requirements

These requirements define Search Quality 2.0 for broad architecture questions.
They are intentionally traceable to the design, task list, and validation
matrix in this directory.

| ID | Requirement | Acceptance Criteria |
| --- | --- | --- |
| NLS-REQ-1 | Preserve exact-symbol behavior. | Queries for exact identifiers such as `WorkspaceIndexer`, `SearchService`, `TrailResult`, `SourceGroupCxxCdb`, and `getCommentAuth` still return the exact typed symbol ahead of semantic or repo-text suggestions. |
| NLS-REQ-2 | Classify broad architecture queries. | Runtime detects broad natural-language architecture intent and emits a query assessment with intents, extracted terms, dropped terms, and planner eligibility. |
| NLS-REQ-3 | Decompose broad queries into bounded subqueries. | Eligible broad queries produce 3 to 8 subqueries covering symbols, nouns, boundaries, verbs, and relationship terms. The subqueries are visible in JSON and `--why` output. |
| NLS-REQ-4 | Collect separate candidate windows before truncation. | Typed-symbol, lexical/text, semantic, repo-text, and bridge candidates stay separately labeled until anchor grouping. Each window reports limits and truncation. |
| NLS-REQ-5 | Promote repo-text leads to typed anchors only when supported. | Repo-text hits are low-confidence leads unless the planner can bind them to a concrete indexed symbol by exact identifier, nearest symbol, or file-qualified retry. Unpromoted leads require source reads. |
| NLS-REQ-6 | Rank by multi-term, co-location, and source role. | Candidates matching multiple query concepts, anchors appearing in the same production file/subsystem, exact declarations, and structural entry points outrank single generic-term hits. |
| NLS-REQ-7 | Preserve semantic retrieval as one signal, not the whole answer. | Semantic suggestions can contribute to candidate windows, but cannot erase exact-symbol, lexical, path, or graph reasons. Hybrid score breakdown remains explainable. |
| NLS-REQ-8 | Add bridge-aware evidence planning. | Selected anchor groups include forward graph paths when available, reverse paths marked as directional risk, shared-file bridges marked low confidence, and isolated anchors flagged as unsupported for flow claims. |
| NLS-REQ-9 | Keep command boundaries explicit. | `search` returns discovery plans and hits; `context`/`explore` build target-first packets; `drill` composes question search, anchors, bridge evidence, claim ledger, and source-truth checklist. |
| NLS-REQ-10 | Expose agent-usable output. | Markdown and JSON include selected anchors, rejected/weak hits, promotion status, bridge status, score reasons, next CodeStory commands, and source-truth checks. |
| NLS-REQ-11 | Validate with deterministic quality gates. | Fixtures cover exact symbols, broad architecture flows, route/page queries, comments/auth/feed surfaces, negative noisy queries, bridge completeness, MRR, latency, and real-repo drill outputs. |
| NLS-REQ-12 | Bound cost and latency. | Planner work expands only after broad-query eligibility, bridge traversal runs only after anchor grouping, and eval fixtures enforce max-latency thresholds before promotion. |

## Non-Requirements

- CodeStory does not need to generate final answer prose from `search`.
- CodeStory does not need to make semantic retrieval mandatory for correctness.
- CodeStory does not need to copy CodeGraph's exact stop-word list, query
  weights, or agent instructions.
- CodeStory does not remove the source-truth verification phase from the drill.
