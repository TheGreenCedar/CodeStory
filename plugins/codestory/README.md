# CodeStory Plugin Package

This directory is the Codex plugin source for CodeStory. CodeStory owns this
package; the plugin marketplace catalog is expected to live separately in
`TheGreenCedar/AgentPluginMarketplace` with catalog name `TheGreenCedar`.

The package is intentionally thin:

- `.codex-plugin/plugin.json` describes the plugin.
- `.mcp.json` registers the local MCP server.
- `scripts/codestory-mcp.mjs` launches `codestory-cli serve --stdio`.
- `skills/codestory-grounding/SKILL.md` tells Codex how to use the status,
  tools, and readiness contract.

The launcher does not index repositories, repair sidecars, or implement packet
or search logic. It delegates those paths to `codestory-cli`.

## Review Checks

```powershell
python C:\Users\alber\.codex\skills\.system\plugin-creator\scripts\validate_plugin.py plugins\codestory
node --test plugins\codestory\tests\plugin-static.test.mjs
git diff --check
```

Packet/search is ready only when `codestory://status` reports strict
`retrieval_mode=full`. Otherwise use the repair commands from the status
resource and do not make packet/search-backed claims.
