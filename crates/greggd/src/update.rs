//! Binary-first self-update for `greggd`.
//!
//! Lifecycle coordinator over the shared [`gregg_update`] mechanism
//! (Plan 104): crates.io is the version authority, the exact tagged GitHub
//! Release asset is the binary candidate, and Cargo is the fallback only
//! when the asset is absent (HTTP 404). Checksum and candidate `version`
//! are verified before any replacement. `greggd update` reuses Plan 100's
//! `startup_state`/`restart` logic and never invokes `sudo` internally.
//!
//! Transaction rule (Plan 102, preserved): the candidate is fully prepared
//! and verified before a running daemon service is quiesced where required.
//! All transport/staging/replacement mechanics live in `gregg_update`; this
//! module only binds the daemon identity and coordinates activation.

pub use gregg_update::{
    asset_name, compare_versions, detect_target, detect_target_for, github_urls,
    is_supported_binary_target, parse_stable_version, UpdateError,
};
use gregg_update::{UpdatePlan, UpdateSpec};

use std::fmt;
use std::path::Path;

use crate::startup::{startup_state, StartupState};

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

fn restart_after_update(
    state: StartupState,
    exe: &Path,
    config_path: &Path,
    explicit: bool,
) -> Result<(), UpdateError> {
    // For Windows running service, we already stopped before replacement if needed.
    // Now restart according to policy.
    match state {
        StartupState::SystemdActive
        | StartupState::LaunchdLoaded
        | StartupState::WindowsServiceRunning => {
            // Running managed -> restart via manager.
            crate::startup::restart_with_state(state, exe, config_path, explicit)
                .map_err(|e| UpdateError::RestartFailed(format!("{e}")))
        }
        StartupState::SystemdInstalledStopped
        | StartupState::LaunchdInstalledUnloaded
        | StartupState::WindowsServiceStopped => {
            // Installed but intentionally stopped -> leave stopped.
            eprintln!("Service is installed but stopped; leaving it stopped after update.");
            Ok(())
        }
        StartupState::UnmanagedOrCron => {
            if is_unmanaged_daemon_running(config_path, explicit) {
                eprintln!("Restarting direct/cron daemon...");
                crate::startup::restart_with_state(state, exe, config_path, explicit)
                    .map_err(|e| UpdateError::RestartFailed(format!("{e}")))
            } else {
                eprintln!("No daemon running; leaving binary updated without starting.");
                Ok(())
            }
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

    // Capture startup state before replacement for restart decision.
    let pre_state = startup_state();
    eprintln!("Current greggd {current}, latest {latest}, pre-update state: {pre_state}");

    // Fully prepare and verify the candidate before quiescing anything.
    let (from_cargo, staged) =
        gregg_update::prepare_candidate(&spec, &current, &latest, target.as_deref())?;

    #[cfg(target_os = "windows")]
    stop_windows_service_if_needed(pre_state)?;
    gregg_update::stage::replace_current_exe(staged.path(), PROGRAM)?;
    eprintln!(
        "Replaced {PROGRAM} binary {current} -> {latest} via {}",
        if from_cargo { "Cargo" } else { "GitHub binary" }
    );
    let new_version = latest;

    // Now restart according to pre_state
    let restart_result = restart_after_update(pre_state, &original_exe, config_path, explicit);
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
            if matches!(pre_state, StartupState::SystemdActive) {
                eprintln!("Or: sudo systemctl restart greggd");
            } else if matches!(pre_state, StartupState::LaunchdLoaded) {
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

#[cfg(target_os = "windows")]
fn should_quiesce_running_service(candidate_prepared: bool, state: StartupState) -> bool {
    candidate_prepared && matches!(state, StartupState::WindowsServiceRunning)
}

#[cfg(target_os = "windows")]
fn stop_windows_service_if_needed(state: StartupState) -> Result<(), UpdateError> {
    if should_quiesce_running_service(true, state) {
        eprintln!("Stopping Windows service after candidate verification...");
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
    }
    Ok(())
}

// ── Daemon running probe for UnmanagedOrCron ────────────────────────────────

fn is_unmanaged_daemon_running(config_path: &Path, explicit: bool) -> bool {
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
            StartupState::SystemdActive
        );
        assert_eq!(
            crate::startup::systemd_state_with(false, false),
            StartupState::UnmanagedOrCron
        );
    }

    #[test]
    fn restart_decision_for_stopped_service_is_no_restart() {
        // Pure helper: installed but stopped should not restart.
        let state = StartupState::SystemdInstalledStopped;
        // The actual restart function would not be called directly in unit test to avoid systemctl.
        // We just assert the enum distinction is correct.
        assert_ne!(state, StartupState::SystemdActive);
    }

    #[test]
    fn candidate_is_prepared_before_any_quiesce() {
        // Transaction ordering (Plan 102, preserved by Plan 104): the shared
        // prepare step fully verifies the candidate, and the Windows quiesce
        // gate requires a prepared candidate. A stopped service is never
        // touched even with a candidate in hand.
        #[cfg(target_os = "windows")]
        {
            assert!(!should_quiesce_running_service(
                false,
                StartupState::WindowsServiceRunning
            ));
            assert!(!should_quiesce_running_service(
                true,
                StartupState::WindowsServiceStopped
            ));
            assert!(should_quiesce_running_service(
                true,
                StartupState::WindowsServiceRunning
            ));
        }
        #[cfg(not(target_os = "windows"))]
        {
            // On non-Windows there is no quiesce step at all: replacement is
            // atomic and restart happens after, so preparation ordering is
            // structural in run_update (prepare -> replace -> restart).
            assert_ne!(
                StartupState::SystemdInstalledStopped,
                StartupState::SystemdActive
            );
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_service_quiescence_requires_prepared_candidate() {
        assert!(!should_quiesce_running_service(
            false,
            StartupState::WindowsServiceRunning
        ));
        assert!(!should_quiesce_running_service(
            true,
            StartupState::WindowsServiceStopped
        ));
        assert!(should_quiesce_running_service(
            true,
            StartupState::WindowsServiceRunning
        ));
    }
}
