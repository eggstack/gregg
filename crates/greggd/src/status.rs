//! Read-only local diagnostic status for `greggd status` (Plan 106).
//!
//! `status` composes information Gregg already computes: the resolved
//! config path and bind address, the binary version, the authoritative
//! bounded `/v2/healthz` probe shared with `croncheck`, and the detected
//! startup-manager state. It never starts, stops, restarts, installs, or
//! mutates configuration or service-manager state, and it never invokes
//! `sudo` or authorization.
//!
//! Exit contract: exit 0 when a valid Gregg health endpoint answered
//! (`ready`, `warming`, or `failed` — the same "running" definition
//! `croncheck` uses); nonzero when configuration is invalid/unreadable or
//! no valid Gregg endpoint answered (see [`status_is_present`]). The printed
//! report always shows the classification, so `failed` health is visible
//! even though the endpoint counts as present.

use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::cli::{croncheck_target, HealthProbe};
use crate::config::Config;
use crate::startup::StartupState;

/// A gathered status snapshot. Gathering is separated from rendering so
/// tests can inject deterministic probe/startup results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    /// Rendered binary version (`greggd 1.0.12`).
    pub version: String,
    /// Resolved config path for this invocation.
    pub config_path: PathBuf,
    /// Configured canonical bind address.
    pub listen: SocketAddr,
    /// Classified local health probe result.
    pub health: HealthProbe,
    /// Detected startup-manager state.
    pub startup: StartupState,
}

/// Stable health token rendered by [`render_status`].
#[must_use]
pub fn health_token(health: HealthProbe) -> &'static str {
    match health {
        HealthProbe::Ready => "ready",
        HealthProbe::Warming => "warming",
        HealthProbe::Failed => "failed",
        HealthProbe::Unreachable => "unreachable",
        HealthProbe::NotGregg => "not-gregg",
    }
}

/// Gather a status report with injected probe/startup results.
///
/// `probe` receives the loopback probe target derived from `config`
/// (wildcards become loopback, as in `croncheck`) and returns the
/// classified health. Production callers pass [`crate::cli::probe_health`]
/// and [`crate::startup::startup_state`].
pub fn gather_status(
    config: &Config,
    config_path: &Path,
    version: String,
    probe: impl FnOnce(SocketAddr) -> HealthProbe,
    startup: StartupState,
) -> StatusReport {
    let target = croncheck_target(config);
    StatusReport {
        version,
        config_path: config_path.to_path_buf(),
        listen: SocketAddr::new(config.host, config.port),
        health: probe(target),
        startup,
    }
}

/// Render a status report in the stable human-readable form:
///
/// ```text
/// version: greggd 1.0.12
/// config: /etc/gregg/greggd.toml
/// listen: 127.0.0.1:11310
/// health: ready
/// startup: unmanaged-or-cron
/// ```
///
/// `health: not-gregg` means something answered the probe but is not a
/// valid Gregg health endpoint; no process ownership is inferred from port
/// occupancy.
#[must_use]
pub fn render_status(report: &StatusReport) -> String {
    format!(
        "version: {}\nconfig: {}\nlisten: {}\nhealth: {}\nstartup: {}\n",
        report.version,
        report.config_path.display(),
        report.listen,
        health_token(report.health),
        report.startup,
    )
}

/// Whether the report counts as "a valid Gregg endpoint is present": any
/// valid readiness state, including warming and failed. This matches
/// `croncheck`'s definition of running; the printed `health:` token still
/// distinguishes which state was observed.
#[must_use]
pub fn status_is_present(report: &StatusReport) -> bool {
    matches!(
        report.health,
        HealthProbe::Ready | HealthProbe::Warming | HealthProbe::Failed
    )
}

/// Short machine-stable outcome word for messaging.
#[must_use]
pub fn status_outcome(report: &StatusReport) -> &'static str {
    if status_is_present(report) {
        "running"
    } else {
        "not running"
    }
}

impl fmt::Display for StatusReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", render_status(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn test_config() -> Config {
        Config {
            name: "status-test".to_string(),
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 11310,
            sample_interval_ms: 1000,
            stale_after_ms: 5000,
        }
    }

    fn report_with(health: HealthProbe, startup: StartupState) -> StatusReport {
        gather_status(
            &test_config(),
            Path::new("/tmp/greggd.toml"),
            "greggd 1.0.12".to_string(),
            |_| health,
            startup,
        )
    }

    #[test]
    fn ready_endpoint_is_present_and_running() {
        let report = report_with(HealthProbe::Ready, StartupState::UnmanagedOrCron);
        assert!(status_is_present(&report));
        assert_eq!(status_outcome(&report), "running");
        let text = render_status(&report);
        assert!(text.contains("version: greggd 1.0.12"));
        assert!(text.contains("config: /tmp/greggd.toml"));
        assert!(text.contains("listen: 127.0.0.1:11310"));
        assert!(text.contains("health: ready"));
        assert!(text.contains("startup: unmanaged-or-cron"));
    }

    #[test]
    fn warming_endpoint_counts_as_present() {
        let report = report_with(HealthProbe::Warming, StartupState::UnmanagedOrCron);
        assert!(status_is_present(&report));
        assert!(render_status(&report).contains("health: warming"));
    }

    #[test]
    fn failed_health_counts_as_present_without_leaking_internals() {
        let report = report_with(HealthProbe::Failed, StartupState::SystemdActive);
        assert!(status_is_present(&report));
        let text = render_status(&report);
        assert!(text.contains("health: failed"));
        assert!(text.contains("startup: systemd-active"));
    }

    #[test]
    fn refused_endpoint_is_not_present() {
        let report = report_with(HealthProbe::Unreachable, StartupState::UnmanagedOrCron);
        assert!(!status_is_present(&report));
        assert_eq!(status_outcome(&report), "not running");
        assert!(render_status(&report).contains("health: unreachable"));
    }

    #[test]
    fn non_gregg_occupant_is_distinguished_from_absence() {
        let report = report_with(HealthProbe::NotGregg, StartupState::UnmanagedOrCron);
        assert!(!status_is_present(&report));
        assert!(render_status(&report).contains("health: not-gregg"));
    }

    #[test]
    fn startup_state_is_rendered_from_injected_state() {
        let report = report_with(HealthProbe::Ready, StartupState::SystemdInstalledStopped);
        assert!(render_status(&report).contains("startup: systemd-installed-stopped"));
    }

    #[test]
    fn gathering_is_pure_and_read_only() {
        // Gathering takes an immutable config reference plus injected
        // results and returns a value; calling it twice yields the same
        // report and there is no path that installs, starts, stops, or
        // restarts anything (no `&mut`, no manager handles in scope).
        let first = report_with(HealthProbe::Ready, StartupState::UnmanagedOrCron);
        let second = report_with(HealthProbe::Ready, StartupState::UnmanagedOrCron);
        assert_eq!(first, second);
    }
}
