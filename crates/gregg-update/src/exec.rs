//! Bounded external-process execution and network fetch helpers.
//!
//! All subprocess I/O is bounded: `curl` requests carry `--max-time`,
//! spawned children are killed and reaped on timeout, and every pipe drain is
//! given its own post-exit settle bound so a descendant holding an inherited
//! write end cannot park a caller forever. No shell is invoked; no `sudo` is
//! invoked.

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

/// Maximum bytes accepted on a captured stderr pipe (64 KiB). Stderr is
/// never part of the version/identity decision; the cap only prevents an
/// unbounded pipe buffer from a verbose child.
pub const MAX_STDERR_BYTES: usize = 64 * 1024;

/// Maximum bytes accepted on a download child's stdout pipe (1 KiB).
///
/// The asset body is written to a file with `-o`, so stdout carries only the
/// `-w %{http_code}` status line — three digits. The cap exists so a mirror
/// that answers with an unbounded body on the wrong channel fails the download
/// rather than filling memory.
pub const MAX_DOWNLOAD_STATUS_BYTES: usize = 1024;

/// Maximum bytes accepted from a staged candidate `version` probe on each
/// of stdout/stderr (16 KiB). A legitimate identity line is under 100
/// bytes; anything larger is rejected without buffering it fully.
pub const MAX_CANDIDATE_OUTPUT_BYTES: usize = 16 * 1024;

/// Maximum `cargo install --list` stdout accepted (256 KiB). The listing
/// scales with the number of installed packages, not with the queried
/// package, so it is bounded like crates.io metadata.
pub const MAX_CARGO_LIST_BYTES: usize = 256 * 1024;

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

/// Wall-clock bound for Cargo-owned uninstall handoff (`cargo uninstall`).
/// Removal is metadata-only and returns in seconds; the 600s build bound
/// must not apply here.
pub const CARGO_UNINSTALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Wall-clock bound for a `curl`/`cargo` `--version` discovery probe.
///
/// Without this, an earlier-`PATH` shim that never exits blocks
/// `resolve_plan`/`prepare_candidate` forever. Kept in line with the other
/// discovery-scale bounds; a real `curl`/`cargo --version` returns in
/// milliseconds.
pub const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Locate `curl` in `PATH`.
///
/// The probe is bounded by [`DISCOVERY_TIMEOUT`] and killed/reaped past the
/// deadline, like every other external invocation in this crate.
pub fn find_curl() -> Result<String, UpdateError> {
    discover_tool(
        &["curl", "curl.exe"],
        UpdateError::CurlMissing("curl not found in PATH".to_string()),
    )
}

/// Locate `cargo` in `PATH`.
///
/// The probe is bounded by [`DISCOVERY_TIMEOUT`] and killed/reaped past the
/// deadline, like every other external invocation in this crate.
pub fn find_cargo() -> Result<String, UpdateError> {
    discover_tool(
        &["cargo", "cargo.exe"],
        UpdateError::CargoMissing("cargo not found in PATH".to_string()),
    )
}

/// Run each candidate's `--version` under the shared bounded child runner and
/// return the first one that exits successfully.
fn discover_tool(candidates: &[&str], missing: UpdateError) -> Result<String, UpdateError> {
    for candidate in candidates {
        let mut cmd = Command::new(candidate);
        cmd.arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if run_child_with_timeout(cmd, DISCOVERY_TIMEOUT)
            .is_ok_and(|status| status.status.success())
        {
            return Ok((*candidate).to_string());
        }
    }
    Err(missing)
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
/// Currently unused by the production single-request [`download_file`]
/// classification (which captures `%{http_code}` inline); retained for
/// manual debugging and shell-parity reference.
///
/// The child is bounded by [`PROBE_TIMEOUT`] (killed and reaped past the
/// deadline) so a hung curl binary cannot block the caller forever.
/// Uses the same `-fsSL --max-time` contract as [`download_file`] so the
/// flags cannot drift silently.
#[allow(dead_code)]
pub fn probe_http_code(curl: &str, url: &str) -> Option<u16> {
    #[cfg(windows)]
    let null_device = "NUL";
    #[cfg(not(windows))]
    let null_device = "/dev/null";
    let mut cmd = Command::new(curl);
    // `--proto '=https'` pins production HTTPS fetches to TLS only;
    // plain-http fixtures (local deterministic tests) keep `http,https`
    // so the same helper stays testable without weakening production.
    if url.starts_with("https://") {
        cmd.args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "-fsSL",
            "-o",
            null_device,
            "-w",
            "%{http_code}",
            "--max-time",
            "15",
            url,
        ]);
    } else {
        cmd.args([
            "-fsSL",
            "-o",
            null_device,
            "-w",
            "%{http_code}",
            "--max-time",
            "15",
            url,
        ]);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
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
///
/// Plan 126 experiment closed RETAIN CURL: the external-`curl` transport
/// below is the production path. Response parsing lives in the
/// transport-neutral [`parse_stable_version_response`] helper so it stays
/// unit-tested without network access.
pub fn fetch_latest_stable_version(
    crate_name: &str,
    program: &str,
    current_version: &str,
) -> Result<String, UpdateError> {
    if !is_valid_crate_name(crate_name) {
        return Err(UpdateError::VersionLookup(format!(
            "invalid crate name: {crate_name:?}"
        )));
    }
    let curl = find_curl()?;
    let url = format!("https://crates.io/api/v1/crates/{crate_name}");
    let user_agent = format!("{program}/{current_version} (https://github.com/eggstack/gregg)");
    let args = [
        "--proto",
        "=https",
        "--tlsv1.2",
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
    parse_stable_version_response(&stdout)
}

/// Whether `crate_name` is safe to interpolate into the crates.io URL.
/// crates.io names are `[A-Za-z0-9_-]+`; anything else is rejected before
/// it reaches the command line so path separators or shell metacharacters
/// can never alter the request target (no shell is invoked regardless).
fn is_valid_crate_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Parse and validate a crates.io metadata body.
///
/// Transport-neutral extraction of the inline logic above (kept from the
/// Plan 126 experiment): unit-tested without network access.
fn parse_stable_version_response(body: &[u8]) -> Result<String, UpdateError> {
    if body.len() > MAX_CRATES_IO_BYTES {
        return Err(UpdateError::VersionLookup(
            "crates.io response too large".to_string(),
        ));
    }
    let json: serde_json::Value = serde_json::from_slice(body)
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
///
/// No custom `User-Agent` is sent here: unlike the crates.io API (which
/// requires one), GitHub release downloads need no UA, and the installer
/// shell path sets its own `program/installer` UA for mirror accounting.
/// Keeping this transport UA-free avoids threading caller identity through
/// every test stub for no server requirement.
pub fn download_file(curl: &str, url: &str, dest: &std::path::Path) -> DownloadOutcome {
    let max_filesize = MAX_DOWNLOAD_BYTES.to_string();
    let mut cmd = Command::new(curl);
    // `--proto '=https'` pins production HTTPS fetches to TLS only;
    // plain-http fixtures (local deterministic tests) keep `http,https`
    // so the same helper stays testable without weakening production.
    if url.starts_with("https://") {
        cmd.args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "-fsSL",
            "--max-time",
            DOWNLOAD_TIMEOUT_SECS,
            "--max-filesize",
            &max_filesize,
        ]);
    } else {
        cmd.args([
            "-fsSL",
            "--max-time",
            DOWNLOAD_TIMEOUT_SECS,
            "--max-filesize",
            &max_filesize,
        ]);
    }
    // `dest` is passed as a path, never as a lossy string: staging is built
    // under `env::temp_dir()`, and a `TMPDIR` carrying non-UTF-8 bytes would
    // otherwise name a different (normally nonexistent) file and surface as a
    // misleading download failure.
    cmd.arg("-o")
        .arg(dest)
        .arg("-w")
        .arg("%{http_code}")
        .arg(url);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    // Capped like every other child: the asset body goes to `-o <dest>`, so
    // these pipes carry only the `%{http_code}` line and curl's diagnostics,
    // and a mirror that floods them fails the download instead of growing this
    // process's heap.
    let output = run_child_with_timeout_capped_both(
        cmd,
        DOWNLOAD_WALL_TIMEOUT,
        MAX_DOWNLOAD_STATUS_BYTES,
        MAX_STDERR_BYTES,
    );
    match output {
        Ok(out) if out.status.success() => {
            // Defense in depth: callers pass `-f` (fail on non-2xx), but
            // assert the captured `%{http_code}` is 2xx anyway so a future
            // curl without `-f` cannot accept an error body as success.
            // An unparsable code is a hard failure (partial removed) so a
            // curl warning on stdout can never be accepted as an asset.
            let code_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
            match code_str.parse::<u16>() {
                Ok(code) if (200..300).contains(&code) => downloaded_asset_within_cap(dest),
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

/// Confirm a 2xx download actually landed within [`MAX_DOWNLOAD_BYTES`].
///
/// `--max-filesize` is the primary guard, but it is a curl-side
/// best-effort: an older curl, a chunked response, or a regression that drops
/// the flag would otherwise feed an oversized file to the checksum, chmod,
/// and exec steps. This is the metadata-path equivalent of the
/// `take(MAX+1)` + `body.len()` re-check used for the metadata capture, and
/// it is the last chance to reject before the file becomes a candidate.
fn downloaded_asset_within_cap(dest: &std::path::Path) -> DownloadOutcome {
    match std::fs::metadata(dest) {
        Ok(meta) if meta.len() <= MAX_DOWNLOAD_BYTES => DownloadOutcome::Success,
        Ok(meta) => {
            let _ = std::fs::remove_file(dest);
            DownloadOutcome::Failed(format!(
                "downloaded asset is {} bytes, exceeding the {MAX_DOWNLOAD_BYTES} byte maximum",
                meta.len()
            ))
        }
        Err(error) => {
            let _ = std::fs::remove_file(dest);
            DownloadOutcome::Failed(format!("downloaded asset is unreadable: {error}"))
        }
    }
}

/// Run a child process with a deadline. On timeout the child is killed and
/// reaped before returning a `TimedOut` error, so no orphan keeps running
/// after the caller gives up.
///
/// The deadline bounds the child's *lifetime*, never the pipe drains that
/// follow it: see [`settle_pipe`]. An unbounded join there would
/// turn this from a bounded call into a silent hang whenever a descendant
/// inherited a write end.
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
                let _ = settle_pipe(stdout);
                let _ = settle_pipe(stderr);
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
        stdout: settle_pipe(stdout)?,
        stderr: settle_pipe(stderr)?,
    })
}

/// Budget a pipe reader gets after the child's wait result to hand over its
/// bytes, per stream.
///
/// One fixed, non-configurable bound, never restarted per stream or per wake.
/// It exists because the child's exit does not close the pipe: a descendant
/// that inherited the write end can hold it open indefinitely, and gregg-update
/// neither owns nor kills descendants. Without this, waiting for the reader is
/// an unbounded wait on a writer that may never close.
const POST_EXIT_DRAIN_SETTLE: Duration = Duration::from_millis(250);

/// One child pipe being drained on its own thread.
///
/// The bytes arrive over a channel rather than through [`thread::JoinHandle`],
/// because a join cannot be given a deadline. Once the budget expires the
/// reader is abandoned: the `JoinHandle` was dropped when the `PipeReader` was
/// built, so the thread detaches and cannot keep this call parked. Its read end
/// stays open until it finishes, which is the price of not killing somebody
/// else's process — and the only alternative would be an unbounded wait.
struct PipeReader {
    result: std::sync::mpsc::Receiver<io::Result<Vec<u8>>>,
}

impl PipeReader {
    /// Wait up to `budget` for this pipe's bytes.
    ///
    /// An expired budget is reported as an error rather than as short output:
    /// an incomplete capture that looks complete would let a caller act on a
    /// version line or status code it never actually received.
    fn recv_within(self, budget: Duration) -> io::Result<Vec<u8>> {
        match self.result.recv_timeout(budget) {
            Ok(outcome) => outcome,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "child output did not settle within the post-exit bound; \
                 an inherited writer is holding the pipe open",
            )),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::other(
                "child output reader ended without producing a result",
            )),
        }
    }
}

/// Wait up to [`POST_EXIT_DRAIN_SETTLE`] for one pipe's bytes.
///
/// Every path that used to join a reader unboundedly goes through here. An
/// unpiped stream settles instantly; a piped one gets its own single budget,
/// which is what closes the wait at `2 * POST_EXIT_DRAIN_SETTLE` for a child
/// with both streams inherited by a descendant.
fn settle_pipe(reader: Option<PipeReader>) -> io::Result<Vec<u8>> {
    match reader {
        Some(reader) => reader.recv_within(POST_EXIT_DRAIN_SETTLE),
        None => Ok(Vec::new()),
    }
}

fn pipe_reader<R: Read + Send + 'static>(reader: Option<R>) -> Option<PipeReader> {
    reader.map(|mut reader| {
        let (sender, result) = std::sync::mpsc::sync_channel(1);
        // The handle is dropped immediately: the thread detaches and this
        // process never blocks on it. `recv_timeout` is the only wait.
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let outcome = reader.read_to_end(&mut bytes).map(|_| bytes);
            // A failed send means the caller abandoned the drain; the bytes go
            // with the thread.
            let _ = sender.send(outcome);
        });
        PipeReader { result }
    })
}

/// Bounded pipe reader: streams at most `limit + 1` bytes so an oversized
/// body is detected without buffering it fully.
fn pipe_reader_limited<R: Read + Send + 'static>(
    reader: Option<R>,
    limit: usize,
) -> Option<PipeReader> {
    reader.map(|reader| {
        let (sender, result) = std::sync::mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let outcome = reader
                .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = sender.send(outcome);
        });
        PipeReader { result }
    })
}

/// `run_child_with_timeout` variant that caps captured stdout at
/// `max_stdout_bytes` and stderr at `max_stderr_bytes`. Returns an
/// `OutOfMemory`-kind [`io::Error`] when either cap is exceeded so callers
/// can map it without allocating the full body.
pub(crate) fn run_child_with_timeout_capped(
    cmd: Command,
    timeout: Duration,
    max_stdout_bytes: usize,
) -> io::Result<Output> {
    run_child_with_timeout_capped_both(cmd, timeout, max_stdout_bytes, MAX_STDERR_BYTES)
}

/// `run_child_with_timeout` variant with independent stdout/stderr caps.
/// Both pipes stream at most `limit + 1` bytes so an oversized body is
/// detected without buffering it fully.
pub(crate) fn run_child_with_timeout_capped_both(
    mut cmd: Command,
    timeout: Duration,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
) -> io::Result<Output> {
    let mut child = cmd.spawn()?;
    let stdout = pipe_reader_limited(child.stdout.take(), max_stdout_bytes);
    let stderr = pipe_reader_limited(child.stderr.take(), max_stderr_bytes);
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = settle_pipe(stdout);
                let _ = settle_pipe(stderr);
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "child process timed out and was killed",
                ));
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    };
    let stdout = settle_pipe(stdout)?;
    if stdout.len() > max_stdout_bytes {
        return Err(io::Error::new(
            io::ErrorKind::OutOfMemory,
            format!("child {RESPONSE_TOO_LARGE_FRAGMENT}"),
        ));
    }
    let stderr = settle_pipe(stderr)?;
    if stderr.len() > max_stderr_bytes {
        return Err(io::Error::new(
            io::ErrorKind::OutOfMemory,
            format!("child {RESPONSE_TOO_LARGE_FRAGMENT}"),
        ));
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Run a command with a timeout, mapping spawn/timeout failures into the
/// candidate-verification error category.
///
/// Both pipes are capped at [`MAX_CANDIDATE_OUTPUT_BYTES`] (16 KiB): a
/// legitimate `<program> X.Y.Z` identity line is under 100 bytes, so a
/// larger probe output is rejected without buffering it fully.
pub fn run_command_with_timeout(cmd: Command, timeout: Duration) -> Result<Output, UpdateError> {
    run_child_with_timeout_capped_both(
        cmd,
        timeout,
        MAX_CANDIDATE_OUTPUT_BYTES,
        MAX_CANDIDATE_OUTPUT_BYTES,
    )
    .map_err(|error| {
        if error.kind() == io::ErrorKind::TimedOut {
            UpdateError::CandidateMismatch("candidate 'version' timed out".to_string())
        } else if error.kind() == io::ErrorKind::OutOfMemory {
            UpdateError::CandidateMismatch("candidate 'version' output too large".to_string())
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
            UpdateError::CargoFallback(format!(
                "cargo install timed out after {}s",
                timeout.as_secs()
            ))
        } else {
            UpdateError::CargoFallback(format!("cargo process failed: {error}"))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_version_response_parses_valid() {
        let body = br#"{"crate":{"max_stable_version":"1.2.3"}}"#;
        assert_eq!(parse_stable_version_response(body).unwrap(), "1.2.3");
    }

    #[test]
    fn stable_version_response_rejects_missing_field() {
        let body = br#"{"crate":{}}"#;
        let error = parse_stable_version_response(body).expect_err("must fail");
        assert!(
            error.to_string().contains("max_stable_version"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn stable_version_response_rejects_empty() {
        let body = br#"{"crate":{"max_stable_version":""}}"#;
        let error = parse_stable_version_response(body).expect_err("must fail");
        assert!(
            error.to_string().contains("empty"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn stable_version_response_rejects_non_stable() {
        let body = br#"{"crate":{"max_stable_version":"2.0.0-beta.1"}}"#;
        let error = parse_stable_version_response(body).expect_err("must fail");
        assert!(
            error.to_string().contains("non-stable"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn stable_version_response_rejects_invalid_json() {
        let error = parse_stable_version_response(b"not json").expect_err("must fail");
        assert!(
            error.to_string().contains("parse failed"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn stable_version_response_rejects_oversized() {
        let mut body = b"{\"crate\":{\"max_stable_version\":\"1.2.3\",\"pad\":\"".to_vec();
        body.extend(std::iter::repeat_n(b'x', MAX_CRATES_IO_BYTES));
        body.extend_from_slice(b"\"}}");
        let error = parse_stable_version_response(&body).expect_err("must fail");
        assert!(
            error.to_string().contains("too large"),
            "unexpected error: {error}"
        );
    }

    /// Bounded attempts for a stub `curl` that never got started.
    ///
    /// A loaded CI runner can refuse a `fork` with `EAGAIN`.
    /// `download_file` reports that as a hard `Failed` (it cannot know the
    /// difference between "the server said no" and "the child never ran"),
    /// which is honest, and it is also indistinguishable from a real
    /// classification result unless the caller looks for it.
    #[cfg(unix)]
    const SPAWN_RETRY_ATTEMPTS: u32 = 4;

    /// Backoff between spawn retries. Short, because the contention being
    /// retried is momentary; long enough not to spin a loaded runner harder.
    #[cfg(unix)]
    const SPAWN_RETRY_DELAY: Duration = Duration::from_millis(50);

    /// Invoke `download_file` and require that the stub child actually ran.
    ///
    /// Every stub in this module appends a line to `calls` as its first act,
    /// so a missing `calls` file is direct evidence that no child was ever
    /// started — as opposed to a child that ran and was classified. That
    /// distinction is what lets a download test assert a *classification*
    /// rather than the far wider `DownloadOutcome::Failed(_)`, which a
    /// `fork` failure satisfies just as happily as a genuine HTTP 500.
    ///
    /// "The child started" is therefore proven by the child's own side effect
    /// rather than by parsing a production error string, so this helper cannot
    /// drift away from what `download_file` does. A spawn failure is retried
    /// up to [`SPAWN_RETRY_ATTEMPTS`]; exhausting them is a named assertion
    /// about the spawn, carrying the reason `download_file` reported.
    #[cfg(unix)]
    fn download_file_with_started_stub(
        curl: &str,
        url: &str,
        dest: &std::path::Path,
        calls: &std::path::Path,
    ) -> DownloadOutcome {
        let mut last = String::new();
        for attempt in 1..=SPAWN_RETRY_ATTEMPTS {
            let _ = std::fs::remove_file(calls);
            let outcome = download_file(curl, url, dest);
            if calls.exists() {
                return outcome;
            }
            last = match &outcome {
                DownloadOutcome::Failed(reason) => reason.clone(),
                other => format!("{other:?}"),
            };
            eprintln!("attempt {attempt}/{SPAWN_RETRY_ATTEMPTS} started no stub: {last}; retrying");
            thread::sleep(SPAWN_RETRY_DELAY);
        }
        panic!(
            "no stub child was started in {SPAWN_RETRY_ATTEMPTS} attempts \
             (the `calls` log was never written); the last reported failure was {last:?}. \
             That is a spawn failure in the test environment, not an HTTP \
             classification result."
        );
    }

    /// Stub `curl` that writes a body larger than `MAX_DOWNLOAD_BYTES` while
    /// reporting HTTP 200, simulating a curl that ignored `--max-filesize`.
    ///
    /// The `calls` log line is written first, exactly as the other stubs do,
    /// so every stub-based download test can tell "curl never started" from
    /// "curl started and was classified" with the same evidence.
    #[cfg(unix)]
    fn stub_curl_oversized_asset(dir: &std::path::Path) -> String {
        use std::os::unix::fs::PermissionsExt;
        let stub = dir.join("curl");
        // Discover curl's `-o` target so the stub writes where the caller
        // expects the asset, then produce one MiB block more than the cap.
        let script = format!(
            concat!(
                "#!/bin/sh\n",
                "echo x >> \"{}\"\n",
                "printf '200'\n",
                "out=\"\"\n",
                "while [ \"$#\" -gt 0 ]; do\n",
                "  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; fi\n",
                "  shift\n",
                "done\n",
                "dd if=/dev/zero of=\"$out\" bs=1048576 count=65 2>/dev/null\n",
                "exit 0\n",
            ),
            dir.join("calls").display(),
        );
        let _ = std::fs::write(&stub, script);
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        stub.to_string_lossy().to_string()
    }

    /// Stub `curl` that floods the *status* pipe instead of the asset file: a
    /// server answering on the wrong channel, with a body the caller never asked
    /// for. The asset itself is written correctly, so only the cap on that pipe
    /// can catch it.
    #[cfg(unix)]
    fn stub_curl_flooding_status_pipe(dir: &std::path::Path) -> String {
        use std::os::unix::fs::PermissionsExt;
        let stub = dir.join("curl");
        let script = format!(
            concat!(
                "#!/bin/sh\n",
                "echo x >> \"{}\"\n",
                "out=\"\"\n",
                "while [ \"$#\" -gt 0 ]; do\n",
                "  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; fi\n",
                "  shift\n",
                "done\n",
                "printf 'asset' > \"$out\"\n",
                // No `-w` status line at all: just an unbounded stream on stdout.
                "dd if=/dev/zero bs=1024 count=512 2>/dev/null\n",
                "exit 0\n",
            ),
            dir.join("calls").display(),
        );
        let _ = std::fs::write(&stub, script);
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        stub.to_string_lossy().to_string()
    }

    /// A child's pipes are bounded like any other child: an unbounded stdout is
    /// a failed download, never a buffer this process grows to the server's
    /// liking. `download_file` only ever reads a `%{http_code}` line from it.
    #[cfg(unix)]
    #[test]
    fn download_fails_when_the_status_pipe_is_flooded() {
        let temp = crate::stage::create_temp_dir("gregg-update-test-statusflood").unwrap();
        let dir = temp.path().to_path_buf();
        let dest = dir.join("asset");
        let calls = dir.join("calls");
        let curl = stub_curl_flooding_status_pipe(&dir);
        let outcome =
            download_file_with_started_stub(&curl, "https://example.invalid/asset", &dest, &calls);
        let DownloadOutcome::Failed(reason) = outcome else {
            panic!("a flooded status pipe must not be accepted: {outcome:?}");
        };
        assert!(
            reason.contains(RESPONSE_TOO_LARGE_FRAGMENT),
            "the download must fail for exceeding the cap, \
             not for some unrelated reason: {reason}"
        );
        assert!(!dest.exists(), "a failed download must not leave the asset");
    }

    #[cfg(unix)]
    #[test]
    fn download_rejects_an_oversized_asset_even_on_http_200() {
        let temp = crate::stage::create_temp_dir("gregg-update-test-oversize").unwrap();
        let dir = temp.path().to_path_buf();
        let dest = dir.join("asset");
        let calls = dir.join("calls");
        let curl = stub_curl_oversized_asset(&dir);
        // Without the started-stub check, a child that never spawned would
        // also report `Failed` and also leave no `dest` — both assertions
        // below would pass while the cap was never exercised at all.
        let outcome =
            download_file_with_started_stub(&curl, "https://example.invalid/asset", &dest, &calls);
        let DownloadOutcome::Failed(reason) = outcome else {
            panic!("an oversized asset must never become a candidate: {outcome:?}");
        };
        assert!(
            reason.contains(&format!("exceeding the {MAX_DOWNLOAD_BYTES} byte maximum")),
            "an oversized asset must be rejected for exceeding the cap, \
             not for some unrelated reason: {reason}"
        );
        assert!(!dest.exists(), "oversized partial must be removed");
    }

    #[cfg(unix)]
    #[test]
    fn discovery_probe_is_bounded_and_kills_a_hanging_binary() {
        // A shim earlier on PATH that never exits must not block discovery
        // forever; the probe is killed and reaped past DISCOVERY_TIMEOUT.
        use std::os::unix::fs::PermissionsExt;
        let temp = crate::stage::create_temp_dir("gregg-update-test-discovery").unwrap();
        let dir = temp.path().to_path_buf();
        let stub = dir.join("curl");
        std::fs::write(&stub, "#!/bin/sh\nsleep 120\n").unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = std::time::Instant::now();
        let error = discover_tool(
            &[
                stub.to_str().expect("utf-8 stub path"),
                "curl-absent-for-sure",
            ],
            UpdateError::CurlMissing("curl not found in PATH".to_string()),
        )
        .expect_err("a hanging probe must not report success");
        assert!(matches!(error, UpdateError::CurlMissing(_)));
        assert!(
            started.elapsed() < DISCOVERY_TIMEOUT + Duration::from_secs(10),
            "discovery took {:?}, which is not bounded",
            started.elapsed()
        );
    }

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
        let outcome =
            download_file_with_started_stub(&curl, "https://example.invalid/asset", &dest, &calls);
        assert_eq!(outcome, DownloadOutcome::NotFound);
        assert_eq!(std::fs::read_to_string(&calls).unwrap(), "x\n");

        // A 500 requested from a URL that mentions 404 must still be a hard
        // failure, never a fallback-permitting NotFound.
        //
        // This asserts the specific classification, not `Failed(_)`: a
        // `Failed` is also what a child that never started produces, so the
        // wide match could be satisfied by a runner that could not fork at
        // all. Production always passes `-f`, so a 5xx exits non-zero and is
        // classified through the failed-run arm.
        let curl = stub_curl_with_code(&dir, "500");
        let outcome = download_file_with_started_stub(
            &curl,
            "https://example.invalid/404-docs",
            &dest,
            &calls,
        );
        let DownloadOutcome::Failed(reason) = outcome else {
            panic!(
                "a 500 must be a hard failure, never a fallback-permitting NotFound: {outcome:?}"
            );
        };
        assert!(
            reason.contains("curl exit"),
            "a 500 must be reported from the failed curl run, never as a status \
             the fallback could use: {reason}"
        );
        assert_eq!(std::fs::read_to_string(&calls).unwrap(), "x\n");

        // The defense in depth the status capture exists for: a curl that did
        // *not* fail on the status and exited 0 on a 5xx. The recorded code
        // decides here, not the body's mention of 404, and the error body
        // never becomes a candidate.
        let curl = stub_curl_success_with_code(&dir, "500");
        let outcome = download_file_with_started_stub(
            &curl,
            "https://example.invalid/404-docs",
            &dest,
            &calls,
        );
        let DownloadOutcome::Failed(reason) = outcome else {
            panic!("a recorded 500 on an exit-0 curl must never be a candidate: {outcome:?}");
        };
        assert!(
            reason.contains("unexpected HTTP 500"),
            "the recorded code, not the body, must decide: {reason}"
        );
        assert!(!dest.exists(), "an error body must never land as an asset");
        assert_eq!(std::fs::read_to_string(&calls).unwrap(), "x\n");
    }

    /// Stub `curl` that exits 0 (as a curl without `-f` would) while reporting
    /// `code`, and whose captured message mentions a `404`.
    #[cfg(unix)]
    fn stub_curl_success_with_code(dir: &std::path::Path, code: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let stub = dir.join("curl");
        let script = format!(
            "#!/bin/sh\necho x >> \"{}\"\nprintf '%s' \"{code}\"\necho 'error page, see /404-docs' >&2\nexit 0\n",
            dir.join("calls").display(),
        );
        std::fs::write(&stub, script).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        stub.to_string_lossy().to_string()
    }

    /// A stub that cannot be spawned at all is the one case `download_file`
    /// cannot classify, and the reason [`download_file_with_started_stub`]
    /// exists. This locks that such a child is reported as a *named spawn
    /// failure* carrying the underlying error, rather than as a bare `ENOENT`
    /// from reading a `calls` file the stub never had the chance to write.
    #[cfg(unix)]
    #[test]
    fn a_stub_that_never_started_is_reported_as_a_spawn_failure() {
        let temp = crate::stage::create_temp_dir("gregg-update-test-nospawn").unwrap();
        let dir = temp.path().to_path_buf();
        let dest = dir.join("asset");
        let calls = dir.join("calls");
        let absent = dir.join("curl-never-created");
        let absent_str = absent.to_str().expect("utf-8 stub path").to_string();
        let url = "https://example.invalid/asset";
        assert!(
            !absent.exists(),
            "the fixture must name a curl that is absent"
        );

        // The reason production reports for a child that never started,
        // taken from production itself so this asserts no fixed strerror.
        let DownloadOutcome::Failed(reason) = download_file(&absent_str, url, &dest) else {
            panic!("a curl that does not exist can only fail to spawn");
        };

        let reported = std::panic::catch_unwind(|| {
            download_file_with_started_stub(&absent_str, url, &dest, &calls)
        })
        .expect_err("a stub that cannot be spawned must not report a classification");
        let reported = reported
            .downcast_ref::<String>()
            .expect("the helper panics with a formatted message")
            .clone();

        assert!(
            reported.contains("spawn failure in the test environment"),
            "the failure must name the spawn it is about, not an unrelated file: {reported}"
        );
        assert!(
            reported.contains(&reason),
            "the failure must carry the spawn error production reported: {reported}"
        );
        assert!(
            reported.contains(&format!("{SPAWN_RETRY_ATTEMPTS} attempts")),
            "the failure must say how many spawn attempts it made: {reported}"
        );
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
        let calls = dir.join("calls");
        std::fs::write(&dest, b"partial").unwrap();

        let curl = stub_curl_success_with_output(&dir, "garbage");
        // The pre-seeded `dest` is removed by *both* the unparseable-status
        // arm and the spawn-failure arm, so the removal check alone cannot
        // tell them apart; the started-stub check supplies the difference.
        let outcome =
            download_file_with_started_stub(&curl, "https://example.invalid/asset", &dest, &calls);
        let DownloadOutcome::Failed(reason) = outcome else {
            panic!("an unparseable status must never become a candidate: {outcome:?}");
        };
        assert!(
            reason.contains("unparseable HTTP status"),
            "a curl that exits 0 with junk on stdout must be rejected for that \
             reason, not for some unrelated one: {reason}"
        );
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

    /// A descendant that inherited the write end keeps the pipe open after the
    /// direct child has exited, so the drain — not the child's lifetime — is
    /// what has to be bounded. Without the post-exit settle bound this call
    /// waits for a process gregg-update neither owns nor kills, which is a
    /// silent hang rather than an error.
    #[test]
    #[cfg(unix)]
    fn an_inherited_writer_cannot_hold_the_caller_open() {
        let mut command = Command::new("/bin/sh");
        // `echo` writes, then the shell exits while the backgrounded `sleep`
        // still holds both write ends open.
        command
            .args(["-c", "echo started; sleep 20 & exit 0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let started = std::time::Instant::now();
        let outcome = run_child_with_timeout(command, Duration::from_secs(10));
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "an inherited writer must not extend the call past its settle bound: {elapsed:?}"
        );
        // Reported as an error rather than as short output: an incomplete
        // capture that looked complete would let a caller act on a version line
        // or a status code it never received.
        let error = outcome.expect_err("a drain that never settled must not read as success");
        assert!(
            error.to_string().contains("inherited writer"),
            "the reason must survive to the caller: {error}"
        );
    }

    /// The ordinary case still returns the child's own output, unchanged, when
    /// nothing inherits the write ends. The bound must not cost a byte.
    #[test]
    #[cfg(unix)]
    fn a_child_that_closes_its_own_pipes_still_reports_its_output() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "printf 'v1.2.3\\n'; printf 'warning\\n' >&2"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = run_child_with_timeout(command, Duration::from_secs(10))
            .expect("a settled capture succeeds");
        assert_eq!(output.stdout, b"v1.2.3\n");
        assert_eq!(output.stderr, b"warning\n");
        assert!(output.status.success());
    }

    // Plan 126 closed RETAIN CURL and kept these end-to-end fixtures for
    // the retained external-curl transport: they drive the real `curl`
    // binary against local deterministic servers and lock the redirect,
    // exact-final-404, hard-failure, and metadata-capture contract that
    // stub-`curl`-script tests cannot prove (those fake curl's output).
    // Skipped when curl is absent.
    #[cfg(test)]
    mod curl_baseline {
        use super::*;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream};

        fn have_curl() -> Option<String> {
            find_curl().ok()
        }

        /// Proxy environment keys honored by `curl`.
        const PROXY_ENV_KEYS: [&str; 8] = [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
        ];

        /// Serializes proxy-environment sanitizing across baseline tests.
        /// Tests share one process, so clearing ambient proxy variables for
        /// the `curl` child requires a static lock; the guard restores every
        /// key on drop.
        static PROXY_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

        /// Cleared proxy environment held for one baseline test.
        struct ProxyEnvGuard {
            _lock: std::sync::MutexGuard<'static, ()>,
            saved: Vec<(&'static str, Option<String>)>,
        }

        impl Drop for ProxyEnvGuard {
            fn drop(&mut self) {
                for (key, value) in std::mem::take(&mut self.saved) {
                    match value {
                        Some(previous) => {
                            std::env::set_var(key, previous);
                        }
                        None => {
                            std::env::remove_var(key);
                        }
                    }
                }
            }
        }

        /// Clear ambient proxy variables so the `curl` child always talks to
        /// the fixture directly. Hold the guard for the whole test.
        fn lock_proxy_env() -> ProxyEnvGuard {
            let lock = PROXY_ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let saved = PROXY_ENV_KEYS
                .iter()
                .map(|key| (*key, std::env::var(key).ok()))
                .collect();
            for key in PROXY_ENV_KEYS {
                std::env::remove_var(key);
            }
            ProxyEnvGuard { _lock: lock, saved }
        }

        /// Run `serve` on a background thread with its own current-thread
        /// runtime and return the bound loopback port. Fixtures must live
        /// off the test thread: `download_file`/`run_curl_capture` block
        /// their caller, which would starve a same-thread `#[tokio::test]`
        /// executor holding the only server task.
        fn spawn_server<F, Fut>(serve: F) -> u16
        where
            F: FnOnce(TcpListener) -> Fut + Send + 'static,
            Fut: std::future::Future<Output = ()> + Send + 'static,
        {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async {
                    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let port = listener.local_addr().unwrap().port();
                    tx.send(port).unwrap();
                    serve(listener).await;
                    std::future::pending::<()>().await;
                });
            });
            rx.recv().unwrap()
        }

        /// Serve one canned `response` on a background thread; return its port.
        fn spawn_response(response: Vec<u8>) -> u16 {
            spawn_server(|listener| serve_once(listener, response))
        }

        /// Serve one canned `response` and report through `connected` whether
        /// a client actually arrived.
        ///
        /// The flag is the fixture's own evidence that a child ran, which is
        /// what lets a baseline assert a *classification* instead of the far
        /// wider `DownloadOutcome::Failed(_)`: a `curl` that could not be
        /// spawned reports the same `Failed` without ever reaching the server.
        /// The child exits only after the request completes, so by the time
        /// `download_file` returns the flag is already settled.
        fn spawn_recording_response(response: Vec<u8>, connected: Arc<AtomicBool>) -> u16 {
            spawn_server(move |listener| async move {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                connected.store(true, Ordering::SeqCst);
                respond(stream, response).await;
            })
        }

        /// Serve one connection: read the head, write `response`, close.
        async fn serve_once(listener: TcpListener, response: Vec<u8>) {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            respond(stream, response).await;
        }

        /// Read one request head off an accepted connection, answer it, close.
        async fn respond(mut stream: TcpStream, response: Vec<u8>) {
            let mut buf = [0u8; 4096];
            let mut total = 0;
            while let Ok(n) = stream.read(&mut buf[total..]).await {
                if n == 0 || total + n >= buf.len() {
                    break;
                }
                total += n;
                if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = stream.write_all(&response).await;
        }

        fn static_response(status: &str, body: &[u8]) -> Vec<u8> {
            let mut out = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .into_bytes();
            out.extend_from_slice(body);
            out
        }

        #[test]
        fn baseline_download_direct_200() {
            let _proxy_guard = lock_proxy_env();
            let Some(curl) = have_curl() else {
                eprintln!("skipping: curl not in PATH");
                return;
            };
            let port = spawn_response(static_response("200 OK", b"baseline-asset"));
            let temp = crate::stage::create_temp_dir("gregg-update-test-base200").unwrap();
            let dest = temp.path().join("asset");
            let outcome = download_file(&curl, &format!("http://127.0.0.1:{port}/asset"), &dest);
            assert_eq!(outcome, DownloadOutcome::Success);
            assert_eq!(std::fs::read(&dest).unwrap(), b"baseline-asset");
        }

        #[test]
        fn baseline_download_follows_redirect_to_200() {
            let _proxy_guard = lock_proxy_env();
            let Some(curl) = have_curl() else {
                eprintln!("skipping: curl not in PATH");
                return;
            };
            let target_port = spawn_response(static_response("200 OK", b"baseline-redirected"));
            let front_port = spawn_response(
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{target_port}/asset\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .into_bytes(),
            );
            let temp = crate::stage::create_temp_dir("gregg-update-test-baseredir").unwrap();
            let dest = temp.path().join("asset");
            let outcome = download_file(
                &curl,
                &format!("http://127.0.0.1:{front_port}/asset"),
                &dest,
            );
            assert_eq!(outcome, DownloadOutcome::Success);
            assert_eq!(std::fs::read(&dest).unwrap(), b"baseline-redirected");
        }

        #[test]
        fn baseline_download_404_permits_fallback() {
            let _proxy_guard = lock_proxy_env();
            let Some(curl) = have_curl() else {
                eprintln!("skipping: curl not in PATH");
                return;
            };
            let port = spawn_response(static_response("404 Not Found", b"gone"));
            let temp = crate::stage::create_temp_dir("gregg-update-test-base404").unwrap();
            let dest = temp.path().join("asset");
            let outcome = download_file(&curl, &format!("http://127.0.0.1:{port}/asset"), &dest);
            assert_eq!(outcome, DownloadOutcome::NotFound);
            assert!(!dest.exists());
        }

        #[test]
        fn baseline_download_500_is_hard_failure() {
            let _proxy_guard = lock_proxy_env();
            let Some(curl) = have_curl() else {
                eprintln!("skipping: curl not in PATH");
                return;
            };
            let connected = Arc::new(AtomicBool::new(false));
            let port = spawn_recording_response(
                static_response("500 Internal Server Error", b"see /404-docs"),
                Arc::clone(&connected),
            );
            let temp = crate::stage::create_temp_dir("gregg-update-test-base500").unwrap();
            let dest = temp.path().join("asset");
            let outcome = download_file(&curl, &format!("http://127.0.0.1:{port}/asset"), &dest);
            // Without this the `Failed` below is satisfied just as well by a
            // `curl` that never started, and the 500 would never be exercised.
            assert!(
                connected.load(Ordering::SeqCst),
                "no client reached the fixture, so no HTTP status was classified: {outcome:?}"
            );
            let DownloadOutcome::Failed(reason) = outcome else {
                panic!("a 500 must be a hard failure, never a fallback-permitting NotFound: {outcome:?}");
            };
            // A `-f` curl exits non-zero on a 5xx, so the code arrives through
            // the non-success arm rather than the "unexpected HTTP" one.
            assert!(
                reason.contains("curl exit"),
                "the 500 must be reported from the failed curl run, not as \
                 something else: {reason}"
            );
            assert!(!dest.exists());
        }

        #[test]
        fn baseline_metadata_capture_200_within_limit() {
            let _proxy_guard = lock_proxy_env();
            let Some(curl) = have_curl() else {
                eprintln!("skipping: curl not in PATH");
                return;
            };
            let body = br#"{"crate":{"max_stable_version":"9.9.9"}}"#;
            let port = spawn_response(static_response("200 OK", body));
            let args = [
                "-fsSL",
                "--max-time",
                "10",
                &format!("http://127.0.0.1:{port}/api/v1/crates/foo"),
            ];
            let captured = run_curl_capture(&curl, &args).unwrap();
            assert_eq!(captured, body);
            assert_eq!(parse_stable_version_response(&captured).unwrap(), "9.9.9");
        }

        #[test]
        fn baseline_metadata_capture_rejects_oversized() {
            let _proxy_guard = lock_proxy_env();
            let Some(curl) = have_curl() else {
                eprintln!("skipping: curl not in PATH");
                return;
            };
            let big = vec![b'x'; MAX_CRATES_IO_BYTES + 1024];
            let port = spawn_response(static_response("200 OK", &big));
            let args = [
                "-fsSL",
                "--max-time",
                "10",
                &format!("http://127.0.0.1:{port}/api/v1/crates/foo"),
            ];
            let error = run_curl_capture(&curl, &args).expect_err("must reject oversized");
            assert!(
                error.to_string().contains("too large"),
                "unexpected error: {error}"
            );
        }
    }
}
