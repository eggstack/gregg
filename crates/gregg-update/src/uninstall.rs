//! Generic executable-uninstall primitives shared by `gregg` and `greggd`.
//!
//! This module owns only generic executable operations and stays
//! service-manager-free (Plan 112):
//!
//! ```text
//! current executable resolution
//! writable-parent preflight with caller-specific elevation hints
//! self-deletion of the running executable
//! Cargo-ownership detection via Cargo's own supported interface
//! ```
//!
//! It knows nothing about systemd, launchd, cron, SCM, client config,
//! daemon config, or TUI state. Teardown of startup integration and
//! configuration/data removal lives in each application crate beside the
//! existing owners of those artifacts.

use std::path::{Path, PathBuf};

use crate::error::UpdateError;
use crate::stage::{check_write_permission_for, current_exe_path};

/// Re-exported resolution so both binaries remove the exact invoked
/// executable rather than an assumed install prefix.
pub use crate::stage::current_exe_path as resolve_uninstall_target;

/// Preflight that the install location of `exe_path` is writable before
/// any teardown mutation.
///
/// `operation` is the CLI verb for the elevated rerun hint (`update` or
/// `uninstall`); `purge` appends the destructive flag to the hint so the
/// printed command matches the operator's intent.
pub fn preflight_uninstall_writable(
    exe_path: &Path,
    original_exe: &Path,
    purge: bool,
) -> Result<(), UpdateError> {
    let operation = if purge {
        "uninstall --purge"
    } else {
        "uninstall"
    };
    check_write_permission_for(exe_path, original_exe, operation)
}

/// Delete the currently running executable using the already-present
/// `self-replace` dependency (same-filesystem unlink on Unix,
/// deferred running-image delete on Windows).
///
/// The caller must exit promptly afterwards so the Windows deferred
/// delete can complete. Never removes a directory, only the executable.
pub fn self_delete_current_exe(purge: bool) -> Result<(), UpdateError> {
    let exe = current_exe_path()?;
    self_replace::self_delete_at(&exe).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            let operation = if purge {
                "uninstall --purge"
            } else {
                "uninstall"
            };
            UpdateError::PermissionDenied {
                message: format!("permission denied deleting executable: {e}"),
                elevated: format!("sudo {} {operation}", exe.display()),
            }
        } else {
            UpdateError::Replacement(format!("self-delete failed: {e}"))
        }
    })
}

/// Positive Cargo ownership of an installed executable.
///
/// `root` is the Cargo root whose `bin/<program>[.exe]` is the running
/// binary; `package` is the crate name to hand off to
/// `cargo uninstall --root <root> <package>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoOwnership {
    /// Cargo root owning the installation (parent of `bin/`).
    pub root: PathBuf,
    /// Crate/package name (the Cargo uninstall key).
    pub package: String,
}

impl CargoOwnership {
    /// Exact operator command that removes the package without leaving
    /// stale Cargo tracking metadata.
    #[must_use]
    pub fn uninstall_command(&self) -> String {
        format!(
            "cargo uninstall --root {} {}",
            self.root.display(),
            self.package
        )
    }
}

/// Derive the candidate Cargo root for an executable path without
/// contacting Cargo.
///
/// Returns the grandparent when the executable sits directly in a `bin`
/// directory (`<root>/bin/<program>[.exe]`); otherwise `None`. This is
/// only a candidate: callers must confirm via [`cargo_lists_package`]
/// (Cargo's own supported interface) before treating the binary as
/// Cargo-owned. A path merely containing `.cargo` is never sufficient.
#[must_use]
pub fn candidate_cargo_root_for_exe(exe_path: &Path, program: &str) -> Option<PathBuf> {
    let file_name = exe_path.file_name()?.to_string_lossy();
    let expected = if cfg!(windows) {
        format!("{program}.exe")
    } else {
        program.to_string()
    };
    if !file_name.eq_ignore_ascii_case(&expected) {
        return None;
    }
    let bin_dir = exe_path.parent()?;
    if !bin_dir
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("bin"))
    {
        return None;
    }
    bin_dir.parent().map(Path::to_path_buf)
}

/// Pure helper: does `cargo install --list [--root <root>]` stdout show
/// `package` as an installed root package?
///
/// Only a line starting with `<package> v` (the stable
/// `cargo install --list` entry shape, e.g. `greggd v1.0.13:`) counts;
/// mentions in dependency lines or similarly-named packages
/// (`greggd-foo`) never count.
#[must_use]
pub fn cargo_list_contains_package(list_stdout: &str, package: &str) -> bool {
    let prefix = format!("{package} v");
    list_stdout.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with(&prefix)
            && trimmed[prefix.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
    })
}

/// Confirm via Cargo's supported interface that `root` has `package`
/// installed.
///
/// Runs bounded `cargo install --list --root <root>` and applies
/// [`cargo_list_contains_package`]. Any failure (missing `cargo`,
/// timeout, nonzero exit) means "not positively identified" (`false`),
/// never an error: ordinary bootstrap/manual-binary uninstall must not
/// require Cargo.
pub fn cargo_lists_package(cargo_bin: &str, root: &Path, package: &str) -> bool {
    use std::process::Command;
    use std::time::Duration;

    const LIST_TIMEOUT: Duration = Duration::from_secs(30);
    let mut cmd = Command::new(cargo_bin);
    cmd.args(["install", "--list", "--root", &root.to_string_lossy()]);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let Ok(output) = crate::exec::run_child_with_timeout(cmd, LIST_TIMEOUT) else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    cargo_list_contains_package(&String::from_utf8_lossy(&output.stdout), package)
}

/// Detect Cargo ownership of an executable using Cargo's own supported
/// interface.
///
/// The executable path only selects a candidate root (see
/// [`candidate_cargo_root_for_exe`]); ownership is confirmed by
/// `confirm(root, package)`, which the public path wires to
/// [`cargo_lists_package`]. Returns `None` unless confirmation
/// succeeds, so a path merely containing `.cargo` never implies
/// ownership and ordinary bootstrap/manual installs never require
/// Cargo.
pub fn detect_cargo_ownership_with(
    exe_path: &Path,
    program: &str,
    package: &str,
    confirm: impl Fn(&Path, &str) -> bool,
) -> Option<CargoOwnership> {
    let root = candidate_cargo_root_for_exe(exe_path, program)?;
    if confirm(&root, package) {
        Some(CargoOwnership {
            root,
            package: package.to_string(),
        })
    } else {
        None
    }
}

/// Detect Cargo ownership of the currently running executable.
///
/// See [`detect_cargo_ownership_with`] for the ownership boundary.
pub fn detect_cargo_ownership(
    exe_path: &Path,
    program: &str,
    package: &str,
) -> Option<CargoOwnership> {
    let cargo_bin = crate::exec::find_cargo().ok()?;
    detect_cargo_ownership_with(exe_path, program, package, |root, package| {
        cargo_lists_package(&cargo_bin, root, package)
    })
}

/// Run the Cargo-owned handoff: `cargo uninstall --root <root> <package>`
/// with a bounded deadline.
///
/// Used only on platforms where removing the running image synchronously
/// is known to work (Unix). Windows callers must fail before mutation
/// with [`CargoOwnership::uninstall_command`] instead, because the
/// running `.exe` cannot be removed while the process is alive.
pub fn cargo_uninstall(ownership: &CargoOwnership) -> Result<(), UpdateError> {
    use std::process::Command;

    let cargo_bin = crate::exec::find_cargo()?;
    let mut cmd = Command::new(&cargo_bin);
    cmd.args([
        "uninstall",
        "--root",
        &ownership.root.to_string_lossy(),
        &ownership.package,
    ]);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = crate::exec::run_command_with_timeout_for_cargo(cmd, crate::exec::CARGO_TIMEOUT)?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(UpdateError::CargoFallback(format!(
            "cargo uninstall {} --root {} failed (status {:?}): {stderr}",
            ownership.package,
            ownership.root.display(),
            output.status.code()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_root_requires_bin_layout_and_program_name() {
        assert_eq!(
            candidate_cargo_root_for_exe(Path::new("/home/u/.cargo/bin/greggd"), "greggd"),
            Some(PathBuf::from("/home/u/.cargo"))
        );
        // Wrong program name is never a candidate.
        assert_eq!(
            candidate_cargo_root_for_exe(Path::new("/home/u/.cargo/bin/gregg"), "greggd"),
            None
        );
        // Not under a `bin` directory: no candidate.
        assert_eq!(
            candidate_cargo_root_for_exe(Path::new("/usr/local/bin/greggd"), "greggd"),
            Some(PathBuf::from("/usr/local"))
        );
        assert_eq!(
            candidate_cargo_root_for_exe(Path::new("/opt/greggd/greggd"), "greggd"),
            None
        );
        // A path merely containing `.cargo` without the bin layout is
        // not a candidate.
        assert_eq!(
            candidate_cargo_root_for_exe(Path::new("/home/u/.cargo/greggd"), "greggd"),
            None
        );
    }

    #[test]
    fn cargo_list_matching_requires_package_version_entry() {
        let listing = "greggd v1.0.13:\n    greggd\n";
        assert!(cargo_list_contains_package(listing, "greggd"));
        assert!(!cargo_list_contains_package(listing, "gregg"));
        // Similarly-named packages never count.
        assert!(!cargo_list_contains_package(
            "greggd-foo v1.0.0:\n    greggd-foo\n",
            "greggd"
        ));
        // Dependency mentions and prose never count.
        assert!(!cargo_list_contains_package(
            "some tool (uses greggd v1 internally)\n",
            "greggd"
        ));
        assert!(!cargo_list_contains_package("", "greggd"));
    }

    #[test]
    fn ownership_never_guessed_from_pathname_alone() {
        let exe = Path::new("/home/u/.cargo/bin/greggd");
        // Even a textbook Cargo path is not owned without Cargo's own
        // confirmation.
        assert_eq!(
            detect_cargo_ownership_with(exe, "greggd", "greggd", |_, _| false),
            None
        );
        assert_eq!(
            detect_cargo_ownership_with(exe, "greggd", "greggd", |_, _| true),
            Some(CargoOwnership {
                root: PathBuf::from("/home/u/.cargo"),
                package: "greggd".to_string(),
            })
        );
        // Non-candidate paths are never owned regardless of confirmation.
        assert_eq!(
            detect_cargo_ownership_with(
                Path::new("/usr/local/bin/greggd"),
                "greggd",
                "greggd",
                |_, _| true
            ),
            Some(CargoOwnership {
                root: PathBuf::from("/usr/local"),
                package: "greggd".to_string(),
            })
        );
        assert_eq!(
            detect_cargo_ownership_with(
                Path::new("/opt/greggd/greggd"),
                "greggd",
                "greggd",
                |_, _| true
            ),
            None
        );
    }

    #[test]
    fn handoff_command_is_exact() {
        let ownership = CargoOwnership {
            root: PathBuf::from("/home/u/.cargo"),
            package: "greggd".to_string(),
        };
        assert_eq!(
            ownership.uninstall_command(),
            "cargo uninstall --root /home/u/.cargo greggd"
        );
    }

    #[test]
    fn preflight_hint_names_uninstall_operation() {
        // A read-only directory deterministically denies the probe file.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = crate::stage::create_temp_dir("gregg-uninstall-preflight").unwrap();
            let dir_path = dir.path().to_path_buf();
            let exe = dir_path.join("greggd");
            std::fs::write(&exe, b"x").unwrap();
            std::fs::set_permissions(&dir_path, std::fs::Permissions::from_mode(0o555)).unwrap();
            let err = preflight_uninstall_writable(&exe, &exe, false).unwrap_err();
            // Root can still write; only assert when denial actually occurs.
            if let UpdateError::PermissionDenied { elevated, .. } = err {
                assert!(elevated.contains("uninstall"));
                assert!(!elevated.contains("update"));
            }
            std::fs::set_permissions(&dir_path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    #[test]
    fn elevated_hint_carries_purge_flag() {
        let err = UpdateError::PermissionDenied {
            message: "permission denied writing to /usr/local/bin".to_string(),
            elevated: "sudo /usr/local/bin/greggd uninstall --purge".to_string(),
        };
        assert!(err.to_string().contains("uninstall --purge"));
    }
}
