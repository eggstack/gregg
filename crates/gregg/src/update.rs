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
