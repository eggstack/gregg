//! Windows daemon smoke tests.
//!
//! - `windows_daemon_binary_compiles_and_runs`: verifies `--help` works.
//! - `foreground_daemon_serves_v2_status`: starts the daemon, polls
//!   health, fetches `/v2/status`, validates the response, and shuts
//!   down cleanly.
//!
//! Both tests run only on Windows.

#![cfg(target_os = "windows")]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/greggd.exe")
}

fn ensure_binary() {
    let status = Command::new("cargo")
        .args(["build", "-p", "greggd"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .expect("cargo build should execute");
    assert!(status.success(), "cargo build must succeed");
    let path = binary_path();
    assert!(
        path.exists(),
        "greggd.exe should exist at {}",
        path.display()
    );
}

/// Derive a TCP port from the test name so parallel test binaries do not
/// collide.  The result is in the dynamic range (49152..=65535).
#[allow(clippy::cast_possible_truncation)]
fn unique_port(name: &str) -> u16 {
    let hash: u32 = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    (49152 + (hash % 16383)) as u16
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

// ===== Test 1: binary compiles and runs --help =====

#[test]
fn windows_daemon_binary_compiles_and_runs() {
    ensure_binary();

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
    ensure_binary();

    let port = unique_port("foreground_daemon_serves_v2_status");
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

    // Start daemon as a child process.
    let mut child = Command::new(binary_path())
        .args(["--config", config_path.to_str().unwrap(), "run"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn greggd");

    let start = Instant::now();
    let timeout = Duration::from_secs(30);

    // Poll /v2/healthz until ready. Use v2 because the v1 health
    // response is never updated on Windows (stays Warming).
    let mut ready = false;
    while start.elapsed() < timeout {
        if let Ok((status, body)) = http_get("127.0.0.1", port, "/v2/healthz") {
            if status == 200 && body.contains("\"ready\"") {
                ready = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(ready, "daemon did not become ready within {timeout:?}");

    // Fetch /v2/status.
    let (status, body) =
        http_get("127.0.0.1", port, "/v2/status").expect("GET /v2/status should succeed");
    assert_eq!(status, 200, "/v2/status should return 200");

    // Validate JSON structure.
    let json: serde_json::Value =
        serde_json::from_str(&body).expect("/v2/status body should be valid JSON");
    assert_eq!(json["schema_version"], 2, "schema_version should be 2");

    // Validate Windows capabilities.
    let caps = &json["capabilities"];
    assert_eq!(caps["cpu_iowait"], false, "cpu_iowait should be false");
    assert_eq!(caps["load_average"], false, "load_average should be false");
    assert_eq!(caps["swap"], false, "swap should be false");
    assert_eq!(caps["memory_commit"], true, "memory_commit should be true");

    // Validate identity.
    assert_eq!(
        json["system"]["os_name"], "windows",
        "os_name should be windows"
    );
    assert!(
        json["system"]["hostname"].is_string(),
        "hostname should be a string"
    );
    assert!(
        !json["system"]["hostname"].as_str().unwrap().is_empty(),
        "hostname should not be empty"
    );

    // Validate metrics are present.
    assert!(
        json["cpu"]["logical_cores"].as_u64().unwrap_or(0) > 0,
        "logical_cores should be > 0"
    );
    assert!(
        json["memory"]["total_bytes"].as_u64().unwrap_or(0) > 0,
        "memory total_bytes should be > 0"
    );
    assert!(
        json["commit"].is_object(),
        "commit should be present (not null)"
    );

    // Unsupported metrics absent.
    assert!(json["load"].is_null(), "load should be null");
    assert!(json["swap"].is_null(), "swap should be null");

    // Shut down the daemon.
    child.kill().expect("kill daemon");
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&tmp_dir);
}
