//! Installer-rerun lifecycle coverage for the Unix bootstrap installer
//! (Plan 112).
//!
//! Drives the real `packaging/install.sh` with fake `curl`/`cargo`
//! commands in an isolated `HOME`, proving same-scope rerun semantics
//! without network access: first install, same-version rerun,
//! older-version replacement, pinned-tag honor, foreign-destination
//! protection, and staging-only Cargo fallback.
//!
//! Unix-only: the harness needs `bash`, `mktemp`, and `sha256sum`/`shasum`.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

/// Repository root derived from this crate's manifest directory.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

#[test]
fn installer_rerun_same_scope_replacement() {
    let harness = repo_root().join("scripts/tests/test-install-rerun.sh");
    assert!(
        harness.exists(),
        "installer rerun harness missing: {}",
        harness.display()
    );
    let output = Command::new("bash")
        .arg(&harness)
        .env("LC_ALL", "C")
        .output()
        .expect("failed to run installer rerun harness");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "installer rerun harness failed:\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("fail=0"),
        "harness must report zero failures:\n{stdout}"
    );
}
