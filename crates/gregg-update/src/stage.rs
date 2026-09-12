//! Private staging, current-executable resolution, permission probes,
//! and atomic replacement.
//!
//! Staging uses an exclusive owner-private `tempfile::TempDir` that is
//! removed on drop, so a failed prepared update never mutates the installed
//! executable. Replacement uses `self-replace` (same-filesystem atomic
//! rename on Unix, running-image-safe replacement on Windows).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tempfile::{Builder, TempDir};

use crate::error::UpdateError;

/// Create an exclusive owner-private temp dir for update staging.
/// On Unix the directory mode is `0o700`.
pub fn create_temp_dir(prefix: &str) -> Result<TempDir, UpdateError> {
    let temp_dir = Builder::new()
        .prefix(prefix)
        .tempdir()
        .map_err(|e| UpdateError::Io(format!("failed to create private temp dir: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temp_dir.path(), fs::Permissions::from_mode(0o700))
            .map_err(|e| UpdateError::Io(format!("failed to secure private temp dir: {e}")))?;
    }
    Ok(temp_dir)
}

/// A fully prepared, verified update candidate.
///
/// Holds the staging directory guard so the candidate is removed if the
/// caller drops it without replacing.
pub struct StagedCandidate {
    _temp_dir: TempDir,
    path: PathBuf,
}

impl StagedCandidate {
    /// Create a staged candidate from an already-prepared temp dir.
    #[must_use]
    pub fn new(temp_dir: TempDir, path: PathBuf) -> Self {
        Self {
            _temp_dir: temp_dir,
            path,
        }
    }

    /// Path of the verified candidate executable.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for StagedCandidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagedCandidate")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Resolve the current executable, following one level of symlink so
/// replacement targets the real installed file (mirroring `self-replace`
/// semantics).
pub fn current_exe_path() -> Result<PathBuf, UpdateError> {
    let exe = std::env::current_exe()
        .map_err(|e| UpdateError::CurrentExe(format!("current_exe failed: {e}")))?;
    // On Unix, current_exe may be via /proc/self/exe which is already canonical.
    // Try canonicalize for symlink handling; fallback to original if fails.
    if let Ok(canonical) = exe.canonicalize() {
        Ok(canonical)
    } else {
        // Fallback: check symlink one level like self-replace does.
        if fs::symlink_metadata(&exe).is_ok_and(|m| m.file_type().is_symlink()) {
            if let Ok(target) = fs::read_link(&exe) {
                if target.is_relative() {
                    if let Some(parent) = exe.parent() {
                        return Ok(parent.join(target));
                    }
                }
                return Ok(target);
            }
        }
        Ok(exe)
    }
}

/// Probe whether the install location is writable by attempting to create
/// an exclusive probe file in the executable's parent directory.
///
/// Must run before any download so permission failures fail fast with an
/// actionable elevated command instead of after minutes of fetching.
pub fn check_write_permission(exe_path: &Path, original_exe: &Path) -> Result<(), UpdateError> {
    let parent = exe_path.parent().ok_or_else(|| {
        UpdateError::Io(format!(
            "executable has no parent directory: {}",
            exe_path.display()
        ))
    })?;
    // Retry on `AlreadyExists`: two updates from the same PID within one
    // nanosecond collide on the probe name. A fresh timestamp per attempt
    // keeps a collision from masquerading as a permission failure.
    for _ in 0..3 {
        let probe = parent.join(format!(
            ".gregg-update-perm-{}-{}.tmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
        {
            Ok(_) => {
                let _ = fs::remove_file(&probe);
                return Ok(());
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                return Err(UpdateError::PermissionDenied {
                    message: format!("permission denied writing to {}", parent.display()),
                    elevated: format!("sudo {} update", original_exe.display()),
                });
            }
            Err(e) => {
                return Err(UpdateError::Io(format!(
                    "permission probe failed for {}: {e}",
                    parent.display()
                )));
            }
        }
    }
    Err(UpdateError::Io(format!(
        "permission probe collided for {}",
        parent.display()
    )))
}

/// Replace the current executable with a verified candidate.
///
/// On Unix this is a same-filesystem atomic rename where practical and
/// preserves symlink targets; on Windows it handles the running-image
/// semantics. Never overwrites a symlink file itself.
pub fn replace_current_exe(candidate: &Path, program: &str) -> Result<(), UpdateError> {
    self_replace::self_replace(candidate).map_err(|e| {
        if e.kind() == io::ErrorKind::PermissionDenied {
            UpdateError::PermissionDenied {
                message: format!("permission denied replacing executable: {e}"),
                elevated: format!(
                    "sudo {} update",
                    std::env::current_exe()
                        .map_or_else(|_| program.to_string(), |p| p.display().to_string())
                ),
            }
        } else {
            UpdateError::Replacement(format!("self-replace failed: {e}"))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_staging_is_exclusive_and_cleans_up() {
        let first = create_temp_dir("gregg-update-test-stage").unwrap();
        let second = create_temp_dir("gregg-update-test-stage").unwrap();
        assert_ne!(first.path(), second.path());
        assert!(first.path().is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(first.path()).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let path = first.path().join("candidate");
        fs::write(&path, b"candidate").unwrap();
        let first_path = first.path().to_path_buf();
        drop(first);
        assert!(!first_path.exists());
        drop(second);
    }

    #[test]
    fn permission_error_contains_elevated_command() {
        let err = UpdateError::PermissionDenied {
            message: "permission denied writing to /usr/local/bin".to_string(),
            elevated: "sudo /usr/local/bin/gregg update".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("sudo /usr/local/bin/gregg update"));
    }
}
