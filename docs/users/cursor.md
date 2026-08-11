# Cursor

CodeStory ships as a Cursor plugin. It installs the grounding rule, skill,
session-start context, MCP launcher, and matching managed runtime together.

## Install

1. Open **Customize → Plugins** in Cursor and install **codestory** for your
   user or project.
2. Open the installed plugin in Customize and enable its **codestory** MCP
   server. Cursor requires this one-time toggle; plugins cannot enable MCP on
   your behalf.
3. Reload the Cursor window and open the repository root as the workspace.

Ask:

```text
Where is request validation implemented, who calls it, and which tests cover it?
```

The first call may install the matching CodeStory runtime and prepare the
repository. Cursor should retry the same tool after its reported delay. A
healthy answer cites real files and symbols; if MCP is unavailable, the agent
uses ordinary source inspection and says that CodeStory evidence was not
available.

## Update

Refresh **codestory** in Customize, reload the Cursor window, and start a fresh
agent session. The plugin refresh replaces the rule, skill, hook, and launcher;
the launcher then selects the matching managed runtime.

## Team distribution

Teams can use **Import from Repo** on the Cursor Dashboard and select this
repository. The repository marketplace manifest at
[`.cursor-plugin/marketplace.json`](../../.cursor-plugin/marketplace.json)
points Cursor at the publish-ready package under `plugins/codestory`.
Publication in the public Cursor Marketplace is a separate maintainer step.

## Advanced: repository-managed setup

Teams that do not use Cursor's plugin marketplace can commit a rule and MCP
configuration directly. This repository's [rule](../../.cursor/rules/codestory.mdc)
and [MCP configuration](../../.cursor/mcp.json) are working examples. Keep the
MCP command rooted at `${workspaceFolder}` and do not add a repository-local
plugin-data directory; the adapter infers Cursor's private per-user data path.

For local CodeStory development, run:

```sh
node scripts/install-codestory-cursor-plugin.mjs \
  --cli "$(pwd)/target/release/codestory-cli"
```

Without `--cli`, the plugin uses the version-matched managed runtime. Cursor's
MCP install deeplinks can add an MCP server for users who intentionally choose
manual setup, but they do not install the rule, skill, or session hook and are
not the primary CodeStory install path.

## Troubleshooting

| Symptom | Action |
| --- | --- |
| Plugin is installed but tools are missing | Enable **codestory** MCP in Customize and reload the window |
| MCP fails to start | Confirm `node` is on `PATH`, then refresh the plugin |
| Runtime is stale after an update | Refresh the plugin, reload Cursor, and start a fresh session |
| A tool remains preparing | Retry that same tool after its returned delay |

Shared first-use behavior: [User guide](README.md#first-use). Readiness and
cache recovery: [Troubleshooting](troubleshooting.md).
