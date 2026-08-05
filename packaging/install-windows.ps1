<#
.SYNOPSIS
    Installs greggd as a native Windows service.

.DESCRIPTION
    Installs the greggd metrics daemon as a Windows service managed by the
    Service Control Manager (SCM). The service runs under the LocalService
    account with minimal required privileges.

    This script must be run as Administrator.

.PARAMETER SourcePath
    Path to the greggd.exe binary. Defaults to the current directory.

.PARAMETER ConfigPath
    Path to the TOML configuration file. If not specified, a default config
    is created at %ProgramData%\gregg\greggd.toml.

.EXAMPLE
    .\install-windows.ps1 -SourcePath .\target\release\greggd.exe
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateScript({ Test-Path $_ -PathType Leaf })]
    [string]$SourcePath = ".\greggd.exe",

    [string]$ConfigPath
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
$DisplayName = "Gregg Metrics Daemon"
$InstallDir = Join-Path $env:ProgramFiles "Gregg"
$ProgramDataDir = Join-Path $env:ProgramData "gregg"
$DefaultConfigPath = Join-Path $ProgramDataDir "greggd.toml"

# ── Resolve source path ───────────────────────────────────────────────────

$SourcePath = (Resolve-Path $SourcePath).Path
Write-Host "Source binary: $SourcePath"

# ── Create install directory ──────────────────────────────────────────────

if (-not (Test-Path $InstallDir)) {
    Write-Host "Creating install directory: $InstallDir"
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

# ── Stop existing service if present ──────────────────────────────────────

$existingService = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($existingService) {
    Write-Host "Stopping existing service..."
    Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue
    # Wait for the service to stop
    $existingService.WaitForStatus("Stopped", (New-TimeSpan -Seconds 30)) -ErrorAction SilentlyContinue
}

# ── Copy binary ───────────────────────────────────────────────────────────

$InstalledExe = Join-Path $InstallDir "greggd.exe"
Write-Host "Installing binary to: $InstalledExe"
Copy-Item -Path $SourcePath -Destination $InstalledExe -Force

# ── Create ProgramData directory and default config ───────────────────────

if (-not (Test-Path $ProgramDataDir)) {
    Write-Host "Creating config directory: $ProgramDataDir"
    New-Item -ItemType Directory -Path $ProgramDataDir -Force | Out-Null
}

if ($ConfigPath) {
    # Use the explicitly provided config.
    $ConfigPath = (Resolve-Path $ConfigPath).Path
    Write-Host "Using config: $ConfigPath"
} elseif (-not (Test-Path $DefaultConfigPath)) {
    # Create default config only if none exists.
    Write-Host "Creating default config: $DefaultConfigPath"
    $DefaultConfig = @"
name = "greggd"
host = "0.0.0.0"
port = 11310
sample_interval_ms = 1000
stale_after_ms = 10000
"@
    [System.IO.File]::WriteAllText($DefaultConfigPath, $DefaultConfig)
    $ConfigPath = $DefaultConfigPath
} else {
    Write-Host "Existing config preserved: $DefaultConfigPath"
    $ConfigPath = $DefaultConfigPath
}

# ── Build the service image path ──────────────────────────────────────────

# Quote the executable path and pass the service subcommand.
# The config path is passed via --config.
$ImagePath = "`"$InstalledExe`" service --config `"$ConfigPath`""

# ── Register the service ──────────────────────────────────────────────────

$service = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($service) {
    # Update existing service configuration.
    Write-Host "Updating service registration..."
    sc.exe config $ServiceName binPath= $ImagePath start= auto | Out-Null
} else {
    # Create new service.
    Write-Host "Registering service..."
    sc.exe create $ServiceName binPath= $ImagePath start= auto DisplayName= $DisplayName | Out-Null
}

# Configure LocalService account.
Write-Host "Configuring service account: NT AUTHORITY\LocalService"
sc.exe config $ServiceName obj= "NT AUTHORITY\LocalService" | Out-Null

# ── Configure failure recovery ────────────────────────────────────────────

# Restart the service up to 3 times with 60-second delays on failure.
Write-Host "Configuring failure recovery..."
sc.exe failure $ServiceName reset= 86400 actions= restart/60000/restart/60000/restart/60000 | Out-Null

# ── Start the service ─────────────────────────────────────────────────────

Write-Host "Starting service..."
Start-Service -Name $ServiceName

# Wait for the service to reach Running state.
$service = Get-Service -Name $ServiceName
$service.WaitForStatus("Running", (New-TimeSpan -Seconds 30))

# ── Summary ───────────────────────────────────────────────────────────────

Write-Host ""
Write-Host "=== greggd installed successfully ===" -ForegroundColor Green
Write-Host ""
Write-Host "  Binary:       $InstalledExe"
Write-Host "  Config:       $ConfigPath"
Write-Host "  Service:      $ServiceName ($DisplayName)"
Write-Host "  Account:      NT AUTHORITY\LocalService"
Write-Host "  Start type:   Automatic"
Write-Host ""
Write-Host "Useful commands:"
Write-Host "  greggd stop              # stop the service"
Write-Host "  greggd start             # start the service"
Write-Host "  greggd restart           # restart the service"
Write-Host "  greggd run               # run in foreground (diagnostics)"
Write-Host "  Get-Service greggd       # check service status"
Write-Host ""
Write-Host "The daemon uses the selected configuration file for its bind address and port."
Write-Host "Use 'greggd host 127.0.0.1' to restrict to localhost (recommended)."
Write-Host "No firewall rule is created. LAN exposure is operator-controlled."
