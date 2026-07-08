# CodeStory plugin friction log - 2026-07-07

## Context

- Host cwd: `C:\Users\alber\source\repos\codestory`.
- Git state before the note: `## main...origin/main`, no local changes.
- Plugin requested through `[@codestory](plugin://codestory@TheGreenCedar)`.
- Live MCP discovery required `tool_search` for `codestory mcp ground status packet search`.

## Observations

| Surface | Result | Evidence |
| --- | --- | --- |
| `codestory://status` | MCP was visible, but repo binding failed. | `plugin_version=0.13.11`, managed CLI path under `plugins\data\codestory-TheGreenCedar`, `project_root=null`, `project_root_source=plugin_active_state_stale`, warning `project_root_unavailable`. |
| MCP tools | Only `sidecar_setup` was exposed. | A second `tool_search` after repair still did not expose `ground`, `packet`, or `search` tools. |
| `codestory://agent-guide` | Correctly pointed back to host restart/read status. | `status=repair_setup`; recommended calls were host restart and `codestory://status`. |
| Managed CLI with explicit `--project` | Worked around MCP repo-root failure. | `doctor` resolved `C:/Users/alber/source/repos/codestory` and found the cache. |
| Local graph repair | Successful. | `ready --goal local --repair` changed local navigation from stale to `ready`; index became fresh with 280/280 files and 0 errors. |
| Agent sidecar repair | Initially reported success, then degraded immediately. | `ready --goal agent --repair --run-id shared-agent` ended with `retrieval_mode=full`, but immediate `retrieval status` and `doctor` reported `retrieval_mode=no_semantic`. |
| Packet call | Correctly failed closed after sidecar degradation. | `packet` returned `retrieval_unavailable` because expected `profile=agent mode=full`, observed `mode=no_semantic`. |
| Native embedding process | Starts during repair, then is gone after the command exits. | `llama-server-native.log` shows `listening on http://127.0.0.1:37040`; later `Get-Process llama-server` found no process and `netstat` showed only `TIME_WAIT` for port 37040. |
| Qdrant/Zoekt sidecars | Stayed up. | `docker ps` showed `shared-agent-qdrant` and `shared-agent-zoekt` running. |
| MCP `sidecar_setup repair` | Did not perform fresh repair. | Returned state `enabled`, but `last_repair` still referenced an old `0.12.3` command from 2026-06-27. |

## Current state

- Installed/plugin-managed runtime is current for this session: `0.13.11`.
- Live MCP status remains blocked by `project_root_unavailable`; repaired CLI state did not update MCP active state.
- Local navigation via explicit managed CLI is usable.
- Agent packet/search must not be trusted: latest verified mode is `no_semantic`, with `embedding_runtime_unavailable` from refused connections to `http://127.0.0.1:37040/v1/embeddings`.

## Likely fix targets

1. MCP startup/root inference: preserve or recover the active target project root instead of reporting `plugin_active_state_stale` when the Codex thread cwd is the repo.
2. Native embedding process lifetime: ensure `llama-server.exe` survives beyond `ready --goal agent --repair` when `embedding_launch.launch_mode=native_spawned`, or make repair fail instead of briefly reporting `full`.
3. `sidecar_setup repair`: either run a current repair through the managed `0.13.11` binary or report that it only updates policy metadata.

## Follow-up source trace

- `git fetch origin --prune` on 2026-07-07 left local `main` matching `origin/main`.
- The MCP/CLI divergence is before shared Rust runtime logic: `plugins/codestory/scripts/codestory-mcp.cjs` resolves project root from explicit env/args, process cwd, thread active state, then global active state. If that fails, it starts diagnostic fail-open MCP with only `sidecar_setup`, `codestory://status`, and `codestory://agent-guide` instead of spawning `codestory-cli serve --stdio --project <root>`.
- The normal MCP handoff would share CLI logic: the launcher spawns the managed binary as `serve --stdio --refresh none --project <root>`, and Rust stdio dispatch then calls the same packet/search/grounding runtime handlers.
- After the failure, `.codestory-active` refreshed to `C:\Users\alber\source\repos\codestory`, but the already-running fail-open MCP process still reported the startup decision `plugin_active_state_stale`. That narrows the MCP issue to launch lifecycle or fail-open rebinding, not graph indexing.
- The semantic sidecar failure was introduced by commits pushed on 2026-07-07. The most relevant commit is `2db3c20b fix windows native sidecar repair`, later carried into `988478ff release 0.13.11`. It changed `crates/codestory-retrieval/src/compose.rs` and `crates/codestory-retrieval/src/embeddings.rs`.
- Current native spawn code starts `llama-server.exe` from the repair process and logs it, but there is no durable process supervision/detach boundary recorded there. Observed behavior on this machine: the server is reachable during repair, then no `llama-server` process remains after the command exits, so `retrieval status` downgrades to `no_semantic`.

## Non-claims

- This note does not prove a fresh host restart fixes MCP root binding.
- This note does not prove whether the embedding process exits voluntarily or is killed with the repair command parent.
- This note does not change product guidance; it records this session's evidence.

## Fix evidence on branch `codex/plugin-mcp-ux-fix`

- The plugin MCP wrapper now re-reads active project state in diagnostic fail-open mode and, when a project root appears after startup, hands the next MCP request to `codestory-cli serve --stdio` instead of freezing the stale `plugin_active_state_stale` response.
- `node --test plugins/codestory/tests/plugin-static.test.mjs` passed with a regression case where fail-open starts from stale state, the client initializes and sees only `sidecar_setup`, a fresh `.codestory-active` appears, and both `sidecar_setup repair` and a later `tools/list` are served by the delegated stdio runtime.
- The Windows native sidecar spawn path now applies detached process creation flags to `llama-server.exe`; `cargo test -p codestory-retrieval --lib native_embedding` passed the Windows flag regression test.
- `target\release\codestory-cli.exe ready --goal agent --repair --project C:\Users\alber\source\repos\codestory --format json --run-id shared-agent` returned `agent_packet_search` ready with `retrieval_mode=full`.
- Immediate and 20-second delayed `target\release\codestory-cli.exe retrieval status --project C:\Users\alber\source\repos\codestory --profile agent --run-id shared-agent --format json` both reported `retrieval_mode=full`.
- `Get-Process -Name llama-server` showed PID `45768`, and `netstat -ano` showed `127.0.0.1:37040` listening after the repair command exited.
- A fresh Codex CLI child could test the local plugin without publishing a new
  release after installing `codestory@CodeStoryLocal`, mirroring this worktree's
  `plugins/codestory` into the local marketplace, and setting
  `CODESTORY_CLI=C:\Users\alber\source\repos\codestory\target\release\codestory-cli.exe`
  inside the local plugin `.mcp.json` env block.
- Plain parent-process `CODESTORY_CLI` and
  `-c shell_environment_policy.inherit=all` did not affect the plugin MCP
  runtime. The fresh child still reported `runtime_source=managed` until the
  local plugin `.mcp.json` carried the override. A partial
  `-c mcp_servers.codestory.env.CODESTORY_CLI=...` override failed with
  `invalid transport`.
- With the local manifest override, fresh `codex.cmd exec` read
  `codestory://status` through `codestory@CodeStoryLocal` and reported
  `runtime_source=local_dev_override`,
  `server_executable=C:/Users/alber/source/repos/codestory/target/release/codestory-cli.exe`,
  and model-visible tools including `ground`, `search`, `packet`,
  `sidecar_setup`, and `repair_all`.
- A local Codex CLI child ran MCP `sidecar_setup repair`, then reread
  `codestory://status`; repair returned `exit_code=0`, `retrieval_mode=full`,
  `packet.allowed=true`, `search.allowed=true`, and `blocked_reason=null`.
- Repeated MCP repair exposed a second native-process friction point: pre-fix
  repairs had already left two persistent `llama-server.exe` processes for port
  `37040`. After adding the healthy-endpoint reuse guard and rebuilding
  `target\release\codestory-cli.exe`, a later MCP repair logged
  `reusing existing native llama.cpp embedding server: llama.cpp embeddings reachable dim=768`
  and status remained `retrieval_mode=full`. No new persistent server process
  remained after the transient child activity ended.
- After amending the plan doc, `codestory://status` correctly failed closed with
  `sidecar_manifest_stale`, but MCP `sidecar_setup repair` ran in foreground
  long enough to hit the host's 300-second tool timeout. The Rust stdio runtime
  now starts that repair in background and returns `started` or
  `already_running` with a `retrieval status` command instead of blocking the
  MCP request.
- Fresh release-stdio smoke with the rebuilt local binary returned
  `sidecar_setup repair` as `status=started`, `mode=background`, `pid=37900`;
  the background repair exited on its own, and a delayed
  `retrieval status --profile agent --run-id shared-agent` reported
  `retrieval_mode=full` with healthy Zoekt, Qdrant `points_count=940`, and SCIP
  artifacts.

## Local Codex CLI friction captured

- `codex.cmd exec` rejected the top-level CLI shorthand `-a never` with
  `unexpected argument '-a'`; using
  `--dangerously-bypass-approvals-and-sandbox` worked for the automation smoke.
- Having both `codestory@TheGreenCedar` and `codestory@CodeStoryLocal`
  installed produced duplicate MCP server warnings. The resolver selected
  `codestory@CodeStoryLocal` for `server="codestory"` when the prompt used
  `plugin://codestory@CodeStoryLocal`, but the warning is noisy and should be
  treated as operator friction.
- Fresh child sessions repeatedly emitted unrelated plugin-loader noise
  (`chatgpt-apps@openai-curated` not installed, remote installed bundle sync
  failures, shortened skill descriptions). The CodeStory MCP signal was still
  usable, but the raw `--json` stream is noisy for plugin recovery work.
