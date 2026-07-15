# `drill` - Build a Repeatable Agent-Grounding Evidence Packet

Runs a deterministic evidence collection pass for a realistic codebase question. The command does not answer the question; it writes the artifacts an agent should use before drafting and verifying an answer.

## Usage

```
<codestory-cli> drill [OPTIONS]
```

## Arguments

| Argument | Type | Default | Description |
|----------|------|---------|-------------|
| `--project` | path | `.` | Project root directory (alias: `--path`) |
| `--cache-dir` | path | *auto* | Override the cache directory |
| `--anchors` | string list | **required** | Concrete anchors to investigate; comma-separated and repeatable |
| `--question` | string | *none* | Natural-language architecture question to search with repo text; stored in the report |
| `--label` | string | *none* | Human label for the run |
| `--output-dir` | path | **required** | Directory for the drill report and artifacts; created if missing |
| `--refresh` | enum | `full` | Refresh strategy: `auto`, `full`, `incremental`, `none` |
| `--format` | enum | `markdown` | Primary output format: `markdown` or `json` |
| `--jobs` | integer | `1` | Read-only anchor and bridge evidence workers for `--refresh none`; capped automatically |

## Output

The command writes `drill-report.md`, `drill-report.json`, and compact `drill-summary.json` in `--output-dir`, plus per-anchor and bridge artifacts in the requested primary format.

The report includes:

- mechanical index status before and after refresh
- optional question repo-text search artifact
- bounded supplemental question searches for likely public surfaces, data collections, and store modules when the question terms imply them
- per-anchor search, symbol, trail, explore, and snippet artifacts
- per-anchor `consumer_summary` entries for visible callers, related collection/API/native method consumers, and ranked repo-text hints
- cross-anchor bridge artifacts using graph paths first, then endpoint files, shared-file fallback diagnostics, and ranked `evidence_files` when no graph bridge is visible
- chosen anchor, endpoint files, and source-truth verification targets
- an `evidence_packet` with typed evidence items, repo-text hints, negative evidence, source locations, confidence, and readiness status
- an Answer Readiness report with `safe_to_say`, `inferred_claims`, `needs_verification`, `next_commands`, and `source_truth_checks`
- compact mechanical status, retrieval/freshness status, drill runtime timings, bridge counts, source-truth file list plus target roles/ranking reasons, and verdict/next action in `drill-summary.json`
- an answer-quality contract requiring a CodeStory-only draft before source reads and source-truth verification afterward
- a fillable claim-ledger template for source-truth classification, correction counts, and material-revision tracking
- a verification checklist requiring `correct`, `partial`, `misleading`, or `unsupported` classifications

## Examples

```bash
# CodeStory-first evidence packet for an architecture question
<codestory-cli> drill --project <target-workspace> --refresh full --question "how the public API reaches the backing store" --anchors ApiController,Repository,StorageClient --output-dir target/drill/api-store-flow

# JSON-first run for automation, while still writing Markdown too
<codestory-cli> drill --project <target-workspace> --refresh none --anchors EntryPoint,Coordinator,BackingStore --output-dir target/drill/entrypoint-flow --format json

# Optional read-only anchor and bridge workers against an already-fresh local index
<codestory-cli> drill --project <target-workspace> --refresh none --anchors EntryPoint,Coordinator,BackingStore --output-dir target/drill/entrypoint-flow --format json --jobs 4
```

## Interpretation

Use the drill report as the CodeStory-only phase. Draft the architecture answer from those artifacts first, then open only files named or implied by the artifacts and classify each claim against source truth. If the answer changes materially after source reads, record that as a CodeStory or agent-UX finding.

Start with `drill-summary.json` for compact health, retrieval/freshness state, drill runtime timings, bridge status, bridge `evidence_kind`, source-truth target roles, and the verdict next action, then read `evidence_packet.readiness`. Claims in `safe_to_say` are anchored enough for a draft. Claims in `inferred_claims` or `needs_verification` must stay uncertain until the listed `source_truth_checks` or equivalent source reads confirm them. Repo-text and cross-language framework hits are navigation hints unless supported by typed symbol/trail/snippet evidence or source-truth verification. A `source_truth_only` bridge is deliberately not proof; it means CodeStory found the concrete files to read but no typed graph/framework/data path strong enough to answer without source verification.

`mechanical.drill_timings` breaks the evidence-collection runtime into setup, question search, anchor resolution, supplemental search, bridge evidence, and evidence assembly. Per-anchor `timings`, command `duration_ms`, and summary `slowest_command` fields further split anchor work into search, query resolution, consumer-summary, and artifact-command costs. Use these fields to localize slow drills before changing ranking or graph traversal logic; they are diagnostic timing, not answer-quality evidence by themselves.

Consumer summaries inspect direct incoming production consumers for the selected anchor first. Related payload/API/native targets are searched only when the selected anchor has no visible graph consumers, so ordinary drills do not pay broad related-target search costs unless the direct graph evidence is missing.

If `drill-summary.json` reports stale freshness, refresh the index before promoting claims. If retrieval is not full or semantic diagnostics report degraded state, wait for a complete publication or run the maintainer-directed rebuild before trusting broad natural-language recall; use symbol, trail, snippet, and source-truth files deliberately while broad retrieval is unavailable.

`--jobs` is default-off and read-only. Use it only with `--refresh none` after
the index is fresh, and measure the run: multi-case suites can benefit from
parallel case execution, while single-case anchor resolution and bridge checks
may be limited by storage and graph traversal contention on some repos.

The optional `question_search` artifact and any `question_supplemental_searches` are intentionally partial discovery evidence. They can add public page, component, collection, and store files to the source-truth checklist when the broad question points there, but they do not prove the architecture by themselves. Use them to avoid missing verification files, then rely on each anchor's symbol/trail/explore/snippet artifacts and focused source reads before promoting claims.

If a trail is `structural_only=true`, it is still useful containment/type evidence, but it does not prove runtime flow or application access by itself. Follow up on concrete methods/functions from the trail with `snippet --function-body`, `explore`, or an additional anchor before drafting flow claims.

For native Sourcetrail-style anchors, `consumer_summary` may add bounded related targets such as `SourceGroupCxxCdb::getIndexerCommands` or `IndexerJava::doIndex`. Treat these as concrete follow-up anchors for snippets/trails; member containment still needs source-truth verification before claiming runtime invocation.
