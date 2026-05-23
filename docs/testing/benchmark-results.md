# CodeStory Benchmark Results

This page is the short, decision-grade benchmark source for the README. It
separates exploratory agent A/B checks from promotable runtime evidence so
marketing claims do not outrun the measurements.

## Latest Agent A/B Check

On 2026-05-23, the harness completed a one-repeat CodeStory repo run with the
default Codex runner model:

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --quick --repos codestory --repeats 1 --timeout-ms 600000 --sandbox danger-full-access --out-dir target\agent-benchmark\codestory-quick-2026-05-23f
```

This is an exploratory runner check, not a publishable savings claim. It used a
single repeat on one Windows workstation, no pricing variables were configured,
and `danger-full-access` was required because nested local command execution
failed under `read-only` and `workspace-write` in this environment. The
CodeStory arm did use CodeStory first: the transcript includes `doctor`,
`ground`, `search`, `trail`, and `snippet` before the final answer.

| Arm | Wall time | Total tokens | Input tokens | Output tokens | Tool starts | Status |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Without CodeStory | `272.60s` | `2,953,233` | `2,945,377` | `7,856` | `36` | Pass |
| With CodeStory | `291.12s` | `2,440,580` | `2,430,976` | `9,604` | `43` | Pass |

That one run showed `512,653` fewer total tokens with CodeStory (`17.4%`
lower), while wall time was `18.52s` slower (`6.8%` slower) and the runner
started `7` more tool commands. The right public statement is therefore narrow:
the harness can now collect real with/without rows, and this exploratory row
suggests a token-saving path worth repeating. It does not prove a general
cost, wall-time, or tool-call win.

## Runner Verification

The current Codex CLI supports the harness flags `exec --json --ephemeral
--sandbox --cd`. It does not support `--ask-for-approval`, so the harness does
not pass that flag. On Windows, the harness launches `codex.cmd` through
`cmd.exe` and sends the benchmark prompt over stdin to avoid shell quoting and
`.cmd` spawn failures.

Public harness defaults are reproducible from this repository: `--quick` and the
default repo set use only `codestory`. Private sibling repositories are opt-in
through `--include-local-repos` or explicit `--repos` values.

Use `--publishable` only when the selected runner reports token usage and every
run succeeds. For a public benchmark row, use at least three repeats, the same
model, the same sandbox mode, the same cache policy, and the same semantic
backend for both arms.

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

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --list
node .\scripts\codestory-agent-ab-benchmark.mjs --quick --repos codestory --repeats 3 --timeout-ms 600000 --publishable
```

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

This page does not claim that CodeStory generally reduces agent cost, token
count, wall time, or tool calls. General savings claims require repeated
controlled with/without-agent measurements from the benchmark harness, not one
exploratory row or representative estimates.

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
