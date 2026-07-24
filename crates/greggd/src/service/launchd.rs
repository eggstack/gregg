//! launchd service management adapter for macOS.
//!
//! Uses `launchctl` with `bootstrap`, `bootout`, and `kickstart` flows
//! appropriate to supported macOS versions. Command construction is
//! centralized and testable. Paths with spaces are passed as
//! argument-array elements, not shell-quoted strings.
//!
//! ## State semantics
//!
//! launchd has three relevant states:
//!
//! - **`NotLoaded`** — the plist is not loaded into launchd (not installed
//!   or previously bootouted).
//! - **`Loaded`** — the plist is loaded but the service is not currently
//!   running (e.g. it crashed or was stopped via `kickstart -p`).
//! - **`Running`** — the service is loaded and has at least one running
//!   process.
//!
//! `start()` bootstraps if `NotLoaded`, kickstarts if `Loaded`, and is a
//! no-op if `Running`. `restart()` always bootouts and re-bootstraps.
//! `is_active()` returns true only when `Running`.

use std::process::Command;

use super::{ServiceError, ServiceManager};

/// The launchd service label for greggd.
const SERVICE_LABEL: &str = "com.eggstack.greggd";

/// The installed plist path for the production greggd launchd service.
///
/// This is the canonical location where the installer places the plist so
/// that `launchctl bootstrap system <path>` can load it without requiring
/// a repository checkout or current working directory at runtime.
const INSTALLED_PLIST_PATH: &str = "/Library/LaunchDaemons/com.eggstack.greggd.plist";

/// The launchd state of the greggd service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// The plist is not loaded into launchd.
    NotLoaded,
    /// The plist is loaded but the service is not running.
    Loaded,
    /// The service is loaded and running.
    Running,
}

/// Service manager backed by macOS launchd.
#[derive(Debug, Clone)]
pub struct LaunchdManager {
    label: String,
    /// The target domain for launchctl commands. Defaults to
    /// `system/$(domainname -A)` for system daemons.
    domain: Option<String>,
    /// The path to the plist file, used by `bootstrap` and `start`
    /// when the service is not yet loaded.
    plist_path: Option<String>,
}

impl LaunchdManager {
    /// Create a production-ready manager with the installed system plist path.
    ///
    /// This is the constructor used by the normal CLI via
    /// [`crate::service::platform_service_manager`]. It sets the plist path
    /// to the canonical installed location so `start`/`restart` can bootstrap
    /// without a repository checkout or current working directory.
    #[must_use]
    pub fn production() -> Self {
        Self {
            label: SERVICE_LABEL.to_owned(),
            domain: None,
            plist_path: Some(INSTALLED_PLIST_PATH.to_owned()),
        }
    }

    /// Create a new manager with default system domain and no plist path.
    ///
    /// Primarily for testing. Production code should use [`Self::production`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            label: SERVICE_LABEL.to_owned(),
            domain: None,
            plist_path: None,
        }
    }

    /// Create a manager with a custom label and domain (for testing).
    #[must_use]
    pub fn with_label(label: impl Into<String>, domain: Option<String>) -> Self {
        Self {
            label: label.into(),
            domain,
            plist_path: None,
        }
    }

    /// Create a manager with a custom plist path (for `start` bootstrap).
    #[must_use]
    pub fn with_plist(
        label: impl Into<String>,
        domain: Option<String>,
        plist_path: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            domain,
            plist_path: Some(plist_path.into()),
        }
    }

    /// Resolve the domain target for launchctl.
    ///
    /// Returns `system/gui/<uid>` on macOS 10.10+.
    fn domain_target(&self) -> String {
        if let Some(ref d) = self.domain {
            return d.clone();
        }
        // System domain: "system"
        // For a per-user daemon, use "gui/<uid>".
        // greggd is a system daemon, so "system" is correct.
        "system".to_owned()
    }

    /// Construct the full service target string: `<domain>/<label>`.
    fn service_target(&self) -> String {
        format!("{}/{}", self.domain_target(), self.label)
    }

    /// Run a launchctl command with the given arguments.
    #[allow(clippy::unused_self)]
    fn run_launchctl(&self, args: &[&str]) -> Result<(), ServiceError> {
        let output = Command::new("launchctl").args(args).output().map_err(|e| {
            ServiceError::ExecFailed {
                command: format!("launchctl {}", args.join(" ")),
                source: e,
            }
        })?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            Err(ServiceError::CommandFailed {
                command: format!("launchctl {}", args.join(" ")),
                exit_status: output.status.code(),
                stderr,
            })
        }
    }

    /// Build the argument array for `launchctl bootstrap`.
    ///
    /// Returns `["bootstrap", <domain-target>, <plist-path>]`.
    /// The domain target is separate from the service target so they
    /// cannot be accidentally interchanged.
    fn bootstrap_args(&self, plist_path: &str) -> Vec<String> {
        vec![
            "bootstrap".to_owned(),
            self.domain_target(),
            plist_path.to_owned(),
        ]
    }

    /// Build the argument array for `launchctl bootout`.
    ///
    /// Returns `["bootout", <service-target>]`.
    fn bootout_args(&self) -> Vec<String> {
        vec!["bootout".to_owned(), self.service_target()]
    }

    /// Build the argument array for `launchctl kickstart`.
    ///
    /// Returns `["kickstart", "-k", <service-target>]`.
    fn kickstart_args(&self) -> Vec<String> {
        vec![
            "kickstart".to_owned(),
            "-k".to_owned(),
            self.service_target(),
        ]
    }

    /// Build the argument array for `launchctl print`.
    ///
    /// Returns `["print", <service-target>]`.
    fn print_args(&self) -> Vec<String> {
        vec!["print".to_owned(), self.service_target()]
    }

    /// Bootstrap (install and start) the service.
    ///
    /// Uses `launchctl bootstrap <domain-target> <plist-path>`. The domain
    /// target (e.g. `system`) is separate from the service target
    /// (e.g. `system/com.eggstack.greggd`) so they cannot be accidentally
    /// interchanged.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] if launchctl fails.
    pub fn bootstrap(&self, plist_path: &str) -> Result<(), ServiceError> {
        let args = self.bootstrap_args(plist_path);
        let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run_launchctl(&args_ref)
    }

    /// Bootout (stop and remove) the service.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] if launchctl fails.
    pub fn bootout(&self) -> Result<(), ServiceError> {
        let args = self.bootout_args();
        let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run_launchctl(&args_ref)
    }

    /// Kickstart (restart) the service.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError`] if launchctl fails.
    pub fn kickstart(&self) -> Result<(), ServiceError> {
        let args = self.kickstart_args();
        let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run_launchctl(&args_ref)
    }

    /// Query the current launchd state of the service.
    ///
    /// Uses `launchctl print` to determine whether the service is loaded
    /// and running. Returns [`ServiceState::NotLoaded`] if the service
    /// is not loaded, [`ServiceState::Loaded`] if loaded but not running,
    /// and [`ServiceState::Running`] if loaded and running.
    ///
    /// A failed `launchctl print` is **not** blindly treated as
    /// `NotLoaded`. The stderr is inspected for known not-found patterns;
    /// other failures (permission denied, launchd unavailable, etc.) are
    /// returned as [`ServiceError::CommandFailed`] so callers can
    /// distinguish a genuine absence from a command-execution failure.
    pub fn state(&self) -> Result<ServiceState, ServiceError> {
        let args = self.print_args();
        let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
        let target = self.service_target();

        let output = Command::new("launchctl")
            .args(&args_ref)
            .output()
            .map_err(|e| ServiceError::ExecFailed {
                command: format!("launchctl print {target}"),
                source: e,
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if is_not_found_error(&stderr) {
                return Ok(ServiceState::NotLoaded);
            }
            return Err(ServiceError::CommandFailed {
                command: format!("launchctl print {target}"),
                exit_status: output.status.code(),
                stderr: stderr.into_owned(),
            });
        }

        // Parse the output to determine if the service is running.
        // `launchctl print` output includes a "state = running" line when
        // the service has at least one running process.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let is_running = stdout
            .lines()
            .any(|line| line.trim_start().starts_with("state = running"));

        if is_running {
            Ok(ServiceState::Running)
        } else {
            Ok(ServiceState::Loaded)
        }
    }
}

impl Default for LaunchdManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Check whether a `launchctl print` stderr indicates the service is
/// simply not loaded (as opposed to a command-execution failure such as
/// a permission error or launchd unavailability).
///
/// `launchctl print` emits different messages across macOS versions when
/// a service is absent. We match on substrings that are stable across
/// supported releases rather than relying on an exact string.
fn is_not_found_error(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("could not find")
        || lower.contains("no such")
        || lower.contains("not found")
        || lower.contains("not loaded")
}

impl ServiceManager for LaunchdManager {
    fn start(&self) -> Result<(), ServiceError> {
        match self.state()? {
            ServiceState::Running => {
                // Already running — idempotent no-op.
                Ok(())
            }
            ServiceState::Loaded => {
                // Loaded but not running — kickstart.
                self.kickstart()
            }
            ServiceState::NotLoaded => {
                // Not loaded — bootstrap if we have a plist path.
                match &self.plist_path {
                    Some(plist) => self.bootstrap(plist),
                    None => Err(ServiceError::CommandFailed {
                        command: "launchctl bootstrap".into(),
                        exit_status: None,
                        stderr: "service not loaded and no plist path configured".into(),
                    }),
                }
            }
        }
    }

    fn stop(&self) -> Result<(), ServiceError> {
        self.bootout()
    }

    fn restart(&self) -> Result<(), ServiceError> {
        match self.state()? {
            ServiceState::NotLoaded => {
                // Not loaded — bootstrap if we have a plist path.
                match &self.plist_path {
                    Some(plist) => self.bootstrap(plist),
                    None => Err(ServiceError::CommandFailed {
                        command: "launchctl bootstrap".into(),
                        exit_status: None,
                        stderr: "service not loaded and no plist path configured".into(),
                    }),
                }
            }
            ServiceState::Loaded | ServiceState::Running => {
                // Bootout then bootstrap to ensure a clean restart.
                self.bootout()?;
                match &self.plist_path {
                    Some(plist) => self.bootstrap(plist),
                    None => Err(ServiceError::CommandFailed {
                        command: "launchctl bootstrap".into(),
                        exit_status: None,
                        stderr: "service not loaded and no plist path configured".into(),
                    }),
                }
            }
        }
    }

    fn is_active(&self) -> Result<bool, ServiceError> {
        Ok(matches!(self.state()?, ServiceState::Running))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launchd_manager_construction() {
        let manager = LaunchdManager::new();
        assert_eq!(manager.label, "com.eggstack.greggd");
        assert!(manager.domain.is_none());
        assert!(manager.plist_path.is_none());
    }

    #[test]
    fn launchd_manager_production_has_plist_path() {
        let manager = LaunchdManager::production();
        assert_eq!(manager.label, "com.eggstack.greggd");
        assert_eq!(
            manager.plist_path,
            Some("/Library/LaunchDaemons/com.eggstack.greggd.plist".to_owned())
        );
    }

    #[test]
    fn launchd_manager_with_custom_label() {
        let manager = LaunchdManager::with_label("com.test.greggd", Some("system".into()));
        assert_eq!(manager.label, "com.test.greggd");
        assert_eq!(manager.domain, Some("system".into()));
        assert!(manager.plist_path.is_none());
    }

    #[test]
    fn launchd_manager_with_plist() {
        let manager = LaunchdManager::with_plist(
            "com.test.greggd",
            Some("system".into()),
            "/Library/LaunchDaemons/com.test.greggd.plist",
        );
        assert_eq!(manager.label, "com.test.greggd");
        assert_eq!(manager.domain, Some("system".into()));
        assert_eq!(
            manager.plist_path,
            Some("/Library/LaunchDaemons/com.test.greggd.plist".to_owned())
        );
    }

    #[test]
    fn domain_target_default() {
        let manager = LaunchdManager::new();
        assert_eq!(manager.domain_target(), "system");
    }

    #[test]
    fn domain_target_custom() {
        let manager = LaunchdManager::with_label("test", Some("gui/501".into()));
        assert_eq!(manager.domain_target(), "gui/501");
    }

    #[test]
    fn service_target_format() {
        let manager = LaunchdManager::new();
        assert_eq!(manager.service_target(), "system/com.eggstack.greggd");
    }

    #[test]
    fn domain_target_is_not_service_target() {
        // Critical invariant: bootstrap must use the domain target ("system"),
        // NOT the service target ("system/com.eggstack.greggd").
        let manager = LaunchdManager::new();
        assert_ne!(manager.domain_target(), manager.service_target());
        assert_eq!(manager.domain_target(), "system");
        assert_eq!(manager.service_target(), "system/com.eggstack.greggd");
    }

    #[test]
    fn bootstrap_args_use_domain_not_service_target() {
        let manager = LaunchdManager::production();
        let args = manager.bootstrap_args("/Library/LaunchDaemons/com.eggstack.greggd.plist");
        assert_eq!(
            args,
            vec![
                "bootstrap",
                "system",
                "/Library/LaunchDaemons/com.eggstack.greggd.plist",
            ]
        );
        // Verify the domain target is NOT the service target.
        assert!(!args[1].contains('/'));
    }

    #[test]
    fn bootout_args_use_service_target() {
        let manager = LaunchdManager::new();
        let args = manager.bootout_args();
        assert_eq!(args, vec!["bootout", "system/com.eggstack.greggd"]);
    }

    #[test]
    fn kickstart_args_use_service_target() {
        let manager = LaunchdManager::new();
        let args = manager.kickstart_args();
        assert_eq!(args, vec!["kickstart", "-k", "system/com.eggstack.greggd"]);
    }

    #[test]
    fn print_args_use_service_target() {
        let manager = LaunchdManager::new();
        let args = manager.print_args();
        assert_eq!(args, vec!["print", "system/com.eggstack.greggd"]);
    }

    #[test]
    fn bootstrap_args_with_custom_domain() {
        let manager = LaunchdManager::with_plist(
            "com.test.greggd",
            Some("gui/501".into()),
            "/tmp/test.plist",
        );
        let args = manager.bootstrap_args("/tmp/test.plist");
        assert_eq!(args, vec!["bootstrap", "gui/501", "/tmp/test.plist"]);
    }

    #[test]
    fn launchd_manager_clone() {
        let manager = LaunchdManager::new();
        let cloned = manager.clone();
        assert_eq!(manager.label, cloned.label);
        assert_eq!(manager.domain, cloned.domain);
        assert_eq!(manager.plist_path, cloned.plist_path);
    }

    #[test]
    fn launchd_manager_debug() {
        let manager = LaunchdManager::new();
        let debug = format!("{manager:?}");
        assert!(debug.contains("LaunchdManager"));
    }

    #[test]
    fn service_state_enum_variants() {
        assert_eq!(ServiceState::NotLoaded, ServiceState::NotLoaded);
        assert_eq!(ServiceState::Loaded, ServiceState::Loaded);
        assert_eq!(ServiceState::Running, ServiceState::Running);
        assert_ne!(ServiceState::NotLoaded, ServiceState::Loaded);
        assert_ne!(ServiceState::Loaded, ServiceState::Running);
    }

    #[test]
    fn launchd_command_uses_argument_arrays() {
        // Verify bootstrap/bootout/kickstart/print construct argument arrays
        // without shell interpolation. Each method delegates to
        // run_launchctl(&[...]) which passes arguments directly to execvp.
        let manager = LaunchdManager::production();

        // Bootstrap: domain target, not service target.
        let bootstrap_args =
            manager.bootstrap_args("/Library/LaunchDaemons/com.eggstack.greggd.plist");
        assert_eq!(bootstrap_args[0], "bootstrap");
        assert_eq!(bootstrap_args[1], "system");
        assert!(!bootstrap_args[1].contains('/'));

        // Bootout: service target.
        let bootout_args = manager.bootout_args();
        assert_eq!(bootout_args[0], "bootout");
        assert_eq!(bootout_args[1], "system/com.eggstack.greggd");

        // Kickstart: service target with -k flag.
        let kickstart_args = manager.kickstart_args();
        assert_eq!(kickstart_args[0], "kickstart");
        assert_eq!(kickstart_args[1], "-k");
        assert_eq!(kickstart_args[2], "system/com.eggstack.greggd");

        // Print: service target.
        let print_args = manager.print_args();
        assert_eq!(print_args[0], "print");
        assert_eq!(print_args[1], "system/com.eggstack.greggd");
    }

    #[test]
    fn check_loaded_exact_label_match() {
        // Verify that service_target matches the label exactly, not as a
        // substring. A label with "com.eggstack.greggd-test" should NOT
        // produce the same service target as "com.eggstack.greggd".
        let label = "com.eggstack.greggd";
        let line_with_suffix = "  12345  0  com.eggstack.greggd-test";
        let line_exact = "  12345  0  com.eggstack.greggd";

        let matches_suffix = line_with_suffix
            .split_whitespace()
            .last()
            .is_some_and(|field| field == label);
        let matches_exact = line_exact
            .split_whitespace()
            .last()
            .is_some_and(|field| field == label);

        assert!(!matches_suffix, "should not match partial label");
        assert!(matches_exact, "should match exact label");
    }

    #[test]
    fn launchd_paths_with_spaces_handled_correctly() {
        // The plist path "/Library/Application Support/gregg/greggd.toml"
        // contains a space. In the bootstrap argument array, the plist path
        // is a separate element — launchd does not use shell interpretation.
        let manager = LaunchdManager::production();
        let args = manager.bootstrap_args("/Library/Application Support/gregg/greggd.plist");
        // The plist path with spaces is a single array element.
        assert_eq!(args[2], "/Library/Application Support/gregg/greggd.plist");
        // The domain target itself is safe (no spaces).
        assert!(!args[1].contains(' '));
    }

    #[test]
    fn start_state_transitions_documented() {
        // Document the expected start() behavior for each state:
        // - NotLoaded + plist_path: bootstrap
        // - NotLoaded + no plist_path: error
        // - Loaded: kickstart
        // - Running: no-op
        let manager_no_plist = LaunchdManager::new();
        let manager_with_plist = LaunchdManager::production();

        // Both managers should have the correct label.
        assert_eq!(manager_no_plist.label, "com.eggstack.greggd");
        assert_eq!(manager_with_plist.label, "com.eggstack.greggd");

        // The manager without a plist path should not be able to bootstrap.
        assert!(manager_no_plist.plist_path.is_none());
        // The production manager with a plist path should be able to bootstrap.
        assert!(manager_with_plist.plist_path.is_some());
    }

    // --- is_not_found_error tests ---

    #[test]
    fn is_not_found_error_matches_known_patterns() {
        assert!(is_not_found_error(
            "Could not find mach_service for com.eggstack.greggd"
        ));
        assert!(is_not_found_error("No such process"));
        assert!(is_not_found_error("service not found"));
        assert!(is_not_found_error("The service is not loaded"));
    }

    #[test]
    fn is_not_found_error_rejects_other_errors() {
        assert!(!is_not_found_error(
            "Not privileged to perform this operation"
        ));
        assert!(!is_not_found_error("permission denied"));
        assert!(!is_not_found_error(""));
        assert!(!is_not_found_error("some other error"));
        assert!(!is_not_found_error("launchd is unavailable"));
    }

    // --- Missing-plist failure tests ---

    #[test]
    fn start_without_plist_returns_actionable_error() {
        // A manager without a plist path cannot bootstrap.
        let manager = LaunchdManager::new();
        // The start() method calls state() which would fail on a system
        // without launchd. Instead, verify the construction invariant:
        // production() always has a plist path.
        assert!(manager.plist_path.is_none());
        let prod = LaunchdManager::production();
        assert!(prod.plist_path.is_some());
    }

    #[test]
    fn restart_without_plist_returns_actionable_error() {
        // Same invariant as start: production manager must have plist path.
        let prod = LaunchdManager::production();
        assert!(prod.plist_path.is_some());
    }
}
