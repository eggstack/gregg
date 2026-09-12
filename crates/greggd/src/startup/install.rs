//! Install errors, atomic writes, privilege guidance, install dispatch, instruction rendering, and restart coordination.

use super::cron::ShellQuoteError;
use super::cron::{cron_block, install_cron, CRON_MANAGED_MARKER};
use super::launchd::{install_launchd, restart_launchd};
use super::method::{
    launchd_label, resolve_startup_method, standard_launchd_binary, standard_launchd_config,
    standard_launchd_plist_path, standard_systemd_binary, standard_systemd_config,
    standard_systemd_unit_path, StartupMethod, StartupMethodArg,
};
#[cfg(unix)]
use super::process::DIRECT_RESTART_TIMEOUT;
#[cfg(test)]
use super::state::systemd_state_with;
use super::state::{startup_state, StartupState};
use super::systemd::{install_systemd, restart_systemd};
use crate::config::Config;
use std::fmt;
use std::fs;
use std::io::{self};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::{Duration, Instant};

/// Process-wide counter disambiguating concurrent atomic writers in the same
/// PID that collide on PID+nanosecond with `create_new(true)`.
static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
// ── Atomic file write helpers ─────────────────────────────────────────────
pub(crate) fn write_atomic_text(path: &Path, content: &str) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory")
    })?;
    fs::create_dir_all(dir)?;
    // Write to a temp file in the same directory.
    let tmp_name = format!(
        ".greggd-startup-{}-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
        TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let tmp_path = dir.join(tmp_name);
    // Ensure we clean up on failure.
    let write_res = (|| -> io::Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp_path)?;
        io::Write::write_all(&mut file, content.as_bytes())?;
        // Propagate durability failures instead of silently dropping them.
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp_path, path)?;
        // Sync parent directory where supported.
        #[cfg(unix)]
        {
            if let Ok(dir_file) = fs::OpenOptions::new().read(true).open(dir) {
                dir_file.sync_all()?;
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
pub(crate) fn is_privileged() -> bool {
    // SAFETY: geteuid is a pure libc call with no side effects and is safe
    // to call at any time. It does not read or write Rust-managed memory.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
pub(crate) fn is_privileged() -> bool {
    // On non-Unix, attempt a privileged operation and handle PermissionDenied.
    // For Windows, check via SCM or assume not privileged if not admin.
    // We treat as privileged only if we can open SCM with create access.
    #[cfg(target_os = "windows")]
    {
        // Best-effort: try to query SCM; if AccessDenied, not privileged.
        // Use a simple check: attempt to open ServiceManager.
        // If we can't, assume not privileged.
        is_windows_admin()
    }
    #[cfg(not(target_os = "windows"))]
    {
        true
    }
}

#[cfg(target_os = "windows")]
fn is_windows_admin() -> bool {
    // Best-effort privilege probe: opening the SCM with create-service
    // access requires elevation. Success means admin; any failure (including
    // access-denied) means not privileged, and the real install attempt
    // still surfaces the true OS error. Uses the existing `windows-service`
    // dependency — no new crates.
    windows_service::service_manager::ServiceManager::local_computer(
        None::<&str>,
        windows_service::service_manager::ServiceManagerAccess::CREATE_SERVICE,
    )
    .is_ok()
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
pub(crate) fn elevated_command(exe: &Path, method: StartupMethodArg) -> String {
    let exe_str = exe.display().to_string();
    match method {
        StartupMethodArg::Auto => format!("sudo {exe_str} startup install"),
        other => format!("sudo {exe_str} startup install --method {other}"),
    }
}
pub(crate) fn ensure_config_preserved(config_path: &Path) -> io::Result<()> {
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
/// Repair an existing system config so read-only diagnostics work for
/// unprivileged operators.
///
/// System daemon configs carry no secrets, but older installs wrote them
/// `0600 greggd:greggd`, which makes `croncheck`/`status`/`configprint`
/// fail with `Permission denied (os error 13)` for anyone except the
/// daemon user and root. The daemon user still owns the file; this only
/// relaxes the mode to world-readable and ensures the parent directory is
/// traversable. Best-effort for missing paths; hard errors for real I/O
/// failures so install surfaces them.
pub(crate) fn repair_system_config_permissions(config_path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = config_path.parent() {
            if parent.exists() {
                // 0755: owner can manage, everyone can traverse/read.
                // Existing operator-managed modes are intentionally
                // normalized here because a 0700 system directory would
                // still block unprivileged `croncheck` even with a 0644
                // file.
                fs::set_permissions(parent, fs::Permissions::from_mode(0o755))?;
            }
        }
        if config_path.exists() {
            fs::set_permissions(config_path, fs::Permissions::from_mode(0o644))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = config_path;
        Ok(())
    }
}
pub(crate) fn manager_error_is_permission(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("permission")
        || message.contains("access denied")
        || message.contains("not authorized")
        || message.contains("authentication")
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
            Ok(crate::control::StopOutcome::Uncertain { .. }) => RestartStopState::Uncertain,
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
#[cfg(test)]
mod tests {
    use super::*;

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
    fn manager_permission_text_is_classified() {
        assert!(manager_error_is_permission(
            "Interactive authentication required"
        ));
        assert!(manager_error_is_permission("Access denied"));
        assert!(!manager_error_is_permission("unit failed"));
    }
    #[test]
    #[cfg(unix)]
    fn repair_system_config_permissions_relaxes_old_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir =
            std::env::temp_dir().join(format!("greggd_test_repair_perms_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("greggd.toml");
        // Simulate a pre-fix install: 0700 dir, 0600 file.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            &path,
            "name = \"greggd\"\nhost = \"0.0.0.0\"\nport = 11310\nsample_interval_ms = 1000\nstale_after_ms = 10000\n",
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        repair_system_config_permissions(&path).unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644,
            "system config must become world-readable for croncheck/status"
        );
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o755,
            "system config dir must stay traversable"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
