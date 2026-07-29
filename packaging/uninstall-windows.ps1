<#
.SYNOPSIS
    Removes the greggd Windows service and optionally its configuration.

.DESCRIPTION
    Stops and removes the greggd Windows service, deletes the installed
    binary, and optionally removes the configuration directory.

    This script must be run as Administrator.

.PARAMETER RemoveConfig
    If specified, removes the ProgramData configuration directory
    (%ProgramData%\gregg). By default, configuration is preserved.

.EXAMPLE
    .\uninstall-windows.ps1
    .\uninstall-windows.ps1 -RemoveConfig
#>
[CmdletBinding()]
param(
    [switch]$RemoveConfig
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ── Admin check ────────────────────────────────────────────────────────────

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Error "This script must be run as Administrator."
    exit 1
}

# ── Constants ──────────────────────────────────────────────────────────────

$ServiceName = "greggd"
$InstallDir = Join-Path $env:ProgramFiles "Gregg"
$ProgramDataDir = Join-Path $env:ProgramData "gregg"

# ── Stop and remove service ───────────────────────────────────────────────

$service = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($service) {
    Write-Host "Stopping service..."
    Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue

    # Wait for the service to stop (up to 30 seconds).
    try {
        $service.WaitForStatus("Stopped", (New-TimeSpan -Seconds 30))
    } catch {
        Write-Warning "Service did not stop within 30 seconds."
    }

    Write-Host "Removing service registration..."
    sc.exe delete $ServiceName | Out-Null
    Start-Sleep -Seconds 2  # Give SCM time to process the deletion.
    Write-Host "Service removed."
} else {
    Write-Host "Service '$ServiceName' is not registered. Continuing..."
}

# ── Remove installed binary ───────────────────────────────────────────────

if (Test-Path $InstallDir) {
    Write-Host "Removing installed binary directory: $InstallDir"
    try {
        Remove-Item -Path $InstallDir -Recurse -Force
        Write-Host "Binary directory removed."
    } catch {
        Write-Warning "Could not remove $InstallDir : $($_.Exception.Message)"
        Write-Warning "The file may be in use. Please close any running instances and try again."
    }
} else {
    Write-Host "Binary directory not found. Continuing..."
}

# ── Remove config directory if requested ──────────────────────────────────

if ($RemoveConfig) {
    if (Test-Path $ProgramDataDir) {
        Write-Host "Removing configuration directory: $ProgramDataDir"
        Remove-Item -Path $ProgramDataDir -Recurse -Force
        Write-Host "Configuration directory removed."
    } else {
        Write-Host "Configuration directory not found. Continuing..."
    }
} else {
    if (Test-Path $ProgramDataDir) {
        Write-Host "Configuration preserved at: $ProgramDataDir"
        Write-Host "Use -RemoveConfig to delete the configuration directory."
    }
}

# ── Summary ───────────────────────────────────────────────────────────────

Write-Host ""
Write-Host "=== greggd uninstalled ===" -ForegroundColor Green
if ($RemoveConfig) {
    Write-Host "Service, binary, and configuration have been removed."
} else {
    Write-Host "Service and binary removed. Configuration preserved."
}
