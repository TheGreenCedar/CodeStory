param(
  [string]$ProcessName,
  [string]$Title,
  [int]$WindowHandle,
  [Parameter(Mandatory = $true)][string]$OutPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Drawing | Out-Null
Add-Type @"
using System;
using System.Runtime.InteropServices;
public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
public static class Win32 {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
}
"@ | Out-Null

function Resolve-WindowHandle {
  if ($WindowHandle -and $WindowHandle -ne 0) {
    return [IntPtr]$WindowHandle
  }
  if ($Title) {
    $proc = Get-Process | Where-Object { $_.MainWindowTitle -like "*$Title*" } | Select-Object -First 1
    if (-not $proc) { throw "No window found with title matching: $Title" }
    return [IntPtr]$proc.MainWindowHandle
  }
  if ($ProcessName) {
    $proc = Get-Process -Name $ProcessName -ErrorAction Stop | Select-Object -First 1
    return [IntPtr]$proc.MainWindowHandle
  }
  throw "Provide -ProcessName, -Title, or -WindowHandle."
}

$handle = Resolve-WindowHandle
if ($handle -eq [IntPtr]::Zero) { throw "Window handle is zero. Is the app minimized or not ready?" }

$rect = New-Object RECT
[Win32]::GetWindowRect($handle, [ref]$rect) | Out-Null
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
if ($width -le 0 -or $height -le 0) { throw "Invalid window size: $width x $height" }

$dir = Split-Path -Parent $OutPath
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }

$bmp = New-Object System.Drawing.Bitmap $width, $height
$graphics = [System.Drawing.Graphics]::FromImage($bmp)
$graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bmp.Size)
$bmp.Save($OutPath, [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bmp.Dispose()

[PSCustomObject]@{
  path = $OutPath
  width = $width
  height = $height
  left = $rect.Left
  top = $rect.Top
}

