$ErrorActionPreference = "Stop"

# Add correctness checks here. Keep success output quiet and failures actionable.
function Invoke-CheckStep {
    param(
        [Parameter(Mandatory = $true)]
        [string] $Name,
        [Parameter(Mandatory = $true)]
        [scriptblock] $Command
    )

    Write-Host "check: $Name"
    $global:LASTEXITCODE = 0
    & $Command
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

Invoke-CheckStep "runtime semantic doc budgeting" { cargo test -p codestory-runtime semantic_doc --lib }
Invoke-CheckStep "cli output format contracts" { cargo test -p codestory-cli non_trail_commands_reject_dot_format_before_running }
Invoke-CheckStep "runtime-backed query/trail contracts" { cargo test -p codestory-cli --test runtime_backed_flows -- --ignored }
