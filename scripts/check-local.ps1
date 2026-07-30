# check-local.ps1 — local validation entry point for gregg (Windows).
#
# Tiers:
#   (default)   Fast developer check: fmt, clippy, test, doc, deny.
#   -Full       Full local check: adds shellcheck, python tests, package checks.
#   -Release    Release preflight: adds clean-tree, version consistency, package list.
#
# Usage:
#   .\scripts\check-local.ps1
#   .\scripts\check-local.ps1 -Full
#   .\scripts\check-local.ps1 -Release
#   .\scripts\check-local.ps1 -SkipDeny

[CmdletBinding()]
param(
    [switch]$Full,
    [switch]$Release,
    [switch]$SkipDeny
)

$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Split-Path -Parent $ScriptDir

if ($Release) {
    $Mode = 'release'
} elseif ($Full) {
    $Mode = 'full'
} else {
    $Mode = 'default'
}

Push-Location $RepoRoot

$script:CURRENT_STEP = ''
$script:FAILED = $false

function Write-Step {
    param([string]$Message)
    $script:CURRENT_STEP = $Message
    Write-Host "==> $Message"
}

function Invoke-OrFail {
    param([scriptblock]$Command)
    & $Command
    if ($LASTEXITCODE -ne 0 -and $null -ne $LASTEXITCODE) {
        Write-Error "local check failed: $($script:CURRENT_STEP)"
        exit 1
    }
}

function Test-CommandAvailable {
    param([string]$Name)
    $null -ne (Get-Command $Name -ErrorAction SilentlyContinue)
}

function Get-WorkspaceVersion {
    $manifestPath = Join-Path $RepoRoot 'Cargo.toml'
    $inPackage = $false
    foreach ($line in Get-Content -LiteralPath $manifestPath) {
        if ($line -match '^\s*\[workspace\.package\]\s*$') {
            $inPackage = $true
            continue
        }
        if ($inPackage -and $line -match '^\s*\[') {
            break
        }
        if ($inPackage -and $line -match '^\s*version\s*=') {
            $match = [regex]::Match($line, '^\s*version\s*=\s*"([^"]+)"')
            if (-not $match.Success) {
                throw "error: $manifestPath has a malformed workspace version"
            }
            return $match.Groups[1].Value
        }
    }
    throw "error: $manifestPath has no version in [workspace.package]"
}

function Test-VersionConsistency {
    $workspaceVersion = Get-WorkspaceVersion
    $crates = @('crates/gregg-protocol', 'crates/greggd', 'crates/gregg')
    foreach ($crate in $crates) {
        $manifest = Join-Path $RepoRoot "$crate/Cargo.toml"
        $inheritance = @(Select-String -LiteralPath $manifest -Pattern '^\s*version\.workspace\s*=\s*true\s*$')
        if ($inheritance.Count -eq 0) {
            throw "error: $manifest is missing version.workspace = true"
        }
    }

    foreach ($crate in @('crates/greggd', 'crates/gregg')) {
        $manifest = Join-Path $RepoRoot "$crate/Cargo.toml"
        $dependencyLines = @(Get-Content -LiteralPath $manifest | Where-Object {
                $_ -match '^\s*gregg-protocol\s*='
            })
        if ($dependencyLines.Count -eq 0) {
            throw "error: $manifest has no gregg-protocol dependency declaration"
        }
        foreach ($line in $dependencyLines) {
            $match = [regex]::Match([string]$line, 'version\s*=\s*"([^"]+)"')
            if (-not $match.Success) {
                throw "error: $manifest gregg-protocol dependency has no registry version"
            }
            $dependencyVersion = $match.Groups[1].Value
            if ($dependencyVersion -ne $workspaceVersion) {
                throw "error: $manifest gregg-protocol dependency version $dependencyVersion != workspace $workspaceVersion"
            }
        }
    }

    Write-Host "  workspace version $workspaceVersion; all members inherit it and gregg-protocol constraints match"
}

function Get-FreeLoopbackPort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    try {
        $listener.Start()
        return [int]$listener.LocalEndpoint.Port
    } finally {
        $listener.Stop()
    }
}

function Invoke-InstalledDaemonSmoke {
    param([string]$BinaryPath)

    $tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
    $process = $null
    try {
        New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
        $port = Get-FreeLoopbackPort
        $configPath = Join-Path $tempDir 'greggd.toml'
        $stdoutPath = Join-Path $tempDir 'greggd.stdout.log'
        $stderrPath = Join-Path $tempDir 'greggd.stderr.log'
        $config = @"
name = "loopback-test"
host = "127.0.0.1"
port = $port
sample_interval_ms = 250
stale_after_ms = 10000
"@
        [System.IO.File]::WriteAllText($configPath, $config)
        $arguments = @('--config', "`"$configPath`"", 'run')
        $process = Start-Process -FilePath $BinaryPath -ArgumentList $arguments -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath

        $ready = $false
        $deadline = (Get-Date).AddSeconds(15)
        while ((Get-Date) -lt $deadline) {
            if ($process.HasExited) {
                throw "installed greggd exited during startup"
            }
            try {
                $health = Invoke-RestMethod -Uri "http://127.0.0.1:$port/v2/healthz" -TimeoutSec 2
                if ($health.state -eq 'ready') {
                    $ready = $true
                    break
                }
            } catch {
            }
            Start-Sleep -Milliseconds 200
        }
        if (-not $ready) {
            throw "installed greggd did not become ready within 15 seconds"
        }

        $status = Invoke-RestMethod -Uri "http://127.0.0.1:$port/v2/status" -TimeoutSec 2
        if ([int]$status.schema_version -ne 2) {
            throw "installed greggd returned an unexpected v2 schema version"
        }
        if ([string]::IsNullOrWhiteSpace([string]$status.system.name)) {
            throw "installed greggd returned an empty system name"
        }
        Write-Host "  installed greggd v2 loopback smoke passed on port $port"
    } finally {
        if ($null -ne $process) {
            $process.Refresh()
            if (-not $process.HasExited) {
                Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
                $process.WaitForExit(5000)
            }
            if (-not $process.HasExited) {
                throw "installed greggd process did not terminate"
            }
        }
        if (Test-Path -LiteralPath $tempDir) {
            Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
        }
        if (Test-Path -LiteralPath $tempDir) {
            throw "temporary installed-daemon smoke directory could not be removed"
        }
    }
}

Write-Step "cargo fmt --all -- --check"
Invoke-OrFail { cargo fmt --all -- --check }

Write-Step "cargo clippy --workspace --all-targets --all-features -- -D warnings"
Invoke-OrFail { cargo clippy --workspace --all-targets --all-features -- -D warnings }

Write-Step "cargo test --workspace --all-targets --all-features"
Invoke-OrFail { cargo test --workspace --all-targets --all-features }

Write-Step "cargo doc --workspace --no-deps"
Invoke-OrFail { cargo doc --workspace --no-deps }

if (-not $SkipDeny) {
    if (Test-CommandAvailable 'cargo-deny') {
        Write-Step "cargo deny check"
        Invoke-OrFail { cargo deny check }
    } else {
        Write-Host "cargo-deny not installed, skipping (install with: cargo install cargo-deny)"
    }
}

# Platform-native collector tests
if ($IsWindows -or $env:OS -eq 'Windows_NT') {
    Write-Step "native Windows collector tests"
    Invoke-OrFail { cargo test -p greggd --all-features -- collector::windows }
} elseif ($IsLinux) {
    Write-Step "native Linux collector tests"
    Invoke-OrFail { cargo test -p greggd --all-features -- collector::linux }
} elseif ($IsMacOS) {
    Write-Step "native macOS collector tests"
    Invoke-OrFail { cargo test -p greggd --all-features -- collector::macos::ffi::native_tests }
} else {
    Write-Host "  skipping platform-native collector tests"
}

# ── Tier 2 (-Full): full local check ────────────────────────────────────────

if ($Mode -eq 'full' -or $Mode -eq 'release') {

    # Shell syntax checks
    if (Test-CommandAvailable 'shellcheck') {
        Write-Step "shellcheck packaging/install scripts"
        Invoke-OrFail { shellcheck packaging/install-linux.sh packaging/install-macos.sh }
    } else {
        Write-Host "shellcheck not installed, skipping"
    }

    # Python tests
    if (Test-CommandAvailable 'python3') {
        Write-Step "python3 tests for scripts"
        Invoke-OrFail { python3 -m pytest scripts/tests/ -v --tb=short }
    } elseif (Test-CommandAvailable 'python') {
        Write-Step "python tests for scripts"
        Invoke-OrFail { python -m pytest scripts/tests/ -v --tb=short }
    } else {
        Write-Host "python3 not installed, skipping python tests"
    }

    # Package content check (no publish)
    Write-Step "cargo package --list (gregg-protocol)"
    Invoke-OrFail { cargo package --list -p gregg-protocol }

    Write-Step "cargo package --list (greggd)"
    Invoke-OrFail { cargo package --list -p greggd }

    Write-Step "cargo package --list (gregg)"
    Invoke-OrFail { cargo package --list -p gregg }

    # Install binary loopback smoke
    Write-Step "installed-binary loopback smoke"
    $TempInstallDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
    try {
        Invoke-OrFail { cargo install --path crates/greggd --locked --root $TempInstallDir --debug }
        $GreggdBin = Join-Path $TempInstallDir 'bin' 'greggd.exe'
        if (-not (Test-Path -LiteralPath $GreggdBin -PathType Leaf)) {
            throw "installed greggd binary was not created at $GreggdBin"
        }
        Invoke-OrFail { & $GreggdBin --version }
        Invoke-OrFail { & $GreggdBin --help }
        Invoke-InstalledDaemonSmoke -BinaryPath $GreggdBin
    } finally {
        if (Test-Path -LiteralPath $TempInstallDir) {
            Remove-Item -LiteralPath $TempInstallDir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

# ── Tier 3 (-Release): release preflight ────────────────────────────────────

if ($Mode -eq 'release') {

    Write-Step "clean-tree check"
    $status = git status --porcelain
    if ($status) {
        Write-Error "working tree is not clean"
        git status --short
        exit 1
    }

    Write-Step "version consistency check"
    Test-VersionConsistency

    Write-Step "cargo publish -p gregg-protocol --dry-run --locked"
    Invoke-OrFail { cargo publish -p gregg-protocol --dry-run --locked }
}

Pop-Location

Write-Host ""
Write-Host "=== all checks passed (tier: $Mode) ==="
