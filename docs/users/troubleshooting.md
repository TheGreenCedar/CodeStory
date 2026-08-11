# Troubleshooting

Start with the failed CodeStory call. Most first-use and refresh states resolve
by retrying that same tool; status and CLI commands are diagnostics, not setup
rituals.

## Fast path

```mermaid
flowchart TD
  call["Call the tool for the repository question"] --> state{"Result"}
  state -->|"ready"| use["Use the cited evidence"]
  state -->|"preparing or updating"| wait["Wait for retry_after_ms"]
  wait --> call
  state -->|"working_locally"| local["Use local graph tools and retry broad search later"]
  state -->|"unavailable"| fallback["Inspect source directly and report the gap"]
  call -->|"tool missing"| host["Restore the host MCP connection"]
```

| Symptom | First action | Healthy result |
| --- | --- | --- |
| Local files or symbols look stale | Retry `ground`, `files`, or the requested local graph tool | The tool cites the current checkout |
| Packet/search is preparing | Retry that exact tool after its returned delay | It returns cited evidence or a clear unavailable state |
| First use stays "preparing" for a long time | Nothing — the runtime is downloading; see [First use downloads the runtime](#first-use-downloads-the-runtime) | Progress reports rising bytes and percentage |
| First use fails with a download error | Retry the same tool; the transfer resumes from what it already has | The download completes and the tool answers |
| CodeStory tools are missing | Fix or reload the host MCP registration | A project-scoped CodeStory tool is visible |
| Update installed but old runtime is active | Replace the plugin package and start a fresh host | Fresh status identifies the new CLI |
| Automatic preparation becomes unavailable | Use local graph tools or source inspection; collect diagnostics only if needed | No unsupported broad-search claim is made |

Trust boundaries: [Trust and readiness](trust-and-readiness.md).

## Host connection problems

| Host | Check |
| --- | --- |
| Codex | Confirm **TheGreenCedar -> codestory** is installed, then start a fresh host session |
| Cursor | Confirm the **codestory** plugin is installed, enable its MCP server in Customize, then reload Cursor; inspect `.cursor/mcp.json` only for the advanced repository-managed setup |
| Claude Code | Confirm the plugin hook and the separately configured MCP server |
| Copilot | Hooks or instructions do not start MCP; configure it explicitly |

Host instructions: [Codex](codex.md), [Cursor](cursor.md),
[Claude Code](claude-code.md), and [Copilot](copilot.md).

CLI health does not prove that MCP is live inside the agent host. If the skill or
hook loaded but no CodeStory tools exist, fix the host connection rather than
rebuilding an index.

## Repository map is stale

Symptoms include missing symbols, deleted paths, or trails that disagree with
the checkout.

With MCP live, retry `ground`, `files`, or the requested local graph tool.
Project-scoped local tools refresh the map through the managed path.

For a maintainer transcript:

```sh
codestory-cli index --project <repo> --refresh auto --format json
codestory-cli doctor --project <repo>
```

Use a full rebuild only when diagnostics identify cache, schema, or publication
uncertainty:

```sh
codestory-cli index --project <repo> --refresh full --format json
```

Moving a project cache aside is a last-resort diagnostic. Get its exact path
from `doctor`, verify it is under the active CodeStory cache root, preserve the
old directory, and rebuild before removing anything.

## Broad search is preparing or unavailable

`packet`, `search`, and `context` require a coherent lexical, vector, and graph
publication.

- `preparing`: wait for `retry_after_ms` and retry the same tool.
- `working_locally`: continue with symbols, trails, and snippets while broad
  search prepares.
- `unavailable`: inspect source directly and state that broad search was not
  available. Do not treat partial or local-only output as a complete packet.

Maintainers can inspect the persisted state with:

```sh
codestory-cli retrieval status --project <repo> --format json
codestory-cli retrieval index --project <repo> --refresh full --format json
```

`retrieval_mode: "full"` is required before trusting broad-search evidence.
Backend, adapter, model, and smoke details are in the
[retrieval operations guide](../ops/retrieval-engine.md).

### macOS

Apple Silicon selects Metal automatically. The first broad request may take
longer while the embedded model initializes; later repositories reuse that warm
engine. Production does not silently switch to CPU when acceleration is
unavailable.

No macOS user needs to start a server, choose an endpoint, approve retrieval
infrastructure, or install a model.

### Windows and Linux

Both select Vulkan automatically and both need a working Vulkan driver for a
physical GPU. Production never silently falls back to CPU, so a missing or
software-only adapter leaves broad search unavailable rather than slow.

If broad search stays unavailable on a machine that has a GPU:

- Confirm the vendor GPU driver is installed and current. Vulkan arrives with
  the graphics driver, not with CodeStory.
- On Linux, confirm the Vulkan loader and the vendor ICD are present
  (`vulkaninfo --summary` lists the adapter your driver provides).
- Remote desktop, virtual machines, and containers frequently expose only a
  software adapter such as llvmpipe or SwiftShader. CodeStory rejects those on
  purpose; the answer is a session with real GPU access, not a configuration
  change.

Local graph tools keep working throughout. Only broad search depends on the
accelerator.

## First use downloads the runtime

The plugin ships as a small package and downloads the CodeStory runtime the
first time you use it. That download is a few hundred megabytes, because the
runtime carries the local model, and it happens once per version.

While it runs, tools answer with a preparing state that reports real bytes,
percentage, and attempt. On a slow link this legitimately takes a while.

If it is interrupted, the next attempt resumes from the bytes already on disk
rather than starting over, and partial downloads survive a restart of your agent
host. Retrying the same tool is the whole recovery procedure.

Three environment variables override the defaults if you need them:

| Variable | Default | Effect |
| --- | --- | --- |
| `CODESTORY_PLUGIN_DOWNLOAD_STALL_TIMEOUT_MS` | 60000 | How long a silent connection is tolerated before the attempt fails |
| `CODESTORY_PLUGIN_DOWNLOAD_TIMEOUT_MS` | 3600000 | Ceiling on the whole download, across attempts |
| `CODESTORY_PLUGIN_DOWNLOAD_ATTEMPTS` | 20 | How many resuming attempts before giving up |

## Update and runtime drift

`runtime_update.state=available` is advisory while the current CLI remains
compatible. Restart when `restart_recommended=true`.

`repair_setup` is a wire-state name for an actual managed CLI launch or
compatibility failure. It does not mean the user must repair an embedding
service. Follow `recommended_next_calls`, replace the plugin package if needed,
and confirm the change in a fresh host session.

For Codex, marketplace refresh, package refresh, and runtime reload are distinct.
On Windows builds that expose terminal management:

```powershell
codex.cmd plugin marketplace upgrade TheGreenCedar
codex.cmd plugin add codestory@TheGreenCedar
```

The first command refreshes only the catalog. The second replaces the installed
package. Close stale Codex windows before replacement if Windows reports
`Access is denied`, then start a fresh host session.

For local development, set `CODESTORY_CLI` to the exact built binary. Status
labels this path `local_dev_override`.

## Diagnostic transcript

Use these only after the normal tool loop fails to converge or when a maintainer
needs structured evidence:

```sh
codestory-cli agent preflight --project <repo> --format json
codestory-cli doctor --project <repo>
codestory-cli retrieval status --project <repo> --format json
```

When MCP is live, the project-bound `codestory://status{?project}` resource is
the authoritative host-visible
diagnostic. It is observational and does not start indexing or engine work.

Further detail:

- [CLI reference](cli-reference.md)
- [Contributor debugging](../contributors/debugging.md)
- [Agent status contract](../../plugins/codestory/skills/codestory-grounding/references/status-contract.md)
