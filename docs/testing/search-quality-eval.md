# Search Quality Eval Harness

The lightweight search-quality harness checks whether expected symbols and
framework routes are discoverable after ranking or indexing changes.

## Metrics

- Recall: fraction of expected anchors found in the top returned indexed-symbol
  hits.
- MRR: mean reciprocal rank of the expected anchor.
- Latency: maximum per-query wall-clock latency observed by the harness.

## Command

```
cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
```

The test prints a line like:

```
search_quality_eval recall=1.000 mrr=0.833 max_latency_ms=42
```

## When To Run

- Search ranking changes.
- Framework route extraction changes.
- Changes to semantic-doc aliases or lexical fallback behavior.
- Before claiming a new framework route class is searchable.

## Interpreting Failures

- Low recall means an expected anchor was not indexed or not returned.
- Low MRR means the right anchor exists but noisy hits outrank it.
- High latency means the query path may be doing too much work for small
  fixtures; compare with repo-scale benchmarks before tuning.
