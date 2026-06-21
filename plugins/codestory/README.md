# CodeStory Plugin Package

This directory is the Codex plugin source for CodeStory. CodeStory owns this
package; the plugin marketplace catalog belongs in
`TheGreenCedar/AgentPluginMarketplace` with catalog name `TheGreenCedar`.

The package is intentionally thin:

- `.codex-plugin/plugin.json` describes the plugin.
- `.mcp.json` launches `codestory-cli serve --stdio --refresh none` directly
  from the host workspace.
- `skills/codestory-grounding/SKILL.md` tells Codex how to use readiness,
  tools, and resources.

There is no adapter runtime in this package. Indexing, retrieval, packet,
search, and sidecar repair remain `codestory-cli` responsibilities.

## Install Prerequisite

Install `codestory-cli` for the host OS and make it available on `PATH`.

| OS | Setup |
| --- | --- |
| Windows | Download `codestory-cli-v0.11.0-windows-x64.zip` or `codestory-cli-v0.11.0-windows-arm64.zip`, or run `powershell -ExecutionPolicy Bypass -File scripts/install-codestory.ps1` from a CodeStory checkout. |
| macOS | Download `codestory-cli-v0.11.0-macos-arm64.tar.gz`, place `codestory-cli` on `PATH`, and run `chmod +x codestory-cli` if needed. macOS x64 should use the source fallback until a matching asset exists. |
| Linux | Download `codestory-cli-v0.11.0-linux-x64.tar.gz` or `codestory-cli-v0.11.0-linux-arm64.tar.gz`, place `codestory-cli` on `PATH`, and run `chmod +x codestory-cli` if needed. |

Verify downloaded archives against `SHA256SUMS.txt`. Source fallback for any
OS: build CodeStory and add `target/release` to `PATH`.

## Review Checks

```powershell
python C:\Users\alber\.codex\skills\.system\plugin-creator\scripts\validate_plugin.py plugins\codestory
node --test plugins\codestory\tests\plugin-static.test.mjs
git diff --check
```

Packet/search is ready only when `codestory://status` reports strict
`retrieval_mode=full`. Otherwise use the repair commands from the status
resource and do not make packet/search-backed claims.
