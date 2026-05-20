# Framework Route Coverage Verification

CodeStory indexes framework routes as graph symbols when extraction is backed by
fixtures and confidence labels. Do not claim full framework support from a
single heuristic hit.

## Current Coverage Target

- JavaScript/TypeScript: Express, React Router, SvelteKit.
- Python: Django, Flask, FastAPI.
- Ruby/PHP/Java/C#: Rails, Laravel, Spring, ASP.NET.
- Rust: Axum, Actix, Rocket.
- Existing OpenAPI endpoint indexing remains separate and should continue to
  produce endpoint symbols and speculative client-call edges.

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
4. Run `cargo test -p codestory-indexer --lib framework_route`.
5. Run the search-quality eval harness when route names should be discoverable:

   ```
   cargo test -p codestory-cli --test search_json_output -- --ignored --nocapture search_quality_eval
   ```

6. For broader claims, probe at least one real repo or representative sample and
   record any unsupported syntax as partial coverage.

## Reporting Rules

- Say "indexed with heuristic confidence" when handler resolution is not proven.
- Say "route node plus handler edge" only when a test or real probe shows the
  edge.
- Keep unsupported framework syntax visible in coverage notes, docs, or tests
  rather than treating absence as success.
