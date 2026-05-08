# `ask` - DB-First Answer Packet

Runs indexed retrieval for an investigation prompt and returns an answer packet. It uses CodeStory's retrieval layer only.

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
| `--investigate` | off | Use bounded investigation retrieval with weak-hit fallback, query expansion, and explicit gap trace. Prefer this for repo explanations. |
| `--max-results <n>` | `8` | Retrieval cap, clamped to 1-25. |
| `--focus-id <node_id>` | none | Seed retrieval from a node returned by `search`, `symbol`, `trail`, or `explore`. |
| `--bookmark <bookmark_id>` | none | Seed retrieval from a saved investigation bookmark. Mutually exclusive with `--focus-id`. |
| `--refresh <auto|full|incremental|none>` | `none` | Read an existing cache unless you intentionally refresh. |
| `--format <markdown|json>` | `markdown` | Human or structured output. |
| `--bundle <dir>` | none | Write Markdown, JSON, and graph handoff artifacts. |
| `--no-evidence` | off | Omit citation edge ids and score breakdowns. Avoid this for grounded answers. |

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | `target/release/codestory-cli(.exe) ask --project . --investigate "How does search ranking work?"` | Markdown answer with cited indexed evidence and a DB-first mode line. |
| Failure path | If it reports the index is unavailable, run `doctor --project .`, then `setup embeddings --project .` if needed, `index --project . --refresh full`, and retry with `--refresh none`. If `doctor` reports semantic partial/stale/failed, use lexical/repo-text fallback until the rebuild is clean. | Avoids treating an empty, stale, or partial semantic cache as evidence. |
| Integration edge | Use `search --why`, `explore`, or `bookmark list` first, then pass the selected node via `--focus-id <node_id>` or `--bookmark <bookmark_id>`; use `--bundle out/ask-search-ranking` for reviewer handoff. | Keeps the final answer tied to prior browser evidence. |

## Notes

- Keep evidence on for user-facing claims; citations are the value of this command.
- Do not use broad `ask` to explain a repo while `doctor` reports semantic partial/stale/failed. Run `search --repo-text on --why`, `ground`, and focused `symbol`/`trail`/`snippet` instead.
- Use `--format json` when another tool needs structured citations and score data.
