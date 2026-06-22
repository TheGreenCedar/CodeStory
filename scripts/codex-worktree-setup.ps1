[CmdletBinding()]
param(
    [string]$Project = ".",
    [switch]$ResolveCliOnly,
    [switch]$SelfTest
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

function Get-ReadinessVerdict {
    param(
        $Doctor,
        [string]$Goal
    )

    foreach ($verdict in @($Doctor.readiness)) {
        if ($verdict.goal -eq $Goal) {
            return $verdict
        }
    }

    return $null
}

function Test-ReadinessReady {
    param($Verdict)

    return ($Verdict -and $Verdict.status -eq "ready")
}

function Get-MinimumNextCommand {
    param(
        $LocalVerdict,
        $AgentVerdict,
        $Doctor
    )

    foreach ($verdict in @($LocalVerdict, $AgentVerdict)) {
        if (-not (Test-ReadinessReady $verdict)) {
            foreach ($command in @($verdict.minimum_next)) {
                if ($command) {
                    return [string]$command
                }
            }
        }
    }

    foreach ($command in @($Doctor.next_commands)) {
        if ($command) {
            return [string]$command
        }
    }

    return $null
}

function Invoke-DoctorJson {
    param(
        [string]$Cli,
        [string]$ProjectPath
    )

    $json = (& $Cli doctor --project $ProjectPath --format json 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) {
        throw "codestory-cli doctor failed with exit code $LASTEXITCODE`n$json"
    }

    return ($json | ConvertFrom-Json)
}

function Write-DoctorReadinessSummary {
    param($Doctor)

    $local = Get-ReadinessVerdict $Doctor "local_navigation"
    $agent = Get-ReadinessVerdict $Doctor "agent_packet_search"
    $minimumNext = Get-MinimumNextCommand $local $agent $Doctor

    Write-Host "CodeStory worktree readiness"
    Write-Host ("  local_navigation: {0}" -f $(if ($local) { $local.status } else { "unknown" }))
    if ($local -and $local.summary) {
        Write-Host ("    reason: {0}" -f $local.summary)
    }
    Write-Host ("  agent_packet_search: {0}" -f $(if ($agent) { $agent.status } else { "unknown" }))
    if ($agent -and $agent.summary) {
        Write-Host ("    reason: {0}" -f $agent.summary)
    }
    Write-Host ("  retrieval_mode: {0}" -f $Doctor.retrieval_mode)
    Write-Host ("  degraded_reason: {0}" -f $(if ($Doctor.degraded_reason) { $Doctor.degraded_reason } else { "none" }))
    if ($minimumNext) {
        Write-Host ("  minimum_next: {0}" -f $minimumNext)
    }
    if (-not (Test-ReadinessReady $agent)) {
        Write-Host "  handoff: CodeStory packet/search is unavailable; use direct source reads until the minimum_next command repairs readiness."
    }
}

function Test-CodeStoryCli {
    param(
        [string]$Candidate,
        [string]$ExpectedVersion,
        [ref]$ActualVersion
    )

    if (-not $Candidate) {
        return $false
    }
    if (-not (Get-Command $Candidate -ErrorAction SilentlyContinue)) {
        return $false
    }

    $versionOutput = & $Candidate --version 2>$null
    if ($LASTEXITCODE -ne 0) {
        return $false
    }
    $versionLine = $versionOutput | Select-Object -First 1
    if ($versionLine -notmatch '^codestory-cli\s+([0-9][0-9A-Za-z.+-]*)$') {
        return $false
    }

    $ActualVersion.Value = $Matches[1]
    return $ActualVersion.Value -eq $ExpectedVersion
}

function Get-ExpectedCodeStoryCliVersion {
    param([string]$Root)

    $manifest = Join-Path $Root "crates\codestory-cli\Cargo.toml"
    foreach ($line in Get-Content -LiteralPath $manifest) {
        if ($line -match '^\s*version\s*=\s*"([^"]+)"') {
            return $Matches[1]
        }
    }

    throw "Unable to read expected codestory-cli version from $manifest."
}

function Get-CodeStoryInstallDir {
    if ($env:CODESTORY_HOME) {
        return (Join-Path $env:CODESTORY_HOME "bin")
    }
    if ($env:LOCALAPPDATA) {
        return (Join-Path $env:LOCALAPPDATA "CodeStory\bin")
    }
    return (Join-Path $HOME ".codestory\bin")
}

function Get-VersionedCodeStoryInstallDir {
    param([string]$Version)

    return (Join-Path (Join-Path (Get-CodeStoryInstallDir) "releases") $Version)
}

function Get-CodeStoryCliCandidates {
    param(
        [string]$Root,
        [string]$ExpectedVersion
    )

    $candidates = @()
    if ($env:CODESTORY_CLI) {
        $candidates += $env:CODESTORY_CLI
    }

    $pathCli = Get-Command "codestory-cli" -ErrorAction SilentlyContinue
    if ($pathCli) {
        $candidates += $pathCli.Source
    }

    $installDir = Get-CodeStoryInstallDir
    $candidates += @(
        (Join-Path $installDir "codestory-cli.exe"),
        (Join-Path $installDir "codestory-cli.cmd"),
        (Join-Path $installDir "codestory-cli")
    )
    if ($ExpectedVersion) {
        $versionedInstallDir = Get-VersionedCodeStoryInstallDir $ExpectedVersion
        $candidates += @(
            (Join-Path $versionedInstallDir "codestory-cli.exe"),
            (Join-Path $versionedInstallDir "codestory-cli.cmd"),
            (Join-Path $versionedInstallDir "codestory-cli")
        )
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

    $expectedVersion = Get-ExpectedCodeStoryCliVersion $Root
    $candidates = Get-CodeStoryCliCandidates $Root $expectedVersion
    $staleCandidates = @()
    foreach ($candidate in $candidates) {
        $actualVersion = $null
        if (Test-CodeStoryCli $candidate $expectedVersion ([ref]$actualVersion)) {
            return $candidate
        }
        if ($actualVersion) {
            $staleCandidates += "$candidate reported $actualVersion"
        }
    }

    $message = "No ready codestory-cli $expectedVersion found via CODESTORY_CLI, PATH, this worktree's target\release, or sibling worktree target\release directories."
    if ($staleCandidates.Count -gt 0) {
        $message += " Stale candidates: $($staleCandidates -join '; ')."
    }
    throw $message
}

function Invoke-CurrentReleaseCliInstall {
    param(
        [string]$Root,
        [string]$ExpectedVersion
    )

    $installer = Join-Path $PSScriptRoot "install-codestory.ps1"
    if (-not (Test-Path -LiteralPath $installer)) {
        throw "Current-release installer is missing: $installer"
    }

    Write-Host ""
    Write-Host "==> Install current release CLI"
    Write-Host "Trying codestory-cli $ExpectedVersion release install before Cargo build."
    & $installer -Project $Root -Version $ExpectedVersion
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

function Assert-SelfTest {
    param(
        [bool]$Condition,
        [string]$Message
    )

    if (-not $Condition) {
        throw "Self-test failed: $Message"
    }
}

function Remove-SetupSelfTestTemp {
    param([string]$Path)

    $resolved = Resolve-Path -LiteralPath $Path -ErrorAction SilentlyContinue
    if (-not $resolved) {
        return
    }
    $tempRoot = [System.IO.Path]::GetTempPath()
    $leaf = Split-Path -Leaf $resolved.Path
    if (-not $resolved.Path.StartsWith($tempRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove setup self-test temp outside system temp: $($resolved.Path)"
    }
    if (-not $leaf.StartsWith("codestory-setup-", [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove unexpected setup self-test temp: $($resolved.Path)"
    }
    Remove-Item -LiteralPath $resolved.Path -Recurse -Force
}

function Invoke-SelfTest {
    $oldCodeStoryHome = $env:CODESTORY_HOME
    $oldCodeStoryCli = $env:CODESTORY_CLI
    $oldPath = $env:PATH
    $tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("codestory-setup-" + [System.Guid]::NewGuid().ToString("N"))
    try {
        $isolatedPath = Join-Path $tempRoot "path"
        New-Item -ItemType Directory -Force -Path $isolatedPath | Out-Null
        $env:CODESTORY_CLI = $null
        $env:PATH = $isolatedPath

        $projectRoot = Join-Path $tempRoot "project"
        $manifestDir = Join-Path $projectRoot "crates\codestory-cli"
        New-Item -ItemType Directory -Force -Path $manifestDir | Out-Null
        Set-Content -LiteralPath (Join-Path $manifestDir "Cargo.toml") -Value 'version = "0.11.4"' -Encoding ASCII

        $env:CODESTORY_HOME = Join-Path $tempRoot "home"
        $installDir = Get-CodeStoryInstallDir
        $versionedInstallDir = Get-VersionedCodeStoryInstallDir "0.11.4"
        New-Item -ItemType Directory -Force -Path $installDir, $versionedInstallDir | Out-Null
        Set-Content -LiteralPath (Join-Path $installDir "codestory-cli.cmd") -Value "@echo codestory-cli 0.11.3" -Encoding ASCII
        $currentCli = Join-Path $versionedInstallDir "codestory-cli.cmd"
        Set-Content -LiteralPath $currentCli -Value "@echo codestory-cli 0.11.4" -Encoding ASCII

        $resolvedCli = Find-CodeStoryCli $projectRoot
        Assert-SelfTest (Same-Path $resolvedCli $currentCli) "stale default install should be rejected and versioned current install should be used"
    } finally {
        $env:CODESTORY_HOME = $oldCodeStoryHome
        $env:CODESTORY_CLI = $oldCodeStoryCli
        $env:PATH = $oldPath
        Remove-SetupSelfTestTemp $tempRoot
    }

    Write-Host "codex-worktree-setup self-test: ok"
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

if ($SelfTest) {
    Invoke-SelfTest
    return
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
        $resolveError = $_.Exception.Message
        $expectedVersion = Get-ExpectedCodeStoryCliVersion $projectPath
        try {
            Invoke-CurrentReleaseCliInstall $projectPath $expectedVersion
            $cli = Find-CodeStoryCli $projectPath
        } catch {
            $installError = $_.Exception.Message
            $installCommand = ".\scripts\install-codestory.ps1 -Project . -Version $expectedVersion"
            if ($ResolveCliOnly) {
                throw "$resolveError Current-release install failed: $installError. Run $installCommand, or set CODESTORY_CLI to a ready binary."
            }
            Write-Host ""
            Write-Host "==> Build release CLI"
            Write-Warning "$resolveError Current-release install failed: $installError. Building release CLI with cargo."
            Invoke-Checked "cargo" @("build", "--release", "-p", "codestory-cli")
            $cli = Find-CodeStoryCli $projectPath
        }
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

    Invoke-SetupStep "Doctor readiness handoff" {
        $doctor = Invoke-DoctorJson $cli $projectPath
        Write-DoctorReadinessSummary $doctor
    } -Optional
} finally {
    Pop-Location
}
