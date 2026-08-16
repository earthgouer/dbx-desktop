#!/usr/bin/env pwsh
# Boot the globally installed `dsh` tool once and confirm its web UI comes up.
# This both verifies the tool works in the CI environment and pre-warms the
# dsh profile (first boot is slow: it initializes the cordis profile), so the
# app-under-test boots quickly afterwards.

param(
    [int]$Port = 3180,
    [int]$TimeoutSeconds = 240
)

$ErrorActionPreference = 'Continue'

$logOut = Join-Path $PWD 'smoke-dsh.log'
$logErr = Join-Path $PWD 'smoke-dsh.err.log'

Write-Host "Smoke-testing dsh web on port $Port (timeout ${TimeoutSeconds}s)..."

$proc = $null
try {
    if ($IsWindows) {
        $proc = Start-Process -FilePath 'cmd' `
            -ArgumentList '/c', 'dsh', 'web', '--port', "$Port" `
            -RedirectStandardOutput $logOut -RedirectStandardError $logErr `
            -WindowStyle Hidden -PassThru
    } else {
        $dsh = (Get-Command 'dsh' -ErrorAction Stop).Source
        $proc = Start-Process -FilePath $dsh `
            -ArgumentList 'web', '--port', "$Port" `
            -RedirectStandardOutput $logOut -RedirectStandardError $logErr `
            -PassThru
    }
} catch {
    Write-Host "::error::Failed to start dsh: $($_.Exception.Message)"
    exit 1
}
Write-Host "dsh started, PID $($proc.Id)"

# --- poll for the dsh web UI ---------------------------------------------------
$serverOk = $false
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
while ((Get-Date) -lt $deadline) {
    try {
        $resp = Invoke-WebRequest -Uri "http://127.0.0.1:$Port/" -TimeoutSec 5 -UseBasicParsing
        $low = $resp.Content.ToLowerInvariant()
        if ($resp.StatusCode -lt 400 -and ($low -match 'deepseek' -or $low -match 'harness')) {
            $serverOk = $true
            Write-Host "dsh web UI came up on port $Port."
            break
        }
    } catch {
        # not ready yet; keep polling
    }
    Start-Sleep -Seconds 3
}

if (-not $serverOk) {
    Write-Host '::error::dsh web did not respond in time. Tail of logs:'
    foreach ($f in @($logOut, $logErr)) {
        if (Test-Path -LiteralPath $f) {
            Write-Host "--- $f ---"
            Get-Content -LiteralPath $f -Tail 40
        }
    }
    $logsPresent = (Test-Path -LiteralPath $logOut) -or (Test-Path -LiteralPath $logErr)
    if (-not $logsPresent) { Write-Host '(no logs were produced)' }
    Write-Host '::error::dsh tool smoke test FAILED (dsh web did not serve its UI).'
    exit 1
}

# --- shut dsh down so the app-under-test starts clean ---------------------------
if ($IsWindows) {
    try {
        $listeners = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction Stop
        $pids = $listeners.OwningProcess | Sort-Object -Unique
        foreach ($pid in $pids) {
            cmd /c "taskkill /PID $pid /T /F" 2>&1 | Out-Null
        }
    } catch { }
    if ($proc -and -not $proc.HasExited) {
        try { cmd /c "taskkill /PID $($proc.Id) /T /F" 2>&1 | Out-Null } catch { }
    }
} else {
    if ($proc -and -not $proc.HasExited) {
        try { Stop-Process -Id $proc.Id -Force -ErrorAction Stop } catch { }
    }
    # kill any remaining dsh/cordis worker processes
    pkill -f '@deepseek-ai/dsh' 2>$null | Out-Null
    pkill -f 'bin.js web --port' 2>$null | Out-Null
}

# wait for the smoke server to fully release the port
$deadline = (Get-Date).AddSeconds(20)
while ((Get-Date) -lt $deadline) {
    $still = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
    if (-not $still) { break }
    Start-Sleep -Seconds 1
}

Write-Host '::notice::dsh tool smoke test PASSED (dsh web served its UI; profile pre-warmed).'
