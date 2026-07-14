[CmdletBinding()]
param(
    [string]$Project = ".",
    [string]$IntendedBaseRef = $(if ($env:CODESTORY_INTENDED_BASE_REF) { $env:CODESTORY_INTENDED_BASE_REF } else { "origin/dev/codestory-next" }),
    [string]$PrHeadRef = $env:CODESTORY_PR_HEAD_REF,
    [switch]$BranchHeadProof,
    [switch]$ResolveCliOnly,
    [switch]$SelfTest,
    [switch]$Help
)

$ErrorActionPreference = "Stop"
$node = if ($env:CODESTORY_NODE) {
    $env:CODESTORY_NODE
} else {
    (Get-Command node -ErrorAction Stop).Source
}
$dispatcher = Join-Path $PSScriptRoot "codex-worktree-setup.mjs"
$arguments = @("--project", $Project, "--intended-base-ref", $IntendedBaseRef)
if ($PrHeadRef) { $arguments += @("--pr-head-ref", $PrHeadRef) }
if ($BranchHeadProof) { $arguments += "--branch-head-proof" }
if ($ResolveCliOnly) { $arguments += "--resolve-cli-only" }
if ($SelfTest) { $arguments += "--self-test" }
if ($Help) { $arguments += "--help" }

& $node $dispatcher @arguments
exit $LASTEXITCODE
