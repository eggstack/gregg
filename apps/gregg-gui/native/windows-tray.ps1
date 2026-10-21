# The Windows half of the GUI's native system-tray exercise.
#
# What this script asserts, and what it deliberately does NOT:
#
#   ASSERTED  - the CORE client-daemon launch works on Windows: `gregg daemon
#             restart` takes the launch lock, spawns a daemon and waits for it
#             to answer, `gregg daemon status` identifies it through the
#             daemon's own local protocol handshake, and `gregg daemon stop`
#             releases it;
#             - the GUI starts and prints its `tray=` startup line;
#             - the GUI created a NATIVE top-level window, proved by
#               enumerating the process's windows with EnumWindows;
#             - the GUI process is still alive afterwards;
#             - the per-config client daemon the GUI launched is running,
#               identified through the daemon's own local protocol handshake
#               (`gregg daemon status`), not through a process name;
#             - the fleet fixture served a request, so the daemon really
#               polls the remote.
#
#   NOT ASSERTED - that a notification-area (system tray) ICON was rendered.
#             The GitHub-hosted Windows runner executes the agent as a service
#             in session 0: there is no interactive desktop, no explorer.exe
#             shell, and therefore no notification area to put an icon in.
#             Shell_NotifyIconW can succeed or fail there depending on the
#             runner image, and neither outcome is observable by a human or by
#             this script. The script therefore reports WHICH `tray=` value
#             the GUI printed and asserts only that the tray start did not
#             take the process down. A green run here is NOT evidence that the
#             tray icon appears in the notification area of a real desktop; it
#             is evidence that the tray start path runs without killing the
#             application and that a native window was really created.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $GuiPath,

    [Parameter(Mandatory = $true)]
    [string] $GreggPath,

    [string] $Python = "python",

    [string] $FixtureScript,

    [int] $TimeoutSeconds = 90
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Write-Log {
    param([string] $Message)
    Write-Host "windows-tray: $Message"
}

function Fail {
    param([string] $Message)
    Write-Host "windows-tray: FAIL $Message"
    exit 1
}

# Reads a file that another process is still writing to.
function Read-SharedFile {
    param([string] $Path)
    if (-not (Test-Path -LiteralPath $Path)) {
        return ""
    }
    try {
        $stream = [System.IO.File]::Open(
            $Path,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]::ReadWrite)
        try {
            $reader = New-Object System.IO.StreamReader($stream)
            try {
                return $reader.ReadToEnd()
            } finally {
                $reader.Dispose()
            }
        } finally {
            $stream.Dispose()
        }
    } catch {
        return ""
    }
}

# The native window enumeration. This is the whole point of the exercise:
# it proves the GUI created a real Win32 top-level window, not that it merely
# said it did.
if (-not ("GreggGui.NativeWindows" -as [type])) {
    Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

namespace GreggGui {
    public static class NativeWindows {
        public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

        [DllImport("user32.dll")]
        public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

        [DllImport("user32.dll")]
        public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);

        [DllImport("user32.dll")]
        public static extern int GetWindowTextLength(IntPtr hWnd);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        public static extern int GetWindowText(IntPtr hWnd, StringBuilder lpString, int nMaxCount);

        [DllImport("user32.dll")]
        public static extern bool IsWindowVisible(IntPtr hWnd);

        /// Every top-level window owned by the given process.
        public static List<string> TopLevelWindowsForProcess(int processId) {
            List<string> found = new List<string>();
            uint wanted = (uint)processId;
            EnumWindows(delegate(IntPtr hWnd, IntPtr lParam) {
                uint owner;
                GetWindowThreadProcessId(hWnd, out owner);
                if (owner == wanted) {
                    int length = GetWindowTextLength(hWnd);
                    StringBuilder text = new StringBuilder(length + 1);
                    GetWindowText(hWnd, text, text.Capacity);
                    string visibility = IsWindowVisible(hWnd) ? "visible" : "hidden";
                    found.Add(hWnd.ToInt64().ToString() + "|" + visibility + "|" + text.ToString());
                }
                return true;
            }, IntPtr.Zero);
            return found;
        }
    }
}
"@
}

# ------------------------------------------------------------------ preflight
if (-not (Test-Path -LiteralPath $GuiPath)) {
    Fail "the GUI executable $GuiPath does not exist"
}
if (-not (Test-Path -LiteralPath $GreggPath)) {
    Fail "the gregg executable $GreggPath does not exist"
}
$pythonCommand = Get-Command $Python -ErrorAction SilentlyContinue
if ($null -eq $pythonCommand) {
    Fail "no $Python on PATH for the fleet fixture"
}

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
if ([string]::IsNullOrWhiteSpace($FixtureScript)) {
    $FixtureScript = Join-Path $scriptRoot "..\..\..\scripts\tests\fleet-fixture.py"
}
if (-not (Test-Path -LiteralPath $FixtureScript)) {
    Fail "the fleet fixture $FixtureScript does not exist"
}

$workDir = Join-Path ([System.IO.Path]::GetTempPath()) ("gregg-gui-tray-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $workDir -Force | Out-Null

$guiProcess = $null
$fixtureProcess = $null

function Stop-IfRunning {
    param($Process)
    if ($null -eq $Process) {
        return
    }
    try {
        if (-not $Process.HasExited) {
            Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
            $Process.WaitForExit(10000) | Out-Null
        }
    } catch {
        # Cleanup is best effort: the process may already be gone.
    }
}

try {
    # ------------------------------------------------------------- the fixture
    Write-Log "starting the fleet fixture"
    $portFile = Join-Path $workDir "fixture.port"
    $errFile = Join-Path $workDir "fixture.err"
    $fixtureProcess = Start-Process -FilePath $Python `
        -ArgumentList @(
            $FixtureScript,
            "--port", "0",
            "--log", (Join-Path $workDir "fixture.log"),
            "--default-mode", "healthy"
        ) `
        -RedirectStandardOutput $portFile `
        -RedirectStandardError $errFile `
        -NoNewWindow -PassThru

    $port = ""
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        if ($fixtureProcess.HasExited) {
            Write-Host (Read-SharedFile $errFile)
            Fail "the fleet fixture exited before it reported a port"
        }
        $text = Read-SharedFile $portFile
        foreach ($line in ($text -split "`r?`n")) {
            if ($line -like "PORT=*") {
                $port = $line.Substring(5).Trim()
                break
            }
        }
        if ($port -ne "") {
            break
        }
        Start-Sleep -Milliseconds 200
    }
    if ($port -eq "") {
        Fail "the fleet fixture never reported a port"
    }
    Write-Log "the fleet fixture is listening on 127.0.0.1:$port"

    # -------------------------------------------------------------- the config
    $configPath = Join-Path $workDir "gregg.toml"
    $systemId = [guid]::NewGuid().ToString()
    @(
        "config_version = 1"
        "refresh_seconds = 5"
        "request_timeout_ms = 1500"
        "max_concurrent_requests = 16"
        "default_port = 8123"
        ""
        "[[systems]]"
        "id = `"$systemId`""
        "host = `"127.0.0.1`""
        "port = $port"
        "name = `"gui-fixture`""
    ) | Set-Content -LiteralPath $configPath -Encoding UTF8
    Write-Log "wrote $configPath"

    # ------------------------------------------------- the daemon, standalone
    #
    # A diagnostic, and it is here because the GUI's own launcher starts the
    # daemon with every standard stream null: if the daemon dies, it dies
    # silently and the GUI can only say "it did not become ready". Running
    # the same daemon in the foreground for a few seconds separates the two
    # possibilities that matter:
    #
    #   * the daemon exits on its own -> the daemon itself is unhappy on this
    #     platform, and its own message says why;
    #   * the daemon keeps running   -> the daemon is fine, and whatever is
    #     wrong is in how the GUI starts it.
    Write-Log "starting the client daemon in the foreground to see what it says"
    $daemonOut = Join-Path $workDir "daemon.stdout"
    $daemonErr = Join-Path $workDir "daemon.stderr"
    $daemonProcess = Start-Process -FilePath $GreggPath `
        -ArgumentList @("--config", $configPath, "daemon", "run") `
        -RedirectStandardOutput $daemonOut `
        -RedirectStandardError $daemonErr `
        -NoNewWindow -PassThru
    $daemonDeadline = (Get-Date).AddSeconds(10)
    while ((Get-Date) -lt $daemonDeadline -and -not $daemonProcess.HasExited) {
        Start-Sleep -Milliseconds 250
    }
    if ($daemonProcess.HasExited) {
        Write-Log "the daemon exited on its own with code $($daemonProcess.ExitCode):"
        Write-Host (Read-SharedFile $daemonOut)
        Write-Host (Read-SharedFile $daemonErr)
    } else {
        Write-Log "the daemon runs in the foreground (it was still running after 10s)"
        Stop-IfRunning $daemonProcess
    }

    # ------------------------------------------------- the core launch, on Windows
    #
    # The GUI is not the only frontend: the terminal client launches the same
    # per-config client daemon, through the same launch lock, and it launches
    # it from its own executable rather than from a named one. So the core
    # launch path is proved here on its own, with the CLI, *before* the GUI is
    # started at all:
    #
    #   * `gregg daemon restart` takes the launch lock, spawns a daemon and
    #     waits for it to answer — the exact path a terminal-client launch
    #     takes, and the path that was broken when the lock was derived from
    #     the Windows named pipe;
    #   * `gregg daemon status` then identifies the daemon through its own
    #     local protocol handshake, never through a process name;
    #   * `gregg daemon stop` releases it again.
    #
    # The daemon is stopped before the GUI starts, so the GUI that follows
    # finds a genuinely absent endpoint and has to launch a daemon of its own.
    # That keeps the "the fixture served a request" check later a proof about
    # the GUI's own daemon rather than about this one.
    Write-Log "proving the core client-daemon launch path"
    $daemonPattern = 'client daemon \S+ running \(protocol'
    # The output of this one command is redirected to a FILE, not piped.
    #
    # `gregg daemon restart` spawns the daemon detached, and on Windows a
    # detached child inherits the launcher's inheritable handles — including
    # the write end of a PowerShell pipeline. The daemon would then hold that
    # pipe open for as long as it lives and the pipeline would never see EOF,
    # so the script would block on a command that had already finished. A file
    # has no such semantics: the redirection ends when the process exits.
    #
    # This is a real defect in the launcher, not a quirk of the exercise: it
    # is recorded in `crates/gregg/src/clientd/launch.rs` and in
    # `apps/gregg-gui/EVIDENCE.md`. The GUI is not affected, because its
    # standard streams are a console or a file rather than a pipe.
    $restartOut = Join-Path $workDir "restart.out"
    & $GreggPath --config $configPath daemon restart *> $restartOut
    $restartExit = $LASTEXITCODE
    $restartText = [string](Get-Content -LiteralPath $restartOut -Raw -ErrorAction SilentlyContinue)
    Write-Log "daemon restart: $($restartText.Trim())"
    if ($restartExit -ne 0) {
        Fail "the core client-daemon launch failed on Windows: $($restartText.Trim())"
    }
    $coreStatus = & $GreggPath --config $configPath daemon status 2>&1 | Out-String
    Write-Log "daemon status after the core launch: $($coreStatus.Trim())"
    if ($coreStatus -notmatch $daemonPattern) {
        Fail "the core launch did not leave a running client daemon: $($coreStatus.Trim())"
    }
    $stopOutput = & $GreggPath --config $configPath daemon stop 2>&1 | Out-String
    Write-Log "daemon stop: $($stopOutput.Trim())"
    if ($LASTEXITCODE -ne 0) {
        Fail "the client daemon did not stop on Windows: $($stopOutput.Trim())"
    }
    $afterStop = & $GreggPath --config $configPath daemon status 2>&1 | Out-String
    if ($afterStop -notmatch 'client daemon \S+ not running') {
        Fail "the client daemon is still running after a stop: $($afterStop.Trim())"
    }
    Write-Log "the core launch, the status handshake and the stop all work on Windows"

    # ---------------------------------------------------------------- the GUI
    $stdoutFile = Join-Path $workDir "gui.stdout"
    $stderrFile = Join-Path $workDir "gui.stderr"
    Write-Log "starting the GUI"
    $guiProcess = Start-Process -FilePath $GuiPath `
        -ArgumentList @("--config", $configPath, "--gregg", $GreggPath) `
        -RedirectStandardOutput $stdoutFile `
        -RedirectStandardError $stderrFile `
        -NoNewWindow -PassThru

    # The GUI's own startup line about its tray support.
    $trayLine = ""
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        $combined = (Read-SharedFile $stdoutFile) + "`n" + (Read-SharedFile $stderrFile)
        foreach ($line in ($combined -split "`r?`n")) {
            if ($line -like "gregg-gui: tray=*") {
                $trayLine = $line.Trim()
                break
            }
        }
        if ($trayLine -ne "") {
            break
        }
        if ($guiProcess.HasExited) {
            Write-Host $combined
            Fail "the GUI exited with code $($guiProcess.ExitCode) before printing its tray line"
        }
        Start-Sleep -Milliseconds 250
    }
    if ($trayLine -eq "") {
        Write-Host (Read-SharedFile $stdoutFile)
        Write-Host (Read-SharedFile $stderrFile)
        Fail "the GUI never printed a `gregg-gui: tray=` line within $TimeoutSeconds seconds"
    }
    # Report which way it went. On this runner the notification-area icon
    # itself cannot be observed (no interactive shell), so the value is
    # reported, not asserted.
    Write-Log "the GUI printed: $trayLine"
    Write-Log "NOTE: the notification-area icon itself is NOT observable on this runner (session 0, no explorer.exe)."

    # ------------------------------------------------------- the native window
    $windows = @()
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        if ($guiProcess.HasExited) {
            Fail "the GUI exited with code $($guiProcess.ExitCode) before creating a window"
        }
        $windows = [GreggGui.NativeWindows]::TopLevelWindowsForProcess($guiProcess.Id)
        if ($windows.Count -gt 0) {
            break
        }
        Start-Sleep -Milliseconds 250
    }
    if ($windows.Count -eq 0) {
        Fail "the GUI created no top-level window (EnumWindows found none for process $($guiProcess.Id))"
    }
    foreach ($window in $windows) {
        Write-Log "top-level window: $window"
    }
    Write-Log "the GUI created $($windows.Count) native top-level window(s)"

    # ------------------------------------------------------- the process lives
    if ($guiProcess.HasExited) {
        Fail "the GUI exited with code $($guiProcess.ExitCode)"
    }
    Write-Log "the GUI process $($guiProcess.Id) is still running"

    # ---------------------------------------------------------- the daemon runs
    #
    # The GUI launches the client daemon for its config, so the daemon must
    # come up. Two things this check has to get right, and the first version
    # of it got both wrong:
    #
    #   * it has to WAIT. The daemon is a process the GUI spawns, so it is
    #     not listening a few hundred milliseconds after the GUI starts.
    #   * it has to match the running form and not the other one. The
    #     daemon reports "client daemon <id> not running at <endpoint>: ..."
    #     when it is down, and a pattern loose enough to match "running"
    #     inside "not running" turns a missing daemon into a PASS.
    #
    # The running form is "client daemon <id> running (protocol <n>, version
    # <v>) at <endpoint>"; the parenthesis after "running" is what tells the
    # two apart. `$daemonPattern` was defined above, where the core launch
    # check used it.
    $daemonDeadline = (Get-Date).AddSeconds(60)
    $statusOutput = ""
    $daemonRunning = $false
    while ((Get-Date) -lt $daemonDeadline) {
        $statusOutput = & $GreggPath --config $configPath daemon status 2>&1 | Out-String
        if ($statusOutput -match $daemonPattern) {
            $daemonRunning = $true
            break
        }
        if ($guiProcess.HasExited) {
            Fail "the GUI exited with code $($guiProcess.ExitCode) before the client daemon came up"
        }
        Start-Sleep -Milliseconds 500
    }
    Write-Log "daemon status: $($statusOutput.Trim())"
    if (-not $daemonRunning) {
        # The GUI's own words come first: it is the thing under test, and the
        # reason it gives ("it did not become ready") is the operator's clue.
        Write-Host "the GUI said:"
        Write-Host (Read-SharedFile $stdoutFile)
        Write-Host (Read-SharedFile $stderrFile)
        Fail "the client daemon the GUI launched is not running: $($statusOutput.Trim())"
    }
    if ($statusOutput -match 'client daemon \S+ not running') {
        Fail "the daemon status contradicts itself: $($statusOutput.Trim())"
    }
    Write-Log "the client daemon is running"

    # ------------------------------------------------- the daemon really polls
    #
    # "The daemon is running" and "the daemon is polling the fleet" are two
    # different claims, and only the second one proves the GUI is attached to
    # a working daemon. The fixture writes one JSON line per request it
    # serves, so a non-empty log is the end-to-end evidence: GUI -> client
    # daemon -> remote.
    $fixtureLog = Join-Path $workDir "fixture.log"
    $requestDeadline = (Get-Date).AddSeconds(60)
    $requestCount = 0
    while ((Get-Date) -lt $requestDeadline) {
        if (Test-Path -LiteralPath $fixtureLog) {
            $requestCount = @(Get-Content -LiteralPath $fixtureLog -ErrorAction SilentlyContinue).Count
        }
        if ($requestCount -gt 0) {
            break
        }
        Start-Sleep -Milliseconds 500
    }
    if ($requestCount -le 0) {
        Fail "the client daemon is running but the fleet fixture received no request in 60 seconds"
    }
    Write-Log "the fleet fixture served $requestCount request(s): the daemon is polling"

    Write-Log "PASS every Windows tray check"
}
finally {
    # Stop the GUI first, then the daemon it launched: the daemon is detached
    # and would otherwise outlive the job.
    Stop-IfRunning $guiProcess
    try {
        & $GreggPath --config (Join-Path $workDir "gregg.toml") daemon stop 2>&1 | Out-Null
    } catch {
        # Best effort: the daemon may already be gone.
    }
    Stop-IfRunning $fixtureProcess
    Remove-Item -LiteralPath $workDir -Recurse -Force -ErrorAction SilentlyContinue
}
