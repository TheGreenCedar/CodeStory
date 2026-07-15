param(
  [Parameter(Mandatory = $true)]
  [string]$CommandLine,
  [int]$SampleMilliseconds = 500,
  [string]$OutDir = ""
)

$ErrorActionPreference = "Stop"

if (-not $OutDir) {
  $stamp = Get-Date -Format "yyyyMMddTHHmmss"
  $OutDir = Join-Path (Resolve-Path .) "target\memory-measure\$stamp"
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$stdoutPath = Join-Path $OutDir "stdout.log"
$stderrPath = Join-Path $OutDir "stderr.log"
$summaryPath = Join-Path $OutDir "summary.json"
$exitCodePath = Join-Path $OutDir "exitcode.txt"
$filePath = $env:COMSPEC
if (-not $filePath) {
  $filePath = "cmd.exe"
}
$commandLineWithExit = "$CommandLine & set CODESTORY_MEASURE_EXIT=!ERRORLEVEL! & echo !CODESTORY_MEASURE_EXIT! > `"$exitCodePath`" & exit /b !CODESTORY_MEASURE_EXIT!"
$argumentList = @("/d", "/v:on", "/s", "/c", $commandLineWithExit)

function Get-DescendantProcessIds {
  param([int]$RootProcessId)

  $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name
  $childrenByParent = @{}
  foreach ($process in $all) {
    $parent = [int]$process.ParentProcessId
    if (-not $childrenByParent.ContainsKey($parent)) {
      $childrenByParent[$parent] = New-Object System.Collections.Generic.List[int]
    }
    $childrenByParent[$parent].Add([int]$process.ProcessId)
  }

  $ids = New-Object System.Collections.Generic.HashSet[int]
  $queue = New-Object System.Collections.Generic.Queue[int]
  [void]$ids.Add($RootProcessId)
  $queue.Enqueue($RootProcessId)
  while ($queue.Count -gt 0) {
    $current = $queue.Dequeue()
    if (-not $childrenByParent.ContainsKey($current)) {
      continue
    }
    foreach ($child in $childrenByParent[$current]) {
      if ($ids.Add($child)) {
        $queue.Enqueue($child)
      }
    }
  }
  return @($ids)
}

function Get-NvidiaVramBytes {
  $nvidia = Get-Command nvidia-smi -ErrorAction SilentlyContinue
  if (-not $nvidia) {
    return $null
  }
  $values = & $nvidia.Source --query-gpu=memory.used --format=csv,noheader,nounits 2>$null
  if ($LASTEXITCODE -ne 0 -or -not $values) {
    return $null
  }
  $sumMb = 0.0
  foreach ($value in $values) {
    $parsed = 0.0
    if ([double]::TryParse(($value -replace '[^0-9.]', ''), [ref]$parsed)) {
      $sumMb += $parsed
    }
  }
  return [int64]($sumMb * 1MB)
}

function Update-Peaks {
  param([int]$RootProcessId)

  $ids = Get-DescendantProcessIds -RootProcessId $RootProcessId
  $processes = @()
  if ($ids.Count -gt 0) {
    $processes = @(Get-Process -Id $ids -ErrorAction SilentlyContinue)
  }

  $descendantBytes = 0L
  foreach ($process in $processes) {
    $descendantBytes += [int64]$process.WorkingSet64
    if ($script:PeakProcessWorkingSetBytes -lt [int64]$process.WorkingSet64) {
      $script:PeakProcessWorkingSetBytes = [int64]$process.WorkingSet64
    }
    if ($process.ProcessName -eq "codestory-cli" -and $script:PeakCodestoryCliWorkingSetBytes -lt [int64]$process.WorkingSet64) {
      $script:PeakCodestoryCliWorkingSetBytes = [int64]$process.WorkingSet64
    }
  }
  if ($script:PeakDescendantWorkingSetBytes -lt $descendantBytes) {
    $script:PeakDescendantWorkingSetBytes = $descendantBytes
  }

  $vramBytes = Get-NvidiaVramBytes
  if ($null -ne $vramBytes -and ($null -eq $script:PeakVramBytes -or $script:PeakVramBytes -lt $vramBytes)) {
    $script:PeakVramBytes = $vramBytes
  }
}

function Format-MetricMb {
  param([int64]$Bytes)
  return [Math]::Round($Bytes / 1MB, 6)
}

$script:PeakDescendantWorkingSetBytes = 0L
$script:PeakProcessWorkingSetBytes = 0L
$script:PeakCodestoryCliWorkingSetBytes = 0L
$script:PeakVramBytes = $null

$started = Get-Date
$process = Start-Process `
  -FilePath $filePath `
  -ArgumentList $argumentList `
  -WorkingDirectory (Resolve-Path .) `
  -RedirectStandardOutput $stdoutPath `
  -RedirectStandardError $stderrPath `
  -PassThru `
  -WindowStyle Hidden

while (-not $process.HasExited) {
  Update-Peaks -RootProcessId $process.Id
  Start-Sleep -Milliseconds $SampleMilliseconds
  $process.Refresh()
}
$process.WaitForExit()
$process.Refresh()
Update-Peaks -RootProcessId $process.Id
$ended = Get-Date
$exitCode = $process.ExitCode
if (Test-Path $exitCodePath) {
  $exitCodeText = (Get-Content -Raw -Path $exitCodePath).Trim()
  $parsedExitCode = 0
  if ([int]::TryParse($exitCodeText, [ref]$parsedExitCode)) {
    $exitCode = $parsedExitCode
  }
}
if ($null -eq $exitCode) {
  $exitCode = 1
}

if (Test-Path $stdoutPath) {
  Get-Content $stdoutPath | ForEach-Object { Write-Output $_ }
}
if (Test-Path $stderrPath) {
  Get-Content $stderrPath | ForEach-Object { Write-Output $_ }
}

$summary = [ordered]@{
  command_line = $CommandLine
  command = @($filePath) + $argumentList
  exit_code = $exitCode
  started_at = $started.ToString("o")
  ended_at = $ended.ToString("o")
  elapsed_seconds = [Math]::Round(($ended - $started).TotalSeconds, 3)
  sample_milliseconds = $SampleMilliseconds
  peak_descendant_working_set_mb = Format-MetricMb $script:PeakDescendantWorkingSetBytes
  peak_process_working_set_mb = Format-MetricMb $script:PeakProcessWorkingSetBytes
  peak_codestory_cli_working_set_mb = Format-MetricMb $script:PeakCodestoryCliWorkingSetBytes
  peak_vram_mb = if ($null -eq $script:PeakVramBytes) { $null } else { Format-MetricMb $script:PeakVramBytes }
  stdout_path = $stdoutPath
  stderr_path = $stderrPath
  exit_code_path = $exitCodePath
}
$summary | ConvertTo-Json -Depth 5 | Set-Content -Path $summaryPath -Encoding UTF8

Write-Output "memory summary: $summaryPath"
Write-Output "METRIC peak_descendant_working_set_mb=$($summary.peak_descendant_working_set_mb)"
Write-Output "METRIC peak_process_working_set_mb=$($summary.peak_process_working_set_mb)"
Write-Output "METRIC peak_codestory_cli_working_set_mb=$($summary.peak_codestory_cli_working_set_mb)"
if ($null -ne $summary.peak_vram_mb) {
  Write-Output "METRIC peak_vram_mb=$($summary.peak_vram_mb)"
} else {
  Write-Output "INFO peak_vram_mb unavailable: nvidia-smi not found or did not return memory.used"
}

exit $exitCode
