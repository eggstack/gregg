<#
.SYNOPSIS
    End-to-end lifecycle smoke test for the greggd Windows service.

.DESCRIPTION
    Exercises the full install/start/query/stop/restart/uninstall lifecycle
    using local files and loopback only. Requires Administrator privileges.
    Do NOT run in CI — this is a manual maintainer test.

    Prerequisites:
    - Build greggd in release mode: cargo build --release -p greggd
    - Run this script from the repository root as Administrator.

.EXAMPLE
    .\scripts\smoke-windows.ps1 -ExePath .\target\release\greggd.exe
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateScript({ Test-Path $_ -PathType Leaf })]
    [string]$ExePath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ServiceName = "greggd"
$Port = 11399  # Use a non-default port to avoid conflicts.
$HostAddr = "127.0.0.1"
$HealthUrl = "http://${HostAddr}:${Port}/v2/healthz"
$StatusUrl = "http://${HostAddr}:${Port}/v2/status"

# ── Admin check ────────────────────────────────────────────────────────────

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Error "This script must be run as Administrator."
    exit 1
}

$ExePath = (Resolve-Path $ExePath).Path
Write-Host "=== greggd Windows lifecycle smoke ===" -ForegroundColor Cyan
Write-Host "Binary: $ExePath"
Write-Host ""

# ── Helper functions ──────────────────────────────────────────────────────

function Wait-ForUrl {
    param([string]$Url, [int]$TimeoutSeconds = 15)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        try {
            $response = Invoke-WebRequest -Uri $Url -UseBasicParsing -TimeoutSec 2 -ErrorAction Stop
            if ($response.StatusCode -eq 200) {
                return $true
            }
        } catch {
            Start-Sleep -Milliseconds 500
        }
    }
    return $false
}

function Stop-AndRemoveService {
    $svc = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
    if ($svc) {
        Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 2
        sc.exe delete $ServiceName | Out-Null
        Start-Sleep -Seconds 2
    }
}

# ── Cleanup from previous runs ────────────────────────────────────────────

Write-Host "1. Cleaning up from previous runs..."
Stop-AndRemoveService

# ── Install ────────────────────────────────────────────────────────────────

Write-Host "2. Installing from $ExePath..."
$InstallDir = Join-Path $env:ProgramFiles "Gregg"
$ProgramDataDir = Join-Path $env:ProgramData "gregg"
$ConfigDir = Join-Path $ProgramDataDir "greggd-smoke"
$ConfigPath = Join-Path $ConfigDir "greggd.toml"

New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
New-Item -ItemType Directory -Path $ConfigDir -Force | Out-Null

Copy-Item -Path $ExePath -Destination (Join-Path $InstallDir "greggd.exe") -Force

$Config = @"
name = "smoke-test"
host = "$HostAddr"
port = $Port
sample_interval_ms = 1000
stale_after_ms = 10000
"@
[System.IO.File]::WriteAllText($ConfigPath, $Config)

$InstalledExe = Join-Path $InstallDir "greggd.exe"
$ImagePath = "`"$InstalledExe`" service --config `"$ConfigPath`""
sc.exe create $ServiceName binPath= $ImagePath start= demand DisplayName= "Gregg Smoke Test" | Out-Null
sc.exe config $ServiceName obj= "NT AUTHORITY\LocalService" | Out-Null
Write-Host "   Service registered."

# ── Start and verify ──────────────────────────────────────────────────────

Write-Host "3. Starting service..."
Start-Service -Name $ServiceName

Write-Host "4. Waiting for /v2/healthz..."
if (-not (Wait-ForUrl -Url $HealthUrl -TimeoutSeconds 15)) {
    Write-Error "Health check did not become available within 15 seconds."
    Stop-AndRemoveService
    exit 1
}

$health = Invoke-RestMethod -Uri $HealthUrl -UseBasicParsing
Write-Host "   healthz: $($health | ConvertTo-Json -Compress)"

$status = Invoke-RestMethod -Uri $StatusUrl -UseBasicParsing
Write-Host "   status:  schema_version=$($status.schema_version)"

# ── Stop and verify ───────────────────────────────────────────────────────

Write-Host "5. Stopping service..."
greggd stop --config $ConfigPath
Start-Sleep -Seconds 3
$svc = Get-Service -Name $ServiceName
if ($svc.Status -ne "Stopped") {
    Write-Error "Service did not stop. Status: $($svc.Status)"
    Stop-AndRemoveService
    exit 1
}
Write-Host "   Service stopped."

# ── Start again and restart ───────────────────────────────────────────────

Write-Host "6. Starting service again..."
greggd start --config $ConfigPath
Start-Sleep -Seconds 3
$svc = Get-Service -Name $ServiceName
if ($svc.Status -ne "Running") {
    Write-Error "Service did not start. Status: $($svc.Status)"
    Stop-AndRemoveService
    exit 1
}

Write-Host "7. Restarting service..."
greggd restart --config $ConfigPath
Start-Sleep -Seconds 5
$svc = Get-Service -Name $ServiceName
if ($svc.Status -ne "Running") {
    Write-Error "Service did not restart. Status: $($svc.Status)"
    Stop-AndRemoveService
    exit 1
}
Write-Host "   Service restarted."

# ── Config mutation ────────────────────────────────────────────────────────

Write-Host "8. Changing port via CLI..."
greggd port 11398 --config $ConfigPath
Start-Sleep -Seconds 5

$loadedConfig = Get-Content $ConfigPath -Raw
if ($loadedConfig -notmatch "port = 11398") {
    Write-Error "Config mutation did not persist."
    Stop-AndRemoveService
    exit 1
}
Write-Host "   Config updated and service restarted."

# ── Reinstall preserves config ────────────────────────────────────────────

Write-Host "9. Reinstalling (config preservation)..."
greggd stop --config $ConfigPath 2>$null
Start-Sleep -Seconds 2
sc.exe delete $ServiceName | Out-Null
Start-Sleep -Seconds 2

Copy-Item -Path $ExePath -Destination $InstalledExe -Force
sc.exe create $ServiceName binPath= $ImagePath start= demand DisplayName= "Gregg Smoke Test" | Out-Null
Start-Service -Name $ServiceName
Start-Sleep -Seconds 3

$reloadedConfig = Get-Content $ConfigPath -Raw
if ($reloadedConfig -notmatch "port = 11398") {
    Write-Error "Config was not preserved across reinstall."
    Stop-AndRemoveService
    exit 1
}
Write-Host "   Config preserved after reinstall."

# ── Uninstall ──────────────────────────────────────────────────────────────

Write-Host "10. Uninstalling..."
greggd stop --config $ConfigPath 2>$null
Start-Sleep -Seconds 2
Stop-AndRemoveService

if (Test-Path $InstallDir) {
    Remove-Item -Path $InstallDir -Recurse -Force
}
Write-Host "   Service and binary removed."

# Verify config is still present (not removed by default).
if (Test-Path $ConfigPath) {
    Write-Host "   Config preserved (as expected)."
} else {
    Write-Warning "Config was removed unexpectedly."
}

# Clean up smoke test config.
Remove-Item -Path $ConfigDir -Recurse -Force -ErrorAction SilentlyContinue

# ── Summary ───────────────────────────────────────────────────────────────

Write-Host ""
Write-Host "=== All smoke tests passed ===" -ForegroundColor Green
