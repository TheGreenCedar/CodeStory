$ErrorActionPreference = "Stop"

# Add correctness checks here. Keep success output quiet and failures actionable.
$global:LASTEXITCODE = 0
cargo test -p codestory-cli --test cli_golden_path
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
