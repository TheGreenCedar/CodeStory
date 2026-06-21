# CodeStory Usage

Start with the human task, then run the smallest path that proves the state you
need. The plugin is the normal path. The CLI is for setup, repair, debugging,
and transcripts.

## Operator Journey

| Stage | Human action | Agent/CLI action | Trust check |
| --- | --- | --- | --- |
| Install | Install the `codestory` plugin from `TheGreenCedar`. | Plugin starts `codestory-cli serve --stdio --refresh none`. | Fresh thread sees the active MCP runtime. |
| First grounding | Ask the agent to check readiness and ground the repo. | Read `codestory://status`, then `codestory://grounding` or `ground`. | `local_navigation` is ready before using local graph output. |
| Source work | Ask for a plan, review, or code path. | Use `files`, `symbol`, `trail`, `snippet`, `context`, and `affected`. | Claims cite concrete files, node ids, snippets, or trails. |
| Broad discovery | Ask a repo-wide question. | Use `packet` or `search`. | Trust only when `agent_packet_search` is ready and `retrieval_mode=full`. |
| Repair | Ask for a transcript or run CLI directly. | Use `doctor`, `index`, `retrieval status`, and sidecar repair commands. | Repeat readiness checks after repair. |

Packet/search output from degraded retrieval, missing sidecars, stale manifests,
or any non-`full` retrieval mode is navigation help only. It is not proof.

Most humans should start from the plugin flow in
[README - Use It With An Agent](../README.md#use-it-with-an-agent). Use the CLI
when you need the exact setup, repair, or debug record.

Shell examples below are POSIX unless noted. On Windows PowerShell, use
`.\target\release\codestory-cli.exe` for a source-built binary and set
environment variables with `$env:NAME = "value"`.

## Install And Ground A Repo

Install the CodeStory plugin once, then start a fresh agent thread for the
workspace you want to ground. The canonical skill package lives at
[../plugins/codestory/skills/codestory-grounding/SKILL.md](../plugins/codestory/skills/codestory-grounding/SKILL.md).

For a direct CLI transcript:

```sh
codestory-cli doctor --project <target-workspace>
codestory-cli index --project <target-workspace> --refresh auto
codestory-cli ground --project <target-workspace> --why
```

`doctor` separates local navigation readiness from agent packet/search
readiness. Do not infer packet/search readiness from a successful local
grounding command.

When changing CodeStory itself or testing the current checkout:

```sh
cargo build --release -p codestory-cli
CODESTORY_CLI="./target/release/codestory-cli"
"$CODESTORY_CLI" doctor --project <target-workspace>
```

The plugin source-build setup fallback accepts `CODESTORY_REPO_URL` and
`CODESTORY_REPO_REF` when you need a specific source artifact. Without an
explicit ref, setup fetches and builds the remote default branch.

## Readiness Lanes

| Question | Local navigation | Agent packet/search |
| --- | --- | --- |
| Lane id | `local_navigation` | `agent_packet_search` |
| Built by | `index` | `index`, then `retrieval index` |
| Requires | Healthy SQLite cache and graph | Healthy sidecars and `retrieval_mode=full` |
| Good for | Known files, symbols, trails, snippets, changed-file impact | Broad candidate discovery and bounded task packets |
| Commands | `ground`, `report`, `files`, `symbol`, `trail`, `snippet`, `explore`, `context --id`, `affected` | `packet`, `search`, broad `context --query` discovery |
| Does not prove | Broad sidecar search is ready | That cache-only browsing is enough for broad agent search |

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
If `packet`, `search`, or broad `context --query` reports
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
llama.cpp embedding contract.

```sh
node scripts/setup-retrieval-env.mjs --fetch-embed-model
export CODESTORY_EMBED_MODEL_DIR="$(pwd)/target/retrieval-models"
export CODESTORY_EMBED_BACKEND="llamacpp"
export CODESTORY_EMBED_LLAMACPP_URL="http://127.0.0.1:8080/v1/embeddings"

codestory-cli retrieval bootstrap --project <target-workspace> --format json
codestory-cli index --project <target-workspace> --refresh full
codestory-cli retrieval index --project <target-workspace> --refresh full --format json
codestory-cli retrieval status --project <target-workspace> --format json
codestory-cli doctor --project <target-workspace> --format markdown
```

`setup-retrieval-env.mjs --fetch-embed-model` verifies the pinned GGUF before
renaming it into `CODESTORY_EMBED_MODEL_DIR`. The accepted artifact is exactly
`117974304` bytes with SHA-256
`ad1afe72cd6654a558667a3db10878b049a75bfd72912e1dabb91310d671173c`.

`retrieval status --format json` reports `query_embedding_backend`,
`manifest_vector_embedding_backend`, and
`stored_doc_vector_producer_backend` so backend drift is visible.

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

## Command Cheat Sheet

| Command | Use |
| --- | --- |
| `doctor` | Read-only health check for project, cache, index, retrieval, and environment readiness. |
| `index` | Build or refresh the SQLite graph and derived local read models. |
| `ground` | Broad repo-level orientation snapshot. |
| `report` | Derived Markdown repo report or JSON graph export from the current SQLite store. |
| `files` | Indexed file inventory, language counts, roles, and coverage notes. |
| `symbol`, `trail`, `snippet`, `context --id` | Exact-target source inspection once you have a node id. |
| `affected` | Changed-file impact hints for review planning. |
| `packet`, `search`, `context --query` | Broad sidecar-backed discovery; trust only with `retrieval_mode=full`. |
| `retrieval bootstrap`, `retrieval index`, `retrieval status` | Sidecar setup, indexing, and readiness checks. |
| `serve --stdio` | Persistent local read surface for repeated agent queries. |
| `generate-completions` | Shell completions from the command model. |

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

- [concepts/how-codestory-works.md](concepts/how-codestory-works.md)
- [architecture/overview.md](architecture/overview.md)
- [architecture/runtime-execution-path.md](architecture/runtime-execution-path.md)
- [contributors/debugging.md](contributors/debugging.md)
- [contributors/testing-matrix.md](contributors/testing-matrix.md)
- [ops/retrieval-sidecars.md](ops/retrieval-sidecars.md)
