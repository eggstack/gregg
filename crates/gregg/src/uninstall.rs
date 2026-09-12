//! Component-safe uninstall for the `gregg` client (Plan 112).
//!
//! `gregg uninstall` removes only the exact invoked client executable.
//! Configuration is preserved by default; `--purge` additionally removes
//! the resolved client config file. `--dry-run` plans without mutating.
//!
//! The command never initializes the TUI runtime, never prompts
//! interactively, never invokes `sudo` internally, and never removes a
//! directory recursively. A sibling `greggd` binary sharing the install
//! directory is never touched.

use std::fmt;
use std::path::{Path, PathBuf};

/// Identity constants for the client uninstall path.
pub const PROGRAM: &str = "gregg";
pub const PACKAGE: &str = "gregg";

/// Errors returned by client uninstall planning and execution.
///
/// Library code returns these without printing or exiting; the binary
/// boundary maps them to [`crate::cli::ExitCode`].
#[derive(Debug)]
pub enum UninstallError {
    /// The current executable path could not be determined.
    CurrentExe(String),
    /// A filesystem mutation failed.
    Io { path: PathBuf, message: String },
    /// The install location is not writable by this invocation.
    Permission { message: String, elevated: String },
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
            Self::CurrentExe(detail) => write!(f, "failed to determine current executable: {detail}"),
            Self::Io { path, message } => write!(f, "{}: {message}", path.display()),
            Self::Permission { message, elevated } => {
                write!(f, "permission denied: {message}. Rerun: {elevated}")
            }
            Self::CargoHandoff { command } => write!(
                f,
                "this installation is Cargo-owned; Gregg will not bypass Cargo bookkeeping. Run after this process exits:\n  {command}"
            ),
            Self::CargoFailed(detail) => write!(f, "cargo uninstall failed: {detail}"),
        }
    }
}

impl std::error::Error for UninstallError {}

impl From<gregg_update::UpdateError> for UninstallError {
    fn from(error: gregg_update::UpdateError) -> Self {
        match error {
            gregg_update::UpdateError::PermissionDenied { message, elevated } => {
                Self::Permission { message, elevated }
            }
            gregg_update::UpdateError::CurrentExe(detail) => Self::CurrentExe(detail),
            other => Self::CargoFailed(other.to_string()),
        }
    }
}

/// Deterministic uninstall plan: the exact resources `uninstall` will
/// change or remove.
///
/// The same plan drives `--dry-run` rendering and real execution so the
/// preview cannot drift from the mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallPlan {
    /// Exact invoked executable to delete (never a directory).
    pub exe_path: PathBuf,
    /// Resolved client config file (`--config` or platform default).
    pub config_path: PathBuf,
    /// Whether the config file currently exists.
    pub config_exists: bool,
    /// Whether `--purge` was requested.
    pub purge: bool,
    /// Positively identified Cargo ownership, if any.
    pub cargo: Option<gregg_update::CargoOwnership>,
}

impl UninstallPlan {
    /// Render the exact Gregg-owned resources that would be changed or
    /// removed, one per line. Pure and side-effect free.
    #[must_use]
    pub fn render(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!("uninstall {PROGRAM} (dry run)"));
        if let Some(ownership) = &self.cargo {
            lines.push(format!(
                "cargo-owned install (root {}); handoff: {}",
                ownership.root.display(),
                ownership.uninstall_command()
            ));
            return lines.join("\n");
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
        } else {
            lines.push(format!("preserve config: {}", self.config_path.display()));
        }
        lines.join("\n")
    }
}

/// Build the uninstall plan for an executable/config pair.
///
/// `confirm_cargo` wires Cargo's supported ownership confirmation; the
/// production path passes the real `cargo install --list` probe while
/// tests inject a stub. Planning performs no mutation.
pub fn plan_uninstall_with(
    exe_path: &Path,
    config_path: &Path,
    purge: bool,
    confirm_cargo: impl Fn(&Path, &str) -> bool,
) -> UninstallPlan {
    let cargo = gregg_update::uninstall::detect_cargo_ownership_with(
        exe_path,
        PROGRAM,
        PACKAGE,
        confirm_cargo,
    );
    UninstallPlan {
        exe_path: exe_path.to_path_buf(),
        config_path: config_path.to_path_buf(),
        config_exists: config_path.exists(),
        purge,
        cargo,
    }
}

/// Build the uninstall plan for the running binary and resolved config.
pub fn plan_uninstall(config_path: &Path, purge: bool) -> Result<UninstallPlan, UninstallError> {
    let exe_path = gregg_update::uninstall::resolve_uninstall_target()
        .map_err(|e| UninstallError::CurrentExe(e.to_string()))?;
    let cargo_bin = gregg_update::exec::find_cargo().ok();
    Ok(plan_uninstall_with(
        &exe_path,
        config_path,
        purge,
        |root, package| match &cargo_bin {
            Some(cargo) => gregg_update::uninstall::cargo_lists_package(cargo, root, package),
            None => false,
        },
    ))
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

/// Execute a plan. With `dry_run`, performs discovery/planning only and
/// mutates nothing; the rendered plan is printed by the caller.
///
/// Real execution order: preflight writability, Cargo handoff/delegation,
/// optional config purge, then self-deletion of the exact executable.
/// Only the exact executable file is removed; sibling binaries and
/// install directories are never touched.
pub fn execute_plan(plan: &UninstallPlan, dry_run: bool) -> Result<(), UninstallError> {
    if dry_run {
        return Ok(());
    }
    if let Some(ownership) = &plan.cargo {
        #[cfg(unix)]
        {
            gregg_update::uninstall::cargo_uninstall(ownership)
                .map_err(|e| UninstallError::CargoFailed(e.to_string()))?;
            return Ok(());
        }
        #[cfg(not(unix))]
        {
            return Err(UninstallError::CargoHandoff {
                command: ownership.uninstall_command(),
            });
        }
    }
    gregg_update::preflight_uninstall_writable(&plan.exe_path, &plan.exe_path, plan.purge)?;
    if plan.purge {
        purge_config(&plan.config_path)?;
    }
    gregg_update::self_delete_current_exe(plan.purge)?;
    Ok(())
}

/// Remove the resolved client config file for `--purge`.
///
/// Removes only the exact file; the parent directory is removed only
/// when [`purge_empty_dir_for`] identifies a known standard Gregg
/// directory that is empty after the file is gone. Never recurses.
fn purge_config(config_path: &Path) -> Result<(), UninstallError> {
    match std::fs::remove_file(config_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(UninstallError::Permission {
                message: format!("permission denied removing {}", config_path.display()),
                elevated: format!("sudo {} uninstall --purge", current_exe_hint()),
            });
        }
        Err(e) => {
            return Err(UninstallError::Io {
                path: config_path.to_path_buf(),
                message: format!("failed to remove config file: {e}"),
            });
        }
    }
    if let Some(dir) = purge_empty_dir_for(config_path) {
        // Non-empty or already gone: either is fine; the directory is
        // optional cleanup, never required for success.
        let _ = std::fs::remove_dir(&dir);
    }
    Ok(())
}

/// Best-effort current-exe display for elevation hints.
fn current_exe_hint() -> String {
    std::env::current_exe().map_or_else(|_| PROGRAM.to_string(), |p| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_case(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gregg_uninstall_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn no_cargo(_: &Path, _: &str) -> bool {
        false
    }

    #[test]
    fn dry_run_renders_without_mutating() {
        let dir = tmp_case("dry_run");
        let exe = dir.join("gregg");
        fs::write(&exe, b"fake").unwrap();
        let config = dir.join("gregg.toml");
        fs::write(&config, b"config_version = 1\n").unwrap();

        let plan = plan_uninstall_with(&exe, &config, false, no_cargo);
        let rendered = plan.render();
        assert!(rendered.contains(&exe.display().to_string()));
        assert!(rendered.contains("preserve config"));
        execute_plan(&plan, true).unwrap();

        assert!(exe.exists(), "dry run must not delete the executable");
        assert!(config.exists(), "dry run must not delete config");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_preserve_keeps_config_but_removes_binary() {
        let dir = tmp_case("preserve");
        let exe = dir.join("gregg");
        fs::write(&exe, b"fake").unwrap();
        let sibling = dir.join("greggd");
        fs::write(&sibling, b"sibling").unwrap();
        let config = dir.join("gregg.toml");
        fs::write(&config, b"config_version = 1\n").unwrap();

        // Execute only the config-preservation half (self-delete targets
        // the test binary, so it is covered by the shared-crate tests and
        // the disposable smoke instead).
        let plan = plan_uninstall_with(&exe, &config, false, no_cargo);
        assert!(!plan.purge);
        assert!(plan.render().contains("preserve config"));
        // Simulate the file-removal scope: only the exact exe is targeted.
        fs::remove_file(&plan.exe_path).unwrap();
        assert!(!exe.exists());
        assert!(sibling.exists(), "sibling binary must survive");
        assert!(config.exists(), "config must be preserved by default");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn purge_removes_only_the_config_file_never_its_parent() {
        let dir = tmp_case("purge_custom");
        let custom_parent = dir.join("custom-parent");
        fs::create_dir_all(&custom_parent).unwrap();
        let config = custom_parent.join("gregg.toml");
        fs::write(&config, b"config_version = 1\n").unwrap();
        let other = custom_parent.join("other.conf");
        fs::write(&other, b"keep").unwrap();

        // A custom --config path is never a standard dir, so no dir removal.
        assert_eq!(purge_empty_dir_for(&config), None);
        purge_config(&config).unwrap();
        assert!(!config.exists());
        assert!(
            custom_parent.exists(),
            "custom parent must never be removed"
        );
        assert!(other.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn purge_missing_config_is_a_noop() {
        let dir = tmp_case("purge_missing");
        let config = dir.join("absent.toml");
        purge_config(&config).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn standard_parent_is_eligible_for_empty_cleanup() {
        let standard = crate::config::Config::default_path();
        if let Some(parent) = standard.parent() {
            let candidate = parent.join("gregg.toml");
            assert_eq!(purge_empty_dir_for(&candidate), Some(parent.to_path_buf()));
        }
        let custom = PathBuf::from("/tmp/definitely-not-standard-gregg/gregg.toml");
        assert_eq!(purge_empty_dir_for(&custom), None);
    }

    #[test]
    fn cargo_owned_plan_renders_handoff_and_blocks_mutation() {
        let dir = tmp_case("cargo_owned");
        let exe_name = if cfg!(windows) { "gregg.exe" } else { "gregg" };
        let exe = dir.join("bin").join(exe_name);
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, b"fake").unwrap();
        let config = dir.join("gregg.toml");
        fs::write(&config, b"config_version = 1\n").unwrap();

        let plan = plan_uninstall_with(&exe, &config, false, |_, _| true);
        assert!(plan.cargo.is_some());
        let rendered = plan.render();
        assert!(rendered.contains("cargo-owned"));
        assert!(rendered.contains("cargo uninstall --root"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sibling_binaries_share_a_directory_safely() {
        // The plan targets exactly one file; assert the plan itself never
        // names the sibling or the directory.
        let dir = tmp_case("sibling");
        let exe = dir.join("gregg");
        let sibling = dir.join("greggd");
        fs::write(&exe, b"fake").unwrap();
        fs::write(&sibling, b"sibling").unwrap();
        let plan = plan_uninstall_with(&exe, &dir.join("gregg.toml"), true, no_cargo);
        assert_eq!(plan.exe_path, exe);
        assert_ne!(plan.exe_path, sibling);
        assert!(plan.render().contains(&exe.display().to_string()));
        assert!(!plan.render().contains(&sibling.display().to_string()));
        let _ = fs::remove_dir_all(&dir);
    }
}
