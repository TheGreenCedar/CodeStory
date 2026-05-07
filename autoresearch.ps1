$ErrorActionPreference = "Stop"

# This recipe command is responsible for printing METRIC lines.
$global:LASTEXITCODE = 0
node scripts/codestory-manual-friction-check.mjs --setup-embeddings
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
