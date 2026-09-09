# Installed navigation comparisons

The `--installed-navigation` profile in `scripts/codestory-agent-ab-benchmark.mjs`
compares ordinary installed CodeStory plugins with native repository tools. It
uses Terra at low reasoning, the same workspace-write sandbox in each arm, and a
ten-minute session timeout. It adds no investigation prelude or approval override.
Authenticated accounts can discover remote plugins even with an empty isolated
Codex home. This profile passes `--disable remote_plugin` to inventory and model
execution in every arm. It records that setting explicitly; installed local
CodeStory plugins and native source-reading tools remain available.
Historical benchmark profiles retain their existing contracts.

An independent evaluator freezes the task manifest and keeps answer keys outside
it. The manifest names pinned complete repository clones, tasks, arms, repeats,
and the complete ordered session schedule. The maintenance profile requires three
repositories, six tasks, two repeats and three arms; discovery, relationships and
edit/refresh each receive two tasks.

Prepare isolated Codex homes using normal marketplace installation. A native
home has no plugins; each CodeStory home has exactly one enabled plugin. Keep
these templates free of sessions, memories and host instructions. Record the
installed package digest, exact source commit, executable digest and ordered
`tools/list` digest in the installation receipt. Source-build development
receipts and published archives are different provenance tiers; identify them
literally. A local mirror may supply authenticated release archives without
changing the launcher. Templates must not contain binary or custom-server overrides.

The runner registers the authenticated installed launcher explicitly in each
CodeStory arm's Codex configuration. It preserves the installed command, arguments,
working directory and timeouts, and forwards only isolated infrastructure settings.
The plugin stays enabled for its unchanged guidance and hooks; only its duplicate
MCP registration is disabled. This tests an explicit host registration of the
installed launcher, not the default plugin registration's environment forwarding.
The latter can discard parent variables, including the archive mirror and private
embedding namespace. Host-environment observation supports macOS and Linux;
other hosts fail preflight before model execution.

The installation JSON binds to the task manifest's SHA-256, contains the `arms`
map and freezes `budget.max_sessions` and `budget.max_wall_ms`. A pilot can also
set its original `budget.deadline_utc`. Each arm names `codex_home_template`;
CodeStory arms additionally name `marketplace_name`, `plugin_relative_path`,
`package_sha256`, `source_commit`, `runtime_sha256`, `runtime_source`,
`schema_version`, `version`, `tools_sha256` and optionally
`release_directory`. An optional `auth_file` is copied privately into isolated
homes; credentials are never written into results.

Run deterministic checks before model execution:

```sh
node scripts/codestory-agent-ab-benchmark.mjs --installed-navigation \
  --manifest /absolute/tasks.json --installations /absolute/installations.json \
  --out /absolute/new-preflight-directory --preflight-only
```

Omit `--preflight-only` and use a new output directory for the comparison. Every
arm must pass installation, launcher, schema and edit/refresh canaries before any
model starts. Canaries use Codex's zero-model app-server MCP path and verify the
actual child environment, full retrieval, native identity, private native
completion with CPU fallback disabled, and changed source after refresh. Each
participant also gets an environment and catalog check without activating its
project. Sessions get fresh whole checkouts and state, including separate
embedding qualification namespaces. The runner records prompts, effective
configuration, package identity, canary traffic, model transcripts, usage, timing
and failures. Every scheduled session has a durable row, including preparation,
spawn and budget failures. Output directories are never overwritten or silently resumed.
Managed-runtime provisioning and identity failures are recorded separately from
model exit status. Two equivalent runtime failures stop the remaining attempts.
Independent correctness grading and release acceptance follow execution; process
success alone does not establish either.

Focused verification:

```sh
node --test scripts/tests/installed-navigation-profile.test.mjs
node scripts/codestory-agent-ab-benchmark.mjs --self-test
```
