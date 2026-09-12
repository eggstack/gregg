//! Shared binary-first self-update mechanics for `gregg` and `greggd`.
//!
//! This crate is workspace-internal infrastructure (Plan 104), not a
//! user-facing product. It owns cross-program update mechanics only:
//!
//! ```text
//! stable-version parsing/comparison
//! supported-target mapping
//! release asset naming/URL construction
//! curl and Cargo discovery
//! bounded download/build execution
//! SHA-256 verification
//! staged candidate validation
//! executable replacement
//! shared update error/outcome primitives
//! ```
//!
//! A caller provides an [`UpdateSpec`] identifying its program and receives
//! a prepared or replaced result. The crate knows nothing about systemd,
//! launchd, cron, SCM, TUI state, or `EggPool` — daemon activation/restart
//! policy stays in `greggd`, CLI presentation stays in each application
//! crate, and the wire contract stays in `gregg-protocol`.
//!
//! Update contract (unchanged from Plans 099-102):
//!
//! - crates.io `max_stable_version` is the version authority;
//! - the exact tagged GitHub Release asset is the binary candidate;
//! - Cargo is the fallback only when the asset is absent (HTTP 404);
//! - checksum and candidate `version` are verified before any replacement;
//! - Unix replacement uses `self-replace` (same-filesystem atomic rename
//!   where practical); Windows uses the same helper for running-image
//!   semantics;
//! - no `sudo` is invoked internally.

pub mod error;
pub mod exec;
pub mod stage;
pub mod target;
pub mod uninstall;
pub mod verify;
pub mod version;

pub use error::UpdateError;
pub use exec::DownloadOutcome;
pub use stage::StagedCandidate;
pub use target::{
    asset_name, detect_target, detect_target_for, github_urls, is_supported_binary_target,
    GITHUB_REPO, SUPPORTED_TARGETS,
};
pub use uninstall::{
    candidate_cargo_root_for_exe, cargo_list_contains_package, detect_cargo_ownership,
    preflight_uninstall_writable, self_delete_current_exe, CargoOwnership,
};
pub use version::{compare_versions, is_update_available, parse_stable_version};

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Identity of one updatable program.
///
/// Conceptually the caller provides `crate_name`, `program_name`,
/// `current_version`, and the release repository, and receives a
/// prepared/replaced result without the shared crate knowing anything
/// about service managers or UI state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateSpec {
    /// crates.io crate name (the version authority key).
    pub crate_name: String,
    /// Binary/release-asset program name (`gregg` or `greggd`).
    pub program_name: String,
    /// Currently installed version (`env!("CARGO_PKG_VERSION")`).
    pub current_version: String,
}

impl UpdateSpec {
    /// Build a spec for one of the two Gregg programs.
    #[must_use]
    pub fn new(crate_name: &str, program_name: &str, current_version: &str) -> Self {
        Self {
            crate_name: crate_name.to_string(),
            program_name: program_name.to_string(),
            current_version: current_version.to_string(),
        }
    }
}

/// Outcome of a successful simple (non-daemon) update.
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
    /// Replaced via the Cargo fallback (asset absent with HTTP 404).
    UpdatedFromCargo {
        /// Previous version.
        from: String,
        /// Installed version.
        to: String,
    },
}

impl std::fmt::Display for UpdateOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyCurrent { version } => {
                write!(f, "{version} is already the latest stable version")
            }
            Self::UpdatedBinary { from, to } => {
                write!(f, "updated {from} -> {to} (GitHub binary)")
            }
            Self::UpdatedFromCargo { from, to } => {
                write!(f, "updated {from} -> {to} (Cargo)")
            }
        }
    }
}

/// A resolved update plan: the newest stable version and how to fetch it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdatePlan {
    /// Already current; no fetch needed.
    AlreadyCurrent {
        /// Installed (and latest) version.
        version: String,
    },
    /// A newer stable version exists.
    Available {
        /// Installed version.
        current: String,
        /// Newest stable version.
        latest: String,
        /// Prebuilt target, or `None` for source-only hosts (Cargo path).
        target: Option<String>,
    },
}

/// Resolve the update plan for `spec`: query crates.io, compare versions,
/// and detect the host target. Performs no download and mutates nothing.
pub fn resolve_plan(spec: &UpdateSpec) -> Result<UpdatePlan, UpdateError> {
    let latest = exec::fetch_latest_stable_version(
        &spec.crate_name,
        &spec.program_name,
        &spec.current_version,
    )?;
    let current = spec.current_version.clone();
    let ordering =
        compare_versions(&current, &latest).ok_or_else(|| UpdateError::InvalidVersion {
            input: format!("current={current} latest={latest}"),
            reason: "failed to parse version".to_string(),
        })?;
    if ordering != std::cmp::Ordering::Less {
        return Ok(UpdatePlan::AlreadyCurrent { version: current });
    }
    Ok(UpdatePlan::Available {
        current,
        latest,
        target: detect_target(),
    })
}

/// Prepare (download, checksum-verify, identity-verify, stage) the update
/// candidate for an available plan.
///
/// Returns the staged candidate plus whether it came from the Cargo
/// fallback. Cargo is used only when the host has no supported prebuilt
/// target or the exact asset is absent (HTTP 404); checksum/version
/// mismatches are hard errors and never fall back.
#[allow(clippy::too_many_lines)]
pub fn prepare_candidate(
    spec: &UpdateSpec,
    current: &str,
    latest: &str,
    target_opt: Option<&str>,
) -> Result<(bool, StagedCandidate), UpdateError> {
    let supported = target_opt.is_some_and(is_supported_binary_target);
    if !supported {
        eprintln!(
            "No prebuilt {} asset for {}/{} (target {target_opt:?}); trying Cargo fallback...",
            spec.program_name,
            std::env::consts::OS,
            std::env::consts::ARCH,
        );
        let staged = cargo_fallback(&spec.program_name, latest)?;
        return Ok((true, staged));
    }

    let target = target_opt.expect("supported target checked above");
    let (asset_url, sha_url) = github_urls(&spec.program_name, target, latest);
    eprintln!(
        "Latest {} is {latest} (current {current}); downloading {asset_url} ...",
        spec.program_name
    );

    let curl = exec::find_curl()?;
    let temp_dir = stage::create_temp_dir(&format!("{}-update", spec.program_name))?;

    let asset_name_str = asset_name(&spec.program_name, target);
    let asset_path = temp_dir.path().join(&asset_name_str);
    let sha_path = temp_dir.path().join(format!("{asset_name_str}.sha256"));

    match exec::download_file(&curl, &asset_url, &asset_path) {
        DownloadOutcome::Success => match exec::download_file(&curl, &sha_url, &sha_path) {
            DownloadOutcome::Success => {
                verify::verify_checksum(&asset_path, &sha_path)?;
                verify::validate_candidate(&asset_path, &spec.program_name, latest)?;
                Ok((false, StagedCandidate::new(temp_dir, asset_path)))
            }
            DownloadOutcome::NotFound => Err(UpdateError::ChecksumRetrieval(format!(
                "checksum not found at {sha_url} (HTTP 404)"
            ))),
            DownloadOutcome::Failed(reason) => Err(UpdateError::ChecksumRetrieval(reason)),
        },
        DownloadOutcome::NotFound => {
            eprintln!("No prebuilt asset at {asset_url} (HTTP 404); falling back to Cargo...");
            let staged = cargo_fallback(&spec.program_name, latest)?;
            Ok((true, staged))
        }
        DownloadOutcome::Failed(reason) => Err(UpdateError::ReleaseDownloadFailed {
            url: asset_url,
            reason,
        }),
    }
}

/// Build and stage the program via `cargo install --locked` into an
/// exclusive owner-private root, then identity-verify the result.
pub fn cargo_fallback(program: &str, version: &str) -> Result<StagedCandidate, UpdateError> {
    let cargo_bin = exec::find_cargo()?;
    let temp_root = stage::create_temp_dir(&format!("gregg-cargo-{program}"))?;
    let cargo_root = temp_root.path().join("cargo-root");
    std::fs::create_dir_all(&cargo_root)
        .map_err(|e| UpdateError::Io(format!("failed to create cargo root: {e}")))?;
    let cargo_root_str = cargo_root.to_string_lossy().to_string();
    let version_arg = format!("={version}");
    let mut cmd = Command::new(&cargo_bin);
    cmd.args([
        "install",
        "--locked",
        "--version",
        &version_arg,
        "--root",
        &cargo_root_str,
        program,
    ]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = exec::run_command_with_timeout_for_cargo(cmd, exec::CARGO_TIMEOUT)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(UpdateError::CargoFallback(format!(
            "cargo install {program} --version ={version} failed (status {:?}): {stderr}",
            output.status.code()
        )));
    }
    let bin_name = if cfg!(windows) {
        format!("{program}.exe")
    } else {
        program.to_string()
    };
    let staged = cargo_root.join("bin").join(&bin_name);
    if !staged.exists() {
        return Err(UpdateError::CargoFallback(format!(
            "cargo install succeeded but {} not found",
            staged.display()
        )));
    }
    verify::validate_candidate(&staged, program, version)?;
    Ok(StagedCandidate::new(temp_root, staged))
}

/// Run the full simple update flow: resolve, permission-probe, prepare,
/// replace. No daemon restart is performed; `greggd` coordinates its own
/// activation around [`prepare_candidate`] instead.
///
/// Must not be called from an async runtime. Prints progress to stderr.
pub fn run_simple_update(spec: &UpdateSpec) -> Result<UpdateOutcome, UpdateError> {
    let plan = resolve_plan(spec)?;
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

    // Permission check before any download.
    // Reuse the resolved exe path for the elevated-command hint instead of a
    // second `current_exe()` syscall; the canonical path is valid for rerun.
    let exe_path = stage::current_exe_path()?;
    let original_exe = exe_path.clone();
    stage::check_write_permission(&exe_path, &original_exe)?;

    let (from_cargo, staged) = prepare_candidate(spec, &current, &latest, target.as_deref())?;
    stage::replace_current_exe(staged.path(), &spec.program_name)?;
    if from_cargo {
        eprintln!(
            "Updated {} {current} -> {latest} via Cargo",
            spec.program_name
        );
        Ok(UpdateOutcome::UpdatedFromCargo {
            from: current,
            to: latest,
        })
    } else {
        eprintln!(
            "Updated {} {current} -> {latest} via GitHub binary",
            spec.program_name
        );
        Ok(UpdateOutcome::UpdatedBinary {
            from: current,
            to: latest,
        })
    }
}

/// Permission-probe helper shared by daemon coordination: resolve the
/// executable paths and verify writability before any download.
pub fn preflight_exe_writable(_program: &str) -> Result<(PathBuf, PathBuf), UpdateError> {
    let exe_path = stage::current_exe_path()?;
    // Same single-syscall reuse as `run_simple_update`; canonical path is a
    // valid rerun hint.
    let original_exe = exe_path.clone();
    stage::check_write_permission(&exe_path, &original_exe)?;
    Ok((exe_path, original_exe))
}
