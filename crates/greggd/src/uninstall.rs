//! Component-safe uninstall for the `greggd` daemon (Plan 112).
//!
//! `greggd uninstall` removes only the exact invoked daemon executable
//! plus the Gregg-owned startup integration actually present for it.
//! Configuration is preserved by default; `--purge` additionally removes
//! the resolved daemon config (and the macOS daemon log). `--dry-run`
//! plans without mutating anything.
//!
//! Ownership rules:
//!
//! - Startup artifacts are discovered independently (a stale systemd unit
//!   plus a managed cron block are both removed); auto-detection alone
//!   never hides an artifact.
//! - Only canonical Gregg identities are addressed (`greggd` unit,
//!   `com.eggstack.greggd` label, `# greggd managed watchdog` block,
//!   `greggd` SCM registration). No process-name scanning, no `pkill`,
//!   no PID files, no other users' crontabs, no recursive directory
//!   removal, no user/group deletion.
//! - Permissions are preflighted before any teardown mutation; nothing
//!   invokes `sudo` internally.
//! - An uncertain direct-stop outcome blocks binary deletion rather than
//!   orphaning a running daemon.

use std::fmt;
use std::path::{Path, PathBuf};

/// Identity constants for the daemon uninstall path.
pub const PROGRAM: &str = "greggd";
pub const PACKAGE: &str = "greggd";

/// macOS daemon log created by the installer/startup path; removed only
/// under `--purge` on macOS.
#[cfg(target_os = "macos")]
const MACOS_DAEMON_LOG: &str = "/var/log/greggd.log";

/// Errors returned by daemon uninstall planning and execution.
///
/// Library code returns these without printing or exiting; the binary
/// boundary maps them to [`crate::cli::ExitCode`].
#[derive(Debug)]
pub enum UninstallError {
    /// The current executable path could not be determined.
    CurrentExe(String),
    /// A filesystem mutation failed.
    Io { path: PathBuf, message: String },
    /// The operation needs elevation; the message names the exact rerun.
    Permission { message: String },
    /// A service-manager teardown step failed.
    Service { message: String },
    /// Stopping the daemon had an uncertain outcome; deletion is blocked.
    UncertainStop { message: String },
    /// Cron teardown failed.
    Cron { message: String },
    /// The installation is Cargo-owned and Cargo must perform the removal.
    CargoHandoff {
        /// Exact command for the operator to run after this process exits.
        command: String,
    },
    /// The Cargo-delegated removal failed.
    CargoFailed(String),
}

impl fmt::Display for UninstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentExe(detail) => {
                write!(f, "failed to determine current executable: {detail}")
            }
            Self::Io { path, message } => write!(f, "{}: {message}", path.display()),
            Self::Permission { message } => write!(f, "permission denied: {message}"),
            Self::Service { message } => write!(f, "service teardown failed: {message}"),
            Self::UncertainStop { message } => {
                write!(f, "refusing to delete the binary: {message}")
            }
            Self::Cron { message } => write!(f, "cron teardown failed: {message}"),
            Self::CargoHandoff { command } => write!(
                f,
                "this installation is Cargo-owned; Gregg will not bypass Cargo bookkeeping. No startup or config mutation will occur before the Cargo handoff completes. Run after this process exits:\n  {command}"
            ),
            Self::CargoFailed(detail) => write!(f, "cargo uninstall failed: {detail}"),
        }
    }
}

impl std::error::Error for UninstallError {}

impl From<gregg_update::UpdateError> for UninstallError {
    fn from(error: gregg_update::UpdateError) -> Self {
        match error {
            gregg_update::UpdateError::PermissionDenied { message, elevated } => Self::Permission {
                message: format!("{message}. Rerun: {elevated}"),
            },
            gregg_update::UpdateError::CurrentExe(detail) => Self::CurrentExe(detail),
            other => Self::CargoFailed(other.to_string()),
        }
    }
}

impl From<crate::startup::InstallError> for UninstallError {
    fn from(error: crate::startup::InstallError) -> Self {
        match error {
            crate::startup::InstallError::Permission { message } => Self::Permission { message },
            crate::startup::InstallError::CrontabUnavailable { message } => Self::Cron { message },
            other => Self::Service {
                message: other.to_string(),
            },
        }
    }
}

/// Windows SCM discovery for the uninstall plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScmDiscovery {
    /// Non-Windows platform; SCM plays no role.
    NotApplicable,
    /// No `greggd` registration.
    NotInstalled,
    /// Registered but stopped.
    Stopped,
    /// Registered and running.
    Running,
    /// The query itself failed; teardown surfaces the detail.
    Unknown,
}

/// Read-only discovery of every Gregg-owned artifact relevant to the
/// selected daemon component.
///
/// Each artifact is inspected independently so legacy/mixed states (a
/// stale unit plus a managed cron block) are all found. Discovery
/// performs no mutation: bounded health/crontab/manager queries only.
/// Presence, ownership, and manager state are retained independently so
/// mixed installations cannot be collapsed into one host-global decision.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct Discovery {
    /// Exact invoked executable.
    pub exe_path: PathBuf,
    /// Canonical systemd unit exists (Linux).
    pub systemd_unit_exists: bool,
    /// Manager reports `greggd` active (Linux).
    pub systemd_active: bool,
    /// Ownership of the systemd unit target relative to `exe_path`.
    pub systemd_ownership: crate::startup::ArtifactOwnership,
    /// Config path used by the discovered systemd command, when parseable.
    pub systemd_config_path: Option<PathBuf>,
    /// Canonical launchd plist exists (macOS).
    pub launchd_plist_exists: bool,
    /// Gregg launchd label is loaded (macOS).
    pub launchd_loaded: bool,
    /// Ownership of the launchd program target relative to `exe_path`.
    pub launchd_ownership: crate::startup::ArtifactOwnership,
    /// Config path used by the discovered launchd command, when parseable.
    pub launchd_config_path: Option<PathBuf>,
    /// Current account crontab contains the Gregg managed block.
    pub cron_has_block: bool,
    /// Ownership of the managed cron block relative to `exe_path`.
    pub cron_ownership: crate::startup::ArtifactOwnership,
    /// The `crontab` executable is available.
    pub crontab_available: bool,
    /// Bounded health classification of the configured local endpoint.
    pub daemon_probe: crate::cli::HealthProbe,
    /// Windows SCM state.
    pub scm: ScmDiscovery,
    /// Ownership of the registered SCM image relative to `exe_path`.
    pub scm_ownership: crate::startup::ArtifactOwnership,
    /// Registered SCM executable path, when queryable.
    pub scm_target: Option<PathBuf>,
    /// Positively identified Cargo ownership, if any.
    pub cargo: Option<gregg_update::CargoOwnership>,
}

impl Discovery {
    /// Whether any native manager artifact owns (or may own) the daemon.
    #[must_use]
    pub fn has_manager_artifact(&self) -> bool {
        self.systemd_ownership.is_present()
            || self.launchd_ownership.is_present()
            || self.cron_ownership.is_present()
            || self.scm_ownership.is_present()
    }

    /// Whether an active manager is known to explain the selected endpoint.
    #[must_use]
    pub fn manager_explains_endpoint(&self, config_path: &Path) -> bool {
        let systemd = self.systemd_active
            && manager_config_matches(
                self.systemd_ownership,
                self.systemd_config_path.as_deref(),
                config_path,
            );
        let launchd = self.launchd_loaded
            && manager_config_matches(
                self.launchd_ownership,
                self.launchd_config_path.as_deref(),
                config_path,
            );
        // The SCM query currently exposes the registered image, but not a
        // safely parsed config argument. A foreign service therefore cannot
        // be credited with explaining this endpoint; doing so could hide an
        // unmanaged foreground daemon on Windows.
        let scm = matches!(self.scm, ScmDiscovery::Running)
            && self.scm_ownership == crate::startup::ArtifactOwnership::Owned;
        systemd || launchd || scm
    }

    #[cfg(unix)]
    #[must_use]
    fn active_manager_is_unknown(&self) -> bool {
        (self.systemd_active
            && self.systemd_ownership == crate::startup::ArtifactOwnership::Unknown)
            || (self.launchd_loaded
                && self.launchd_ownership == crate::startup::ArtifactOwnership::Unknown)
    }

    /// Whether the configured endpoint proves a Gregg daemon is running.
    #[must_use]
    pub fn daemon_is_running(&self) -> bool {
        matches!(
            self.daemon_probe,
            crate::cli::HealthProbe::Ready
                | crate::cli::HealthProbe::Warming
                | crate::cli::HealthProbe::Failed
        )
    }
}

/// Deterministic uninstall plan: the exact resources `uninstall` will
/// change or remove.
///
/// The same plan drives `--dry-run` rendering and real execution so the
/// preview cannot drift from the mutation. Teardown flags are selected only
/// from positively owned artifacts; foreign and unknown artifacts remain
/// visible in the rendered plan but are never removed.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct UninstallPlan {
    /// Exact invoked executable to delete (never a directory).
    pub exe_path: PathBuf,
    /// Resolved daemon config file (`--config` or platform default).
    pub config_path: PathBuf,
    /// Whether `--config` was explicit.
    pub explicit_config: bool,
    /// Whether the config file currently exists.
    pub config_exists: bool,
    /// Whether `--purge` was requested.
    pub purge: bool,
    /// Remove systemd integration (unit exists or service active).
    pub systemd_teardown: bool,
    /// Remove launchd integration (plist exists or label loaded).
    pub launchd_teardown: bool,
    /// Remove the managed cron block.
    pub cron_teardown: bool,
    /// Stop/delete the Windows SCM registration.
    pub scm_teardown: bool,
    /// Stop the daemon via the direct control path (no manager owns it,
    /// but the endpoint proves it runs under this config identity).
    pub direct_stop: bool,
    /// Refuse deletion: a running daemon with no safe stop path
    /// (Windows unmanaged foreground instance).
    pub blocked_running: bool,
    /// Refuse before mutation because SCM ownership could not be established.
    pub blocked_scm_unknown: bool,
    /// Positively identified Cargo ownership, if any.
    pub cargo: Option<gregg_update::CargoOwnership>,
    /// Discovery detail retained for rendering.
    pub discovery: Discovery,
}

impl UninstallPlan {
    /// Render the exact Gregg-owned resources that would be changed or
    /// removed, one per line. Pure and side-effect free.
    #[must_use]
    pub fn render(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!("uninstall {PROGRAM} (dry run)"));
        if let Some(ownership) = &self.cargo {
            #[cfg(unix)]
            lines.push(format!(
                "cargo-owned install (root {}); Cargo removes the executable/package after Gregg-owned lifecycle work; handoff: {}",
                ownership.root.display(),
                ownership.uninstall_command()
            ));
            #[cfg(not(unix))]
            lines.push(format!(
                "cargo-owned install (root {}); Windows uses a zero-mutation Cargo handoff before any startup/config change; handoff: {}",
                ownership.root.display(),
                ownership.uninstall_command()
            ));
        }
        render_ownership(&mut lines, "systemd", self.discovery.systemd_ownership);
        render_ownership(&mut lines, "launchd", self.discovery.launchd_ownership);
        render_ownership(&mut lines, "cron", self.discovery.cron_ownership);
        render_ownership(&mut lines, "scm", self.discovery.scm_ownership);
        if self.blocked_scm_unknown {
            lines.push("blocked: Windows SCM registration state or executable target is unknown; no mutation is safe".to_string());
            return lines.join("\n");
        }
        if self.blocked_running {
            lines.push(
                "blocked: a Gregg daemon is running on the configured endpoint with no managed stop path; stop it first, then rerun uninstall"
                    .to_string(),
            );
            return lines.join("\n");
        }
        if self.systemd_teardown {
            lines.push("systemd: stop/disable greggd, remove /etc/systemd/system/greggd.service, daemon-reload".to_string());
        }
        if self.launchd_teardown {
            lines.push("launchd: boot out system/com.eggstack.greggd when loaded, remove /Library/LaunchDaemons/com.eggstack.greggd.plist".to_string());
        }
        if self.cron_teardown {
            lines.push(
                "cron: remove the `# greggd managed watchdog` block (unrelated entries preserved)"
                    .to_string(),
            );
        }
        if self.scm_teardown {
            lines.push(
                "scm: stop greggd when running, delete only the greggd registration".to_string(),
            );
        }
        if self.direct_stop {
            lines.push(format!(
                "direct: stop the running daemon for {} via the local control socket",
                self.config_path.display()
            ));
        }
        if !self.systemd_teardown
            && !self.launchd_teardown
            && !self.cron_teardown
            && !self.scm_teardown
            && !self.direct_stop
        {
            lines.push("no startup integration found (nothing to stop)".to_string());
        }
        lines.push(format!("remove executable: {}", self.exe_path.display()));
        if self.purge {
            if self.config_exists {
                lines.push(format!(
                    "remove config file: {}",
                    self.config_path.display()
                ));
                if let Some(dir) = purge_empty_dir_for(&self.config_path) {
                    lines.push(format!(
                        "remove empty Gregg config directory: {}",
                        dir.display()
                    ));
                }
            } else {
                lines.push(format!(
                    "config file already absent: {}",
                    self.config_path.display()
                ));
            }
            #[cfg(target_os = "macos")]
            lines.push(format!("remove daemon log file: {MACOS_DAEMON_LOG}"));
        } else {
            lines.push(format!("preserve config: {}", self.config_path.display()));
        }
        lines.join("\n")
    }
}

fn render_ownership(
    lines: &mut Vec<String>,
    artifact: &str,
    ownership: crate::startup::ArtifactOwnership,
) {
    match ownership {
        crate::startup::ArtifactOwnership::Foreign => {
            lines.push(format!("{artifact}: foreign installation preserved"));
        }
        crate::startup::ArtifactOwnership::Unknown => {
            lines.push(format!(
                "{artifact}: ownership unknown; preserved for safety"
            ));
        }
        crate::startup::ArtifactOwnership::Absent | crate::startup::ArtifactOwnership::Owned => {}
    }
}

fn manager_config_matches(
    ownership: crate::startup::ArtifactOwnership,
    manager_config: Option<&Path>,
    selected_config: &Path,
) -> bool {
    if !matches!(
        ownership,
        crate::startup::ArtifactOwnership::Owned | crate::startup::ArtifactOwnership::Foreign
    ) {
        return false;
    }
    manager_config.map_or(true, |config| {
        gregg_update::uninstall::paths_equivalent(config, selected_config)
    })
}

/// Build a plan from injected discovery (pure; shared by production and
/// tests so `--dry-run` and unit tests exercise the same ownership
/// decisions as the real command).
#[must_use]
pub fn plan_from_discovery(
    exe_path: &Path,
    config_path: &Path,
    explicit_config: bool,
    config_exists: bool,
    purge: bool,
    discovery: Discovery,
) -> UninstallPlan {
    let systemd_teardown = discovery.systemd_ownership.is_owned();
    let launchd_teardown = discovery.launchd_ownership.is_owned();
    let cron_teardown = discovery.cron_ownership.is_owned();
    let scm_teardown = discovery.scm_ownership.is_owned()
        && matches!(discovery.scm, ScmDiscovery::Running | ScmDiscovery::Stopped);
    let blocked_scm_unknown = discovery.scm_ownership == crate::startup::ArtifactOwnership::Unknown;
    // The direct control path exists only on Unix. Elsewhere a running
    // daemon with no manager artifact cannot be stopped safely: block
    // deletion rather than orphaning it.
    #[cfg(unix)]
    let direct_stop = discovery.daemon_is_running()
        && !discovery.manager_explains_endpoint(config_path)
        && !blocked_scm_unknown
        && !discovery.active_manager_is_unknown();
    #[cfg(not(unix))]
    let direct_stop = false;
    #[cfg(unix)]
    let blocked_running = discovery.daemon_is_running()
        && (blocked_scm_unknown || discovery.active_manager_is_unknown());
    #[cfg(not(unix))]
    let blocked_running =
        discovery.daemon_is_running() && !discovery.manager_explains_endpoint(config_path);
    UninstallPlan {
        exe_path: exe_path.to_path_buf(),
        config_path: config_path.to_path_buf(),
        explicit_config,
        config_exists,
        purge,
        systemd_teardown,
        launchd_teardown,
        cron_teardown,
        scm_teardown,
        direct_stop,
        blocked_running,
        blocked_scm_unknown,
        cargo: discovery.cargo.clone(),
        discovery,
    }
}

/// Perform read-only discovery for the running binary and resolved
/// config: manager artifacts, cron block, endpoint health, SCM state,
/// and Cargo ownership. Mutates nothing.
pub fn discover(config_path: &Path, explicit: bool) -> Result<Discovery, UninstallError> {
    let exe_path = gregg_update::uninstall::resolve_uninstall_target()
        .map_err(|e| UninstallError::CurrentExe(e.to_string()))?;
    Ok(discover_for(&exe_path, config_path, explicit))
}

/// Discovery for an explicit executable path (production passes the
/// real current exe; tests inject fakes).
fn discover_for(exe_path: &Path, config_path: &Path, explicit: bool) -> Discovery {
    #[cfg(target_os = "linux")]
    let (systemd_ownership, systemd_active, systemd_config_path) = {
        let (ownership, active, config) = crate::startup::systemd_artifact_ownership(exe_path);
        (ownership, active, config)
    };
    #[cfg(not(target_os = "linux"))]
    let (systemd_ownership, systemd_active, systemd_config_path) =
        (crate::startup::ArtifactOwnership::Absent, false, None);
    let systemd_unit_exists = crate::startup::standard_systemd_unit_path().exists();

    #[cfg(target_os = "macos")]
    let (launchd_ownership, launchd_loaded, launchd_config_path) =
        crate::startup::launchd_artifact_ownership(exe_path);
    #[cfg(not(target_os = "macos"))]
    let (launchd_ownership, launchd_loaded, launchd_config_path) =
        (crate::startup::ArtifactOwnership::Absent, false, None);
    let launchd_plist_exists = crate::startup::standard_launchd_plist_path().exists();

    let (cron_has_block, cron_ownership, crontab_available) = read_cron_evidence(exe_path);

    let daemon_probe = match crate::cli::load_config(config_path, explicit) {
        Ok(config) => crate::cli::probe_health(crate::cli::croncheck_target(&config)),
        // An unloadable config cannot prove a running daemon; teardown of
        // file artifacts still proceeds, and purge still applies to the
        // resolved path when it exists.
        Err(_) => crate::cli::HealthProbe::Unreachable,
    };

    #[cfg(target_os = "windows")]
    let (scm, scm_ownership, scm_target) =
        match crate::service::platform_service_manager().query_registration() {
            Ok(registration) => {
                let state = match registration.state {
                    crate::service::ServiceState::NotInstalled => ScmDiscovery::NotInstalled,
                    crate::service::ServiceState::Stopped => ScmDiscovery::Stopped,
                    crate::service::ServiceState::Running
                    | crate::service::ServiceState::StartPending
                    | crate::service::ServiceState::StopPending => ScmDiscovery::Running,
                };
                let ownership = registration.executable_path.as_deref().map_or(
                    crate::startup::ArtifactOwnership::Absent,
                    |target| {
                        if gregg_update::uninstall::paths_equivalent(target, exe_path) {
                            crate::startup::ArtifactOwnership::Owned
                        } else {
                            crate::startup::ArtifactOwnership::Foreign
                        }
                    },
                );
                (state, ownership, registration.executable_path)
            }
            Err(_) => (
                ScmDiscovery::Unknown,
                crate::startup::ArtifactOwnership::Unknown,
                None,
            ),
        };
    #[cfg(not(target_os = "windows"))]
    let (scm, scm_ownership, scm_target) = (
        ScmDiscovery::NotApplicable,
        crate::startup::ArtifactOwnership::Absent,
        None,
    );

    let cargo = gregg_update::uninstall::detect_cargo_ownership(exe_path, PROGRAM, PACKAGE);

    Discovery {
        exe_path: exe_path.to_path_buf(),
        systemd_unit_exists,
        systemd_active,
        systemd_ownership,
        systemd_config_path,
        launchd_plist_exists,
        launchd_loaded,
        launchd_ownership,
        launchd_config_path,
        cron_has_block,
        cron_ownership,
        crontab_available,
        daemon_probe,
        scm,
        scm_ownership,
        scm_target,
        cargo,
    }
}

/// Read-only cron evidence: whether the current account crontab holds
/// the Gregg managed block, and whether `crontab` exists at all.
fn read_cron_evidence(exe_path: &Path) -> (bool, crate::startup::ArtifactOwnership, bool) {
    use std::io::ErrorKind;
    match crate::startup::cron::run_crontab_list() {
        Ok(content) => (
            content.contains(crate::startup::CRON_MANAGED_MARKER),
            crate::startup::cron_block_ownership(&content, exe_path),
            true,
        ),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            (false, crate::startup::ArtifactOwnership::Absent, false)
        }
        Err(_) => (false, crate::startup::ArtifactOwnership::Unknown, true),
    }
}

/// Discovery builder for tests: every artifact independently settable so
/// mixed stale states are expressible without touching the host.
#[cfg(test)]
fn test_discovery() -> Discovery {
    Discovery {
        exe_path: PathBuf::from("/usr/local/bin/greggd"),
        systemd_unit_exists: false,
        systemd_active: false,
        systemd_ownership: crate::startup::ArtifactOwnership::Absent,
        systemd_config_path: None,
        launchd_plist_exists: false,
        launchd_loaded: false,
        launchd_ownership: crate::startup::ArtifactOwnership::Absent,
        launchd_config_path: None,
        cron_has_block: false,
        cron_ownership: crate::startup::ArtifactOwnership::Absent,
        crontab_available: true,
        daemon_probe: crate::cli::HealthProbe::Unreachable,
        scm: ScmDiscovery::NotApplicable,
        scm_ownership: crate::startup::ArtifactOwnership::Absent,
        scm_target: None,
        cargo: None,
    }
}

/// When `--purge` removes the config file, the Gregg-specific parent
/// directory may additionally be removed, but only when it is one of
/// Gregg's known standard directories and empty afterwards.
///
/// Returns the directory that *would* be removed (the caller re-checks
/// emptiness after deleting the file). Returns `None` for custom
/// `--config` paths: an arbitrary explicit parent is never removed.
fn purge_empty_dir_for(config_path: &Path) -> Option<PathBuf> {
    let parent = config_path.parent()?;
    let standard_parent = crate::config::Config::default_path()
        .parent()
        .map(Path::to_path_buf)?;
    if parent == standard_parent {
        Some(parent.to_path_buf())
    } else {
        None
    }
}

/// Probe that `dir` accepts a temporary file (practical writability
/// preflight for artifact removal).
fn probe_dir_writable(dir: &Path, exe: &Path) -> Result<(), UninstallError> {
    let probe = dir.join(format!(
        ".greggd-uninstall-probe-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    ));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(UninstallError::Permission {
                message: format!(
                    "permission denied writing to {}; rerun as root: sudo {} uninstall",
                    dir.display(),
                    exe.display()
                ),
            })
        }
        Err(e) => Err(UninstallError::Io {
            path: dir.to_path_buf(),
            message: format!("permission probe failed: {e}"),
        }),
    }
}

/// Preflight every practical permission/path requirement before mutating
/// daemon/service state: the executable, service artifacts that will be
/// removed, config/data paths under `--purge`, and cron availability
/// when a managed block exists.
fn preflight(plan: &UninstallPlan) -> Result<(), UninstallError> {
    use crate::startup::{standard_launchd_plist_path, standard_systemd_unit_path};

    if plan.blocked_running || plan.blocked_scm_unknown {
        // Cargo handoff and blocked-running fail before any mutation in
        // `run_uninstall`; preflight has nothing to check for them.
        return Ok(());
    }
    gregg_update::preflight_uninstall_writable(&plan.exe_path, &plan.exe_path, plan.purge)?;
    if plan.systemd_teardown {
        if let Some(parent) = standard_systemd_unit_path().parent() {
            probe_dir_writable(parent, &plan.exe_path)?;
        }
    }
    if plan.launchd_teardown {
        if let Some(parent) = standard_launchd_plist_path().parent() {
            probe_dir_writable(parent, &plan.exe_path)?;
        }
    }
    if plan.cron_teardown && !plan.discovery.crontab_available {
        return Err(UninstallError::Cron {
            message: "the Gregg managed cron block exists but `crontab` is unavailable; remove the block manually, then rerun uninstall".to_string(),
        });
    }
    if plan.purge && plan.config_exists {
        if let Some(parent) = plan.config_path.parent() {
            if !parent.as_os_str().is_empty() {
                probe_dir_writable(parent, &plan.exe_path)?;
            }
        }
    }
    Ok(())
}

/// Outcome of the direct-stop decision: proceed to deletion or block.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectStopDecision {
    Proceed,
    Block,
}

/// Pure helper: map a direct control-stop outcome to the deletion gate.
///
/// `Stopped`/`NotRunning` proceed; `Uncertain` and transport errors
/// block self-deletion rather than orphaning a possibly-running daemon.
#[cfg(unix)]
fn decide_direct_stop(
    outcome: &Result<crate::control::StopOutcome, crate::control::ControlError>,
) -> DirectStopDecision {
    match outcome {
        Ok(
            crate::control::StopOutcome::Stopped { .. } | crate::control::StopOutcome::NotRunning,
        ) => DirectStopDecision::Proceed,
        Ok(crate::control::StopOutcome::Uncertain { .. }) | Err(_) => DirectStopDecision::Block,
    }
}

/// Stop the daemon through the existing direct control path and gate
/// binary deletion on a definite outcome.
#[cfg(unix)]
fn direct_stop_and_gate(plan: &UninstallPlan) -> Result<(), UninstallError> {
    let outcome = crate::control::send_stop(&plan.config_path);
    match decide_direct_stop(&outcome) {
        DirectStopDecision::Proceed => {
            if matches!(outcome, Ok(crate::control::StopOutcome::Stopped { .. })) {
                wait_for_endpoint_absence(plan)?;
            }
            Ok(())
        }
        DirectStopDecision::Block => Err(UninstallError::UncertainStop {
            message: match outcome {
                Ok(crate::control::StopOutcome::Uncertain { detail }) => format!(
                    "stop outcome for {} was uncertain ({detail}); the daemon may still be running",
                    plan.config_path.display()
                ),
                Err(e) => format!(
                    "stop for {} failed ({e}); the daemon may still be running",
                    plan.config_path.display()
                ),
                _ => "stop outcome was uncertain".to_string(),
            },
        }),
    }
}

/// After a direct stop, wait until the configured endpoint is definitely
/// absent (bounded) so deletion cannot orphan a shutting-down daemon.
#[cfg(unix)]
fn wait_for_endpoint_absence(plan: &UninstallPlan) -> Result<(), UninstallError> {
    use std::time::{Duration, Instant};
    let config = crate::cli::load_config(&plan.config_path, plan.explicit_config).map_err(|e| {
        UninstallError::Io {
            path: plan.config_path.clone(),
            message: format!("failed to reload config after stop: {e}"),
        }
    })?;
    let target = crate::cli::croncheck_target(&config);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match crate::cli::probe_greggd(target) {
            crate::cli::CroncheckProbe::Absent => return Ok(()),
            _ if Instant::now() >= deadline => {
                return Err(UninstallError::UncertainStop {
                    message: "the daemon did not release its endpoint after stop; it may still be running".to_string(),
                });
            }
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Remove the resolved daemon config file for `--purge`, plus the
/// Gregg-specific parent directory when standard and empty, plus the
/// macOS daemon log on macOS.
///
/// Removes only the exact file; never recurses into an arbitrary
/// explicit config parent.
fn purge_config(plan: &UninstallPlan) -> Result<(), UninstallError> {
    match std::fs::remove_file(&plan.config_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(UninstallError::Permission {
                message: format!(
                    "permission denied removing {}; rerun as root: sudo {} uninstall --purge",
                    plan.config_path.display(),
                    plan.exe_path.display()
                ),
            });
        }
        Err(e) => {
            return Err(UninstallError::Io {
                path: plan.config_path.clone(),
                message: format!("failed to remove config file: {e}"),
            });
        }
    }
    if plan.config_exists {
        if let Some(dir) = purge_empty_dir_for(&plan.config_path) {
            // Optional cleanup: never required for success.
            let _ = std::fs::remove_dir(&dir);
        }
    }
    #[cfg(target_os = "macos")]
    {
        match std::fs::remove_file(MACOS_DAEMON_LOG) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(UninstallError::Permission {
                    message: format!(
                        "permission denied removing {MACOS_DAEMON_LOG}; rerun as root: sudo {} uninstall --purge",
                        plan.exe_path.display()
                    ),
                });
            }
            Err(e) => {
                return Err(UninstallError::Io {
                    path: PathBuf::from(MACOS_DAEMON_LOG),
                    message: format!("failed to remove daemon log: {e}"),
                });
            }
        }
    }
    Ok(())
}

/// Run the full `greggd uninstall` flow synchronously.
///
/// Discovers the plan, prints it for `--dry-run` without mutating,
/// otherwise preflights, tears down startup integration, stops the
/// daemon through the safe path for its supervision, purges config
/// under `--purge`, and deletes the exact invoked executable. Prints
/// progress to stderr; the caller exits promptly so Windows deferred
/// self-delete can complete.
pub fn run_uninstall(
    config_path: &Path,
    explicit: bool,
    dry_run: bool,
    purge: bool,
) -> Result<(), UninstallError> {
    let discovery = discover(config_path, explicit)?;
    let exe_path = discovery.exe_path.clone();
    let plan = plan_from_discovery(
        &exe_path,
        config_path,
        explicit,
        config_path.exists(),
        purge,
        discovery,
    );
    if dry_run {
        println!("{}", plan.render());
        return Ok(());
    }
    execute_plan(&plan)
}

/// Execute a fully resolved plan (production resolves via
/// [`run_uninstall`]; tests inject plans directly).
fn execute_plan(plan: &UninstallPlan) -> Result<(), UninstallError> {
    #[cfg(not(unix))]
    if let Some(ownership) = &plan.cargo {
        return Err(UninstallError::CargoHandoff {
            command: ownership.uninstall_command(),
        });
    }
    if plan.blocked_running {
        return Err(UninstallError::UncertainStop {
            message: "a Gregg daemon is running on the configured endpoint with no managed stop path; stop the foreground daemon first, then rerun uninstall"
                .to_string(),
        });
    }
    if plan.blocked_scm_unknown {
        return Err(UninstallError::Service {
            message: "Windows SCM registration state or executable target could not be determined; refusing mutation".to_string(),
        });
    }
    // Every practical permission is proven before any teardown mutation.
    preflight(plan)?;

    if plan.systemd_teardown {
        crate::startup::uninstall_systemd(&plan.exe_path)?;
    }
    if plan.launchd_teardown {
        crate::startup::uninstall_launchd(&plan.exe_path)?;
    }
    if plan.cron_teardown {
        crate::startup::uninstall_cron_for(&plan.exe_path)?;
    }
    #[cfg(target_os = "windows")]
    if plan.scm_teardown {
        crate::service::platform_service_manager()
            .unregister()
            .map_err(|e| match e {
                crate::service::ServiceError::AccessDenied => UninstallError::Permission {
                    message: format!(
                        "access denied removing the greggd service; rerun {} from an Administrator shell",
                        plan.exe_path.display()
                    ),
                },
                other => UninstallError::Service {
                    message: other.to_string(),
                },
            })?;
    }
    #[cfg(unix)]
    if plan.direct_stop {
        direct_stop_and_gate(plan)?;
    }
    // A managed stop above must have released the endpoint; a daemon
    // still answering afterwards blocks deletion rather than orphaning.
    if !plan.direct_stop && plan.discovery.daemon_is_running() && managed_stop_performed(plan) {
        let still_running = match crate::cli::load_config(&plan.config_path, plan.explicit_config) {
            Ok(config) => {
                let target = crate::cli::croncheck_target(&config);
                matches!(
                    crate::cli::probe_health(target),
                    crate::cli::HealthProbe::Ready
                        | crate::cli::HealthProbe::Warming
                        | crate::cli::HealthProbe::Failed
                )
            }
            Err(_) => false,
        };
        if still_running {
            return Err(UninstallError::UncertainStop {
                message: "the daemon still answers on its endpoint after managed shutdown; it may still be running"
                    .to_string(),
            });
        }
    }

    #[cfg(unix)]
    if let Some(ownership) = &plan.cargo {
        gregg_update::uninstall::cargo_uninstall(ownership)
            .map_err(|e| UninstallError::CargoFailed(e.to_string()))?;
        eprintln!("uninstalled via {}", ownership.uninstall_command());
        if plan.purge {
            purge_config(plan)?;
        }
    } else {
        if plan.purge {
            purge_config(plan)?;
        }
        gregg_update::self_delete_current_exe(plan.purge)?;
    }
    #[cfg(not(unix))]
    {
        if plan.purge {
            purge_config(plan)?;
        }
        gregg_update::self_delete_current_exe(plan.purge)?;
    }
    eprintln!("greggd uninstalled");
    if plan.purge {
        eprintln!("configuration purged");
    } else {
        eprintln!("configuration preserved");
    }
    Ok(())
}

/// Whether a manager stop was part of this plan (as opposed to merely
/// removing a stale registration while the daemon runs unmanaged).
fn managed_stop_performed(plan: &UninstallPlan) -> bool {
    (plan.systemd_teardown && plan.discovery.systemd_active)
        || (plan.launchd_teardown && plan.discovery.launchd_loaded)
        || matches!(plan.discovery.scm, ScmDiscovery::Running)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_with(discovery: Discovery, purge: bool) -> UninstallPlan {
        let mut discovery = discovery;
        // Keep the older fixture shorthand readable while making production
        // planning require explicit ownership evidence.
        if discovery.systemd_unit_exists || discovery.systemd_active {
            discovery.systemd_ownership = crate::startup::ArtifactOwnership::Owned;
        }
        if discovery.launchd_plist_exists || discovery.launchd_loaded {
            discovery.launchd_ownership = crate::startup::ArtifactOwnership::Owned;
        }
        if discovery.cron_has_block {
            discovery.cron_ownership = crate::startup::ArtifactOwnership::Owned;
        }
        if matches!(discovery.scm, ScmDiscovery::Running | ScmDiscovery::Stopped) {
            discovery.scm_ownership = crate::startup::ArtifactOwnership::Owned;
        }
        plan_from_discovery(
            Path::new("/usr/local/bin/greggd"),
            Path::new("/etc/gregg/greggd.toml"),
            false,
            true,
            purge,
            discovery,
        )
    }

    #[test]
    fn dry_run_plan_renders_without_mutating() {
        let mut discovery = test_discovery();
        discovery.systemd_unit_exists = true;
        discovery.systemd_active = true;
        let plan = plan_with(discovery, false);
        let rendered = plan.render();
        assert!(rendered.contains("systemd"));
        assert!(rendered.contains("/usr/local/bin/greggd"));
        assert!(rendered.contains("preserve config"));
    }

    #[test]
    fn mixed_stale_artifacts_are_discovered_independently() {
        // A stale unit plus a managed cron block must both be torn down
        // even though auto-detection would collapse to one manager.
        let mut discovery = test_discovery();
        discovery.systemd_unit_exists = true;
        discovery.cron_has_block = true;
        let plan = plan_with(discovery, false);
        assert!(plan.systemd_teardown);
        assert!(plan.cron_teardown);
        let rendered = plan.render();
        assert!(rendered.contains("systemd"));
        assert!(rendered.contains("cron"));
    }

    #[test]
    fn no_artifacts_renders_nothing_to_stop() {
        let plan = plan_with(test_discovery(), false);
        assert!(!plan.systemd_teardown);
        assert!(!plan.launchd_teardown);
        assert!(!plan.cron_teardown);
        assert!(!plan.scm_teardown);
        assert!(!plan.direct_stop);
        assert!(plan.render().contains("nothing to stop"));
    }

    #[test]
    fn client_default_preserve_is_rendered_and_purge_lists_config() {
        let preserved = plan_with(test_discovery(), false);
        assert!(preserved.render().contains("preserve config"));
        let purged = plan_with(test_discovery(), true);
        assert!(purged.render().contains("/etc/gregg/greggd.toml"));
    }

    #[test]
    fn custom_config_purge_never_names_its_parent() {
        let discovery = test_discovery();
        let plan = plan_from_discovery(
            Path::new("/usr/local/bin/greggd"),
            Path::new("/tmp/operator/custom/greggd.toml"),
            true,
            true,
            true,
            discovery,
        );
        let rendered = plan.render();
        assert!(rendered.contains("/tmp/operator/custom/greggd.toml"));
        assert!(!rendered.contains("empty Gregg config directory"));
        assert_eq!(
            purge_empty_dir_for(Path::new("/tmp/operator/custom/greggd.toml")),
            None
        );
    }

    #[test]
    fn purge_config_removes_only_the_exact_custom_file() {
        let dir =
            std::env::temp_dir().join(format!("greggd_uninstall_purge_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let custom_parent = dir.join("custom-parent");
        std::fs::create_dir_all(&custom_parent).unwrap();
        let config = custom_parent.join("greggd.toml");
        std::fs::write(&config, b"name = \"x\"\n").unwrap();
        let sibling = custom_parent.join("keep.conf");
        std::fs::write(&sibling, b"keep").unwrap();

        let discovery = test_discovery();
        let plan = plan_from_discovery(
            Path::new("/tmp/fake-greggd"),
            &config,
            true,
            true,
            true,
            discovery,
        );
        purge_config(&plan).unwrap();
        assert!(!config.exists(), "purge must remove the exact config file");
        assert!(
            custom_parent.exists(),
            "custom parent must never be removed"
        );
        assert!(sibling.exists(), "siblings of the config file must survive");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn purge_config_missing_file_is_a_noop() {
        let dir = std::env::temp_dir().join(format!(
            "greggd_uninstall_purge_absent_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let discovery = test_discovery();
        let plan = plan_from_discovery(
            Path::new("/tmp/fake-greggd"),
            &dir.join("absent.toml"),
            true,
            false,
            true,
            discovery,
        );
        purge_config(&plan).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn standard_config_purge_may_clean_its_empty_dir() {
        let standard = crate::config::Config::default_path();
        let candidate = standard.clone();
        if let Some(parent) = standard.parent() {
            assert_eq!(purge_empty_dir_for(&candidate), Some(parent.to_path_buf()));
        }
    }

    #[test]
    fn sibling_binary_is_never_in_the_plan() {
        let discovery = test_discovery();
        let plan = plan_from_discovery(
            Path::new("/usr/local/bin/greggd"),
            Path::new("/etc/gregg/greggd.toml"),
            false,
            true,
            true,
            discovery,
        );
        assert_eq!(plan.exe_path, PathBuf::from("/usr/local/bin/greggd"));
        assert!(!plan.render().contains("gregg\n"));
        assert!(!plan.render().contains("/usr/local/bin/gregg "));
    }

    #[test]
    fn running_unmanaged_daemon_takes_the_direct_path() {
        let mut discovery = test_discovery();
        discovery.daemon_probe = crate::cli::HealthProbe::Ready;
        let plan = plan_with(discovery, false);
        assert!(!plan.systemd_teardown);
        // The direct control path exists only on Unix; elsewhere an
        // unmanaged running daemon blocks deletion instead.
        if cfg!(unix) {
            assert!(plan.direct_stop);
            assert!(!plan.blocked_running);
        } else {
            assert!(!plan.direct_stop);
            assert!(plan.blocked_running);
        }
    }

    #[test]
    fn managed_running_daemon_does_not_take_the_direct_path() {
        let mut discovery = test_discovery();
        discovery.systemd_unit_exists = true;
        discovery.systemd_active = true;
        discovery.daemon_probe = crate::cli::HealthProbe::Ready;
        let plan = plan_with(discovery, false);
        assert!(!plan.direct_stop);
        assert!(plan.systemd_teardown);
        assert!(managed_stop_performed(&plan));
    }

    #[test]
    fn foreign_active_systemd_using_selected_config_is_preserved() {
        let mut discovery = test_discovery();
        discovery.systemd_unit_exists = true;
        discovery.systemd_active = true;
        discovery.systemd_ownership = crate::startup::ArtifactOwnership::Foreign;
        discovery.systemd_config_path = Some(PathBuf::from("/etc/gregg/greggd.toml"));
        discovery.daemon_probe = crate::cli::HealthProbe::Ready;
        let plan = plan_from_discovery(
            Path::new("/home/operator/bin/greggd"),
            Path::new("/etc/gregg/greggd.toml"),
            false,
            true,
            false,
            discovery,
        );
        assert!(!plan.systemd_teardown);
        assert!(!plan.direct_stop);
        assert!(!plan.blocked_running);
        assert!(plan.render().contains("foreign installation preserved"));
    }

    #[cfg(unix)]
    #[test]
    fn foreign_active_manager_on_other_config_does_not_hide_direct_stop() {
        let mut discovery = test_discovery();
        discovery.systemd_unit_exists = true;
        discovery.systemd_active = true;
        discovery.systemd_ownership = crate::startup::ArtifactOwnership::Foreign;
        discovery.systemd_config_path = Some(PathBuf::from("/etc/gregg/foreign.toml"));
        discovery.daemon_probe = crate::cli::HealthProbe::Ready;
        let plan = plan_from_discovery(
            Path::new("/home/operator/bin/greggd"),
            Path::new("/tmp/operator/custom.toml"),
            true,
            true,
            false,
            discovery,
        );
        assert!(!plan.systemd_teardown);
        assert!(plan.direct_stop);
        assert!(!plan.blocked_running);
    }

    #[cfg(not(unix))]
    #[test]
    fn windows_no_scm_running_endpoint_blocks_deletion() {
        let mut discovery = test_discovery();
        discovery.daemon_probe = crate::cli::HealthProbe::Ready;
        let plan = plan_with(discovery, false);
        assert_eq!(plan.discovery.scm, ScmDiscovery::NotApplicable);
        assert!(plan.blocked_running);
    }

    #[cfg(not(unix))]
    #[test]
    fn foreign_scm_registration_is_never_selected_for_teardown() {
        let mut discovery = test_discovery();
        discovery.scm = ScmDiscovery::Running;
        discovery.scm_ownership = crate::startup::ArtifactOwnership::Foreign;
        discovery.scm_target = Some(PathBuf::from(r"C:\Other\greggd.exe"));
        discovery.daemon_probe = crate::cli::HealthProbe::Ready;
        let plan = plan_from_discovery(
            Path::new(r"C:\Gregg\greggd.exe"),
            Path::new(r"C:\ProgramData\gregg\greggd.toml"),
            false,
            true,
            false,
            discovery,
        );
        assert!(!plan.scm_teardown);
        assert!(plan.render().contains("foreign installation preserved"));
    }

    #[cfg(not(unix))]
    #[test]
    fn unknown_scm_ownership_blocks_before_mutation() {
        let mut discovery = test_discovery();
        discovery.scm = ScmDiscovery::Unknown;
        discovery.scm_ownership = crate::startup::ArtifactOwnership::Unknown;
        let plan = plan_with(discovery, true);
        assert!(plan.blocked_scm_unknown);
        assert!(plan.render().contains("no mutation is safe"));
    }

    #[test]
    fn cargo_owned_plan_renders_handoff() {
        let mut discovery = test_discovery();
        discovery.exe_path = PathBuf::from("/home/u/.cargo/bin/greggd");
        discovery.cargo = Some(gregg_update::CargoOwnership {
            root: PathBuf::from("/home/u/.cargo"),
            package: "greggd".to_string(),
        });
        let plan = plan_with(discovery, false);
        assert!(plan.render().contains("cargo-owned"));
        assert!(plan.render().contains("cargo uninstall --root"));
        assert!(plan.render().contains("preserve config"));
        let purge = plan_from_discovery(
            Path::new("/home/u/.cargo/bin/greggd"),
            Path::new("/etc/gregg/greggd.toml"),
            false,
            true,
            true,
            plan.discovery.clone(),
        );
        assert!(purge.render().contains("remove config file"));
    }

    #[test]
    fn launchd_teardown_covers_plist_and_loaded_states() {
        let mut discovery = test_discovery();
        discovery.launchd_plist_exists = true;
        discovery.launchd_loaded = true;
        let plan = plan_with(discovery, false);
        assert!(plan.launchd_teardown);
        assert!(plan.render().contains("com.eggstack.greggd"));
    }

    #[test]
    fn scm_running_plans_teardown_without_direct_stop() {
        let mut discovery = test_discovery();
        discovery.scm = ScmDiscovery::Running;
        discovery.daemon_probe = crate::cli::HealthProbe::Ready;
        let plan = plan_with(discovery, false);
        assert!(plan.scm_teardown);
        assert!(!plan.direct_stop);
    }

    #[cfg(unix)]
    #[test]
    fn uncertain_direct_stop_blocks_deletion() {
        use crate::control::{ControlError, StopOutcome};
        let uncertain: Result<StopOutcome, ControlError> = Ok(StopOutcome::Uncertain {
            detail: "timeout".to_string(),
        });
        assert_eq!(decide_direct_stop(&uncertain), DirectStopDecision::Block);
        let failed: Result<StopOutcome, ControlError> =
            Err(ControlError::Io(std::io::Error::other("refused")));
        assert_eq!(decide_direct_stop(&failed), DirectStopDecision::Block);
        let stopped: Result<StopOutcome, ControlError> = Ok(StopOutcome::Stopped {
            path: PathBuf::from("/tmp/x.sock"),
        });
        assert_eq!(decide_direct_stop(&stopped), DirectStopDecision::Proceed);
        let absent: Result<StopOutcome, ControlError> = Ok(StopOutcome::NotRunning);
        assert_eq!(decide_direct_stop(&absent), DirectStopDecision::Proceed);
    }
}
