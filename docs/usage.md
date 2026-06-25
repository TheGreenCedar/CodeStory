# CodeStory Usage

Every new agent question often restarts repository discovery: search, read,
trace, repeat. That costs wall time and context on work you already did in the
last turn. CodeStory indexes once and serves evidence from that map so the agent
can answer from citations instead of re-exploring the tree.

Start with the human task, then run the smallest path that proves the state you
need. The golden path is CLI preflight first, then the agent plugin/MCP/hooks
path. The CLI is otherwise for repair, debugging, transcripts, and direct stdio
integration.

## Operator Journey

| Stage | Human action | Agent/CLI action | Trust check |
| --- | --- | --- | --- |
| Preflight | Run `codestory-cli agent preflight --project <target-workspace> --format json`. | Reports local graph readiness, full retrieval readiness, safe/blocked surfaces, and repair command. | Local graph surfaces are safe before source work; sidecar surfaces require `retrieval_mode=full`. |
| Install | Install the `codestory` agent plugin from `TheGreenCedar`. | Plugin starts its managed MCP adapter, then `codestory-cli serve --stdio --refresh none`. | Fresh thread sees the active MCP runtime. |
| First grounding | Start a fresh thread in the repo. | Hooks attempt startup grounding; the agent reads `codestory://status`, then `codestory://grounding` or `ground`. | Status reports `server_version`, `cli_version`, `server_executable`, `server_executable_sha256`, `sidecar_contract_version`, `plugin_runtime`, `runtime_truth`, `sidecar_setup`, and `allowed_surfaces`. `plugin_runtime.plugin_root` and `plugin_cache_version` identify the installed package cache when launched by the plugin adapter. |
| Source work | Ask for a plan, review, or code path. | Use allowed local graph surfaces such as `files`, `symbol`, `callers`, `callees`, `trail`, `trace`, `snippet`, `symbols`, `get_node`, `neighbors`, `shortest_path`, `query_subgraph`, and `affected`. | Claims cite concrete files, node ids, snippets, or trails. |
| Broad discovery | Ask a repo-wide question. | Hooks may attempt request-aware packets; the agent may use `packet`, `search`, or `context`. | Trust only when that surface is allowed and `retrieval_mode=full`. |
| Repair | Ask for a transcript or run CLI directly. | Use `ready --goal local --repair` or `ready --goal agent --repair`. | Repeat readiness checks after repair. |

Packet/search output from degraded retrieval, missing sidecars, stale manifests,
or any non-`full` retrieval mode is navigation help only. It is not proof.

Most humans should start from the plugin flow in
[README - Quick start](../README.md#quick-start). Use the CLI when you need the
exact setup, repair, or debug record.

Hook-enabled hosts keep CodeStory ambient. Hooks attempt strict grounding on
session start, resume, clear, and compact handoff; they attempt request-aware
packet grounding on each user prompt; and they fail open with the next
CodeStory check instead of blocking the host.

### Example prompts

**Portable templates (any repository):**

```text
@CodeStory read codestory://status, report allowed_surfaces for this checkout, ground the repo if allowed, and tell me whether packet/search/context need sidecar repair before I use them.
```

```text
@CodeStory Where is [TARGET_FEATURE] defined and who calls it?
```

```text
@CodeStory I am editing [PATH_TO_FILE]. What symbols are affected and what tests should I run first?
```

Replace `[TARGET_FEATURE]` and `[PATH_TO_FILE]` with concrete symbols and paths
from your project. A good answer cites concrete paths and flags gaps when
sidecars or coverage are degraded.

**CodeStory repository examples:**

Use concrete repo terms, not generic architecture words:

```text
@CodeStory read codestory://status, report allowed_surfaces for this checkout, ground the repo if allowed, and tell me whether packet/search/context need sidecar repair before codestory-indexer changes.
```

```text
@CodeStory Where is RefreshMode defined, which codestory-cli commands accept --refresh, and what is the call path from index into codestory-store?
```

```text
@CodeStory I am editing crates/codestory-indexer/src/resolution/mod.rs. What symbols are affected by changes in this file, and what tests should I run first?
```

```text
@CodeStory Explain where strict_sidecar_status decides retrieval_mode=full.
```

Shell examples below are POSIX unless noted. On Windows PowerShell, use
`.\target\release\codestory-cli.exe` for a source-built binary and set
environment variables with `$env:NAME = "value"`.

## Install And Ground A Repo

Run the CLI preflight in the workspace first:

```sh
codestory-cli agent preflight --project <target-workspace> --format json
```

Use its `safe_surfaces`, `blocked_surfaces`, and `repair_command` fields as the
agent handoff. If local graph surfaces are blocked, run the repair command
before source work. If only `packet`, `search`, or `context` is blocked, local
navigation can continue while sidecars are repaired later.

Install the CodeStory plugin once, then start a fresh agent thread for that
workspace. The canonical skill package lives at
[../plugins/codestory/skills/codestory-grounding/SKILL.md](../plugins/codestory/skills/codestory-grounding/SKILL.md).

**Codex plugin installation flow:**

1. Open Codex in the repository you want to ground
2. Run `/plugins` and install **TheGreenCedar → codestory**
3. Start a fresh thread and ask:

```text
@CodeStory read codestory://status, report allowed_surfaces for this checkout, ground the repo if allowed, and tell me whether packet/search/context need sidecar repair before I use them.
```

**Direct CLI transcript (for setup, repair, or debugging):**

```sh
codestory-cli agent preflight --project <target-workspace> --format json
codestory-cli ready --goal local --repair --project <target-workspace> --format json
codestory-cli ground --project <target-workspace> --why
```

**What each command does:**

- `ready --goal local --repair`: Builds or refreshes the SQLite graph and derived local read models
- `ground`: Provides a broad repo-level orientation snapshot

**Key guidance:**

When MCP is live, use `codestory://status` as the runtime truth. Its
`server_version`, `cli_version`, `server_executable`,
`server_executable_sha256`, `sidecar_contract_version`, `plugin_runtime`,
`runtime_truth`, and `sidecar_setup` fields identify the active server, CLI, sidecar contract,
plugin launch source, sidecar setup policy, `build_source`, and `repo_ref`, and
`plugin_runtime.plugin_root` and `plugin_cache_version` identify the installed
plugin cache when the adapter is active. `allowed_surfaces` tells the agent
which tools are safe now. Do not infer
packet/search readiness from a successful local grounding command.

When MCP is not live, use the CLI preflight contract instead:

```sh
codestory-cli agent preflight --project <target-workspace> --format json
```

Use its `safe_surfaces`, `blocked_surfaces`, and `repair_command` fields as the
agent handoff.

**Next steps after installation:**

If the agent reports that local graph surfaces are allowed but `packet`,
`search`, or `context` is not allowed, local browse work can continue. Repair
the sidecar lane only when the task needs those sidecar-backed surfaces:

```sh
codestory-cli ready --goal agent --repair --project <target-workspace> --format json
```

**Common next steps:**

- If `packet`, `search`, or `context` is not allowed: Keep local browse work local; repair retrieval sidecars only when the task needs those surfaces
- If `ground` or `files` is not allowed: The agent will guide you through indexing the repository
- If both are ready: You can proceed with using CodeStory for your task

**Verification checkpoints:**

After the initial setup, you can collect CLI health evidence with:

```sh
codestory-cli doctor --project <target-workspace>
```

Use this when MCP is missing, a repair transcript is needed, or the status
resource points to stale runtime evidence.

When changing CodeStory itself or testing the current checkout:

```sh
cargo build --release -p codestory-cli
CODESTORY_CLI="./target/release/codestory-cli"
"$CODESTORY_CLI" doctor --project <target-workspace>
```

The plugin source-build setup fallback accepts `CODESTORY_REPO_URL` and
`CODESTORY_REPO_REF` when you need a specific source artifact. Without an
explicit ref, setup fetches and builds the remote default branch.

## Readiness Contract

| Runtime truth | Allows | Blocks |
| --- | --- | --- |
| `codestory://status` | Current `server_version`, `cli_version`, `server_executable`, `server_executable_sha256`, `sidecar_contract_version`, `plugin_runtime`, `runtime_truth`, `sidecar_setup`, `build_source`, `repo_ref`, and `allowed_surfaces`; `plugin_runtime.plugin_root` and `plugin_cache_version` identify the installed package cache when launched by the plugin adapter. Use this first when MCP is live. | Guessing active runtime from source checkout, marketplace cache, or `PATH` alone. |
| `allowed_surfaces.<surface>.allowed` for `ground`, `files`, `symbol`, `definition`, `callers`, `callees`, `trail`, `trace`, `references`, `snippet`, `affected`, `symbols`, `get_node`, `neighbors`, `shortest_path`, and `query_subgraph` | The named MCP local graph surface only; check each surface's own `.allowed` bit before calling it. | Other local surfaces, `packet`, `search`, or `context`. |
| `allowed_surfaces.packet.allowed`, `allowed_surfaces.search.allowed`, and `allowed_surfaces.context.allowed` with `retrieval_mode=full` | `packet`, `search`, and `context` for broad candidate discovery and bounded evidence packets. | Answer-quality claims without matching packet-runtime, drill, benchmark, or source evidence. |

`context` is not a local-only browse surface. Even when the target is concrete,
use it only when `allowed_surfaces.context.allowed` is true and
`retrieval_mode=full`. Use each allowed MCP local graph surface's own status bit
for cache-only local navigation when sidecars are degraded. `explore` is
CLI-only and is not an MCP `allowed_surfaces` entry.

Sidecar topology:
[architecture/overview.md](architecture/overview.md),
[ops/retrieval-sidecars.md](ops/retrieval-sidecars.md).

## Local Navigation

Use this lane when you need to understand files, symbols, and likely impact
without broad sidecar search.

```sh
codestory-cli ground --project <target-workspace> --why
codestory-cli files --project <target-workspace> --path src --limit 80
codestory-cli symbol --project <target-workspace> --id <node-id>
codestory-cli trail --project <target-workspace> --id <node-id> --story --hide-speculative
codestory-cli snippet --project <target-workspace> --id <node-id> --context 40
codestory-cli affected --project <target-workspace> --format markdown
```

For review planning, you can pipe changed files into `affected`:

```sh
git diff --name-only HEAD | codestory-cli affected --project <target-workspace> --stdin --format json
```

Impact hints are not a substitute for running the relevant tests.

## Broad Packet/Search

Use this lane when the question is too broad for known node ids or file paths.

```sh
codestory-cli retrieval status --project <target-workspace> --format json
codestory-cli packet --project <target-workspace> --question "<broad task question>" --budget compact
codestory-cli search --project <target-workspace> --query "<symbol/file/literal/behavior>" --why
```

Trust the result only when retrieval status reports `retrieval_mode: "full"`.
If `packet` or `search` reports
`retrieval_unavailable`, degraded retrieval, or a non-`full` mode, use the
output only as a navigation hint and repair the sidecar lane before treating it
as evidence.

## Stale Local Cache

When local navigation looks stale, refresh the SQLite graph before repeating
read commands:

```sh
codestory-cli doctor --project <target-workspace>
codestory-cli index --project <target-workspace> --refresh full
codestory-cli doctor --project <target-workspace>
```

Read commands default to `--refresh none`. Use `--refresh incremental` when a
read should refresh an existing cache first, and `--refresh full` after a cache
reset, schema change, or suspected stale-state incident.

If the cache directory itself is suspect, get the exact project cache path from
`doctor`, verify it is under the CodeStory cache root, move it aside, rebuild,
and delete the backup only after `doctor` is healthy.

## Sidecar Repair

Agent packet/search requires product sidecars and the `bge-base-en-v1.5`
llama.cpp embedding contract. Product sidecar setup is owned by
[ops/retrieval-sidecars.md](ops/retrieval-sidecars.md); follow that runbook for
model download, sidecar lifecycle, environment variables, `retrieval bootstrap`,
`retrieval index`, `retrieval status`, CI smoke, and repair steps.

Operational contract for this usage page:

- Run `retrieval bootstrap` and `retrieval index` for the same target workspace
  you will query.
- Require `retrieval status --format json` to report
  `retrieval_mode: "full"` before trusting packet/search evidence.
- Treat backend drift fields in status JSON as blockers until the sidecar
  runbook explains the mismatch.

Legacy managed embeddings are diagnostic only:

```sh
codestory-cli setup embeddings --project <target-workspace> --dry-run --format json
codestory-cli setup embeddings --project <target-workspace>
```

Those commands do not start llama.cpp, create the retrieval manifest, or prove
agent packet/search readiness.

## Output And Configuration

Most commands default to Markdown. Use `--format json` for automation and
`--output-file <PATH>` when the artifact should live outside terminal logs. The
parent directory must already exist.

`explore` opens the terminal UI by default when a TUI is available. Use
`--no-tui`, `--plain`, or `CODESTORY_NO_TUI=1` for predictable command output in
agent runs, tests, non-interactive terminals, and CI logs.

Optional project config:

```json
{
  "members": ["backend/", "frontend/", "shared/"]
}
```

Team or user defaults can live in `.codestory.toml` at the project root or in
the user home directory. The home file loads first, the project file overrides
it for project-safe preferences, and explicit environment variables still win.

Project `.codestory.toml` files are not trusted to choose cache roots,
network/source-egress settings, or model selectors for source-egress calls. Put
`cache_dir` in the user home `.codestory.toml` or pass `--cache-dir`. Put
summary endpoints/models or embedding endpoints in trusted environment
variables such as `CODESTORY_SUMMARY_ENDPOINT`, `CODESTORY_SUMMARY_MODEL`, or
`CODESTORY_EMBED_LLAMACPP_URL`.

## Command By Situation

Use the command that matches where the agent is stuck. Do not run a generic
inventory sweep when one status field or one graph target will answer the
question.

| Stuck situation | First command | Use next when |
| --- | --- | --- |
| Orientation: "What is in this checkout?" | `codestory-cli ground --project <repo> --why` | Use `files --project <repo>` when the question is about language mix, generated/vendor files, or incomplete coverage. |
| Implementation start: "Where do I edit?" | `codestory-cli symbol --project <repo> --query "<feature-or-type>"` | Use `callers`, `callees`, `trace`, or `trail --story --hide-speculative` after choosing a concrete node. |
| Symbol impact: "What might this change touch?" | `git diff --name-only HEAD | codestory-cli affected --project <repo> --stdin --format json` | Use the output to choose focused tests; it is not a test result. |
| Test choice: "Which verification is smallest?" | `codestory-cli affected --project <repo> --format markdown` | Use repo docs or touched test names to pick one narrow test before broader lanes. |
| Source snippet: "Show me the relevant code." | `codestory-cli snippet --project <repo> --id <node-id> --function-body --lines 80` | Use `callers`, `callees`, or `trace` when relationships matter; if snippet truncates, follow its truncation guidance or read the source file directly. |
| Readiness: "Can I trust CodeStory now?" | `codestory-cli agent preflight --project <repo> --format json` | Use `codestory://status` instead when MCP is live; branch on `allowed_surfaces`. |
| Repair: "A surface is blocked." | `codestory-cli ready --goal local --repair --project <repo> --format json` for local graph, or `codestory-cli ready --goal agent --repair --project <repo> --format json` for packet/search/context | Rerun `agent preflight`, `doctor`, or `retrieval status` after repair; do not retry a failed live surface until the failing layer changes. |
| Broad evidence: "I need a packet/search answer." | `codestory-cli retrieval status --project <repo> --format json` | Run `packet`, `search`, or `context` only after `retrieval_mode` is `full` and the matching surface is allowed. |

## Verification

Run Cargo commands serially in this repo.

Docs-only lane:

```sh
git diff --check
```

Routine code lane:

```sh
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
```

Release-blocking fidelity lanes:

```sh
cargo test -p codestory-indexer --test fidelity_regression
cargo test -p codestory-indexer --test tictactoe_language_coverage
cargo test -p codestory-runtime --test retrieval_eval
```

Heavy repo-scale timing lane:

```sh
cargo build --release -p codestory-cli
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
```

Append fresh headline rows to
[testing/codestory-e2e-stats-log.md](testing/codestory-e2e-stats-log.md) when
default indexing, semantic persistence, embedding reuse, or cold-start behavior
changes.

## Further Reading

- [architecture/overview.md](architecture/overview.md)
- [architecture/runtime-execution-path.md](architecture/runtime-execution-path.md)
- [contributors/debugging.md](contributors/debugging.md)
- [contributors/testing-matrix.md](contributors/testing-matrix.md)
- [ops/retrieval-sidecars.md](ops/retrieval-sidecars.md)
