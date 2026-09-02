//! Startup installation, instruction rendering, and restart helpers for `greggd`.
//!
//! The foreground daemon (`greggd run`) remains unaware of who supervises it.
//! This module owns the explicit deployment boundary: installing system
//! services, rendering cron entries, and restarting through the appropriate
//! manager. No collector, sampler, or HTTP code depends on this module.

use std::fmt;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Config;

const MANAGER_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(unix)]
const DIRECT_RESTART_TIMEOUT: Duration = Duration::from_secs(10);

fn run_bounded_command(program: &str, args: &[&str], timeout: Duration) -> io::Result<Output> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().map(read_pipe);
    let stderr = child.stderr.take().map(read_pipe);
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_pipe(stdout);
                let _ = join_pipe(stderr);
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("{program} timed out after {}s", timeout.as_secs()),
                ));
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    };
    Ok(Output {
        status,
        stdout: join_pipe(stdout)?,
        stderr: join_pipe(stderr)?,
    })
}

fn read_pipe<R: Read + Send + 'static>(mut reader: R) -> thread::JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn join_pipe(reader: Option<thread::JoinHandle<io::Result<Vec<u8>>>>) -> io::Result<Vec<u8>> {
    match reader {
        Some(reader) => reader
            .join()
            .map_err(|_| io::Error::other("child output reader panicked"))?,
        None => Ok(Vec::new()),
    }
}

// ── Startup method ──────────────────────────────────────────────────────────

/// Internal startup method. `Direct` means unmanaged / no manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupMethod {
    Systemd,
    Launchd,
    Cron,
    WindowsScm,
    Direct,
}

impl fmt::Display for StartupMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Systemd => "systemd",
            Self::Launchd => "launchd",
            Self::Cron => "cron",
            Self::WindowsScm => "windows-scm",
            Self::Direct => "direct",
        };
        write!(f, "{s}")
    }
}

/// CLI argument for `startup install` / `instructions`.
///
/// `Auto` defers to platform detection; explicit variants override it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartupMethodArg {
    #[default]
    Auto,
    Systemd,
    Launchd,
    Cron,
}

impl std::str::FromStr for StartupMethodArg {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "systemd" => Ok(Self::Systemd),
            "launchd" => Ok(Self::Launchd),
            "cron" => Ok(Self::Cron),
            other => Err(format!(
                "unknown startup method '{other}'; expected auto, systemd, launchd, or cron"
            )),
        }
    }
}

impl fmt::Display for StartupMethodArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Auto => "auto",
            Self::Systemd => "systemd",
            Self::Launchd => "launchd",
            Self::Cron => "cron",
        };
        write!(f, "{s}")
    }
}

// ── Standard paths ─────────────────────────────────────────────────────────

pub fn standard_systemd_binary() -> PathBuf {
    PathBuf::from("/usr/local/bin/greggd")
}

pub fn standard_systemd_config() -> PathBuf {
    PathBuf::from("/etc/gregg/greggd.toml")
}

pub fn standard_systemd_unit_path() -> PathBuf {
    PathBuf::from("/etc/systemd/system/greggd.service")
}

pub fn standard_systemd_config_dir() -> PathBuf {
    PathBuf::from("/etc/gregg")
}

pub fn standard_launchd_binary() -> PathBuf {
    PathBuf::from("/usr/local/bin/greggd")
}

pub fn standard_launchd_config() -> PathBuf {
    PathBuf::from("/Library/Application Support/gregg/greggd.toml")
}

pub fn standard_launchd_plist_path() -> PathBuf {
    PathBuf::from("/Library/LaunchDaemons/com.eggstack.greggd.plist")
}

pub fn launchd_label() -> &'static str {
    "com.eggstack.greggd"
}

// ── Canonical unit / plist content ────────────────────────────────────────

/// Canonical systemd unit. The installed binary can render this without a
/// checkout. Keep it synchronized with `packaging/systemd/greggd.service`.
pub fn systemd_unit_content() -> String {
    const TEMPLATE: &str = r"[Unit]
Description=Gregg metrics daemon
Documentation=https://github.com/eggstack/gregg
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=greggd
Group=greggd
RuntimeDirectory=gregg
ExecStart=/usr/local/bin/greggd run --config /etc/gregg/greggd.toml
Restart=on-failure
RestartSec=5
StartLimitIntervalSec=60
StartLimitBurst=5

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadOnlyPaths=/proc /sys
ReadWritePaths=/etc/gregg
PrivateTmp=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictNamespaces=true
RestrictSUIDSGID=true
MemoryDenyWriteExecute=true
RestrictRealtime=true
LockPersonality=true
SystemCallFilter=@system-service
SystemCallArchitectures=native

# Network access
IPAddressAllow=any
IPAddressDeny=

# Capabilities
CapabilityBoundingSet=
AmbientCapabilities=

[Install]
WantedBy=multi-user.target
";
    TEMPLATE.to_string()
}

/// Canonical launchd plist. Keep synchronized with
/// `packaging/launchd/com.eggstack.greggd.plist`.
pub fn launchd_plist_content() -> String {
    const TEMPLATE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.eggstack.greggd</string>

    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/greggd</string>
        <string>run</string>
        <string>--config</string>
        <string>/Library/Application Support/gregg/greggd.toml</string>
    </array>

    <key>RunAtLoad</key>
    <true/>

    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
        <key>Crashed</key>
        <true/>
    </dict>

    <key>ThrottleInterval</key>
    <integer>10</integer>

    <key>StandardOutPath</key>
    <string>/var/log/greggd.log</string>

    <key>StandardErrorPath</key>
    <string>/var/log/greggd.log</string>

    <key>HardResourceLimits</key>
    <dict>
        <key>NumberOfFiles</key>
        <integer>1024</integer>
    </dict>

    <key>SoftResourceLimits</key>
    <dict>
        <key>NumberOfFiles</key>
        <integer>1024</integer>
    </dict>
</dict>
</plist>
"#;
    TEMPLATE.to_string()
}

// ── Systemd environment detection ─────────────────────────────────────────

/// Pure helper: does the host look like a running systemd environment given
/// injected probes? Used by tests to avoid touching the real filesystem.
///
/// `run_systemd_exists` mirrors `Path::new("/run/systemd/system").exists()`.
/// `proc1_comm_systemd` mirrors whether `/proc/1/comm` equals `systemd`.
pub fn is_systemd_environment_with(run_systemd_exists: bool, proc1_comm_is_systemd: bool) -> bool {
    // Require the /run/systemd/system directory. On Linux this is the
    // canonical indicator that systemd owns PID 1; checking proc1 adds a
    // second reliable signal without relying solely on systemctl presence.
    run_systemd_exists && proc1_comm_is_systemd
}

/// Real check: inspect `/run/systemd/system` and `/proc/1/comm` when available.
pub fn is_systemd_environment() -> bool {
    let run_exists = Path::new("/run/systemd/system").exists();
    // On non-Linux hosts this file won't exist; treat as non-systemd.
    let proc1_is_systemd = read_proc1_comm().is_some_and(|comm| comm == "systemd");
    // If /proc/1/comm is unavailable (container, non-Linux), fall back to
    // directory existence plus a bounded systemctl probe as equivalent signal.
    if run_exists && proc1_is_systemd {
        return true;
    }
    if run_exists {
        // Bounded systemctl probe: try `systemctl is-system-running --quiet`
        // with a short timeout. Use a best-effort check without blocking long.
        return systemctl_probe_is_running();
    }
    false
}

fn read_proc1_comm() -> Option<String> {
    let content = fs::read_to_string("/proc/1/comm").ok()?;
    Some(content.trim().to_string())
}

fn systemctl_probe_is_running() -> bool {
    run_bounded_command(
        "systemctl",
        &["is-system-running", "--quiet"],
        MANAGER_COMMAND_TIMEOUT,
    )
    .is_ok()
}

// ── Auto detection ────────────────────────────────────────────────────────

/// Pure helper for auto method selection. `os` values are `std::env::consts::OS`
/// style: "linux", "macos", "windows", etc.
pub fn auto_method_for(os: &str, is_systemd: bool) -> StartupMethod {
    match os {
        "windows" => StartupMethod::WindowsScm,
        "macos" | "darwin" => StartupMethod::Launchd,
        "linux" => {
            if is_systemd {
                StartupMethod::Systemd
            } else {
                StartupMethod::Cron
            }
        }
        _ => StartupMethod::Cron,
    }
}

/// Real auto detection using the current OS and live systemd probe.
pub fn auto_detect_method() -> StartupMethod {
    let os = std::env::consts::OS;
    let is_systemd = is_systemd_environment();
    auto_method_for(os, is_systemd)
}

/// Resolve a CLI `--method` argument to a concrete `StartupMethod`.
/// `Auto` defers to `auto_detect_method()`.
pub fn resolve_startup_method(arg: StartupMethodArg) -> StartupMethod {
    match arg {
        StartupMethodArg::Auto => auto_detect_method(),
        StartupMethodArg::Systemd => StartupMethod::Systemd,
        StartupMethodArg::Launchd => StartupMethod::Launchd,
        StartupMethodArg::Cron => StartupMethod::Cron,
    }
}

/// Pure helper for tests.
pub fn resolve_startup_method_with(
    arg: StartupMethodArg,
    os: &str,
    is_systemd: bool,
) -> StartupMethod {
    match arg {
        StartupMethodArg::Auto => auto_method_for(os, is_systemd),
        StartupMethodArg::Systemd => StartupMethod::Systemd,
        StartupMethodArg::Launchd => StartupMethod::Launchd,
        StartupMethodArg::Cron => StartupMethod::Cron,
    }
}

// ── Shell quoting ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellQuoteError {
    ContainsControl { path: String },
    ContainsNewline { path: String },
}

impl fmt::Display for ShellQuoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContainsControl { path } => {
                write!(
                    f,
                    "path contains control character and cannot be shell-quoted: {path:?}"
                )
            }
            Self::ContainsNewline { path } => {
                write!(
                    f,
                    "path contains newline and cannot be shell-quoted: {path:?}"
                )
            }
        }
    }
}

impl std::error::Error for ShellQuoteError {}

/// Quote a path for safe inclusion in a POSIX shell cron line.
///
/// Wraps in single quotes and escapes embedded single quotes as `'\''`.
/// Rejects paths containing newlines or control characters.
pub fn shell_quote(path: &Path) -> Result<String, ShellQuoteError> {
    let s = path.to_string_lossy().to_string();
    if s.contains('\n') || s.contains('\r') {
        return Err(ShellQuoteError::ContainsNewline { path: s });
    }
    if s.chars().any(char::is_control) {
        return Err(ShellQuoteError::ContainsControl { path: s });
    }
    // Escape single quotes: ' -> '\''
    let escaped = s.replace('\'', "'\\''");
    Ok(format!("'{escaped}'"))
}

// ── Cron block rendering ──────────────────────────────────────────────────

pub const CRON_MANAGED_MARKER: &str = "# greggd managed watchdog";

/// Render the canonical cron block for the given executable and config.
///
/// Returns a string with trailing newline after each line, including final.
pub fn cron_block(exe: &Path, config: &Path, explicit: bool) -> Result<String, ShellQuoteError> {
    let exe_q = shell_quote(exe)?;
    let mut block = String::new();
    block.push_str(CRON_MANAGED_MARKER);
    block.push('\n');
    if explicit {
        let cfg_q = shell_quote(config)?;
        let _ = writeln!(block, "@reboot {exe_q} --config {cfg_q} croncheck");
        let _ = writeln!(block, "* * * * * {exe_q} --config {cfg_q} croncheck");
    } else {
        // For implicit default config, omit --config so daemon can use
        // default-path logic (missing file -> defaults). However cron is
        // user-owned and we want deterministic behavior; we still support
        // implicit by not adding --config. Callers that want explicit config
        // should pass explicit = true.
        let _ = writeln!(block, "@reboot {exe_q} croncheck");
        let _ = writeln!(block, "* * * * * {exe_q} croncheck");
    }
    Ok(block)
}

/// Convenience that always includes --config (the common Linux cron path).
pub fn cron_block_with_config(exe: &Path, config: &Path) -> Result<String, ShellQuoteError> {
    cron_block(exe, config, true)
}

// ── Crontab merging ───────────────────────────────────────────────────────

/// Remove any previously installed Gregg managed block from a crontab.
///
/// The block is identified by the marker line `CRON_MANAGED_MARKER` and any
/// following lines that contain `croncheck` (the two cron
/// schedule lines). Byte-for-byte preservation of unrelated lines is the goal.
pub fn remove_managed_cron_block(crontab: &str) -> String {
    let mut out = Vec::new();
    let mut lines = crontab.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() == CRON_MANAGED_MARKER {
            // Skip this marker and any immediately following croncheck lines.
            while let Some(next) = lines.peek() {
                if next.contains("croncheck") {
                    lines.next();
                } else {
                    break;
                }
            }
            continue;
        }
        // Also handle BEGIN/END style if somehow present (defensive).
        if line.trim() == "# BEGIN greggd managed watchdog" {
            for next in lines.by_ref() {
                if next.trim() == "# END greggd managed watchdog" {
                    break;
                }
            }
            continue;
        }
        out.push(line);
    }
    // Reconstruct with trailing newline handling: preserve original's final
    // newline semantics, but ensure we don't produce spurious blank lines at end.
    // `lines()` strips trailing newline, so we need to decide.
    let mut result = out.join("\n");
    if !crontab.is_empty() && crontab.ends_with('\n') && !result.is_empty() {
        result.push('\n');
    } else if result.is_empty() && !out.is_empty() {
        // Empty but had lines? keep as is.
    }
    // If original had no trailing newline but we removed block, keep no extra.
    // Ensure we don't leave double newlines from removal: out.join already does.
    result
}

/// Merge an existing crontab with a new canonical block idempotently.
///
/// Preserves unrelated entries byte-for-byte where practical; ensures exactly
/// one Gregg block at the end. `existing` may be empty (no crontab).
pub fn merge_crontab(existing: &str, new_block: &str) -> String {
    let stripped = remove_managed_cron_block(existing);
    let stripped_trim = stripped.trim_end_matches('\n');
    if stripped_trim.is_empty() {
        // No existing content: just the new block.
        let mut s = new_block.to_string();
        if !s.ends_with('\n') {
            s.push('\n');
        }
        return s;
    }
    // Existing content plus exactly one blank separator if needed, then new block.
    let mut out = String::new();
    out.push_str(stripped_trim);
    out.push('\n');
    // Ensure exactly one newline separation; new_block already starts with marker.
    out.push_str(new_block);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

// ── Instruction rendering ─────────────────────────────────────────────────

/// Render human-readable startup instructions for the given method.
///
/// `exe` and `config` are the current executable and resolved config path.
/// `explicit` controls whether `--config` is shown in cron lines.
#[allow(clippy::too_many_lines)]
#[allow(clippy::uninlined_format_args)]
pub fn render_instructions(
    method: StartupMethod,
    exe: &Path,
    config: &Path,
    _explicit: bool,
) -> String {
    let exe_str = exe.display().to_string();
    let cfg_str = config.display().to_string();
    match method {
        StartupMethod::Systemd => {
            let unit = standard_systemd_unit_path().display().to_string();
            let binary = standard_systemd_binary().display().to_string();
            let cfg = standard_systemd_config().display().to_string();
            format!(
                "Systemd startup (Linux)\n\
                \n\
                Standard paths:\n\
                  binary: {binary}\n\
                  config: {cfg}\n\
                  unit:   {unit}\n\
                \n\
                Install (requires root):\n\
                  sudo {exe_str} startup install --method systemd\n\
                  # or explicitly:\n\
                  sudo {exe_str} startup install --method systemd --config {cfg_str}\n\
                \n\
                The installer will:\n\
                  - verify {binary} exists\n\
                  - ensure the greggd system user/group exists\n\
                  - create {cfg} if absent (preserve existing)\n\
                  - write {unit} atomically\n\
                  - run: systemctl daemon-reload\n\
                  - run: systemctl enable greggd\n\
                  - start or restart the service\n\
                \n\
                Manual/status commands:\n\
                  systemctl status greggd\n\
                  systemctl restart greggd\n\
                  systemctl stop greggd\n\
                  journalctl -u greggd -f\n\
                \n\
                Config location: {cfg}\n"
            )
        }
        StartupMethod::Launchd => {
            let plist = standard_launchd_plist_path().display().to_string();
            let binary = standard_launchd_binary().display().to_string();
            let cfg = standard_launchd_config().display().to_string();
            let label = launchd_label();
            format!(
                "Launchd startup (macOS)\n\
                \n\
                Standard paths:\n\
                  binary: {binary}\n\
                  config: {cfg}\n\
                  plist:  {plist}\n\
                  label:  {label}\n\
                \n\
                Install (requires root):\n\
                  sudo {exe_str} startup install --method launchd\n\
                \n\
                Manual/status commands:\n\
                  sudo launchctl bootstrap system {plist}\n\
                  sudo launchctl kickstart -k system/{label}\n\
                  sudo launchctl bootout system/{label}\n\
                  log show --predicate 'process == \"greggd\"' --last 5m\n\
                \n\
                Config location: {cfg}\n"
            )
        }
        StartupMethod::Cron => {
            // Instructions always show the explicit --config form so the
            // operator can copy-paste a deterministic entry.
            let block = cron_block(exe, config, true)
                .unwrap_or_else(|e| format!("# error: {e}\n"));
            format!(
                "Cron startup (Unix, non-systemd)\n\
                \n\
                This method uses croncheck as the supervisor. No PID file is required.\n\
                The daemon is started only when the health endpoint is definitely absent\n\
                (connection refused). An ambiguous or non-Gregg listener is never\n\
                blindly replaced.\n\
                \n\
                Canonical cron entries for this host:\n\
                {block}\n\
                Install (user-local, no root required):\n\
                  {exe_str} startup install --method cron\n\
                  # with explicit config:\n\
                  {exe_str} startup install --method cron --config {cfg_str}\n\
                \n\
                Manual installation:\n\
                  crontab -l > /tmp/crontab.tmp  # or create empty if no crontab\n\
                  # append the two lines above (including the marker comment)\n\
                  crontab /tmp/crontab.tmp\n\
                  crontab -l   # verify\n\
                \n\
                The managed block is identified by:\n\
                  {marker}\n\
                Rerunning `startup install --method cron` is idempotent and preserves\n\
                unrelated crontab entries.\n\
                \n\
                If `crontab` is unavailable, add the lines through your scheduler's\n\
                native editor and ensure `greggd croncheck` runs at reboot and every minute.\n",
                marker = CRON_MANAGED_MARKER,
            )
        }
        StartupMethod::WindowsScm => {
            String::from(
                "Windows Service (SCM)\n\
                \n\
                The daemon runs as a Windows service via the Service Control Manager.\n\
                \n\
                Install (run PowerShell as Administrator):\n\
                  irm https://github.com/eggstack/gregg/releases/latest/download/install.ps1 | iex\n\
                  .\\packaging\\install.ps1 -Component Greggd\n\
                  # or via bootstrap installer:\n\
                  .\\packaging\\install.ps1 -Component Greggd -Version 1.0.11\n\
                \n\
                Service details:\n\
                  name:        greggd\n\
                  display:     Gregg Metrics Daemon\n\
                  start type:  Automatic\n\
                  account:     NT AUTHORITY\\LocalService\n\
                  config:      %ProgramData%\\gregg\\greggd.toml\n\
                  image:       \"%ProgramFiles%\\Gregg\\greggd.exe\" service --config \"%ProgramData%\\gregg\\greggd.toml\"\n\
                \n\
                Lifecycle commands:\n\
                  greggd start\n\
                  greggd stop\n\
                  greggd restart\n\
                  Get-Service greggd\n\
                  sc.exe query greggd\n\
                \n\
                Existing config at %ProgramData%\\gregg\\greggd.toml is preserved.\n\
                `greggd startup install` on Windows reports service state; SCM\n\
                registration remains owned by the PowerShell installer.\n",
            )
        }
        StartupMethod::Direct => {
            format!(
                "Direct (unmanaged)\n\
                \n\
                No system service manager detected. Run the daemon directly:\n\
                  {exe_str} run --config {cfg_str}\n\
                \n\
                For automatic startup, install via cron:\n\
                  {exe_str} startup install --method cron\n\
                Or use a system service if available:\n\
                  sudo {exe_str} startup install --method systemd   # Linux\n\
                  sudo {exe_str} startup install --method launchd   # macOS\n"
            )
        }
    }
}

// ── Manager state detection for restart / update ──────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupState {
    SystemdActive,
    SystemdInstalledStopped,
    LaunchdLoaded,
    LaunchdInstalledUnloaded,
    WindowsServiceRunning,
    WindowsServiceStopped,
    UnmanagedOrCron,
}

impl fmt::Display for StartupState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::SystemdActive => "systemd-active",
            Self::SystemdInstalledStopped => "systemd-installed-stopped",
            Self::LaunchdLoaded => "launchd-loaded",
            Self::LaunchdInstalledUnloaded => "launchd-installed-unloaded",
            Self::WindowsServiceRunning => "windows-running",
            Self::WindowsServiceStopped => "windows-stopped",
            Self::UnmanagedOrCron => "unmanaged-or-cron",
        };
        write!(f, "{s}")
    }
}

/// Pure helper: decide systemd state from injected probes.
pub fn systemd_state_with(unit_exists: bool, is_active: bool) -> StartupState {
    if is_active {
        StartupState::SystemdActive
    } else if unit_exists {
        StartupState::SystemdInstalledStopped
    } else {
        StartupState::UnmanagedOrCron
    }
}

/// Pure helper: decide launchd state from injected probes.
pub fn launchd_state_with(plist_exists: bool, is_loaded: bool) -> StartupState {
    if is_loaded {
        StartupState::LaunchdLoaded
    } else if plist_exists {
        StartupState::LaunchdInstalledUnloaded
    } else {
        StartupState::UnmanagedOrCron
    }
}

#[allow(dead_code)]
fn systemd_unit_exists() -> bool {
    standard_systemd_unit_path().exists()
}

#[allow(dead_code)]
fn systemd_is_active() -> bool {
    matches!(
        run_bounded_command(
            "systemctl",
            &["is-active", "--quiet", "greggd"],
            MANAGER_COMMAND_TIMEOUT,
        ),
        Ok(output) if output.status.success()
    )
}

#[allow(dead_code)]
fn launchd_plist_exists() -> bool {
    standard_launchd_plist_path().exists()
}

#[allow(dead_code)]
fn launchd_is_loaded() -> bool {
    // `launchctl print system/com.eggstack.greggd` exits 0 when loaded on
    // modern macOS; fall back to `launchctl list | grep`.
    if matches!(
        run_bounded_command(
            "launchctl",
            &["print", &format!("system/{}", launchd_label())],
            MANAGER_COMMAND_TIMEOUT,
        ),
        Ok(output) if output.status.success()
    ) {
        return true;
    }
    // Fallback: `launchctl list` contains label
    if let Ok(output) = run_bounded_command("launchctl", &["list"], MANAGER_COMMAND_TIMEOUT) {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains(launchd_label()) {
                return true;
            }
        }
    }
    false
}

/// Detect the current startup state for restart/update dispatch.
///
/// This is the small helper required by Plan 100 §6 "Manager detection".
/// It answers enough for both `restart` and Plan 101 `update` without building
/// a generalized discovery database.
pub fn startup_state() -> StartupState {
    #[cfg(target_os = "windows")]
    {
        // Delegate to existing SCM manager when on Windows.
        match crate::service::platform_service_manager().is_active() {
            Ok(true) => StartupState::WindowsServiceRunning,
            Ok(false) => StartupState::WindowsServiceStopped,
            Err(_) => StartupState::UnmanagedOrCron,
        }
    }
    #[cfg(all(unix, target_os = "macos"))]
    {
        let plist_exists = launchd_plist_exists();
        let loaded = launchd_is_loaded();
        let state = launchd_state_with(plist_exists, loaded);
        if state != StartupState::UnmanagedOrCron {
            return state;
        }
        // Not launchd-managed: fall through to systemd/cron check.
        // On macOS, systemd is not relevant, so return unmanaged.
        StartupState::UnmanagedOrCron
    }
    #[cfg(all(unix, target_os = "linux"))]
    {
        let unit_exists = systemd_unit_exists();
        let active = if unit_exists || is_systemd_environment() {
            systemd_is_active()
        } else {
            false
        };
        let state = systemd_state_with(unit_exists, active);
        if state != StartupState::UnmanagedOrCron {
            return state;
        }
        StartupState::UnmanagedOrCron
    }
    #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
    {
        StartupState::UnmanagedOrCron
    }
}

// ── Atomic file write helpers ─────────────────────────────────────────────

fn write_atomic_text(path: &Path, content: &str) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory")
    })?;
    fs::create_dir_all(dir)?;
    // Write to a temp file in the same directory.
    let tmp_name = format!(
        ".greggd-startup-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let tmp_path = dir.join(tmp_name);
    // Ensure we clean up on failure.
    let write_res = (|| -> io::Result<()> {
        fs::write(&tmp_path, content)?;
        // Flush to disk where possible.
        let file = fs::OpenOptions::new().read(true).open(&tmp_path)?;
        let _ = file.sync_all();
        fs::rename(&tmp_path, path)?;
        // Sync parent directory where supported.
        #[cfg(unix)]
        {
            if let Ok(dir_file) = fs::OpenOptions::new().read(true).open(dir) {
                let _ = dir_file.sync_all();
            }
        }
        Ok(())
    })();
    if write_res.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    write_res
}

// ── Privilege helpers ─────────────────────────────────────────────────────

#[cfg(unix)]
#[allow(unsafe_code)]
fn is_privileged() -> bool {
    // SAFETY: geteuid is a pure libc call with no side effects and is safe
    // to call at any time. It does not read or write Rust-managed memory.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
fn is_privileged() -> bool {
    // On non-Unix, attempt a privileged operation and handle PermissionDenied.
    // For Windows, check via SCM or assume not privileged if not admin.
    // We treat as privileged only if we can open SCM with create access.
    #[cfg(target_os = "windows")]
    {
        // Best-effort: try to query SCM; if AccessDenied, not privileged.
        // Use a simple check: attempt to open ServiceManager.
        // If we can't, assume not privileged.
        return is_windows_admin();
    }
    #[cfg(not(target_os = "windows"))]
    {
        true
    }
}

#[cfg(target_os = "windows")]
fn is_windows_admin() -> bool {
    // Use `net session` style check via `IsUserAnAdmin` if windows crate
    // were available. Fallback to checking if we can open SCM.
    // For now, attempt to use `whoami /groups`? Simpler: try to create a
    // test file in ProgramFiles; if PermissionDenied, not admin.
    // Keep it small: return false and let the actual install attempt surface
    // PermissionDenied, which we then handle. This avoids adding a windows dep.
    false
}

// ── Systemd installation ──────────────────────────────────────────────────

#[derive(Debug)]
pub enum InstallError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Permission {
        message: String,
    },
    SystemdNotDetected {
        message: String,
    },
    LaunchdNotDetected {
        message: String,
    },
    BinaryMissing {
        path: PathBuf,
    },
    UnsupportedMethod {
        method: StartupMethod,
        message: String,
    },
    ShellQuote(ShellQuoteError),
    CrontabUnavailable {
        message: String,
    },
    Other(String),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Permission { message }
            | Self::SystemdNotDetected { message }
            | Self::LaunchdNotDetected { message }
            | Self::CrontabUnavailable { message }
            | Self::Other(message) => write!(f, "{message}"),
            Self::BinaryMissing { path } => write!(
                f,
                "required binary not found at {}: install it first (e.g., sudo install -m 755 <binary> {})",
                path.display(),
                path.display()
            ),
            Self::UnsupportedMethod { method, message } => {
                write!(f, "method {method} not supported: {message}")
            }
            Self::ShellQuote(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for InstallError {}

impl From<ShellQuoteError> for InstallError {
    fn from(e: ShellQuoteError) -> Self {
        Self::ShellQuote(e)
    }
}

fn elevated_command(exe: &Path, method: StartupMethodArg) -> String {
    let exe_str = exe.display().to_string();
    match method {
        StartupMethodArg::Auto => format!("sudo {exe_str} startup install"),
        other => format!("sudo {exe_str} startup install --method {other}"),
    }
}

fn ensure_greggd_user() -> io::Result<()> {
    // Check if user exists via `id -u greggd`
    let mut cmd = Command::new("id");
    cmd.arg("-u");
    cmd.arg("greggd");
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    if let Ok(status) = cmd.status() {
        if status.success() {
            return Ok(());
        }
    }
    // Create system user
    let mut add = Command::new("useradd");
    add.arg("--system");
    add.arg("--no-create-home");
    add.arg("--shell");
    add.arg("/usr/sbin/nologin");
    add.arg("greggd");
    let status = add.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "useradd greggd failed with status {status}"
        )))
    }
}

fn ensure_config_preserved(config_path: &Path) -> io::Result<()> {
    if config_path.exists() {
        return Ok(());
    }
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Create default config atomically.
    let cfg = Config::default();
    cfg.write_atomic(config_path)
        .map_err(|e| io::Error::other(format!("failed to write default config: {e}")))?;
    Ok(())
}

fn set_config_ownership() -> io::Result<()> {
    // Best-effort: chown -R greggd:greggd /etc/gregg
    let dir = standard_systemd_config_dir();
    if !dir.exists() {
        return Ok(());
    }
    let mut cmd = Command::new("chown");
    cmd.arg("-R");
    cmd.arg("greggd:greggd");
    cmd.arg(&dir);
    let status = cmd.status()?;
    if status.success() {
        Ok(())
    } else {
        // Not fatal for install; log and continue.
        eprintln!(
            "warning: chown greggd:greggd {} failed: {status}",
            dir.display()
        );
        Ok(())
    }
}

fn run_systemctl(args: &[&str]) -> io::Result<()> {
    let output = run_bounded_command("systemctl", args, MANAGER_COMMAND_TIMEOUT)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "systemctl {} failed with status {:?}: {}",
            args.join(" "),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

fn manager_error_is_permission(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("permission")
        || message.contains("access denied")
        || message.contains("not authorized")
        || message.contains("authentication")
}

fn systemd_manager_error(exe: &Path, args: &[&str], error: io::Error) -> InstallError {
    if error.kind() == io::ErrorKind::PermissionDenied
        || manager_error_is_permission(&error.to_string())
    {
        InstallError::Permission {
            message: format!(
                "systemctl {} was denied; rerun as root: {}",
                args.join(" "),
                elevated_command(exe, StartupMethodArg::Systemd)
            ),
        }
    } else {
        InstallError::Io {
            path: PathBuf::from(format!("systemctl {}", args.join(" "))),
            source: error,
        }
    }
}

/// Install systemd service. `exe` is the current executable for elevated message.
pub fn install_systemd(exe: &Path, config_path: &Path) -> Result<(), InstallError> {
    // Verify systemd environment.
    if !is_systemd_environment() {
        return Err(InstallError::SystemdNotDetected {
            message: "systemd not detected: /run/systemd/system missing or PID 1 is not systemd"
                .into(),
        });
    }
    // Verify standard binary exists.
    let bin_path = standard_systemd_binary();
    if !bin_path.exists() {
        // Also check if current exe is at that path; if current exe exists but not at standard path, give actionable error.
        return Err(InstallError::BinaryMissing { path: bin_path });
    }
    // Privilege check: if not root, print elevated command and return PermissionDenied.
    if !is_privileged() {
        let cmd = elevated_command(exe, StartupMethodArg::Systemd);
        return Err(InstallError::Permission {
            message: format!("permission denied: rerun as root: {cmd}"),
        });
    }
    // Device steps (idempotent)
    ensure_greggd_user().map_err(|e| InstallError::Io {
        path: PathBuf::from("useradd greggd"),
        source: e,
    })?;
    // Ensure config dir and default config
    fs::create_dir_all(standard_systemd_config_dir()).map_err(|e| InstallError::Io {
        path: standard_systemd_config_dir(),
        source: e,
    })?;
    ensure_config_preserved(&standard_systemd_config()).map_err(|e| InstallError::Io {
        path: standard_systemd_config(),
        source: e,
    })?;
    set_config_ownership().map_err(|e| InstallError::Io {
        path: standard_systemd_config_dir(),
        source: e,
    })?;
    // Write unit atomically.
    let unit_content = systemd_unit_content();
    let unit_path = standard_systemd_unit_path();
    write_atomic_text(&unit_path, &unit_content).map_err(|e| {
        if e.kind() == io::ErrorKind::PermissionDenied {
            InstallError::Permission {
                message: format!(
                    "permission denied writing {}: rerun as root: {}",
                    unit_path.display(),
                    elevated_command(exe, StartupMethodArg::Systemd)
                ),
            }
        } else {
            InstallError::Io {
                path: unit_path.clone(),
                source: e,
            }
        }
    })?;
    // Ensure permissions 644
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&unit_path, fs::Permissions::from_mode(0o644));
    }
    // daemon-reload, enable, start/restart
    let daemon_reload = ["daemon-reload"];
    run_systemctl(&daemon_reload).map_err(|e| systemd_manager_error(exe, &daemon_reload, e))?;
    let enable = ["enable", "greggd"];
    run_systemctl(&enable).map_err(|e| systemd_manager_error(exe, &enable, e))?;
    // Decide start vs restart: if active, restart; else start.
    if systemd_is_active() {
        let restart = ["restart", "greggd"];
        run_systemctl(&restart).map_err(|e| systemd_manager_error(exe, &restart, e))?;
    } else {
        // Try start; if it fails because already running, try restart.
        if let Err(e) = run_systemctl(&["start", "greggd"]) {
            eprintln!("systemctl start failed ({e}), trying restart...");
            let restart = ["restart", "greggd"];
            run_systemctl(&restart).map_err(|e2| systemd_manager_error(exe, &restart, e2))?;
        }
    }
    println!("greggd systemd service installed: {}", unit_path.display());
    println!("config: {}", standard_systemd_config().display());
    println!("status: systemctl status greggd");
    println!("logs:   journalctl -u greggd -f");
    // Use config_path param to avoid unused warning; it is the caller's resolved config, which should match standard path.
    let _ = config_path;
    Ok(())
}

// ── Launchd installation ──────────────────────────────────────────────────

#[allow(clippy::too_many_lines)]
pub fn install_launchd(exe: &Path, _config_path: &Path) -> Result<(), InstallError> {
    // Verify macOS
    if std::env::consts::OS != "macos" && std::env::consts::OS != "darwin" {
        return Err(InstallError::LaunchdNotDetected {
            message: "launchd is only available on macOS".into(),
        });
    }
    let bin_path = standard_launchd_binary();
    if !bin_path.exists() {
        return Err(InstallError::BinaryMissing { path: bin_path });
    }
    if !is_privileged() {
        let cmd = elevated_command(exe, StartupMethodArg::Launchd);
        return Err(InstallError::Permission {
            message: format!("permission denied: rerun as root: {cmd}"),
        });
    }
    // Ensure config dir and default config
    let cfg_path = standard_launchd_config();
    if let Some(parent) = cfg_path.parent() {
        fs::create_dir_all(parent).map_err(|e| InstallError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    ensure_config_preserved(&cfg_path).map_err(|e| InstallError::Io {
        path: cfg_path.clone(),
        source: e,
    })?;
    // Write plist atomically
    let plist_content = launchd_plist_content();
    let plist_path = standard_launchd_plist_path();
    if let Some(parent) = plist_path.parent() {
        fs::create_dir_all(parent).map_err(|e| InstallError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    write_atomic_text(&plist_path, &plist_content).map_err(|e| {
        if e.kind() == io::ErrorKind::PermissionDenied {
            InstallError::Permission {
                message: format!(
                    "permission denied writing {}: rerun as root: {}",
                    plist_path.display(),
                    elevated_command(exe, StartupMethodArg::Launchd)
                ),
            }
        } else {
            InstallError::Io {
                path: plist_path.clone(),
                source: e,
            }
        }
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&plist_path, fs::Permissions::from_mode(0o644));
    }
    // Create log file
    let log_path = Path::new("/var/log/greggd.log");
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(log_path, fs::Permissions::from_mode(0o644));
    }
    // Manage launchd job: if already loaded, bootout then bootstrap; else bootstrap.
    let label = launchd_label();
    let loaded = launchd_is_loaded();
    if loaded {
        // Bootout existing job (best effort), but never allow a manager call
        // to hang indefinitely.
        let _ = run_bounded_command(
            "launchctl",
            &["bootout", &format!("system/{label}")],
            MANAGER_COMMAND_TIMEOUT,
        );
        let _ = run_bounded_command(
            "launchctl",
            &["bootout", "system", &plist_path.to_string_lossy()],
            MANAGER_COMMAND_TIMEOUT,
        );
    }
    // Bootstrap
    let bootstrap_args = ["bootstrap", "system", &plist_path.to_string_lossy()];
    let bootstrap = run_bounded_command("launchctl", &bootstrap_args, MANAGER_COMMAND_TIMEOUT)
        .map_err(|source| InstallError::Io {
            path: PathBuf::from("launchctl bootstrap"),
            source,
        })?;
    if !bootstrap.status.success() && !loaded {
        let detail = String::from_utf8_lossy(&bootstrap.stderr)
            .trim()
            .to_string();
        if manager_error_is_permission(&detail) {
            return Err(InstallError::Permission {
                message: format!(
                    "launchctl bootstrap failed: {detail}; rerun as root: {}",
                    elevated_command(exe, StartupMethodArg::Launchd)
                ),
            });
        }
        return Err(InstallError::Other(format!(
            "launchctl bootstrap failed with status {:?}: {detail}",
            bootstrap.status.code()
        )));
    }
    // Kickstart if it was previously loaded (restart), otherwise bootstrap already started it (RunAtLoad).
    if loaded {
        let kick = run_bounded_command(
            "launchctl",
            &["kickstart", "-k", &format!("system/{label}")],
            MANAGER_COMMAND_TIMEOUT,
        )
        .map_err(|source| InstallError::Io {
            path: PathBuf::from("launchctl kickstart"),
            source,
        })?;
        if !kick.status.success() {
            let detail = String::from_utf8_lossy(&kick.stderr).trim().to_string();
            if manager_error_is_permission(&detail) {
                return Err(InstallError::Permission {
                    message: format!(
                        "launchctl kickstart failed: {detail}; rerun as root: {}",
                        elevated_command(exe, StartupMethodArg::Launchd)
                    ),
                });
            }
            return Err(InstallError::Other(format!(
                "launchctl kickstart failed with status {:?}: {detail}",
                kick.status.code()
            )));
        }
    }
    println!("greggd launchd service installed: {}", plist_path.display());
    println!("config: {}", cfg_path.display());
    println!("logs: log show --predicate 'process == \"greggd\"' --last 5m");
    Ok(())
}

// ── Cron installation ─────────────────────────────────────────────────────

fn run_crontab_list() -> io::Result<String> {
    let output = Command::new("crontab").arg("-l").output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
        if stderr.contains("no crontab") || stderr.contains("no crontab for") {
            Ok(String::new())
        } else if output.status.code() == Some(1) && stderr.trim().is_empty() {
            // Some crontab implementations exit 1 with empty stderr for no crontab.
            // Treat as empty if stdout is empty.
            if output.stdout.is_empty() {
                Ok(String::new())
            } else {
                Err(io::Error::other(format!(
                    "crontab -l failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )))
            }
        } else {
            Err(io::Error::other(format!(
                "crontab -l failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )))
        }
    }
}

fn run_crontab_install(content: &str) -> io::Result<()> {
    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    {
        use std::io::Write;
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("failed to open crontab stdin"))?;
        stdin.write_all(content.as_bytes())?;
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "crontab - failed with status {status}"
        )))
    }
}

/// Install cron watchdog. Works for both privileged and unprivileged users.
/// No privilege check; cron is user-local.
pub fn install_cron(exe: &Path, config: &Path, explicit: bool) -> Result<(), InstallError> {
    let block = cron_block(exe, config, explicit)?;
    // Check crontab availability by trying to list.
    let existing = match run_crontab_list() {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(InstallError::CrontabUnavailable {
                message: format!(
                    "crontab not found: install cron or add manually:\n{block}\n\
                     Reminder: croncheck is the watchdog; no PID file required."
                ),
            });
        }
        Err(e) => {
            // If crontab exists but list failed for other reason, surface it
            // with manual instructions.
            return Err(InstallError::Other(format!(
                "crontab -l failed ({e}); add manually:\n{block}"
            )));
        }
    };
    let merged = merge_crontab(&existing, &block);
    run_crontab_install(&merged).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            InstallError::CrontabUnavailable {
                message: format!("crontab not found; add manually:\n{block}"),
            }
        } else {
            InstallError::Io {
                path: PathBuf::from("crontab -"),
                source: e,
            }
        }
    })?;
    println!("greggd cron watchdog installed");
    println!("entries:\n{block}");
    println!("Verify with: crontab -l");
    Ok(())
}

// ── Unified install dispatch ──────────────────────────────────────────────

pub fn install_startup(
    exe: &Path,
    config_path: &Path,
    explicit_config: bool,
    method_arg: StartupMethodArg,
) -> Result<(), InstallError> {
    let method = resolve_startup_method(method_arg);
    // Explicit method overrides auto; if auto selected systemd but privilege missing, do not fallback to cron.
    match method {
        StartupMethod::Systemd => install_systemd(exe, config_path),
        StartupMethod::Launchd => install_launchd(exe, config_path),
        StartupMethod::Cron | StartupMethod::Direct => {
            install_cron(exe, config_path, explicit_config)
        }
        StartupMethod::WindowsScm => {
            // On Windows, SCM registration is owned by the PowerShell installer.
            // Report state and instructions rather than duplicating sc.exe logic.
            #[cfg(target_os = "windows")]
            {
                let state = startup_state();
                println!("Windows Service (SCM) state: {state}");
                println!(
                    "{}",
                    render_instructions(method, exe, config_path, explicit_config)
                );
                // If service is not installed, instruct to use installer.
                if state == StartupState::UnmanagedOrCron {
                    println!(
                        "No greggd service found. Run as Administrator:\n  .\\packaging\\install.ps1 -Component Greggd"
                    );
                }
                Ok(())
            }
            #[cfg(not(target_os = "windows"))]
            {
                Err(InstallError::UnsupportedMethod {
                    method,
                    message: "Windows SCM is only available on Windows".into(),
                })
            }
        }
    }
}

// ── Restart ───────────────────────────────────────────────────────────────

/// Restart via systemd using `systemctl restart greggd`.
fn restart_systemd(exe: &Path) -> Result<(), InstallError> {
    if !systemd_unit_exists() {
        return Err(InstallError::Other(
            "systemd unit not installed: run `sudo greggd startup install --method systemd`".into(),
        ));
    }
    match run_systemctl(&["restart", "greggd"]) {
        Ok(()) => {
            println!("greggd restarted via systemd");
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(InstallError::Permission {
            message: format!(
                "permission denied: rerun as root: sudo systemctl restart greggd (original exe: {})",
                exe.display()
            ),
        }),
        Err(e) => {
            let msg = e.to_string();
            if manager_error_is_permission(&msg) {
                Err(InstallError::Permission {
                    message: format!(
                        "permission denied: rerun as root: sudo systemctl restart greggd (exe: {})",
                        exe.display()
                    ),
                })
            } else {
                Err(InstallError::Io {
                    path: PathBuf::from("systemctl restart greggd"),
                    source: e,
                })
            }
        }
    }
}

fn restart_launchd(exe: &Path) -> Result<(), InstallError> {
    let label = launchd_label();
    let args = ["kickstart", "-k", &format!("system/{label}")];
    match run_bounded_command("launchctl", &args, MANAGER_COMMAND_TIMEOUT) {
        Ok(output) if output.status.success() => {
            println!("greggd restarted via launchd (kickstart -k system/{label})");
            Ok(())
        }
        Ok(output) if manager_error_is_permission(&String::from_utf8_lossy(&output.stderr)) => {
            Err(InstallError::Permission {
                message: format!(
                    "permission denied: rerun as root: sudo launchctl kickstart -k system/{label} (exe: {})",
                    exe.display()
                ),
            })
        }
        Ok(output) => Err(InstallError::Io {
            path: PathBuf::from(format!("launchctl kickstart -k system/{label}")),
            source: io::Error::other(format!(
                "launchctl kickstart failed with status {:?}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        }),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(InstallError::Permission {
            message: format!(
                "permission denied: rerun as root: sudo launchctl kickstart -k system/{label} (exe: {})",
                exe.display()
            ),
        }),
        Err(e) => Err(InstallError::Io {
            path: PathBuf::from(format!("launchctl kickstart -k system/{label}")),
            source: e,
        }),
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestartStopState {
    Stopped,
    NotRunning,
    Uncertain,
    ControlError,
}

#[cfg(unix)]
fn restart_spawn_allowed(stop_state: RestartStopState, probe: &crate::cli::CroncheckProbe) -> bool {
    !matches!(stop_state, RestartStopState::Uncertain)
        && matches!(probe, crate::cli::CroncheckProbe::Absent)
}

#[allow(clippy::too_many_lines)]
fn restart_cron_direct(exe: &Path, config_path: &Path, explicit: bool) -> Result<(), InstallError> {
    // Use existing control socket stop + detached start via croncheck primitive.
    // 1) try stop
    #[cfg(unix)]
    {
        let outcome = crate::control::send_stop(config_path);
        let config = crate::cli::load_config(config_path, explicit).map_err(|error| {
            InstallError::Other(format!("failed to load restart config: {error}"))
        })?;
        let target = crate::cli::croncheck_target(&config);
        let stop_state = match outcome {
            Ok(crate::control::StopOutcome::Stopped { .. }) => {
                println!("greggd stopped via control socket");
                RestartStopState::Stopped
            }
            Ok(crate::control::StopOutcome::NotRunning) => {
                println!("greggd not running (control socket)");
                RestartStopState::NotRunning
            }
            Ok(crate::control::StopOutcome::Uncertain) => RestartStopState::Uncertain,
            Err(e) => {
                // If permission denied, surface it
                if let crate::control::ControlError::Io(io_e) = &e {
                    if io_e.kind() == io::ErrorKind::PermissionDenied {
                        return Err(InstallError::Permission {
                            message: format!("permission denied on stop: {e}"),
                        });
                    }
                }
                if !matches!(
                    crate::cli::probe_greggd(target),
                    crate::cli::CroncheckProbe::Absent
                ) {
                    return Err(InstallError::Other(format!(
                        "restart refused after stop error: endpoint is not definitely absent ({e})"
                    )));
                }
                RestartStopState::ControlError
            }
        };
        if matches!(stop_state, RestartStopState::Uncertain) {
            return Err(InstallError::Other(
                "restart refused: stop outcome was uncertain; daemon may still be running".into(),
            ));
        }
        wait_for_endpoint_absence(target)?;
        let probe = crate::cli::probe_greggd(target);
        if !restart_spawn_allowed(stop_state, &probe) {
            return Err(InstallError::Other(
                "restart refused: endpoint is not definitely absent".into(),
            ));
        }
        let mut cmd = crate::cli::build_daemon_command_for(exe, config_path, explicit);
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                return Err(InstallError::Permission {
                    message: format!("permission denied spawning greggd: {e}"),
                });
            }
            Err(e) => {
                return Err(InstallError::Io {
                    path: PathBuf::from("greggd run"),
                    source: e,
                });
            }
        };
        let deadline = Instant::now() + DIRECT_RESTART_TIMEOUT;
        loop {
            match crate::cli::probe_greggd(target) {
                crate::cli::CroncheckProbe::Running => {
                    println!("greggd restarted (direct/cron) and passed health check");
                    return Ok(());
                }
                crate::cli::CroncheckProbe::Ambiguous if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(InstallError::Other(
                        "greggd restart timed out with an ambiguous endpoint".into(),
                    ));
                }
                _ if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(InstallError::Other(
                        "greggd restart timed out before health readiness".into(),
                    ));
                }
                _ => {
                    if let Some(status) = child.try_wait().map_err(|source| InstallError::Io {
                        path: PathBuf::from("greggd run"),
                        source,
                    })? {
                        return Err(InstallError::Other(format!(
                            "greggd restart child exited before readiness: {status}"
                        )));
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (exe, config_path, explicit);
        Err(InstallError::Other(
            "direct restart not supported on this platform".into(),
        ))
    }
}

#[cfg(unix)]
fn wait_for_endpoint_absence(target: std::net::SocketAddr) -> Result<(), InstallError> {
    let deadline = Instant::now() + DIRECT_RESTART_TIMEOUT;
    loop {
        match crate::cli::probe_greggd(target) {
            crate::cli::CroncheckProbe::Absent => return Ok(()),
            crate::cli::CroncheckProbe::Running if Instant::now() >= deadline => {
                return Err(InstallError::Other(
                    "restart refused: configured endpoint remained occupied".into(),
                ));
            }
            crate::cli::CroncheckProbe::Ambiguous if Instant::now() >= deadline => {
                return Err(InstallError::Other(
                    "restart refused: could not prove configured endpoint is absent".into(),
                ));
            }
            _ => thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Manager-aware restart, factoring for Plan 101 reuse.
pub fn restart_with_state(
    state: StartupState,
    exe: &Path,
    config_path: &Path,
    explicit: bool,
) -> Result<(), InstallError> {
    match state {
        StartupState::SystemdActive | StartupState::SystemdInstalledStopped => restart_systemd(exe),
        StartupState::LaunchdLoaded | StartupState::LaunchdInstalledUnloaded => {
            restart_launchd(exe)
        }
        StartupState::WindowsServiceRunning | StartupState::WindowsServiceStopped => {
            #[cfg(target_os = "windows")]
            {
                crate::service::platform_service_manager()
                    .restart()
                    .map_err(|e| {
                        let msg = e.to_string().to_lowercase();
                        if msg.contains("access denied") || msg.contains("permission") {
                            InstallError::Permission {
                                message: format!("permission denied restarting service: {e}; rerun as Administrator"),
                            }
                        } else {
                            InstallError::Other(format!("service restart failed: {e}"))
                        }
                    })?;
                println!("greggd restarted via SCM");
                Ok(())
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = explicit;
                Err(InstallError::Other(
                    "Windows service restart requested on non-Windows".into(),
                ))
            }
        }
        StartupState::UnmanagedOrCron => restart_cron_direct(exe, config_path, explicit),
    }
}

pub fn restart_daemon(exe: &Path, config_path: &Path, explicit: bool) -> Result<(), InstallError> {
    let state = startup_state();
    restart_with_state(state, exe, config_path, explicit)
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_method_linux_systemd_is_systemd() {
        assert_eq!(auto_method_for("linux", true), StartupMethod::Systemd);
    }

    #[test]
    fn auto_method_linux_no_systemd_is_cron() {
        assert_eq!(auto_method_for("linux", false), StartupMethod::Cron);
    }

    #[test]
    fn auto_method_macos_is_launchd() {
        assert_eq!(auto_method_for("macos", false), StartupMethod::Launchd);
        assert_eq!(auto_method_for("macos", true), StartupMethod::Launchd);
    }

    #[test]
    fn auto_method_windows_is_scm() {
        assert_eq!(auto_method_for("windows", false), StartupMethod::WindowsScm);
    }

    #[test]
    fn explicit_overrides_auto() {
        assert_eq!(
            resolve_startup_method_with(StartupMethodArg::Systemd, "linux", false),
            StartupMethod::Systemd
        );
        assert_eq!(
            resolve_startup_method_with(StartupMethodArg::Cron, "linux", true),
            StartupMethod::Cron
        );
        assert_eq!(
            resolve_startup_method_with(StartupMethodArg::Launchd, "linux", true),
            StartupMethod::Launchd
        );
        assert_eq!(
            resolve_startup_method_with(StartupMethodArg::Auto, "linux", true),
            StartupMethod::Systemd
        );
        assert_eq!(
            resolve_startup_method_with(StartupMethodArg::Auto, "linux", false),
            StartupMethod::Cron
        );
    }

    #[test]
    fn shell_quote_simple() {
        assert_eq!(
            shell_quote(Path::new("/usr/local/bin/greggd")).unwrap(),
            "'/usr/local/bin/greggd'"
        );
    }

    #[test]
    fn shell_quote_with_spaces() {
        assert_eq!(
            shell_quote(Path::new("/tmp/my path/greggd")).unwrap(),
            "'/tmp/my path/greggd'"
        );
    }

    #[test]
    fn shell_quote_with_single_quote() {
        assert_eq!(
            shell_quote(Path::new("/tmp/a'b/greggd")).unwrap(),
            "'/tmp/a'\\''b/greggd'"
        );
    }

    #[test]
    fn shell_quote_rejects_newline() {
        assert!(shell_quote(Path::new("/tmp/a\nb")).is_err());
        assert!(shell_quote(Path::new("/tmp/a\rb")).is_err());
    }

    #[test]
    fn shell_quote_rejects_control() {
        assert!(shell_quote(Path::new("/tmp/a\x01b")).is_err());
    }

    #[test]
    fn cron_block_renders_with_quoted_paths() {
        let exe = Path::new("/usr/local/bin/greggd");
        let cfg = Path::new("/etc/gregg/greggd.toml");
        let block = cron_block_with_config(exe, cfg).unwrap();
        assert!(block.starts_with(CRON_MANAGED_MARKER));
        assert!(block.contains(
            "@reboot '/usr/local/bin/greggd' --config '/etc/gregg/greggd.toml' croncheck"
        ));
        assert!(block.contains(
            "* * * * * '/usr/local/bin/greggd' --config '/etc/gregg/greggd.toml' croncheck"
        ));
    }

    #[test]
    fn cron_block_with_spaces_is_quoted() {
        let exe = Path::new("/tmp/my greggd");
        let cfg = Path::new("/tmp/my config.toml");
        let block = cron_block_with_config(exe, cfg).unwrap();
        assert!(block.contains("'/tmp/my greggd'"));
        assert!(block.contains("'/tmp/my config.toml'"));
    }

    #[test]
    fn cron_block_rejects_unsafe_paths() {
        let exe = Path::new("/tmp/a\nb");
        let cfg = Path::new("/etc/gregg/greggd.toml");
        assert!(cron_block_with_config(exe, cfg).is_err());
    }

    #[test]
    fn remove_managed_block_is_idempotent_and_preserves_unrelated() {
        let existing = "FOO=bar\n# greggd managed watchdog\n@reboot '/a' --config '/b' croncheck\n* * * * * '/a' --config '/b' croncheck\nOTHER=1\n";
        let stripped = remove_managed_cron_block(existing);
        assert_eq!(stripped, "FOO=bar\nOTHER=1\n");
        // Stripping again is idempotent
        assert_eq!(remove_managed_cron_block(&stripped), stripped);
    }

    #[test]
    fn merge_crontab_appends_and_is_idempotent() {
        let exe = Path::new("/usr/local/bin/greggd");
        let cfg = Path::new("/etc/gregg/greggd.toml");
        let block = cron_block_with_config(exe, cfg).unwrap();
        let existing = "FOO=bar\n";
        let merged = merge_crontab(existing, &block);
        assert!(merged.contains("FOO=bar"));
        assert!(merged.contains(CRON_MANAGED_MARKER));
        // Merging again should not duplicate
        let merged2 = merge_crontab(&merged, &block);
        assert_eq!(merged, merged2);
        // Count occurrences of marker
        assert_eq!(merged.matches(CRON_MANAGED_MARKER).count(), 1);
    }

    #[test]
    fn merge_empty_crontab() {
        let exe = Path::new("/usr/local/bin/greggd");
        let cfg = Path::new("/etc/gregg/greggd.toml");
        let block = cron_block_with_config(exe, cfg).unwrap();
        let merged = merge_crontab("", &block);
        assert_eq!(merged, block);
    }

    #[test]
    fn instructions_contain_standard_paths() {
        let exe = Path::new("/usr/local/bin/greggd");
        let cfg = Path::new("/etc/gregg/greggd.toml");
        let s = render_instructions(StartupMethod::Systemd, exe, cfg, true);
        assert!(s.contains("/usr/local/bin/greggd"));
        assert!(s.contains("/etc/gregg/greggd.toml"));
        assert!(s.contains("/etc/systemd/system/greggd.service"));

        let s2 = render_instructions(StartupMethod::Cron, exe, cfg, true);
        assert!(s2.contains(CRON_MANAGED_MARKER));
        assert!(s2.contains("croncheck"));
        assert!(s2.contains("No PID file"));

        let s3 = render_instructions(
            StartupMethod::Launchd,
            exe,
            Path::new("/Library/Application Support/gregg/greggd.toml"),
            true,
        );
        assert!(s3.contains("com.eggstack.greggd"));
        assert!(s3.contains("/Library/LaunchDaemons/com.eggstack.greggd.plist"));
    }

    #[test]
    fn systemd_unit_content_contains_hardening() {
        let content = systemd_unit_content();
        assert!(content.contains("ExecStart=/usr/local/bin/greggd"));
        assert!(content.contains("NoNewPrivileges"));
        assert!(content.contains("ProtectSystem"));
        assert!(content.contains("[Service]"));
        assert!(content.contains("[Unit]"));
    }

    #[test]
    fn launchd_plist_content_contains_label() {
        let content = launchd_plist_content();
        assert!(content.contains("com.eggstack.greggd"));
        assert!(content.contains("/usr/local/bin/greggd"));
        assert!(content.contains("KeepAlive"));
    }

    #[test]
    fn is_systemd_environment_with_helper() {
        assert!(is_systemd_environment_with(true, true));
        assert!(!is_systemd_environment_with(true, false));
        assert!(!is_systemd_environment_with(false, true));
        assert!(!is_systemd_environment_with(false, false));
    }

    #[test]
    fn startup_state_helpers() {
        assert_eq!(systemd_state_with(true, true), StartupState::SystemdActive);
        assert_eq!(
            systemd_state_with(true, false),
            StartupState::SystemdInstalledStopped
        );
        assert_eq!(
            systemd_state_with(false, false),
            StartupState::UnmanagedOrCron
        );
        assert_eq!(launchd_state_with(false, true), StartupState::LaunchdLoaded);
        assert_eq!(
            launchd_state_with(true, false),
            StartupState::LaunchdInstalledUnloaded
        );
    }

    #[cfg(unix)]
    #[test]
    fn restart_spawn_decision_requires_definite_absence() {
        use crate::cli::CroncheckProbe;

        assert!(restart_spawn_allowed(
            RestartStopState::Stopped,
            &CroncheckProbe::Absent
        ));
        assert!(restart_spawn_allowed(
            RestartStopState::NotRunning,
            &CroncheckProbe::Absent
        ));
        assert!(restart_spawn_allowed(
            RestartStopState::ControlError,
            &CroncheckProbe::Absent
        ));
        assert!(!restart_spawn_allowed(
            RestartStopState::Uncertain,
            &CroncheckProbe::Absent
        ));
        assert!(!restart_spawn_allowed(
            RestartStopState::Stopped,
            &CroncheckProbe::Running
        ));
        assert!(!restart_spawn_allowed(
            RestartStopState::NotRunning,
            &CroncheckProbe::Ambiguous
        ));

        let mut spawn_count = 0;
        if restart_spawn_allowed(RestartStopState::Stopped, &CroncheckProbe::Absent) {
            spawn_count += 1;
        }
        assert_eq!(spawn_count, 1);
    }

    #[test]
    fn restart_with_state_systemd_calls_systemctl_when_mocked() {
        // This test only verifies the helper maps correctly; it doesn't run systemctl.
        // We test that an unmanaged state would go to cron/direct path without panicking in pure helper.
        // Actual systemctl invocation is not mocked here; we just check state mapping.
        let state = systemd_state_with(true, true);
        assert_eq!(state, StartupState::SystemdActive);
    }

    #[test]
    fn embedded_systemd_unit_matches_packaging_file_when_present() {
        let packaging =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/systemd/greggd.service");
        if let Ok(file) = std::fs::read_to_string(&packaging) {
            let mut file_norm = file.replace("\r\n", "\n");
            if !file_norm.ends_with('\n') {
                file_norm.push('\n');
            }
            assert_eq!(
                systemd_unit_content(),
                file_norm,
                "embedded systemd unit must stay synchronized with packaging/systemd/greggd.service"
            );
        }
    }

    #[test]
    fn embedded_launchd_plist_matches_packaging_file_when_present() {
        let packaging = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/launchd/com.eggstack.greggd.plist");
        if let Ok(file) = std::fs::read_to_string(&packaging) {
            let mut file_norm = file.replace("\r\n", "\n");
            if !file_norm.ends_with('\n') {
                file_norm.push('\n');
            }
            assert_eq!(
                launchd_plist_content(),
                file_norm,
                "embedded launchd plist must stay synchronized with packaging/launchd/com.eggstack.greggd.plist"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn bounded_manager_command_captures_stderr_and_kills_on_timeout() {
        let output = run_bounded_command(
            "sh",
            &["-c", "printf stdout; printf denied >&2; exit 7"],
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, b"stdout");
        assert_eq!(output.stderr, b"denied");

        let error = run_bounded_command("sh", &["-c", "sleep 1"], Duration::from_millis(40))
            .expect_err("slow manager command must time out");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn manager_permission_text_is_classified() {
        assert!(manager_error_is_permission(
            "Interactive authentication required"
        ));
        assert!(manager_error_is_permission("Access denied"));
        assert!(!manager_error_is_permission("unit failed"));
    }
}
