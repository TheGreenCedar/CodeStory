# `explain` - Guided Repo Explanation

Runs the skill-first explanation path in one command: open or refresh the index,
collect grounding, run architecture-aware anchor search, then produce a DB-first
ask packet with citations.

```bash
target/release/codestory-cli(.exe) explain [OPTIONS] [PROMPT]
```

| Option | Default | Purpose |
| --- | --- | --- |
| `--project <path>` | `.` | Repository root to explain. Always pass it explicitly. |
| `PROMPT` | `How does this repo fit together?` | Broad explanation prompt. |
| `--id <node-id>` | none | Seed the explanation around an exact node id from `search`, `symbol`, or `trail`. Alias: `--focus-id`. |
| `--refresh <mode>` | `auto` | Opens or refreshes the index before collecting evidence. |
| `--max-results <n>` | `12` | Caps anchor and ask retrieval breadth. |
| `--format <format>` | `markdown` | `markdown` or `json`. |

## Usage

| Situation | Command | Expected result |
| --- | --- | --- |
| Normal path | `target/release/codestory-cli(.exe) explain --project .` | Markdown handoff with workflow, retrieval health, `agent_handoff`, anchors, focused next commands, answer, citations, and trace. |
| Machine-stable output | `target/release/codestory-cli(.exe) explain --project . --format json` | JSON with `workflow`, `grounding`, `anchors`, `answer`, and `next_commands`. |
| Focused follow-up | `target/release/codestory-cli(.exe) explain --project . --id <anchor> "Explain this symbol and its role in the repo."` | Reuses the repo explanation wrapper while focusing retrieval on a known node. |
| Deep symbol follow-up | Run `ask --focus-id <anchor>`, `trail --story --hide-speculative`, or `snippet --context 40` from `agent_primary_commands`. | Avoids broad exploratory command loops. |

## Notes

- Use `doctor` first in manual E2E tests when you need to prove semantic health.
- If `doctor` reports semantic partial/stale/failed, rebuild or use the lexical
  fallback recipe before treating broad explanations as authoritative.
- `explain` is evidence-first: it labels the answer as a DB-first repo
  explanation packet backed by indexed evidence.
- Prefer the `agent_handoff` and `agent_primary_commands` block before inventing
  new command sequences. It is designed for agent follow-up UX.
