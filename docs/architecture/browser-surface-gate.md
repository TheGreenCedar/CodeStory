# Browser Surface Gate

CodeStory should keep `explore`, `serve --stdio`, and the read-only browser
service as the default codebase-browser surfaces until a separate web cockpit or
`browse` command has evidence that it solves a different workflow.

## Current Status

Status: deferred.

`explore` is the cockpit path for now. It already bundles project status,
query resolution, navigation results, symbol details, trail context, snippets,
and next commands without introducing another UI surface.

Do not add a new `browse` command, web cockpit route, or browser-specific web UI
until all of the gates below have current evidence in the repo.

## Promotion Gates

Before starting web cockpit work:

- Tool, resource, and prompt manifests must be stable under stdio catalog tests.
- HTTP and stdio browser contracts must stay aligned with the read-only browser
  service.
- Warm stdio/browser-loop p50, p95, and p99 timings must be recorded and must
  meet the active Current Promotion Budget in
  `docs/testing/codestory-stdio-warm-loop-stats.md`: small-fixture smoke p95
  stays under the smoke budget, and a current real-repo run meets the Web
  Cockpit Promotion Budget.
- Browser stress lanes must pass at the intended scale, and synthetic evidence
  must not be treated as real-repository promotion proof.
- `explore` must demonstrate the cockpit workflow in JSON/Markdown and
  keyboard-first TUI paths.
- Screenshot-visible review must be planned before implementation, with one
  reviewer for the full viewport and one reviewer for the changed surface or
  acceptance path.

## Evidence Sources

- `crates/codestory-cli/tests/stdio_protocol_contracts.rs` protects tool,
  resource, prompt, and schema stability.
- `crates/codestory-cli/tests/http_transport_contracts.rs` protects HTTP and
  stdio default-browser alignment.
- `crates/codestory-cli/tests/stdio_warm_loop_stats.rs` measures warm loop
  p50, p95, and p99.
- `docs/testing/codestory-stdio-warm-loop-stats.md` owns the active warm p95
  promotion budget and current run evidence.
- `docs/testing/codestory-stress-lanes.md` defines browser-scale stress lanes
  and promotion thresholds.
- `crates/codestory-cli/tests/cli_golden_path.rs` keeps `explore` useful as the
  bundled cockpit path.

## When The Gate Opens

If the gates are satisfied, start with a written implementation plan that names
why the new surface is not a duplicate of `explore`, the exact routes or
commands to add, the screenshot-visible review loop, and the rollback path.
