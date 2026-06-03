# Optional Windows entry point for scripts/setup-retrieval-env.mjs.
# Primary path: cargo retrieval-setup (from repo root).
[CmdletBinding()]
param(
    [switch]$CheckOnly,
    [switch]$DryRun,
    [switch]$SkipBuild,
    [switch]$SkipCompose,
    [switch]$SkipStatus,
    [switch]$WithHoldoutClone,
    [switch]$Release,
    [string]$Project,
    [int]$WaitSecs = 90
)

$ErrorActionPreference = "Stop"

function Require-Command {
    param([string]$Name)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Required command '$Name' was not found on PATH."
    }
}

Require-Command node

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $scriptDir "..")
$mjs = Join-Path $scriptDir "setup-retrieval-env.mjs"

$nodeArgs = @($mjs)
if ($CheckOnly -or $DryRun) { $nodeArgs += "--check-only" }
if ($SkipBuild) { $nodeArgs += "--skip-build" }
if ($SkipCompose) { $nodeArgs += "--skip-compose" }
if ($SkipStatus) { $nodeArgs += "--skip-status" }
if ($WithHoldoutClone) { $nodeArgs += "--with-holdout-clone" }
if ($Release) { $nodeArgs += "--release" }
if ($Project) { $nodeArgs += @("--project", (Resolve-Path $Project).Path) }
if ($WaitSecs -ge 0) { $nodeArgs += @("--wait-secs", "$WaitSecs") }

& node @nodeArgs
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
