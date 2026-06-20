[CmdletBinding()]
param(
    [string]$Project = "."
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

function Find-CodeStoryCli {
    param([string]$Root)

    $candidates = @(
        (Join-Path $Root "target\release\codestory-cli.exe"),
        (Join-Path $Root "target\release\codestory-cli")
    )
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate) {
            return $candidate
        }
    }
    throw "codestory-cli release binary was not found under target\release"
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

    Invoke-SetupStep "Build release CLI" {
        Invoke-Checked "cargo" @("build", "--release", "-p", "codestory-cli")
    }

    $cli = Find-CodeStoryCli $projectPath
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
