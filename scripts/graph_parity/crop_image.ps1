param(
  [Parameter(Mandatory = $true)]
  [string]$InPath,

  [Parameter(Mandatory = $true)]
  [int]$X,

  [Parameter(Mandatory = $true)]
  [int]$Y,

  [Parameter(Mandatory = $true)]
  [int]$Width,

  [Parameter(Mandatory = $true)]
  [int]$Height,

  [Parameter(Mandatory = $true)]
  [string]$OutPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Drawing | Out-Null

$inFull = (Resolve-Path $InPath).Path

$bmp = [System.Drawing.Bitmap]::new($inFull)
try {
  if ($Width -le 0 -or $Height -le 0) { throw "Invalid crop size: ${Width}x${Height}" }
  if ($X -lt 0 -or $Y -lt 0) { throw "Invalid crop origin: ($X,$Y)" }
  if ($X + $Width -gt $bmp.Width -or $Y + $Height -gt $bmp.Height) {
    throw "Crop rectangle out of bounds. image=$($bmp.Width)x$($bmp.Height) crop=($X,$Y,$Width,$Height)"
  }

  $rect = [System.Drawing.Rectangle]::new($X, $Y, $Width, $Height)
  $cropped = $bmp.Clone($rect, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  try {
    $dir = Split-Path -Parent $OutPath
    if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
    $cropped.Save($OutPath, [System.Drawing.Imaging.ImageFormat]::Png)
  }
  finally {
    $cropped.Dispose()
  }
}
finally {
  $bmp.Dispose()
}

[PSCustomObject]@{
  in_path = $InPath
  out_path = $OutPath
  x = $X
  y = $Y
  width = $Width
  height = $Height
}

