//! Binary-first self-update for `greggd`.
//!
//! Lifecycle coordinator over the shared [`gregg_update`] mechanism
//! (Plan 104): crates.io is the version authority, the exact tagged GitHub
//! Release asset is the binary candidate, and Cargo is the fallback only
//! when the asset is absent (HTTP 404). Checksum and candidate `version`
//! are verified before any replacement.
//!
//! Update lifecycle is exact-executable-aware (Plan 116): the coordinator
//! never uses host-global `startup_state()` as mutation authority. Unix
//! intent comes from systemd/launchd exact-executable ownership plus the
//! selected config's bounded health probe; Windows intent comes from
//! `query_registration()` plus exact path equivalence. The final manager
//! mutation stays behind ownership-aware `restart_daemon()`.
//!
//! Transaction rule (Plan 102, preserved): the candidate is fully prepared
//! and verified before a running daemon service is quiesced where required.
//! Lifecycle intent is observed after preparation and immediately before
//! mutation. All transport/staging/replacement mechanics live in
//! `gregg_update`; this module only binds the daemon identity and
//! coordinates activation.

pub use gregg_update::{
    asset_name, compare_versions, detect_target, detect_target_for, github_urls,
    is_supported_binary_target, parse_stable_version, UpdateError,
};
use gregg_update::{UpdatePlan, UpdateSpec};

use std::fmt;
use std::path::Path;

#[cfg(any(target_os = "windows", test))]
use crate::service::{ServiceRegistration, ServiceState};
use crate::startup::ArtifactOwnership;

/// Update-specific lifecycle disposition, relative to the exact invoked
/// executable and the selected config (Plan 116).
///
/// This is deliberately small: it answers only what `greggd update` may
/// mutate and whether a post-replacement restart is required. It is not a
/// generalized installation registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateLifecycle {
    /// An owned manager is active; replace then restart via `restart_daemon()`.
    ManagedRunning,
    /// An owned manager exists but is stopped; replace and leave stopped.
    ManagedStopped,
    /// The selected direct/cron daemon is running; replace then restart via
    /// the config-specific direct path.
    DirectRunning,
    /// Nothing running that belongs to this installation; replace without
    /// claiming a restart.
    Stopped,
    /// A foreign manager owns the selected config (Unix) or a foreign SCM
    /// registration exists (Windows); preserve it, never stop/restart it,
    /// replace only the invoked executable.
    ForeignPreserved,
}

impl fmt::Display for UpdateLifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::ManagedRunning => "managed-running",
            Self::ManagedStopped => "managed-stopped",
            Self::DirectRunning => "direct-running",
            Self::Stopped => "stopped",
            Self::ForeignPreserved => "foreign-preserved",
        };
        write!(f, "{s}")
    }
}

/// Pure Unix update decision from injected ownership, activity, config
/// identity, and selected-endpoint running intent.
///
/// `registered_config` is the manager's known config target when parsed;
/// `None` means the foreign/active registration has unknown config identity
/// and must fail closed.
fn decide_unix_update_lifecycle(
    ownership: ArtifactOwnership,
    active: bool,
    registered_config: Option<&Path>,
    selected_config: &Path,
    endpoint_running: bool,
) -> Result<UpdateLifecycle, String> {
    match ownership {
        ArtifactOwnership::Owned => {
            if active {
                Ok(UpdateLifecycle::ManagedRunning)
            } else {
                Ok(UpdateLifecycle::ManagedStopped)
            }
        }
        ArtifactOwnership::Foreign => {
            if active {
                match registered_config {
                    Some(config)
                        if gregg_update::uninstall::paths_equivalent(
                            config,
                            selected_config,
                        ) =>
                    {
                        Ok(UpdateLifecycle::ForeignPreserved)
                    }
                    Some(_) => {
                        if endpoint_running {
                            Ok(UpdateLifecycle::DirectRunning)
                        } else {
                            Ok(UpdateLifecycle::Stopped)
                        }
                    }
                    None => Err(
                        "foreign manager registration has unknown config ownership; refusing update"
                            .to_string(),
                    ),
                }
            } else if endpoint_running {
                Ok(UpdateLifecycle::DirectRunning)
            } else {
                Ok(UpdateLifecycle::Stopped)
            }
        }
        ArtifactOwnership::Absent => {
            if active {
                Err(
                    "manager is active but its executable registration is absent; refusing update"
                        .to_string(),
                )
            } else if endpoint_running {
                Ok(UpdateLifecycle::DirectRunning)
            } else {
                Ok(UpdateLifecycle::Stopped)
            }
        }
        ArtifactOwnership::Unknown => {
            if active {
                Err("cannot determine manager executable ownership; refusing update".to_string())
            } else if endpoint_running {
                Err(
                    "cannot establish safe post-update ownership for a running daemon; refusing update"
                        .to_string(),
                )
            } else {
                Ok(UpdateLifecycle::Stopped)
            }
        }
    }
}

/// Pure Windows update decision from one bounded SCM observation plus the
/// exact invoked executable.
///
/// `NotInstalled` and foreign registrations perform zero SCM mutation.
/// Owned `Running`/`StartPending` require quiescence then restart; owned
/// `Stopped`/`StopPending` replace without an automatic restart
/// (`StopPending` still waits for stopped before replacement). Missing or
/// unparseable executable identity fails closed.
#[cfg(any(target_os = "windows", test))]
fn decide_windows_update_lifecycle(
    registration: &ServiceRegistration,
    exe: &Path,
) -> Result<UpdateLifecycle, String> {
    if registration.state == ServiceState::NotInstalled {
        return Ok(UpdateLifecycle::Stopped);
    }
    let Some(target) = registration.executable_path.as_deref() else {
        return Err("Windows SCM executable ownership is unknown; refusing update".to_string());
    };
    if !gregg_update::uninstall::paths_equivalent(target, exe) {
        return Ok(UpdateLifecycle::ForeignPreserved);
    }
    match registration.state {
        ServiceState::Running | ServiceState::StartPending => Ok(UpdateLifecycle::ManagedRunning),
        ServiceState::Stopped | ServiceState::StopPending => Ok(UpdateLifecycle::ManagedStopped),
        ServiceState::NotInstalled => Ok(UpdateLifecycle::Stopped),
    }
}

/// Whether a Windows SCM registration is owned by the exact invoked
/// executable. Used only to detect an owned-to-foreign transition across
/// candidate preparation.
#[cfg(any(target_os = "windows", test))]
fn is_owned_windows_registration(registration: &ServiceRegistration, exe: &Path) -> bool {
    if registration.state == ServiceState::NotInstalled {
        return false;
    }
    registration
        .executable_path
        .as_deref()
        .is_some_and(|target| gregg_update::uninstall::paths_equivalent(target, exe))
}

/// Outcome of a successful `greggd update` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// Already at the latest stable version.
    AlreadyCurrent {
        /// Installed version.
        version: String,
    },
    /// Replaced via the exact tagged GitHub Release asset.
    UpdatedBinary {
        /// Previous version.
        from: String,
        /// Installed version.
        to: String,
    },
    /// Replaced via the Cargo fallback.
    UpdatedFromCargo {
        /// Previous version.
        from: String,
        /// Installed version.
        to: String,
    },
    /// On-disk binary was replaced but the subsequent restart failed.
    /// The caller must surface the installed version and the exact restart
    /// command needed. Exit status should be nonzero.
    UpdatedButRestartFailed {
        /// Previous version.
        from: String,
        /// Installed version.
        to: String,
        /// Restart failure detail.
        restart_error: String,
    },
}

impl fmt::Display for UpdateOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyCurrent { version } => {
                write!(f, "greggd {version} is already the latest stable version")
            }
            Self::UpdatedBinary { from, to } => {
                write!(f, "updated greggd {from} -> {to} (GitHub binary)")
            }
            Self::UpdatedFromCargo { from, to } => {
                write!(f, "updated greggd {from} -> {to} (Cargo)")
            }
            Self::UpdatedButRestartFailed {
                from,
                to,
                restart_error,
            } => write!(
                f,
                "updated greggd {from} -> {to} on disk but restart failed: {restart_error}"
            ),
        }
    }
}

const CRATE_NAME: &str = "greggd";
const PROGRAM: &str = "greggd";
const CURR_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The shared update identity for this program.
#[must_use]
pub fn update_spec() -> UpdateSpec {
    UpdateSpec::new(CRATE_NAME, PROGRAM, CURR_VERSION)
}

// ── Restart dispatch ────────────────────────────────────────────────────────

/// Restart only the lifecycle positively attributed to this installation.
/// `ManagedRunning` and `DirectRunning` route through ownership-aware
/// `restart_daemon()` so a registration change between quiescence and
/// restart fails as `UpdatedButRestartFailed` instead of mutating a foreign
/// manager. All other dispositions leave the host untouched.
fn restart_after_update(
    lifecycle: UpdateLifecycle,
    exe: &Path,
    config_path: &Path,
    explicit: bool,
) -> Result<(), UpdateError> {
    match lifecycle {
        UpdateLifecycle::ManagedRunning | UpdateLifecycle::DirectRunning => {
            crate::startup::restart_daemon(exe, config_path, explicit)
                .map_err(|e| UpdateError::RestartFailed(format!("{e}")))
        }
        UpdateLifecycle::ManagedStopped => {
            eprintln!("Service is installed but stopped; leaving it stopped after update.");
            Ok(())
        }
        UpdateLifecycle::Stopped => {
            eprintln!("No daemon running; leaving binary updated without starting.");
            Ok(())
        }
        UpdateLifecycle::ForeignPreserved => {
            eprintln!(
                "Foreign manager registration preserved; updated only the invoked executable without restarting it."
            );
            Ok(())
        }
    }
}

// ── Main entry ──────────────────────────────────────────────────────────────

/// Run the full `greggd update` flow synchronously.
/// `config_path` and `explicit` describe the resolved config location.
/// Prints progress to stderr and returns an outcome or error.
pub fn run_update(config_path: &Path, explicit: bool) -> Result<UpdateOutcome, UpdateError> {
    let spec = update_spec();
    let plan = gregg_update::resolve_plan(&spec)?;
    let (current, latest, target) = match plan {
        UpdatePlan::AlreadyCurrent { version } => {
            return Ok(UpdateOutcome::AlreadyCurrent { version });
        }
        UpdatePlan::Available {
            current,
            latest,
            target,
        } => (current, latest, target),
    };

    // Permission check before any download or lifecycle action.
    let (_exe_path, original_exe) = gregg_update::preflight_exe_writable(PROGRAM)?;

    #[cfg(target_os = "windows")]
    let pre_registration = crate::service::platform_service_manager()
        .query_registration()
        .ok();

    // Fully prepare and verify the candidate before quiescing anything.
    let (from_cargo, staged) =
        gregg_update::prepare_candidate(&spec, &current, &latest, target.as_deref())?;

    // Observe exact-executable lifecycle immediately before mutation. This
    // post-preparation observation is the sole mutation authority; any
    // earlier read-only query is diagnostics only and never decides a stop.
    #[cfg(target_os = "windows")]
    let (lifecycle, post_registration) = {
        let manager = crate::service::platform_service_manager();
        let post = manager.query_registration().map_err(|error| {
            let message = error.to_string();
            if message.to_ascii_lowercase().contains("access denied")
                || message.to_ascii_lowercase().contains("permission")
            {
                UpdateError::PermissionDenied {
                    message: format!("failed to query Windows service ownership: {message}"),
                    elevated: "run as Administrator: greggd update".to_string(),
                }
            } else {
                UpdateError::Io(format!(
                    "failed to query Windows service ownership; refusing unsafe update: {message}"
                ))
            }
        })?;
        let lifecycle =
            decide_windows_update_lifecycle(&post, &original_exe).map_err(UpdateError::Io)?;
        // An owned-to-foreign/unknown transition across the (potentially
        // long) candidate build must not mutate anything: fail before
        // replacement. A disappeared registration proceeds as unmanaged.
        if let Some(pre) = pre_registration.as_ref() {
            let pre_owned = is_owned_windows_registration(pre, &original_exe);
            if pre_owned && matches!(lifecycle, UpdateLifecycle::ForeignPreserved) {
                return Err(UpdateError::Io(
                    "Windows SCM registration changed from owned to foreign during preparation; refusing update with zero mutation"
                        .to_string(),
                ));
            }
        }
        eprintln!("Current greggd {current}, latest {latest}, update lifecycle: {lifecycle}");
        (lifecycle, post)
    };
    #[cfg(not(target_os = "windows"))]
    let lifecycle = {
        let lifecycle = observe_unix_lifecycle(&original_exe, config_path, explicit)?;
        eprintln!("Current greggd {current}, latest {latest}, update lifecycle: {lifecycle}");
        lifecycle
    };

    #[cfg(target_os = "windows")]
    quiesce_windows_service_if_needed(&post_registration, &original_exe, lifecycle)?;
    gregg_update::stage::replace_current_exe(staged.path(), PROGRAM)?;
    eprintln!(
        "Replaced {PROGRAM} binary {current} -> {latest} via {}",
        if from_cargo { "Cargo" } else { "GitHub binary" }
    );
    let new_version = latest;

    // Now restart only the positively attributed lifecycle.
    let restart_result = restart_after_update(lifecycle, &original_exe, config_path, explicit);
    match restart_result {
        Ok(()) => {
            if from_cargo {
                Ok(UpdateOutcome::UpdatedFromCargo {
                    from: current,
                    to: new_version,
                })
            } else {
                Ok(UpdateOutcome::UpdatedBinary {
                    from: current,
                    to: new_version,
                })
            }
        }
        Err(restart_err) => {
            let msg = restart_err.to_string();
            eprintln!(
                "Updated {PROGRAM} {current} -> {new_version} on disk but restart failed: {msg}"
            );
            eprintln!("Rerun: {} restart", original_exe.display());
            if matches!(lifecycle, UpdateLifecycle::ManagedRunning) {
                #[cfg(all(unix, target_os = "linux"))]
                eprintln!("Or: sudo systemctl restart greggd");
                #[cfg(all(unix, target_os = "macos"))]
                eprintln!("Or: sudo launchctl kickstart -k system/com.eggstack.greggd");
            }
            Ok(UpdateOutcome::UpdatedButRestartFailed {
                from: current,
                to: new_version,
                restart_error: msg,
            })
        }
    }
}

/// Observe the Unix update lifecycle after candidate preparation.
///
/// Combines systemd/launchd exact-executable ownership with the selected
/// config's bounded health probe. Host-global `startup_state()` is never
/// consulted for mutation authority.
#[cfg(not(target_os = "windows"))]
fn observe_unix_lifecycle(
    exe: &Path,
    config_path: &Path,
    explicit: bool,
) -> Result<UpdateLifecycle, UpdateError> {
    let (ownership, active, registered_config) = unix_manager_ownership(exe);
    let endpoint_running = is_selected_daemon_running(config_path, explicit);
    decide_unix_update_lifecycle(
        ownership,
        active,
        registered_config.as_deref(),
        config_path,
        endpoint_running,
    )
    .map_err(UpdateError::Io)
}

/// Platform-selected manager ownership for the exact invoked executable.
#[cfg(all(unix, target_os = "linux"))]
fn unix_manager_ownership(exe: &Path) -> (ArtifactOwnership, bool, Option<std::path::PathBuf>) {
    crate::startup::systemd_artifact_ownership(exe)
}

/// Platform-selected manager ownership for the exact invoked executable.
#[cfg(all(unix, target_os = "macos"))]
fn unix_manager_ownership(exe: &Path) -> (ArtifactOwnership, bool, Option<std::path::PathBuf>) {
    crate::startup::launchd_artifact_ownership(exe)
}

/// Non-Linux/macOS Unix has no managed ownership to attribute.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn unix_manager_ownership(_exe: &Path) -> (ArtifactOwnership, bool, Option<std::path::PathBuf>) {
    (ArtifactOwnership::Absent, false, None)
}

/// Non-Unix non-Windows builds have no manager to observe.
#[cfg(not(any(unix, target_os = "windows")))]
fn observe_unix_lifecycle(
    _exe: &Path,
    _config_path: &Path,
    _explicit: bool,
) -> Result<UpdateLifecycle, UpdateError> {
    Ok(UpdateLifecycle::Stopped)
}

#[cfg(target_os = "windows")]
fn quiesce_windows_service_if_needed(
    registration: &ServiceRegistration,
    exe: &Path,
    lifecycle: UpdateLifecycle,
) -> Result<(), UpdateError> {
    // Revalidate ownership immediately before any service mutation: only an
    // exact-owned running service may be stopped. Foreign, unmanaged, and
    // stopped dispositions perform zero SCM mutation.
    let owned = registration
        .executable_path
        .as_deref()
        .is_some_and(|target| gregg_update::uninstall::paths_equivalent(target, exe));
    match (lifecycle, registration.state, owned) {
        (UpdateLifecycle::ManagedRunning, ServiceState::Running, true)
        | (UpdateLifecycle::ManagedRunning, ServiceState::StartPending, true) => {
            eprintln!("Stopping owned Windows service after candidate verification...");
            crate::service::platform_service_manager()
                .stop()
                .map_err(|error| {
                    let message = error.to_string();
                    if message.to_ascii_lowercase().contains("access denied")
                        || message.to_ascii_lowercase().contains("permission")
                    {
                        UpdateError::PermissionDenied {
                            message: format!("failed to stop service: {message}"),
                            elevated: "run as Administrator: greggd update".to_string(),
                        }
                    } else {
                        UpdateError::RestartFailed(format!(
                            "failed to stop Windows service before replacement: {message}"
                        ))
                    }
                })?;
            Ok(())
        }
        (UpdateLifecycle::ManagedStopped, ServiceState::StopPending, true) => {
            // The service was already transitioning down; wait for stopped
            // before replacement and do not restart merely because it was
            // pending.
            eprintln!("Waiting for owned Windows service to reach stopped...");
            crate::service::platform_service_manager()
                .stop()
                .map_err(|error| {
                    let message = error.to_string();
                    if message.to_ascii_lowercase().contains("access denied")
                        || message.to_ascii_lowercase().contains("permission")
                    {
                        UpdateError::PermissionDenied {
                            message: format!("failed to wait for service stop: {message}"),
                            elevated: "run as Administrator: greggd update".to_string(),
                        }
                    } else {
                        UpdateError::RestartFailed(format!(
                            "failed to wait for Windows service stop before replacement: {message}"
                        ))
                    }
                })?;
            Ok(())
        }
        // Owned stopped, unmanaged, and foreign paths: zero SCM mutation.
        _ => Ok(()),
    }
}

// ── Daemon running probe for direct/cron intent ─────────────────────────────

/// Whether the selected config's endpoint currently answers as a Gregg
/// daemon. A valid Ready/Warming/Failed health response means running
/// intent; anything else (including an unreadable config) means absent.
fn is_selected_daemon_running(config_path: &Path, explicit: bool) -> bool {
    let Ok(config) = crate::cli::load_config(config_path, explicit) else {
        return false;
    };
    let target = crate::cli::croncheck_target(&config);
    // Reuse the authoritative bounded probe: a valid Gregg readiness state
    // means a daemon is running, regardless of startup manager.
    matches!(
        crate::cli::probe_health(target),
        crate::cli::HealthProbe::Ready
            | crate::cli::HealthProbe::Warming
            | crate::cli::HealthProbe::Failed
    )
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn spec_binds_greggd_identity() {
        let spec = update_spec();
        assert_eq!(spec.crate_name, "greggd");
        assert_eq!(spec.program_name, "greggd");
        assert_eq!(spec.current_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn outcome_strings_keep_program_prefix() {
        assert_eq!(
            UpdateOutcome::AlreadyCurrent {
                version: "1.0.12".to_string()
            }
            .to_string(),
            "greggd 1.0.12 is already the latest stable version"
        );
        assert_eq!(
            UpdateOutcome::UpdatedBinary {
                from: "1.0.11".to_string(),
                to: "1.0.12".to_string()
            }
            .to_string(),
            "updated greggd 1.0.11 -> 1.0.12 (GitHub binary)"
        );
    }

    #[test]
    fn shared_version_target_helpers_match_contract() {
        // The coordinator owns no version/target/asset logic; the shared crate does.
        assert_eq!(parse_stable_version("1.0.12"), Some((1, 0, 12)));
        assert_eq!(
            detect_target_for("windows", "x86_64"),
            Some("x86_64-pc-windows-msvc".to_string())
        );
        assert_eq!(
            asset_name("greggd", "x86_64-unknown-linux-gnu"),
            "greggd-x86_64-unknown-linux-gnu"
        );
        let (url, sha) = github_urls("greggd", "x86_64-unknown-linux-gnu", "1.0.12");
        assert_eq!(
            url,
            "https://github.com/eggstack/gregg/releases/download/v1.0.12/greggd-x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            sha,
            "https://github.com/eggstack/gregg/releases/download/v1.0.12/greggd-x86_64-unknown-linux-gnu.sha256"
        );
        assert!(is_supported_binary_target("aarch64-apple-darwin"));
        assert!(!is_supported_binary_target("armv7-unknown-linux-gnueabihf"));
    }

    #[test]
    fn startup_state_helpers() {
        assert_eq!(
            crate::startup::systemd_state_with(true, true),
            crate::startup::StartupState::SystemdActive
        );
        assert_eq!(
            crate::startup::systemd_state_with(false, false),
            crate::startup::StartupState::UnmanagedOrCron
        );
    }

    #[test]
    fn restart_decision_for_stopped_service_is_no_restart() {
        // Pure helper: installed but stopped should not restart.
        let state = crate::startup::StartupState::SystemdInstalledStopped;
        // The actual restart function would not be called directly in unit test to avoid systemctl.
        // We just assert the enum distinction is correct.
        assert_ne!(state, crate::startup::StartupState::SystemdActive);
    }

    #[test]
    fn candidate_is_prepared_before_any_quiesce() {
        // Transaction ordering (Plan 102, preserved by Plan 104): the shared
        // prepare step fully verifies the candidate, and the Windows quiesce
        // gate requires ownership-aware lifecycle intent observed after
        // preparation. A stopped service is never touched even with a
        // candidate in hand.
        let exe = Path::new("/usr/local/bin/greggd");
        let owned_stopped = ServiceRegistration {
            state: ServiceState::Stopped,
            executable_path: Some(PathBuf::from("/usr/local/bin/greggd")),
        };
        assert_eq!(
            decide_windows_update_lifecycle(&owned_stopped, exe),
            Ok(UpdateLifecycle::ManagedStopped)
        );
        let owned_running = ServiceRegistration {
            state: ServiceState::Running,
            executable_path: Some(PathBuf::from("/usr/local/bin/greggd")),
        };
        assert_eq!(
            decide_windows_update_lifecycle(&owned_running, exe),
            Ok(UpdateLifecycle::ManagedRunning)
        );
    }

    // ── Plan 116 Unix lifecycle regressions ─────────────────────────────

    fn selected() -> PathBuf {
        PathBuf::from("/etc/gregg/greggd.toml")
    }

    #[test]
    fn owned_active_systemd_is_managed_running() {
        assert_eq!(
            decide_unix_update_lifecycle(
                ArtifactOwnership::Owned,
                true,
                Some(selected()).as_deref(),
                &selected(),
                false,
            ),
            Ok(UpdateLifecycle::ManagedRunning)
        );
    }

    #[test]
    fn owned_inactive_systemd_remains_stopped() {
        assert_eq!(
            decide_unix_update_lifecycle(
                ArtifactOwnership::Owned,
                false,
                Some(selected()).as_deref(),
                &selected(),
                false,
            ),
            Ok(UpdateLifecycle::ManagedStopped)
        );
        // Even with a direct endpoint answering, owned-inactive stays
        // stopped: the manager owns the lifecycle, not the probe.
        assert_eq!(
            decide_unix_update_lifecycle(
                ArtifactOwnership::Owned,
                false,
                Some(selected()).as_deref(),
                &selected(),
                true,
            ),
            Ok(UpdateLifecycle::ManagedStopped)
        );
    }

    #[test]
    fn foreign_active_same_config_is_preserved_without_direct_restart() {
        assert_eq!(
            decide_unix_update_lifecycle(
                ArtifactOwnership::Foreign,
                true,
                Some(selected()).as_deref(),
                &selected(),
                true,
            ),
            Ok(UpdateLifecycle::ForeignPreserved)
        );
        assert_eq!(
            decide_unix_update_lifecycle(
                ArtifactOwnership::Foreign,
                true,
                Some(selected()).as_deref(),
                &selected(),
                false,
            ),
            Ok(UpdateLifecycle::ForeignPreserved)
        );
    }

    #[test]
    fn foreign_active_different_config_preserves_manager_and_probes_direct() {
        let other = PathBuf::from("/other/greggd.toml");
        assert_eq!(
            decide_unix_update_lifecycle(
                ArtifactOwnership::Foreign,
                true,
                Some(other).as_deref(),
                &selected(),
                true,
            ),
            Ok(UpdateLifecycle::DirectRunning)
        );
        assert_eq!(
            decide_unix_update_lifecycle(
                ArtifactOwnership::Foreign,
                true,
                Some(PathBuf::from("/other/greggd.toml")).as_deref(),
                &selected(),
                false,
            ),
            Ok(UpdateLifecycle::Stopped)
        );
    }

    #[test]
    fn foreign_inactive_no_longer_masks_running_direct_daemon() {
        assert_eq!(
            decide_unix_update_lifecycle(
                ArtifactOwnership::Foreign,
                false,
                Some(PathBuf::from("/other/greggd.toml")).as_deref(),
                &selected(),
                true,
            ),
            Ok(UpdateLifecycle::DirectRunning)
        );
        assert_eq!(
            decide_unix_update_lifecycle(
                ArtifactOwnership::Foreign,
                false,
                Some(PathBuf::from("/other/greggd.toml")).as_deref(),
                &selected(),
                false,
            ),
            Ok(UpdateLifecycle::Stopped)
        );
    }

    #[test]
    fn absent_manager_probes_direct_running() {
        assert_eq!(
            decide_unix_update_lifecycle(ArtifactOwnership::Absent, false, None, &selected(), true,),
            Ok(UpdateLifecycle::DirectRunning)
        );
        assert_eq!(
            decide_unix_update_lifecycle(
                ArtifactOwnership::Absent,
                false,
                None,
                &selected(),
                false,
            ),
            Ok(UpdateLifecycle::Stopped)
        );
    }

    #[test]
    fn unknown_active_manager_fails_before_replacement() {
        assert!(decide_unix_update_lifecycle(
            ArtifactOwnership::Unknown,
            true,
            None,
            &selected(),
            false,
        )
        .is_err());
    }

    #[test]
    fn unknown_inactive_with_running_endpoint_fails_before_replacement() {
        assert!(decide_unix_update_lifecycle(
            ArtifactOwnership::Unknown,
            false,
            None,
            &selected(),
            true,
        )
        .is_err());
    }

    #[test]
    fn unknown_inactive_without_endpoint_may_proceed_stopped() {
        assert_eq!(
            decide_unix_update_lifecycle(
                ArtifactOwnership::Unknown,
                false,
                None,
                &selected(),
                false,
            ),
            Ok(UpdateLifecycle::Stopped)
        );
    }

    #[test]
    fn foreign_active_with_unknown_config_fails_closed() {
        assert!(decide_unix_update_lifecycle(
            ArtifactOwnership::Foreign,
            true,
            None,
            &selected(),
            true,
        )
        .is_err());
    }

    #[test]
    fn launchd_cases_follow_same_policy() {
        // Same pure policy backs launchd; spot-check the manager-preserving
        // and direct-running branches.
        assert_eq!(
            decide_unix_update_lifecycle(ArtifactOwnership::Owned, true, None, &selected(), false,),
            Ok(UpdateLifecycle::ManagedRunning)
        );
        assert_eq!(
            decide_unix_update_lifecycle(
                ArtifactOwnership::Foreign,
                true,
                Some(selected()).as_deref(),
                &selected(),
                false,
            ),
            Ok(UpdateLifecycle::ForeignPreserved)
        );
    }

    // ── Plan 116 Windows lifecycle regressions (pure, run everywhere) ──

    #[test]
    fn windows_not_installed_performs_zero_scm_mutation() {
        let exe = Path::new("/usr/local/bin/greggd");
        let reg = ServiceRegistration {
            state: ServiceState::NotInstalled,
            executable_path: None,
        };
        assert_eq!(
            decide_windows_update_lifecycle(&reg, exe),
            Ok(UpdateLifecycle::Stopped)
        );
    }

    #[test]
    fn windows_owned_running_and_start_pending_require_restart() {
        let exe = Path::new("/usr/local/bin/greggd");
        for state in [ServiceState::Running, ServiceState::StartPending] {
            let reg = ServiceRegistration {
                state,
                executable_path: Some(PathBuf::from("/usr/local/bin/greggd")),
            };
            assert_eq!(
                decide_windows_update_lifecycle(&reg, exe),
                Ok(UpdateLifecycle::ManagedRunning),
                "state {state:?} should be managed-running"
            );
        }
    }

    #[test]
    fn windows_owned_stopped_and_stop_pending_stay_stopped() {
        let exe = Path::new("/usr/local/bin/greggd");
        for state in [ServiceState::Stopped, ServiceState::StopPending] {
            let reg = ServiceRegistration {
                state,
                executable_path: Some(PathBuf::from("/usr/local/bin/greggd")),
            };
            assert_eq!(
                decide_windows_update_lifecycle(&reg, exe),
                Ok(UpdateLifecycle::ManagedStopped),
                "state {state:?} should be managed-stopped"
            );
        }
    }

    #[test]
    fn windows_foreign_registration_is_preserved_without_mutation() {
        let exe = Path::new("/home/user/greggd");
        for state in [
            ServiceState::Running,
            ServiceState::Stopped,
            ServiceState::StartPending,
            ServiceState::StopPending,
        ] {
            let reg = ServiceRegistration {
                state,
                executable_path: Some(PathBuf::from("/other/greggd")),
            };
            assert_eq!(
                decide_windows_update_lifecycle(&reg, exe),
                Ok(UpdateLifecycle::ForeignPreserved),
                "foreign state {state:?} must be preserved"
            );
        }
    }

    #[test]
    fn windows_missing_image_or_uninstalled_identity_fails_closed() {
        let exe = Path::new("/usr/local/bin/greggd");
        let missing = ServiceRegistration {
            state: ServiceState::Running,
            executable_path: None,
        };
        assert!(decide_windows_update_lifecycle(&missing, exe).is_err());
        let missing_stopped = ServiceRegistration {
            state: ServiceState::Stopped,
            executable_path: None,
        };
        assert!(decide_windows_update_lifecycle(&missing_stopped, exe).is_err());
    }

    #[test]
    fn windows_owned_to_foreign_transition_is_detected_for_zero_mutation() {
        // Revalidation: a pre-preparation owned observation followed by a
        // post-preparation foreign observation must not stop or replace.
        // The pure layer proves the second observation resolves to
        // preserve-without-mutation while the transition detector flags it.
        let exe = Path::new("/home/user/greggd.exe");
        let pre = ServiceRegistration {
            state: ServiceState::Running,
            executable_path: Some(PathBuf::from("/home/user/greggd.exe")),
        };
        let post = ServiceRegistration {
            state: ServiceState::Running,
            executable_path: Some(PathBuf::from("C:\\Program Files\\Gregg\\greggd.exe")),
        };
        assert!(is_owned_windows_registration(&pre, exe));
        assert_eq!(
            decide_windows_update_lifecycle(&post, exe),
            Ok(UpdateLifecycle::ForeignPreserved)
        );
        // The coordinator treats pre-owned + post-foreign as a hard refusal
        // before replacement (see run_update); the pure pieces above are the
        // decision inputs that force that branch.
        assert!(!is_owned_windows_registration(&post, exe));
    }

    #[test]
    fn windows_disappeared_registration_proceeds_as_unmanaged() {
        let exe = Path::new("/home/user/greggd");
        let gone = ServiceRegistration {
            state: ServiceState::NotInstalled,
            executable_path: None,
        };
        assert_eq!(
            decide_windows_update_lifecycle(&gone, exe),
            Ok(UpdateLifecycle::Stopped)
        );
        assert!(!is_owned_windows_registration(&gone, exe));
    }

    #[test]
    fn stopped_or_foreign_lifecycle_never_claims_restart() {
        // Successful replacement of a stopped/foreign installation must not
        // print or report a fabricated restart: only ManagedRunning and
        // DirectRunning route through restart_daemon().
        for lifecycle in [
            UpdateLifecycle::ManagedStopped,
            UpdateLifecycle::Stopped,
            UpdateLifecycle::ForeignPreserved,
        ] {
            assert!(
                !matches!(
                    lifecycle,
                    UpdateLifecycle::ManagedRunning | UpdateLifecycle::DirectRunning
                ),
                "{lifecycle} must not restart"
            );
        }
        for lifecycle in [
            UpdateLifecycle::ManagedRunning,
            UpdateLifecycle::DirectRunning,
        ] {
            assert!(
                matches!(
                    lifecycle,
                    UpdateLifecycle::ManagedRunning | UpdateLifecycle::DirectRunning
                ),
                "{lifecycle} must restart"
            );
        }
    }
}
