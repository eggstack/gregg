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
    [string]$ExePath,

    [Parameter(Mandatory = $false)]
    [ValidateScript({ Test-Path $_ -PathType Leaf })]
    [string]$GreggExePath
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
$InstalledGreggExe = Join-Path $InstallDir "gregg.exe"
$RepoRoot = Split-Path -Parent $PSScriptRoot
$OccupiedListener = $null
$OccupiedPort = $null

# ── Admin check ────────────────────────────────────────────────────────────

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Error "This script must be run as Administrator."
    exit 1
}

$ExePath = (Resolve-Path $ExePath).Path
if ($GreggExePath) {
    $GreggExePath = (Resolve-Path $GreggExePath).Path
}
Write-Host "=== greggd Windows lifecycle smoke ===" -ForegroundColor Cyan
Write-Host "Binary: $ExePath"
if ($GreggExePath) {
    Write-Host "Client binary: $GreggExePath"
} else {
    Write-Host "Client binary: (not provided; component-safety checks will be skipped)"
}
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
if ([string]$status.system.name -cne "smoke-test") {
    throw "service returned an unexpected configured system name: '$($status.system.name)'"
}
if ([string]::IsNullOrWhiteSpace([string]$status.system.hostname)) {
    throw "service returned an empty system hostname"
}
foreach ($identityValue in @([string]$status.system.name, [string]$status.system.hostname)) {
    if ($identityValue.IndexOf([char]0) -ge 0) {
        throw "service returned a NUL-containing identity value"
    }
}
Write-Host "   identity: name=$($status.system.name), hostname=$($status.system.hostname)"

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

# ── Installer helper self-check (Plan 112) ──────────────────────────────
#
# Deterministic coverage for the install.ps1 same-scope helpers without
# network access: parse the real installer file, load only the pure
# classification functions, and assert absent/replace/foreign behavior
# with the real built binary as the identified component.

Write-Host "11. Checking install.ps1 destination classification helpers..."
$InstallPs1 = Join-Path $RepoRoot "packaging\install.ps1"
if (-not (Test-Path -LiteralPath $InstallPs1 -PathType Leaf)) {
    throw "Installer under test missing: $InstallPs1"
}
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($InstallPs1, [ref]$null, [ref]$parseErrors)
if ($parseErrors -and $parseErrors.Count -gt 0) {
    throw "install.ps1 has syntax errors: $($parseErrors | Out-String)"
}
$wanted = @("Get-ExistingVersion", "Get-DestinationClassification")
$funcs = $ast.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $wanted -contains $node.Name
}, $true)
if ($funcs.Count -ne $wanted.Count) {
    throw "install.ps1 must define $($wanted -join ', ') (found $($funcs.Count))."
}
foreach ($func in $funcs) {
    Invoke-Expression $func.Extent.Text
}
$HelperCheckDir = Join-Path ([System.IO.Path]::GetTempPath()) ("gregg-helper-check-" + [System.IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $HelperCheckDir -Force | Out-Null
try {
    $absentExe = Join-Path $HelperCheckDir "greggd.exe"
    if ((Get-DestinationClassification -DestPath $absentExe -Program "greggd") -ne "absent") {
        throw "missing destination must classify as absent"
    }
    $knownExe = Join-Path $HelperCheckDir "known-greggd.exe"
    Copy-Item -Path $ExePath -Destination $knownExe -Force
    if ((Get-DestinationClassification -DestPath $knownExe -Program "greggd") -ne "replace") {
        throw "real greggd binary must classify as replace"
    }
    $existingVersion = Get-ExistingVersion -DestPath $knownExe
    if (-not $existingVersion.StartsWith("greggd ")) {
        throw "existing version must identify the component (got: '$existingVersion')"
    }
    $foreignExe = Join-Path $HelperCheckDir "foreign.exe"
    "not a gregg binary" | Out-File -LiteralPath $foreignExe -Encoding ascii
    if ((Get-DestinationClassification -DestPath $foreignExe -Program "greggd") -ne "foreign") {
        throw "unidentifiable destination must classify as foreign"
    }
    Write-Host "   absent/replace/foreign classification proven against install.ps1."
} finally {
    if (Test-Path -LiteralPath $HelperCheckDir) {
        Remove-Item -LiteralPath $HelperCheckDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# ── Component-safe uninstall (Plan 112) ───────────────────────────────────
#
# install both -> uninstall greggd -> SCM registration gone, greggd.exe
# gone, gregg.exe still runnable, config preserved. The CLI owns the
# lifecycle; no script performs its own recursive directory removal.

if (-not $GreggExePath) {
    Write-Host "12. Skipping component-safety uninstall (no -GreggExePath provided)."
} else {
    Write-Host "12. Installing client alongside daemon (same-scope layout)..."
    Copy-Item -Path $GreggExePath -Destination $InstalledGreggExe -Force
    $greggVersion = & $InstalledGreggExe version 2>&1
    if ($LASTEXITCODE -ne 0) { throw "installed gregg.exe failed version check: $greggVersion" }
    Write-Host "   gregg.exe runnable: $greggVersion"

    Write-Host "13. Uninstalling greggd via the CLI (config preserved)..."
    Invoke-Greggd stop --config $ConfigPath
    Wait-ForServiceStatus -Status "Stopped"
    & $InstalledExe uninstall --config $ConfigPath
    if ($LASTEXITCODE -ne 0) { throw "greggd uninstall failed with exit code $LASTEXITCODE." }

    $deadline = (Get-Date).AddSeconds(15)
    while ((Test-Path -LiteralPath $InstalledExe -PathType Leaf) -and ((Get-Date) -lt $deadline)) {
        Start-Sleep -Milliseconds 250
    }
    if (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
        throw "SCM registration was not removed by greggd uninstall."
    }
    Write-Host "   SCM registration removed."
    if (Test-Path -LiteralPath $InstalledExe -PathType Leaf) {
        throw "greggd.exe was not removed by greggd uninstall."
    }
    Write-Host "   greggd.exe removed."
    if (-not (Test-Path -LiteralPath $InstalledGreggExe -PathType Leaf)) {
        throw "Sibling gregg.exe must survive greggd uninstall."
    }
    $greggStill = & $InstalledGreggExe version 2>&1
    if ($LASTEXITCODE -ne 0) { throw "sibling gregg.exe is not runnable after greggd uninstall: $greggStill" }
    Write-Host "   sibling gregg.exe still runnable: $greggStill"
    if (-not (Test-Path -LiteralPath $ConfigPath -PathType Leaf)) {
        throw "Daemon config was removed without --purge."
    }
    Write-Host "   daemon config preserved (as expected without --purge)."

    Write-Host "14. Uninstalling gregg via the CLI..."
    & $InstalledGreggExe uninstall
    if ($LASTEXITCODE -ne 0) { throw "gregg uninstall failed with exit code $LASTEXITCODE." }
    $deadline = (Get-Date).AddSeconds(15)
    while ((Test-Path -LiteralPath $InstalledGreggExe -PathType Leaf) -and ((Get-Date) -lt $deadline)) {
        Start-Sleep -Milliseconds 250
    }
    if (Test-Path -LiteralPath $InstalledGreggExe -PathType Leaf) {
        throw "gregg.exe was not removed by gregg uninstall."
    }
    Write-Host "   gregg.exe removed."
}

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
