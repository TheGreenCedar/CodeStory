param(
    [string]$OutputDir = "models/all-minilm-l6-v2",
    [switch]$Force
)

$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Resolve-Path (Join-Path $scriptDir "..")

if ([System.IO.Path]::IsPathRooted($OutputDir)) {
    $modelDir = [System.IO.Path]::GetFullPath($OutputDir)
} else {
    $modelDir = [System.IO.Path]::GetFullPath((Join-Path $repoRoot $OutputDir))
}

New-Item -ItemType Directory -Path $modelDir -Force | Out-Null

$modelPath = Join-Path $modelDir "model.onnx"
$tokenizerPath = Join-Path $modelDir "tokenizer.json"

$modelUri = "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx?download=true"
$tokenizerUri = "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main/tokenizer.json?download=true"

function Download-Artifact {
    param(
        [Parameter(Mandatory = $true)][string]$Uri,
        [Parameter(Mandatory = $true)][string]$Destination
    )

    if ((Test-Path $Destination) -and -not $Force) {
        Write-Host "Using existing artifact: $Destination"
        return
    }

    Write-Host "Downloading $Uri"
    Invoke-WebRequest -Uri $Uri -OutFile $Destination
}

Download-Artifact -Uri $modelUri -Destination $modelPath
Download-Artifact -Uri $tokenizerUri -Destination $tokenizerPath

if ((Get-Item $modelPath).Length -lt 1024) {
    throw "Downloaded model at $modelPath is unexpectedly small."
}

if ((Get-Item $tokenizerPath).Length -lt 256) {
    throw "Downloaded tokenizer at $tokenizerPath is unexpectedly small."
}

Write-Host ""
Write-Host "Embedding model bundled successfully."
Write-Host "Model path: $modelPath"
Write-Host "Tokenizer path: $tokenizerPath"
Write-Host ""
Write-Host "When running codestory-server from the repository root, no extra env vars are needed."
Write-Host "If running from another working directory, set:"
Write-Host "  `$env:CODESTORY_EMBED_MODEL_PATH = `"$modelPath`""
