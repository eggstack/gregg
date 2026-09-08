//! systemd unit content, installation, and restart.

use super::install::{
    elevated_command, ensure_config_preserved, is_privileged, manager_error_is_permission,
    write_atomic_text, InstallError,
};
use super::method::{
    is_systemd_environment, standard_systemd_binary, standard_systemd_config,
    standard_systemd_config_dir, standard_systemd_unit_path, StartupMethodArg,
};
use super::process::{run_bounded_command, MANAGER_COMMAND_TIMEOUT};
use std::fs;
use std::io::{self};
use std::path::{Path, PathBuf};
use std::process::Command;

// ── Canonical unit / plist content ────────────────────────────────────────

/// Canonical systemd unit. The installed binary can render this without a
/// checkout. Keep it synchronized with `packaging/systemd/greggd.service`.
pub fn systemd_unit_content() -> String {
    const TEMPLATE: &str = r"[Unit]
Description=Gregg metrics daemon
Documentation=https://github.com/eggstack/gregg
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=greggd
Group=greggd
RuntimeDirectory=gregg
ExecStart=/usr/local/bin/greggd run --config /etc/gregg/greggd.toml
Restart=on-failure
RestartSec=5
StartLimitIntervalSec=60
StartLimitBurst=5

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadOnlyPaths=/proc /sys
ReadWritePaths=/etc/gregg
PrivateTmp=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictNamespaces=true
RestrictSUIDSGID=true
MemoryDenyWriteExecute=true
RestrictRealtime=true
LockPersonality=true
SystemCallFilter=@system-service
SystemCallArchitectures=native

# Network access
IPAddressAllow=any
IPAddressDeny=

# Capabilities
CapabilityBoundingSet=
AmbientCapabilities=

[Install]
WantedBy=multi-user.target
";
    TEMPLATE.to_string()
}

/// Canonical launchd plist. Keep synchronized with
/// `packaging/launchd/com.eggstack.greggd.plist`.
#[allow(dead_code)]
pub(crate) fn systemd_unit_exists() -> bool {
    standard_systemd_unit_path().exists()
}
#[allow(dead_code)]
pub(crate) fn systemd_is_active() -> bool {
    matches!(
        run_bounded_command(
            "systemctl",
            &["is-active", "--quiet", "greggd"],
            MANAGER_COMMAND_TIMEOUT,
        ),
        Ok(output) if output.status.success()
    )
}
fn ensure_greggd_user() -> io::Result<()> {
    // Check if user exists via `id -u greggd`
    let mut cmd = Command::new("id");
    cmd.arg("-u");
    cmd.arg("greggd");
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    if let Ok(status) = cmd.status() {
        if status.success() {
            return Ok(());
        }
    }
    // Create system user
    let mut add = Command::new("useradd");
    add.arg("--system");
    add.arg("--no-create-home");
    add.arg("--shell");
    add.arg("/usr/sbin/nologin");
    add.arg("greggd");
    let status = add.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "useradd greggd failed with status {status}"
        )))
    }
}
fn set_config_ownership() -> io::Result<()> {
    // Best-effort: chown -R greggd:greggd /etc/gregg
    let dir = standard_systemd_config_dir();
    if !dir.exists() {
        return Ok(());
    }
    let mut cmd = Command::new("chown");
    cmd.arg("-R");
    cmd.arg("greggd:greggd");
    cmd.arg(&dir);
    let status = cmd.status()?;
    if status.success() {
        Ok(())
    } else {
        // Not fatal for install; log and continue.
        eprintln!(
            "warning: chown greggd:greggd {} failed: {status}",
            dir.display()
        );
        Ok(())
    }
}
fn run_systemctl(args: &[&str]) -> io::Result<()> {
    let output = run_bounded_command("systemctl", args, MANAGER_COMMAND_TIMEOUT)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "systemctl {} failed with status {:?}: {}",
            args.join(" "),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}
fn systemd_manager_error(exe: &Path, args: &[&str], error: io::Error) -> InstallError {
    if error.kind() == io::ErrorKind::PermissionDenied
        || manager_error_is_permission(&error.to_string())
    {
        InstallError::Permission {
            message: format!(
                "systemctl {} was denied; rerun as root: {}",
                args.join(" "),
                elevated_command(exe, StartupMethodArg::Systemd)
            ),
        }
    } else {
        InstallError::Io {
            path: PathBuf::from(format!("systemctl {}", args.join(" "))),
            source: error,
        }
    }
}

/// Install systemd service. `exe` is the current executable for elevated message.
pub fn install_systemd(exe: &Path, config_path: &Path) -> Result<(), InstallError> {
    // Verify systemd environment.
    if !is_systemd_environment() {
        return Err(InstallError::SystemdNotDetected {
            message: "systemd not detected: /run/systemd/system missing or PID 1 is not systemd"
                .into(),
        });
    }
    // Verify standard binary exists.
    let bin_path = standard_systemd_binary();
    if !bin_path.exists() {
        // Also check if current exe is at that path; if current exe exists but not at standard path, give actionable error.
        return Err(InstallError::BinaryMissing { path: bin_path });
    }
    // Privilege check: if not root, print elevated command and return PermissionDenied.
    if !is_privileged() {
        let cmd = elevated_command(exe, StartupMethodArg::Systemd);
        return Err(InstallError::Permission {
            message: format!("permission denied: rerun as root: {cmd}"),
        });
    }
    // Device steps (idempotent)
    ensure_greggd_user().map_err(|e| InstallError::Io {
        path: PathBuf::from("useradd greggd"),
        source: e,
    })?;
    // Ensure config dir and default config
    fs::create_dir_all(standard_systemd_config_dir()).map_err(|e| InstallError::Io {
        path: standard_systemd_config_dir(),
        source: e,
    })?;
    ensure_config_preserved(&standard_systemd_config()).map_err(|e| InstallError::Io {
        path: standard_systemd_config(),
        source: e,
    })?;
    set_config_ownership().map_err(|e| InstallError::Io {
        path: standard_systemd_config_dir(),
        source: e,
    })?;
    // Write unit atomically.
    let unit_content = systemd_unit_content();
    let unit_path = standard_systemd_unit_path();
    write_atomic_text(&unit_path, &unit_content).map_err(|e| {
        if e.kind() == io::ErrorKind::PermissionDenied {
            InstallError::Permission {
                message: format!(
                    "permission denied writing {}: rerun as root: {}",
                    unit_path.display(),
                    elevated_command(exe, StartupMethodArg::Systemd)
                ),
            }
        } else {
            InstallError::Io {
                path: unit_path.clone(),
                source: e,
            }
        }
    })?;
    // Ensure permissions 644
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&unit_path, fs::Permissions::from_mode(0o644));
    }
    // daemon-reload, enable, start/restart
    let daemon_reload = ["daemon-reload"];
    run_systemctl(&daemon_reload).map_err(|e| systemd_manager_error(exe, &daemon_reload, e))?;
    let enable = ["enable", "greggd"];
    run_systemctl(&enable).map_err(|e| systemd_manager_error(exe, &enable, e))?;
    // Decide start vs restart: if active, restart; else start.
    if systemd_is_active() {
        let restart = ["restart", "greggd"];
        run_systemctl(&restart).map_err(|e| systemd_manager_error(exe, &restart, e))?;
    } else {
        // Try start; if it fails because already running, try restart.
        if let Err(e) = run_systemctl(&["start", "greggd"]) {
            eprintln!("systemctl start failed ({e}), trying restart...");
            let restart = ["restart", "greggd"];
            run_systemctl(&restart).map_err(|e2| systemd_manager_error(exe, &restart, e2))?;
        }
    }
    println!("greggd systemd service installed: {}", unit_path.display());
    println!("config: {}", standard_systemd_config().display());
    println!("status: systemctl status greggd");
    println!("logs:   journalctl -u greggd -f");
    // Use config_path param to avoid unused warning; it is the caller's resolved config, which should match standard path.
    let _ = config_path;
    Ok(())
}
pub(crate) fn restart_systemd(exe: &Path) -> Result<(), InstallError> {
    if !systemd_unit_exists() {
        return Err(InstallError::Other(
            "systemd unit not installed: run `sudo greggd startup install --method systemd`".into(),
        ));
    }
    match run_systemctl(&["restart", "greggd"]) {
        Ok(()) => {
            println!("greggd restarted via systemd");
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(InstallError::Permission {
            message: format!(
                "permission denied: rerun as root: sudo systemctl restart greggd (original exe: {})",
                exe.display()
            ),
        }),
        Err(e) => {
            let msg = e.to_string();
            if manager_error_is_permission(&msg) {
                Err(InstallError::Permission {
                    message: format!(
                        "permission denied: rerun as root: sudo systemctl restart greggd (exe: {})",
                        exe.display()
                    ),
                })
            } else {
                Err(InstallError::Io {
                    path: PathBuf::from("systemctl restart greggd"),
                    source: e,
                })
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_unit_content_contains_hardening() {
        let content = systemd_unit_content();
        assert!(content.contains("ExecStart=/usr/local/bin/greggd"));
        assert!(content.contains("NoNewPrivileges"));
        assert!(content.contains("ProtectSystem"));
        assert!(content.contains("[Service]"));
        assert!(content.contains("[Unit]"));
    }
    #[test]
    fn embedded_systemd_unit_matches_packaging_file_when_present() {
        let packaging =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/systemd/greggd.service");
        if let Ok(file) = std::fs::read_to_string(&packaging) {
            let mut file_norm = file.replace("\r\n", "\n");
            if !file_norm.ends_with('\n') {
                file_norm.push('\n');
            }
            assert_eq!(
                systemd_unit_content(),
                file_norm,
                "embedded systemd unit must stay synchronized with packaging/systemd/greggd.service"
            );
        }
    }
}
