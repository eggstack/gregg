<#
.SYNOPSIS
    Bootstrap installer for prebuilt gregg/greggd binaries on Windows.

.DESCRIPTION
    Binary-first, Cargo fallback second. Detects the current Windows
    architecture, downloads the matching GitHub Release asset for the pinned
    or latest version, verifies SHA-256 and candidate version, then installs
    to %ProgramFiles%\Gregg when Administrator or to %LOCALAPPDATA%\Gregg
    otherwise. Existing %ProgramData%\gregg\greggd.toml is preserved.

    If no matching asset exists (HTTP 404 or intentionally source-only host
    such as ARM64/ARMv7) and Cargo is available, falls back to
    `cargo install`.

.PARAMETER Component
    Which binary to install: Gregg (client), Greggd (daemon), or Both.

.PARAMETER Version
    Pinned version X.Y.Z (with or without leading v). When omitted, the
    latest release is used via releases/latest/download.

.EXAMPLE
    irm https://github.com/eggstack/gregg/releases/latest/download/install.ps1 | iex
    .\install.ps1 -Component Gregg
    .\install.ps1 -Component Greggd -Version 1.0.11
    .\install.ps1 -Component Both -Version v1.0.11
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet("Gregg", "Greggd", "Both", "gregg", "greggd", "both")]
    [string]$Component = "Gregg",

    [string]$Version
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ── Normalize component ────────────────────────────────────────────────────

$Component = $Component.ToLower()
switch ($Component) {
    "gregg"  { $ComponentNorm = "gregg" }
    "greggd" { $ComponentNorm = "greggd" }
    "both"   { $ComponentNorm = "both" }
    default  { throw "Unknown component: $Component" }
}

# ── Version handling ───────────────────────────────────────────────────────

$StrippedVersion = ""
$Tag = ""
if ($Version) {
    $StrippedVersion = $Version.Trim()
    if ($StrippedVersion.StartsWith("v")) { $StrippedVersion = $StrippedVersion.Substring(1) }
    if ($StrippedVersion -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z\.\-]+)?(\+[0-9A-Za-z\.\-]+)?$') {
        throw "Version must be X.Y.Z (got '$Version')"
    }
    $Tag = "v$StrippedVersion"
}

# ── Host mapping ───────────────────────────────────────────────────────────

$Repo = "eggstack/gregg"
$BaseUrl = "https://github.com/$Repo/releases"

$Arch = $env:PROCESSOR_ARCHITECTURE
if (-not $Arch) { $Arch = "unknown" }

$Target = $null
$SupportedBinary = $false

# Only x86_64 is published in this phase; ARM64/ARMv7 are source-only.
if ($Arch -eq "AMD64") {
    # Confirm 64-bit OS (defense in depth; PROCESSOR_ARCHITECTURE AMD64 already implies it)
    $Is64BitOS = [Environment]::Is64BitOperatingSystem
    if ($Is64BitOS) {
        $Target = "x86_64-pc-windows-msvc"
        $SupportedBinary = $true
    } else {
        $Target = $null
        $SupportedBinary = $false
    }
} elseif ($Arch -eq "ARM64") {
    $Target = "aarch64-pc-windows-msvc"
    $SupportedBinary = $false
} else {
    $Target = $null
    $SupportedBinary = $false
}

Write-Host "Detected: ARCH=$Arch TARGET=$($Target ? $Target : 'unknown') component=$ComponentNorm $($Tag ? $Tag : 'latest')"

# ── Privilege / destination ────────────────────────────────────────────────

$IsAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

function Get-DestDir {
    param([string]$Program, [bool]$IsAdmin)
    if ($Program -eq "greggd" -and $IsAdmin) {
        return (Join-Path $env:ProgramFiles "Gregg")
    }
    if ($Program -eq "gregg" -and $IsAdmin) {
        # Installing gregg system-wide when explicitly privileged is acceptable
        return (Join-Path $env:ProgramFiles "Gregg")
    }
    # User-local fallback
    $local = $env:LOCALAPPDATA
    if (-not $local) { $local = Join-Path $env:USERPROFILE "AppData\Local" }
    return (Join-Path $local "Gregg")
}

# ── Helpers ────────────────────────────────────────────────────────────────

function Test-OnPath {
    param([string]$Dir)
    $pathEntries = $env:PATH -split ';'
    foreach ($e in $pathEntries) {
        if ($e.TrimEnd('\') -eq $Dir.TrimEnd('\')) { return $true }
    }
    return $false
}

function Invoke-CargoFallback {
    param([string]$Program)

    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cargo) {
        Write-Error "No prebuilt $Program asset for $($Target ? $Target : 'unknown') ($Arch) and Cargo is not installed."
        Write-Error "Install Rust from https://rustup.rs, then rerun:"
        if ($StrippedVersion) {
            Write-Error "  cargo install $Program --version `"=$StrippedVersion`" --locked"
        } else {
            Write-Error "  cargo install $Program --locked"
        }
        Write-Error "Or download a matching asset manually from https://github.com/$Repo/releases"
        if ($Target -eq "aarch64-pc-windows-msvc") {
            Write-Error "Windows ARM64 is source-build only in this phase."
        }
        throw "Cargo not available for fallback"
    }

    $destDir = Get-DestDir -Program $Program -IsAdmin $IsAdmin
    # cargo --root expects the prefix (parent of bin)
    $cargoRoot = Split-Path $destDir -Parent
    if (-not $cargoRoot -or $cargoRoot -eq "") {
        # Fallback: use destDir itself as root when it has no parent (should not happen)
        $cargoRoot = $destDir
    }
    # For %ProgramFiles%\Gregg, cargoRoot would be %ProgramFiles%; that's not ideal for cargo's layout.
    # In that case, just use the default cargo install location and copy.
    $useDirectCopy = $false
    if ($IsAdmin -and $destDir -like "*Program Files*") {
        $useDirectCopy = $true
        $cargoRoot = $null
    }

    $cargoArgs = @("install", "--locked")
    if ($StrippedVersion) {
        $cargoArgs += @("--version", "=$StrippedVersion")
    }
    if (-not $useDirectCopy) {
        $cargoArgs += @("--root", $cargoRoot)
    }
    $cargoArgs += $Program

    Write-Host "No prebuilt $Program asset for $($Target ? $Target : 'unknown'); building from source: cargo $($cargoArgs -join ' ')"
    $proc = Start-Process -FilePath "cargo" -ArgumentList $cargoArgs -NoNewWindow -Wait -PassThru
    if ($proc.ExitCode -ne 0) { throw "Cargo fallback failed for $Program (exit $($proc.ExitCode))" }

    if ($useDirectCopy) {
        $installed = Join-Path $destDir "$Program.exe"
        # cargo installed to default %USERPROFILE%\.cargo\bin; copy to dest if needed
        $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin\$Program.exe"
        if (-not (Test-Path $cargoBin)) {
            # Also check $env:CARGO_HOME
            if ($env:CARGO_HOME) { $cargoBin = Join-Path $env:CARGO_HOME "bin\$Program.exe" }
        }
        if (Test-Path $cargoBin) {
            New-Item -ItemType Directory -Path $destDir -Force | Out-Null
            Copy-Item -Path $cargoBin -Destination $installed -Force
        } else {
            throw "cargo install succeeded but $cargoBin not found"
        }
    } else {
        $installed = Join-Path $destDir "$Program.exe"
        # When using --root, binary is at $cargoRoot\bin\$Program.exe which may already be $installed
        # If destDir is LOCALAPPDATA\Gregg, cargoRoot is LOCALAPPDATA, so bin is LOCALAPPDATA\bin\gregg.exe not LOCALAPPDATA\Gregg\gregg.exe
        # Prefer the actual cargo output location, then copy to expected destDir.
        $cargoBin = Join-Path $cargoRoot "bin\$Program.exe"
        if ((Test-Path $cargoBin) -and ($cargoBin -ne $installed)) {
            New-Item -ItemType Directory -Path $destDir -Force | Out-Null
            Copy-Item -Path $cargoBin -Destination $installed -Force
        }
        if (-not (Test-Path $installed)) {
            if (Test-Path $cargoBin) { $installed = $cargoBin } else { throw "cargo install succeeded but $installed not found" }
        }
    }

    # Verify version
    $out = & $installed version 2>&1
    if ($LASTEXITCODE -ne 0) { throw "installed $Program failed version check: $out" }
    if (-not $out.StartsWith("$Program ")) { throw "candidate version output does not start with '$Program ': $out" }
    $verPart = ($out -split ' ')[1]
    if ($StrippedVersion -and $verPart -ne $StrippedVersion) { throw "candidate version $verPart != requested $StrippedVersion" }
    Write-Host "Verified candidate: $out"
    Write-Host "$Program installed via Cargo to $installed"

    if (-not (Test-OnPath $destDir)) {
        Write-Host "note: $destDir is not in PATH; add it to your PATH or use the full path." -ForegroundColor Yellow
    }
}

function Install-Program {
    param([string]$Program)

    # Source-only hosts go directly to Cargo fallback
    if (-not $Target -or -not $SupportedBinary) {
        Write-Host "Host $Arch ($($Target ? $Target : 'unknown')) has no prebuilt $Program asset; trying Cargo fallback..."
        Invoke-CargoFallback -Program $Program
        return
    }

    $Asset = "$Program-$Target.exe"
    if ($Tag) {
        $Url = "$BaseUrl/download/$Tag/$Asset"
    } else {
        $Url = "$BaseUrl/latest/download/$Asset"
    }
    $ShaUrl = "$Url.sha256"

    if ($Url -notlike "https://github.com/$Repo/releases/*") {
        throw "constructed URL is not under expected repo: $Url"
    }

    $Tmp = Join-Path $env:TEMP "gregg-install-$(Get-Random)-$Program"
    New-Item -ItemType Directory -Path $Tmp -Force | Out-Null
    try {
        $Candidate = Join-Path $Tmp $Asset
        $ShaFile = Join-Path $Tmp "$Asset.sha256"

        Write-Host "Downloading $Program from $Url ..."
        $downloadSucceeded = $true
        $httpCode = ""
        try {
            Invoke-WebRequest -Uri $Url -OutFile $Candidate -UseBasicParsing -ErrorAction Stop
        } catch {
            $downloadSucceeded = $false
            # Try to extract HTTP status
            $resp = $_.Exception.Response
            if ($resp -and $resp.StatusCode) {
                $httpCode = [int]$resp.StatusCode
            } else {
                # Fallback: probe with a HEAD
                try {
                    $probe = Invoke-WebRequest -Uri $Url -Method Head -UseBasicParsing -ErrorAction Stop
                    $httpCode = $probe.StatusCode
                } catch {
                    if ($_.Exception.Response) { $httpCode = [int]$_.Exception.Response.StatusCode } else { $httpCode = "000" }
                }
            }
            if ($httpCode -eq 404) {
                Write-Host "No prebuilt $Program asset at $Url (HTTP 404); trying Cargo fallback..."
                Invoke-CargoFallback -Program $Program
                return
            } else {
                throw "failed to download $Asset from $Url (HTTP $httpCode): $($_.Exception.Message)"
            }
        }

        if (-not $downloadSucceeded) { return }

        Write-Host "Downloading checksum $ShaUrl ..."
        try {
            Invoke-WebRequest -Uri $ShaUrl -OutFile $ShaFile -UseBasicParsing -ErrorAction Stop
        } catch {
            throw "failed to download checksum for $Asset from $ShaUrl : $($_.Exception.Message)"
        }

        # Verify SHA-256 before execution
        $Expected = ((Get-Content -LiteralPath $ShaFile -Raw).Split()[0]).Trim().ToLower()
        if (-not $Expected -or $Expected.Length -ne 64) {
            throw "checksum file is empty or malformed: $ShaFile"
        }
        $Actual = (Get-FileHash -Algorithm SHA256 -Path $Candidate).Hash.ToLower()
        if ($Expected -ne $Actual) {
            Write-Error "expected: $Expected"
            Write-Error "actual:   $Actual"
            throw "SHA-256 mismatch for $Asset"
        }
        Write-Host "Checksum OK: $Actual"

        # Verify candidate version before install
        $out = & $Candidate version 2>&1
        if ($LASTEXITCODE -ne 0) { throw "candidate $Program failed 'version' check: $out" }
        if (-not $out.StartsWith("$Program ")) { throw "candidate version output does not start with '$Program ': $out" }
        $verPart = ($out -split ' ')[1]
        if ($StrippedVersion -and $verPart -ne $StrippedVersion) { throw "candidate version $verPart != requested $StrippedVersion (output: $out)" }
        Write-Host "Verified candidate: $out"

        # Do not install unverified partial download — verification done

        $DestDir = Get-DestDir -Program $Program -IsAdmin $IsAdmin
        New-Item -ItemType Directory -Path $DestDir -Force | Out-Null
        $DestPath = Join-Path $DestDir "$Program.exe"

        # For greggd, preserve existing config and handle SCM registration
        if ($Program -eq "greggd") {
            $ProgramDataDir = Join-Path $env:ProgramData "gregg"
            $DefaultConfigPath = Join-Path $ProgramDataDir "greggd.toml"
            if (-not (Test-Path $ProgramDataDir)) {
                New-Item -ItemType Directory -Path $ProgramDataDir -Force | Out-Null
            }
            if (-not (Test-Path $DefaultConfigPath)) {
                $DefaultConfig = @"
name = "greggd"
host = "0.0.0.0"
port = 11310
sample_interval_ms = 1000
stale_after_ms = 10000
"@
                [System.IO.File]::WriteAllText($DefaultConfigPath, $DefaultConfig)
                Write-Host "Created default config: $DefaultConfigPath"
            } else {
                Write-Host "Existing config preserved: $DefaultConfigPath"
            }
        }

        # Install binary (overwrite)
        # If greggd is running as a service, stop it first when Administrator
        $serviceWasRunning = $false
        if ($Program -eq "greggd" -and $IsAdmin) {
            $svc = Get-Service -Name greggd -ErrorAction SilentlyContinue
            if ($svc -and $svc.Status -eq 'Running') {
                Write-Host "Stopping existing greggd service..."
                Stop-Service -Name greggd -Force -ErrorAction SilentlyContinue
                $svc.WaitForStatus("Stopped", (New-TimeSpan -Seconds 30)) | Out-Null
                $serviceWasRunning = $true
            }
        }

        Copy-Item -Path $Candidate -Destination $DestPath -Force
        Write-Host "Installed $Program to $DestPath"

        # Register / update SCM service for greggd when Administrator
        if ($Program -eq "greggd" -and $IsAdmin) {
            $InstalledExe = $DestPath
            $ProgramDataDir = Join-Path $env:ProgramData "gregg"
            $DefaultConfigPath = Join-Path $ProgramDataDir "greggd.toml"
            $ImagePath = "`"$InstalledExe`" service --config `"$DefaultConfigPath`""
            $svc = Get-Service -Name greggd -ErrorAction SilentlyContinue
            if ($svc) {
                Write-Host "Updating service registration..."
                sc.exe config greggd binPath= $ImagePath start= auto | Out-Null
                if ($LASTEXITCODE -ne 0) { throw "sc.exe config failed" }
            } else {
                Write-Host "Registering service..."
                sc.exe create greggd binPath= $ImagePath start= auto DisplayName= "Gregg Metrics Daemon" | Out-Null
                if ($LASTEXITCODE -ne 0) { throw "sc.exe create failed" }
            }
            sc.exe config greggd obj= "NT AUTHORITY\LocalService" | Out-Null
            sc.exe failure greggd reset= 86400 actions= restart/60000/restart/60000/restart/60000 | Out-Null
            if ($serviceWasRunning -or $svc) {
                Write-Host "Starting service..."
                Start-Service -Name greggd -ErrorAction SilentlyContinue
                $svc2 = Get-Service -Name greggd
                $svc2.WaitForStatus("Running", (New-TimeSpan -Seconds 30)) | Out-Null
                Write-Host "Service running"
            } else {
                Write-Host "Starting service..."
                Start-Service -Name greggd
                $svc2 = Get-Service -Name greggd
                $svc2.WaitForStatus("Running", (New-TimeSpan -Seconds 30)) | Out-Null
            }
            Write-Host "Service: greggd (Gregg Metrics Daemon) Automatic, LocalService"
        } elseif ($Program -eq "greggd" -and -not $IsAdmin) {
            Write-Host "note: greggd installed to $DestDir\$Program.exe (user-local)." -ForegroundColor Yellow
            Write-Host "For a system service, rerun as Administrator:" -ForegroundColor Yellow
            if ($Tag) {
                Write-Host "  irm https://github.com/$Repo/releases/download/$Tag/install.ps1 | iex  # then -Component Greggd" -ForegroundColor Yellow
            } else {
                Write-Host "  irm https://github.com/$Repo/releases/latest/download/install.ps1 | iex  # then -Component Greggd" -ForegroundColor Yellow
            }
            Write-Host "No service was registered; Plan 100 will refine startup registration." -ForegroundColor Yellow
        }

        if (-not (Test-OnPath $DestDir)) {
            Write-Host "note: $DestDir is not in PATH; add it to your PATH or use the full path." -ForegroundColor Yellow
        }
    } finally {
        if (Test-Path $Tmp) { Remove-Item -LiteralPath $Tmp -Recurse -Force -ErrorAction SilentlyContinue }
    }
}

# ── Main ───────────────────────────────────────────────────────────────────

switch ($ComponentNorm) {
    "gregg"  { Install-Program -Program "gregg" }
    "greggd" { Install-Program -Program "greggd" }
    "both"   {
        Install-Program -Program "gregg"
        Install-Program -Program "greggd"
    }
}

Write-Host "Done. Installed $ComponentNorm."
if ($ComponentNorm -eq "both" -or $ComponentNorm -eq "gregg") {
    $dest = Get-DestDir -Program "gregg" -IsAdmin $IsAdmin
    $p = Join-Path $dest "gregg.exe"
    if (Test-Path $p) { $v = & $p version 2>&1; Write-Host "  $p version => $v" }
}
if ($ComponentNorm -eq "both" -or $ComponentNorm -eq "greggd") {
    $dest = Get-DestDir -Program "greggd" -IsAdmin $IsAdmin
    $p = Join-Path $dest "greggd.exe"
    if (Test-Path $p) { $v = & $p version 2>&1; Write-Host "  $p version => $v" }
}
