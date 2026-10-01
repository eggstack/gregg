//! Windows daemon smoke tests.
//!
//! - `windows_daemon_binary_compiles_and_runs`: verifies `--help` works.
//! - `foreground_daemon_serves_v2_status`: starts the daemon, polls
//!   health, fetches `/v2/status`, validates the response, and shuts
//!   down cleanly.
//!
//! Both tests run only on Windows.
//!
//! Cargo builds the `greggd` binary before this integration test starts and
//! exposes the exact path through `CARGO_BIN_EXE_greggd`, so the harness never
//! runs a nested `cargo build`.
//!
//! The foreground smoke is written so a failure is diagnosable rather than a
//! bare timeout: the port comes from an OS `127.0.0.1:0` allocation, child
//! stdout/stderr are captured in files inside the test's temporary directory
//! instead of unread pipes, readiness polling fails immediately when the child
//! exits, and every exit path terminates and reaps the child.

#![cfg(target_os = "windows")]

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Overall readiness budget for one foreground daemon.
///
/// A healthy daemon satisfies this on ordinary hosted Windows; it is not
/// raised to mask a startup failure, a bind failure, or a collector failure.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Poll cadence while waiting for readiness.
const READY_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Maximum characters of a health body kept in the last-probe diagnostic.
const PROBE_EXCERPT_CHARS: usize = 200;

/// Maximum bytes of captured daemon output kept per stream, so a pathological
/// child cannot flood CI logs. Only the tail is kept.
const CAPTURE_TAIL_BYTES: usize = 8 * 1024;

/// The `greggd` binary Cargo built for this integration test.
fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_greggd"))
}

/// Ask the OS for a currently free loopback port.
///
/// The temporary listener is released immediately before `greggd` is spawned,
/// which leaves a small unavoidable bind-after-release race. Resolving that
/// race properly would need a broker or socket inheritance in production code;
/// both are out of scope for a smoke test, and an early child exit from a lost
/// race is reported explicitly by the readiness loop.
fn free_loopback_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral loopback port");
    let port = listener
        .local_addr()
        .expect("read the OS-selected loopback port")
        .port();
    drop(listener);
    port
}

/// Send a raw HTTP/1.1 GET request and return `(status_code, body)`.
fn http_get(host: &str, port: u16, path: &str) -> Result<(u16, String), String> {
    let addr = format!("{host}:{port}");
    let mut stream = TcpStream::connect(&addr).map_err(|e| format!("connect to {addr}: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write: {e}"))?;

    let mut reader = BufReader::new(&stream);
    let mut status_code = 0u16;
    let mut body = String::new();
    let mut in_body = false;
    let mut content_length: Option<usize> = None;

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("read: {e}")),
        }

        if in_body {
            body.push_str(&line);
        } else {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                in_body = true;
                continue;
            }
            if trimmed.starts_with("HTTP/") {
                let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
                if parts.len() >= 2 {
                    status_code = parts[1].parse().unwrap_or(0);
                }
            } else if trimmed.to_lowercase().starts_with("content-length:") {
                let val = trimmed.split_once(':').map_or("", |x| x.1).trim();
                content_length = val.parse().ok();
            }
        }
    }

    // Trim body to content-length if present.
    if let Some(len) = content_length {
        body.truncate(len);
    }

    Ok((status_code, body))
}

/// Bound a diagnostic excerpt without panicking on a char boundary.
fn excerpt(text: &str, max_chars: usize) -> String {
    let mut bounded: String = text.chars().take(max_chars).collect();
    if text.chars().nth(max_chars).is_some() {
        bounded.push_str("...");
    }
    bounded
}

/// Read the tail of one capture file, bounded so CI logs stay readable.
fn capture_tail(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return "<capture file unavailable>".to_string();
    };
    if bytes.is_empty() {
        return "<empty>".to_string();
    }
    let truncated = bytes.len() > CAPTURE_TAIL_BYTES;
    let tail = if truncated {
        &bytes[bytes.len() - CAPTURE_TAIL_BYTES..]
    } else {
        bytes.as_slice()
    };
    let text = String::from_utf8_lossy(tail).into_owned();
    if truncated {
        format!("[truncated to last {CAPTURE_TAIL_BYTES} bytes]\n{text}")
    } else {
        text
    }
}

fn describe_exit(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exited with code {code}"),
        None => format!("terminated without an exit code ({status})"),
    }
}

fn require(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.to_string())
    }
}

/// The spawned foreground daemon plus everything a failure report needs.
///
/// Dropping this value terminates and reaps the child, so a failed assertion
/// can never leave a live `greggd.exe` behind for later work in the same job.
struct DaemonProcess {
    child: Option<Child>,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    config_path: PathBuf,
    capture_dir: PathBuf,
    port: u16,
    started: Instant,
    exit: Option<ExitStatus>,
    terminated_by_test: bool,
    last_probe: String,
}

impl DaemonProcess {
    /// Spawn `greggd run` with file-backed stdout/stderr capture.
    fn spawn(config_path: &Path, capture_dir: &Path, port: u16) -> Self {
        let stdout_path = capture_dir.join("daemon-stdout.log");
        let stderr_path = capture_dir.join("daemon-stderr.log");
        let stdout = File::create(&stdout_path).expect("create daemon stdout capture file");
        let stderr = File::create(&stderr_path).expect("create daemon stderr capture file");
        let child = Command::new(binary_path())
            .arg("--config")
            .arg(config_path)
            .arg("run")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("spawn greggd");

        Self {
            child: Some(child),
            stdout_path,
            stderr_path,
            config_path: config_path.to_path_buf(),
            capture_dir: capture_dir.to_path_buf(),
            port,
            started: Instant::now(),
            exit: None,
            terminated_by_test: false,
            last_probe: "no readiness probe completed".to_string(),
        }
    }

    /// Record the most recent readiness probe outcome, bounded for reporting.
    fn note_probe(&mut self, outcome: &str) {
        self.last_probe = excerpt(outcome, PROBE_EXCERPT_CHARS);
    }

    /// Non-blocking child check; returns the exit status once the child is gone.
    fn poll_exit(&mut self) -> Option<ExitStatus> {
        let status = self.child.as_mut()?.try_wait().ok().flatten();
        if status.is_some() {
            self.exit = status;
        }
        status
    }

    /// Terminate and reap the child if it is still running. Never panics, so it
    /// is safe to call from both the orderly path and `Drop`.
    fn stop(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(status)) => self.exit = Some(status),
            Ok(None) => {
                self.terminated_by_test = true;
                let _ = child.kill();
                if let Ok(status) = child.wait() {
                    self.exit = Some(status);
                }
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        self.child = None;
    }

    /// Wait for v2 health readiness, failing fast on an early child exit.
    ///
    /// v2 health is used because the v1 health response is never updated on
    /// Windows (it stays `Warming`).
    fn await_ready(&mut self) -> Result<(), String> {
        while self.started.elapsed() < READY_TIMEOUT {
            if let Some(status) = self.poll_exit() {
                return Err(format!(
                    "greggd {} before it became ready",
                    describe_exit(status)
                ));
            }

            match http_get("127.0.0.1", self.port, "/v2/healthz") {
                Ok((status, body)) => {
                    let state = serde_json::from_str::<serde_json::Value>(&body)
                        .ok()
                        .and_then(|json| {
                            json.get("state")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned)
                        });
                    match state.as_deref() {
                        Some("ready") if status == 200 => return Ok(()),
                        Some(state) => self.note_probe(&format!(
                            "HTTP {status} state={state} body={}",
                            excerpt(&body, PROBE_EXCERPT_CHARS)
                        )),
                        None => self.note_probe(&format!(
                            "HTTP {status} body is not a v2 health response: {}",
                            excerpt(&body, PROBE_EXCERPT_CHARS)
                        )),
                    }
                }
                Err(error) => self.note_probe(&error),
            }
            std::thread::sleep(READY_POLL_INTERVAL);
        }

        Err(format!(
            "daemon did not become ready within {READY_TIMEOUT:?}"
        ))
    }

    /// Child lifecycle state as of teardown: whether the daemon was still alive
    /// when the failure was detected, and how it ended.
    fn child_state(&self) -> String {
        match self.exit {
            Some(status) if self.terminated_by_test => format!(
                "still running at failure, terminated by this test ({})",
                describe_exit(status)
            ),
            Some(status) => format!("exited before the failure ({})", describe_exit(status)),
            None => "state unknown (no reap result)".to_string(),
        }
    }

    /// Bounded diagnostic report: child state, request context, and the tail of
    /// the daemon's own output.
    fn report(&self, reason: &str) -> String {
        format!(
            "{reason}\n\
             port: {}\n\
             config: {}\n\
             elapsed: {:?}\n\
             child: {}\n\
             last readiness probe: {}\n\
             daemon stdout (tail):\n{}\n\
             daemon stderr (tail):\n{}\n\
             retained capture directory: {}",
            self.port,
            self.config_path.display(),
            self.started.elapsed(),
            self.child_state(),
            self.last_probe,
            capture_tail(&self.stdout_path),
            capture_tail(&self.stderr_path),
            self.capture_dir.display(),
        )
    }

    /// Stop the child, then fail with the collected diagnostics.
    fn fail(&mut self, reason: &str) -> ! {
        self.stop();
        panic!("{}", self.report(reason));
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Validate the daemon's v2 status payload.
fn check_v2_status(port: u16) -> Result<(), String> {
    let (status, body) = http_get("127.0.0.1", port, "/v2/status")
        .map_err(|error| format!("GET /v2/status should succeed: {error}"))?;
    require(status == 200, "/v2/status should return 200")?;

    // Validate JSON structure.
    let json: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
        format!(
            "/v2/status body should be valid JSON: {error} (body: {})",
            excerpt(&body, PROBE_EXCERPT_CHARS)
        )
    })?;
    require(json["schema_version"] == 2, "schema_version should be 2")?;

    // Validate Windows capabilities.
    let caps = &json["capabilities"];
    require(
        caps["cpu_iowait"].as_bool() == Some(false),
        "cpu_iowait should be false",
    )?;
    require(
        caps["load_average"].as_bool() == Some(false),
        "load_average should be false",
    )?;
    require(
        caps["swap"].as_bool() == Some(false),
        "swap should be false",
    )?;
    require(
        caps["memory_commit"].as_bool() == Some(true),
        "memory_commit should be true",
    )?;

    // Validate identity.
    require(
        json["system"]["os_name"] == "windows",
        "os_name should be windows",
    )?;
    require(
        json["system"]["name"] == "smoke-test",
        "configured name should be preserved",
    )?;
    let hostname = json["system"]["hostname"]
        .as_str()
        .ok_or_else(|| "hostname should be a string".to_string())?;
    require(!hostname.is_empty(), "hostname should not be empty")?;
    require(!hostname.contains('\0'), "hostname should not contain NUL")?;
    let name = json["system"]["name"]
        .as_str()
        .ok_or_else(|| "name should be a string".to_string())?;
    require(!name.contains('\0'), "name should not contain NUL")?;

    // Validate metrics are present.
    require(
        json["cpu"]["logical_cores"].as_u64().unwrap_or(0) > 0,
        "logical_cores should be > 0",
    )?;
    require(
        json["memory"]["total_bytes"].as_u64().unwrap_or(0) > 0,
        "memory total_bytes should be > 0",
    )?;
    require(
        json["commit"].is_object(),
        "commit should be present (not null)",
    )?;

    // Unsupported metrics absent.
    require(json["load"].is_null(), "load should be null")?;
    require(json["swap"].is_null(), "swap should be null")?;

    Ok(())
}

// ===== Test 1: binary compiles and runs --help =====

#[test]
fn windows_daemon_binary_compiles_and_runs() {
    let help_output = Command::new(binary_path())
        .arg("--help")
        .output()
        .expect("greggd --help should execute");
    assert!(help_output.status.success(), "greggd --help must succeed");

    let stdout = String::from_utf8_lossy(&help_output.stdout);
    assert!(
        stdout.contains("greggd"),
        "help output should mention greggd"
    );
    assert!(
        stdout.contains("CPU") || stdout.contains("metrics"),
        "help output should mention metrics"
    );
}

// ===== Test 2: foreground daemon serves v2 status =====

#[test]
fn foreground_daemon_serves_v2_status() {
    let port = free_loopback_port();
    let tmp_dir = std::env::temp_dir().join(format!("greggd-smoke-{port}"));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");

    let config_path = tmp_dir.join("greggd.toml");
    let config = format!(
        r#"
name = "smoke-test"
host = "127.0.0.1"
port = {port}
sample_interval_ms = 250
stale_after_ms = 0
"#
    );
    std::fs::write(&config_path, config).expect("write config");

    let mut daemon = DaemonProcess::spawn(&config_path, &tmp_dir, port);

    if let Err(reason) = daemon.await_ready() {
        // The capture directory is left in place on failure; its path is part
        // of the diagnostic and the content is already inlined in it.
        daemon.fail(&reason);
    }

    if let Err(reason) = check_v2_status(port) {
        daemon.fail(&reason);
    }

    // Orderly teardown: stop and reap the child, then drop its files.
    daemon.stop();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}
