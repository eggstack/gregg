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

# ── Tier 1 (default): fast developer check ──────────────────────────────────

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
        Invoke-OrFail { cargo install greggd --root $TempInstallDir --debug }
        $GreggdBin = Join-Path $TempInstallDir 'bin' 'greggd.exe'
        if (Test-Path $GreggdBin) {
            & $GreggdBin run --help 2>$null
        }
    } finally {
        if (Test-Path $TempInstallDir) {
            Remove-Item -Recurse -Force $TempInstallDir -ErrorAction SilentlyContinue
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
    $workspaceVersion = (Get-Content Cargo.toml | Select-String '^\s*version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
    foreach ($crate in @('crates/gregg-protocol', 'crates/greggd', 'crates/gregg')) {
        $crateVersion = (Get-Content "$crate/Cargo.toml" | Select-String '^\s*version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
        if ($crateVersion -ne $workspaceVersion) {
            Write-Error "$crate version $crateVersion != workspace $workspaceVersion"
            exit 1
        }
    }
    Write-Host "  all crate versions match workspace version $workspaceVersion"

    Write-Step "cargo publish --dry-run (gregg-protocol)"
    Invoke-OrFail { cargo publish --dry-run -p gregg-protocol }

    Write-Step "cargo publish --dry-run (greggd)"
    Invoke-OrFail { cargo publish --dry-run -p greggd }

    Write-Step "cargo publish --dry-run (gregg)"
    Invoke-OrFail { cargo publish --dry-run -p gregg }
}

Pop-Location

Write-Host ""
Write-Host "=== all checks passed (tier: $Mode) ==="
