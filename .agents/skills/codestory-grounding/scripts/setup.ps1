[CmdletBinding()]
param(
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

function Get-CodeStoryHome {
    if ($env:CODESTORY_HOME) {
        return $env:CODESTORY_HOME
    }
    if ($env:LOCALAPPDATA) {
        return (Join-Path $env:LOCALAPPDATA "CodeStory")
    }
    return (Join-Path $HOME ".codestory")
}

function Require-Command {
    param([string]$Name)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Required command '$Name' was not found on PATH."
    }
}

function Invoke-Checked {
    param(
        [string]$FilePath,
        [string[]]$Arguments
    )

    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code ${LASTEXITCODE}: $FilePath $($Arguments -join ' ')"
    }
}

$skillRoot = Split-Path -Parent $PSScriptRoot
$repoRefPath = Join-Path $skillRoot "CODESTORY_REF"
$repoUrl = if ($env:CODESTORY_REPO_URL) {
    $env:CODESTORY_REPO_URL
} else {
    "https://github.com/TheGreenCedar/CodeStory.git"
}
$repoRef = if ($env:CODESTORY_REPO_REF) {
    $env:CODESTORY_REPO_REF
} elseif (Test-Path -LiteralPath $repoRefPath) {
    (Get-Content -LiteralPath $repoRefPath -Raw).Trim()
} else {
    throw "CODESTORY_REPO_REF is not set and CODESTORY_REF is missing from the skill."
}
if (-not $repoRef) {
    throw "CODESTORY_REPO_REF resolved to an empty value."
}

$isWindows = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)
$binaryName = if ($isWindows) { "codestory-cli.exe" } else { "codestory-cli" }
$codestoryHome = Get-CodeStoryHome
$sourceDir = Join-Path $codestoryHome "src"
$binDir = Join-Path $codestoryHome "bin"
$dest = Join-Path $binDir $binaryName

Write-Host "CodeStory setup"
Write-Host "  home: $codestoryHome"
Write-Host "  source: $sourceDir"
Write-Host "  binary: $dest"
Write-Host "  repo: $repoUrl"
Write-Host "  ref: $repoRef"

if ($DryRun) {
    Write-Host "Dry run only; no clone, build, or copy performed."
    Write-Host "CODESTORY_CLI=$dest"
    exit 0
}

Require-Command git
Require-Command cargo

New-Item -ItemType Directory -Force -Path $codestoryHome, $binDir | Out-Null

if (-not (Test-Path -LiteralPath (Join-Path $sourceDir ".git"))) {
    if (Test-Path -LiteralPath $sourceDir) {
        $hasContents = Get-ChildItem -LiteralPath $sourceDir -Force | Select-Object -First 1
        if ($hasContents) {
            throw "Source directory exists but is not a git checkout: $sourceDir"
        }
    }
    Invoke-Checked git @("clone", $repoUrl, $sourceDir)
} else {
    $originUrl = & git -C $sourceDir config --get remote.origin.url
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to read CodeStory source artifact remote: $sourceDir"
    }
    if ($originUrl.TrimEnd("/") -ne $repoUrl.TrimEnd("/")) {
        throw "CodeStory source artifact remote is '$originUrl', expected '$repoUrl'. Set CODESTORY_HOME or CODESTORY_REPO_URL intentionally."
    }

    $dirty = & git -C $sourceDir status --porcelain
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to inspect CodeStory source artifact status: $sourceDir"
    }
    if ($dirty) {
        throw "CodeStory source artifact has local changes; refusing to update: $sourceDir"
    }
}

Invoke-Checked git @("-C", $sourceDir, "fetch", "--tags", "origin")
Invoke-Checked git @("-C", $sourceDir, "checkout", "--detach", $repoRef)

Invoke-Checked cargo @("build", "--release", "-p", "codestory-cli", "--manifest-path", (Join-Path $sourceDir "Cargo.toml"))

$built = Join-Path (Join-Path (Join-Path $sourceDir "target") "release") $binaryName
if (-not (Test-Path -LiteralPath $built)) {
    throw "Build completed but expected binary was not found: $built"
}

Copy-Item -LiteralPath $built -Destination $dest -Force
& $dest --help | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Built CodeStory CLI did not run successfully: $dest"
}

Write-Host "CODESTORY_CLI=$dest"
