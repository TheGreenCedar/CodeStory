# Search Quality Eval Harness

**Audience:** Evidence record — not an install guide.

The lightweight search-quality harness checks whether expected symbols and
framework routes are discoverable after ranking or indexing changes.

## Metrics

- Recall: fraction of expected anchors found in the top returned indexed-symbol
  or repo-text evidence buckets.
- MRR: mean reciprocal rank of the expected anchor.
- Latency: maximum per-query wall-clock latency observed by the harness.
- Anchor buckets: whether expected anchors appeared in `indexed_symbol_hits`,
  `repo_text_hits`, or both.
- Search plan coverage: broad architecture queries must expose `search_plan`
  with bounded subqueries, candidate windows, anchor groups, and source-truth
  checks.
- Promotion precision: repo-text-only or ambiguous plan groups must not be
  marked high confidence.
- Field-qualified recall: `kind:`, `path:`, `name:`, and `lang:` queries must
  keep the expected anchor while filtering unrelated hits.
- Negative/noisy guard: a deliberately noisy query must not produce exact
  anchors.

## Command

```
cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
```

The test prints a line like:

```
search_quality_eval recall=1.000 mrr=0.833 max_latency_ms=42 anchor_buckets=indexed_symbol_hits=3,repo_text_hits=1
```

Production packet/search fixture schema and anchor baselines live in
`crates/codestory-cli/tests/fixtures/packet_search_eval/`. Run the lightweight
schema, category, baseline, and non-full-mode gating checks with:

```
cargo test -p codestory-cli --test packet_search_eval
```

This non-ignored command is the CI-safe packet/search gate. It validates fixture
schema, category coverage, baselines, and non-full-mode behavior without claiming
live retrieval readiness.

The live production-path check is ignored by default because it repairs and
requires `retrieval_mode=full` agent retrievals for the checkout:

```
cargo test -p codestory-cli --test packet_search_eval -- --ignored --nocapture packet_search_eval_live_runs_production_cli_path
```

Rows where readiness is not `ready` or retrieval mode is not `full` stay
diagnostic and do not count toward the full-retrieval baseline.

## When To Run

- Search ranking changes.
- Packet/search fixture or anchor-baseline changes.
- Framework route extraction changes.
- `explore`, `context`, `files`, or `affected` output changes that depend on
  search ranking or route discovery.
- Changes to semantic-doc aliases or lexical fallback behavior.
- Before claiming a new framework route class is searchable.
- After a performance or parallelization candidate touches search, fallback, or
  route ranking.

## Interpreting Failures

- Low recall means an expected anchor was not indexed or not returned. Check the
  failing query class first: exact symbol, CamelCase, compound term,
  natural-language query, route/endpoint, handler, likely-test hint,
  field-qualified filter, repo-text fallback, or negative/noisy query.
- Low MRR means the right anchor exists but noisy hits outrank it. Use
  `search --why --format json` on the failing query and inspect scoring,
  fallback, and `query_assessment` before changing ranking.
- High latency means the query path may be doing too much work for small
  fixtures. Compare with the performance playbook and repo-scale benchmarks
  before tuning.
- Missing `indexed_symbol_hits` means the graph/index side did not expose the
  expected anchor. Missing `repo_text_hits` means fallback text evidence did not
  find the expected file/excerpt. Missing both is a hard failure for the query.
- Missing `search_plan` on a broad architecture query means the planner did not
  classify or decompose the question; inspect `query_assessment`, extracted
  terms, and architecture-intent vocabulary before changing ranking weights.
- A `search_plan` group with `promotion_status=needs_source_read` or
  `ambiguous` and `confidence=high` is a calibration failure.
- A negative/noisy query that returns an exact expected anchor is a precision
  failure, even if recall looks high.
- A route query failure blocks route-support promotion until the route coverage
  playbook explains the gap or the fixture/search expectation is fixed.
- Field-qualified misses usually mean the candidate was retrieved but filtered
  out by kind/path/name/language normalization; rerun the same query with
  `--why --format json` and compare `indexed_symbol_hits` before weakening the
  filter.

## Promotion Rules

- Do not set permanent MRR or latency thresholds before the first expanded eval
  establishes a baseline for the current branch.
- Ranking and route-search changes pass only when expected anchors remain
  present, MRR stays above the agreed threshold, max latency stays under the
  fixture cap, and fallback source stays explainable.
- Broad architecture search changes also pass only when planned anchors remain
  visible, repo-text promotion status remains explicit, and source-truth checks
  are emitted for agent handoff.
- If a candidate regresses one metric for an intentional reason, record the
  reason in the validation notes. Silent regressions are rejected.
- Keep the eval CLI-first. Do not add or require server, MCP, watch, or
  transport behavior for Search Quality 2.0.

## Related Playbooks

- [framework-route-coverage.md](framework-route-coverage.md): route support
  status, confidence labels, fixture promotion, and non-promotable rules.
- [performance-review-playbook.md](performance-review-playbook.md): baseline
  capture, parallelization candidate gate, and rejected optimization records.
