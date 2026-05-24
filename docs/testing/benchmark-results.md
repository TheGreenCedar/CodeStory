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
| Strict packet-first rows | Several with-CodeStory public-checkout rows passed quality, packet-first, and zero ordinary source reads after packet. | Behavior evidence only; paired savings still needs broader quality-passing baselines. |
| Packet runtime | Public-core warm stdio and cold CLI packet rows passed repeated publishable quality gates. | Runtime evidence, not agent-token savings. |
| Repo-scale cold index/read timing | The current timing source is the latest row in [codestory-e2e-stats-log.md](codestory-e2e-stats-log.md). | Current only after a fresh row is logged for the relevant change. |
| Warm stdio smoke | The current warm-loop timing source is [codestory-stdio-warm-loop-stats.md](codestory-stdio-warm-loop-stats.md). | Smoke evidence for the persistent read surface. |

## What Is Solid

- CodeStory can produce quality-passing packet-first answers on selected public
  tasks while avoiding ordinary source reads after the answer packet.
- Repeated packet-runtime rows show `packet` can fit inside an agent workflow
  budget in both cold CLI and warm stdio modes.
- Repo-scale timing history is tracked in the stats log instead of copied into
  prose that silently drifts.

## What Is Not Claimed

This page does not claim that CodeStory generally reduces agent cost, token
count, wall time, or tool calls. General savings claims require repeated
controlled with/without-agent measurements from the benchmark harness, not one
exploratory row or representative estimates.

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
