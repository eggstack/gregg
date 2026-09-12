//! Startup installation, instruction rendering, and restart helpers for `greggd`.
//!
//! The foreground daemon (`greggd run`) remains unaware of who supervises it.
//! This module owns the explicit deployment boundary: installing system
//! services, rendering cron entries, and restarting through the appropriate
//! manager. No collector, sampler, or HTTP code depends on this module.
//!
//! Ownership split (Plan 105, behavior-preserving):
//!
//! - [`method`] — method identity, standard paths, detection, selection;
//! - [`process`] — bounded child-process execution shared by probes/commands;
//! - [`systemd`] — unit content, installation, restart;
//! - [`launchd`] — plist content, installation, restart;
//! - [`cron`] — shell quoting, watchdog block rendering/merging, installation;
//! - [`state`] — detected manager state for restart/update decisions;
//! - [`install`] — install errors, atomic writes, privilege guidance,
//!   install dispatch, instruction rendering, restart coordination.
//!
//! The re-exports below preserve the historical `crate::startup::X` paths
//! so no call site changes with the move.

pub mod cron;
pub mod install;
pub mod launchd;
pub mod method;
pub mod process;
pub mod state;
pub mod systemd;

/// Ownership of a startup artifact relative to the exact executable being
/// uninstalled. Presence alone is never sufficient evidence of ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactOwnership {
    /// No artifact was found.
    Absent,
    /// The artifact targets the exact executable under consideration.
    Owned,
    /// The artifact targets another installation.
    Foreign,
    /// The artifact exists, but its target could not be established safely.
    Unknown,
}

impl ArtifactOwnership {
    #[must_use]
    pub fn is_present(self) -> bool {
        !matches!(self, Self::Absent)
    }

    #[must_use]
    pub fn is_owned(self) -> bool {
        matches!(self, Self::Owned)
    }
}

pub use cron::{
    cron_block, cron_block_ownership, cron_block_with_config, cron_uninstall_changed,
    cron_uninstall_changed_for, install_cron, merge_crontab, remove_managed_cron_block,
    shell_quote, uninstall_cron, uninstall_cron_for, ShellQuoteError, CRON_MANAGED_MARKER,
};
pub use install::{
    install_startup, render_instructions, restart_daemon, restart_with_state, InstallError,
};
pub use launchd::{
    install_launchd, launchd_artifact_ownership, launchd_plist_content, launchd_uninstall_steps,
    parse_program_arguments, uninstall_launchd,
};
pub use method::{
    auto_detect_method, auto_method_for, is_systemd_environment, is_systemd_environment_with,
    launchd_label, resolve_startup_method, resolve_startup_method_with, standard_launchd_binary,
    standard_launchd_config, standard_launchd_plist_path, standard_systemd_binary,
    standard_systemd_config, standard_systemd_config_dir, standard_systemd_unit_path,
    StartupMethod, StartupMethodArg,
};
pub use state::{launchd_state_with, startup_state, systemd_state_with, StartupState};
pub use systemd::{
    install_systemd, parse_exec_start_target, systemd_artifact_ownership, systemd_uninstall_steps,
    systemd_unit_content, uninstall_systemd,
};
