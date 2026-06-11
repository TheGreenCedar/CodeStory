# Agent Benchmark Harness Verification

Scope: transcript analysis and manifest-backed quality scoring for
`scripts/codestory-agent-ab-benchmark.mjs`.

The harness exposes pure analyzer/scorer functions and keeps a built-in
fixture smoke test:

```sh
node ./scripts/codestory-agent-ab-benchmark.mjs --self-test
```

The focused Node fixture lives at
`scripts/tests/codestory-agent-ab-analyzer.test.mjs`:

```sh
node --test ./scripts/tests/codestory-agent-ab-analyzer.test.mjs
```

The fixture verifies:

- command category counts for CodeStory CLI, shell search, direct file reads,
  git, and build/test commands;
- ordinary source reads after the first successful packet command;
- duplicate file reads by normalized path;
- expected file, symbol, claim, and citation recall;
- missed anchors as quality evidence, separate from operational run status.

`drill-suite` answer-quality ledgers are the repo-grounded counterpart to this
transcript scorer. Use the transcript harness to check how an agent behaved; use
`drill-suite --ledger <file>` to merge focused source-truth classifications back
into a real-repo evidence packet. Ledger claim classifications are `correct`,
`partial`, `misleading`, and `unsupported`, and the suite keeps the final
answer-quality verdict separate from green index/build mechanics.

For source-truth recall, `drill` now feeds the broad question search and bounded
supplemental searches into the verification target list. Treat those targets as
candidate files for verification, not as final answer support.

Keep `node ./scripts/codestory-agent-ab-benchmark.mjs --list` as the cheapest
configuration smoke check.

Do not make public savings claims from these fixtures. They only prove parser
and scorer behavior. Promotion evidence still requires real benchmark runs with
raw transcripts, repeated medians, and quality thresholds.
