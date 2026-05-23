# `drill` - Build a Repeatable Agent-Grounding Evidence Packet

Runs a deterministic evidence collection pass for a realistic codebase question. The command does not answer the question; it writes the artifacts an agent should use before drafting and verifying an answer.

## Usage

```
target/release/codestory-cli(.exe) drill [OPTIONS]
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

## Output

The command writes `drill-report.md`, `drill-report.json`, and compact `drill-summary.json` in `--output-dir`, plus per-anchor and bridge artifacts in the requested primary format.

The report includes:

- mechanical index status before and after refresh
- optional question repo-text search artifact
- per-anchor search, symbol, trail, explore, and snippet artifacts
- per-anchor `consumer_summary` entries for visible callers, related collection/API consumers, and ranked repo-text hints
- cross-anchor bridge artifacts using graph paths first, then endpoint files, shared-file fallback diagnostics, and ranked `evidence_files` when no graph bridge is visible
- chosen anchor, endpoint files, and source-truth verification targets
- an `evidence_packet` with typed evidence items, repo-text hints, negative evidence, source locations, confidence, and readiness status
- an Answer Readiness report with `safe_to_say`, `inferred_claims`, `needs_verification`, `next_commands`, and `source_truth_checks`
- compact mechanical status, retrieval/freshness status, bridge counts, source-truth file list, and verdict/next action in `drill-summary.json`
- an answer-quality contract requiring a CodeStory-only draft before source reads and source-truth verification afterward
- a fillable claim-ledger template for source-truth classification, correction counts, and material-revision tracking
- a verification checklist requiring `correct`, `partial`, `misleading`, or `unsupported` classifications

## Examples

```bash
# CodeStory-first evidence packet for an architecture question
target/release/codestory-cli(.exe) drill --project . --refresh full --question "how full indexing supports search trail and snippet commands" --anchors WorkspaceIndexer,SearchService,TrailResult --output-dir target/drill/codestory

# JSON-first run for automation, while still writing Markdown too
target/release/codestory-cli(.exe) drill --project . --refresh none --anchors Posts,getElsewhereFeed,getCommentAuth --output-dir target/drill/rootandruntime --format json
```

## Interpretation

Use the drill report as the CodeStory-only phase. Draft the architecture answer from those artifacts first, then open only files named or implied by the artifacts and classify each claim against source truth. If the answer changes materially after source reads, record that as a CodeStory or agent-UX finding.

Start with `drill-summary.json` for compact health, retrieval/freshness state, bridge status, and the verdict next action, then read `evidence_packet.readiness`. Claims in `safe_to_say` are anchored enough for a draft. Claims in `inferred_claims` or `needs_verification` must stay uncertain until the listed `source_truth_checks` or equivalent source reads confirm them. Repo-text and cross-language framework hits are navigation hints unless supported by typed symbol/trail/snippet evidence or source-truth verification.

If `drill-summary.json` reports stale freshness, refresh the index before promoting claims. If retrieval is symbolic-only or semantic fallback is reported, broad natural-language recall is degraded even when exact anchors resolve; use repo-text, symbol, trail, snippet, and source-truth files deliberately.

The optional `question_search` artifact is intentionally partial discovery evidence. A weak natural-language top hit does not answer the question by itself; use it to refine anchors, then rely on each anchor's symbol/trail/explore/snippet artifacts and the source-truth checklist.

If a trail is `structural_only=true`, it is still useful containment/type evidence, but it does not prove runtime flow or application access by itself. Follow up on concrete methods/functions from the trail with `snippet --function-body`, `explore`, or an additional anchor before drafting flow claims.
