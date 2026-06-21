---
name: codestory-grounding
description: Use when Codex should ground repository claims through CodeStory's local CLI and stdio server before answering or editing.
---

# CodeStory Grounding

Use CodeStory as a local, read-only grounding layer. The product boundary stays
at `codestory-cli`; this plugin only packages Codex metadata and the stdio
launch path.

## Start Here

1. Read `codestory://status` before trusting packet or search.
2. If `codestory-cli` is missing, run the setup action printed by the MCP
   launcher or `scripts/install-codestory.ps1 -Project <workspace>`.
3. If local navigation is not ready, follow the status resource's repair
   commands before source-backed claims.
4. Treat packet/search as degraded unless strict retrieval reports
   `retrieval_mode=full`.
5. Use packet for broad repository questions, search for candidate discovery,
   then snippet/context/trail after selecting a concrete target.

## Safety

The stdio tools are local-only, read-only, non-destructive, idempotent, and
closed-world. Do not claim packet/search readiness from semantic-only,
repo-text-only, stale, missing-manifest, or unavailable retrieval states.
