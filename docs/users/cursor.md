# Cursor

CodeStory is a Cursor plugin: grounding rule, skill, session-start hook, and
managed runtime launcher. Cursor shows it in **Customize** only after you load
that package onto the machine or your team imports this repository as a
marketplace. It is not listed on the public [Cursor
Marketplace](https://cursor.com/marketplace). Searching Customize's official
catalog for **codestory** will not find it.

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

## Make CodeStory visible in Customize

Pick the path that matches how you get software.

### Personal install

This is the path for an individual machine. It links the shipped plugin into
Cursor's local plugin directory so **Customize** can list it.

1. Clone [TheGreenCedar/CodeStory](https://github.com/TheGreenCedar/CodeStory)
   and check out the release you want, or use a clone whose `plugins/codestory`
   tree is clean and committed.
2. From that repository root, run:

   ```sh
   node scripts/install-codestory-cursor-plugin.mjs
   ```

   The installer links `plugins/codestory` to
   `~/.cursor/plugins/local/codestory` (on Windows,
   `%USERPROFILE%\.cursor\plugins\local\codestory`) and uses
   `~/.cursor/plugins/data/codestory` for plugin state. It refuses a dirty or
   incomplete plugin tree.
3. Reload the Cursor window (**Developer: Reload Window**).
4. Open **Customize** in the sidebar. **codestory** should appear as a local
   plugin.

Without `--cli`, the first MCP call downloads the version-matched signed
runtime. Use `--cli` only when dogfooding a local `codestory-cli` build; see
[Local plugin development](#local-plugin-development).

### Team marketplace

This is the path that makes CodeStory **browsable** in Customize for everyone
on a Teams or Enterprise workspace.

An administrator opens **Dashboard → Plugins → Team Marketplaces → Add
Marketplace → Import from Repo** and selects this repository. Cursor's
repository access settings must grant the workspace access to the repository;
private repositories also need the corresponding organization or repository
permission. The import reads
[`.cursor-plugin/marketplace.json`](../../.cursor-plugin/marketplace.json),
which points at `plugins/codestory`.

Enable **Auto Refresh** for automatic catalog updates, or use **Refresh** from
the Team Marketplaces dashboard after a repository update. That refresh updates
the catalog only.

After the import, each developer:

1. Opens **Customize → Plugins**.
2. Finds **codestory** from the team marketplace.
3. Selects **Install** and chooses a user or project scope.

Publication on cursor.com/marketplace is a separate maintainer step and is not
an install path today.

## Enable MCP and verify

Cursor will not start CodeStory tools until you enable the plugin's MCP server.
Plugins cannot flip that toggle for you.

1. In **Customize**, open the installed **codestory** plugin and enable its
   **codestory** MCP server.
2. Reload the Cursor window if the server does not connect.
3. Open the repository you want to ground. The plugin is not limited to the
   CodeStory checkout; every CodeStory tool call still names that repository's
   absolute `project` root.
4. Start a fresh agent chat. Cursor asks before running MCP tools by default;
   approve the CodeStory calls.
5. Ask:

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

How you refresh depends on how the plugin was loaded:

- **Personal install:** update the clone, keep `plugins/codestory` clean and
  committed, re-run the installer if the local link is missing, then reload
  Cursor.
- **Team marketplace:** an administrator refreshes the team catalog, then each
  user refreshes **codestory** in Customize and reloads Cursor.

Start a fresh agent session after reload. The replacement covers the rule,
skill, hook, and launcher; the launcher then selects the matching managed
runtime.

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

From a clean committed CodeStory checkout, pass a local CLI only when you need
that exact binary instead of the managed download:

```sh
node scripts/install-codestory-cursor-plugin.mjs \
  --cli "$(pwd)/target/release/codestory-cli"
```

Without `--cli`, the plugin uses the version-matched managed runtime. Reload
Cursor, enable **codestory** MCP in Customize, and start a fresh agent session.

## Troubleshooting

| Symptom | Action |
| --- | --- |
| **codestory** is missing from Customize | It is not on the public marketplace. Run the [personal installer](#personal-install) or ask an admin to [import this repository](#team-marketplace) |
| Plugin is listed but tools are missing | Enable **codestory** MCP in Customize, approve the tool prompt, and reload the window |
| MCP fails to start | Confirm `node` is on `PATH`, then reload. Inspect **MCP Logs** in the Output panel (**Cmd+Shift+U** / **Ctrl+Shift+U**) |
| Runtime is stale after an update | Refresh the plugin the same way you installed it, reload Cursor, and start a fresh session |
| A tool remains preparing | Retry that same tool after its returned delay |

Readiness and cache recovery: [Troubleshooting](troubleshooting.md).

## Differences from Codex

Codex installs from `/plugins` against the public **TheGreenCedar** catalog.
Cursor has no equivalent public listing today: load the plugin locally, or
install it from a team marketplace that imported this repository. Once MCP is
connected, repository preparation and the per-user retrieval server behave the
same as in Codex.
