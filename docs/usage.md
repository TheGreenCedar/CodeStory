# CodeStory Usage

This is the operator guide. It keeps setup, common workflows, retrieval defaults,
and recovery notes in one place.

Examples use POSIX shell syntax unless a block is labeled PowerShell. On
Windows, use `.\target\release\codestory-cli.exe` for the release binary,
`$env:NAME = "value"` for environment variables, and Windows paths when that is
the workspace you are indexing.

## Install The Skill

Install the grounding skill once, then point it at explicit target workspaces.
See [README — Install As An Agent Skill](../README.md#install-as-an-agent-skill)
for the full copy/setup commands and Windows PowerShell variant.

The source skill package lives at
[../.agents/skills/codestory-grounding/SKILL.md](../.agents/skills/codestory-grounding/SKILL.md).
If you need a different source artifact, set `CODESTORY_REPO_URL` and
`CODESTORY_REPO_REF` before running setup. Without an explicit ref, installed
setup fetches and builds the remote default branch.

## Use From Source

Use this path when you are changing CodeStory itself or testing the current
checkout.

```sh
cargo build --release -p codestory-cli
CODESTORY_CLI="./target/release/codestory-cli"
"$CODESTORY_CLI" --help
```

Pick a target workspace explicitly:

```sh
TARGET_WORKSPACE="/path/to/repo"
"$CODESTORY_CLI" doctor --project "$TARGET_WORKSPACE"
"$CODESTORY_CLI" index --project "$TARGET_WORKSPACE" --refresh auto
"$CODESTORY_CLI" ground --project "$TARGET_WORKSPACE" --why
```

## Readiness Tracks

CodeStory has two readiness tracks. Keep them separate when deciding whether an
agent can rely on packet/search output.

### Local navigation/cache readiness

This lane is for local browsing and source navigation. It uses the project
SQLite cache built by `index` and read by commands such as `ground`, `symbol`,
`trail`, `snippet`, `explore`, `context`, `files`, and `affected`.

`doctor` may report this lane as `local_navigation`. Local navigation readiness
means the local cache, graph, lexical index, and DB-backed navigation commands
are usable. It does not prove agent packet/search readiness.

### Agent packet/search sidecar readiness

This lane is for agent-facing `packet` and `search` evidence. It requires the
sidecar retrieval stack to be built and healthy: Zoekt lexical shards, Qdrant
semantic vectors, SCIP graph artifacts, the llama.cpp query embedding endpoint,
and a current retrieval manifest.

`doctor` may report this lane as `agent_packet_search`. Agent packet/search
readiness means sidecar packet/search evidence is trustworthy only when
retrieval status reports `retrieval_mode: "full"`. Missing, stale, stubbed,
hash-vector, or non-product sidecar state is diagnostic only and must not be
described as agent packet/search readiness.

## Common Workflows

### I need a repo overview

```sh
codestory-cli doctor --project <target-workspace>
codestory-cli index --project <target-workspace> --refresh full
codestory-cli ground --project <target-workspace> --why
codestory-cli report --project <target-workspace> --output-file out/codestory-report.md
codestory-cli report --project <target-workspace> --format json --output-file out/codestory-graph.json
```

Use this when the repository is new to the agent. `doctor` tells you whether the
cache and retrieval state are usable. `ground --why` gives broad orientation and
reports limited coverage or gaps. `report` reads the current SQLite store
without refreshing it and emits generated artifacts: Markdown for repo summary,
hotspots, entry points, bridge/high-connectivity nodes, and next queries; JSON
for automation that needs the full current graph, including nodes, edges,
confidence/certainty, source locations, and generation metadata. `--limit`
bounds the Markdown report sections, not the full JSON graph export. Treat both
files as outputs to regenerate, not source-of-truth state.

### I need evidence for a broad question

```sh
codestory-cli packet --project <target-workspace> --question "<broad task question>" --budget compact
```

Use `packet` for questions like "how does routing work?" or "what owns indexing
state?" It returns a `sufficient`, `partial`, or `blocked` status with
citations, trust limits, gaps, and follow-up commands. If the packet is
`partial` or `blocked`, follow the named source-truth commands instead of
opening unstructured source files directly. Treat `sufficient` as evidence
coverage, not final answer-quality proof.

### I need to understand one symbol or file

```sh
codestory-cli search --project <target-workspace> --query "<symbol/file/literal/API path>" --why
codestory-cli explore --project <target-workspace> --id <node-id> --no-tui
codestory-cli trail --project <target-workspace> --id <node-id> --story --hide-speculative
codestory-cli snippet --project <target-workspace> --id <node-id> --context 40
```

Start with `search`, pick a concrete `node-id`, then inspect the relationships
and source. Use `context` when you want a bundled handoff around that target:

```sh
codestory-cli context --project <target-workspace> --id <node-id> --bundle out/context-name
```

Target context is DB-first evidence for one concrete target. `context` is
target-first; it is not an open chat endpoint and is not a replacement for broad
`packet`, `search`, or `drill` questions.

### I changed files and need likely impact

```sh
codestory-cli index --project <target-workspace> --refresh incremental
codestory-cli affected --project <target-workspace> --format markdown
git diff --name-only HEAD | codestory-cli affected --project <target-workspace> --stdin --format json
git diff --name-status HEAD | codestory-cli affected --project <target-workspace> --stdin --stdin-format name-status --format json
```

Treat `affected` as test-selection evidence, not a replacement for tests. The
default command preserves git name-status records; path-only stdin remains
available when another tool already chose the file list.

### The cache or local navigation looks stale

```sh
codestory-cli doctor --project <target-workspace>
codestory-cli index --project <target-workspace> --refresh full
codestory-cli doctor --project <target-workspace>
```

If `doctor` reports stale inventory, dense-anchor contract mismatch, missing
managed assets, or a non-`full` retrieval mode, fix that layer before
investigating answer quality. Treat the health report as the first source of
truth for cache and retrieval state.

For agent-facing packet/search recovery, use the full sidecar repair sequence
that `ready --goal agent` reports:

```sh
codestory-cli retrieval bootstrap --project <target-workspace> --format json
codestory-cli retrieval index --project <target-workspace> --refresh full --format json
codestory-cli retrieval status --project <target-workspace> --format json
codestory-cli doctor --project <target-workspace> --format markdown
```

When the core index is missing, stale, unchecked, or has recorded fatal indexing
errors, `ready` reports the necessary `codestory-cli index` repair first.
Otherwise, sidecar recovery does not need to repeat a full core reindex.
`retrieval bootstrap` prepares or checks the local sidecar services. The target
workspace is not packet/search-ready until `retrieval index` writes a current
target manifest and `doctor` or `retrieval status` reports `retrieval_mode=full`.

## Core Commands

- `doctor`: read-only health check for project, cache, index, retrieval, and
  environment readiness.
- `index`: build or refresh the SQLite graph, snapshots, search state,
  graph-native symbol docs, component reports, and selected dense anchors.
- `ground`: broad repo-level orientation snapshot; `--why` explains retrieval
  mode, coverage, gaps, and next commands.
- `report`: derived Markdown repo report or JSON graph export from the current
  SQLite store; use `--output-file` to keep artifacts separate from terminal
  logs.
- `packet`: bounded broad-task evidence packet with citations, budget usage,
  gaps, and follow-up commands.
- `search`: candidate discovery for symbols, files, literals, API paths,
  modules, and behavior terms.
- `symbol`: inspect one exact symbol and relationships.
- `trail`: follow caller, callee, and reference relationships around a symbol.
- `snippet`: fetch source context around a symbol.
- `explore`: bundled navigation packet or terminal explorer around a target.
- `context`: deep evidence bundle for one concrete target selected by `--id`,
  `--query`, or `--bookmark`.
- `affected`: map changed files to impacted symbols and likely tests.
- `files`: inspect indexed file inventory, language counts, roles, and coverage
  notes.
- `query`: run structured graph-query pipelines.
- `bookmark`: save, list, or remove investigation focus nodes.
- `drill`: write a deterministic investigation report for selected anchors.
- `setup embeddings`: install managed local embedding assets.
- `serve --stdio`: persistent local read surface for repeated agent queries.
  Use `get_node`, `neighbors`, `shortest_path`, or `query_subgraph` for cheap
  graph probes from known node ids before asking for a broad `packet`.
- `generate-completions`: emit shell completions from the command model.

## Index Options

`codestory-cli index` accepts these common options:

| Option | Default | Notes |
| --- | --- | --- |
| `--project <PROJECT>` | `.` | Repository root to index. `--path` is an alias. |
| `--cache-dir <DIR>` | per-project user cache | Uses the exact directory passed. |
| `--refresh <auto|full|incremental|none>` | `auto` | Controls indexing work before the summary returns. |
| `--format <markdown|json>` | `markdown` | JSON exposes the same summary for tests and automation. |
| `--output-file <PATH>` | stdout | Parent directory must already exist. |
| `--dry-run` | off | Computes the refresh plan without parsing or writing storage. |
| `--summarize` | off | Generates cached symbol summaries after indexing. |
| `--progress` | off | Prints progress to stderr so stdout stays parseable. |
| `--watch` | off | Keeps running and incrementally refreshes after file changes. |

Refresh modes:

| Mode | Behavior |
| --- | --- |
| `auto` | Full on an empty cache, incremental once indexed files exist. |
| `full` | Rebuilds the workspace graph and publishes a staged SQLite database. |
| `incremental` | Reindexes changed, new, and removed files in the live cache. |
| `none` | Opens the existing cache and returns a summary without indexing. |

Read commands default to `--refresh none`. Use `--refresh incremental` when a
read should refresh an existing cache first, and `--refresh full` after a cache
reset, schema change, or suspected stale-state incident.

## Predictable Output Modes

Most commands default to Markdown for human review. Use `--format json` when automation needs the complete structured result, including exact field comparisons such as `retrieval_mode` or cache paths. Use `--output-file <PATH>` when the artifact should live outside terminal logs. The parent directory must already exist.

`explore` opens the terminal UI by default when a TUI is available. Use `--no-tui`, `--plain`, or `CODESTORY_NO_TUI=1` for predictable command output in agent runs, tests, non-interactive terminals, and CI logs.

Agent-facing Markdown may start with `Status`, `Trust`, `Next Action`, and
`Proof Tier` before dense citations. Use `search --why --plan-details` only when
you need the full broad-query search plan.

## Retrieval Defaults

Sidecar retrieval is mandatory for agent-facing packet/search workflows. Agent
packet/search readiness means sidecar packet/search evidence is trustworthy only
when retrieval status reports `retrieval_mode=full`; missing sidecars, stale
manifests, or embedding-contract drift fail closed instead of falling back to an
older local search path.

Basic local index:

```sh
codestory-cli doctor --project <target-workspace>
codestory-cli index --project <target-workspace> --refresh full
codestory-cli ground --project <target-workspace> --why
```

That lane builds and reads the local SQLite cache. It does not start sidecars,
write the retrieval manifest, or prove agent packet/search readiness.

Product sidecar setup for agent-facing packet/search:

```sh
node scripts/setup-retrieval-env.mjs --fetch-embed-model
export CODESTORY_EMBED_MODEL_DIR="$(pwd)/target/retrieval-models"
export CODESTORY_EMBED_BACKEND="llamacpp"
export CODESTORY_EMBED_LLAMACPP_URL="http://127.0.0.1:8080/v1/embeddings"
cargo retrieval-setup

codestory-cli index --project <target-workspace> --refresh full
codestory-cli retrieval index --project <target-workspace> --refresh full
codestory-cli retrieval status --project <target-workspace> --format json
codestory-cli doctor --project <target-workspace>
```

`setup-retrieval-env.mjs --fetch-embed-model` downloads the configured GGUF to a
temporary path and verifies the pinned artifact before renaming it into
`CODESTORY_EMBED_MODEL_DIR`. The accepted artifact is exactly `117974304` bytes
with SHA-256
`ad1afe72cd6654a558667a3db10878b049a75bfd72912e1dabb91310d671173c`; all
configured mirrors must pass the same check.

Run `codestory-cli retrieval index` only after the local sidecar services,
llama.cpp embedding endpoint, and `bge-base-en-v1.5` model configuration are
ready, then require `retrieval status --format json` to report
`retrieval_mode: "full"` before trusting agent-facing packet/search evidence.
The status JSON also reports `query_embedding_backend`,
`manifest_vector_embedding_backend`, and `stored_doc_vector_producer_backend`
so backend drift is visible.

Legacy managed embedding setup is local semantic/diagnostic only:

```sh
codestory-cli setup embeddings --project <target-workspace> --dry-run --format json
codestory-cli setup embeddings --project <target-workspace>
```

Those commands install managed ONNX assets. They do not start llama.cpp, create
the retrieval manifest, or prove agent packet/search readiness. Retrieval sidecar
commands do not silently switch to ONNX mode just because managed assets are
installed; unset retrieval backend means the product llama.cpp sidecar contract.

Useful environment knobs:

- `CODESTORY_EMBED_BACKEND=llamacpp`: product embedding sidecar selection.
- `CODESTORY_EMBED_LLAMACPP_URL=http://127.0.0.1:8080/v1/embeddings`: local
  bge-base-en-v1.5 embedding endpoint.
- `CODESTORY_SEMANTIC_DOC_SCOPE=all`: include lower-signal symbols while
  investigating.
- `CODESTORY_LLM_DOC_EMBED_BATCH_SIZE=<n>`: override only while profiling.

Hash embeddings, ONNX-only experiments, lexical-only switches, and non-sidecar
embedding paths are diagnostic or historical comparison modes only.
Agent packet/search readiness requires repaired sidecars and
`retrieval_mode=full`.

`index`, `ground`, `search`, `context`, and `doctor` report retrieval mode and
degraded-state notes when retrieval state is available.

## Workspace And Config

CodeStory supports an optional `codestory_workspace.json` file at the repository
root for monorepo sessions:

```json
{
  "members": ["backend/", "frontend/", "shared/"]
}
```

Use `codestory_project.json` when one project needs explicit source groups:

```json
{
  "name": "api",
  "version": 1,
  "source_groups": [
    {
      "id": "11111111-1111-4111-8111-111111111111",
      "language": "TypeScript",
      "standard": "Default",
      "source_paths": ["src/"],
      "exclude_patterns": ["**/node_modules/**", "**/dist/**"],
      "include_paths": [],
      "defines": {},
      "language_specific": "Other"
    }
  ]
}
```

Team or user defaults can live in `.codestory.toml` at the project root or in
the user home directory. The home file loads first, the project file overrides
it for project-safe preferences, and explicit environment variables still win.

Example:

```toml
embedding_profile = "bge-base-en-v1.5"
embedding_model_id = "BAAI/bge-base-en-v1.5-local"
hybrid_retrieval_enabled = true
```

Project `.codestory.toml` files are not trusted to choose cache roots,
network/source-egress settings, or model selectors for source-egress calls. Put
`cache_dir` in the user home `.codestory.toml` or pass `--cache-dir`. Put
summary endpoints/models or embedding endpoints in trusted environment
variables such as `CODESTORY_SUMMARY_ENDPOINT`, `CODESTORY_SUMMARY_MODEL`, or
`CODESTORY_EMBED_LLAMACPP_URL`; a project file containing `summary_endpoint`,
`summary_model`, or `embedding_endpoint` is rejected unless
`CODESTORY_ALLOW_PROJECT_NETWORK_CONFIG=1` is set deliberately for that run.

`semantic_doc_scope` is intentionally omitted above because durable semantic
docs are the default. Set it only when opting into the broader all-symbol scope;
accepted all-symbol values are `all`, `full`, `all-symbols`, and `all_symbols`.
Other values currently resolve to the durable default.

## Cache Recovery

Typical recovery flow:

```sh
codestory-cli doctor --project <target-workspace>
codestory-cli index --project <target-workspace> --refresh full
codestory-cli search --project <target-workspace> --query WorkspaceIndexer
```

If the cache directory itself is suspect, get the exact project cache path from
`doctor`, verify that it is under the CodeStory cache root, move it aside first,
then rebuild. Remove the backup only after the fresh index is healthy:

```sh
cache_dir="<project-cache-dir-from-doctor>"
cache_root="${XDG_CACHE_HOME:-$HOME/.cache}/codestory"
resolved_cache="$(realpath "$cache_dir")"
resolved_root="$(realpath "$cache_root")"
case "$resolved_cache" in
  "$resolved_root"/*) ;;
  *) echo "Refusing to touch cache outside CodeStory cache root: $resolved_cache" >&2; exit 1 ;;
esac
backup="${resolved_cache}.bak-$(date +%Y%m%d%H%M%S)"
mv "$resolved_cache" "$backup"
codestory-cli index --project <target-workspace> --refresh full
codestory-cli doctor --project <target-workspace>
rm -rf "$backup"
```

Low-memory guidance:

- Prefer `index --refresh incremental` over repeated full refreshes.
- Avoid running multiple Cargo commands at once in this repo.
- If embedding assets or retrieval sidecars are unavailable, fix that setup
  layer before using packet/search evidence for broad agent grounding.
- If a cold index is slow, inspect semantic timing before changing parser or
  graph code.

## Verification

Run Cargo commands serially in this repo:

```sh
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
```

Focused docs/onboarding lane:

```sh
cargo test -p codestory-cli --test onboarding_contracts
```

Release-blocking fidelity lanes:

```sh
cargo test -p codestory-indexer --test fidelity_regression
cargo test -p codestory-indexer --test tictactoe_language_coverage
cargo test -p codestory-runtime --test retrieval_eval
```

`retrieval_eval` runs a fail-closed sidecar-primary check by default. Set
`CODESTORY_RETRIEVAL_EVAL_FULL_TESTS=1` only in an environment with real full sidecars to run the
semantic quality assertions.

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

- [concepts/how-codestory-works.md](concepts/how-codestory-works.md)
- [architecture/overview.md](architecture/overview.md)
- [architecture/runtime-execution-path.md](architecture/runtime-execution-path.md)
- [contributors/debugging.md](contributors/debugging.md)
- [contributors/testing-matrix.md](contributors/testing-matrix.md)
- [testing/benchmark-ledger.md](testing/benchmark-ledger.md)
- [testing/codestory-stdio-warm-loop-stats.md](testing/codestory-stdio-warm-loop-stats.md)
