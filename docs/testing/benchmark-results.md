# CodeStory Benchmark Results

This page is the short, decision-grade benchmark source for the README. It
separates exploratory agent A/B checks from promotable runtime evidence so
marketing claims do not outrun the measurements.

Runs recorded before the 2026-05-24 harness tightening are historical unless
they are reanalyzed or rerun with answer-level expected-file/symbol recall,
immutable manifest refs, and CodeStory cache provenance. The harness now keeps
transcript-observed anchors separate from anchors actually present in the final
answer, so tool output alone cannot make a row quality-pass.

## Latest Agent A/B Check

On 2026-05-23, the harness completed a three-repeat CodeStory repo run with the
default Codex runner model:

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --quick --repos codestory --repeats 3 --timeout-ms 900000 --sandbox danger-full-access --publishable --out-dir target\agent-benchmark\codestory-quick-2026-05-23-r3
```

This is a real baseline, not a savings claim. It used one Windows workstation,
no pricing variables were configured, and `danger-full-access` was required
because nested local command execution failed under `read-only` and
`workspace-write` in this environment. Before the run, the CodeStory repo cache
was refreshed in hash semantic mode and `doctor` reported `47,327` nodes,
`40,003` edges, `146` files, `6,379` semantic docs, and fresh inventory. The
CodeStory arm did use CodeStory first: the transcripts include `doctor`,
`ground`, `search`, `trail`, and `snippet` before final answers.

| Arm | Wall time | Total tokens | Input tokens | Output tokens | Tool starts | Status |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Without CodeStory | `214.90s` | `1,605,030` | `1,598,355` | `7,656` | `29` | `3/3` pass |
| With CodeStory | `306.24s` | `2,724,490` | `2,715,774` | `9,536` | `43` | `3/3` pass |

This run does not support a token, wall-time, or tool-call savings claim. The
CodeStory arm used `1,119,460` more median total tokens (`69.7%` more), took
`91.33s` longer (`42.5%` slower), and started `14` more tool commands (`48.3%`
more). The likely next benchmark work is to reduce duplicated ordinary file
reads after CodeStory grounding and to add non-Rust repositories before
promoting agent-savings claims.

### Exploratory Packet-First A/B Diagnostic

After the packet workflow, `CODESTORY_CLI` injection, packet-first publishable
gate, claim-token quality scoring, aggregate anchor scoring, PowerShell-wrapped
command classification, and the sufficient-packet stop rule were added, a
three-repeat manifest-backed A/B diagnostic was run for the CodeStory
indexing-flow task:

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --task-manifest benchmarks\tasks\codestory-indexing-flow.task.json --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-codestory-indexing-flow-r19 --allow-failures
node .\scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\ab-codestory-indexing-flow-r19
```

Strict reanalysis exposed an important flaw in the original packet-first
telemetry: `packet --help` and later packets were being counted as packet-first.
After the analyzer was fixed to require an answer packet with `--question` as
the first successful repository-context command, the old row remained
quality-positive but was no longer packet-first:

| Arm | Quality | Packet first | Wall time | Total tokens | Tool starts | Direct source reads | Reads after packet |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Without CodeStory | `1/3` | n/a | `347.90s` | `3,216,267` | `43` | `31` | n/a |
| With CodeStory r19 | `3/3` | `0/3` | `102.50s` | `362,855` | `5` | `1` | `0` |

A stricter prompt then gave the agent an exact first packet command and ran
with `--publishable`, which now fails if the answer packet is not first or if
ordinary source reads happen after packet:

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --task-manifest benchmarks\tasks\codestory-indexing-flow.task.json --arms with_codestory --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-codestory-indexing-flow-r20 --publishable
```

| Arm | Quality | Answer packet first | Wall time | Total tokens | Reasoning tokens | Tool starts | Direct source reads | Reads after packet |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| With CodeStory r20 | `3/3` | `3/3` | `71.92s` | `167,102` | `1,006` | `2` | `0` | `0` |

This is the first strict packet-first, publishable with-CodeStory behavior row:
the answer packet was the first repository-context command in every repeat,
quality passed every repeat, and the agent performed no ordinary source reads.
It is still not a public savings claim because it is one task, the paired
no-CodeStory baseline quality-passed only `1/3`, and the CodeStory manifest row
uses the active local checkout.

Additional strict with-CodeStory public-checkout rows were then run against
clean materialized public repositories:

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --task-manifest benchmarks\tasks\vite-dev-server-architecture.task.json --arms with_codestory --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-vite-dev-server-architecture-r21 --publishable
node .\scripts\codestory-agent-ab-benchmark.mjs --task-manifest benchmarks\tasks\express-response-send-bug-localization.task.json --arms with_codestory --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-express-response-send-bug-localization-r22 --publishable
node .\scripts\codestory-agent-ab-benchmark.mjs --task-manifest benchmarks\tasks\mux-router-matching-flow.task.json --arms with_codestory --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-mux-router-matching-flow-with-r24 --publishable
node .\scripts\codestory-agent-ab-benchmark.mjs --task-suite public-core --task-ids express-response-symbol-ownership --arms with_codestory,without_codestory --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-express-response-symbol-ownership-r26 --allow-failures
node .\scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\ab-express-response-symbol-ownership-r26 --publishable
node .\scripts\codestory-agent-ab-benchmark.mjs --task-suite public-core --task-ids mux-cors-middleware-edit-plan --arms with_codestory,without_codestory --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-mux-cors-middleware-edit-plan-r27 --allow-failures
node .\scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\ab-mux-cors-middleware-edit-plan-r27 --publishable
node .\scripts\codestory-agent-ab-benchmark.mjs --task-suite public-core --task-ids express-application-routing-flow --arms with_codestory,without_codestory --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-express-application-routing-flow-r28 --allow-failures
node .\scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\ab-express-application-routing-flow-r28 --publishable
```

| Repo | Task class | Quality | Answer packet first | Wall time | Total tokens | Reasoning tokens | Tool starts | Direct source reads |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| vite | architecture_explanation | `3/3` | `3/3` | `66.78s` | `163,815` | `1,011` | `2` | `0` |
| express | bug_localization | `3/3` | `3/3` | `67.39s` | `164,291` | `1,018` | `2` | `0` |
| mux | architecture_explanation | `3/3` | `3/3` | `65.12s` | `163,935` | `1,093` | `2` | `0` |
| express | symbol_ownership | `3/3` | `3/3` | `63.28s` | `165,715` | n/a | `2` | `0` |
| mux | edit_planning | `3/3` | `3/3` | `59.08s` | `100,833` | n/a | `2` | `0` |
| express | route_tracing | `3/3` | `3/3` | `62.80s` | `102,137` | n/a | `2` | `0` |

These rows used clean manifest checkouts (`manifest_overridden_by_builtin=false`
and `git_dirty=false`) and passed the strict post-packet source-read budget.
They strengthen the behavior claim across public TypeScript, JavaScript, and Go
tasks, while the Express and mux paired rows are now treated as historical
diagnostics until rerun or reanalyzed under the stricter 2026-05-24 gates.

### Historical Paired Diagnostics

The Express response-helper bug-localization task now has a paired no-CodeStory
baseline after its manifest claims were tightened to describe the required
technical facts rather than packet-specific wording:

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --task-manifest benchmarks\tasks\express-response-send-bug-localization.task.json --arms without_codestory --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-express-response-send-bug-localization-without-r23 --allow-failures
node .\scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\ab-express-response-send-bug-localization-r22 --publishable
node .\scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\ab-express-response-send-bug-localization-without-r23 --publishable
```

| Arm | Quality | Answer packet first | Wall time | Total tokens | Tool starts | Direct source reads |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Without CodeStory | `3/3` | n/a | `136.57s` | `635,793` | `18` | `1` |
| With CodeStory | `3/3` | `3/3` | `67.39s` | `164,291` | `2` | `0` |

Under the pre-2026-05-24 quality scorer, this paired row showed `74.2%` fewer
median total tokens, `50.7%` faster median wall time, and `88.9%` fewer tool
commands. Treat it as historical until it is rerun or reanalyzed with
answer-level expected-file/symbol recall and cache provenance.

The mux router matching-flow task adds a Go architecture row. Its baseline first
found every expected file and symbol, but missed claim recall because the
manifest required overly sentence-specific wording. The manifest claims were
tightened to the underlying source facts and both arms then passed
`--publishable` reanalysis:

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --task-manifest benchmarks\tasks\mux-router-matching-flow.task.json --arms with_codestory --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-mux-router-matching-flow-with-r24 --publishable
node .\scripts\codestory-agent-ab-benchmark.mjs --task-manifest benchmarks\tasks\mux-router-matching-flow.task.json --arms without_codestory --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-mux-router-matching-flow-without-r25 --allow-failures
node .\scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\ab-mux-router-matching-flow-with-r24 --publishable
node .\scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\ab-mux-router-matching-flow-without-r25 --publishable
```

| Arm | Quality | Answer packet first | Wall time | Total tokens | Tool starts | Direct source reads |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Without CodeStory | `3/3` | n/a | `123.37s` | `321,908` | `16` | `8` |
| With CodeStory | `3/3` | `3/3` | `65.12s` | `163,935` | `2` | `0` |

Under the pre-2026-05-24 quality scorer, this paired row showed `49.1%` fewer
median total tokens, `47.2%` faster median wall time, and `87.5%` fewer tool
commands. It also avoided the baseline's median `8` direct source reads. Treat
it as historical until it is rerun or reanalyzed with answer-level
expected-file/symbol recall and cache provenance.

The Express response symbol-ownership task adds a second Express row and a new
task class:

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --task-suite public-core --task-ids express-response-symbol-ownership --arms with_codestory,without_codestory --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-express-response-symbol-ownership-r26 --allow-failures
node .\scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\ab-express-response-symbol-ownership-r26 --publishable
```

| Arm | Quality | Answer packet first | Wall time | Total tokens | Tool starts | Direct source reads |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Without CodeStory | `3/3` | n/a | `125.10s` | `397,110` | `15` | `0` |
| With CodeStory | `3/3` | `3/3` | `63.28s` | `165,715` | `2` | `0` |

Under the pre-2026-05-24 quality scorer, this paired row showed `58.3%` fewer
median total tokens, `49.4%` faster median wall time, and `86.7%` fewer tool
commands. This row is especially useful as historical context because it covers
`symbol_ownership`, while the earlier Express row covered `bug_localization`.

The mux CORS middleware edit-planning task adds a Go edit-planning row:

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --task-suite public-core --task-ids mux-cors-middleware-edit-plan --arms with_codestory,without_codestory --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-mux-cors-middleware-edit-plan-r27 --allow-failures
node .\scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\ab-mux-cors-middleware-edit-plan-r27 --publishable
```

| Arm | Quality | Answer packet first | Wall time | Total tokens | Tool starts | Citation coverage |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Without CodeStory | `3/3` | n/a | `115.12s` | `364,175` | `14` | `75%` |
| With CodeStory | `3/3` | `3/3` | `59.08s` | `100,833` | `2` | `100%` |

Under the pre-2026-05-24 quality scorer, this paired row showed `72.3%` fewer
median total tokens, `48.7%` faster median wall time, and `85.7%` fewer tool
commands. It remains useful historical context because it tested whether the
packet could produce an edit plan without pushing the agent into a broad
file-reading pass.

The Express application routing-flow task adds a JavaScript route-tracing row:

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --task-suite public-core --task-ids express-application-routing-flow --arms with_codestory,without_codestory --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-express-application-routing-flow-r28 --allow-failures
node .\scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\ab-express-application-routing-flow-r28 --publishable
```

| Arm | Quality | Answer packet first | Wall time | Total tokens | Tool starts | Citation coverage |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Without CodeStory | `3/3` | n/a | `127.46s` | `334,231` | `15` | `100%` |
| With CodeStory | `3/3` | `3/3` | `62.80s` | `102,137` | `2` | `100%` |

Under the pre-2026-05-24 quality scorer, this paired row showed `69.4%` fewer
median total tokens, `50.7%` faster median wall time, and `86.7%` fewer tool
commands. It also covers `route_tracing`, bringing the historical paired
diagnostic set to five task rows.

A follow-up public-core subset run added the Vite dev-server architecture task
to test whether the packet-first stop rule generalized beyond this repository:

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --task-suite public-core --task-ids codestory-indexing-flow,vite-dev-server-architecture --arms with_codestory --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-public-core-subset-with-r1 --allow-failures
node .\scripts\codestory-agent-ab-benchmark.mjs --task-suite public-core --task-ids vite-dev-server-architecture --arms without_codestory --repeats 3 --timeout-ms 600000 --sandbox danger-full-access --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\ab-public-core-subset-vite-without-r1 --allow-failures
node .\scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\ab-public-core-subset-vite-without-r1
```

| Repo | Arm | Quality | Packet first | Wall time | Total tokens | Tool starts | Direct source reads | Reads after packet |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| codestory | With CodeStory | `3/3` | `0/3` | `93.66s` | `405,789` | `6` | `1` | `0` |
| vite | With CodeStory | `3/3` | `0/3` | `144.70s` | `472,782` | `7` | `0` | `0` |
| vite | Without CodeStory | `1/3` | n/a | `200.16s` | `907,113` | `26` | `18` | n/a |

The subset rows still show quality rescue, but they no longer support a
packet-first claim after strict reanalysis because the agents probed or searched
before the answer packet. The right public framing remains: CodeStory can rescue
answer quality and avoid broad file exploration on these tasks, but headline
savings need strict, quality-passing paired baselines across a larger corpus
under the answer-level quality and cache-provenance gates.

## Latest Packet Runtime Check

On 2026-05-23, the release CLI completed three-repeat packet runtime runs
against the full public-core manifest suite in both warm stdio and cold CLI
modes:

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --task-suite public-core --repeats 3 --packet-runtime-mode warm-stdio --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\packet-runtime-public-core-warm-r8 --timeout-ms 120000 --publishable
node .\scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --task-suite public-core --repeats 3 --packet-runtime-mode cold-cli --codestory-cli .\target\release\codestory-cli.exe --out-dir target\agent-benchmark\packet-runtime-public-core-cold-r9 --timeout-ms 120000 --publishable
```

This is now repeated packet-runtime evidence, not an agent savings claim. Across
both modes, all `108` packet rows passed operationally and quality gates. Every
row reported `sufficient` packet coverage with `0` sufficiency/quality
mismatches. Warm stdio task medians ranged from `2.69s` to `3.60s`, with an
aggregate task median of `3.13s`; cold CLI task medians ranged from `4.22s` to
`5.76s`, with an aggregate task median of `4.86s`. Median avoid-opening counts
were `8` to `10`, and every row had `0` median follow-up commands.

## Latest Public-Core Diagnostic

On 2026-05-23, the public-core repositories were materialized under
`target/agent-benchmark/repos` and indexed with the release CLI:

| Repo | Indexed files | Nodes | Edges | Semantic docs |
| --- | ---: | ---: | ---: | ---: |
| express | `153` | `13,046` | `1,138` | `241` |
| flask | `83` | `8,303` | `5,661` | `1,814` |
| mux | `16` | `259` | `243` | `243` |
| vite/packages/vite | `204` | `15,250` | `9,957` | `1,801` |

The latest full-suite diagnostics are warm `r8` and cold `r9`, generated by the
commands above. Warm median response-line size was `99,656` bytes across task
rows, with a max task median of `112,389` bytes. Cold median response-line size
was `165,065` bytes, reflecting process output and one-shot invocation overhead.
Warm median packet payload size was `95,381` bytes, and max median graph payload
size was `41,720` bytes after pruning graph nodes that were no longer referenced
by retained trail edges.

The first agent-level public-core subset has two with-CodeStory rows
(`codestory-indexing-flow` and `vite-dev-server-architecture`) that both
quality-passed `3/3`, but strict reanalysis shows neither row ran an answer
packet as the first repository-context command. The corrected CodeStory
indexing-flow r20 row fixed that prompt behavior and passed `3/3` packet-first
with zero ordinary source reads. Vite r21, Express r22, mux r24, and Express r26
extended that strict with-CodeStory shape to clean public checkouts. Mux r27 and
Express r28 added edit-planning and route-tracing rows. Express r23, mux r25,
Express r26, mux r27, and Express r28 provide five historical paired
diagnostic rows. The remaining blocker is breadth under the stricter gates:
more repositories and language families need paired quality-passing rows before
a headline savings claim.

Expected-file recall was `100%` on the strict packet-first public-checkout
rows except `express-response-send-bug-localization`, which still passed its
manifest gate with `75%` file recall and `75%` citation coverage. The mux CORS
baseline also passed with `75%` citation coverage. The next promotion blocker
is no longer packet quality or packet-runtime medians; it is broader
with/without-agent savings across more public repositories and language
families.

## Runner Verification

The current Codex CLI supports the harness flags `exec --json --ephemeral
--sandbox --cd`. It does not support `--ask-for-approval`, so the harness does
not pass that flag. On Windows, the harness launches `codex.cmd` through
`cmd.exe`, rejects command metacharacters in runner arguments, sends the
benchmark prompt over stdin, and kills the process tree on timeout to avoid
orphaned runner processes.

Public harness defaults are reproducible from this repository: `--quick` and the
default repo set use only `codestory`. Private sibling repositories are opt-in
through `--include-local-repos` or explicit `--repos` values.

Use `--publishable` only when the selected runner reports token usage and every
run succeeds. For agent A/B rows, `--publishable` also requires with-CodeStory
runs to execute `packet` first and stay within the post-packet ordinary
source-read budget, which defaults to zero reads after packet. Publishable rows
must carry clean repository provenance pinned to an immutable commit or tag plus
CodeStory cache provenance from `doctor --format json`; local, branch-like,
manifest-overridden, or cache-opaque checkouts are diagnostic rows, not
publishable public evidence. For a public benchmark row, use at least three
repeats, the same model, the same sandbox mode, the same cache policy, and the
same semantic backend for both arms.

## Runtime Budgets

These numbers are current local evidence for the CodeStory runtime itself. They
show that the index and read surfaces fit inside an agent workflow budget, but
they are not substitutes for with/without-agent savings.

| Lane | Current evidence | What it proves | Source |
| --- | ---: | --- | --- |
| CodeStory repo cold index and one-shot reads | `12.30s` index, `1.04s` search, `0.65s` symbol, `0.24s` trail, `0.21s` snippet | A release CLI can rebuild and query the CodeStory repo quickly with hash semantic mode on the Windows workstation | [codestory-e2e-stats-log.md](codestory-e2e-stats-log.md) |
| Indexed graph scale for that run | `56,362` nodes, `47,659` edges, `149` files, `7,530` semantic docs | The repo-scale gate exercises a real Rust workspace, not only toy fixtures | [codestory-e2e-stats-log.md](codestory-e2e-stats-log.md) |
| Warm stdio agent loop smoke | `53.50ms` per `search -> symbol -> trail -> snippet` loop across `20` reps | Once an index exists, the persistent read surface stays in tens of milliseconds on the small-fixture smoke | [codestory-stdio-warm-loop-stats.md](codestory-stdio-warm-loop-stats.md) |
| Warm stdio search p95 smoke | `25.96ms` p95 search | The smoke loop has a stable low-latency search budget and clean protocol stdout | [codestory-stdio-warm-loop-stats.md](codestory-stdio-warm-loop-stats.md) |
| Historical cross-repo retrieval gate | Hit@10 `1.0`, adversarial Hit@10 `1.0`, MRR@10 `0.826831`, search p95 `84.7ms` across `4` projects and `225` queries | The historical externally validated retrieval profile found expected anchors across several repo families | [embedding-backend-benchmarks.md](embedding-backend-benchmarks.md) |

## Methodology

The agent A/B harness runs the same repository prompt in two arms:

- `without_codestory`: the agent is instructed to avoid CodeStory and use normal
  repository exploration.
- `with_codestory`: the agent is instructed to use CodeStory grounding first,
  run `packet` for broad repository questions, then ordinary source reads only
  for named gaps.

The harness writes raw stdout/stderr per run, a JSONL run ledger, transcript
analysis, a machine summary, and a Markdown summary under
`target/agent-benchmark/<timestamp>`. The analyzer counts command categories,
duplicate command patterns, duplicate direct file reads, and ordinary source
reads after the first successful CodeStory command or packet. Reported
comparisons should use medians across successful repeats for the same runner,
repository set, prompt set, cache policy, semantic backend, and model.
Each run records repository provenance, including resolved checkout path,
manifest URL/ref, actual git HEAD, dirty status, and whether a built-in local
repo config overrode the manifest checkout. Rows with local overrides are
diagnostics, not public reproducibility evidence.
For the with-CodeStory arm, the harness injects `CODESTORY_CLI` from
`--codestory-cli` or the local release/debug binary and `--publishable` fails
when a with-CodeStory run does not execute an answer packet with `--question`
as the first successful repository-context command, or when it exceeds
`--max-source-reads-after-packet` after that packet.

```powershell
node .\scripts\codestory-agent-ab-benchmark.mjs --list
node .\scripts\codestory-agent-ab-benchmark.mjs --quick --repos codestory --repeats 3 --timeout-ms 600000 --publishable
node .\scripts\codestory-agent-ab-benchmark.mjs --task-suite public-core --list
node .\scripts\codestory-agent-ab-benchmark.mjs --task-suite public-core --task-ids codestory-indexing-flow,vite-dev-server-architecture --arms with_codestory --repeats 3 --max-source-reads-after-packet 0 --allow-failures
node .\scripts\codestory-agent-ab-benchmark.mjs --reanalyze-dir target\agent-benchmark\<run-dir>
node .\scripts\codestory-agent-ab-benchmark.mjs --task-suite public-core --materialize-repos --list
node .\scripts\codestory-agent-ab-benchmark.mjs --packet-runtime --task-suite public-core --repeats 3
```

Manifest-backed runs load public `*.task.json` files from
`benchmarks/tasks/`, score expected files, symbols, claims, citations, and
forbidden claims in the final answer, and separately report transcript-observed
anchors for diagnostics. They fail `--publishable` when a manifest-backed run
lacks quality scoring or misses its answer-level quality gate.
`--reanalyze-dir` recomputes transcript analysis, packet-first telemetry,
quality scores, and summaries from existing raw stdout JSONL files so analyzer
fixes can be applied without spending another model run.

Packet runtime runs compare cold CLI `packet` invocations with warm
`serve --stdio` packet calls. They are runtime rows, not agent-token rows, and
still use the manifest quality gates before a result can be promoted.
`--publishable` also enforces repeated runs, public-core corpus shape, immutable
repo refs, CodeStory cache provenance, and non-null passing answer-level quality
scores for every packet row. The earlier warm stdio `r8` and cold CLI `r9`
medians remain useful historical diagnostics, but rows should be refreshed
before being promoted under the stricter gates.

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
