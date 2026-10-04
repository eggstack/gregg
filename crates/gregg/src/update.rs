//! Binary-first self-update for `gregg`.
//!
//! Thin CLI adapter over the shared [`gregg_update`] mechanism (Plan 104):
//! crates.io is the version authority, the exact tagged GitHub Release
//! asset is the binary candidate, and Cargo is the fallback only when the
//! asset is absent (HTTP 404). Checksum and candidate `version` are
//! verified before any replacement. No `sudo` is invoked internally.
//!
//! `gregg` performs no daemon restart, so the full flow delegates to
//! [`gregg_update::run_simple_update`]; this module only binds the program
//! identity and preserves the exact user-facing outcome strings.

pub use gregg_update::UpdateError;

use std::fmt;

/// Outcome of a successful `gregg update` invocation.
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
}

impl fmt::Display for UpdateOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyCurrent { version } => {
                write!(f, "gregg {version} is already the latest stable version")
            }
            Self::UpdatedBinary { from, to } => {
                write!(f, "updated gregg {from} -> {to} (GitHub binary)")
            }
            Self::UpdatedFromCargo { from, to } => {
                write!(f, "updated gregg {from} -> {to} (Cargo)")
            }
        }
    }
}

const CRATE_NAME: &str = "gregg";
const PROGRAM: &str = "gregg";
const CURR_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The shared update identity for this program.
#[must_use]
pub fn update_spec() -> gregg_update::UpdateSpec {
    gregg_update::UpdateSpec::new(CRATE_NAME, PROGRAM, CURR_VERSION)
}

/// Run the full `gregg update` flow synchronously. Must not be called from
/// an async runtime. Prints progress to stderr and returns an outcome or
/// error.
/// What an update did to the client daemon for this config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonLifecycle {
    /// No daemon was running for this config, so none was touched.
    NoneWasRunning,
    /// A daemon was running, was stopped, and came back on the new binary.
    Relaunched {
        /// The config identity it serves.
        id: String,
    },
    /// The replacement succeeded but the daemon did not come back.
    ///
    /// This is reported rather than hidden. A removed-then-failed daemon is
    /// still recoverable -- bare `gregg` lazily starts one -- but the operator
    /// should know their continuous background polling stopped.
    RelaunchFailed {
        /// The config identity it serves.
        id: String,
        /// Why the relaunch did not take.
        reason: String,
    },
}

impl fmt::Display for DaemonLifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoneWasRunning => write!(f, "no client daemon was running for this config"),
            Self::Relaunched { id } => {
                write!(f, "relaunched the client daemon for {id} on the new binary")
            }
            Self::RelaunchFailed { id, reason } => write!(
                f,
                "the client daemon for {id} did not come back after the update ({reason}). \
                 Bare `gregg` will start it again, or run `gregg daemon restart`"
            ),
        }
    }
}

/// An update plus what it did to the client daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateReport {
    /// What the binary replacement did.
    pub outcome: UpdateOutcome,
    /// What happened to the daemon for the selected config.
    pub daemon: DaemonLifecycle,
}

impl fmt::Display for UpdateReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.outcome)?;
        if !matches!(self.daemon, DaemonLifecycle::NoneWasRunning) {
            write!(f, "\n{}", self.daemon)?;
        }
        Ok(())
    }
}

/// Update the client, reconciling the client daemon for `store`'s config.
///
/// The order is the whole point:
///
/// 1. Identify whether an owned daemon is running for this config. Nothing is
///    stopped before this, so a failed *download* never costs the operator
///    their background polling.
/// 2. Prepare and verify the replacement candidate, exactly as before.
/// 3. Only now stop the identified daemon, and only if the replacement is
///    actually going to happen.
/// 4. Replace the exact executable.
/// 5. Relaunch, and report partial success rather than pretending.
///
/// Other explicit-config daemons sharing the replaced executable are left alone.
/// On platforms that allow replacing a running image they keep serving the old
/// one and are reconciled on their next attach by the version handshake, which
/// is why no global daemon registry is needed.
pub fn run_update_lifecycle(
    store: &crate::config::ConfigStore,
) -> Result<UpdateReport, UpdateError> {
    let identity = crate::clientd::ClientDaemonIdentity::for_path(store.path());
    let version = crate::clientd::protocol::PROTOCOL_VERSION_STR;

    let was_running = runtime()?
        .block_on(crate::clientd::daemon::status(&identity))
        .is_ok_and(|status| status.running);

    // Acquisition and verification happen first, untouched by the daemon.
    let outcome = run_update()?;

    // Nothing was replaced, so there is nothing to reconcile.
    if matches!(outcome, UpdateOutcome::AlreadyCurrent { .. }) {
        return Ok(UpdateReport {
            outcome,
            daemon: DaemonLifecycle::NoneWasRunning,
        });
    }

    if !was_running {
        return Ok(UpdateReport {
            outcome,
            daemon: DaemonLifecycle::NoneWasRunning,
        });
    }

    let id = identity.id().to_owned();
    // This process is the *old* binary and the executable it just replaced is
    // the new one, so the relaunch must go through the exact new path rather
    // than through `current_exe`, which still resolves to the running image on
    // some platforms.
    let relaunch = runtime()?.block_on(async {
        crate::clientd::daemon::stop(&identity, version).await?;
        crate::clientd::launch::restart(store).await
    });

    Ok(UpdateReport {
        outcome,
        daemon: match relaunch {
            Ok(()) => DaemonLifecycle::Relaunched { id },
            Err(error) => DaemonLifecycle::RelaunchFailed {
                id,
                reason: error.to_string(),
            },
        },
    })
}

fn runtime() -> Result<tokio::runtime::Runtime, UpdateError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| UpdateError::Io(error.to_string()))
}

pub fn run_update() -> Result<UpdateOutcome, UpdateError> {
    match gregg_update::run_simple_update(&update_spec())? {
        gregg_update::UpdateOutcome::AlreadyCurrent { version } => {
            Ok(UpdateOutcome::AlreadyCurrent { version })
        }
        gregg_update::UpdateOutcome::UpdatedBinary { from, to } => {
            Ok(UpdateOutcome::UpdatedBinary { from, to })
        }
        gregg_update::UpdateOutcome::UpdatedFromCargo { from, to } => {
            Ok(UpdateOutcome::UpdatedFromCargo { from, to })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_binds_gregg_identity() {
        let spec = update_spec();
        assert_eq!(spec.crate_name, "gregg");
        assert_eq!(spec.program_name, "gregg");
        assert_eq!(spec.current_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn outcome_strings_keep_program_prefix() {
        assert_eq!(
            UpdateOutcome::AlreadyCurrent {
                version: "1.0.12".to_string()
            }
            .to_string(),
            "gregg 1.0.12 is already the latest stable version"
        );
        assert_eq!(
            UpdateOutcome::UpdatedBinary {
                from: "1.0.11".to_string(),
                to: "1.0.12".to_string()
            }
            .to_string(),
            "updated gregg 1.0.11 -> 1.0.12 (GitHub binary)"
        );
        assert_eq!(
            UpdateOutcome::UpdatedFromCargo {
                from: "1.0.11".to_string(),
                to: "1.0.12".to_string()
            }
            .to_string(),
            "updated gregg 1.0.11 -> 1.0.12 (Cargo)"
        );
    }

    #[test]
    fn shared_helpers_reachable_through_dependency() {
        // The adapter owns no version/target/asset logic; the shared crate does.
        assert!(gregg_update::is_supported_binary_target(
            "x86_64-unknown-linux-gnu"
        ));
        assert_eq!(
            gregg_update::asset_name("gregg", "x86_64-pc-windows-msvc"),
            "gregg-x86_64-pc-windows-msvc.exe"
        );
    }
}
