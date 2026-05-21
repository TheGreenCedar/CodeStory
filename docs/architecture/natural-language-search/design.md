# Natural-Language Search Design

## Current Boundary

The current command model is sound and should stay in place:

- `codestory-cli` parses arguments and renders DTOs.
- `codestory-runtime` owns retrieval orchestration and ranking.
- `codestory-store` persists graph/search/snapshot state.
- `codestory-contracts` defines DTOs.
- `drill` composes evidence and verification structure, but does not silently
  treat broad search as final answer support.

Search Quality 2.0 changes the runtime search assembly and output contracts. It
does not move business logic into CLI rendering.

## Data Contracts

Add optional fields to the search DTO surface in
`crates/codestory-contracts/src/api/dto.rs`.

```rust
pub struct SearchPlanDto {
    pub original_query: String,
    pub eligible: bool,
    pub intents: Vec<SearchIntentDto>,
    pub terms: SearchTermsDto,
    pub subqueries: Vec<SearchSubqueryDto>,
    pub candidate_windows: Vec<SearchCandidateWindowDto>,
    pub anchor_groups: Vec<SearchAnchorGroupDto>,
    pub bridges: Vec<SearchBridgePlanDto>,
    pub rejected_hits: Vec<SearchRejectedHitDto>,
    pub next_commands: Vec<String>,
    pub source_truth_checks: Vec<String>,
}
```

The exact shape can evolve during implementation, but these concepts are part
of the contract:

- original query and eligibility
- visible extracted/dropped terms
- subqueries with intent and source channel
- candidate windows with source, limit, returned count, truncation, and score
  reasons
- anchor groups with chosen symbol, supporting hits, promotion status, and
  confidence
- bridges with direction, evidence kind, confidence, and truncation
- next commands and source-truth checks

## Query Assessment And Term Extraction

Implement a tested `QueryTermExtractor` in runtime, likely near
`crates/codestory-runtime/src/symbol_query.rs` or a sibling search module.

Inputs:

- raw query text
- field-qualified filters from the existing search request
- language/framework hints when already available from grounding state

Outputs:

- exact identifiers: quoted text, CamelCase, snake_case, SCREAMING_CASE,
  dotted names, and function-like names
- meaningful natural-language terms
- compound terms preserved before splitting
- relation verbs such as `calls`, `routes`, `persists`, `loads`, `indexes`,
  `renders`, `auth`, `feed`, and `handoff`
- dropped terms with reasons

CodeGraph's term extraction is the useful model: preserve identifiers, split
compound forms, filter common terms, generate variants, and make this behavior
testable. CodeStory should make dropped terms visible rather than hiding them in
static policy.

Requirements: NLS-REQ-2, NLS-REQ-3, NLS-REQ-10.

## Planner

Add an `ArchitectureQueryPlanner` in runtime search orchestration. The planner
runs only for broad-query eligible searches. It should reuse existing
architecture intent detection instead of introducing a second vocabulary.

Planner responsibilities:

- decompose one broad question into bounded subqueries
- assign each subquery to one or more channels: typed symbol, lexical/text,
  semantic, repo text
- identify expected anchor roles, such as entry point, store, route, feed,
  auth, indexer, runtime, command, or DTO
- keep exact-symbol queries on the fast path
- expose the plan even when no strong anchors are found

Requirements: NLS-REQ-1, NLS-REQ-2, NLS-REQ-3, NLS-REQ-9.

## Candidate Windows

Collect windows independently before ranking across channels:

- exact typed symbols
- broader indexed symbol/text matches
- semantic suggestions
- repo-text leads
- optional bridge-neighborhood hints after anchor selection

The key change is timing. CodeGraph reranks and merges before graph traversal;
CodeStory should do the same for broad queries. Do not let a single global
semantic top-N starve exact, lexical, or repo-text evidence.

Each window reports:

- channel
- subquery
- limit
- returned count
- truncation
- score components where available
- whether candidates are eligible for anchor promotion

Requirements: NLS-REQ-4, NLS-REQ-7, NLS-REQ-12.

## Repo-Text Promotion

Repo-text evidence is useful but should not be treated as an anchor without a
binding step.

Promotion order:

1. Extract exact identifiers from the matched line and nearby excerpt.
2. Resolve extracted identifiers against indexed symbols in the same file.
3. If needed, retry with file-qualified search terms such as `path:<file>
   name:<identifier>`.
4. Fall back to the nearest indexed symbol around the matched line when
   occurrence ranges make that possible.
5. Preserve unpromoted file/line evidence as `needs_source_read`.

Promotion must record how the binding happened. Ambiguous promotions stay weak
and should not unlock high-confidence flow claims.

Requirements: NLS-REQ-5, NLS-REQ-10.

## Reranking

Rank anchor groups, not raw hits. Suggested ranking factors:

- exact identifier match
- typed symbol availability
- declaration or structural node kind
- production-source path over test/fixture/generated paths
- multi-term coverage
- co-location with other distinctive query terms
- semantic score, when explainable
- source role alignment with the detected architecture intent

CodeGraph's strongest transferable pattern is multi-term reranking before
truncation: nodes matching several query concepts should beat nodes that only
match one generic word. Exact matches stay exempt from dampening.

Requirements: NLS-REQ-1, NLS-REQ-6, NLS-REQ-7.

## Bridge Evidence

After anchor grouping, run bounded graph expansion only for selected anchors.

Bridge confidence:

- forward graph path: high, or medium when truncated
- reverse graph path: medium, or low when truncated
- shared file/subsystem only: low
- isolated anchors: unsupported for flow claims

Recover edges between selected nodes after trimming so relationship evidence is
not lost by output budgets. The drill already has bridge concepts; the planner
should make similar bridge status visible earlier without replacing the drill's
claim ledger.

Requirements: NLS-REQ-8, NLS-REQ-9, NLS-REQ-12.

## CLI Rendering

`search --why --format markdown` should add a compact "Search Plan" section:

- query assessment
- extracted and dropped terms
- subqueries
- selected anchor groups
- repo-text promotions
- bridge status
- next commands
- source-truth checks

JSON should expose the full DTO. Markdown should stay readable and avoid answer
prose. Existing hit sections remain available for compatibility.

`drill --question` should consume the same plan and can suggest anchors, but the
report must continue to label question search as partial discovery until source
truth is checked.

Requirements: NLS-REQ-9, NLS-REQ-10.

## Risks And Mitigations

| Risk | Mitigation |
| --- | --- |
| Exact search regresses while broad search improves. | Add exact-symbol guard tests before planner behavior changes. |
| Repo-text promotion binds to the wrong symbol. | Require same-file evidence, record promotion method, and keep ambiguous promotions weak. |
| Planner becomes opaque. | Render subqueries, dropped terms, rejected hits, score reasons, and truncation. |
| Bridge expansion hurts latency. | Expand only selected anchors and enforce fixture latency gates. |
| Search becomes an answer surface. | Keep answer prose out of `search`; route final claim structure through `drill`. |
| CodeGraph habits are copied too literally. | Adopt retrieval mechanics, not the "trust the tool" guidance. |
