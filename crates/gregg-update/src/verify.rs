//! Checksum and candidate-identity verification.
//!
//! Verification order is fixed: SHA-256 checksum first, then staged
//! candidate `version` identity. A mismatch in either is a hard error and
//! never falls back to Cargo.

use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

use crate::error::UpdateError;
use crate::exec::CANDIDATE_TIMEOUT;

/// Parse a `<asset>.sha256` file and return the lowercase hex digest.
pub fn parse_checksum_file(path: &Path) -> Result<String, UpdateError> {
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

/// Compute the SHA-256 hex digest of a file using the `sha2` crate
/// (never platform checksum tools, so verification is identical on every
/// host).
pub fn compute_sha256(path: &Path) -> Result<String, UpdateError> {
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

/// Verify that `file` matches the digest published in `sha_file`.
pub fn verify_checksum(file: &Path, sha_file: &Path) -> Result<(), UpdateError> {
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

/// Pure helper: does staged-candidate `version` stdout equal the exact
/// expected `<program> <version>` identity line?
///
/// Only a single trailing newline (`\n`, optionally preceded by `\r`) is
/// stripped; surrounding whitespace is significant so padded output fails.
#[must_use]
pub fn candidate_output_matches(program: &str, expected_version: &str, stdout: &str) -> bool {
    let normalized = stdout.strip_suffix('\n').unwrap_or(stdout);
    let normalized = normalized.strip_suffix('\r').unwrap_or(normalized);
    normalized == format!("{program} {expected_version}")
}

/// Validate a staged candidate: minimum size sanity, Unix executable bit,
/// then a bounded `version` invocation whose stdout must equal exactly
/// `<program> <expected_version>` with exit 0.
pub fn validate_candidate(
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
    let output = crate::exec::run_command_with_timeout(
        {
            let mut cmd = Command::new(candidate);
            cmd.arg("version");
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            cmd
        },
        CANDIDATE_TIMEOUT,
    )?;
    if !output.status.success() {
        return Err(UpdateError::CandidateMismatch(format!(
            "candidate 'version' failed with status {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let expected = format!("{program} {expected_version}");
    if !candidate_output_matches(program, expected_version, &stdout) {
        return Err(UpdateError::CandidateMismatch(format!(
            "candidate version mismatch: expected {expected:?}, got {:?}",
            stdout.trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_parser() {
        let dir = std::env::temp_dir().join(format!(
            "gregg-update-test-checksum-{}",
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
    fn checksum_parser_rejects_garbage() {
        let dir = std::env::temp_dir().join(format!(
            "gregg-update-test-checksum-bad-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&dir);
        let empty = dir.join("empty.sha256");
        fs::write(&empty, "").unwrap();
        assert!(parse_checksum_file(&empty).is_err());
        let short = dir.join("short.sha256");
        fs::write(&short, "abc123  file\n").unwrap();
        assert!(parse_checksum_file(&short).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn checksum_mismatch_is_rejected() {
        let dir = std::env::temp_dir().join(format!(
            "gregg-update-test-mismatch-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("asset");
        fs::write(&file, b"tampered bytes").unwrap();
        let sha = dir.join("asset.sha256");
        fs::write(
            &sha,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  asset\n",
        )
        .unwrap();
        let err = verify_checksum(&file, &sha).expect_err("tampered bytes must not verify");
        assert!(matches!(err, UpdateError::ChecksumMismatch { .. }));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn candidate_version_matching() {
        assert!(candidate_output_matches("gregg", "1.0.12", "gregg 1.0.12"));
        assert!(candidate_output_matches(
            "gregg",
            "1.0.12",
            "gregg 1.0.12\n"
        ));
        assert!(!candidate_output_matches("gregg", "1.0.12", "gregg 1.0.11"));
        assert!(!candidate_output_matches(
            "gregg",
            "1.0.12",
            "greggd 1.0.12"
        ));
        assert!(!candidate_output_matches(
            "gregg",
            "1.0.12",
            "  gregg 1.0.12  "
        ));
        assert!(!candidate_output_matches(
            "gregg",
            "1.0.12",
            "gregg 1.0.12\n\n"
        ));
    }
}
