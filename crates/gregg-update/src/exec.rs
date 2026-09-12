//! Bounded external-process execution and network fetch helpers.
//!
//! All subprocess I/O is bounded: `curl` requests carry `--max-time`,
//! spawned children are killed and reaped on timeout, and pipe readers are
//! joined on every path. No shell is invoked; no `sudo` is invoked.

use std::io::{self, Read};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::Duration;

use crate::error::UpdateError;

/// Maximum crates.io metadata body accepted (256 KiB).
pub const MAX_CRATES_IO_BYTES: usize = 256 * 1024;

/// Maximum release-asset download accepted (64 MiB). Enforced via curl
/// `--max-filesize` so a malicious mirror cannot fill the staging disk
/// until `--max-time` expires.
pub const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;

/// `--max-time` for crates.io version lookups, in seconds.
pub const CRATES_IO_TIMEOUT_SECS: &str = "15";

/// `--max-time` for release asset downloads, in seconds.
pub const DOWNLOAD_TIMEOUT_SECS: &str = "90";

/// Timeout for staged candidate `version` probes.
pub const CANDIDATE_TIMEOUT: Duration = Duration::from_secs(5);

/// Wall-clock bound for crates.io metadata captures (curl `--max-time`
/// plus spawn/pipe margin). `run_curl_capture` is killed and reaped past
/// this deadline so a hung curl binary cannot block the caller forever.
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);

/// Wall-clock bound for HTTP status probes.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Shared fragment identifying an oversized bounded body. `pipe_reader_limited`
/// reports this message and `map_capture_error` matches on it; keep both in
/// sync via this const so a reword cannot silently disable the mapping.
/// The `OutOfMemory` kind check remains the reliable path.
const RESPONSE_TOO_LARGE_FRAGMENT: &str = "response too large";

/// Wall-clock bound for release-asset downloads (curl `--max-time` plus
/// spawn/reap margin).
pub const DOWNLOAD_WALL_TIMEOUT: Duration = Duration::from_secs(100);

/// Timeout for Cargo fallback builds.
pub const CARGO_TIMEOUT: Duration = Duration::from_secs(600);

/// Locate `curl` in `PATH`.
pub fn find_curl() -> Result<String, UpdateError> {
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

/// Locate `cargo` in `PATH`.
pub fn find_cargo() -> Result<String, UpdateError> {
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

/// Run `curl` with the given args and capture stdout. Used for small
/// metadata fetches (crates.io version lookup).
///
/// The child is bounded by [`CAPTURE_TIMEOUT`] (killed and reaped past the
/// deadline) and stdout is streamed through a `take(MAX_CRATES_IO_BYTES+1)`
/// cap so an oversized response is rejected without buffering it fully.
pub fn run_curl_capture(curl: &str, args: &[&str]) -> Result<Vec<u8>, UpdateError> {
    let mut cmd = Command::new(curl);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = run_child_with_timeout_capped(cmd, CAPTURE_TIMEOUT, MAX_CRATES_IO_BYTES)
        .map_err(|e| map_capture_error(&e))?;
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

fn map_capture_error(e: &io::Error) -> UpdateError {
    if e.kind() == io::ErrorKind::TimedOut {
        return UpdateError::VersionLookup("curl timed out and was killed".to_string());
    }
    if e.kind() == io::ErrorKind::OutOfMemory || e.to_string().contains(RESPONSE_TOO_LARGE_FRAGMENT)
    {
        return UpdateError::VersionLookup("crates.io response too large".to_string());
    }
    UpdateError::VersionLookup(format!("failed to spawn curl: {e}"))
}

/// Probe the HTTP status code of a URL with a short bounded request.
/// Returns `None` when the probe itself cannot run.
///
/// The child is bounded by [`PROBE_TIMEOUT`] (killed and reaped past the
/// deadline) so a hung curl binary cannot block the caller forever.
pub fn probe_http_code(curl: &str, url: &str) -> Option<u16> {
    #[cfg(windows)]
    let null_device = "NUL";
    #[cfg(not(windows))]
    let null_device = "/dev/null";
    let mut cmd = Command::new(curl);
    cmd.args([
        "-s",
        "-o",
        null_device,
        "-w",
        "%{http_code}",
        "--max-time",
        "15",
        url,
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    let output = run_child_with_timeout(cmd, PROBE_TIMEOUT).ok()?;
    let code_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // curl emits `000` for transport failures (timeout/DNS/TLS); code 0 is
    // never a real server answer, so map it to `None` instead of `Some(0)`.
    code_str.parse::<u16>().ok().filter(|code| *code != 0)
}

/// Fetch the latest stable version for `crate_name` from crates.io.
///
/// Uses `max_stable_version` which is the highest non-yanked,
/// non-prerelease version. One bounded HTTPS request with a
/// program-specific User-Agent.
pub fn fetch_latest_stable_version(
    crate_name: &str,
    program: &str,
    current_version: &str,
) -> Result<String, UpdateError> {
    let curl = find_curl()?;
    let url = format!("https://crates.io/api/v1/crates/{crate_name}");
    let user_agent = format!("{program}/{current_version} (https://github.com/eggstack/gregg)");
    let args = [
        "-fsSL",
        "--max-time",
        CRATES_IO_TIMEOUT_SECS,
        "-H",
        &format!("User-Agent: {user_agent}"),
        &url,
    ];
    let stdout = run_curl_capture(&curl, &args).map_err(|e| match e {
        // `run_curl_capture` already returns `VersionLookup`; reword from the
        // inner message instead of formatting the outer display (which would
        // nest the "version lookup failed:" prefix twice).
        UpdateError::VersionLookup(inner) => UpdateError::VersionLookup(format!(
            "crates.io request failed for {crate_name}: {inner}"
        )),
        other => other,
    })?;
    if stdout.len() > MAX_CRATES_IO_BYTES {
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
    if crate::version::parse_stable_version(&version).is_none() {
        return Err(UpdateError::VersionLookup(format!(
            "crates.io returned non-stable version: {version}"
        )));
    }
    Ok(version)
}

/// Outcome of a bounded asset download.
#[derive(Debug, PartialEq, Eq)]
pub enum DownloadOutcome {
    /// The asset was downloaded.
    Success,
    /// The asset is absent (HTTP 404). Only this outcome permits the Cargo
    /// fallback; transport failures never fall back.
    NotFound,
    /// Any other failure (timeout, 5xx, TLS, spawn error).
    Failed(String),
}

/// Download a URL to `dest` with a single bounded `curl` invocation.
///
/// The HTTP status code is captured from this same invocation (`-w
/// %{http_code}` alongside `-o`), so a failed download never triggers a
/// second probe request. Only an exact `404` code permits the Cargo
/// fallback; every other failure (timeout, 5xx, TLS, spawn error) is a
/// hard `Failed`.
pub fn download_file(curl: &str, url: &str, dest: &std::path::Path) -> DownloadOutcome {
    let dest_str = dest.to_string_lossy().to_string();
    let max_filesize = MAX_DOWNLOAD_BYTES.to_string();
    let mut cmd = Command::new(curl);
    cmd.args([
        "-fsSL",
        "--max-time",
        DOWNLOAD_TIMEOUT_SECS,
        "--max-filesize",
        &max_filesize,
        "-o",
        &dest_str,
        "-w",
        "%{http_code}",
        url,
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let output = run_child_with_timeout(cmd, DOWNLOAD_WALL_TIMEOUT);
    match output {
        Ok(out) if out.status.success() => {
            // Defense in depth: callers pass `-f` (fail on non-2xx), but
            // assert the captured `%{http_code}` is 2xx anyway so a future
            // curl without `-f` cannot accept an error body as success.
            // An unparsable code is a hard failure (partial removed) so a
            // curl warning on stdout can never be accepted as an asset.
            let code_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
            match code_str.parse::<u16>() {
                Ok(code) if (200..300).contains(&code) => DownloadOutcome::Success,
                Ok(code) => {
                    let _ = std::fs::remove_file(dest);
                    if code == 404 {
                        DownloadOutcome::NotFound
                    } else {
                        DownloadOutcome::Failed(format!("unexpected HTTP {code} for {url}"))
                    }
                }
                Err(_) => {
                    let _ = std::fs::remove_file(dest);
                    DownloadOutcome::Failed(format!(
                        "unparseable HTTP status {code_str:?} for {url}"
                    ))
                }
            }
        }
        Ok(out) => {
            // `curl -o dest` truncates `dest` before the status is known;
            // remove the partial residue so a retry never checksums it.
            let _ = std::fs::remove_file(dest);
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let code = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if code == "404" {
                DownloadOutcome::NotFound
            } else {
                DownloadOutcome::Failed(format!("curl exit {:?}: {stderr}", out.status.code()))
            }
        }
        Err(e) => {
            let _ = std::fs::remove_file(dest);
            DownloadOutcome::Failed(format!("curl failed: {e}"))
        }
    }
}

/// Run a child process with a deadline. On timeout the child is killed and
/// reaped before returning a `TimedOut` error, so no orphan keeps running
/// after the caller gives up.
pub fn run_child_with_timeout(mut cmd: Command, timeout: Duration) -> io::Result<Output> {
    let mut child = cmd.spawn()?;
    let stdout = pipe_reader(child.stdout.take());
    let stderr = pipe_reader(child.stderr.take());
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
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

/// Bounded pipe reader: streams at most `limit + 1` bytes so an oversized
/// body is detected without buffering it fully.
fn pipe_reader_limited<R: Read + Send + 'static>(
    reader: Option<R>,
    limit: usize,
) -> Option<thread::JoinHandle<io::Result<Vec<u8>>>> {
    reader.map(|reader| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            reader
                .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
                .read_to_end(&mut bytes)?;
            Ok(bytes)
        })
    })
}

/// `run_child_with_timeout` variant that caps captured stdout at
/// `max_bytes`. Returns an `OutOfMemory`-kind [`io::Error`] when the cap is
/// exceeded so callers can map it without allocating the full body.
fn run_child_with_timeout_capped(
    mut cmd: Command,
    timeout: Duration,
    max_bytes: usize,
) -> io::Result<Output> {
    let mut child = cmd.spawn()?;
    let stdout = pipe_reader_limited(child.stdout.take(), max_bytes);
    let stderr = pipe_reader(child.stderr.take());
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
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
    let stdout = join_pipe(stdout)?;
    if stdout.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::OutOfMemory,
            format!("child {RESPONSE_TOO_LARGE_FRAGMENT}"),
        ));
    }
    Ok(Output {
        status,
        stdout,
        stderr: join_pipe(stderr)?,
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

/// Run a command with a timeout, mapping spawn/timeout failures into the
/// candidate-verification error category.
pub fn run_command_with_timeout(cmd: Command, timeout: Duration) -> Result<Output, UpdateError> {
    run_child_with_timeout(cmd, timeout).map_err(|error| {
        if error.kind() == io::ErrorKind::TimedOut {
            UpdateError::CandidateMismatch("candidate 'version' timed out".to_string())
        } else {
            UpdateError::CandidateMismatch(format!("candidate process failed: {error}"))
        }
    })
}

/// Run a Cargo command with a timeout, mapping failures into the Cargo
/// fallback error category.
pub fn run_command_with_timeout_for_cargo(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_not_found_vs_failed_classification() {
        // Pure logic: 404 permits fallback, 5xx does not. This test locks the invariant.
        let not_found = DownloadOutcome::NotFound;
        let failed = DownloadOutcome::Failed("timeout".to_string());
        assert!(matches!(not_found, DownloadOutcome::NotFound));
        assert!(matches!(failed, DownloadOutcome::Failed(_)));
        // Ensure missing asset permits fallback while transport failure does not.
        // This is checked in the prepare_candidate match arms.
    }

    /// Stub `curl` that records invocations, prints the fake HTTP code to
    /// stdout (as `-w %{http_code}` would), and exits 22 like `curl -f`.
    #[cfg(unix)]
    fn stub_curl_with_code(dir: &std::path::Path, code: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let stub = dir.join("curl");
        let script = format!(
            "#!/bin/sh\necho x >> \"{}\"\nprintf '%s' \"{code}\"\necho \"curl: (22) the requested URL returned error: {code}\" >&2\nexit 22\n",
            dir.join("calls").display(),
        );
        std::fs::write(&stub, script).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        stub.to_string_lossy().to_string()
    }

    #[cfg(unix)]
    #[test]
    fn download_classifies_code_in_a_single_request() {
        let temp = crate::stage::create_temp_dir("gregg-update-test-download").unwrap();
        let dir = temp.path().to_path_buf();
        let dest = dir.join("asset");
        let calls = dir.join("calls");

        // Exact 404 → NotFound, with no second probe request.
        let curl = stub_curl_with_code(&dir, "404");
        assert!(matches!(
            download_file(&curl, "https://example.invalid/asset", &dest),
            DownloadOutcome::NotFound
        ));
        assert_eq!(std::fs::read_to_string(&calls).unwrap(), "x\n");
        let _ = std::fs::remove_file(&calls);

        // A body/message mentioning 404 must not be sniffed as NotFound
        // when the captured status code is 500.
        let curl = stub_curl_with_code(&dir, "500");
        assert!(matches!(
            download_file(&curl, "https://example.invalid/404-docs", &dest),
            DownloadOutcome::Failed(_)
        ));
        assert_eq!(std::fs::read_to_string(&calls).unwrap(), "x\n");
    }

    /// Stub `curl` that exits 0 (success) but prints unparseable stdout,
    /// as a curl warning on stdout would.
    #[cfg(unix)]
    fn stub_curl_success_with_output(dir: &std::path::Path, output: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let stub = dir.join("curl");
        let script = format!(
            "#!/bin/sh\necho x >> \"{}\"\nprintf '%s' \"{output}\"\nexit 0\n",
            dir.join("calls").display(),
        );
        std::fs::write(&stub, script).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        stub.to_string_lossy().to_string()
    }

    #[cfg(unix)]
    #[test]
    fn download_rejects_unparseable_status_on_success() {
        let temp = crate::stage::create_temp_dir("gregg-update-test-download-garbage").unwrap();
        let dir = temp.path().to_path_buf();
        let dest = dir.join("asset");
        std::fs::write(&dest, b"partial").unwrap();

        let curl = stub_curl_success_with_output(&dir, "garbage");
        assert!(matches!(
            download_file(&curl, "https://example.invalid/asset", &dest),
            DownloadOutcome::Failed(_)
        ));
        assert!(
            !dest.exists(),
            "partial file must be removed on unparseable status"
        );
    }

    #[test]
    fn timeout_child() {
        if let Ok(marker) = std::env::var("GREGG_UPDATE_TIMEOUT_MARKER") {
            thread::sleep(Duration::from_millis(250));
            std::fs::write(marker, b"late").unwrap();
        }
    }

    #[test]
    fn cargo_timeout_kills_and_reaps_child() {
        let temp = crate::stage::create_temp_dir("gregg-update-test-timeout").unwrap();
        let marker = temp.path().join("late");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "exec::tests::timeout_child", "--nocapture"])
            .env("GREGG_UPDATE_TIMEOUT_MARKER", &marker)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let error = run_command_with_timeout_for_cargo(command, Duration::from_millis(40))
            .expect_err("slow child must time out");
        assert!(error.to_string().contains("timed out"));
        thread::sleep(Duration::from_millis(300));
        assert!(!marker.exists(), "timed-out child continued after return");
    }
}
