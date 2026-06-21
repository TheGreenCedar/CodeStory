---
name: codestory-grounding
description: Use when an agent should ground a local repository with CodeStory before making source claims, planning edits, choosing tests, reviewing changes, or using packet/search evidence through the CodeStory plugin or codestory-cli.
---

# CodeStory Grounding

Use CodeStory as the agent's local grounding surface. Prefer the MCP server when
the plugin is installed and healthy; use the CLI for setup, repair, explicit
transcripts, or when MCP is unavailable.

The target is always a repository workspace. The CodeStory checkout is only the
tool source unless the user is editing CodeStory itself.

## Setup Gate

1. Resolve `<target-workspace>` explicitly. Do not infer the target from the
   current shell if the user named another repo.
2. Resolve `<codestory-cli>`:
   - prefer `CODESTORY_CLI` when set;
   - otherwise use `codestory-cli` on `PATH`;
   - otherwise use a nearby `target/release/codestory-cli*` from the current or
     sibling CodeStory checkout;
   - otherwise install the released binary for the host OS.
3. If `codestory-cli` is missing, download and unpack the matching release asset
   before asking the human to run manual commands. The archive names below are
   release-bound to CodeStory `v0.11.1`; use that version unless the user asks
   for another version:
   - Windows x64: `codestory-cli-v0.11.1-windows-x64.zip`
   - Windows arm64: `codestory-cli-v0.11.1-windows-arm64.zip`
   - macOS arm64: `codestory-cli-v0.11.1-macos-arm64.tar.gz`
   - Linux x64: `codestory-cli-v0.11.1-linux-x64.tar.gz`
   - Linux arm64: `codestory-cli-v0.11.1-linux-arm64.tar.gz`
   - macOS x64 or missing asset: Source fallback. Build from source.
4. Put the binary in a stable user bin directory, verify
   `codestory-cli --version`, and prefer checking `SHA256SUMS.txt` when the host
   has the tools. If `PATH` changed, say that the plugin MCP process may need a
   new agent thread to see it.
5. Use `scripts/setup.ps1` or `scripts/setup.sh` from this skill only for the
   source-build fallback or explicit source-artifact setup.

## MCP Loop

When the plugin MCP server is available:

1. Read `codestory://status`.
2. Read `codestory://agent-guide`.
3. Read `codestory://grounding` for a compact repo map before planning.
4. Use `packet` for broad repo questions only when status says
   `agent_packet_search` is ready and strict sidecar status reports
   `retrieval_mode=full`.
5. Use `search` to discover candidates only when packet/search is ready.
6. Use `symbol`, `trail`, `references`, `snippet`, and `context` after selecting
   a concrete target.

If status is degraded, fall back to direct source reads or CLI local-navigation
commands and label packet/search as blocked.

## CLI Loop

When MCP is unavailable or a transcript is needed, use the CLI directly:

1. `doctor --project <target-workspace>` for cache, index, freshness, and
   sidecar health.
2. `index --project <target-workspace> --refresh full` for a first index;
   `--refresh incremental` for normal repair.
3. `ground --project <target-workspace> --why` for compact orientation.
4. `search --project <target-workspace> --query ... --why` for candidate
   discovery after sidecars are full.
5. `context`, `symbol`, `trail --story --hide-speculative`, `snippet`, `files`,
   and `affected` for concrete source-backed follow-up.
6. `packet --project <target-workspace> --question ...` for broad answers only
   when packet/search readiness is full.

Always pass `--project <target-workspace>` explicitly.

## Evidence Rules

- Treat CodeStory output as evidence, not omniscience.
- When `packet` reports `sufficient` and has no `follow_up_commands`, answer
  from the packet and preserve its cited file/symbol anchors.
- When `packet` reports `partial`, run the named follow-up commands before
  making proof claims.
- Treat repo-text hits, semantic suggestions, fallback retrieval, stale caches,
  and missing sidecar manifests as navigation hints only.
- `retrieval_mode=full` means graph and lexical sidecars are complete, generated
  symbol docs/component reports are current, and dense anchors are valid for the
  selected corpus. Anything weaker is not product packet/search proof.
- Do not run broad reindexing, sidecar rebuilds, benchmarks, or Cargo builds in
  parallel with another noise-sensitive lane unless the user accepts the timing
  noise.

## Command Routing

- Setup and health: `setup embeddings`, `doctor`, `ready`, `index`, `cache`.
- Agent orientation: MCP `codestory://grounding` or CLI `ground`.
- Broad task packet: MCP/CLI `packet`.
- Candidate discovery: MCP/CLI `search --why`.
- Focused source view: `symbol`, `trail`, `snippet`, `context`, `explore`.
- Coverage and impact: `files`, `affected`.
- Reusable targets: `bookmark`.
- Structured evaluation: `drill`, `drill-suite`.
- Local integration surface: `serve --stdio`.

Load the matching reference only when detailed flags, examples, or
troubleshooting rules are needed:

- [index](references/index.md)
- [cache](references/cache.md)
- [ground](references/ground.md)
- [doctor](references/doctor.md)
- [packet](references/packet.md)
- [search](references/search.md)
- [context](references/context.md)
- [symbol](references/symbol.md)
- [trail](references/trail.md)
- [snippet](references/snippet.md)
- [drill](references/drill.md)
- [drill-suite](references/drill-suite.md)
- [query](references/query.md)
- [explore](references/explore.md)
- [files](references/files.md)
- [affected](references/affected.md)
- [bookmark](references/bookmark.md)
- [setup](references/setup.md)
- [retrieval-rollout](references/retrieval-rollout.md)
- [serve](references/serve.md)
