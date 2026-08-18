# Language expansion holdout evidence

**Audience:** Evidence record, not an install guide.

This is the evidence behind the [README evaluation section](../../README.md#evaluation). The [`language-support-ab.task.json`](../../benchmarks/tasks/language-expansion-holdout/language-support-ab.task.json) manifest covers 18 pinned public OSS packages with one architecture question per package.

There is **no current public 18-task performance or answer-quality claim**. Do not quote the 2026-08-10 token line, the packet-gate 16/18 score, or any four-task excerpt as that claim. A README savings headline requires a nested `language-support-ab` `summary.json` in which CodeStory answer quality is at least the no-CodeStory arm on the same 18×3 sheet, tokens and tools drop, post-packet source reads are zero, and accelerator execution identity is verified.

## 2026-08-18 packet-gate census (diagnostic)

This is a packet-manifest census, not a nested agent A/B and not a README table.

| Field | Value |
| --- | --- |
| Artifact | `target/agent-benchmark/language-support-ab-census-d154a8b0` |
| Source head | `d154a8b0` |
| CodeStory CLI | 0.17.0, SHA-256 `b6abd4e0414f16a97b4b6b45e312c6bf7c5366567664929e538e9d0cfa228158` |
| Date | 2026-08-18 |
| Mode | `--packet-gate` / cold-cli packet runtime, one repeat |
| CPU embeddings | disabled (`CODESTORY_EMBED_ALLOW_CPU=0`) |
| Packet-manifest quality | **16 / 18** |
| Material obligations | 42 proven / 62 |

The two remaining packet-manifest misses, after two ranking-shaped tries on each, were `swift-alamofire-request-flow` (files 100%, symbols 0.50: `Session.request`, `Request.resume`, `DataRequest.validate`) and `html-mdn-form-validation` (files 50%: `full-example.html`, `min-max.html`). The other 16 tasks, including Requests, AutoMapper, animate.css, Redis, and Chinook, passed their packet-manifest thresholds. That 16/18 score is the promotion floor for attempting a nested A/B. It is not answer quality and not a public claim.

## 2026-08-18 nested A/B attempt (no sheet)

The Requests canary packet on the same head was `supported`, under the compact 98,304-byte cap (about 91,631 bytes), with Metal `embedding_accelerator_execution_verified: true`. A launcher-provisioned explicit package of that CLI stamped managed 0.17.0 identity (`cli_source=managed`, pinned plugin/CLI pair, `known_override_skew_channel=false`). Direct `--codestory-cli target/release/codestory-cli` is `direct_cli_launch` and fails `--publishable`.

The nested Codex runner did not start in the Cursor private-worker sandbox: `codex exec` stayed in `dyld_start` at about 112 KB RSS with empty stdout, so there is no `summary.json` / `runs.jsonl` for an 18×3 sheet. Stopped rather than letting 107 more agents run on a rejected or empty sheet.

Finish the nested run from an unsandboxed Apple Silicon login session (Terminal, not a Cursor cloud/private-worker shell) after provisioning the same CLI through the plugin launcher:

```zsh
source target/agent-benchmark/managed-local-0.17.0/managed-env.sh
unset CODESTORY_CLI
CODESTORY_CACHE_ROOT="$PWD/target/agent-benchmark/cache-ab-$(git rev-parse --short HEAD)" \
CODESTORY_RETRIEVAL=1 \
CODESTORY_EMBED_ALLOW_CPU=0 \
node scripts/codestory-agent-ab-benchmark.mjs \
  --task-suite language-expansion-holdout \
  --arms without_codestory,with_codestory \
  --repeats 3 \
  --materialize-repos \
  --prepare-codestory-cache \
  --codestory-cli "$CODESTORY_PLUGIN_CLI_PATH" \
  --model gpt-5.6-sol \
  --sandbox read-only \
  --timeout-ms 1200000 \
  --jobs 4 \
  --prepare-codestory-jobs 1 \
  --publishable \
  --max-source-reads-after-packet 0 \
  --out-dir "$PWD/target/agent-benchmark/language-support-ab-$(git rev-parse --short HEAD)" \
  --canary-task-id python-requests-session-flow
```

If the first CodeStory packet is `partial` with cap or obligation failures, stop the suite. Replace the README Evaluation table only from that run's `summary.json` / `runs.jsonl`. Headline a savings claim only if CodeStory quality is at least the no-CodeStory arm on that sheet.

## Rejected 2026-08-10 nested rerun

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

## Why the 2026-08-10 run was rejected

Only 34 of 54 answers in each arm passed the task manifest's quality checks. In the CodeStory arm, 51 packet preludes were `partial` and three were `blocked`; none was `sufficient`. The agent then made 237 ordinary source reads after the packet, while the publishable contract allowed zero. The run also lacked the accelerator execution identity required by the benchmark's environment gate. These failures mean the two arms did not establish the complete, packet-backed answer quality required for a public cost comparison.

## Raw diagnostic totals (2026-08-10 only)

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

The superseded 2026-06-17 numbers measured the pre-0.16 retrieval path and are no longer used in the README. A current performance claim requires a fresh paired nested run in which every repeat has complete token accounting, both arms pass their answer-quality checks, every CodeStory packet meets its sufficiency contract, the post-packet source-read budget is respected, the runtime identity is managed 0.17.0, and the accelerator execution identity is present. Packet-gate rows cannot substitute for that sheet. `summary.json` and `runs.jsonl` from a nested `--publishable` run are the source of truth for any future README table.
