# `context` - Target Context For One Concrete Target

Builds target context around one concrete retrieval target (`--query` must name a symbol, file, literal, API path, module, or behavior term — not a broad question). Runs through the Investigate agent path and fails closed unless full retrieval is full. For broad questions use `packet`; for discovery use `search`; for repeatable reports use `drill`.

## Syntax

See [generated CLI syntax](generated-cli-syntax.md) for the current command usage.
Use `<codestory-cli> <command> --help` for the complete option set.

## Agent Paths

| Path | Command | Expected result |
|------|---------|-----------------|
| Normal path | `<codestory-cli> context --project <target-workspace> --query AppController` | Markdown context packet with resolution metadata, retrieval trace, citations, gaps, and next commands when full retrieval is full. |
| Failure path | If the target is ambiguous or missing, run `search --project <target-workspace> --query "<target>" --why`, choose a concrete `node_id`, then rerun `context --id <node_id>`. If retrieval readiness is weak, run `doctor --project <target-workspace>` and `retrieval index --project <target-workspace> --refresh full`. | Keeps target context tied to a resolvable target and avoids treating stale retrieval as strong evidence. |
| Integration edge | Use `search --why`, `explore`, or `bookmark list` first, then pass the selected node via `--id <node_id>` or `--bookmark <bookmark_id>`; use `--bundle out/context-AppController` for reviewer handoff. | Converts candidate discovery into a deeper, shareable evidence packet. |

## Notes

- Do not pass broad questions to `context`. Use `packet --question` for broad task questions, `search --why` for candidate discovery, `drill` for deterministic reports, and then `context --id <node_id>` for selected anchors.
- Good `--query` values are symbol names, file names, string literals, API paths, module names, and specific behavior terms.
- Use `symbol`, `trail`, `snippet`, or `explore` for cache-only local navigation when retrievals are degraded.
- Treat `context` output as incomplete when it reports weak hits, semantic stale/partial/failed states, missing snippets, no citations, or unresolved graph edges.
