param(
    [Parameter(Mandatory = $true)]
    [string]$ReferencePath,

    [Parameter(Mandatory = $true)]
    [string]$CandidatePath,

    [Parameter(Mandatory = $true)]
    [string]$OutPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Drawing | Out-Null

function Convert-To32bppArgb([System.Drawing.Bitmap]$bmp) {
    if ($bmp.PixelFormat -eq [System.Drawing.Imaging.PixelFormat]::Format32bppArgb) {
        return $bmp
    }

    $converted = New-Object System.Drawing.Bitmap(
        $bmp.Width,
        $bmp.Height,
        [System.Drawing.Imaging.PixelFormat]::Format32bppArgb
    )
    $g = [System.Drawing.Graphics]::FromImage($converted)
    $g.DrawImage($bmp, 0, 0, $bmp.Width, $bmp.Height) | Out-Null
    $g.Dispose()
    $bmp.Dispose()
    return $converted
}

$refBmp = [System.Drawing.Bitmap]::new($ReferencePath)
$candBmp = [System.Drawing.Bitmap]::new($CandidatePath)

try {
    $refBmp = Convert-To32bppArgb $refBmp
    $candBmp = Convert-To32bppArgb $candBmp

    if ($refBmp.Width -ne $candBmp.Width -or $refBmp.Height -ne $candBmp.Height) {
        throw "Image size mismatch: reference=$($refBmp.Width)x$($refBmp.Height), candidate=$($candBmp.Width)x$($candBmp.Height)"
    }

    $width = $refBmp.Width
    $height = $refBmp.Height
    $totalPixels = [long]$width * [long]$height

    $rect = [System.Drawing.Rectangle]::new(0, 0, $width, $height)
    $refData = $refBmp.LockBits(
        $rect,
        [System.Drawing.Imaging.ImageLockMode]::ReadOnly,
        [System.Drawing.Imaging.PixelFormat]::Format32bppArgb
    )
    $candData = $candBmp.LockBits(
        $rect,
        [System.Drawing.Imaging.ImageLockMode]::ReadOnly,
        [System.Drawing.Imaging.PixelFormat]::Format32bppArgb
    )

    $refBytes = New-Object byte[] ($refData.Stride * $refData.Height)
    $candBytes = New-Object byte[] ($candData.Stride * $candData.Height)
    [System.Runtime.InteropServices.Marshal]::Copy($refData.Scan0, $refBytes, 0, $refBytes.Length)
    [System.Runtime.InteropServices.Marshal]::Copy($candData.Scan0, $candBytes, 0, $candBytes.Length)

    $diffBmp = New-Object System.Drawing.Bitmap(
        $width,
        $height,
        [System.Drawing.Imaging.PixelFormat]::Format24bppRgb
    )
    $diffData = $diffBmp.LockBits(
        $rect,
        [System.Drawing.Imaging.ImageLockMode]::WriteOnly,
        [System.Drawing.Imaging.PixelFormat]::Format24bppRgb
    )
    $diffBytes = New-Object byte[] ($diffData.Stride * $diffData.Height)

    $diffCount = 0L
    $sumAbs = 0L
    $maxHeat = 0

    for ($y = 0; $y -lt $height; $y++) {
        $refRow = $y * $refData.Stride
        $candRow = $y * $candData.Stride
        $diffRow = $y * $diffData.Stride
        for ($x = 0; $x -lt $width; $x++) {
            $i32 = $refRow + ($x * 4)
            $j32 = $candRow + ($x * 4)

            # Format32bppArgb is BGRA in memory.
            $b1 = $refBytes[$i32 + 0]
            $g1 = $refBytes[$i32 + 1]
            $r1 = $refBytes[$i32 + 2]

            $b2 = $candBytes[$j32 + 0]
            $g2 = $candBytes[$j32 + 1]
            $r2 = $candBytes[$j32 + 2]

            $dr = [Math]::Abs([int]$r1 - [int]$r2)
            $dg = [Math]::Abs([int]$g1 - [int]$g2)
            $db = [Math]::Abs([int]$b1 - [int]$b2)

            $absSum = $dr + $dg + $db
            if ($absSum -ne 0) {
                $diffCount++
            }
            $sumAbs += $absSum

            $heat = $dr
            if ($dg -gt $heat) { $heat = $dg }
            if ($db -gt $heat) { $heat = $db }
            if ($heat -gt $maxHeat) { $maxHeat = $heat }

            # Format24bppRgb is BGR in memory. Use red heatmap.
            $k24 = $diffRow + ($x * 3)
            $diffBytes[$k24 + 0] = 0
            $diffBytes[$k24 + 1] = 0
            $diffBytes[$k24 + 2] = [byte]$heat
        }
    }

    [System.Runtime.InteropServices.Marshal]::Copy($diffBytes, 0, $diffData.Scan0, $diffBytes.Length)

    $diffBmp.UnlockBits($diffData)
    $refBmp.UnlockBits($refData)
    $candBmp.UnlockBits($candData)

    $outDir = Split-Path -Parent $OutPath
    if ($outDir -and -not (Test-Path $outDir)) {
        New-Item -ItemType Directory -Force -Path $outDir | Out-Null
    }

    $diffBmp.Save($OutPath, [System.Drawing.Imaging.ImageFormat]::Png)

    $pct = if ($totalPixels -gt 0) { (100.0 * $diffCount / $totalPixels) } else { 0.0 }
    $meanPerChannel = if ($totalPixels -gt 0) { ($sumAbs / ($totalPixels * 3.0)) } else { 0.0 }

    Write-Output ("diff_pixels={0} total_pixels={1} percent_diff={2:N3}% mean_abs_per_channel={3:N3} max_channel_diff={4}" -f `
        $diffCount, $totalPixels, $pct, $meanPerChannel, $maxHeat)
}
finally {
    if ($refBmp) { $refBmp.Dispose() }
    if ($candBmp) { $candBmp.Dispose() }
    if ($diffBmp) { $diffBmp.Dispose() }
}

