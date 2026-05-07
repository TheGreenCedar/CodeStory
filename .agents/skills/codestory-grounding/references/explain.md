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
| `--refresh <mode>` | `auto` | Opens or refreshes the index before collecting evidence. |
| `--max-results <n>` | `12` | Caps anchor and ask retrieval breadth. |
| `--format <format>` | `markdown` | `markdown` or `json`. |
| `--with-local-agent` | off | Run the configured local agent after indexed retrieval. |

## Usage

| Situation | Command | Expected result |
| --- | --- | --- |
| Normal path | `target/release/codestory-cli(.exe) explain --project .` | Markdown handoff with workflow, retrieval health, anchors, next commands, answer, citations, and trace. |
| Machine-stable output | `target/release/codestory-cli(.exe) explain --project . --format json` | JSON with `workflow`, `grounding`, `anchors`, `answer`, and `next_commands`. |
| Focused follow-up | Run `ask --investigate` or `explore --id <anchor>` from the returned anchors. | Keeps repo overview separate from deeper subsystem inspection. |

## Notes

- Use `doctor` first in manual E2E tests when you need to prove semantic health.
- If `doctor` reports semantic partial/stale/failed, rebuild or use the lexical
  fallback recipe before treating broad explanations as authoritative.
- `explain` is still evidence-first: without `--with-local-agent`, it labels the
  answer as DB-first/no-local-agent rather than pretending a synthesis model read
  the whole repository.
