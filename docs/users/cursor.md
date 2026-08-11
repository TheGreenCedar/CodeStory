# Cursor

CodeStory ships as a Cursor plugin. It installs the grounding rule, skill,
session-start context, and managed runtime launcher together.

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

On the first call, the launcher fetches the matching CodeStory runtime if it is
not already installed, then prepares the repository. Cursor should retry the
same tool after its reported delay. A healthy answer cites real files and
symbols; if MCP is unavailable, the agent
uses ordinary source inspection and says that CodeStory evidence was not
available.

## Update

Refresh **codestory** in Customize, reload the Cursor window, and start a fresh
agent session. The plugin refresh replaces the rule, skill, hook, and launcher;
the launcher then selects the matching managed runtime.

## Team distribution

For a Teams or Enterprise workspace, an administrator opens **Dashboard →
Plugins → Team Marketplaces → Add Marketplace → Import from Repo**, then selects
this repository. Cursor's repository access settings must grant the workspace
access to the repository; private repositories also need the corresponding
organization or repository permission. The marketplace manifest at
[`.cursor-plugin/marketplace.json`](../../.cursor-plugin/marketplace.json)
points Cursor at the package under `plugins/codestory`.

Enable **Auto Refresh** for automatic marketplace updates, or use **Refresh**
from the Team Marketplaces dashboard after a repository update. This refreshes
the team marketplace catalog. Individual users still refresh the installed
plugin from Customize and reload Cursor. Publication in the public Cursor
Marketplace is a separate maintainer step.

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

From a clean committed CodeStory checkout, run:

```sh
node scripts/install-codestory-cursor-plugin.mjs \
  --cli "$(pwd)/target/release/codestory-cli"
```

Without `--cli`, the plugin uses the version-matched managed runtime.

## Troubleshooting

| Symptom | Action |
| --- | --- |
| Plugin is installed but tools are missing | Enable **codestory** MCP in Customize and reload the window |
| MCP fails to start | Confirm `node` is on `PATH`, then refresh the plugin |
| Runtime is stale after an update | Refresh the plugin, reload Cursor, and start a fresh session |
| A tool remains preparing | Retry that same tool after its returned delay |

Shared first-use behavior: [User guide](README.md#first-use). Readiness and
cache recovery: [Troubleshooting](troubleshooting.md).
