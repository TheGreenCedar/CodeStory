# OSS Language Corpus

The OSS language corpus is an ignored, opt-in test suite for checking each
public language-support profile against a pinned medium-sized open source project.
It is intentionally outside the default test lane because it clones external
repositories and can take several minutes.

The suite has two sides for each language:

- `raw_without_codestory`: a plain `std::fs` crawl of the pinned checkout. This
  code does not call CodeStory workspace discovery, indexing, runtime, or store
  APIs. It counts files and LOC for the language's supported extensions.
- `with_codestory`: CodeStory indexes the exact raw file list into an in-memory
  store. The suite compares stored/indexed file counts, node counts, edge
  counts, errors, and timing stats against thresholds.

This is a language-indexing corpus, not an agent answer-quality or agent-cost
benchmark. It does not measure tokens, tool calls, command counts, or elapsed
agent time. Use the `language-expansion-holdout` agent A/B suite when the
question is whether CodeStory improves an agent over raw source access.

The paired A/B suite lives at
`benchmarks/tasks/language-expansion-holdout/language-support-ab.task.json` and
uses the same pinned projects. It compares `without_codestory` against
`with_codestory` and reports time, tokens, estimated cost, observed tool calls,
command counts, source reads, post-packet source reads, and manifest quality
scores. Its `summary.json` / `reanalyzed-summary.json` files include a
`cost_accounting` block that totals those costs per arm and compares
`with_codestory` against `without_codestory`. The no-CodeStory arm counts a
harness-run local `rg` plus bounded source-read prelude before the nested agent
starts. The CodeStory arm counts a harness-run packet prelude before the nested
agent starts. A baseline row cannot be promoted if it uses CodeStory or never
inspects the local repository.

## Commands

Validate the manifest without cloning:

```powershell
$env:CODESTORY_OSS_CORPUS_DRY_RUN = "1"
cargo test -p codestory-indexer --test oss_language_corpus -- --ignored --nocapture
Remove-Item Env:CODESTORY_OSS_CORPUS_DRY_RUN
```

Run one or more languages:

```powershell
$env:CODESTORY_RUN_OSS_LANGUAGE_CORPUS = "1"
$env:CODESTORY_OSS_CORPUS_LANGUAGES = "python,go"
cargo test -p codestory-indexer --test oss_language_corpus -- --ignored --nocapture
Remove-Item Env:CODESTORY_RUN_OSS_LANGUAGE_CORPUS
Remove-Item Env:CODESTORY_OSS_CORPUS_LANGUAGES
```

Run the full corpus:

```powershell
$env:CODESTORY_RUN_OSS_LANGUAGE_CORPUS = "1"
cargo test -p codestory-indexer --test oss_language_corpus -- --ignored --nocapture
Remove-Item Env:CODESTORY_RUN_OSS_LANGUAGE_CORPUS
```

Run the paired agent A/B suite instead:

```powershell
node scripts/codestory-agent-ab-benchmark.mjs `
  --task-suite language-expansion-holdout `
  --arms without_codestory,with_codestory `
  --repeats 3 --materialize-repos --prepare-codestory-cache `
  --out-dir target/agent-benchmark/language-expansion-holdout `
  --timeout-ms 600000
```

By default, checkouts are cached in
`target/oss-language-corpus/repos`. To use another cache directory:

```powershell
$env:CODESTORY_OSS_CORPUS_CACHE = "D:\codestory-oss-corpus"
```

The latest JSONL report is written to:

```text
target/oss-language-corpus/reports/oss-language-corpus-latest.jsonl
```

## Latest Verification

Last checked: 2026-06-12.

The full ignored corpus was run against the materialized benchmark repo cache:

```powershell
$env:CODESTORY_RUN_OSS_LANGUAGE_CORPUS = "1"
$env:CODESTORY_OSS_CORPUS_CACHE = "target\agent-benchmark\repos"
cargo test -p codestory-indexer --test oss_language_corpus -- --ignored --nocapture
```

Result: 18/18 public language-support profiles passed the indexing-only corpus. The run compared 4,308 raw files and
1,272,498 raw LOC against CodeStory indexing of the same file lists. CodeStory
indexed 4,308 files and produced 385,735 nodes and 312,268 edges with 0 errors
and 0 fatal errors. The latest per-language JSONL evidence is in
`target/oss-language-corpus/reports/oss-language-corpus-latest.jsonl`.

The cheap integrity check used by the Autoresearch gate is:

```powershell
node scripts\codestory-language-holdout-integrity.mjs
```

It validates the recorded artifact shape and provenance: all 18
language-expansion repos are materialized at their manifest commits, and the
latest OSS corpus report has 18 passed rows with matching raw/indexed file
counts and zero errors. It is not a fresh indexing run unless the corpus test is
rerun with `CODESTORY_RUN_OSS_LANGUAGE_CORPUS=1`.

## Manifest

| Language | Project | Pinned commit |
| --- | --- | --- |
| Python | `psf/requests` | `6f66281a1d6326b1b9c4ac09ca30de0fc4e6ef43` |
| Java | `apache/commons-lang` | `57f39420fef8413ea42f045f1bdba4864ff75a0c` |
| Rust | `BurntSushi/ripgrep` | `82313cf95849bfe425109ad9506a52154879b1b1` |
| JavaScript | `expressjs/express` | `dae209ae6559c29cfca2a1f4414c51d89ea643d5` |
| TypeScript/TSX | `vercel/swr` | `f8d4995ac555f02a2784c8fc40bc819782c60568` |
| C++ | `fmtlib/fmt` | `e8deaf2ec3b53ced589fce6f640061e5b32eeeaa` |
| C | `redis/redis` | `df63a65d4d4ee33ae67e9f101885074febe0bccb` |
| Go | `gin-gonic/gin` | `d75fcd4c9ab260e5225de590f1f0f8c0e0e12d11` |
| Ruby | `jekyll/jekyll` | `202df571314ba1d18e9fccd81d12aaad4a703c38` |
| PHP | `Seldaek/monolog` | `04c3499db98d7471abd9261dc83232f8fe1a252d` |
| C# | `AutoMapper/AutoMapper` | `b57c206dc7291821e42bdf816a5637a5c1d8cb54` |
| Kotlin | `square/okio` | `722c8be0043d99b7b08d169b0ae90a24c15267ff` |
| Swift | `Alamofire/Alamofire` | `7595cbcf59809f9977c5f6378500de2ad73b7ddb` |
| Dart | `dart-lang/http` | `89cec60a4249ae0a0316f7a50d37ac56597f52c3` |
| Bash | `nvm-sh/nvm` | `7079a5d61c2b49c7d35a72006860ce5edb0fac51` |
| HTML | `mdn/learning-area` | `ca1ff0bd06e12b96a6742ffdf040bb22966e5a5e` |
| CSS | `animate-css/animate.css` | `3f8ab233dbbd9d2fe577528d2296382954be3d1a` |
| SQL | `lerocha/chinook-database` | `7f67772503d71ba90f19283c38e93923addb43fa` |

## Maintenance Rule

Every language returned by `language_support_profile_for_language_name` must
have exactly one corpus entry. The dry-run mode validates that the manifest and
the runtime support map stay aligned, so a future language addition must also
add a pinned OSS project before the manifest check passes.
