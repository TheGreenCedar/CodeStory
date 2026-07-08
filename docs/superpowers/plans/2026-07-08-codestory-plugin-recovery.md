# CodeStory Plugin Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` or `superpowers:subagent-driven-development` to implement this plan task-by-task. Keep checkbox state current as work lands.

**Goal:** Restore plugin-first CodeStory grounding after the 0.13.11 release by fixing the MCP project-root bind failure and the Windows native semantic sidecar lifetime failure.

**Architecture:** The CodeStory plugin is a thin Node MCP launcher around the managed `codestory-cli` binary. The launcher decides the target project root, then starts `codestory-cli serve --stdio --project <root>`. The Rust CLI owns graph refresh, sidecar setup, packet/search readiness, and retrieval status. The recovery keeps that boundary: the wrapper should only bind or diagnose the runtime, while the CLI remains the source of retrieval behavior.

**Tech Stack:** Node.js plugin launcher and hooks, MCP stdio, Rust 2024 workspace crates, Windows native `llama.cpp` embeddings, Qdrant, Zoekt, SQLite graph cache.

---

## Evidence Snapshot

- Live `codestory://status` reports plugin/runtime `0.13.11`, managed CLI path under `C:\Users\alber\.codex\plugins\data\codestory-TheGreenCedar`, `project_root: null`, `project_root_source: plugin_active_state_stale`, and all grounding surfaces blocked with `project_root_unavailable`.
- The plugin active-state file later refreshed to `C:\Users\alber\source\repos\codestory`, but the already-running fail-open MCP server continued to report the stale startup decision. This makes the MCP failure a wrapper lifecycle problem, not a CLI graph problem.
- Managed CLI with explicit `--project C:\Users\alber\source\repos\codestory` can repair local graph readiness and run `ground`.
- `ready --goal agent --repair --run-id shared-agent` can temporarily report `retrieval_mode=full`, but immediate `retrieval status` degrades to `no_semantic` because the native embedding endpoint refuses connections on the recorded port.
- The native `llama-server.exe` log shows successful startup and listening during repair, while `Get-Process -Name llama-server` is empty after the command exits. Qdrant and Zoekt containers remain running.
- Today-facing source history points at `2db3c20b fix windows native sidecar repair` and the later `0.13.11` release as the sidecar regression window.

## Workstreams

### A. MCP Project Root Binding

- [x] Add a focused regression test in `plugins/codestory/tests/plugin-static.test.mjs` for the startup ordering that failed here: MCP starts from the plugin root with stale or missing active state, then a hook writes fresh global active state for the current repo.
- [x] Decide and implement the least surprising recovery behavior in `plugins/codestory/scripts/codestory-mcp.cjs`:
  - If fail-open mode is protocol-bound and cannot become a normal MCP server, make `codestory://status` re-read active state live and report `project_root_available_after_launch` plus `restart_required`, instead of freezing `plugin_active_state_stale`.
  - If protocol-safe, let fail-open mode lazily re-resolve active state and start/proxy the real stdio CLI once a fresh root appears.
- [x] Keep the wrapper thin: do not duplicate graph or retrieval logic in Node. Normal grounding tools should still come from `codestory-cli serve --stdio --project <root>`.
- [x] Expose enough diagnostic fields in fail-open status to debug this without filesystem spelunking: active state path, active state timestamp, thread id observed by the MCP process, thread-state path, launch cwd, runtime cwd, and the startup-vs-current root decision.
- [x] Update hook or launcher tests if the real failure is an env mismatch between hook `PLUGIN_DATA` and MCP `PLUGIN_DATA`, or a missing `CODEX_THREAD_ID`. Current proof points to stale fail-open startup state rather than env mismatch; the launcher regression now covers stale active state becoming fresh after MCP startup.

### B. Windows Native Semantic Sidecar Lifetime

- [x] Add a narrow test around native embedding launch configuration in `crates/codestory-retrieval/src/compose.rs` that captures Windows process-detach intent without requiring a real model.
- [x] Refactor native launch construction so Windows creation flags are explicit and testable. The expected behavior is that the embedding server can outlive `ready --goal agent --repair` when the sidecar state records a native spawned endpoint.
- [x] Investigate whether the Codex host or PowerShell job object requires `CREATE_BREAKAWAY_FROM_JOB` in addition to `DETACHED_PROCESS`, `CREATE_NEW_PROCESS_GROUP`, and `CREATE_NO_WINDOW`. If breakaway is unavailable, fail clearly and fall back to a durable backend rather than recording a dead native endpoint.
- [x] After launch, perform a post-repair semantic smoke check that proves the recorded embedding endpoint is still reachable after bootstrap completes. Do not write a `full` manifest if the native process is already gone or cannot be reattached.
- [x] Keep Qdrant/Zoekt evidence separate from semantic evidence in `retrieval status`; `no_semantic` with healthy Qdrant/Zoekt is not full agent packet/search readiness.

### C. Operator Surfaces And Release Hygiene

- [x] Update `CHANGELOG.md` under `Unreleased` before committing behavior, release, packaging, or operator-guidance changes.
- [x] Update `docs/testing/codestory-plugin-friction-2026-07-07.md` as each hypothesis is confirmed or rejected.
- [x] If plugin source or packaged behavior changes, verify whether `TheGreenCedar/AgentPluginMarketplace` needs a corresponding pointer or metadata update before treating Codex plugin pickup as complete. This PR is verified through the local `CodeStoryLocal` override rather than a released marketplace pointer; marketplace follow-through is still required when the plugin release is published.
- [x] Keep current-state proof separate from source proof: source changes are not enough until the managed plugin runtime or explicit target binary shows the fixed behavior.
- [x] Keep MCP repair calls bounded: `sidecar_setup repair` starts Rust repair in background and returns status inspection guidance instead of blocking the stdio request through long sidecar rebuilds.

## Verification Plan

- [x] `node --test plugins/codestory/tests/plugin-static.test.mjs`
- [x] `cargo test -p codestory-retrieval --lib native_embedding`
- [x] `cargo build --release -p codestory-cli`
- [x] `target\release\codestory-cli.exe ready --goal local --repair --project C:\Users\alber\source\repos\codestory --format json`
- [x] `target\release\codestory-cli.exe ready --goal agent --repair --project C:\Users\alber\source\repos\codestory --format json --run-id shared-agent`
- [x] `target\release\codestory-cli.exe retrieval status --project C:\Users\alber\source\repos\codestory --profile agent --run-id shared-agent --format json`
- [x] `cargo test -p codestory-cli --test stdio_protocol_contracts tools_call_sidecar_setup_updates_plugin_policy_without_cli_user_steps -- --nocapture`
- [x] Fresh release-stdio smoke: `sidecar_setup repair` returned `status=started`, `mode=background`; delayed `retrieval status --profile agent --run-id shared-agent` reported `retrieval_mode=full`.
- [x] Fresh Codex host or plugin reload: read `codestory://status` and confirm `project_root` is the repo path, local grounding surfaces are allowed, and packet/search readiness reflects sidecar truth.

## Stop Conditions

- Stop and split work if MCP root binding and sidecar lifetime require unrelated release vehicles.
- Stop before claiming plugin recovery if the fixed source binary has not been exercised through the same managed runtime surface Codex will actually use.
- Stop before closing any issue if packet/search readiness is semantic-only fallback, `no_semantic`, or inferred from local graph freshness alone.
