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

function Protect-UrlUserInfo {
    param([string]$Url)
    if (-not $Url) {
        return $Url
    }
    return ($Url -replace "^(https?://)([^/@\s]+)@", '$1***@')
}

function Invoke-Checked {
    param(
        [string]$FilePath,
        [string[]]$Arguments
    )

    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        $displayArgs = $Arguments | ForEach-Object { Protect-UrlUserInfo $_ }
        throw "Command failed with exit code ${LASTEXITCODE}: $FilePath $($displayArgs -join ' ')"
    }
}

function Get-LocalCheckoutRoot {
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        return $null
    }
    $candidate = Resolve-Path (Join-Path $PSScriptRoot "..\..\..\..") -ErrorAction SilentlyContinue
    if (-not $candidate) {
        return $null
    }
    if (-not (Test-Path -LiteralPath (Join-Path $candidate.Path ".git"))) {
        return $null
    }
    return $candidate.Path
}

function Get-GitHeadRef {
    param([string]$Path)
    if (-not $Path) {
        return $null
    }
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        return $null
    }
    $head = & git -C $Path rev-parse HEAD 2>$null
    if ($LASTEXITCODE -eq 0 -and $head) {
        return $head.Trim()
    }
    return $null
}

$localCheckoutRoot = Get-LocalCheckoutRoot
$useLocalCheckout = $localCheckoutRoot -and -not $env:CODESTORY_REPO_URL -and -not $env:CODESTORY_REPO_REF
$localCheckoutHeadRef = if ($useLocalCheckout) { Get-GitHeadRef $localCheckoutRoot } else { $null }
$repoUrl = if ($env:CODESTORY_REPO_URL) {
    $env:CODESTORY_REPO_URL
} else {
    "https://github.com/TheGreenCedar/CodeStory.git"
}
$repoRef = if ($env:CODESTORY_REPO_REF) {
    $env:CODESTORY_REPO_REF
} elseif ($useLocalCheckout -and $localCheckoutHeadRef) {
    "working-tree:$localCheckoutHeadRef"
} else {
    $null
}
$repoRefForDisplay = if ($repoRef) { $repoRef } else { "remote default branch" }

$runningOnWindows = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)
$binaryName = if ($runningOnWindows) { "codestory-cli.exe" } else { "codestory-cli" }
$codestoryHome = Get-CodeStoryHome
$sourceDir = if ($useLocalCheckout) { $localCheckoutRoot } else { Join-Path $codestoryHome "src" }
$binDir = Join-Path $codestoryHome "bin"
$dest = Join-Path $binDir $binaryName
$repoUrlForDisplay = Protect-UrlUserInfo $repoUrl

Write-Host "CodeStory setup"
Write-Host "  home: $codestoryHome"
Write-Host "  source: $sourceDir"
Write-Host "  binary: $dest"
Write-Host "  repo: $repoUrlForDisplay"
Write-Host "  ref: $repoRefForDisplay"

if ($DryRun) {
    Write-Host "Dry run only; no clone, build, or copy performed."
    Write-Host "CODESTORY_CLI=$dest"
    exit 0
}

Require-Command git
Require-Command cargo

New-Item -ItemType Directory -Force -Path $codestoryHome, $binDir | Out-Null

if (-not $useLocalCheckout) {
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
            $originForDisplay = Protect-UrlUserInfo $originUrl
            throw "CodeStory source artifact remote is '$originForDisplay', expected '$repoUrlForDisplay'. Set CODESTORY_HOME or CODESTORY_REPO_URL intentionally."
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
    if ($repoRef) {
        Invoke-Checked git @("-C", $sourceDir, "checkout", "--detach", $repoRef)
    } else {
        $originHead = & git -C $sourceDir rev-parse --verify --quiet origin/HEAD 2>$null
        if ($LASTEXITCODE -ne 0 -or -not $originHead) {
            Invoke-Checked git @("-C", $sourceDir, "remote", "set-head", "origin", "--auto")
        }
        Invoke-Checked git @("-C", $sourceDir, "checkout", "--detach", "origin/HEAD")
    }
}

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
