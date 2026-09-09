# Upgrading to CodeStory 0.17.6

CodeStory 0.17.6 adopts publication schema 3. This is a breaking response-schema
change for integrations built against 0.17.5. Install the matching plugin and
CLI together, start a fresh host session, and update custom consumers before
using the new responses. MCP protocol negotiation does not translate schema 3
back to schema 2.

Agents can discover candidates, inspect source and follow relationships in any
useful order. Native search and source reads remain available. Automatic packets
are explicit experiments and never decide whether an investigation is complete.

## Read evidence rows

| 0.17.5 consumer | 0.17.6 consumer |
| --- | --- |
| Search `hits` and `query_assessment` | Read `evidence`, `status`, `gaps` and `retrieval`. |
| Older context/citation shape | Read `target`, `evidence`, `status` and `gaps`. |
| Packet `answer`, `support` or `disposition` | Inspect `evidence` and `gaps`; `answer_sufficiency` is `not_asserted`. |
| Response assumed compatible after MCP negotiation | Require publication schema 3 and minimum compatible schema 3. |

For modern MCP profiles, read `result.structuredContent`; older supported
profiles return the same JSON document in `result.content[0].text`. Handle tool
errors and preparation responses before treating the document as evidence.

```js
if (result.isError) throw new Error(result.content[0].text);
const evidence = result.structuredContent ?? JSON.parse(result.content[0].text);
if (evidence.kind === "preparing") {
  // Wait evidence.retry_after_ms, then retry the unchanged request.
} else {
  for (const row of evidence.evidence ?? []) {
    // Preserve row.identity, row.path and nullable row.symbol_id/line bounds.
    // Inspect row.excerpt or read the source when the claim needs more context.
  }
  // Keep evidence.publication and disclose material evidence.gaps.
}
```

A concrete navigation sequence can discover a symbol and inspect the selected
result without a packet:

```js
const found = await search({project, query: "handleRequest", repo_text: "off"});
// Select using path, scope and source if there is more than one candidate.
const selected = chooseCandidate(found.evidence);
if (selected.symbol_id !== null) {
  await context({project, id: selected.symbol_id});
}
// Native source reads and additional relationship operations remain available.
```

These examples show consumer logic, not a new JavaScript SDK. Use the actual
host's MCP bindings. Nullable excerpts and line bounds stay nullable; a missing
result is not an absence proof. `repo_text: "off"` requires an existing complete
core publication and does not prepare embeddings.

## Request the framework catalog when needed

`files` still returns project file counts, language support tiers, errors,
exclusions and coverage gaps by default. Its global framework capability
catalog is now opt-in: pass `include_framework_coverage: true` through MCP or
`--include-framework-coverage` on the CLI. This returns the complete catalog,
independent of file filters.

When `summary.framework_route_coverage_included` is false, the empty
`framework_route_coverage` array means the catalog was omitted. It does not
mean that no frameworks are supported. Older responses lack the inclusion
marker; do not infer their inclusion state from a missing marker.

## Update packet requests

| Removed input | Replacement |
| --- | --- |
| MCP `task_class`; CLI `--task-class` | Omit it. The question is not routed through an answer taxonomy. |
| MCP `extra_probes`; CLI `--extra-probe` | Use tagged `probes` / repeated `--probe` only for known selectors or explicit free queries. |
| MCP `include_evidence`; CLI `--no-evidence` on packet | Omit it. Evidence is part of the packet contract. |
| CLI `--step-trace-out` | Use `--diagnostics-out` for the separate diagnostic projection. |

Obsolete fields are rejected, not silently ignored. This table describes packet
requests; use each other tool's own advertised schema.

```json
{
  "project": "/absolute/repository",
  "question": "How does request authentication reach the handler?",
  "probes": [{"kind": "exact_path", "path": "src/auth.ts"}]
}
```

The exact path above is appropriate only if it is already known. Do not invent
paths or expected answer stages to fill probes. A CLI equivalent is:

```sh
codestory-cli packet --project /absolute/repository \
  --question "How does request authentication reach the handler?" \
  --probe '{"kind":"exact_path","path":"src/auth.ts"}' \
  --diagnostics-out /absolute/output/packet-diagnostics.json --format json
```

Packets retain at most sixteen evidence rows and a complete MCP result of
16 KiB. An offered continuation is limited to one round with the unchanged
question and returned publication pins. These bounds do not restrict ordinary
source exploration. The default MCP catalog does not expose the experimental
indexed-call verifier.

## Cache upgrade and rollback

The new cache uses schema 32 and immutable core generations, replacing schema
31's single core database. Normal managed preparation upgrades or rebuilds
derived state; bookmarks and user annotations must be preserved. No manual
cache deletion is part of the normal upgrade.

Before an upgrade where rollback is required, stop clients using that cache and
preserve a complete pre-upgrade copy, including its annotation sidecar. Keep the
0.17.5 archive and its verified checksum with that backup. A version string
alone does not distinguish a published archive from a development build.

For rollback, stop the newer clients and use the authenticated old archive with
the preserved pre-upgrade cache copy. Do not point 0.17.5 at an upgraded cache.
Keep the newer cache separately so annotations added after the snapshot remain
recoverable; restoring an earlier copy does not include those later changes.

Use explicit isolated roots for upgrade tests. If preparation fails, preserve
its diagnostic and previous publication before taking the focused recovery
steps in [Troubleshooting](troubleshooting.md). Do not delete the user's cache to
make an upgrade test pass.
