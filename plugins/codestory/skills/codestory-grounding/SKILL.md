---
name: codestory-grounding
description: Use when Codex should inspect a local repository through CodeStory before making source claims, choosing tests, or editing code.
---

# CodeStory Grounding

CodeStory runs through `codestory-cli serve --stdio`. This plugin does not
implement indexing, runtime, retrieval, packet, search, or sidecar behavior.

## Setup First

1. Make sure `codestory-cli` is installed and on `PATH`.
2. If `codestory-cli` is missing, install the release binary for the current OS:
   - Windows: download `codestory-cli-v0.11.0-windows-x64.zip` or
     `codestory-cli-v0.11.0-windows-arm64.zip`, or run
     `powershell -ExecutionPolicy Bypass -File scripts/install-codestory.ps1`
     from a CodeStory checkout.
   - macOS: download `codestory-cli-v0.11.0-macos-arm64.tar.gz`, place
     `codestory-cli` on `PATH`, and run `chmod +x codestory-cli` if needed.
     For macOS x64, use the source fallback until a matching asset exists.
   - Linux: download `codestory-cli-v0.11.0-linux-x64.tar.gz` or
     `codestory-cli-v0.11.0-linux-arm64.tar.gz`, place `codestory-cli` on
     `PATH`, and run `chmod +x codestory-cli` if needed.
   - Source fallback: build `codestory-cli` from the CodeStory checkout and add
     `target/release` to `PATH`.
3. Read `codestory://status` before trusting any CodeStory result.
4. If `local_navigation` is not `ready`, run the status resource's repair
   commands before relying on source claims.
5. If `agent_packet_search` is not `ready`, packet/search is blocked. Run the
   repair commands and re-read `codestory://status`.

Packet/search claims are allowed only when strict sidecar status reports
`retrieval_mode=full`.

## Operating Loop

Use this order unless the user asks for a narrower source read:

1. `resources/read` `codestory://status`
2. `resources/read` `codestory://agent-guide`
3. `tools/call` `packet` for broad questions, only when packet/search is ready
4. `tools/call` `search` for candidate discovery, only when packet/search is
   ready
5. `resources/read` `codestory://snippet/<node_id>` or
   `codestory://trail/<node_id>` after selecting a concrete target

If readiness is degraded, use direct source reads or ordinary local commands and
label CodeStory packet/search as blocked.

## Safety

The stdio server is local-only and read-only. Treat repo-text, semantic-only,
stale, missing-manifest, or unavailable retrieval states as navigation hints at
most. They are not packet/search proof.
