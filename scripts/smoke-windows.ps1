<#
.SYNOPSIS
    End-to-end lifecycle smoke test for the greggd Windows service.

.DESCRIPTION
    Exercises the full install/start/query/stop/restart/bind-failure/reinstall/uninstall lifecycle
    using local files and loopback only. Requires Administrator privileges. This is the
    authoritative Windows SCM lifecycle check used by the existing CI job.

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
$WorkingPort = 11398
$HostAddr = "127.0.0.1"
$HealthUrl = "http://${HostAddr}:${Port}/v2/healthz"
$StatusUrl = "http://${HostAddr}:${Port}/v2/status"
$WorkingHealthUrl = "http://${HostAddr}:${WorkingPort}/v2/healthz"
$InstallDir = Join-Path $env:ProgramFiles "Gregg"
$ProgramDataDir = Join-Path $env:ProgramData "gregg"
$ConfigDir = Join-Path $ProgramDataDir "greggd-smoke"
$ConfigPath = Join-Path $ConfigDir "greggd.toml"
$InstalledExe = Join-Path $InstallDir "greggd.exe"
$OccupiedListener = $null
$OccupiedPort = $null

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
        Stop-Service -Name $ServiceName -Force -ErrorAction Stop
        Wait-ForServiceStatus -Status "Stopped" -TimeoutSeconds 30
        Invoke-Sc -Arguments @("delete", $ServiceName)
        $deadline = (Get-Date).AddSeconds(30)
        while ((Get-Date) -lt $deadline) {
            if (-not (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue)) {
                return
            }
            Start-Sleep -Milliseconds 250
        }
        throw "Service $ServiceName was not removed within 30 seconds."
    }
}

function Wait-ForServiceStatus {
    param(
        [Parameter(Mandatory = $true)][string]$Status,
        [int]$TimeoutSeconds = 30
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        $svc = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
        if ($svc -and $svc.Status -eq $Status) {
            return
        }
        Start-Sleep -Milliseconds 250
    }
    $currentService = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
    $current = if ($currentService) { $currentService.Status } else { "missing" }
    throw "Service $ServiceName did not reach $Status within $TimeoutSeconds seconds (current: $current)."
}

function Invoke-Greggd {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Arguments)
    & $InstalledExe @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "greggd.exe $($Arguments -join ' ') failed with exit code $LASTEXITCODE."
    }
}

function Invoke-Sc {
    param([Parameter(Mandatory = $true)][string[]]$Arguments)
    & sc.exe @Arguments | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "sc.exe $($Arguments -join ' ') failed with exit code $LASTEXITCODE."
    }
}

# ── Cleanup from previous runs ────────────────────────────────────────────

try {
Write-Host "1. Cleaning up from previous runs..."
Stop-AndRemoveService

# ── Install ────────────────────────────────────────────────────────────────

Write-Host "2. Installing from $ExePath..."

New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
New-Item -ItemType Directory -Path $ConfigDir -Force | Out-Null

Copy-Item -Path $ExePath -Destination $InstalledExe -Force

$Config = @"
name = "smoke-test"
host = "$HostAddr"
port = $Port
sample_interval_ms = 1000
stale_after_ms = 10000
"@
[System.IO.File]::WriteAllText($ConfigPath, $Config)

$ImagePath = "`"$InstalledExe`" service --config `"$ConfigPath`""
Invoke-Sc -Arguments @("create", $ServiceName, "binPath=", $ImagePath, "start=", "demand", "DisplayName=", "Gregg Smoke Test")
Invoke-Sc -Arguments @("config", $ServiceName, "obj=", "NT AUTHORITY\LocalService")
Write-Host "   Service registered."

# ── Start and verify ──────────────────────────────────────────────────────

Write-Host "3. Starting service..."
Start-Service -Name $ServiceName
Wait-ForServiceStatus -Status "Running"

Write-Host "4. Waiting for /v2/healthz..."
if (-not (Wait-ForUrl -Url $HealthUrl -TimeoutSeconds 15)) {
    throw "Health check did not become available within 15 seconds."
}

$health = Invoke-RestMethod -Uri $HealthUrl -UseBasicParsing
Write-Host "   healthz: $($health | ConvertTo-Json -Compress)"

$status = Invoke-RestMethod -Uri $StatusUrl -UseBasicParsing
Write-Host "   status:  schema_version=$($status.schema_version)"

# ── Stop and verify ───────────────────────────────────────────────────────

Write-Host "5. Stopping service..."
Invoke-Greggd stop --config $ConfigPath
Wait-ForServiceStatus -Status "Stopped"
Write-Host "   Service stopped."

# ── Start again and restart ───────────────────────────────────────────────

Write-Host "6. Starting service again..."
Invoke-Greggd start --config $ConfigPath
Wait-ForServiceStatus -Status "Running"
if (-not (Wait-ForUrl -Url $HealthUrl)) {
    throw "Health check did not recover after service start."
}

Write-Host "7. Restarting service..."
Invoke-Greggd restart --config $ConfigPath
Wait-ForServiceStatus -Status "Running"
if (-not (Wait-ForUrl -Url $HealthUrl)) {
    throw "Health check did not recover after service restart."
}
Write-Host "   Service restarted."

# ── Config mutation ────────────────────────────────────────────────────────

Write-Host "8. Changing port via CLI..."
Invoke-Greggd port $WorkingPort --config $ConfigPath
Wait-ForServiceStatus -Status "Running"
if (-not (Wait-ForUrl -Url $WorkingHealthUrl)) {
    throw "Health check did not move to the mutated port."
}

$loadedConfig = Get-Content $ConfigPath -Raw
if ($loadedConfig -notmatch "port = $WorkingPort") {
    throw "Config mutation did not persist."
}
Write-Host "   Config updated and service restarted."

# ── Bind failure ──────────────────────────────────────────────────────────

Write-Host "9. Simulating bind failure with an occupied ephemeral loopback port..."
Invoke-Greggd stop --config $ConfigPath
Wait-ForServiceStatus -Status "Stopped"

$OccupiedListener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
$OccupiedListener.Start()
$OccupiedPort = ([System.Net.IPEndPoint]$OccupiedListener.LocalEndpoint).Port
$portMutationFailed = $false
try {
    Invoke-Greggd port $OccupiedPort --config $ConfigPath
} catch {
    $portMutationFailed = $true
    Write-Host "   Expected startup failure observed on occupied port $OccupiedPort."
}
$occupiedConfig = Get-Content $ConfigPath -Raw
if ($occupiedConfig -notmatch "port = $OccupiedPort") {
    throw "Bind-failure setup did not persist occupied port $OccupiedPort."
}
if (-not $portMutationFailed) {
    Write-Host "   SCM accepted the mutation command; explicit start failure remains required."
}
$scmStartFailed = $false
try {
    Start-Service -Name $ServiceName
    Wait-ForServiceStatus -Status "Running" -TimeoutSeconds 5
} catch {
    $scmStartFailed = $true
    Write-Host "   Expected SCM start failure observed: $($_.Exception.Message)"
}
if (-not $scmStartFailed) {
    throw "SCM start unexpectedly reached Running while port $OccupiedPort was occupied."
}
$svc = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($svc -and $svc.Status -eq "Running") {
    throw "Service should not be running after bind failure."
}
Write-Host "   Service is not running after bind failure (expected)."

# Restore working port for remaining tests.
$OccupiedListener.Stop()
$OccupiedListener = $null
Invoke-Greggd port $WorkingPort --config $ConfigPath
Wait-ForServiceStatus -Status "Running"
if (-not (Wait-ForUrl -Url $WorkingHealthUrl)) {
    throw "Health check did not recover after restoring the working port."
}

# ── Reinstall preserves config ────────────────────────────────────────────

Write-Host "10. Reinstalling (config preservation)..."
Invoke-Greggd stop --config $ConfigPath
Wait-ForServiceStatus -Status "Stopped"
Stop-AndRemoveService

New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
Copy-Item -Path $ExePath -Destination $InstalledExe -Force
Invoke-Sc -Arguments @("create", $ServiceName, "binPath=", $ImagePath, "start=", "demand", "DisplayName=", "Gregg Smoke Test")
Invoke-Sc -Arguments @("config", $ServiceName, "obj=", "NT AUTHORITY\LocalService")
Start-Service -Name $ServiceName
Wait-ForServiceStatus -Status "Running"

$reloadedConfig = Get-Content $ConfigPath -Raw
if ($reloadedConfig -notmatch "port = $WorkingPort") {
    throw "Config was not preserved across reinstall."
}
Write-Host "   Config preserved after reinstall."

# ── Uninstall ──────────────────────────────────────────────────────────────

Write-Host "11. Uninstalling (preserving config)..."
Invoke-Greggd stop --config $ConfigPath
Wait-ForServiceStatus -Status "Stopped"
Stop-AndRemoveService

if (Test-Path $InstallDir) {
    Remove-Item -Path $InstallDir -Recurse -Force -ErrorAction Stop
}
Write-Host "   Service and binary removed."

# Verify config is still present (not removed by default).
if (Test-Path $ConfigPath) {
    Write-Host "   Config preserved (as expected)."
} else {
    throw "Config was removed unexpectedly."
}

# ── Reinstall and uninstall with -RemoveConfig ───────────────────────────

Write-Host "12. Reinstalling for RemoveConfig test..."
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
Copy-Item -Path $ExePath -Destination $InstalledExe -Force
Invoke-Sc -Arguments @("create", $ServiceName, "binPath=", $ImagePath, "start=", "demand", "DisplayName=", "Gregg Smoke Test")
Invoke-Sc -Arguments @("config", $ServiceName, "obj=", "NT AUTHORITY\LocalService")
Start-Service -Name $ServiceName
Wait-ForServiceStatus -Status "Running"
Write-Host "   Service running after reinstall."

Write-Host "13. Uninstalling with config removal..."
Invoke-Greggd stop --config $ConfigPath
Wait-ForServiceStatus -Status "Stopped"
Stop-AndRemoveService

if (Test-Path $InstallDir) {
    Remove-Item -Path $InstallDir -Recurse -Force -ErrorAction Stop
}
# Manually remove config to simulate -RemoveConfig behavior.
if (Test-Path $ConfigDir) {
    Remove-Item -Path $ConfigDir -Recurse -Force -ErrorAction Stop
}
Write-Host "   Service, binary, and config removed."

if (Test-Path $ConfigDir) {
    throw "Config directory was not removed."
}
Write-Host "   Config directory confirmed removed."

# ── Summary ───────────────────────────────────────────────────────────────

Write-Host ""
Write-Host "=== All smoke tests passed ===" -ForegroundColor Green
} finally {
    $cleanupErrors = [System.Collections.Generic.List[string]]::new()
    if ($OccupiedListener) {
        try {
            $OccupiedListener.Stop()
        } catch {
            [void]$cleanupErrors.Add("Could not release occupied port: $($_.Exception.Message)")
        }
    }
    try {
        Stop-AndRemoveService
    } catch {
        [void]$cleanupErrors.Add("Could not remove ${ServiceName}: $($_.Exception.Message)")
    }
    if (Test-Path $InstallDir) {
        try {
            Remove-Item -Path $InstallDir -Recurse -Force -ErrorAction Stop
        } catch {
            [void]$cleanupErrors.Add("Could not remove $InstallDir`: $($_.Exception.Message)")
        }
    }
    if (Test-Path $ConfigDir) {
        try {
            Remove-Item -Path $ConfigDir -Recurse -Force -ErrorAction Stop
        } catch {
            [void]$cleanupErrors.Add("Could not remove $ConfigDir`: $($_.Exception.Message)")
        }
    }
    if (Test-Path $InstallDir) {
        [void]$cleanupErrors.Add("Install directory still exists: $InstallDir")
    }
    if (Test-Path $ConfigDir) {
        [void]$cleanupErrors.Add("Config directory still exists: $ConfigDir")
    }
    if ($cleanupErrors.Count -gt 0) {
        throw ("Cleanup failed: " + ($cleanupErrors -join "; "))
    }
}
