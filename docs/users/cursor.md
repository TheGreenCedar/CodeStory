# Cursor

CodeStory is a Cursor plugin: grounding rule, skill, session-start hook, and
managed runtime launcher. It is not listed on the public [Cursor
Marketplace](https://cursor.com/marketplace). Searching that catalog for
**codestory** will not find it.

Add this repository as a marketplace in Customize, then install **codestory**
from it.

## Requirements

- One of the supported platform and accelerator combinations below;
- Node.js on `PATH` for the plugin adapter; and
- a Cursor reload after installing or replacing the plugin package.

<!-- codestory-public-support:start -->
| Platform | Release support |
| --- | --- |
| macOS 15+ on Apple Silicon | Supported with Metal |
| Windows x64 | Supported with Vulkan |
| Linux x64 | Supported with Vulkan |
| CPU-only Windows and Linux | Unsupported |
| Intel Mac | Unsupported |
| Windows ARM | Unsupported |
<!-- codestory-public-support:end -->

The packaged CLI contains its model and embedding engine, so normal use needs
no Docker, external embedding endpoint, model download, or Xcode toolchain.
Building CodeStory from source has separate [contributor
prerequisites](../contributors/getting-started.md#prerequisites).

## Install

1. Open **Customize** in the Cursor sidebar.
2. Select **+ Add Marketplace**.
3. Import this repository:
   - **Import from Github** —
     [TheGreenCedar/CodeStory](https://github.com/TheGreenCedar/CodeStory); or
   - **Import from Disk** — a local clone of the same repository.
4. Install **codestory** from that marketplace, for your user or the current
   project.
5. Open the installed plugin and enable its **codestory** MCP server. Cursor
   requires this one-time toggle; the plugin cannot enable MCP on your behalf.
6. Reload the Cursor window if the server does not connect.

The import reads
[`.cursor-plugin/marketplace.json`](../../.cursor-plugin/marketplace.json),
which points Cursor at `plugins/codestory`. Do not use **Create New**; that
starts an empty marketplace instead of loading CodeStory.

Open the repository you want to ground. The plugin is not limited to the
CodeStory checkout; every CodeStory tool call still names that repository's
absolute `project` root.

Start a fresh agent chat. Cursor asks before running MCP tools by default;
approve the CodeStory calls, then ask:

```text
Where is request validation implemented, who calls it, and which tests cover it?
```

On the first call, the launcher fetches the matching CodeStory runtime if it is
not already installed, then prepares the repository. The agent should retry the
same tool after the reported delay. A healthy answer cites real files and
symbols. If MCP is unavailable, the agent uses ordinary source inspection and
says that CodeStory evidence was not available.

Shared first-use behavior: [User guide](README.md#first-use).

## Update

Refresh the CodeStory marketplace in Customize, then refresh or reinstall
**codestory** from it and reload the Cursor window. Start a fresh agent
session. The replacement covers the rule, skill, hook, and launcher; the
launcher then selects the matching managed runtime.

## Advanced: repository-managed setup

This is a rule-and-MCP-only mode; it does not install the plugin's grounding
skill or session hook. Teams using it must vendor the complete
`plugins/codestory` package before committing the rule and MCP configuration,
because the configuration alone does not contain the launcher. This repository's
[rule](../../.cursor/rules/codestory.mdc) and
[MCP configuration](../../.cursor/mcp.json) work because that complete package
is present at `plugins/codestory`. Keep the MCP command rooted at
`${workspaceFolder}` and do not add a repository-local plugin-data directory.

Cursor's MCP install deeplinks can add an MCP server for users who intentionally
choose manual setup, but they do not install the rule, skill, or session hook
and are not the primary CodeStory install path.

## Local plugin development

The installer below is only for dogfooding a checkout with an optional local
CLI. Normal installs use [Add Marketplace](#install).

From a clean committed CodeStory checkout:

```sh
node scripts/install-codestory-cursor-plugin.mjs
```

Pass `--cli "$(pwd)/target/release/codestory-cli"` to use that exact binary
instead of the managed download. The installer links `plugins/codestory` into
`~/.cursor/plugins/local/codestory` and writes any CLI override only into
Cursor's private CodeStory data directory. Reload Cursor, enable **codestory**
MCP in Customize, and start a fresh agent session.

## Troubleshooting

| Symptom | Action |
| --- | --- |
| **codestory** is missing from Customize | It is not on the public marketplace. [Add this repository as a marketplace](#install), then install **codestory** from it |
| Plugin is listed but tools are missing | Enable **codestory** MCP in Customize, approve the tool prompt, and reload the window |
| MCP fails to start | Confirm `node` is on `PATH`, then reload. Inspect **MCP Logs** in the Output panel (**Cmd+Shift+U** / **Ctrl+Shift+U**) |
| Runtime is stale after an update | Refresh the marketplace and the plugin in Customize, reload Cursor, and start a fresh session |
| A tool remains preparing | Retry that same tool after its returned delay |

Readiness and cache recovery: [Troubleshooting](troubleshooting.md).

## Differences from Codex

Codex installs from `/plugins` against the public **TheGreenCedar** catalog.
Cursor has no equivalent public listing: add this repository with **+ Add
Marketplace**, then install **codestory**. Once MCP is connected, repository
preparation and the per-user retrieval server behave the same as in Codex.
