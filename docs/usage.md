# CodeStory Usage

This is the operator guide. It keeps setup, common workflows, retrieval defaults,
and recovery notes in one place.

## Install The Skill

Install the grounding skill once, then point it at explicit target workspaces.

```powershell
$SkillHome = "<agent-global-skill-directory>"
New-Item -ItemType Directory -Force -Path $SkillHome | Out-Null
Copy-Item -Recurse -Force .\.agents\skills\codestory-grounding "$SkillHome\codestory-grounding"
& "$SkillHome\codestory-grounding\scripts\setup.ps1"
```

On Unix-like systems:

```sh
bash "<agent-global-skill-directory>/codestory-grounding/scripts/setup.sh"
```

The setup script prints the resolved `CODESTORY_CLI` path. Persist it if your
agent environment does not already preserve the variable between sessions.

```powershell
setx CODESTORY_CLI "C:\Users\you\AppData\Local\CodeStory\bin\codestory-cli.exe"
```

The source skill package lives at
[../.agents/skills/codestory-grounding/SKILL.md](../.agents/skills/codestory-grounding/SKILL.md).
If you need a different source artifact, set `CODESTORY_REPO_URL` and
`CODESTORY_REPO_REF` before running setup. Without an explicit ref, installed
setup fetches and builds the remote default branch.

## Use From Source

Use this path when you are changing CodeStory itself or testing the current
checkout.

```powershell
cargo build --release -p codestory-cli
$CodeStoryCli = ".\target\release\codestory-cli.exe"
& $CodeStoryCli --help
```

Pick a target workspace explicitly:

```powershell
$TargetWorkspace = "C:\path\to\repo"
& $CodeStoryCli doctor --project $TargetWorkspace
& $CodeStoryCli index --project $TargetWorkspace --refresh auto
& $CodeStoryCli ground --project $TargetWorkspace --why
```

## Common Workflows

### I need a repo overview

```powershell
codestory-cli doctor --project <target-workspace>
codestory-cli index --project <target-workspace> --refresh full
codestory-cli ground --project <target-workspace> --why
```

Use this when the repository is new to the agent. `doctor` tells you whether the
cache and retrieval state are usable. `ground --why` gives broad orientation and
reports limited coverage or gaps.

### I need evidence for a broad question

```powershell
codestory-cli packet --project <target-workspace> --question "<broad task question>" --budget compact
```

Use `packet` for questions like "how does routing work?" or "what owns indexing
state?" It returns citations, gaps, and follow-up commands. If the packet says
the evidence is incomplete, follow the named commands instead of opening
unstructured source files directly.

### I need to understand one symbol or file

```powershell
codestory-cli search --project <target-workspace> --query "<symbol/file/literal/API path>" --why
codestory-cli explore --project <target-workspace> --id <node-id> --no-tui
codestory-cli trail --project <target-workspace> --id <node-id> --story --hide-speculative
codestory-cli snippet --project <target-workspace> --id <node-id> --context 40
```

Start with `search`, pick a concrete `node-id`, then inspect the relationships
and source. Use `context` when you want a bundled handoff around that target:

```powershell
codestory-cli context --project <target-workspace> --id <node-id> --bundle out/context-name
```

`context` is target-first. It is not an open chat endpoint.

### I changed files and need likely impact

```powershell
codestory-cli index --project <target-workspace> --refresh incremental
codestory-cli affected --project <target-workspace> --format markdown
git diff --name-only HEAD | codestory-cli affected --project <target-workspace> --stdin --format json
```

Treat `affected` as test-selection evidence, not a replacement for tests.

### The cache or retrieval looks stale

```powershell
codestory-cli doctor --project <target-workspace>
codestory-cli index --project <target-workspace> --refresh full
codestory-cli doctor --project <target-workspace>
```

If `doctor` reports stale inventory, semantic contract mismatch, missing managed
assets, or a fallback retrieval mode, fix that layer before investigating answer
quality. Treat the health report as the first source of truth for cache and
retrieval state.

## Core Commands

- `doctor`: read-only health check for project, cache, index, retrieval, and
  environment readiness.
- `index`: build or refresh the SQLite graph, snapshots, search state, and
  semantic docs.
- `ground`: broad repo-level orientation snapshot; `--why` explains retrieval
  mode, coverage, gaps, and next commands.
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

## Retrieval Defaults

Hybrid retrieval is the intended default when local embedding assets are
available. If they are unavailable, CodeStory falls back to symbolic or lexical
ranking and reports the fallback reason.

Managed setup:

```powershell
codestory-cli setup embeddings --project <target-workspace> --dry-run --format json
codestory-cli setup embeddings --project <target-workspace>
codestory-cli index --project <target-workspace> --refresh full
codestory-cli doctor --project <target-workspace>
```

Useful environment knobs:

- `CODESTORY_HYBRID_RETRIEVAL_ENABLED=false`: lexical-only mode.
- `CODESTORY_EMBED_RUNTIME_MODE=hash`: fast local development semantics.
- `CODESTORY_EMBED_BACKEND=onnx`, `llamacpp`, or `hash`: backend selection.
- `CODESTORY_EMBED_PROFILE=bge-base-en-v1.5`: default managed profile unless
  overridden.
- `CODESTORY_SEMANTIC_DOC_SCOPE=all`: include lower-signal symbols while
  investigating.
- `CODESTORY_LLM_DOC_EMBED_BATCH_SIZE=<n>`: override only while profiling.

`index`, `ground`, `search`, `context`, and `doctor` report retrieval mode and
fallback notes when retrieval state is available.

## Workspace And Config

CodeStory supports an optional `codestory_workspace.json` file at the repository
root for monorepo sessions:

```json
{
  "members": ["backend/", "frontend/", "shared/"]
}
```

Team or user defaults can live in `.codestory.toml` at the project root or in
the user home directory. The home file loads first, the project file overrides
it, and explicit environment variables still win.

Example:

```toml
embedding_profile = "bge-base-en-v1.5"
embedding_model_id = "BAAI/bge-base-en-v1.5-local"
hybrid_retrieval_enabled = true
```

`semantic_doc_scope` is intentionally omitted above because durable semantic
docs are the default. Set it only when opting into the broader all-symbol scope;
accepted all-symbol values are `all`, `full`, `all-symbols`, and `all_symbols`.
Other values currently resolve to the durable default.

## Cache Recovery

Typical recovery flow:

```powershell
codestory-cli doctor --project <target-workspace>
codestory-cli index --project <target-workspace> --refresh full
codestory-cli search --project <target-workspace> --query WorkspaceIndexer
```

If the cache directory itself is suspect, get the exact project cache path from
`doctor`, verify that it is under the CodeStory cache root, move it aside first,
then rebuild. Remove the backup only after the fresh index is healthy:

```powershell
$cacheDir = "<project-cache-dir-from-doctor>"
$cacheRoot = Join-Path $env:LOCALAPPDATA "CodeStory"
$resolvedCache = (Resolve-Path -LiteralPath $cacheDir).Path
$resolvedRoot = (Resolve-Path -LiteralPath $cacheRoot).Path
$relative = [System.IO.Path]::GetRelativePath($resolvedRoot, $resolvedCache)
if ($relative.StartsWith("..") -or [System.IO.Path]::IsPathRooted($relative)) {
  throw "Refusing to touch cache outside CodeStory cache root: $resolvedCache"
}
$backup = "$resolvedCache.bak-$(Get-Date -Format yyyyMMddHHmmss)"
Rename-Item -LiteralPath $resolvedCache -NewName (Split-Path -Leaf $backup)
codestory-cli index --project <target-workspace> --refresh full
codestory-cli doctor --project <target-workspace>
Remove-Item -LiteralPath $backup -Recurse -Force
```

Low-memory guidance:

- Prefer `index --refresh incremental` over repeated full refreshes.
- Avoid running multiple Cargo commands at once in this repo.
- If embedding assets are unavailable or too heavy, symbolic retrieval remains
  supported and is reported explicitly.
- If a cold index is slow, inspect semantic timing before changing parser or
  graph code.

## Verification

Run Cargo commands serially in this repo:

```powershell
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
```

Focused docs/onboarding lane:

```powershell
cargo test -p codestory-cli --test onboarding_contracts
```

Release-blocking fidelity lanes:

```powershell
cargo test -p codestory-indexer --test fidelity_regression
cargo test -p codestory-indexer --test tictactoe_language_coverage
cargo test -p codestory-runtime --test retrieval_eval
```

Heavy repo-scale timing lane:

```powershell
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
- [testing/benchmark-results.md](testing/benchmark-results.md)
- [testing/codestory-stdio-warm-loop-stats.md](testing/codestory-stdio-warm-loop-stats.md)
