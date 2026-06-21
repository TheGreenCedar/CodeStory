---
name: codestory-grounding
description: Use when Codex should inspect a local repository through CodeStory before making source claims, choosing tests, or editing code.
---

# CodeStory Grounding

CodeStory is a local read-only grounding service. This plugin only connects
Codex to `codestory-cli serve --stdio`; all indexing, runtime, retrieval,
packet, search, and sidecar behavior stays in `codestory-cli`.

## Setup First

1. Start by reading `codestory://status`.
2. If the launcher says `codestory-cli` is missing, run the printed
   `powershell -ExecutionPolicy Bypass -File scripts/install-codestory.ps1`
   setup action, or set `CODESTORY_CLI` to a ready binary.
3. If `local_navigation` is not `ready`, run the status resource's repair
   commands before relying on CodeStory output.
4. If `agent_packet_search` is not `ready`, do not use packet/search results as
   proof. Follow the repair commands and re-read `codestory://status`.
5. Packet/search claims are allowed only when strict sidecar status reports
   `retrieval_mode=full`.

## Operating Loop

Use this order unless the user asks for a narrower source read:

1. `resources/read` `codestory://status`
2. `resources/read` `codestory://agent-guide`
3. `tools/call` `packet` for broad task questions, only when packet/search is
   ready
4. `tools/call` `search` for candidate discovery, only when packet/search is
   ready
5. `resources/read` `codestory://snippet/<node_id>` or
   `codestory://trail/<node_id>` after selecting a concrete target

If readiness is degraded, use direct source reads or ordinary local commands for
the task, and label CodeStory packet/search as blocked.

## Safety

The stdio server is local-only and read-only. Treat repo-text, semantic-only,
stale, missing-manifest, or unavailable retrieval states as navigation hints at
most. They are not packet/search proof.
