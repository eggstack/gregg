<#
.SYNOPSIS
    Compatibility entry point for removing the greggd daemon on Windows.

.DESCRIPTION
    Delegates to the installed `greggd.exe uninstall` command, which owns the
    component-safe lifecycle: it stops and deletes only the `greggd` SCM
    registration, deletes only the exact invoked `greggd.exe`, preserves
    configuration by default, and never removes a sibling `gregg.exe` or
    recursively deletes the shared install directory.

    The previous recursive `%ProgramFiles%\Gregg` removal is retired: shared
    directories are containers, not component-owned artifacts.

    This script must be run as Administrator when the daemon was installed
    system-wide.

.PARAMETER RemoveConfig
    Maps to `greggd uninstall --purge`: additionally removes the daemon
    config file (`%ProgramData%\gregg\greggd.toml`). By default,
    configuration is preserved.

.PARAMETER GreggdExe
    Explicit path to the installed `greggd.exe`. Defaults to
    `%ProgramFiles%\Gregg\greggd.exe`.

.EXAMPLE
    .\uninstall-windows.ps1
    .\uninstall-windows.ps1 -RemoveConfig
    .\uninstall-windows.ps1 -GreggdExe "C:\tools\greggd.exe" -RemoveConfig
#>
[CmdletBinding()]
param(
    [switch]$RemoveConfig,
    [string]$GreggdExe
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ── Admin check ────────────────────────────────────────────────────────────

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Error "This script must be run as Administrator when greggd was installed system-wide. Otherwise run the installed binary directly: greggd uninstall"
    exit 1
}

# ── Resolve the installed daemon binary ────────────────────────────────────

if (-not $GreggdExe) {
    $GreggdExe = Join-Path $env:ProgramFiles "Gregg\greggd.exe"
}
if (-not (Test-Path -LiteralPath $GreggdExe -PathType Leaf)) {
    Write-Error "Installed daemon not found at $GreggdExe. Nothing was changed. If greggd was installed user-local, pass -GreggdExe explicitly or run that binary's uninstall command directly: <path>\greggd.exe uninstall"
    exit 1
}

# ── Delegate to the CLI-owned uninstall ────────────────────────────────────

$uninstallArgs = @("uninstall")
if ($RemoveConfig) {
    $uninstallArgs += "--purge"
}

Write-Host "Delegating to: $GreggdExe $($uninstallArgs -join ' ')"
& $GreggdExe @uninstallArgs
$exitCode = $LASTEXITCODE
if ($exitCode -ne 0) {
    Write-Error "greggd uninstall failed with exit code $exitCode."
    exit $exitCode
}

Write-Host ""
Write-Host "=== greggd uninstalled ===" -ForegroundColor Green
if ($RemoveConfig) {
    Write-Host "Service and binary removed; configuration purged."
} else {
    Write-Host "Service and binary removed. Configuration preserved."
    Write-Host "Rerun with -RemoveConfig to also remove %ProgramData%\gregg\greggd.toml."
}
