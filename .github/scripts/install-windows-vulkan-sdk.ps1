[CmdletBinding()]
param(
    [switch]$SelfTest
)

$ErrorActionPreference = "Stop"
$version = "1.4.350.0"
$packages = @{
    X64 = @{
        Platform = "windows"
        File = "vulkansdk-windows-X64-1.4.350.0.exe"
        Sha256 = "855b27ba05d2d8119c5114c5d4ff870ca38f2c632b11e1bb9923b9b7e6ecfe7b"
    }
    ARM64 = @{
        Platform = "warm"
        File = "vulkansdk-windows-ARM64-1.4.350.0.exe"
        Sha256 = "f1f1dda1e5f5c7dae9e1b03ac53b3860a27aaad214c22bb3fa59f22ea116834b"
    }
}

if ($SelfTest) {
    foreach ($architecture in @("X64", "ARM64")) {
        $package = $packages[$architecture]
        if ($package.Sha256 -notmatch "^[0-9a-f]{64}$") {
            throw "Invalid SHA-256 for $architecture."
        }
        if ($package.File -notlike "*$version.exe") {
            throw "Installer filename for $architecture does not match SDK $version."
        }
    }
    Write-Output "Windows Vulkan SDK installer contract is valid for SDK $version."
    exit 0
}

$runnerArchitecture = $env:RUNNER_ARCH
if (-not $runnerArchitecture) {
    throw "RUNNER_ARCH is required."
}
$architecture = $runnerArchitecture.ToUpperInvariant()
if (-not $packages.ContainsKey($architecture)) {
    throw "Unsupported Windows runner architecture '$($env:RUNNER_ARCH)'."
}
if (-not $env:RUNNER_TEMP) {
    throw "RUNNER_TEMP is required."
}
if (-not $env:GITHUB_ENV -or -not $env:GITHUB_PATH) {
    throw "GITHUB_ENV and GITHUB_PATH are required."
}

$package = $packages[$architecture]
$downloadUrl = "https://sdk.lunarg.com/sdk/download/$version/$($package.Platform)/$($package.File)"
$installer = Join-Path $env:RUNNER_TEMP $package.File
$sdkRoot = Join-Path $env:RUNNER_TEMP "VulkanSDK-$version-$architecture"

try {
    Invoke-WebRequest -Uri $downloadUrl -OutFile $installer
    $actualSha256 = (Get-FileHash -Path $installer -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualSha256 -ne $package.Sha256) {
        throw "Vulkan SDK checksum mismatch for ${architecture}: expected $($package.Sha256), got $actualSha256."
    }

    $process = Start-Process -FilePath $installer -ArgumentList @(
        "--root", $sdkRoot,
        "--accept-licenses",
        "--default-answer",
        "--confirm-command",
        "install",
        "copy_only=1"
    ) -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        throw "Vulkan SDK installer exited with code $($process.ExitCode)."
    }

    $header = Join-Path $sdkRoot "Include\vulkan\vulkan.h"
    $library = Join-Path $sdkRoot "Lib\vulkan-1.lib"
    if (-not (Test-Path -LiteralPath $header) -or -not (Test-Path -LiteralPath $library)) {
        throw "Vulkan SDK $version did not install the required headers and import library for $architecture."
    }

    Add-Content -LiteralPath $env:GITHUB_ENV -Value "VULKAN_SDK=$sdkRoot"
    Add-Content -LiteralPath $env:GITHUB_PATH -Value (Join-Path $sdkRoot "Bin")
    Write-Output "Installed checksum-pinned Vulkan SDK $version for $architecture at $sdkRoot."
}
finally {
    Remove-Item -LiteralPath $installer -Force -ErrorAction SilentlyContinue
}
