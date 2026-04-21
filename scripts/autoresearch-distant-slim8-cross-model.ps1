param(
    [ValidateSet(1, 2, 3)]
    [int]$Repeat = 1
)

$ErrorActionPreference = "Stop"

$cases = @(
    "onnx-bge-small-no_alias-scope-all-b256-s2-fast-profile-semantic99-lex1-slim8-w0.01-0.99-0-lim-l20-s8-run$Repeat",
    "onnx-bge-base-alias_variant-scope-all-b256-s2-onnx-semantic99-lex1-slim8-w0.01-0.99-0-lim-l20-s8-run$Repeat",
    "onnx-minilm-no_alias-scope-all-b256-s2-minilm-semantic95-lex5-slim8-w0.05-0.95-0-lim-l20-s8-run$Repeat",
    "llama-gemma-no_alias-scope-all-b128-r2-np2-ctx4096-pool-mean-gemma-semantic95-lex5-slim8-w0.05-0.95-0-lim-l20-s8-run$Repeat"
)

$env:AUTORESEARCH_STAGE = "finalists2"
$env:AUTORESEARCH_CASES = $cases -join ","
$env:AUTORESEARCH_QUERY_BUCKETS = "__all"
$env:AUTORESEARCH_OUT_LABEL = "ar-distant-slim8-cross-model-r$Repeat"
Remove-Item Env:CODESTORY_EMBED_RESEARCH_CACHE_FROM -ErrorAction SilentlyContinue

& (Join-Path $PSScriptRoot "..\autoresearch.ps1")
exit $LASTEXITCODE
