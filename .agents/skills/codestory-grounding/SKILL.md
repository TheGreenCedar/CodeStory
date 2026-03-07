---
name: codestory-grounding
description: Ground repository answers and edits with `codestory-cli` workspace queries. Use when you need to index a workspace, gather broad grounding, search code, inspect a symbol, follow a trail, or fetch a snippet before making claims or changes in Codestory.
---

# Codestory Grounding

Use this skill to collect repo evidence with `codestory-cli` before answering architecture, navigation, or implementation questions.

## Workflow

1. Refresh the index when the workspace may be stale with `scripts/index.py`.
2. Start with `scripts/ground.py` when you need broad task grounding before opening files.
3. Narrow with `scripts/search.py`, `scripts/symbol.py`, `scripts/trail.py`, or `scripts/snippet.py` based on the question.
4. Treat command output as evidence, then open only the files needed for edits or verification.

## Scripts

- `scripts/index.py`: Run `codestory-cli index ...`
- `scripts/ground.py`: Run `codestory-cli ground ...`
- `scripts/search.py`: Run `codestory-cli search ...`
- `scripts/symbol.py`: Run `codestory-cli symbol ...`
- `scripts/trail.py`: Run `codestory-cli trail ...`
- `scripts/snippet.py`: Run `codestory-cli snippet ...`

Pass extra arguments through unchanged. The scripts resolve the workspace root automatically, so they can be launched from anywhere inside the repo checkout.

Use `--dry-run` first if you only need to inspect the exact `cargo run -p codestory-cli -- <subcommand> ...` command that would execute.

If a subcommand is unavailable in the current checkout, report that plainly and fall back to direct repo inspection instead of inventing grounded results.
