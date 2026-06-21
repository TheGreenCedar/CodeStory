# `context` - Target Context For One Concrete Target

Builds DB-first target context around one concrete retrieval target. This is not a question-answering or chat command: it does not interpret natural-language questions. Use `packet` for broad task questions, `search` for candidate discovery, and `drill` for repeatable investigation reports.

## Usage

```
<codestory-cli> context [OPTIONS] (--id <NODE_ID> | --query <QUERY> | --bookmark <BOOKMARK_ID>)
```

## Key Options

| Option | Default | Use |
|--------|---------|-----|
| `--project <path>` | `.` | Repository root to query. Always pass it explicitly. |
| `--cache-dir <path>` | auto | Reuse or isolate a specific cache. |
| `--id <node_id>` | none | Build context around an exact node returned by `search`, `symbol`, `trail`, or `explore`. |
| `--query <target>` | none | Resolve a concrete symbol, file, literal, API path, module, or behavior term, then build context around the resolved target. |
| `--bookmark <bookmark_id>` | none | Build context around a saved investigation bookmark. |
| `--max-results <n>` | `8` | Secondary retrieval cap, clamped to 1-25. |
| `--refresh <auto|full|incremental|none>` | `none` | Read an existing cache unless you intentionally refresh. |
| `--format <markdown|json>` | `markdown` | Human or structured output. |
| `--output-file <path>` | none | Write output to a file. |
| `--bundle <dir>` | none | Write `context.md`, `context.json`, generated graph artifacts, and a bundle manifest. |
| `--no-evidence` | off | Omit citation edge ids and score breakdowns. Avoid this for grounded claims. |

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | `<codestory-cli> context --project <target-workspace> --query AppController` | Markdown context packet with resolution metadata, retrieval trace, citations, gaps, and next commands. |
| Failure path | If the target is ambiguous or missing, run `search --project <target-workspace> --query "<target>" --why`, choose a concrete `node_id`, then rerun `context --id <node_id>`. If local navigation readiness is weak, run `doctor --project <target-workspace>`, `setup embeddings --project <target-workspace>` when needed, and `index --project <target-workspace> --refresh full`. | Keeps target context tied to a resolvable target and avoids treating stale retrieval as strong evidence. |
| Integration edge | Use `search --why`, `explore`, or `bookmark list` first, then pass the selected node via `--id <node_id>` or `--bookmark <bookmark_id>`; use `--bundle out/context-AppController` for reviewer handoff. | Converts candidate discovery into a deeper, shareable evidence packet. |

## Notes

- Do not pass broad questions to `context`. Use `packet --question` for broad task questions, `search --repo-text on --why` for candidate discovery, `drill` for deterministic reports, and then `context --id <node_id>` for selected anchors.
- Good `--query` values are symbol names, file names, string literals, API paths, module names, and specific behavior terms.
- Treat `context` output as incomplete when it reports weak hits, semantic stale/partial/failed states, missing snippets, no citations, or unresolved graph edges.
