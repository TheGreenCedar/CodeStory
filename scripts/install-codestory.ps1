#Requires -Version 5.1
[CmdletBinding()]
param(
    [string]$Project = ".",
    [string]$CodestoryCli,
    [string]$InstallDir,
    [string]$Version,
    [switch]$NoDownload,
    [switch]$SelfTest
)

$ErrorActionPreference = "Stop"
$RequiredVersion = $null

function Convert-ReleaseTagToVersion {
    param([string]$Tag)

    if ($Tag -match "^v?(\d+\.\d+\.\d+)$") {
        return $matches[1]
    }
    throw "Unable to parse CodeStory release tag: $Tag"
}

function Get-LatestReleaseVersion {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/TheGreenCedar/CodeStory/releases/latest" -UseBasicParsing
    return Convert-ReleaseTagToVersion $release.tag_name
}

function Set-RequiredVersion {
    param([string]$ReleaseVersion)

    $script:RequiredVersion = [Version](Convert-ReleaseTagToVersion $ReleaseVersion)
}

function Get-CodeStoryHome {
    if ($env:CODESTORY_HOME) {
        return $env:CODESTORY_HOME
    }
    if ($env:LOCALAPPDATA) {
        return (Join-Path $env:LOCALAPPDATA "CodeStory")
    }
    return (Join-Path $HOME ".codestory")
}

function Get-DefaultInstallDir {
    return (Join-Path (Get-CodeStoryHome) "bin")
}

function Get-VersionedInstallDir {
    param(
        [string]$InstallDirectory,
        [string]$ReleaseVersion
    )

    return (Join-Path (Join-Path $InstallDirectory "releases") $ReleaseVersion)
}

function Convert-VersionText {
    param([string]$Text)

    if ($Text -match "(\d+\.\d+\.\d+)") {
        return [Version]$matches[1]
    }
    throw "Unable to parse codestory-cli version from: $Text"
}

function Invoke-CliVersion {
    param([string]$Cli)

    $versionText = (& $Cli --version 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "codestory-cli --version failed for $Cli with exit code $LASTEXITCODE"
    }
    [PSCustomObject]@{
        Text = $versionText
        Version = Convert-VersionText $versionText
    }
}

function Test-RequiredVersion {
    param([Version]$Candidate)

    return $Candidate.CompareTo($script:RequiredVersion) -eq 0
}

function Resolve-CandidatePath {
    param([string]$Candidate)

    if (-not $Candidate) {
        return $null
    }
    if (Test-Path -LiteralPath $Candidate) {
        return (Resolve-Path -LiteralPath $Candidate).Path
    }
    return $null
}

function Get-CliCandidates {
    param(
        [string]$ExplicitCli,
        [string]$InstallDirectory
    )

    $candidates = @()
    if ($ExplicitCli) {
        $candidates += $ExplicitCli
    }
    if ($env:CODESTORY_CLI) {
        $candidates += $env:CODESTORY_CLI
    }
    if ($InstallDirectory) {
        $candidates += (Join-Path $InstallDirectory "codestory-cli.exe")
        $candidates += (Join-Path $InstallDirectory "codestory-cli")
        if ($script:RequiredVersion) {
            $versionedInstallDir = Get-VersionedInstallDir $InstallDirectory $script:RequiredVersion.ToString()
            $candidates += (Join-Path $versionedInstallDir "codestory-cli.exe")
            $candidates += (Join-Path $versionedInstallDir "codestory-cli")
        }
    }
    $candidates += @(
        (Join-Path (Get-Location) "target\release\codestory-cli.exe"),
        (Join-Path (Get-Location) "target\release\codestory-cli")
    )

    $seen = @{}
    foreach ($candidate in $candidates) {
        $resolved = Resolve-CandidatePath $candidate
        if (-not $resolved) {
            continue
        }
        $key = $resolved.ToLowerInvariant()
        if (-not $seen.ContainsKey($key)) {
            $seen[$key] = $true
            $resolved
        }
    }
}

function Find-ExistingCli {
    param(
        [string]$ExplicitCli,
        [string]$InstallDirectory
    )

    if ($ExplicitCli) {
        $resolved = Resolve-CandidatePath $ExplicitCli
        if (-not $resolved) {
            throw "Explicit codestory-cli path was not found or runnable: $ExplicitCli"
        }
        $version = Invoke-CliVersion $resolved
        if (-not (Test-RequiredVersion $version.Version)) {
            throw "Explicit codestory-cli is $($version.Version), but CodeStory readiness wrapper requires $script:RequiredVersion`: $resolved"
        }
        return [PSCustomObject]@{
            Path = $resolved
            VersionText = $version.Text
            Version = $version.Version
            Source = "existing"
        }
    }

    foreach ($candidate in (Get-CliCandidates $ExplicitCli $InstallDirectory)) {
        try {
            $version = Invoke-CliVersion $candidate
            if (Test-RequiredVersion $version.Version) {
                return [PSCustomObject]@{
                    Path = $candidate
                    VersionText = $version.Text
                    Version = $version.Version
                    Source = "existing"
                }
            }
        } catch {
            # Keep scanning implicit candidates; stale or broken binaries should not block install.
        }
    }
    return $null
}

function Get-WindowsReleaseTarget {
    param(
        [bool]$HostIsWindows = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
            [System.Runtime.InteropServices.OSPlatform]::Windows
        ),
        [System.Runtime.InteropServices.Architecture]$Architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    )

    if (-not $HostIsWindows) {
        return $null
    }
    if ($Architecture -eq [System.Runtime.InteropServices.Architecture]::X64) {
        return "windows-x64"
    }
    if ($Architecture -eq [System.Runtime.InteropServices.Architecture]::Arm64) {
        return "windows-arm64"
    }
    return $null
}

function Get-ExpectedHash {
    param(
        [string]$SumsPath,
        [string]$ArchiveName
    )

    foreach ($line in (Get-Content -LiteralPath $SumsPath)) {
        if ($line -match ("^([0-9a-fA-F]{64})\s+\*?" + [regex]::Escape($ArchiveName) + "$")) {
            return $matches[1].ToLowerInvariant()
        }
    }
    throw "SHA256SUMS.txt did not contain an entry for $ArchiveName"
}

function Remove-InstallerTemp {
    param([string]$Path)

    $resolved = Resolve-Path -LiteralPath $Path -ErrorAction SilentlyContinue
    if (-not $resolved) {
        return
    }
    $tempRoot = [System.IO.Path]::GetTempPath()
    $leaf = Split-Path -Leaf $resolved.Path
    if (-not $resolved.Path.StartsWith($tempRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove installer temp outside system temp: $($resolved.Path)"
    }
    if (-not $leaf.StartsWith("codestory-install-", [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove unexpected temp directory: $($resolved.Path)"
    }
    Remove-Item -LiteralPath $resolved.Path -Recurse -Force
}

function Copy-ReleaseCliBinary {
    param(
        [string]$BinaryPath,
        [string]$InstallDirectory,
        [string]$ReleaseVersion
    )

    $dest = Join-Path $InstallDirectory "codestory-cli.exe"
    try {
        Copy-Item -LiteralPath $BinaryPath -Destination $dest -Force
        return $dest
    } catch {
        $defaultError = $_.Exception.Message
    }

    $versionedInstallDir = Get-VersionedInstallDir $InstallDirectory $ReleaseVersion
    $versionedDest = Join-Path $versionedInstallDir "codestory-cli.exe"
    try {
        New-Item -ItemType Directory -Force -Path $versionedInstallDir | Out-Null
        Copy-Item -LiteralPath $BinaryPath -Destination $versionedDest -Force
    } catch {
        throw "Default install path is locked or not writable: $defaultError. Alternate install path also failed: $($_.Exception.Message). Stop the process holding $dest or restart the host, then run .\scripts\install-codestory.ps1 -Project . -Version $ReleaseVersion."
    }

    Write-Warning "Default install path is locked or not writable: $defaultError. Installed current release to $versionedDest. Set CODESTORY_CLI to that path for local development overrides, or stop the locking process and rerun .\scripts\install-codestory.ps1 -Project . -Version $ReleaseVersion."
    return $versionedDest
}

function Install-ReleaseCli {
    param(
        [string]$InstallDirectory,
        [string]$ReleaseVersion
    )

    $releaseTarget = Get-WindowsReleaseTarget
    if (-not $releaseTarget) {
        throw "Automatic download currently supports Windows x64 and Windows ARM64 only. Pass -CodestoryCli or set CODESTORY_CLI to reuse an existing $script:RequiredVersion binary."
    }

    $archiveName = "codestory-cli-v$ReleaseVersion-$releaseTarget.zip"
    $baseUrl = "https://github.com/TheGreenCedar/CodeStory/releases/download/v$ReleaseVersion"
    $archiveUrl = "$baseUrl/$archiveName"
    $sumsUrl = "$baseUrl/SHA256SUMS.txt"
    $tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("codestory-install-" + [System.Guid]::NewGuid().ToString("N"))
    $archivePath = Join-Path $tempDir $archiveName
    $sumsPath = Join-Path $tempDir "SHA256SUMS.txt"
    $extractDir = Join-Path $tempDir "extract"

    New-Item -ItemType Directory -Force -Path $tempDir, $extractDir, $InstallDirectory | Out-Null
    try {
        Invoke-WebRequest -Uri $sumsUrl -OutFile $sumsPath -UseBasicParsing
        Invoke-WebRequest -Uri $archiveUrl -OutFile $archivePath -UseBasicParsing
        $expectedHash = Get-ExpectedHash $sumsPath $archiveName
        $actualHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actualHash -ne $expectedHash) {
            throw "Downloaded archive checksum mismatch for $archiveName`: expected $expectedHash, got $actualHash"
        }
        Expand-Archive -LiteralPath $archivePath -DestinationPath $extractDir -Force
        $binary = Get-ChildItem -LiteralPath $extractDir -Recurse -Filter "codestory-cli.exe" |
            Select-Object -First 1
        if (-not $binary) {
            throw "Downloaded archive did not contain codestory-cli.exe"
        }
        $dest = Copy-ReleaseCliBinary $binary.FullName $InstallDirectory $ReleaseVersion
        $version = Invoke-CliVersion $dest
        if (-not (Test-RequiredVersion $version.Version)) {
            throw "Downloaded codestory-cli is $($version.Version), expected $script:RequiredVersion"
        }
        return [PSCustomObject]@{
            Path = (Resolve-Path -LiteralPath $dest).Path
            VersionText = $version.Text
            Version = $version.Version
            Source = "download"
        }
    } finally {
        Remove-InstallerTemp $tempDir
    }
}

function Get-ReadinessVerdict {
    param(
        $Doctor,
        [string]$Goal
    )

    @($Doctor.readiness) | Where-Object { $_.goal -eq $Goal } | Select-Object -First 1
}

function Test-VerdictReady {
    param($Verdict)

    return ($Verdict -and $Verdict.status -eq "ready")
}

function Add-Commands {
    param(
        [System.Collections.Generic.List[string]]$Commands,
        $Values
    )

    foreach ($value in @($Values)) {
        if ($value -and -not $Commands.Contains([string]$value)) {
            $Commands.Add([string]$value)
        }
    }
}

function Convert-DoctorToReadinessState {
    param(
        $Doctor,
        [string]$CliPath,
        [string]$VersionText,
        [string]$Source
    )

    $local = Get-ReadinessVerdict $Doctor "local_navigation"
    $agent = Get-ReadinessVerdict $Doctor "agent_packet_search"
    $commands = [System.Collections.Generic.List[string]]::new()
    foreach ($verdict in @($local, $agent)) {
        if (-not (Test-VerdictReady $verdict)) {
            Add-Commands $commands $verdict.minimum_next
            Add-Commands $commands $verdict.full_repair
        }
    }
    Add-Commands $commands $Doctor.next_commands

    [PSCustomObject]@{
        Binary = [PSCustomObject]@{
            ready = $true
            path = $CliPath
            version = $VersionText
            source = $Source
        }
        LocalNavigation = [PSCustomObject]@{
            ready = Test-VerdictReady $local
            status = if ($local) { $local.status } else { "unknown" }
            summary = if ($local) { $local.summary } else { "doctor did not return a local_navigation readiness verdict" }
        }
        AgentPacketSearch = [PSCustomObject]@{
            ready = Test-VerdictReady $agent
            status = if ($agent) { $agent.status } else { "unknown" }
            summary = if ($agent) { $agent.summary } else { "doctor did not return an agent_packet_search readiness verdict" }
            retrieval_mode = $Doctor.retrieval_mode
            degraded_reason = $Doctor.degraded_reason
        }
        RepairCommands = @($commands)
    }
}

function Write-StateLine {
    param(
        [string]$Name,
        [bool]$Ready,
        [string]$Status,
        [string]$Summary
    )

    $label = if ($Ready) { "ready" } else { $Status }
    Write-Host ("  {0}: {1} - {2}" -f $Name, $label, $Summary)
}

function Write-ReadinessState {
    param($State)

    Write-Host "CodeStory readiness"
    Write-Host ("  binary installed: ready - {0} ({1}, {2})" -f $State.Binary.path, $State.Binary.version, $State.Binary.source)
    Write-StateLine "local navigation" $State.LocalNavigation.ready $State.LocalNavigation.status $State.LocalNavigation.summary
    Write-StateLine "agent packet/search" $State.AgentPacketSearch.ready $State.AgentPacketSearch.status $State.AgentPacketSearch.summary
    if (-not $State.AgentPacketSearch.ready) {
        Write-Host ("  retrieval_mode: {0}" -f $State.AgentPacketSearch.retrieval_mode)
        if ($State.AgentPacketSearch.degraded_reason) {
            Write-Host ("  degraded_reason: {0}" -f $State.AgentPacketSearch.degraded_reason)
        }
    }
    if ($State.RepairCommands.Count -gt 0) {
        Write-Host ""
        Write-Host "Repair commands:"
        foreach ($command in $State.RepairCommands) {
            Write-Host "  $command"
        }
    }
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

function Assert-SelfTest {
    param(
        [bool]$Condition,
        [string]$Message
    )

    if (-not $Condition) {
        throw "Self-test failed: $Message"
    }
}

function Invoke-SelfTest {
    Set-RequiredVersion "v0.11.4"
    Assert-SelfTest ((Get-WindowsReleaseTarget $true ([System.Runtime.InteropServices.Architecture]::X64)) -eq "windows-x64") "Windows x64 should map to windows-x64 release assets"
    Assert-SelfTest ((Get-WindowsReleaseTarget $true ([System.Runtime.InteropServices.Architecture]::Arm64)) -eq "windows-arm64") "Windows ARM64 should map to windows-arm64 release assets"
    Assert-SelfTest (-not (Get-WindowsReleaseTarget $false ([System.Runtime.InteropServices.Architecture]::X64))) "non-Windows hosts should not auto-download Windows assets"
    Assert-SelfTest ((Convert-ReleaseTagToVersion "v0.11.4") -eq "0.11.4") "release tag parser should strip v prefix"
    $parsedVersion = Convert-VersionText "codestory-cli 0.11.4"
    Assert-SelfTest (Test-RequiredVersion $parsedVersion) "version gate should accept current release"
    Assert-SelfTest (-not (Test-RequiredVersion ([Version]"0.11.3"))) "version gate should reject stale 0.11.3"

    $lockRoot = $null
    $lockStream = $null
    try {
        $lockRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("codestory-install-" + [System.Guid]::NewGuid().ToString("N"))
        $lockInstallDir = Join-Path $lockRoot "bin"
        New-Item -ItemType Directory -Force -Path $lockInstallDir | Out-Null
        $sourceCli = Join-Path $lockRoot "source.exe"
        $lockedDefault = Join-Path $lockInstallDir "codestory-cli.exe"
        Set-Content -LiteralPath $sourceCli -Value "current" -Encoding ASCII
        Set-Content -LiteralPath $lockedDefault -Value "stale" -Encoding ASCII
        $lockStream = [System.IO.File]::Open($lockedDefault, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
        $fallback = Copy-ReleaseCliBinary $sourceCli $lockInstallDir "0.11.4"
        $expectedFallback = Join-Path (Get-VersionedInstallDir $lockInstallDir "0.11.4") "codestory-cli.exe"
        Assert-SelfTest ($fallback -ieq $expectedFallback) "locked default install should fall back to versioned release path"
    } finally {
        if ($lockStream) {
            $lockStream.Dispose()
        }
        if ($lockRoot) {
            Remove-InstallerTemp $lockRoot
        }
    }

    $explicitStale = $false
    try {
        $staleCli = Join-Path ([System.IO.Path]::GetTempPath()) ("codestory-stale-" + [System.Guid]::NewGuid().ToString("N") + ".cmd")
        Set-Content -LiteralPath $staleCli -Value "@echo codestory-cli 0.11.3" -Encoding ASCII
        Find-ExistingCli $staleCli $null | Out-Null
    } catch {
        $explicitStale = $_.Exception.Message -match "requires 0.11.4"
    } finally {
        if ($staleCli -and (Test-Path -LiteralPath $staleCli)) {
            Remove-Item -LiteralPath $staleCli -Force
        }
    }
    Assert-SelfTest $explicitStale "explicit stale codestory-cli override should fail loudly"

    Set-RequiredVersion "v0.11.9"
    $staleLatestRepair = $false
    try {
        $staleCli = Join-Path ([System.IO.Path]::GetTempPath()) ("codestory-stale-" + [System.Guid]::NewGuid().ToString("N") + ".cmd")
        Set-Content -LiteralPath $staleCli -Value "@echo codestory-cli 0.11.6" -Encoding ASCII
        Find-ExistingCli $staleCli $null | Out-Null
    } catch {
        $staleLatestRepair = $_.Exception.Message -match "requires 0.11.9"
    } finally {
        if ($staleCli -and (Test-Path -LiteralPath $staleCli)) {
            Remove-Item -LiteralPath $staleCli -Force
        }
    }
    Assert-SelfTest $staleLatestRepair "stale 0.11.6 should require latest 0.11.9 repair"
    Set-RequiredVersion "v0.11.4"

    try {
        $currentCli = Join-Path ([System.IO.Path]::GetTempPath()) ("codestory-current-" + [System.Guid]::NewGuid().ToString("N") + ".cmd")
        Set-Content -LiteralPath $currentCli -Value "@echo codestory-cli 0.11.4" -Encoding ASCII
        $current = Find-ExistingCli $currentCli $null
        Assert-SelfTest ($current.Version -eq [Version]"0.11.4") "explicit current codestory-cli override should be accepted"
    } finally {
        if ($currentCli -and (Test-Path -LiteralPath $currentCli)) {
            Remove-Item -LiteralPath $currentCli -Force
        }
    }

    $staleDoctor = @'
{
  "retrieval_mode": "unavailable",
  "degraded_reason": "retrieval_manifest_missing",
  "readiness": [
    {
      "goal": "local_navigation",
      "status": "repair_index",
      "summary": "The index has changed, new, or removed files.",
      "minimum_next": ["codestory-cli index --project C:/repo --refresh incremental"],
      "full_repair": ["codestory-cli index --project C:/repo --refresh incremental", "codestory-cli doctor --project C:/repo"]
    },
    {
      "goal": "agent_packet_search",
      "status": "repair_index",
      "summary": "The index has changed, new, or removed files.",
      "minimum_next": ["codestory-cli index --project C:/repo --refresh incremental"],
      "full_repair": ["codestory-cli index --project C:/repo --refresh incremental", "codestory-cli doctor --project C:/repo"]
    }
  ],
  "next_commands": []
}
'@ | ConvertFrom-Json
    $stale = Convert-DoctorToReadinessState $staleDoctor "C:/tools/codestory-cli.exe" "codestory-cli 0.11.1" "existing"
    Assert-SelfTest (-not $stale.LocalNavigation.ready) "stale index must not report local navigation ready"
    Assert-SelfTest (-not $stale.AgentPacketSearch.ready) "stale index must not report agent packet/search ready"
    Assert-SelfTest ($stale.RepairCommands -contains "codestory-cli index --project C:/repo --refresh incremental") "stale index repair command missing"

    $missingRetrievalDoctor = @'
{
  "retrieval_mode": "unavailable",
  "degraded_reason": "retrieval_manifest_missing",
  "readiness": [
    {
      "goal": "local_navigation",
      "status": "ready",
      "summary": "Local navigation can use the current index.",
      "minimum_next": ["codestory-cli ground --project C:/repo"],
      "full_repair": ["codestory-cli ground --project C:/repo"]
    },
    {
      "goal": "agent_packet_search",
      "status": "repair_retrieval",
      "summary": "Agent packet/search needs full retrieval; current mode is `unavailable`.",
      "minimum_next": ["codestory-cli retrieval index --project C:/repo --refresh full --format json"],
      "full_repair": ["codestory-cli retrieval index --project C:/repo --refresh full --format json", "codestory-cli retrieval status --project C:/repo --format json", "codestory-cli doctor --project C:/repo --format markdown"]
    }
  ],
  "next_commands": []
}
'@ | ConvertFrom-Json
    $retrieval = Convert-DoctorToReadinessState $missingRetrievalDoctor "C:/tools/codestory-cli.exe" "codestory-cli 0.11.1" "existing"
    Assert-SelfTest $retrieval.LocalNavigation.ready "fresh local index should report local navigation ready"
    Assert-SelfTest (-not $retrieval.AgentPacketSearch.ready) "missing full retrieval must not report agent packet/search ready"
    Assert-SelfTest ($retrieval.RepairCommands -contains "codestory-cli retrieval status --project C:/repo --format json") "retrieval status command missing"
    Assert-SelfTest (-not (($retrieval.RepairCommands -join "`n") -match "codestory-cli (packet|search)")) "repair commands must not attempt broad packet/search fallback"

    Write-Host "install-codestory self-test: ok"
}

if ($SelfTest) {
    Invoke-SelfTest
    return
}

if (-not $InstallDir) {
    $InstallDir = Get-DefaultInstallDir
}

if (-not $Version) {
    $Version = Get-LatestReleaseVersion
} else {
    $Version = Convert-ReleaseTagToVersion $Version
}
Set-RequiredVersion $Version

$cliInfo = Find-ExistingCli $CodestoryCli $InstallDir
if (-not $cliInfo) {
    if ($NoDownload) {
        throw "No existing codestory-cli $script:RequiredVersion found. Pass -CodestoryCli, set CODESTORY_CLI, use the install directory, or rerun without -NoDownload on Windows x64 or Windows ARM64."
    }
    $cliInfo = Install-ReleaseCli $InstallDir $Version
}

$projectPath = (Resolve-Path -LiteralPath $Project).Path
$doctor = Invoke-DoctorJson $cliInfo.Path $projectPath
$state = Convert-DoctorToReadinessState $doctor $cliInfo.Path $cliInfo.VersionText $cliInfo.Source
Write-ReadinessState $state
Write-Host ""
Write-Host "CODESTORY_CLI=$($cliInfo.Path)"
