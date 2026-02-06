param(
  # Prefer passing -WindowHandle from a windows-mcp Snapshot for reliable targeting.
  [int]$WindowHandle,

  [string]$Title = "CodeStory",

  [int]$Width = 1280,

  [int]$Height = 720,

  [string]$OutDir = "artifacts/graph_parity/run"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\\..")).Path
Set-Location $repoRoot

$outDirFull = (Resolve-Path -LiteralPath $OutDir -ErrorAction SilentlyContinue)
if (-not $outDirFull) {
  New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
  $outDirFull = (Resolve-Path -LiteralPath $OutDir).Path
} else {
  $outDirFull = $outDirFull.Path
}

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win32Move {
  [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr hWnd, int X, int Y, int nWidth, int nHeight, bool bRepaint);
}
"@ | Out-Null

function Require-File([string]$path) {
  if (-not (Test-Path -LiteralPath $path)) { throw "Missing file: $path" }
}

Require-File ".\\scripts\\graph_parity\\capture_window.ps1"
Require-File ".\\scripts\\graph_parity\\crop_image.ps1"
Require-File ".\\scripts\\graph_parity\\diff_images.ps1"

# Move/resize window to a consistent location for stable crops.
if ($WindowHandle -and $WindowHandle -ne 0) {
  [Win32Move]::MoveWindow([IntPtr]$WindowHandle, 100, 100, $Width, $Height, $true) | Out-Null
}

Start-Sleep -Milliseconds 250

$stamp = Get-Date -Format "yyyyMMdd_HHmmss"
$candidateFull = Join-Path $outDirFull "codestory_graph_${stamp}.png"

if ($WindowHandle -and $WindowHandle -ne 0) {
  .\\scripts\\graph_parity\\capture_window.ps1 -WindowHandle $WindowHandle -OutPath $candidateFull | Out-Null
} else {
  .\\scripts\\graph_parity\\capture_window.ps1 -Title $Title -OutPath $candidateFull | Out-Null
}

Start-Sleep -Milliseconds 200

# Reference crops are already checked into `artifacts/graph_parity/`.
$refControls = "artifacts/graph_parity/ref_graph_view_controls_crop.png"
$refDepth = "artifacts/graph_parity/ref_graph_view_depth_crop.png"
$refGrouping = "artifacts/graph_parity/ref_graph_view_grouping_crop.png"

Require-File $refControls
Require-File $refDepth
Require-File $refGrouping

# Crop coordinates assume CodeStory window is 1280x720 and the graph fills the center pane.
# These are intended to be tuned via the delta report.
$candControls = Join-Path $outDirFull "candidate_controls_${stamp}.png"
$candDepth = Join-Path $outDirFull "candidate_depth_${stamp}.png"
$candGrouping = Join-Path $outDirFull "candidate_grouping_${stamp}.png"

# Top-left controls crop: match size of `ref_graph_view_controls_crop.png` (220x520).
.\\scripts\\graph_parity\\crop_image.ps1 -InPath $candidateFull -X 0 -Y 100 -Width 220 -Height 520 -OutPath $candControls | Out-Null

# Depth slider crop: match size of `ref_graph_view_depth_crop.png` (160x380).
.\\scripts\\graph_parity\\crop_image.ps1 -InPath $candidateFull -X 0 -Y 240 -Width 160 -Height 380 -OutPath $candDepth | Out-Null

# Grouping pill crop: match size of `ref_graph_view_grouping_crop.png` (160x120).
.\\scripts\\graph_parity\\crop_image.ps1 -InPath $candidateFull -X 60 -Y 100 -Width 160 -Height 120 -OutPath $candGrouping | Out-Null

$diffControls = Join-Path $outDirFull "diff_controls_${stamp}.png"
$diffDepth = Join-Path $outDirFull "diff_depth_${stamp}.png"
$diffGrouping = Join-Path $outDirFull "diff_grouping_${stamp}.png"

$m1 = .\\scripts\\graph_parity\\diff_images.ps1 -ReferencePath $refControls -CandidatePath $candControls -OutPath $diffControls
$m2 = .\\scripts\\graph_parity\\diff_images.ps1 -ReferencePath $refDepth -CandidatePath $candDepth -OutPath $diffDepth
$m3 = .\\scripts\\graph_parity\\diff_images.ps1 -ReferencePath $refGrouping -CandidatePath $candGrouping -OutPath $diffGrouping

$reportPath = Join-Path $outDirFull "delta_report_${stamp}.txt"
@"
Graph Parity Capture

Date: $(Get-Date -Format "yyyy-MM-dd HH:mm:ss")
WindowHandle: $WindowHandle
WindowSize: ${Width}x${Height}

Candidate:
- $candidateFull

Crops:
- controls: $candControls
- depth: $candDepth
- grouping: $candGrouping

Diffs:
- controls: $diffControls
  metrics: $m1
- depth: $diffDepth
  metrics: $m2
- grouping: $diffGrouping
  metrics: $m3
"@ | Set-Content -Encoding UTF8 -Path $reportPath

Write-Output $reportPath

