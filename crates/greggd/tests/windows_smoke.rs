//! Windows daemon binary smoke test.
//!
//! Verifies the daemon binary compiles and responds to `--help`.
//! Runs only on Windows.

#![cfg(target_os = "windows")]

use std::process::{Command, Stdio};

#[test]
fn windows_daemon_binary_compiles_and_runs() {
    // Build the daemon binary.
    let build_status = Command::new("cargo")
        .args(["build", "-p", "greggd"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .status()
        .expect("cargo build should execute");
    assert!(build_status.success(), "cargo build must succeed");

    // Verify the binary exists and runs --help.
    let binary =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/greggd.exe");
    assert!(binary.exists(), "greggd.exe should exist at {binary:?}");

    let help_output = Command::new(&binary)
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
