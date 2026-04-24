# Research Data Catalog

This catalog preserves how to find CodeStory research evidence without trying
to commit every generated cache and log file. It is a map to raw data plus the
rules for keeping future runs interpretable.

The raw data is local and gitignored. Before deleting `target/`, moving this
checkout, or pruning generated files, archive the roots listed below if the
research still matters.

## Tracked Synthesis

| File | Purpose |
| --- | --- |
| [`../research.md`](../research.md) | Human research front door and current decisions. |
| [`embedding-backend-benchmarks.md`](embedding-backend-benchmarks.md) | Main embedding/backend decision sheet and detailed artifact-root ledger. |
| [`embedding-research-run-2.md`](embedding-research-run-2.md) | Harness contract, stage definitions, scoring rules, and continuation notes. |
| [`codestory-e2e-stats-log.md`](codestory-e2e-stats-log.md) | Rolling repo-scale index/search timing history. |
| [`../project-delight-roadmap.md`](../project-delight-roadmap.md) | Product/UX research synthesis and implemented roadmap snapshot. |

## Local Raw Artifact Roots

Counts below reflect the local checkout when this catalog was written. They are
inventory checks, not promises about every future machine.

| Root | Directories | Files | CSV | JSON | Markdown | Logs | Purpose |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `target/embedding-research/` | 1,505 | 24,794 | 824 | 1,752 | 518 | 14,897 | GPU fair benchmark runs, Run 2 controls/retrieval/finalists, quantization probes, per-query ranks, manifests, and per-case logs. |
| `target/autoresearch/indexer-embedder/` | 1,194 | 24,581 | 744 | 1,746 | 626 | 16,222 | Later autoresearch runs for pipeline scoring, compact stored vectors, cache/scoring experiments, and local promotion candidates. |
| `target/autoresearch/cross-repo-promotion/` | 809 | 18,585 | 59 | 399 | 59 | 11,348 | External gates over sibling repos and focused cross-repo follow-up probes. |

Common artifact files across those roots:

| File name | Count | Why it matters |
| --- | ---: | --- |
| `manifest.json` | 555 | Run metadata, selected cases, source ledger, artifact paths, and provider details. |
| `sources.md` | 555 | Human source notes and blocked-candidate notes for each run. |
| `results.json` | 452 | Machine-readable row metrics and skip/failure state. |
| `results.csv` | 393 | Spreadsheet-friendly row metrics. |
| `query-ranks.csv` | 452 | Per-query ranks, persistent misses, and row-level retrieval evidence. |
| `repeat-summary.csv` | 388 | Repeat averages for finalist or repeated cases. |
| `summary.md` | 251 | Human summaries for autoresearch and cross-repo runs. |

## Canonical Evidence Roots

These are the raw locations most likely to answer "why did we decide that?"

| Evidence | Roots |
| --- | --- |
| Accepted runtime default and current model/backend decision sheet | `docs/testing/embedding-backend-benchmarks.md`, plus the benchmark roots listed in that file. |
| Run 2 controls and retrieval isolation | `target/embedding-research/controls-run2-20260419`, `target/embedding-research/retrieval-run2-20260419`, and the Run 2 speedcheck roots. |
| BGE-small crossed full-query repeats | `target/embedding-research/autoresearch-bge-small-crossed-full-run1-20260421T051205Z`, `target/embedding-research/autoresearch-bge-small-crossed-full-run2-20260421T051606Z`, `target/embedding-research/autoresearch-bge-small-crossed-full-run3-20260421T051948Z`. |
| BGE-base llama.cpp b512/r4 leader repeats | `target/embedding-research/ar-bgeb-b512-r4-20260421T145924Z`, `target/embedding-research/ar-bgeb-b512-r4-r2-20260421T150215Z`, `target/embedding-research/ar-bgeb-b512-r4-r3-20260421T150640Z`. |
| Clean-source BGE-base Q5_K_M b512/r4 compression repeats | `target/embedding-research/ar-q5-b512-r1-20260421T162402Z`, `target/embedding-research/ar-q5-b512-r2-20260421T162659Z`, `target/embedding-research/ar-q5-b512-r3-20260421T162950Z`. |
| 60/30/10 compact-storage local promotion | `target/autoresearch/indexer-embedder/20260423T015924`, `target/autoresearch/indexer-embedder/20260423T020818`, `target/autoresearch/indexer-embedder/20260423T022445`, `target/autoresearch/indexer-embedder/20260423T024103`. |
| 60/30/10 compact-storage cross-repo gate | `target/autoresearch/cross-repo-promotion/20260423022731`, `target/autoresearch/cross-repo-promotion/20260423024405`. |

## Preservation Rules

- Do not delete raw roots after a successful research loop until the tracked
  synthesis has been updated and the user no longer needs the local evidence.
- Do not commit generated caches, SQLite databases, Tantivy search indexes, or
  model weights. Commit only readable synthesis and small scripts.
- If a result might affect defaults, keep `results.csv`, `results.json`,
  `query-ranks.csv`, provider logs, `manifest.json`, and `sources.md`.
- If the data must leave this machine, archive the raw roots outside git and
  record the archive location in this catalog or the release note that uses it.
- If a run is query-sliced, label it as exploratory in the doc that cites it.
  Promotion evidence needs full-query repeats and provider verification.

## Refresh Inventory Commands

Use these commands from the repo root when updating this catalog:

```powershell
$roots = @(
  'target\embedding-research',
  'target\autoresearch\indexer-embedder',
  'target\autoresearch\cross-repo-promotion'
)

$rows = @()
foreach ($root in $roots) {
  if (Test-Path $root) {
    $dirs = @(Get-ChildItem -Path $root -Directory -Recurse)
    $files = @(Get-ChildItem -Path $root -File -Recurse)
    $rows += [PSCustomObject]@{
      Root = $root
      Directories = $dirs.Count
      Files = $files.Count
      Csv = ($files | Where-Object Extension -eq '.csv').Count
      Json = ($files | Where-Object Extension -eq '.json').Count
      Markdown = ($files | Where-Object Extension -eq '.md').Count
      Logs = ($files | Where-Object Extension -eq '.log').Count
    }
  }
}
$rows | Format-Table -AutoSize

Get-ChildItem -Path $roots -Recurse -File -Include `
  results.csv,results.json,query-ranks.csv,repeat-summary.csv,manifest.json,summary.md,sources.md |
  Group-Object Name |
  Sort-Object Name |
  Select-Object Name,Count |
  Format-Table -AutoSize
```
