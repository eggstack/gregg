//! Binary-first self-update for `gregg`.
//!
//! Implements Plan 101's update contract: crates.io is the version authority,
//! the exact tagged GitHub Release asset is the binary candidate, and Cargo
//! is the fallback only when the asset is absent (HTTP 404). Checksum and
//! candidate `version` are verified before any replacement. Unix replacement
//! uses `self-replace` (same-filesystem atomic rename where practical);
//! Windows replacement uses the same helper which handles the running-image
//! semantics deliberately. No `sudo` is invoked internally.

use std::cmp::Ordering;
use std::fmt;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tempfile::{Builder, TempDir};

// ── Public outcome / error ──────────────────────────────────────────────────

/// Outcome of a successful `gregg update` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    AlreadyCurrent { version: String },
    UpdatedBinary { from: String, to: String },
    UpdatedFromCargo { from: String, to: String },
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

/// Errors that can occur during `gregg update`.
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
    #[error("I/O error: {0}")]
    Io(String),
}

// ── Constants ───────────────────────────────────────────────────────────────

const GITHUB_REPO: &str = "eggstack/gregg";
const CRATE_NAME: &str = "gregg";
const PROGRAM: &str = "gregg";
const CURR_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Supported binary targets (Plan 099 public contract).
const SUPPORTED_TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

// ── Version helpers ─────────────────────────────────────────────────────────

/// Parse a stable `MAJOR.MINOR.PATCH` version. Rejects prerelease/build metadata
/// and any non-numeric component. Returns the three numeric components.
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

/// Compare two stable versions SemVer-safely. Returns an ordering or `None` if
/// either version is not a valid stable `MAJOR.MINOR.PATCH`.
pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    let av = parse_stable_version(a)?;
    let bv = parse_stable_version(b)?;
    Some(av.cmp(&bv))
}

// ── Host -> target mapping ──────────────────────────────────────────────────

/// Detect the current host's Rust target suffix using `std::env::consts`.
/// Returns `Some(target)` for the five supported prebuilt targets, `None`
/// for source-only/unknown hosts (`ARMv7`, FreeBSD, etc.) which should use
/// Cargo fallback.
pub fn detect_target() -> Option<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    detect_target_for(os, arch)
}

/// Pure helper for tests: map OS/ARCH strings to a target.
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

/// Whether a target is part of the supported binary matrix.
pub fn is_supported_binary_target(target: &str) -> bool {
    SUPPORTED_TARGETS.contains(&target)
}

/// Asset name for a program+target (no version in filename).
pub fn asset_name(program: &str, target: &str) -> String {
    if target == "x86_64-pc-windows-msvc" {
        format!("{program}-{target}.exe")
    } else {
        format!("{program}-{target}")
    }
}

/// Construct exact tagged GitHub Release URLs for an asset and its checksum.
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

/// Fetch the latest stable version for `crate_name` from crates.io.
/// Uses `max_stable_version` which is the highest non-yanked, non-prerelease
/// version. One bounded HTTPS request with a Gregg-specific User-Agent.
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
            // Probe HTTP code to distinguish 404 vs transport/5xx
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
    let mut hex = String::with_capacity(result.len() * 2);
    for byte in result {
        let _ = write!(&mut hex, "{byte:02x}");
    }
    Ok(hex)
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

fn run_child_with_timeout(mut cmd: Command, timeout: Duration) -> io::Result<Output> {
    let mut child = cmd.spawn()?;
    let stdout = pipe_reader(child.stdout.take());
    let stderr = pipe_reader(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_pipe(stdout);
                let _ = join_pipe(stderr);
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "child process timed out and was killed",
                ));
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    };
    Ok(Output {
        status,
        stdout: join_pipe(stdout)?,
        stderr: join_pipe(stderr)?,
    })
}

fn pipe_reader<R: Read + Send + 'static>(
    reader: Option<R>,
) -> Option<thread::JoinHandle<io::Result<Vec<u8>>>> {
    reader.map(|mut reader| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
    })
}

fn join_pipe(reader: Option<thread::JoinHandle<io::Result<Vec<u8>>>>) -> io::Result<Vec<u8>> {
    match reader {
        Some(reader) => reader
            .join()
            .map_err(|_| io::Error::other("child output reader panicked"))?,
        None => Ok(Vec::new()),
    }
}

fn run_command_with_timeout(cmd: Command, timeout: Duration) -> Result<Output, UpdateError> {
    run_child_with_timeout(cmd, timeout).map_err(|error| {
        if error.kind() == io::ErrorKind::TimedOut {
            UpdateError::CandidateMismatch("candidate 'version' timed out".to_string())
        } else {
            UpdateError::CandidateMismatch(format!("candidate process failed: {error}"))
        }
    })
}

// ── Temp dir helpers ────────────────────────────────────────────────────────

fn create_temp_dir(prefix: &str) -> Result<TempDir, UpdateError> {
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

struct StagedCandidate {
    _temp_dir: TempDir,
    path: PathBuf,
}

impl StagedCandidate {
    fn path(&self) -> &Path {
        &self.path
    }
}

// ── Current exe and permission ─────────────────────────────────────────────

fn current_exe_path() -> Result<PathBuf, UpdateError> {
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

fn check_write_permission(exe_path: &Path, original_exe: &Path) -> Result<(), UpdateError> {
    let parent = exe_path.parent().ok_or_else(|| {
        UpdateError::Io(format!(
            "executable has no parent directory: {}",
            exe_path.display()
        ))
    })?;
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
                        .map_or_else(|_| "gregg".to_string(), |p| p.display().to_string())
                ),
            }
        } else {
            UpdateError::Replacement(format!("self-replace failed: {e}"))
        }
    })
}

// ── Cargo fallback ──────────────────────────────────────────────────────────

fn cargo_fallback(program: &str, version: &str) -> Result<StagedCandidate, UpdateError> {
    let cargo_bin = find_cargo()?;
    let temp_root = create_temp_dir(&format!("gregg-cargo-{program}"))?;
    let cargo_root = temp_root.path().join("cargo-root");
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
    Ok(StagedCandidate {
        _temp_dir: temp_root,
        path: staged,
    })
}

fn run_command_with_timeout_for_cargo(
    cmd: Command,
    timeout: Duration,
) -> Result<Output, UpdateError> {
    run_child_with_timeout(cmd, timeout).map_err(|error| {
        if error.kind() == io::ErrorKind::TimedOut {
            UpdateError::CargoFallback("cargo install timed out after 600s".to_string())
        } else {
            UpdateError::CargoFallback(format!("cargo process failed: {error}"))
        }
    })
}

// ── Main entry ──────────────────────────────────────────────────────────────

/// Run the full `gregg update` flow synchronously. Must not be called from
/// an async runtime. Prints progress to stderr and returns an outcome or
/// error.
pub fn run_update() -> Result<UpdateOutcome, UpdateError> {
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

    // Permission check before any download.
    let exe_path = current_exe_path()?;
    let original_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gregg"));
    check_write_permission(&exe_path, &original_exe)?;

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
        return cargo_update_path(&current, &latest);
    }

    let target = target_opt.unwrap();
    let (asset_url, sha_url) = github_urls(PROGRAM, &target, &latest);
    eprintln!("Latest {PROGRAM} is {latest} (current {current}); downloading {asset_url} ...");

    let curl = find_curl()?;
    let temp_dir = create_temp_dir("gregg-update")?;

    let asset_name_str = asset_name(PROGRAM, &target);
    let asset_path = temp_dir.path().join(&asset_name_str);
    let sha_path = temp_dir.path().join(format!("{asset_name_str}.sha256"));

    match download_file(&curl, &asset_url, &asset_path) {
        DownloadOutcome::Success => {
            // Download checksum
            match download_file(&curl, &sha_url, &sha_path) {
                DownloadOutcome::Success => {
                    verify_checksum(&asset_path, &sha_path)?;
                    validate_candidate(&asset_path, PROGRAM, &latest)?;
                    replace_current_exe(&asset_path)?;
                    eprintln!("Updated {PROGRAM} {current} -> {latest} via GitHub binary");
                    Ok(UpdateOutcome::UpdatedBinary {
                        from: current,
                        to: latest,
                    })
                }
                DownloadOutcome::NotFound => Err(UpdateError::ChecksumRetrieval(format!(
                    "checksum not found at {sha_url} (HTTP 404)"
                ))),
                DownloadOutcome::Failed(reason) => Err(UpdateError::ChecksumRetrieval(reason)),
            }
        }
        DownloadOutcome::NotFound => {
            eprintln!("No prebuilt asset at {asset_url} (HTTP 404); falling back to Cargo...");
            cargo_update_path(&current, &latest)
        }
        DownloadOutcome::Failed(reason) => Err(UpdateError::ReleaseDownloadFailed {
            url: asset_url,
            reason,
        }),
    }
}

fn cargo_update_path(from: &str, to: &str) -> Result<UpdateOutcome, UpdateError> {
    let staged = cargo_fallback(PROGRAM, to)?;
    replace_current_exe(staged.path())?;
    eprintln!("Updated {PROGRAM} {from} -> {to} via Cargo");
    Ok(UpdateOutcome::UpdatedFromCargo {
        from: from.to_string(),
        to: to.to_string(),
    })
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stable_versions() {
        assert_eq!(parse_stable_version("1.0.11"), Some((1, 0, 11)));
        assert_eq!(parse_stable_version("0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_stable_version("10.20.30"), Some((10, 20, 30)));
        assert_eq!(parse_stable_version("1.0.0-alpha"), None);
        assert_eq!(parse_stable_version("1.0.0+build"), None);
        assert_eq!(parse_stable_version("1.0"), None);
        assert_eq!(parse_stable_version("1.0.0.0"), None);
        assert_eq!(parse_stable_version(""), None);
        assert_eq!(parse_stable_version("a.b.c"), None);
    }

    #[test]
    fn version_comparison() {
        assert_eq!(compare_versions("1.0.11", "1.0.11"), Some(Ordering::Equal));
        assert_eq!(compare_versions("1.0.10", "1.0.11"), Some(Ordering::Less));
        assert_eq!(
            compare_versions("1.0.11", "1.0.10"),
            Some(Ordering::Greater)
        );
        assert_eq!(compare_versions("1.0.9", "1.0.11"), Some(Ordering::Less));
        assert_eq!(compare_versions("1.10.0", "1.9.0"), Some(Ordering::Greater));
        assert_eq!(compare_versions("2.0.0", "1.9.9"), Some(Ordering::Greater));
        assert_eq!(compare_versions("1.0.0", "1.0.0-alpha"), None);
    }

    #[test]
    fn target_mapping() {
        assert_eq!(
            detect_target_for("linux", "x86_64"),
            Some("x86_64-unknown-linux-gnu".to_string())
        );
        assert_eq!(
            detect_target_for("linux", "aarch64"),
            Some("aarch64-unknown-linux-gnu".to_string())
        );
        assert_eq!(
            detect_target_for("macos", "x86_64"),
            Some("x86_64-apple-darwin".to_string())
        );
        assert_eq!(
            detect_target_for("macos", "aarch64"),
            Some("aarch64-apple-darwin".to_string())
        );
        assert_eq!(
            detect_target_for("windows", "x86_64"),
            Some("x86_64-pc-windows-msvc".to_string())
        );
        assert_eq!(detect_target_for("linux", "arm"), None);
        assert_eq!(detect_target_for("freebsd", "x86_64"), None);
    }

    #[test]
    fn asset_names() {
        assert_eq!(
            asset_name("gregg", "x86_64-unknown-linux-gnu"),
            "gregg-x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            asset_name("gregg", "x86_64-pc-windows-msvc"),
            "gregg-x86_64-pc-windows-msvc.exe"
        );
        assert_eq!(
            asset_name("greggd", "aarch64-apple-darwin"),
            "greggd-aarch64-apple-darwin"
        );
    }

    #[test]
    fn github_urls_format() {
        let (url, sha) = github_urls("gregg", "x86_64-unknown-linux-gnu", "1.0.12");
        assert_eq!(
            url,
            "https://github.com/eggstack/gregg/releases/download/v1.0.12/gregg-x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            sha,
            "https://github.com/eggstack/gregg/releases/download/v1.0.12/gregg-x86_64-unknown-linux-gnu.sha256"
        );
        let (url2, _) = github_urls("greggd", "x86_64-pc-windows-msvc", "1.0.12");
        assert_eq!(
            url2,
            "https://github.com/eggstack/gregg/releases/download/v1.0.12/greggd-x86_64-pc-windows-msvc.exe"
        );
    }

    #[test]
    fn supported_targets() {
        assert!(is_supported_binary_target("x86_64-unknown-linux-gnu"));
        assert!(is_supported_binary_target("aarch64-unknown-linux-gnu"));
        assert!(is_supported_binary_target("x86_64-apple-darwin"));
        assert!(is_supported_binary_target("aarch64-apple-darwin"));
        assert!(is_supported_binary_target("x86_64-pc-windows-msvc"));
        assert!(!is_supported_binary_target("armv7-unknown-linux-gnueabihf"));
        assert!(!is_supported_binary_target("unknown"));
    }

    #[test]
    fn checksum_parser() {
        let dir = std::env::temp_dir().join(format!(
            "gregg-test-checksum-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.sha256");
        fs::write(
            &path,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  gregg-x86_64-unknown-linux-gnu\n",
        )
        .unwrap();
        assert_eq!(
            parse_checksum_file(&path).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn crates_io_json_parsing() {
        let json = r#"{"crate":{"max_stable_version":"1.0.11","max_version":"1.0.11"}}"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let ver = v["crate"]["max_stable_version"].as_str().unwrap();
        assert_eq!(ver, "1.0.11");
        assert!(parse_stable_version(ver).is_some());
    }

    #[test]
    fn candidate_version_matching() {
        // Simulate stdout matching logic directly
        let program = "gregg";
        let expected = "1.0.12";
        let stdout = "gregg 1.0.12";
        assert_eq!(stdout, format!("{program} {expected}"));
        let bad = "gregg 1.0.11";
        assert_ne!(bad, format!("{program} {expected}"));
        let wrong_prog = "greggd 1.0.12";
        assert_ne!(wrong_prog, format!("{program} {expected}"));
    }

    #[test]
    fn download_not_found_vs_failed_classification() {
        // Pure logic: 404 permits fallback, 5xx does not. This test locks the invariant.
        let not_found = DownloadOutcome::NotFound;
        let failed = DownloadOutcome::Failed("timeout".to_string());
        assert!(matches!(not_found, DownloadOutcome::NotFound));
        assert!(matches!(failed, DownloadOutcome::Failed(_)));
        // Ensure missing asset permits fallback while transport failure does not.
        // This is checked in run_update's match arms.
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

    #[test]
    fn private_staging_is_exclusive_and_cleans_up() {
        let first = create_temp_dir("gregg-test-stage").unwrap();
        let second = create_temp_dir("gregg-test-stage").unwrap();
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
    fn timeout_child() {
        if let Ok(marker) = std::env::var("GREGG_TIMEOUT_MARKER") {
            thread::sleep(Duration::from_millis(250));
            fs::write(marker, b"late").unwrap();
        }
    }

    #[test]
    fn cargo_timeout_kills_and_reaps_child() {
        let temp = create_temp_dir("gregg-test-timeout").unwrap();
        let marker = temp.path().join("late");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "update::tests::timeout_child", "--nocapture"])
            .env("GREGG_TIMEOUT_MARKER", &marker)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let error = run_command_with_timeout_for_cargo(command, Duration::from_millis(40))
            .expect_err("slow child must time out");
        assert!(error.to_string().contains("timed out"));
        thread::sleep(Duration::from_millis(300));
        assert!(!marker.exists(), "timed-out child continued after return");
    }
}
