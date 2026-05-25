# Framework Route Coverage Verification

CodeStory indexes framework routes as graph symbols when extraction is backed by
fixtures and confidence labels. Do not claim full framework support from a
single heuristic hit.

## Current Coverage Target

- JavaScript/TypeScript: Express, React Router, SvelteKit, Next.js,
  Remix, Fastify, Koa, Hono, NestJS.
- Astro/Vue: Astro and Nuxt file-convention routes, plus Vue Router object
  routes.
- Python: Django, Flask, FastAPI.
- Ruby/PHP/Java/C#: Rails, Laravel, Spring, ASP.NET.
- Rust: Axum, Actix, Rocket.
- Go: Gin, Chi, Echo, Fiber as text-only partial route extraction until Go
  parser-backed handler links exist.
- Existing OpenAPI endpoint indexing remains separate and should continue to
  produce endpoint symbols and speculative client-call edges.
- Payload collection config and usage extraction is tracked as data bridge
  evidence. Usage edges preserve operation metadata such as `find`, `create`,
  `update`, `delete`, and `count` in edge callsite identity.

## Support Status

- `supported`: required fixtures pass, route metadata is emitted with the
  documented confidence floor, known unsupported patterns are listed, and
  handler-link claims have fixture or real-repo evidence.
- `heuristic`: extraction is useful but pattern-backed. Review source before
  claiming handler parity or full framework support.
- `partial`: some route shapes are covered, but nested routing, params,
  controller prefixes, handler links, or framework variants are missing.
- `unsupported`: no route coverage claim is made for this framework, syntax, or
  language path.
- `stale`: coverage came from an index that may not match the current checkout.
  Run `doctor --project <workspace>` and refresh before promoting a claim.
- `non-promotable`: required fixtures fail, required fixture classes are
  missing, known gaps are undocumented, or search-quality expectations are not
  updated.

`ambiguous` and `unmatched` are not support levels. They are workflow states:
`ambiguous` means a query must be rerun with `search --why`, `--id`, or `--file`;
`unmatched` means a changed path was not found in the persisted index and should
be checked with `files --path <fragment>` or a fresh index.

## Confidence Labels

- `file_convention`: route comes from a framework file convention such as
  SvelteKit `+page.svelte` or `+server.ts`.
- `decorator` or `annotation`: route comes from a Python decorator, Java
  annotation, C# attribute, or Rust attribute.
- `heuristic`: route comes from text/tree-sitter pattern matching and needs
  source review before claiming handler parity.

## Verification Playbook

1. Add or update a fixture for the framework syntax.
2. Assert route node label, method, path, confidence, and file membership.
3. Assert handler links only when existing graph evidence can resolve the
   handler.
4. For data bridge work, assert collection registration nodes, operation-aware
   usage edges, and the relevant `payload:<operation>:<slug>:...` callsite
   identity.
5. Run `codestory-cli files --project <fixture> --format json` and inspect
   `summary.framework_route_coverage` for framework, language, status,
   fixture status, confidence floor, handler-link support, unsupported
   patterns, known gaps, and promotable status.
6. Run `cargo test -p codestory-indexer --lib framework_route`.
7. Run the search-quality eval harness when route names should be discoverable:

   ```
   cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
   ```

8. For broader claims, probe at least one real repo or representative sample and
   record any unsupported syntax as partial coverage.

## Reporting Rules

- Say `supported` only when fixture and eval evidence meet the coverage floor.
- Say `partial` or `heuristic` when handler resolution is not proven.
- Say `route node plus handler edge` only when a test or real probe shows the
  edge.
- Say `data_collection_usage` only when the graph path includes Payload
  collection nodes or operation-aware usage edges.
- Keep unsupported framework syntax visible in coverage notes, docs, or tests
  rather than treating absence as success.
- Mark the framework `non-promotable` when required fixtures fail, a known gap
  lacks a note, or route-search eval expectations drift.
- Keep this workflow CLI-first. Do not use transport, server, or MCP surfaces to
  prove framework route support for this spec.
