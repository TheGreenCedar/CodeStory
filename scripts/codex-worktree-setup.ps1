[CmdletBinding()]
param(
    [string]$Project = ".",
    [switch]$ResolveCliOnly
)

$ErrorActionPreference = "Stop"

function Invoke-Checked {
    param(
        [string]$File,
        [string[]]$Arguments
    )

    & $File @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$File $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

function Invoke-SetupStep {
    param(
        [string]$Label,
        [scriptblock]$Step,
        [switch]$Optional
    )

    Write-Host ""
    Write-Host "==> $Label"
    try {
        & $Step
    } catch {
        if (-not $Optional) {
            throw
        }
        Write-Warning $_.Exception.Message
    }
}

function Test-CodeStoryCli {
    param([string]$Candidate)

    if (-not $Candidate) {
        return $false
    }
    if (-not (Get-Command $Candidate -ErrorAction SilentlyContinue)) {
        return $false
    }

    # ponytail: --help proves the binary is runnable; doctor remains the real trust gate.
    & $Candidate --help >$null 2>$null
    return $LASTEXITCODE -eq 0
}

function Get-CodeStoryCliCandidates {
    param([string]$Root)

    $candidates = @()
    if ($env:CODESTORY_CLI) {
        $candidates += $env:CODESTORY_CLI
    }

    $pathCli = Get-Command "codestory-cli" -ErrorAction SilentlyContinue
    if ($pathCli) {
        $candidates += $pathCli.Source
    }

    $candidates += @(
        (Join-Path $Root "target\release\codestory-cli.exe"),
        (Join-Path $Root "target\release\codestory-cli")
    )

    $lines = if (Get-Command git -ErrorAction SilentlyContinue) {
        & git worktree list --porcelain 2>$null
    } else {
        @()
    }
    if ($LASTEXITCODE -eq 0) {
        foreach ($line in $lines) {
            if (-not $line.StartsWith("worktree ")) {
                continue
            }
            $candidateRoot = $line.Substring("worktree ".Length)
            if (Same-Path $candidateRoot $Root) {
                continue
            }
            $candidates += (Join-Path $candidateRoot "target\release\codestory-cli.exe")
            $candidates += (Join-Path $candidateRoot "target\release\codestory-cli")
        }
    }

    return $candidates | Where-Object { $_ } | Select-Object -Unique
}

function Find-CodeStoryCli {
    param([string]$Root)

    $candidates = Get-CodeStoryCliCandidates $Root
    foreach ($candidate in $candidates) {
        if (Test-CodeStoryCli $candidate) {
            return $candidate
        }
    }

    throw "No ready codestory-cli found via CODESTORY_CLI, PATH, this worktree's target\release, or sibling worktree target\release directories."
}

function Same-Path {
    param(
        [string]$Left,
        [string]$Right
    )

    $trimChars = [char[]]@('\', '/')
    $leftFull = [System.IO.Path]::GetFullPath($Left).TrimEnd($trimChars)
    $rightFull = [System.IO.Path]::GetFullPath($Right).TrimEnd($trimChars)
    return [string]::Equals($leftFull, $rightFull, [System.StringComparison]::OrdinalIgnoreCase)
}

function Find-RehydrateSource {
    param([string]$Target)

    if ($env:CODESTORY_REHYDRATE_FROM) {
        try {
            $configured = (Resolve-Path -LiteralPath $env:CODESTORY_REHYDRATE_FROM -ErrorAction Stop).Path
        } catch {
            Write-Warning "Ignoring CODESTORY_REHYDRATE_FROM='$env:CODESTORY_REHYDRATE_FROM': $($_.Exception.Message)"
            $configured = $null
        }
        if ($configured -and -not (Same-Path $configured $Target)) {
            return $configured
        }
    }

    $lines = & git worktree list --porcelain 2>$null
    if ($LASTEXITCODE -ne 0) {
        return $null
    }

    foreach ($line in $lines) {
        if (-not $line.StartsWith("worktree ")) {
            continue
        }
        $candidate = $line.Substring("worktree ".Length)
        if (Same-Path $candidate $Target) {
            continue
        }
        if (Test-Path -LiteralPath (Join-Path $candidate "Cargo.toml")) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }

    return $null
}

$projectPath = (Resolve-Path -LiteralPath $Project).Path

Push-Location $projectPath
try {
    $sccache = Join-Path $env:USERPROFILE ".cargo\bin\sccache.exe"
    if (Test-Path -LiteralPath $sccache) {
        $env:RUSTC_WRAPPER = $sccache
        Write-Host "Using RUSTC_WRAPPER=$sccache"
    }

    try {
        $cli = Find-CodeStoryCli $projectPath
    } catch {
        if ($ResolveCliOnly) {
            throw "$($_.Exception.Message) Re-run without -ResolveCliOnly to build with cargo, or set CODESTORY_CLI to a ready binary."
        }
        Write-Host ""
        Write-Host "==> Build release CLI"
        Write-Warning "$($_.Exception.Message) Building release CLI with cargo."
        Invoke-Checked "cargo" @("build", "--release", "-p", "codestory-cli")
        $cli = Find-CodeStoryCli $projectPath
    }

    Write-Host "CODESTORY_CLI=$cli"
    if ($ResolveCliOnly) {
        return
    }

    $source = Find-RehydrateSource $projectPath
    if ($source) {
        Invoke-SetupStep "Rehydrate CodeStory cache from $source" {
            Invoke-Checked $cli @("cache", "rehydrate", "--from-project", $source, "--project", $projectPath)
        } -Optional
    } else {
        Write-Host ""
        Write-Host "==> Rehydrate CodeStory cache"
        Write-Host "No sibling source worktree found; refreshing this worktree directly."
    }

    Invoke-SetupStep "Refresh SQLite graph/search/doc cache" {
        Invoke-Checked $cli @("index", "--project", $projectPath, "--refresh", "auto")
    }

    Invoke-SetupStep "Bootstrap retrieval sidecars" {
        Invoke-Checked $cli @("retrieval", "bootstrap", "--project", $projectPath, "--wait-secs", "90")
    } -Optional

    Invoke-SetupStep "Refresh retrieval sidecar index" {
        Invoke-Checked $cli @("retrieval", "index", "--project", $projectPath, "--refresh", "auto")
    } -Optional

    Invoke-SetupStep "Doctor" {
        Invoke-Checked $cli @("doctor", "--project", $projectPath, "--format", "markdown")
    } -Optional
} finally {
    Pop-Location
}
