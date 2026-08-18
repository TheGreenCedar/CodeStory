# Language expansion holdout evidence

**Audience:** Evidence record, not an install guide.

This is the evidence behind the [README evaluation section](../../README.md#evaluation). The [`language-support-ab.task.json`](../../benchmarks/tasks/language-expansion-holdout/language-support-ab.task.json) manifest covers 18 pinned public OSS packages with one architecture question per package.

## Current 0.17 rerun

The 2026-08-10 paired rerun completed, but it failed the benchmark's publication gate. Its resource totals are diagnostic only and are not a CodeStory performance claim.

| Field | Value |
| --- | --- |
| Artifact | `target/agent-benchmark/language-expansion-holdout-v017-direct-9e1277f3` |
| Harness head | `9e1277f3` |
| CodeStory CLI | 0.17.0, SHA-256 `bebd7f54ddfb75f5d57df249d949b87093036be8b8d10e4de05fb94c69f04a6a` |
| Date | 2026-08-10 |
| Tasks | 18 |
| Repeats | 3 per arm; both arms rerun from scratch |
| Sidecars | 18 of 18 prepared with `retrieval_mode: full` |
| Process completion | 108 of 108 runs; no failures, timeouts, or missing token usage |
| Publication result | Rejected |

The run used isolated, auth-only Codex homes for both arms. The without-CodeStory arm exposed no CodeStory CLI, skill, plugin, or MCP surface. The with-CodeStory arm used only the exact 0.17 CLI named above. No baseline result was reused.

```zsh
CODESTORY_CACHE_ROOT="$PWD/target/agent-benchmark/cache-v017-direct-9e1277f3" CODESTORY_RETRIEVAL=1 CODESTORY_EMBED_ALLOW_CPU=0 node scripts/codestory-agent-ab-benchmark.mjs --task-suite language-expansion-holdout --arms without_codestory,with_codestory --repeats 3 --materialize-repos --prepare-codestory-cache --codestory-cli "$PWD/target/release/codestory-cli" --model gpt-5.6-sol --sandbox read-only --out-dir "$PWD/target/agent-benchmark/language-expansion-holdout-v017-direct-9e1277f3" --timeout-ms 1200000 --jobs 4 --prepare-codestory-jobs 1 --publishable --max-source-reads-after-packet 0
```

## Why it was rejected

Only 34 of 54 answers in each arm passed the task manifest's quality checks. In the CodeStory arm, 51 packet preludes were `partial` and three were `blocked`; none was `sufficient`. The agent then made 237 ordinary source reads after the packet, while the publishable contract allowed zero. The run also lacked the accelerator execution identity required by the benchmark's environment gate. These failures mean the two arms did not establish the complete, packet-backed answer quality required for a public cost comparison.

## Raw diagnostic totals

These values explain the rejected run; they must not be quoted as evidence that 0.17 saves resources.

| Metric | Without | With | Raw change |
| --- | ---: | ---: | ---: |
| Total tokens | 7,977,909 | 5,280,133 | −34% |
| Repeat-task wall time | 2,919s | 2,945s | +0.9% |
| Tool calls | 683 | 548 | −20% |
| Commands | 683 | 548 | −20% |
| Direct source reads | 700 | 237 | −66% |
| All-in wall time | 2,919s | 3,708s | +27% |

All-in wall time includes the 762-second CodeStory cache preparation and packet preludes. The raw reductions in tokens, tool calls, and reads are not publishable because the answer and packet gates above failed.

## Boundary

The superseded 2026-06-17 numbers measured the pre-0.16 retrieval path and are no longer used in the README. A current performance claim requires a fresh paired run in which every repeat has complete token accounting, both arms pass their answer-quality checks, every CodeStory packet meets its sufficiency contract, the post-packet source-read budget is respected, and the accelerator execution identity is present. `summary.json` and `runs.jsonl` in the artifact directory are the source of truth for this rejected rerun.
