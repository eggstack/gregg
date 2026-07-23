//! Test-only fixture loader for the Linux collector.
//!
//! Production code does not include this module. The path is computed from
//! `CARGO_MANIFEST_DIR` plus `src/collector/test_fixtures`, so tests run from
//! the same crate root regardless of the working directory used to launch
//! `cargo test`.

#![allow(dead_code, reason = "exercised by integration tests")]

use std::fs;
use std::path::PathBuf;

/// Return the absolute path of a fixture file under `src/collector/test_fixtures`.
#[must_use]
pub fn fixture_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("src");
    path.push("collector");
    path.push("test_fixtures");
    path.push(name);
    path
}

/// Read a fixture file as a UTF-8 string.
#[must_use]
pub fn read_fixture(name: &str) -> String {
    let path = fixture_path(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
}
