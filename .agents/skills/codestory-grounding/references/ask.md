# `ask` - DB-First Answer Packet

Runs indexed retrieval for an investigation prompt and returns an answer packet. By default it uses CodeStory's retrieval layer only; `--with-local-agent` is opt-in.

## Usage

```
target/release/codestory-cli(.exe) ask [OPTIONS] <PROMPT>
```

## Key Options

| Option | Default | Use |
|--------|---------|-----|
| `--project <path>` | `.` | Repository root to query. Always pass it explicitly. |
| `--cache-dir <path>` | auto | Reuse or isolate a specific cache. |
| `--profile <auto|architecture|callflow|inheritance|impact>` | `auto` | Tune retrieval shape for the question. |
| `--max-results <n>` | `8` | Retrieval cap, clamped to 1-25. |
| `--focus-id <node_id>` | none | Seed retrieval from a node returned by `search`, `symbol`, `trail`, or `explore`. |
| `--refresh <auto|full|incremental|none>` | `none` | Read an existing cache unless you intentionally refresh. |
| `--format <markdown|json>` | `markdown` | Human or structured output. |
| `--bundle <dir>` | none | Write Markdown, JSON, and graph handoff artifacts. |
| `--no-evidence` | off | Omit citation edge ids and score breakdowns. Avoid this for grounded answers. |
| `--with-local-agent` | off | Launch the configured local agent after indexed retrieval. |

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | `target/release/codestory-cli(.exe) ask --project . "How does search ranking work?"` | Markdown answer with cited indexed evidence. |
| Failure path | If it reports the index is unavailable, run `doctor --project .`, then `index --project . --refresh full`, and retry with `--refresh none`. | Avoids treating an empty or stale cache as evidence. |
| Integration edge | Use `search --why` or `explore` first, then pass the selected node via `--focus-id <node_id>`; use `--bundle out/ask-search-ranking` for reviewer handoff. | Keeps the final answer tied to prior browser evidence. |

## Notes

- Keep evidence on for user-facing claims; citations are the value of this command.
- Use `--with-local-agent` only when the local agent command is configured and the user wants a second agent pass.
- Use `--format json` when another tool needs structured citations and score data.
