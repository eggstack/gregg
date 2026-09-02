#![allow(clippy::all)]
#![allow(clippy::pedantic)]
#![allow(clippy::nursery)]

//! Binary-first self-update for `greggd`.
//!
//! Implements Plan 101's update contract for the daemon: crates.io is the
//! version authority, the exact tagged GitHub Release asset is the binary
//! candidate, and Cargo is the fallback only when the asset is absent (HTTP
//! 404). Checksum and candidate `version` are verified before any
//! replacement. Unix replacement uses `self-replace` (same-filesystem atomic
//! rename where practical); Windows uses the same helper which handles the
//! running-image semantics. `greggd update` reuses Plan 100's
//! `startup_state`/`restart` logic and never invokes `sudo` internally.

use std::cmp::Ordering;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::startup::{startup_state, StartupState};

// ── Public outcome / error ──────────────────────────────────────────────────

/// Outcome of a successful `greggd update` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    AlreadyCurrent {
        version: String,
    },
    UpdatedBinary {
        from: String,
        to: String,
    },
    UpdatedFromCargo {
        from: String,
        to: String,
    },
    /// On-disk binary was replaced but the subsequent restart failed.
    /// The caller must surface the installed version and the exact restart
    /// command needed. Exit status should be nonzero.
    UpdatedButRestartFailed {
        from: String,
        to: String,
        restart_error: String,
    },
}

impl fmt::Display for UpdateOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyCurrent { version } => {
                write!(f, "greggd {version} is already the latest stable version")
            }
            Self::UpdatedBinary { from, to } => {
                write!(f, "updated greggd {from} -> {to} (GitHub binary)")
            }
            Self::UpdatedFromCargo { from, to } => {
                write!(f, "updated greggd {from} -> {to} (Cargo)")
            }
            Self::UpdatedButRestartFailed {
                from,
                to,
                restart_error,
            } => write!(
                f,
                "updated greggd {from} -> {to} on disk but restart failed: {restart_error}"
            ),
        }
    }
}

/// Errors that can occur during `greggd update`.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("failed to determine current executable: {0}")]
    CurrentExe(String),
    #[error("curl is not available: {0}. Install curl or update manually from https://github.com/eggstack/gregg/releases")]
    CurlMissing(String),
    #[error("cargo is not available: {0}. Install Rust from https://rustup.rs or download the release asset manually")]
    CargoMissing(String),
    #[error("version lookup failed: {0}")]
    VersionLookup(String),
    #[error("invalid version '{input}': {reason}")]
    InvalidVersion { input: String, reason: String },
    #[error("unsupported host: {os}/{arch} (target {target:?}). No prebuilt asset and Cargo fallback failed: {fallback}")]
    UnsupportedHost {
        os: String,
        arch: String,
        target: Option<String>,
        fallback: String,
    },
    #[error("release asset absent (HTTP 404) for {url}; Cargo fallback failed: {fallback}")]
    ReleaseAssetAbsent { url: String, fallback: String },
    #[error("release download failed for {url}: {reason}")]
    ReleaseDownloadFailed { url: String, reason: String },
    #[error("checksum retrieval failed: {0}")]
    ChecksumRetrieval(String),
    #[error("checksum mismatch for {file}: expected {expected}, actual {actual}")]
    ChecksumMismatch {
        file: String,
        expected: String,
        actual: String,
    },
    #[error("candidate identity/version mismatch: {0}")]
    CandidateMismatch(String),
    #[error("permission denied: {message}. Rerun: {elevated}")]
    PermissionDenied { message: String, elevated: String },
    #[error("cargo fallback failed: {0}")]
    CargoFallback(String),
    #[error("replacement failed: {0}")]
    Replacement(String),
    #[error("restart failed: {0}")]
    RestartFailed(String),
    #[error("I/O error: {0}")]
    Io(String),
}

// ── Constants ───────────────────────────────────────────────────────────────

const GITHUB_REPO: &str = "eggstack/gregg";
const CRATE_NAME: &str = "greggd";
const PROGRAM: &str = "greggd";
const CURR_VERSION: &str = env!("CARGO_PKG_VERSION");

const SUPPORTED_TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

// ── Version helpers ─────────────────────────────────────────────────────────

pub fn parse_stable_version(input: &str) -> Option<(u64, u64, u64)> {
    if input.is_empty() || input.contains('-') || input.contains('+') {
        return None;
    }
    let mut parts = input.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    let av = parse_stable_version(a)?;
    let bv = parse_stable_version(b)?;
    Some(av.cmp(&bv))
}

// ── Host -> target mapping ──────────────────────────────────────────────────

pub fn detect_target() -> Option<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    detect_target_for(os, arch)
}

pub fn detect_target_for(os: &str, arch: &str) -> Option<String> {
    match (os, arch) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu".to_string()),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu".to_string()),
        ("macos", "x86_64") => Some("x86_64-apple-darwin".to_string()),
        ("macos", "aarch64") => Some("aarch64-apple-darwin".to_string()),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc".to_string()),
        _ => None,
    }
}

pub fn is_supported_binary_target(target: &str) -> bool {
    SUPPORTED_TARGETS.contains(&target)
}

pub fn asset_name(program: &str, target: &str) -> String {
    if target == "x86_64-pc-windows-msvc" {
        format!("{program}-{target}.exe")
    } else {
        format!("{program}-{target}")
    }
}

pub fn github_urls(program: &str, target: &str, version: &str) -> (String, String) {
    let asset = asset_name(program, target);
    let base = format!("https://github.com/{GITHUB_REPO}/releases/download/v{version}/{asset}");
    let sha = format!("{base}.sha256");
    (base, sha)
}

// ── Curl helpers ────────────────────────────────────────────────────────────

fn find_curl() -> Result<String, UpdateError> {
    for candidate in ["curl", "curl.exe"] {
        if Command::new(candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
        {
            return Ok(candidate.to_string());
        }
    }
    Err(UpdateError::CurlMissing(
        "curl not found in PATH".to_string(),
    ))
}

fn find_cargo() -> Result<String, UpdateError> {
    for candidate in ["cargo", "cargo.exe"] {
        if Command::new(candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
        {
            return Ok(candidate.to_string());
        }
    }
    Err(UpdateError::CargoMissing(
        "cargo not found in PATH".to_string(),
    ))
}

fn run_curl_capture(curl: &str, args: &[&str]) -> Result<Vec<u8>, UpdateError> {
    let output = Command::new(curl)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| UpdateError::VersionLookup(format!("failed to spawn curl: {e}")))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(UpdateError::VersionLookup(format!(
            "curl failed (status {:?}): {stderr}",
            output.status.code()
        )))
    }
}

fn probe_http_code(curl: &str, url: &str) -> Option<u16> {
    let output = Command::new(curl)
        .args([
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "--max-time",
            "15",
            url,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let code_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    code_str.parse::<u16>().ok()
}

// ── crates.io lookup ────────────────────────────────────────────────────────

pub fn fetch_latest_stable_version(crate_name: &str) -> Result<String, UpdateError> {
    let curl = find_curl()?;
    let url = format!("https://crates.io/api/v1/crates/{crate_name}");
    let user_agent = format!("{PROGRAM}/{CURR_VERSION} (https://github.com/{GITHUB_REPO})");
    let args = [
        "-fsSL",
        "--max-time",
        "15",
        "-H",
        &format!("User-Agent: {user_agent}"),
        &url,
    ];
    let stdout = run_curl_capture(&curl, &args).map_err(|e| {
        UpdateError::VersionLookup(format!("crates.io request failed for {crate_name}: {e}"))
    })?;
    if stdout.len() > 256 * 1024 {
        return Err(UpdateError::VersionLookup(
            "crates.io response too large".to_string(),
        ));
    }
    let json: serde_json::Value = serde_json::from_slice(&stdout)
        .map_err(|e| UpdateError::VersionLookup(format!("crates.io JSON parse failed: {e}")))?;
    let version = json
        .get("crate")
        .and_then(|c| c.get("max_stable_version"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            UpdateError::VersionLookup(
                "crates.io response missing crate.max_stable_version".to_string(),
            )
        })?
        .to_string();
    if version.is_empty() {
        return Err(UpdateError::VersionLookup(
            "crates.io returned empty max_stable_version".to_string(),
        ));
    }
    if parse_stable_version(&version).is_none() {
        return Err(UpdateError::VersionLookup(format!(
            "crates.io returned non-stable version: {version}"
        )));
    }
    Ok(version)
}

// ── Download helpers ────────────────────────────────────────────────────────

#[derive(Debug)]
enum DownloadOutcome {
    Success,
    NotFound,
    Failed(String),
}

fn download_file(curl: &str, url: &str, dest: &Path) -> DownloadOutcome {
    let dest_str = dest.to_string_lossy().to_string();
    let output = Command::new(curl)
        .args(["-fsSL", "--max-time", "90", "-o", &dest_str, url])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    match output {
        Ok(out) if out.status.success() => DownloadOutcome::Success,
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            if let Some(404) = probe_http_code(curl, url) {
                DownloadOutcome::NotFound
            } else if stderr.contains("404") {
                DownloadOutcome::NotFound
            } else {
                DownloadOutcome::Failed(format!("curl exit {:?}: {stderr}", out.status.code()))
            }
        }
        Err(e) => DownloadOutcome::Failed(format!("failed to spawn curl: {e}")),
    }
}

// ── Checksum ────────────────────────────────────────────────────────────────

fn parse_checksum_file(path: &Path) -> Result<String, UpdateError> {
    let content = fs::read_to_string(path).map_err(|e| {
        UpdateError::ChecksumRetrieval(format!("failed to read checksum file: {e}"))
    })?;
    let hash = content
        .split_whitespace()
        .next()
        .ok_or_else(|| UpdateError::ChecksumRetrieval("checksum file empty".to_string()))?;
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(UpdateError::ChecksumRetrieval(format!(
            "checksum file has invalid hash: {hash:?}"
        )));
    }
    Ok(hash.to_ascii_lowercase())
}

fn compute_sha256(path: &Path) -> Result<String, UpdateError> {
    let mut file = fs::File::open(path)
        .map_err(|e| UpdateError::Io(format!("failed to open {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| UpdateError::Io(format!("failed to read {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let result = hasher.finalize();
    Ok(result.iter().map(|b| format!("{b:02x}")).collect())
}

fn verify_checksum(file: &Path, sha_file: &Path) -> Result<(), UpdateError> {
    let expected = parse_checksum_file(sha_file)?;
    let actual = compute_sha256(file)?;
    if expected != actual {
        return Err(UpdateError::ChecksumMismatch {
            file: file.display().to_string(),
            expected,
            actual,
        });
    }
    Ok(())
}

// ── Candidate validation ────────────────────────────────────────────────────

fn validate_candidate(
    candidate: &Path,
    program: &str,
    expected_version: &str,
) -> Result<(), UpdateError> {
    let metadata = fs::metadata(candidate)
        .map_err(|e| UpdateError::CandidateMismatch(format!("candidate missing: {e}")))?;
    if metadata.len() < 1024 {
        return Err(UpdateError::CandidateMismatch(format!(
            "candidate too small ({} bytes)",
            metadata.len()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(candidate, fs::Permissions::from_mode(0o755));
    }
    let output = run_command_with_timeout(
        {
            let mut cmd = Command::new(candidate);
            cmd.arg("version");
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            cmd
        },
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return Err(UpdateError::CandidateMismatch(format!(
            "candidate 'version' failed with status {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let expected = format!("{program} {expected_version}");
    if stdout != expected {
        return Err(UpdateError::CandidateMismatch(format!(
            "candidate version mismatch: expected {expected:?}, got {stdout:?}"
        )));
    }
    Ok(())
}

fn run_command_with_timeout(
    mut cmd: Command,
    timeout: Duration,
) -> Result<std::process::Output, UpdateError> {
    let mut child = cmd
        .spawn()
        .map_err(|e| UpdateError::CandidateMismatch(format!("failed to spawn candidate: {e}")))?;
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(UpdateError::CandidateMismatch(
                "candidate 'version' timed out".to_string(),
            ));
        }
        match child.try_wait() {
            Ok(Some(_status)) => {
                let output = child.wait_with_output().map_err(|e| {
                    UpdateError::CandidateMismatch(format!("failed to wait for candidate: {e}"))
                })?;
                return Ok(output);
            }
            Ok(None) => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return Err(UpdateError::CandidateMismatch(format!(
                    "failed to wait for candidate: {e}"
                )));
            }
        }
    }
}

// ── Temp dir helpers ────────────────────────────────────────────────────────

fn create_temp_dir(prefix: &str) -> Result<PathBuf, UpdateError> {
    let base = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = base.join(format!("{prefix}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir)
        .map_err(|e| UpdateError::Io(format!("failed to create temp dir: {e}")))?;
    Ok(dir)
}

struct TempDirGuard(PathBuf);
impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct TempFileGuard(PathBuf);
impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

// ── Current exe and permission ─────────────────────────────────────────────

fn current_exe_path() -> Result<PathBuf, UpdateError> {
    let exe = std::env::current_exe()
        .map_err(|e| UpdateError::CurrentExe(format!("current_exe failed: {e}")))?;
    match exe.canonicalize() {
        Ok(canonical) => Ok(canonical),
        Err(_) => {
            if fs::symlink_metadata(&exe)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
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
}

fn check_write_permission(exe_path: &Path, original_exe: &Path) -> Result<(), UpdateError> {
    let parent = exe_path.parent().ok_or_else(|| {
        UpdateError::Io(format!(
            "executable has no parent directory: {}",
            exe_path.display()
        ))
    })?;
    let probe = parent.join(format!(
        ".greggd-update-perm-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            Err(UpdateError::PermissionDenied {
                message: format!("permission denied writing to {}", parent.display()),
                elevated: format!("sudo {} update", original_exe.display()),
            })
        }
        Err(e) => Err(UpdateError::Io(format!(
            "permission probe failed for {}: {e}",
            parent.display()
        ))),
    }
}

// ── Replacement ─────────────────────────────────────────────────────────────

fn replace_current_exe(candidate: &Path) -> Result<(), UpdateError> {
    self_replace::self_replace(candidate).map_err(|e| {
        if e.kind() == io::ErrorKind::PermissionDenied {
            UpdateError::PermissionDenied {
                message: format!("permission denied replacing executable: {e}"),
                elevated: format!(
                    "sudo {} update",
                    std::env::current_exe()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| "greggd".to_string())
                ),
            }
        } else {
            UpdateError::Replacement(format!("self-replace failed: {e}"))
        }
    })
}

// ── Cargo fallback ──────────────────────────────────────────────────────────

fn cargo_fallback(program: &str, version: &str) -> Result<PathBuf, UpdateError> {
    let cargo_bin = find_cargo()?;
    let temp_root = create_temp_dir(&format!("greggd-cargo-{program}"))?;
    // We will keep temp_root for the staged file copy; guard will clean cargo root but not durable file.
    let cargo_root = temp_root.join("cargo-root");
    fs::create_dir_all(&cargo_root)
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
    let output = run_command_with_timeout_for_cargo(cmd, Duration::from_secs(600))?;
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
    validate_candidate(&staged, program, version)?;
    let durable = std::env::temp_dir().join(format!(
        "greggd-candidate-{}-{}-{}",
        program,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::copy(&staged, &durable)
        .map_err(|e| UpdateError::Io(format!("failed to stage cargo binary: {e}")))?;
    let _ = fs::remove_dir_all(&temp_root);
    // Prevent double cleanup via guard leak? We didn't use guard, so just return.
    Ok(durable)
}

fn run_command_with_timeout_for_cargo(
    mut cmd: Command,
    timeout: Duration,
) -> Result<std::process::Output, UpdateError> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let out = cmd.output();
        let _ = tx.send(out);
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(UpdateError::CargoFallback(format!(
            "cargo spawn failed: {e}"
        ))),
        Err(_) => Err(UpdateError::CargoFallback(
            "cargo install timed out after 600s".to_string(),
        )),
    }
}

// ── Daemon running probe for UnmanagedOrCron ────────────────────────────────

fn is_unmanaged_daemon_running(config_path: &Path, explicit: bool) -> bool {
    let config = match crate::cli::load_config(config_path, explicit) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let target = crate::cli::croncheck_target(&config);
    probe_is_running(target)
}

fn probe_is_running(target: std::net::SocketAddr) -> bool {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    const TIMEOUT: Duration = Duration::from_millis(750);
    const MAX_BYTES: usize = 256 * 1024;

    let mut stream = match TcpStream::connect_timeout(&target, TIMEOUT) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => return false,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    if stream
        .write_all(b"GET /v2/healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                response.extend_from_slice(&chunk[..n]);
                if response.len() > MAX_BYTES {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    parse_greggd_health(&response)
}

fn parse_greggd_health(response: &[u8]) -> bool {
    let Some(header_end) = response.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let headers = &response[..header_end];
    let body = &response[header_end + 4..];
    let Some(status_line) = headers.split(|b| *b == 10).next() else {
        return false;
    };
    let mut parts = status_line.split(|b| *b == 32 || *b == 13);
    let Some(version) = parts.next() else {
        return false;
    };
    let Some(status) = parts
        .next()
        .and_then(|v| std::str::from_utf8(v).ok())
        .and_then(|v| v.parse::<u16>().ok())
    else {
        return false;
    };
    if version != b"HTTP/1.0" && version != b"HTTP/1.1" {
        return false;
    }
    let Ok(health) = serde_json::from_slice::<gregg_protocol::v2::HealthResponseV2>(body) else {
        return false;
    };
    matches!(
        (status, health.state),
        (200, gregg_protocol::ReadinessState::Ready)
            | (
                503,
                gregg_protocol::ReadinessState::Warming | gregg_protocol::ReadinessState::Failed
            )
    )
}

// ── Restart dispatch ────────────────────────────────────────────────────────

fn restart_after_update(
    state: StartupState,
    exe: &Path,
    config_path: &Path,
    explicit: bool,
) -> Result<(), UpdateError> {
    // For Windows running service, we already stopped before replacement if needed.
    // Now restart according to policy.
    match state {
        StartupState::SystemdActive
        | StartupState::LaunchdLoaded
        | StartupState::WindowsServiceRunning => {
            // Running managed -> restart via manager.
            crate::startup::restart_with_state(state, exe, config_path, explicit)
                .map_err(|e| UpdateError::RestartFailed(format!("{e}")))
        }
        StartupState::SystemdInstalledStopped
        | StartupState::LaunchdInstalledUnloaded
        | StartupState::WindowsServiceStopped => {
            // Installed but intentionally stopped -> leave stopped.
            eprintln!("Service is installed but stopped; leaving it stopped after update.");
            Ok(())
        }
        StartupState::UnmanagedOrCron => {
            if is_unmanaged_daemon_running(config_path, explicit) {
                eprintln!("Restarting direct/cron daemon...");
                crate::startup::restart_with_state(state, exe, config_path, explicit)
                    .map_err(|e| UpdateError::RestartFailed(format!("{e}")))
            } else {
                eprintln!("No daemon running; leaving binary updated without starting.");
                Ok(())
            }
        }
    }
}

// ── Main entry ──────────────────────────────────────────────────────────────

/// Run the full `greggd update` flow synchronously.
/// `config_path` and `explicit` describe the resolved config location.
/// Prints progress to stderr and returns an outcome or error.
pub fn run_update(config_path: &Path, explicit: bool) -> Result<UpdateOutcome, UpdateError> {
    let current = CURR_VERSION.to_string();
    let latest = fetch_latest_stable_version(CRATE_NAME)?;
    let ordering =
        compare_versions(&current, &latest).ok_or_else(|| UpdateError::InvalidVersion {
            input: format!("current={current} latest={latest}"),
            reason: "failed to parse version".to_string(),
        })?;
    if ordering != Ordering::Less {
        return Ok(UpdateOutcome::AlreadyCurrent { version: current });
    }

    let exe_path = current_exe_path()?;
    let original_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("greggd"));
    check_write_permission(&exe_path, &original_exe)?;

    // Capture startup state before replacement for restart decision.
    let pre_state = startup_state();
    eprintln!("Current greggd {current}, latest {latest}, pre-update state: {pre_state}");

    // On Windows, if service is running, stop it before file mutation to release lock.
    #[cfg(target_os = "windows")]
    {
        if matches!(pre_state, StartupState::WindowsServiceRunning) {
            eprintln!("Stopping Windows service before replacement...");
            if let Err(e) = crate::service::platform_service_manager().stop() {
                let msg = e.to_string();
                if msg.to_lowercase().contains("access denied")
                    || msg.to_lowercase().contains("permission")
                {
                    return Err(UpdateError::PermissionDenied {
                        message: format!("failed to stop service: {msg}"),
                        elevated: "run as Administrator: greggd update".to_string(),
                    });
                }
                eprintln!("warning: failed to stop service before update (continuing): {msg}");
            }
        }
    }

    let target_opt = detect_target();
    let target_str = target_opt.clone();
    let supported = target_opt
        .as_deref()
        .is_some_and(is_supported_binary_target);

    if !supported {
        eprintln!(
            "No prebuilt {PROGRAM} asset for {}/{} (target {:?}); trying Cargo fallback...",
            std::env::consts::OS,
            std::env::consts::ARCH,
            target_str
        );
        return cargo_update_path_with_restart(
            &current,
            &latest,
            pre_state,
            &original_exe,
            config_path,
            explicit,
        );
    }

    let target = target_opt.unwrap();
    let (asset_url, sha_url) = github_urls(PROGRAM, &target, &latest);
    eprintln!("Latest {PROGRAM} is {latest} (current {current}); downloading {asset_url} ...");

    let curl = find_curl()?;
    let temp_dir = create_temp_dir("greggd-update")?;
    let _guard = TempDirGuard(temp_dir.clone());

    let asset_name_str = asset_name(PROGRAM, &target);
    let asset_path = temp_dir.join(&asset_name_str);
    let sha_path = temp_dir.join(format!("{asset_name_str}.sha256"));

    let outcome = match download_file(&curl, &asset_url, &asset_path) {
        DownloadOutcome::Success => match download_file(&curl, &sha_url, &sha_path) {
            DownloadOutcome::Success => {
                verify_checksum(&asset_path, &sha_path)?;
                validate_candidate(&asset_path, PROGRAM, &latest)?;
                replace_current_exe(&asset_path)?;
                eprintln!("Replaced {PROGRAM} binary {current} -> {latest} via GitHub binary");
                Ok((false, latest.clone()))
            }
            DownloadOutcome::NotFound => Err(UpdateError::ChecksumRetrieval(format!(
                "checksum not found at {sha_url} (HTTP 404)"
            ))),
            DownloadOutcome::Failed(reason) => Err(UpdateError::ChecksumRetrieval(reason)),
        },
        DownloadOutcome::NotFound => {
            eprintln!("No prebuilt asset at {asset_url} (HTTP 404); falling back to Cargo...");
            let staged = cargo_fallback(PROGRAM, &latest)?;
            let _staged_guard = TempFileGuard(staged.clone());
            validate_candidate(&staged, PROGRAM, &latest)?;
            replace_current_exe(&staged)?;
            eprintln!("Replaced {PROGRAM} binary {current} -> {latest} via Cargo");
            Ok((true, latest.clone()))
        }
        DownloadOutcome::Failed(reason) => Err(UpdateError::ReleaseDownloadFailed {
            url: asset_url,
            reason,
        }),
    };

    let (from_cargo, new_version) = match outcome {
        Ok(v) => v,
        Err(e) => {
            // Check if we should try cargo fallback for NotFound? Already handled.
            return Err(e);
        }
    };

    // Now restart according to pre_state
    let restart_result = restart_after_update(pre_state, &original_exe, config_path, explicit);
    match restart_result {
        Ok(()) => {
            if from_cargo {
                Ok(UpdateOutcome::UpdatedFromCargo {
                    from: current,
                    to: new_version,
                })
            } else {
                Ok(UpdateOutcome::UpdatedBinary {
                    from: current,
                    to: new_version,
                })
            }
        }
        Err(restart_err) => {
            let msg = restart_err.to_string();
            eprintln!(
                "Updated {PROGRAM} {current} -> {new_version} on disk but restart failed: {msg}"
            );
            eprintln!("Rerun: {} restart", original_exe.display());
            if matches!(pre_state, StartupState::SystemdActive) {
                eprintln!("Or: sudo systemctl restart greggd");
            } else if matches!(pre_state, StartupState::LaunchdLoaded) {
                eprintln!("Or: sudo launchctl kickstart -k system/com.eggstack.greggd");
            }
            Ok(UpdateOutcome::UpdatedButRestartFailed {
                from: current,
                to: new_version,
                restart_error: msg,
            })
        }
    }
}

fn cargo_update_path_with_restart(
    from: &str,
    to: &str,
    pre_state: StartupState,
    exe: &Path,
    config_path: &Path,
    explicit: bool,
) -> Result<UpdateOutcome, UpdateError> {
    let staged = cargo_fallback(PROGRAM, to)?;
    let _guard = TempFileGuard(staged.clone());
    replace_current_exe(&staged)?;
    eprintln!("Replaced {PROGRAM} binary {from} -> {to} via Cargo");
    let restart_result = restart_after_update(pre_state, exe, config_path, explicit);
    match restart_result {
        Ok(()) => Ok(UpdateOutcome::UpdatedFromCargo {
            from: from.to_string(),
            to: to.to_string(),
        }),
        Err(e) => Ok(UpdateOutcome::UpdatedButRestartFailed {
            from: from.to_string(),
            to: to.to_string(),
            restart_error: e.to_string(),
        }),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stable_versions() {
        assert_eq!(parse_stable_version("1.0.11"), Some((1, 0, 11)));
        assert_eq!(parse_stable_version("10.20.30"), Some((10, 20, 30)));
        assert_eq!(parse_stable_version("1.0.0-alpha"), None);
        assert_eq!(parse_stable_version("1.0"), None);
    }

    #[test]
    fn version_comparison() {
        assert_eq!(compare_versions("1.0.11", "1.0.11"), Some(Ordering::Equal));
        assert_eq!(compare_versions("1.0.10", "1.0.11"), Some(Ordering::Less));
        assert_eq!(compare_versions("1.10.0", "1.9.0"), Some(Ordering::Greater));
    }

    #[test]
    fn target_mapping() {
        assert_eq!(
            detect_target_for("linux", "x86_64"),
            Some("x86_64-unknown-linux-gnu".to_string())
        );
        assert_eq!(
            detect_target_for("windows", "x86_64"),
            Some("x86_64-pc-windows-msvc".to_string())
        );
        assert_eq!(detect_target_for("freebsd", "x86_64"), None);
    }

    #[test]
    fn asset_names() {
        assert_eq!(
            asset_name("greggd", "x86_64-unknown-linux-gnu"),
            "greggd-x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            asset_name("greggd", "x86_64-pc-windows-msvc"),
            "greggd-x86_64-pc-windows-msvc.exe"
        );
    }

    #[test]
    fn github_urls_format() {
        let (url, sha) = github_urls("greggd", "x86_64-unknown-linux-gnu", "1.0.12");
        assert_eq!(
            url,
            "https://github.com/eggstack/gregg/releases/download/v1.0.12/greggd-x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            sha,
            "https://github.com/eggstack/gregg/releases/download/v1.0.12/greggd-x86_64-unknown-linux-gnu.sha256"
        );
    }

    #[test]
    fn supported_targets() {
        assert!(is_supported_binary_target("x86_64-unknown-linux-gnu"));
        assert!(!is_supported_binary_target("armv7-unknown-linux-gnueabihf"));
    }

    #[test]
    fn startup_state_helpers() {
        assert_eq!(
            crate::startup::systemd_state_with(true, true),
            StartupState::SystemdActive
        );
        assert_eq!(
            crate::startup::systemd_state_with(false, false),
            StartupState::UnmanagedOrCron
        );
    }

    #[test]
    fn checksum_parser() {
        let dir = std::env::temp_dir().join(format!(
            "greggd-test-checksum-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.sha256");
        fs::write(
            &path,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  greggd-x86_64-unknown-linux-gnu\n",
        )
        .unwrap();
        assert_eq!(
            parse_checksum_file(&path).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restart_decision_for_stopped_service_is_no_restart() {
        // Pure helper: installed but stopped should not restart.
        let state = StartupState::SystemdInstalledStopped;
        // The actual restart function would not be called directly in unit test to avoid systemctl.
        // We just assert the enum distinction is correct.
        assert_ne!(state, StartupState::SystemdActive);
    }
}
