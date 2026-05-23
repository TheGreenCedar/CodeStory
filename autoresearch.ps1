$ErrorActionPreference = "Stop"

$Ledger = Join-Path $PSScriptRoot "autoresearch.jsonl"
$PreviousTotal = $null
$PreviousClosed = $null

if (Test-Path -LiteralPath $Ledger) {
    Get-Content -LiteralPath $Ledger | ForEach-Object {
        if ([string]::IsNullOrWhiteSpace($_)) { return }

        try {
            $Entry = $_ | ConvertFrom-Json -ErrorAction Stop
        } catch {
            return
        }

        if ($null -ne $Entry.run -and $null -ne $Entry.metrics) {
            if ($null -ne $Entry.metrics.quality_total) {
                $PreviousTotal = [int]$Entry.metrics.quality_total
            }
            if ($null -ne $Entry.metrics.quality_closed) {
                $PreviousClosed = [int]$Entry.metrics.quality_closed
            } elseif ($null -ne $Entry.metric) {
                $PreviousClosed = [int]$Entry.metric
            }
        }
    }
}

$RawGap = & "C:\\Program Files\\nodejs\\node.exe" "C:\\Users\\alber\\.codex\\plugins\\cache\\thegreencedar-autoresearch\\codex-autoresearch\\1.3.7\\scripts\\autoresearch.mjs" quality-gap --cwd . --research-slug "codestory-real-repo-friction-20260522" --json
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$Gap = ($RawGap -join "`n") | ConvertFrom-Json
$Open = [int]$Gap.open
$Total = [int]$Gap.total
$Closed = [int]$Gap.closed

$NewlyAccepted = if ($null -ne $PreviousTotal) { [Math]::Max(0, $Total - [int]$PreviousTotal) } else { $Total }
$NewlyClosed = if ($null -ne $PreviousClosed) { [Math]::Max(0, $Closed - [int]$PreviousClosed) } else { $Closed }
$Stagnating = if ($Open -eq 0 -and $NewlyAccepted -eq 0 -and $NewlyClosed -eq 0) { 1 } else { 0 }

Write-Output "METRIC quality_closed=$Closed"
Write-Output "METRIC quality_gap=$Open"
Write-Output "METRIC quality_total=$Total"
Write-Output "METRIC quality_newly_accepted=$NewlyAccepted"
Write-Output "METRIC quality_newly_closed=$NewlyClosed"
Write-Output "METRIC quality_stagnating=$Stagnating"
Write-Output "METRIC quality_plateau=$Stagnating"
