//! launchd plist content, installation, and restart.

use super::install::{
    elevated_command, ensure_config_preserved, is_privileged, manager_error_is_permission,
    repair_system_config_permissions, sudo_uninstall_hint, write_atomic_text, InstallError,
};
#[cfg(test)]
use super::method::standard_launchd_config;
use super::method::{
    launchd_label, standard_launchd_binary, standard_launchd_plist_path, StartupMethodArg,
};
use super::process::{run_bounded_command, MANAGER_COMMAND_TIMEOUT};
use super::ArtifactOwnership;
use std::fs;
use std::io::{self};
use std::path::{Path, PathBuf};

/// Canonical launchd plist. Keep synchronized with
/// `packaging/launchd/com.eggstack.greggd.plist`.
///
/// `config_path` is the resolved `--config` the daemon must load. It is
/// rendered into `ProgramArguments` so `greggd --config /custom.toml startup
/// install --method launchd` registers the job that actually reads that
/// file; the default installation passes the standard
/// `/Library/Application Support/gregg/greggd.toml` and produces the
/// packaged plist byte-for-byte.
///
/// The plist is rendered from [`LAUNCHD_PLIST_TEMPLATE`] below.
pub fn launchd_plist_content(config_path: &Path) -> String {
    LAUNCHD_PLIST_TEMPLATE.replace(
        "        <string>/Library/Application Support/gregg/greggd.toml</string>",
        &format!(
            "        <string>{}</string>",
            xml_escape(&config_path.to_string_lossy()),
        ),
    )
}

/// Minimal XML escape for plist string values (`&`, `<`, `>`, `"`, `'`).
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// Plist template with the canonical `ProgramArguments` config substituted at
/// install time.
const LAUNCHD_PLIST_TEMPLATE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.eggstack.greggd</string>

    <key>UserName</key>
    <string>_greggd</string>

    <key>GroupName</key>
    <string>_greggd</string>

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

// ── Service account ───────────────────────────────────────────────────────

/// Fixed service identity, matching what Linux does with its `greggd` user.
///
/// The ids are constants so a reinstall is idempotent and file ownership
/// survives an upgrade. They are never reused: a collision is a hard install
/// error that names the conflicting record, because silently adopting an
/// unrelated account would hand that account's other access to the daemon.
const GREGGD_SERVICE_USER: &str = "_greggd";
const GREGGD_SERVICE_GROUP: &str = "_greggd";
const GREGGD_SERVICE_UID: u32 = 391;
const GREGGD_SERVICE_GID: u32 = 391;
/// Where launchd redirects `StandardOutPath`/`StandardErrorPath`.
const GREGGD_LOG_PATH: &str = "/var/log/greggd.log";

/// Create the hidden `_greggd` account the `LaunchDaemon` runs as.
///
/// launchd runs a job as root when the plist names no user. Naming one gives
/// the daemon the same posture as the Linux unit: a compromise is `_greggd`,
/// not root. Every manager call is bounded and argv-only, so no shell sees a
/// value here.
fn ensure_greggd_user_macos() -> io::Result<()> {
    // Idempotent: an existing account is the post-install state, not an error.
    let exists = matches!(
        run_bounded_command(
            "dscl",
            &[".", "-read", &format!("/Users/{GREGGD_SERVICE_USER}")],
            MANAGER_COMMAND_TIMEOUT,
        ),
        Ok(output) if output.status.success()
    );
    if exists {
        return Ok(());
    }
    ensure_service_id_is_free()?;

    let group = format!("/Groups/{GREGGD_SERVICE_GROUP}");
    let user = format!("/Users/{GREGGD_SERVICE_USER}");
    let uid = GREGGD_SERVICE_UID.to_string();
    let gid = GREGGD_SERVICE_GID.to_string();
    let commands: [&[&str]; 10] = [
        &[".", "-create", group.as_str()],
        &[
            ".",
            "-create",
            group.as_str(),
            "PrimaryGroupID",
            gid.as_str(),
        ],
        &[".", "-create", user.as_str()],
        &[".", "-create", user.as_str(), "UniqueID", uid.as_str()],
        &[
            ".",
            "-create",
            user.as_str(),
            "PrimaryGroupID",
            gid.as_str(),
        ],
        // No home and no login shell: the account exists only to own the job.
        &[
            ".",
            "-create",
            user.as_str(),
            "NFSHomeDirectory",
            "/var/empty",
        ],
        &[".", "-create", user.as_str(), "UserShell", "/usr/bin/false"],
        &[".", "-create", user.as_str(), "RealName", "Gregg daemon"],
        &[".", "-create", user.as_str(), "IsHidden", "1"],
        &[".", "-create", user.as_str(), "Password", "*"],
    ];
    for args in commands {
        let output = run_bounded_command("dscl", args, MANAGER_COMMAND_TIMEOUT)?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "dscl {} failed with status {}: {}",
                args.join(" "),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
    }
    Ok(())
}

/// Fail the install when either fixed id is already taken.
fn ensure_service_id_is_free() -> io::Result<()> {
    for (record, attribute, id) in [
        ("/Users", "UniqueID", GREGGD_SERVICE_UID),
        ("/Groups", "PrimaryGroupID", GREGGD_SERVICE_GID),
    ] {
        let id = id.to_string();
        let output = run_bounded_command(
            "dscl",
            &[".", "-search", record, attribute, &id],
            MANAGER_COMMAND_TIMEOUT,
        )?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "dscl -search {record} {attribute} {id} failed with status {}",
                output.status
            )));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(conflict) = parse_service_id_conflict(&stdout) {
            return Err(io::Error::other(format!(
                "cannot create the greggd service account: {attribute} {id} already belongs to \
                 {conflict}; free that id and rerun the install"
            )));
        }
    }
    Ok(())
}

/// Extract a conflicting record from `dscl . -search` output.
///
/// The search is by exact attribute value, so *any* output means the id is
/// taken and no output means it is free. macOS prints the record first and the
/// matched value in a parenthesised block after it:
///
/// ```text
/// davidbowman     UniqueID = (
///     503
/// )
/// ```
///
/// A record named `_greggd` is this installer's own partial state (a group
/// created before a failed user creation), not a conflict, so creation
/// continues.
fn parse_service_id_conflict(stdout: &str) -> Option<String> {
    let first = stdout.lines().find(|line| !line.trim().is_empty())?;
    let record = first.split_whitespace().next()?;
    (record != GREGGD_SERVICE_USER).then(|| record.to_owned())
}

/// Give the config to root and the log to `_greggd`.
///
/// The daemon reads its configuration but must not own it: a root-owned file is
/// what stops a compromised daemon from editing its own job set. The log is the
/// opposite — launchd opens it as the job's user, so `_greggd` owns it and it
/// stays `0640`.
///
/// This step deliberately does **not** set the config's mode. That is
/// `repair_system_config_permissions`' job, and it is gated on the path being
/// the default config or parsing as one. `chown` and `set_permissions` both
/// follow symlinks, so repeating the link/type check here is what keeps a
/// symlinked `--config` from redirecting a root `chown` at its target.
#[cfg(unix)]
fn ensure_service_file_ownership(config_path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    // `ensure_config_preserved` creates the file as the installing (root) user,
    // so these chowns are idempotent for the normal path and corrective for a
    // pre-existing file written by an older install.
    if is_regular_non_symlink(config_path) {
        std::os::unix::fs::chown(config_path, Some(0), Some(0))?;
    }
    if let Some(parent) = config_path.parent() {
        if is_regular_directory(parent) {
            std::os::unix::fs::chown(parent, Some(0), Some(0))?;
        }
    }

    fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(GREGGD_LOG_PATH)?;
    std::os::unix::fs::chown(
        GREGGD_LOG_PATH,
        Some(GREGGD_SERVICE_UID),
        Some(GREGGD_SERVICE_GID),
    )?;
    fs::set_permissions(GREGGD_LOG_PATH, fs::Permissions::from_mode(0o640))?;
    Ok(())
}

/// Whether the path is a regular file, without following a final symlink.
#[cfg(unix)]
fn is_regular_non_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

/// Whether the path is a directory, without following a symlink.
#[cfg(unix)]
fn is_regular_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
}

#[cfg(not(unix))]
fn ensure_service_file_ownership(config_path: &Path) -> io::Result<()> {
    let _ = config_path;
    Ok(())
}

// ── Launchd installation ──────────────────────────────────────────────────
#[allow(clippy::too_many_lines)]
///
/// `config_path` is the configuration the installed job must load; it is
/// rendered into `ProgramArguments` so `restart`/`update`/`uninstall`
/// ownership checks see the same config the operator selected.
pub fn install_launchd(exe: &Path, config_path: &Path) -> Result<(), InstallError> {
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
    // ProgramArguments[0] names that same path verbatim. See the systemd
    // install path for why an install from a different binary must fail here
    // rather than register a service nobody owns.
    super::install::ensure_standard_binary(exe, &bin_path)?;
    if !is_privileged() {
        let cmd = elevated_command(exe, StartupMethodArg::Launchd);
        return Err(InstallError::Permission {
            message: format!("permission denied: rerun as root: {cmd}"),
        });
    }
    // The job runs as `_greggd` (the plist's `UserName`/`GroupName`), so the
    // account has to exist before launchd bootstraps it: a job naming a missing
    // user fails to load. Doing it before the plist write also means an account
    // failure leaves no registered service behind.
    ensure_greggd_user_macos().map_err(|e| InstallError::Io {
        path: PathBuf::from("dscl _greggd"),
        source: e,
    })?;
    // Ensure the selected config dir and config exist. Using `config_path`
    // (not the hardcoded standard path) is what makes an explicit
    // `--config /custom.toml` install register the file the daemon reads.
    ensure_config_preserved(config_path).map_err(|e| InstallError::Io {
        path: config_path.to_path_buf(),
        source: e,
    })?;
    // Older installs left the system config 0600, which breaks
    // unprivileged `croncheck`/`status`/`configprint` with EACCES.
    repair_system_config_permissions(config_path).map_err(|e| InstallError::Io {
        path: config_path.to_path_buf(),
        source: e,
    })?;
    // `_greggd` must be able to read the config and write the log, and must not
    // be able to rewrite either the config or its own job set.
    ensure_service_file_ownership(config_path).map_err(|e| InstallError::Io {
        path: config_path.to_path_buf(),
        source: e,
    })?;
    // Write plist atomically
    let plist_content = launchd_plist_content(config_path);
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
    println!("config: {}", config_path.display());
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
// ── launchd uninstall ──────────────────────────────────────────────────────

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
                                "launchctl bootout failed: {detail}; rerun as root: {}",
                                sudo_uninstall_hint(exe)
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
                            "permission denied removing {}: rerun as root: {}",
                            plist_path.display(),
                            sudo_uninstall_hint(exe)
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
        let content = launchd_plist_content(&standard_launchd_config());
        assert!(content.contains("com.eggstack.greggd"));
        assert!(content.contains("/usr/local/bin/greggd"));
        assert!(content.contains("KeepAlive"));
    }
    #[test]
    fn launchd_plist_renders_the_selected_config_path() {
        let content = launchd_plist_content(Path::new("/srv/gregg/custom.toml"));
        assert!(content.contains("<string>/srv/gregg/custom.toml</string>"));
        assert!(!content.contains("/Library/Application Support/gregg/greggd.toml"));
        assert!(launchd_plist_content(&standard_launchd_config())
            .contains("<string>/Library/Application Support/gregg/greggd.toml</string>"));
    }

    /// launchd runs a job as root when the plist names no user, so the rendered
    /// job must name the dedicated account. The rest of the job — program,
    /// config, and log destination — is unchanged.
    #[test]
    fn launchd_plist_runs_as_the_dedicated_service_user() {
        let content = launchd_plist_content(&standard_launchd_config());
        assert!(
            content.contains("<key>UserName</key>\n    <string>_greggd</string>"),
            "the job must name the service user: {content}"
        );
        assert!(
            content.contains("<key>GroupName</key>\n    <string>_greggd</string>"),
            "the job must name the service group: {content}"
        );
        assert!(content.contains("<string>/usr/local/bin/greggd</string>"));
        assert!(content.contains(GREGGD_LOG_PATH));
        assert!(content.contains("<string>run</string>"));
    }

    /// A taken UID or GID is an install error naming the conflict, never a
    /// silent adoption of another account.
    #[test]
    fn service_id_collision_names_the_conflicting_record() {
        // Real `dscl . -search /Users UniqueID 503` output.
        assert_eq!(
            parse_service_id_conflict("davidbowman\t\tUniqueID = (\n    503\n)\n"),
            Some("davidbowman".to_owned())
        );
        assert_eq!(
            parse_service_id_conflict("_someoneelse\t\tPrimaryGroupID = (\n    391\n)\n"),
            Some("_someoneelse".to_owned())
        );
        // A partial install of our own (a group created before a failed user
        // creation) is state to finish, not a conflict to refuse.
        assert_eq!(
            parse_service_id_conflict("_greggd\t\tPrimaryGroupID = (\n    391\n)\n"),
            None
        );
        // No output means the id is free.
        assert_eq!(parse_service_id_conflict(""), None);
        assert_eq!(parse_service_id_conflict("  \n\n"), None);
    }

    /// `chown` and `set_permissions` follow symlinks, so the ownership step
    /// decides on the link itself. A planted link must never be treated as a
    /// file or a directory to hand to root.
    #[test]
    #[cfg(unix)]
    fn ownership_target_checks_refuse_symlinks_and_non_files() {
        use std::os::unix::fs::symlink;

        let dir = std::env::temp_dir().join(format!(
            "gregg_launchd_own_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.toml");
        fs::write(&real, "name = \"greggd\"\n").unwrap();
        let link = dir.join("link.toml");
        symlink(&real, &link).unwrap();
        let nested = dir.join("nested");
        fs::create_dir_all(&nested).unwrap();
        let dir_link = dir.join("dir-link");
        symlink(&nested, &dir_link).unwrap();

        assert!(is_regular_non_symlink(&real));
        assert!(
            !is_regular_non_symlink(&link),
            "a symlinked config must not be chowned through"
        );
        assert!(!is_regular_non_symlink(&nested));
        assert!(!is_regular_non_symlink(&dir.join("missing")));

        assert!(is_regular_directory(&nested));
        assert!(is_regular_directory(&dir));
        assert!(
            !is_regular_directory(&dir_link),
            "a symlinked parent must be skipped"
        );
        assert!(!is_regular_directory(&real));

        let _ = fs::remove_dir_all(&dir);
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
                launchd_plist_content(&standard_launchd_config()),
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
        assert_eq!(
            launchd_uninstall_steps(false, false),
            [] as [super::LaunchdUninstallStep; 0]
        );
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
