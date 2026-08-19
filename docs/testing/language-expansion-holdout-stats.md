# Language expansion holdout evidence

**Audience:** Evidence record, not an install guide.

This is the evidence behind the [README evaluation section](../../README.md#evaluation). The [`language-support-ab.task.json`](../../benchmarks/tasks/language-expansion-holdout/language-support-ab.task.json) manifest covers 18 pinned public OSS packages with one architecture question per package.

The latest nested 18×3 sheet (`language-support-ab-window6`) is the measured run behind the [README evaluation table](../../README.md#evaluation): CodeStory **45/54** versus no-CodeStory **44/54**, with token and tool reductions and zero post-packet source reads. It has no `summary.json` because attestation requires a clean tracked checkout, and it was not run with `--publishable`. Do not quote the 2026-08-10 token line, the packet-gate 16/18 score, or any four-task excerpt as that claim.

## 2026-08-19 nested A/B (quality bar met)

| Field | Value |
| --- | --- |
| Artifact | `target/agent-benchmark/language-support-ab-window6` |
| Source head | `b2b0d1f9` plus uncommitted compact-window ranking/classifier/SQL presentation changes on `cursor/holdout-eval-quality-166c` |
| CodeStory CLI | `/private/tmp/cs-ship/release/codestory-cli`, SHA-256 `272b25869cdc8fcb1c4886feec212b41f8ed8974a080eafdf0840c583a6004aa` |
| Runner | ChatGPT Codex 0.148 (`/Applications/ChatGPT.app/Contents/Resources/codex`) |
| Model | `gpt-5.6-sol` |
| Date | 2026-08-19 |
| Repeats | 3 per arm; 108/108 process completions; without-arm reused from `language-support-ab-window3` |
| Quality | **with 45/54**, without **44/54** |
| After-packet source reads | 0 on the with arm |
| Accelerator | `embedding_device_observation_source=per_user_server`, Metal, `embedding_accelerator_execution_verified=true` |
| Publication | No `summary.json` (dirty-tree attestation). Not `--publishable`. |

| Metric | Without | With | Change |
| --- | ---: | ---: | ---: |
| Total tokens | 19,819,661 | 1,723,654 | −91% |
| Tool calls | 834 | 54 | −94% |

SQL and HTML remain 0/3 on both arms. fmt is 2/3 versus 3/3 and bash is 2/3 versus 3/3; kotlin is 3/3 versus 0/3. Packet JSON, not 3-repeat noise on identical packets, is the keep/revert evidence for those clusters.

```zsh
source target/agent-benchmark/managed-local-0.17.0/managed-env.sh
unset CODESTORY_CLI
export PATH="/Applications/ChatGPT.app/Contents/Resources:$PATH"
CODESTORY_CACHE_ROOT="$PWD/target/agent-benchmark/cache-ab-0e29027c" \
CODESTORY_RETRIEVAL=1 \
CODESTORY_EMBED_ALLOW_CPU=0 \
CODESTORY_EMBED_MODEL_SOURCE="$PWD/target/embedding-model-study/models/coderank-release-q8_0.gguf" \
node scripts/codestory-agent-ab-benchmark.mjs \
  --task-suite language-expansion-holdout \
  --arms without_codestory,with_codestory \
  --repeats 3 \
  --jobs 1 \
  --prepare-codestory-jobs 1 \
  --collect-all-failures \
  --allow-failures \
  --codestory-cli /private/tmp/cs-ship/release/codestory-cli \
  --model gpt-5.6-sol \
  --sandbox read-only \
  --timeout-ms 1200000 \
  --max-source-reads-after-packet 0 \
  --materialize-repos \
  --prepare-codestory-cache \
  --reuse-baseline-from "$PWD/target/agent-benchmark/language-support-ab-window3" \
  --out-dir "$PWD/target/agent-benchmark/language-support-ab-window6" \
  --canary-task-id python-requests-session-flow
```

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

## 2026-08-18 nested A/B (diagnostic, no public claim)

This nested 18×3 sheet exists. It is **not** a README claim: CodeStory answer quality is behind the no-CodeStory arm on the same sheet.

| Field | Value |
| --- | --- |
| Artifact | `target/agent-benchmark/language-support-ab-60431e79-full` |
| Source head | `60431e79` |
| CodeStory CLI | managed 0.17.0, SHA-256 `b6abd4e0414f16a97b4b6b45e312c6bf7c5366567664929e538e9d0cfa228158` |
| Runner | ChatGPT Codex 0.148 (`/Applications/ChatGPT.app/Contents/Resources/codex`) |
| Host | Alberts-MacBook-Air.local, Apple M5 |
| Date | 2026-08-18 |
| Repeats | 3 per arm; 54/54 process completions each |
| Quality | **with 30/54**, without **44/54** |
| After-packet source reads | 0 on the with arm |

Do not headline the token, tool, or source-read reductions. Quality is behind, so the sheet cannot support a savings claim.

The nested Codex session had no CodeStory MCP `packet` tool. The harness supplied one CLI packet prelude. `drill_once` packets then dead-ended: CLI `packet` did not forward `parent_packet_id` / `option_ids`, and the nested prompt forbade substituting shell/CLI for MCP. Packet JSON and transcripts, not the summary table, are the evidence for those clusters.

| Task | With quality | Without quality | First disposition (with) | Notes from packet JSON / transcripts |
| --- | ---: | ---: | --- | --- |
| python-requests-session-flow | 3/3 | 3/3 | `supported` | |
| java-commons-lang-string-utils | 3/3 | 3/3 | `supported` | |
| rust-ripgrep-search-pipeline | 3/3 | 3/3 | `supported` | |
| javascript-express-routing-flow | 3/3 | 3/3 | `supported` | |
| typescript-swr-hook-flow | 3/3 | 3/3 | `not_established` | Quality still passed |
| cpp-fmt-formatting-flow | 0/3 | 3/3 | `supported` | Packet ranked wchar `vformat_to` over the char format path |
| c-redis-command-loop | 3/3 | 3/3 | `supported` | |
| go-gin-route-dispatch | 0/3 | 3/3 | `drill_once` | `omitted-edge:request_dispatch`; agent could not continue |
| ruby-jekyll-site-build | 3/3 | 2/3 | `supported` | |
| php-monolog-record-flow | 3/3 | 3/3 | `supported` | |
| csharp-automapper-map-flow | 0/3 | 2/3 | `drill_once` | `omitted-material:mapper_config` / `mapper_execution` |
| kotlin-okio-buffer-flow | 0/3 | 0/3 | `supported` | Both arms fail; not the CodeStory-vs-baseline gap |
| swift-alamofire-request-flow | 2/3 | 3/3 | `not_established` | Packet-manifest 0/3 after two ranking-shaped tries |
| dart-http-client-flow | 0/3 | 3/3 | `drill_once` | `omitted-material:client_transport_send`; `IOClient.send` was already cited |
| bash-nvm-install-dispatch | 3/3 | 3/3 | `not_established` | Quality still passed |
| html-mdn-form-validation | 0/3 | 1/3 | `not_established` | Packet-manifest 0/3 after two ranking-shaped tries |
| css-animate-base-and-keyframes | 1/3 | 3/3 | `drill_once` | `omitted-material:css_animation_structure` (proof atoms, not the dispatch/send carrier idiom) |
| sql-chinook-schema-relations | 0/3 | 0/3 | `supported` | Both arms fail; not the CodeStory-vs-baseline gap |

A Homebrew Cask `codex` 0.146 install hung in `dyld_start` in a Cursor private-worker sandbox; this sheet used the ChatGPT.app binary from an unsandboxed login session. Direct `--codestory-cli target/release/codestory-cli` is `direct_cli_launch` and fails `--publishable`.

`--publishable` still requires a `supported` first packet and managed 0.17.0 identity. This 2026-08-18 nested sheet is historical diagnostic evidence. The README evaluation table uses the 2026-08-19 `language-support-ab-window6` `runs.jsonl` sheet above.

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

The superseded 2026-06-17 numbers measured the pre-0.16 retrieval path and are no longer used in the README. The README evaluation table uses the 2026-08-19 `language-support-ab-window6` nested 18×3 `runs.jsonl` sheet. A later `--publishable` `summary.json` with managed 0.17.0 identity can replace that table. Packet-gate rows cannot substitute for a nested quality sheet.
