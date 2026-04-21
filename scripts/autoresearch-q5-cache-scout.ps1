param(
    [ValidateSet("ub1024-lex0p5", "ub1024-lex0p1", "ub1024-lex0p01", "ub1024-lex0", "no-ubatch-lex1")]
    [string]$Case,

    [ValidateSet(1, 2, 3)]
    [int]$Repeat = 1
)

$ErrorActionPreference = "Stop"

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$ubatchSource = Join-Path $root "target\embedding-research\ar-q5-b512-ub1024-r3-20260421T172657Z\llama-bge-base-alias_variant-scope-all-b512-84947bbe77"
$nonUbatchSource = Join-Path $root "target\embedding-research\ar-q5-b512-r3-20260421T162950Z\llama-bge-base-alias_variant-scope-all-b512-aee5fe6e9e"

$caseIds = @{
    "ub1024-lex0p5" = "llama-bge-base-alias_variant-scope-all-b512-r4-np4-ctx4096-pool-cls-frontier-q5-b512-r4-ub1024-semantic995-lex0p5-slim8-q-q5_k_m-w0.005-0.995-0-lim-l20-s8-run$Repeat"
    "ub1024-lex0p1" = "llama-bge-base-alias_variant-scope-all-b512-r4-np4-ctx4096-pool-cls-frontier-q5-b512-r4-ub1024-semantic999-lex0p1-slim8-q-q5_k_m-w0.001-0.999-0-lim-l20-s8-run$Repeat"
    "ub1024-lex0p01" = "llama-bge-base-alias_variant-scope-all-b512-r4-np4-ctx4096-pool-cls-frontier-q5-b512-r4-ub1024-semantic9999-lex0p01-slim8-q-q5_k_m-w0.0001-0.9999-0-lim-l20-s8-run$Repeat"
    "ub1024-lex0" = "llama-bge-base-alias_variant-scope-all-b512-r4-np4-ctx4096-pool-cls-frontier-q5-b512-r4-ub1024-pure-semantic-lex0-slim8-q-q5_k_m-w0-1-0-lim-l0-s8-run$Repeat"
    "no-ubatch-lex1" = "llama-bge-base-alias_variant-scope-all-b512-r4-np4-ctx4096-pool-cls-frontier-q5-b512-r4-semantic99-lex1-slim8-q-q5_k_m-w0.01-0.99-0-lim-l20-s8-run$Repeat"
}

$cacheSources = @{
    "ub1024-lex0p5" = $ubatchSource
    "ub1024-lex0p1" = $ubatchSource
    "ub1024-lex0p01" = $ubatchSource
    "ub1024-lex0" = $ubatchSource
    "no-ubatch-lex1" = $nonUbatchSource
}

if (-not (Test-Path $cacheSources[$Case])) {
    throw "Missing cache source for $Case at $($cacheSources[$Case])"
}

$env:AUTORESEARCH_STAGE = "finalists2"
$env:AUTORESEARCH_CASES = $caseIds[$Case]
$env:AUTORESEARCH_QUERY_BUCKETS = "__all"
$env:AUTORESEARCH_OUT_LABEL = "ar-cache-q5-$Case-r$Repeat"
$env:CODESTORY_EMBED_RESEARCH_CACHE_FROM = $cacheSources[$Case]

& (Join-Path $root "autoresearch.ps1")
exit $LASTEXITCODE
