# CodeStory Benchmark Results

This page is the short, decision-grade benchmark source for the README. It
separates publishable agent A/B results from internal runtime budget evidence so
marketing claims do not outrun the harness.

## Agent A/B Results

Publishable with/without-CodeStory agent results are pending. Do not estimate or
hand-author savings. The harness must generate the row, including wall time,
token usage when available, estimated cost when pricing is configured, tool-call
observations, runner metadata, raw transcripts, and per-run status.

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --list
node .\scripts\codestory-agent-ab-benchmark.mjs --quick --repos codestory --repeats 1 --timeout-ms 600000
```

Use `--publishable` only when the selected runner reports enough token usage to
support a public cost/token comparison. The harness exits nonzero when any run
fails or times out, unless `--allow-failures` is set for exploratory dry runs.

| Lane | Status | Promotion rule |
| --- | --- | --- |
| Agent wall time with and without CodeStory | Pending harness run | Promote medians only after all selected repo/arm/repeat runs succeed |
| Agent token and cost deltas | Pending runner usage data | Promote only when token usage is observed, not inferred |
| Agent tool-call deltas | Pending runner event data | Promote as observed runner events and keep raw transcripts linked |

## Runtime Budgets

These numbers are current local evidence for the CodeStory runtime itself. They
show that the index and read surfaces fit inside an agent workflow budget, but
they are not substitutes for with/without-agent savings.

| Lane | Current evidence | What it proves | Source |
| --- | ---: | --- | --- |
| CodeStory repo cold index and one-shot reads | `9.23s` index, `0.92s` search, `0.62s` symbol, `0.20s` trail, `0.18s` snippet | A release CLI can rebuild and query the CodeStory repo quickly with hash semantic mode on the Windows workstation | [codestory-e2e-stats-log.md](codestory-e2e-stats-log.md) |
| Indexed graph scale for that run | `47,107` nodes, `39,808` edges, `145` files, `6,358` semantic docs | The repo-scale gate exercises a real Rust workspace, not only toy fixtures | [codestory-e2e-stats-log.md](codestory-e2e-stats-log.md) |
| Warm stdio agent loop smoke | `53.50ms` per `search -> symbol -> trail -> snippet` loop across `20` reps | Once an index exists, the persistent read surface stays in tens of milliseconds on the small-fixture smoke | [codestory-stdio-warm-loop-stats.md](codestory-stdio-warm-loop-stats.md) |
| Warm stdio search p95 smoke | `25.96ms` p95 search | The smoke loop has a stable low-latency search budget and clean protocol stdout | [codestory-stdio-warm-loop-stats.md](codestory-stdio-warm-loop-stats.md) |
| Historical cross-repo retrieval gate | Hit@10 `1.0`, adversarial Hit@10 `1.0`, MRR@10 `0.826831`, search p95 `84.7ms` across `4` projects and `225` queries | The historical externally validated retrieval profile found expected anchors across several repo families | [embedding-backend-benchmarks.md](embedding-backend-benchmarks.md) |

## Methodology

The agent A/B harness runs the same repository prompt in two arms:

- `without_codestory`: the agent is instructed to avoid CodeStory and use normal
  repository exploration.
- `with_codestory`: the agent is instructed to use CodeStory grounding first,
  then ordinary source reads only when needed.

The harness writes raw stdout/stderr per run, a JSONL run ledger, a machine
summary, and a Markdown summary under `target/agent-benchmark/<timestamp>`.
Reported comparisons should use medians across successful repeats for the same
runner, repository set, prompt set, cache policy, semantic backend, and model.

Estimated cost is intentionally absent unless both token usage and pricing
environment variables are present:

```powershell
$env:CODESTORY_BENCH_INPUT_COST_PER_MTOK = "<usd-per-million-input-tokens>"
$env:CODESTORY_BENCH_OUTPUT_COST_PER_MTOK = "<usd-per-million-output-tokens>"
```

The cold repo lane uses the ignored `codestory_repo_e2e_stats` test after
building the release CLI. It creates an isolated cache, indexes the active
CodeStory workspace, then times `ground`, `search`, `symbol`, `trail`, and
`snippet`.

```powershell
cargo build --release -p codestory-cli
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

The warm stdio lane starts `serve --stdio`, runs repeated JSON-RPC tool calls
against a prebuilt small-fixture index, and verifies that stdout remains
protocol-only.

```powershell
cargo build --release -p codestory-cli
cargo test -p codestory-cli --test stdio_warm_loop_stats -- --ignored --nocapture
```

Search and retrieval quality use focused harnesses plus the longer embedding
research gates. The current managed ONNX backend still needs a fresh cross-repo
quality row before it should replace the historical llama.cpp row as promoted
external retrieval evidence.

```powershell
cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
cargo test -p codestory-runtime --test retrieval_eval
node .\scripts\cross-repo-promotion-benchmark.mjs --list
```

## What This Does Not Claim

This page does not yet claim that CodeStory reduces agent cost, token count,
wall time, or tool calls. Those claims require controlled with/without-agent
measurements from the benchmark harness, not representative estimates.

## Promotion Rules

- Use the same project, cache state, semantic backend, command flags, runner,
  model, and sample shape when comparing before/after results.
- Do not promote a speed win if expected anchors, MRR, Hit@10, protocol
  cleanliness, or semantic-doc reuse regress.
- Treat small-fixture warm-loop numbers as smoke evidence, not repo-scale
  product proof.
- Append current repo-scale timing rows to
  [codestory-e2e-stats-log.md](codestory-e2e-stats-log.md) when default
  indexing, semantic persistence, embedding reuse, or cold-start behavior
  changes.
