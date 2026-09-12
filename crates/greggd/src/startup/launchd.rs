//! launchd plist content, installation, and restart.

use super::install::{
    elevated_command, ensure_config_preserved, is_privileged, manager_error_is_permission,
    repair_system_config_permissions, write_atomic_text, InstallError,
};
use super::method::{
    launchd_label, standard_launchd_binary, standard_launchd_config, standard_launchd_plist_path,
    StartupMethodArg,
};
use super::process::{run_bounded_command, MANAGER_COMMAND_TIMEOUT};
use super::ArtifactOwnership;
use std::fs;
use std::io::{self};
use std::path::{Path, PathBuf};

pub fn launchd_plist_content() -> String {
    const TEMPLATE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.eggstack.greggd</string>

    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/greggd</string>
        <string>run</string>
        <string>--config</string>
        <string>/Library/Application Support/gregg/greggd.toml</string>
    </array>

    <key>RunAtLoad</key>
    <true/>

    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
        <key>Crashed</key>
        <true/>
    </dict>

    <key>ThrottleInterval</key>
    <integer>10</integer>

    <key>StandardOutPath</key>
    <string>/var/log/greggd.log</string>

    <key>StandardErrorPath</key>
    <string>/var/log/greggd.log</string>

    <key>HardResourceLimits</key>
    <dict>
        <key>NumberOfFiles</key>
        <integer>1024</integer>
    </dict>

    <key>SoftResourceLimits</key>
    <dict>
        <key>NumberOfFiles</key>
        <integer>1024</integer>
    </dict>
</dict>
</plist>
"#;
    TEMPLATE.to_string()
}
#[allow(dead_code)]
pub(crate) fn launchd_plist_exists() -> bool {
    standard_launchd_plist_path().exists()
}
#[allow(dead_code)]
pub(crate) fn launchd_is_loaded() -> bool {
    // `launchctl print system/com.eggstack.greggd` exits 0 when loaded on
    // modern macOS; fall back to `launchctl list | grep`.
    if matches!(
        run_bounded_command(
            "launchctl",
            &["print", &format!("system/{}", launchd_label())],
            MANAGER_COMMAND_TIMEOUT,
        ),
        Ok(output) if output.status.success()
    ) {
        return true;
    }
    // Fallback: `launchctl list` contains label
    if let Ok(output) = run_bounded_command("launchctl", &["list"], MANAGER_COMMAND_TIMEOUT) {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains(launchd_label()) {
                return true;
            }
        }
    }
    false
}

/// Parse only the canonical `ProgramArguments` string list emitted by Gregg.
/// Missing or malformed arguments are intentionally treated as ambiguous.
pub fn parse_program_arguments(text: &str) -> Option<(PathBuf, Option<PathBuf>)> {
    let section = text
        .split_once("<key>ProgramArguments</key>")?
        .1
        .split_once("</array>")?
        .0;
    let args: Vec<PathBuf> = section
        .split("<string>")
        .skip(1)
        .filter_map(|part| {
            part.split_once("</string>")
                .map(|(value, _)| value.to_string())
        })
        .map(PathBuf::from)
        .collect();
    let target = args.first()?.clone();
    if args.is_empty() || target.as_os_str().is_empty() {
        return None;
    }
    let config = args
        .windows(2)
        .find(|pair| pair[0].as_os_str() == "--config")
        .map(|pair| pair[1].clone());
    Some((target, config))
}

/// Classify the canonical launchd plist relative to `exe`. A loaded job
/// without a readable plist is unknown, so it is never removed by guessing.
pub fn launchd_artifact_ownership(exe: &Path) -> (ArtifactOwnership, bool, Option<PathBuf>) {
    let plist_path = standard_launchd_plist_path();
    let plist_exists = plist_path.exists();
    let loaded = launchd_is_loaded();
    let Some(content) = plist_exists
        .then(|| fs::read_to_string(&plist_path).ok())
        .flatten()
    else {
        return (
            if plist_exists || loaded {
                ArtifactOwnership::Unknown
            } else {
                ArtifactOwnership::Absent
            },
            loaded,
            None,
        );
    };
    let Some((target, config)) = parse_program_arguments(&content) else {
        return (ArtifactOwnership::Unknown, loaded, None);
    };
    let ownership = if gregg_update::uninstall::paths_equivalent(&target, exe) {
        ArtifactOwnership::Owned
    } else {
        ArtifactOwnership::Foreign
    };
    (ownership, loaded, config)
}

/// Detect the current startup state for restart/update dispatch.
///
/// This is the small helper required by Plan 100 §6 "Manager detection".
/// It answers enough for both `restart` and Plan 101 `update` without building
/// a generalized discovery database.
// ── Launchd installation ──────────────────────────────────────────────────
#[allow(clippy::too_many_lines)]
pub fn install_launchd(exe: &Path, _config_path: &Path) -> Result<(), InstallError> {
    // Verify macOS
    if std::env::consts::OS != "macos" && std::env::consts::OS != "darwin" {
        return Err(InstallError::LaunchdNotDetected {
            message: "launchd is only available on macOS".into(),
        });
    }
    let bin_path = standard_launchd_binary();
    if !bin_path.exists() {
        return Err(InstallError::BinaryMissing { path: bin_path });
    }
    if !is_privileged() {
        let cmd = elevated_command(exe, StartupMethodArg::Launchd);
        return Err(InstallError::Permission {
            message: format!("permission denied: rerun as root: {cmd}"),
        });
    }
    // Ensure config dir and default config
    let cfg_path = standard_launchd_config();
    if let Some(parent) = cfg_path.parent() {
        fs::create_dir_all(parent).map_err(|e| InstallError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    ensure_config_preserved(&cfg_path).map_err(|e| InstallError::Io {
        path: cfg_path.clone(),
        source: e,
    })?;
    // Older installs left the system config 0600, which breaks
    // unprivileged `croncheck`/`status`/`configprint` with EACCES.
    repair_system_config_permissions(&cfg_path).map_err(|e| InstallError::Io {
        path: cfg_path.clone(),
        source: e,
    })?;
    // Write plist atomically
    let plist_content = launchd_plist_content();
    let plist_path = standard_launchd_plist_path();
    if let Some(parent) = plist_path.parent() {
        fs::create_dir_all(parent).map_err(|e| InstallError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    write_atomic_text(&plist_path, &plist_content).map_err(|e| {
        if e.kind() == io::ErrorKind::PermissionDenied {
            InstallError::Permission {
                message: format!(
                    "permission denied writing {}: rerun as root: {}",
                    plist_path.display(),
                    elevated_command(exe, StartupMethodArg::Launchd)
                ),
            }
        } else {
            InstallError::Io {
                path: plist_path.clone(),
                source: e,
            }
        }
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&plist_path, fs::Permissions::from_mode(0o644));
    }
    // Create log file
    let log_path = Path::new("/var/log/greggd.log");
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(log_path, fs::Permissions::from_mode(0o644));
    }
    // Manage launchd job: if already loaded, bootout then bootstrap; else bootstrap.
    let label = launchd_label();
    let loaded = launchd_is_loaded();
    if loaded {
        // Bootout existing job (best effort), but never allow a manager call
        // to hang indefinitely.
        let _ = run_bounded_command(
            "launchctl",
            &["bootout", &format!("system/{label}")],
            MANAGER_COMMAND_TIMEOUT,
        );
        let _ = run_bounded_command(
            "launchctl",
            &["bootout", "system", &plist_path.to_string_lossy()],
            MANAGER_COMMAND_TIMEOUT,
        );
    }
    // Bootstrap
    let bootstrap_args = ["bootstrap", "system", &plist_path.to_string_lossy()];
    let bootstrap = run_bounded_command("launchctl", &bootstrap_args, MANAGER_COMMAND_TIMEOUT)
        .map_err(|source| InstallError::Io {
            path: PathBuf::from("launchctl bootstrap"),
            source,
        })?;
    if !bootstrap.status.success() && !loaded {
        let detail = String::from_utf8_lossy(&bootstrap.stderr)
            .trim()
            .to_string();
        if manager_error_is_permission(&detail) {
            return Err(InstallError::Permission {
                message: format!(
                    "launchctl bootstrap failed: {detail}; rerun as root: {}",
                    elevated_command(exe, StartupMethodArg::Launchd)
                ),
            });
        }
        return Err(InstallError::Other(format!(
            "launchctl bootstrap failed with status {:?}: {detail}",
            bootstrap.status.code()
        )));
    }
    // Kickstart if it was previously loaded (restart), otherwise bootstrap already started it (RunAtLoad).
    if loaded {
        let kick = run_bounded_command(
            "launchctl",
            &["kickstart", "-k", &format!("system/{label}")],
            MANAGER_COMMAND_TIMEOUT,
        )
        .map_err(|source| InstallError::Io {
            path: PathBuf::from("launchctl kickstart"),
            source,
        })?;
        if !kick.status.success() {
            let detail = String::from_utf8_lossy(&kick.stderr).trim().to_string();
            if manager_error_is_permission(&detail) {
                return Err(InstallError::Permission {
                    message: format!(
                        "launchctl kickstart failed: {detail}; rerun as root: {}",
                        elevated_command(exe, StartupMethodArg::Launchd)
                    ),
                });
            }
            return Err(InstallError::Other(format!(
                "launchctl kickstart failed with status {:?}: {detail}",
                kick.status.code()
            )));
        }
    }
    println!("greggd launchd service installed: {}", plist_path.display());
    println!("config: {}", cfg_path.display());
    println!("logs: log show --predicate 'process == \"greggd\"' --last 5m");
    Ok(())
}
pub(crate) fn restart_launchd(exe: &Path) -> Result<(), InstallError> {
    let label = launchd_label();
    let args = ["kickstart", "-k", &format!("system/{label}")];
    match run_bounded_command("launchctl", &args, MANAGER_COMMAND_TIMEOUT) {
        Ok(output) if output.status.success() => {
            println!("greggd restarted via launchd (kickstart -k system/{label})");
            Ok(())
        }
        Ok(output) if manager_error_is_permission(&String::from_utf8_lossy(&output.stderr)) => {
            Err(InstallError::Permission {
                message: format!(
                    "permission denied: rerun as root: sudo launchctl kickstart -k system/{label} (exe: {})",
                    exe.display()
                ),
            })
        }
        Ok(output) => Err(InstallError::Io {
            path: PathBuf::from(format!("launchctl kickstart -k system/{label}")),
            source: io::Error::other(format!(
                "launchctl kickstart failed with status {:?}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        }),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(InstallError::Permission {
            message: format!(
                "permission denied: rerun as root: sudo launchctl kickstart -k system/{label} (exe: {})",
                exe.display()
            ),
        }),
        Err(e) => Err(InstallError::Io {
            path: PathBuf::from(format!("launchctl kickstart -k system/{label}")),
            source: e,
        }),
    }
}
// ── launchd uninstall (Plan 112) ────────────────────────────────────────────

/// One teardown step of the canonical Gregg launchd uninstall, in
/// execution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchdUninstallStep {
    /// `launchctl bootout system/<label>` when the job is loaded.
    Bootout,
    /// Remove the canonical plist file.
    RemovePlist,
}

/// Pure helper: ordered launchd teardown steps for injected discovery.
///
/// `plist_exists` mirrors the canonical plist path; `loaded` mirrors the
/// Gregg launchd label state. Missing/unloaded state yields no steps
/// (idempotent success, not an error). Only the canonical Gregg
/// label/plist is ever addressed.
#[must_use]
pub fn launchd_uninstall_steps(plist_exists: bool, loaded: bool) -> Vec<LaunchdUninstallStep> {
    let mut steps = Vec::with_capacity(2);
    if loaded {
        steps.push(LaunchdUninstallStep::Bootout);
    }
    if plist_exists {
        steps.push(LaunchdUninstallStep::RemovePlist);
    }
    steps
}

/// Remove Gregg launchd integration: boot out the Gregg label when
/// loaded, then remove only the canonical Gregg plist.
///
/// Missing/unloaded state is idempotent success. Daemon configuration
/// and logs are preserved here; `--purge` removal of those files is
/// owned by the uninstall command, not this primitive.
pub fn uninstall_launchd(exe: &Path) -> Result<(), InstallError> {
    let plist_path = standard_launchd_plist_path();
    let plist_exists = plist_path.exists();
    let (ownership, loaded, _) = launchd_artifact_ownership(exe);
    if !ownership.is_owned() {
        return Ok(());
    }
    let steps = launchd_uninstall_steps(plist_exists, loaded);
    if steps.is_empty() {
        return Ok(());
    }
    for step in steps {
        match step {
            LaunchdUninstallStep::Bootout => {
                let label = launchd_label();
                let target = format!("system/{label}");
                let args = ["bootout", target.as_str()];
                let output = run_bounded_command("launchctl", &args, MANAGER_COMMAND_TIMEOUT)
                    .map_err(|source| InstallError::Io {
                        path: PathBuf::from("launchctl bootout"),
                        source,
                    })?;
                if !output.status.success() {
                    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    if manager_error_is_permission(&detail) {
                        return Err(InstallError::Permission {
                            message: format!(
                                "launchctl bootout failed: {detail}; rerun as root: sudo {} uninstall",
                                exe.display()
                            ),
                        });
                    }
                    return Err(InstallError::Other(format!(
                        "launchctl bootout failed with status {:?}: {detail}",
                        output.status.code()
                    )));
                }
            }
            LaunchdUninstallStep::RemovePlist => match fs::remove_file(&plist_path) {
                Ok(()) => {}
                // Raced with another uninstall; removal is the goal.
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    return Err(InstallError::Permission {
                        message: format!(
                            "permission denied removing {}: rerun as root: sudo {} uninstall",
                            plist_path.display(),
                            exe.display()
                        ),
                    });
                }
                Err(e) => {
                    return Err(InstallError::Io {
                        path: plist_path.clone(),
                        source: e,
                    });
                }
            },
        }
    }
    println!("greggd launchd integration removed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launchd_plist_content_contains_label() {
        let content = launchd_plist_content();
        assert!(content.contains("com.eggstack.greggd"));
        assert!(content.contains("/usr/local/bin/greggd"));
        assert!(content.contains("KeepAlive"));
    }
    #[test]
    fn embedded_launchd_plist_matches_packaging_file_when_present() {
        let packaging = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/launchd/com.eggstack.greggd.plist");
        if let Ok(file) = std::fs::read_to_string(&packaging) {
            let mut file_norm = file.replace("\r\n", "\n");
            if !file_norm.ends_with('\n') {
                file_norm.push('\n');
            }
            assert_eq!(
                launchd_plist_content(),
                file_norm,
                "embedded launchd plist must stay synchronized with packaging/launchd/com.eggstack.greggd.plist"
            );
        }
    }

    #[test]
    fn launchd_uninstall_orders_bootout_before_remove() {
        use LaunchdUninstallStep::{Bootout, RemovePlist};
        assert_eq!(
            launchd_uninstall_steps(true, true),
            vec![Bootout, RemovePlist]
        );
    }

    #[test]
    fn launchd_uninstall_missing_state_is_idempotent_noop() {
        assert!(launchd_uninstall_steps(false, false).is_empty());
        // Unloaded but present still removes the stale plist; loaded but
        // missing still boots out the stale job.
        assert_eq!(
            launchd_uninstall_steps(true, false),
            vec![LaunchdUninstallStep::RemovePlist]
        );
        assert_eq!(
            launchd_uninstall_steps(false, true),
            vec![LaunchdUninstallStep::Bootout]
        );
    }

    #[test]
    fn parses_program_arguments_and_config() {
        let content = "<key>ProgramArguments</key><array><string>/usr/local/bin/greggd</string><string>run</string><string>--config</string><string>/Library/Application Support/gregg/greggd.toml</string></array>";
        let parsed = parse_program_arguments(content).unwrap();
        assert_eq!(parsed.0, PathBuf::from("/usr/local/bin/greggd"));
        assert_eq!(
            parsed.1,
            Some(PathBuf::from(
                "/Library/Application Support/gregg/greggd.toml"
            ))
        );
    }

    #[test]
    fn malformed_program_arguments_are_unknown() {
        assert!(parse_program_arguments("<key>ProgramArguments</key><array></array>").is_none());
    }
}
