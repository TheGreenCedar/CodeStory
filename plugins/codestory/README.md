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

The plugin launches `codestory-cli` directly from the Codex host `PATH`.
Install `codestory-cli` for that host OS, then start a new Codex thread after
changing `PATH` so the plugin process can see it.

The archive names below are release-bound to CodeStory `v0.11.1`.

| Host OS | Setup |
| --- | --- |
| Windows x64 | Download `codestory-cli-v0.11.1-windows-x64.zip`, or run `powershell -ExecutionPolicy Bypass -File scripts/install-codestory.ps1` from a CodeStory checkout. The helper's automatic download path is Windows x64 only. |
| Windows arm64 | Download `codestory-cli-v0.11.1-windows-arm64.zip`, extract it, and put `codestory-cli.exe` on `PATH`. |
| macOS arm64 | Download `codestory-cli-v0.11.1-macos-arm64.tar.gz`, extract it, put `codestory-cli` on `PATH`, and run `chmod +x codestory-cli` if needed. |
| macOS x64 | Use the source fallback until a matching release asset exists. |
| Linux x64 | Download `codestory-cli-v0.11.1-linux-x64.tar.gz`, extract it, put `codestory-cli` on `PATH`, and run `chmod +x codestory-cli` if needed. |
| Linux arm64 | Download `codestory-cli-v0.11.1-linux-arm64.tar.gz`, extract it, put `codestory-cli` on `PATH`, and run `chmod +x codestory-cli` if needed. |

Verify downloaded archives against `SHA256SUMS.txt`. Source fallback for any
OS: build CodeStory and add `target/release` to the Codex host `PATH`.

## Readiness

Check the binary first:

```console
codestory-cli --version
```

Then read `codestory://status` from the plugin. Without the plugin, the direct
CLI equivalent is:

```console
codestory-cli doctor --project <repo> --format markdown
```

Local navigation is ready only when the status resource reports
`local_navigation=ready`. Packet/search is ready only when strict sidecar status
reports `retrieval_mode=full`; otherwise use the repair commands from the
status resource and do not make packet/search-backed claims.

## Review Checks

```powershell
python C:\Users\alber\.codex\skills\.system\plugin-creator\scripts\validate_plugin.py plugins\codestory
node --test plugins\codestory\tests\plugin-static.test.mjs
git diff --check
```
