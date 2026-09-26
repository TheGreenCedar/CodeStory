Maintainer CLI only. There is no MCP tool with this name. Product
tools own activation; do not call this from the grounding skill.

# `drill` - Build a Repeatable Agent-Grounding Evidence Packet

Runs a deterministic evidence collection pass for a realistic codebase question. The command does not answer the question; it writes the artifacts an agent should use before drafting and verifying an answer.

## Syntax

See [generated CLI syntax](generated-cli-syntax.md) for the current command usage.
Use `<codestory-cli> <command> --help` for the complete option set.

## Output

The command writes `drill-report.md`, `drill-report.json`, and compact
`drill-summary.json` in `--output-dir`.

The report includes:

- mechanical index status before and after refresh
- the closed schema-v3 `evidence_packet`, including evidence rows, explicit gaps,
  continuation state, publication identity, and retrieval state
- anchor and bridge observations adapted from that packet without a second
  search or scoring pass
- evidence targets named by packet citations
- compact observational index/retrieval/freshness state and drill timings
- `evidence_review`, `open_gaps`, and `availability` sections in
  `drill-summary.json`

The summary reports evidence availability only. It does not publish verified
claims, answer quality, safe-to-say counts, or an answer-ready verdict.
This evidence-only shape is `summary_version: 2`.

## Examples

```bash
# CodeStory-first evidence packet for an architecture question
<codestory-cli> drill --project <target-workspace> --refresh full --question "how the public API reaches the backing store" --anchors ApiController,Repository,StorageClient --output-dir target/drill/api-store-flow

# JSON-first run for automation, while still writing Markdown too
<codestory-cli> drill --project <target-workspace> --refresh none --anchors EntryPoint,Coordinator,BackingStore --output-dir target/drill/entrypoint-flow --format json
```

## Interpretation

Start with `drill-summary.json`. `availability.status` is `available`, `partial`,
or `unavailable` and derives from the closed v3 packet plus observational index,
retrieval, freshness, anchor, and bridge state. `available` means evidence rows are
available; it does not establish that an answer is correct. Read the packet's
evidence and gaps before using it. A continuation or any published gap keeps the
summary partial. An `evidence_hint_only` bridge is navigation evidence, not a
proved runtime relationship.

`mechanical.drill_timings` breaks the evidence-collection runtime into setup, question search, anchor resolution, supplemental search, bridge evidence, and evidence assembly. Per-anchor `timings`, command `duration_ms`, and summary `slowest_command` fields further split anchor work into search, query resolution, consumer-summary, and artifact-command costs. Use these fields to localize slow drills before changing ranking or graph traversal logic; they are diagnostic timing, not answer-quality evidence by themselves.

When `--refresh auto` must select full recovery before a safe project summary can be read, the report omits the `before_*` metrics and sets `before_unavailable_reason` to the compatibility cause. The compact summary likewise omits `mechanical.before` and `error_delta`. Do not interpret the after-refresh counts as a measured pre-refresh baseline.

Consumer summaries inspect direct incoming production consumers for the selected anchor first. Related payload/API/native targets are searched only when the selected anchor has no visible graph consumers, so ordinary drills do not pay broad related-target search costs unless the direct graph evidence is missing.

If `drill-summary.json` reports stale freshness, refresh the index before using
the evidence. If retrieval is not full, wait for a complete publication or run
the maintainer-directed rebuild before relying on broad retrieval.

`drill --jobs` is deprecated, hidden, and ignored: the evidence packet owns
drill scheduling, so a single drill has no worker pool to size. Supplying it
prints a notice and changes nothing, and it is removed next release. Case-level
parallelism lives on `drill-suite --jobs`.
