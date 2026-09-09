# Installed navigation comparisons

The `--installed-navigation` profile in `scripts/codestory-agent-ab-benchmark.mjs`
compares ordinary installed CodeStory plugins with native repository tools. It
uses Terra at low reasoning, the same workspace-write sandbox in each arm, and a
ten-minute session timeout. It adds no investigation prelude or approval override.
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
changing the launcher. Binary and custom MCP-server overrides are unsupported.

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
model starts. Sessions get fresh whole checkouts and state, including separate
embedding qualification namespaces. The runner records prompts, effective
configuration, package identity, canary traffic, model transcripts, usage, timing
and failures. Every scheduled session has a durable row, including preparation,
spawn and budget failures. Output directories are never overwritten or silently resumed.
Independent correctness grading and release acceptance follow execution; process
success alone does not establish either.

Focused verification:

```sh
node --test scripts/installed-navigation-profile.test.mjs
node scripts/codestory-agent-ab-benchmark.mjs --self-test
```
