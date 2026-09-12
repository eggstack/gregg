//! Shell quoting, cron watchdog block rendering/merging, and cron installation.

use super::install::InstallError;
use std::fmt;
use std::fmt::Write as FmtWrite;
use std::io::{self};
use std::path::{Path, PathBuf};
use std::process::Command;

// ── Shell quoting ─────────────────────────────────────────────────────────
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellQuoteError {
    ContainsControl { path: String },
    ContainsNewline { path: String },
}
impl fmt::Display for ShellQuoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContainsControl { path } => {
                write!(
                    f,
                    "path contains control character and cannot be shell-quoted: {path:?}"
                )
            }
            Self::ContainsNewline { path } => {
                write!(
                    f,
                    "path contains newline and cannot be shell-quoted: {path:?}"
                )
            }
        }
    }
}
impl std::error::Error for ShellQuoteError {}

/// Quote a path for safe inclusion in a POSIX shell cron line.
///
/// Wraps in single quotes and escapes embedded single quotes as `'\''`.
/// Rejects paths containing newlines or control characters.
pub fn shell_quote(path: &Path) -> Result<String, ShellQuoteError> {
    let s = path.to_string_lossy().to_string();
    if s.contains('\n') || s.contains('\r') {
        return Err(ShellQuoteError::ContainsNewline { path: s });
    }
    if s.chars().any(char::is_control) {
        return Err(ShellQuoteError::ContainsControl { path: s });
    }
    // Escape single quotes: ' -> '\''
    let escaped = s.replace('\'', "'\\''");
    Ok(format!("'{escaped}'"))
}
// ── Cron block rendering ──────────────────────────────────────────────────
pub const CRON_MANAGED_MARKER: &str = "# greggd managed watchdog";

/// Render the canonical cron block for the given executable and config.
///
/// Returns a string with trailing newline after each line, including final.
pub fn cron_block(exe: &Path, config: &Path, explicit: bool) -> Result<String, ShellQuoteError> {
    let exe_q = shell_quote(exe)?;
    let mut block = String::new();
    block.push_str(CRON_MANAGED_MARKER);
    block.push('\n');
    if explicit {
        let cfg_q = shell_quote(config)?;
        let _ = writeln!(block, "@reboot {exe_q} --config {cfg_q} croncheck");
        let _ = writeln!(block, "* * * * * {exe_q} --config {cfg_q} croncheck");
    } else {
        // For implicit default config, omit --config so daemon can use
        // default-path logic (missing file -> defaults). However cron is
        // user-owned and we want deterministic behavior; we still support
        // implicit by not adding --config. Callers that want explicit config
        // should pass explicit = true.
        let _ = writeln!(block, "@reboot {exe_q} croncheck");
        let _ = writeln!(block, "* * * * * {exe_q} croncheck");
    }
    Ok(block)
}

/// Convenience that always includes --config (the common Linux cron path).
pub fn cron_block_with_config(exe: &Path, config: &Path) -> Result<String, ShellQuoteError> {
    cron_block(exe, config, true)
}
// ── Crontab merging ───────────────────────────────────────────────────────

/// Remove any previously installed Gregg managed block from a crontab.
///
/// The block is identified by the marker line `CRON_MANAGED_MARKER` and any
/// following lines that contain `croncheck` (the two cron
/// schedule lines). Byte-for-byte preservation of unrelated lines is the goal.
pub fn remove_managed_cron_block(crontab: &str) -> String {
    let mut out = Vec::new();
    let mut lines = crontab.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() == CRON_MANAGED_MARKER {
            // Skip this marker and any immediately following croncheck lines.
            while let Some(next) = lines.peek() {
                if next.contains("croncheck") {
                    lines.next();
                } else {
                    break;
                }
            }
            continue;
        }
        // Also handle BEGIN/END style if somehow present (defensive).
        if line.trim() == "# BEGIN greggd managed watchdog" {
            for next in lines.by_ref() {
                if next.trim() == "# END greggd managed watchdog" {
                    break;
                }
            }
            continue;
        }
        out.push(line);
    }
    // Reconstruct with trailing newline handling: preserve original's final
    // newline semantics, but ensure we don't produce spurious blank lines at end.
    // `lines()` strips trailing newline, so we need to decide.
    let mut result = out.join("\n");
    if !crontab.is_empty() && crontab.ends_with('\n') && !result.is_empty() {
        result.push('\n');
    } else if result.is_empty() && !out.is_empty() {
        // Empty but had lines? keep as is.
    }
    // If original had no trailing newline but we removed block, keep no extra.
    // Ensure we don't leave double newlines from removal: out.join already does.
    result
}

/// Merge an existing crontab with a new canonical block idempotently.
///
/// Preserves unrelated entries byte-for-byte where practical; ensures exactly
/// one Gregg block at the end. `existing` may be empty (no crontab).
pub fn merge_crontab(existing: &str, new_block: &str) -> String {
    let stripped = remove_managed_cron_block(existing);
    let stripped_trim = stripped.trim_end_matches('\n');
    if stripped_trim.is_empty() {
        // No existing content: just the new block.
        let mut s = new_block.to_string();
        if !s.ends_with('\n') {
            s.push('\n');
        }
        return s;
    }
    // Existing content plus exactly one blank separator if needed, then new block.
    let mut out = String::new();
    out.push_str(stripped_trim);
    out.push('\n');
    // Ensure exactly one newline separation; new_block already starts with marker.
    out.push_str(new_block);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}
// ── Cron installation ─────────────────────────────────────────────────────
pub(crate) fn run_crontab_list() -> io::Result<String> {
    let output = Command::new("crontab").arg("-l").output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
        if stderr.contains("no crontab") || stderr.contains("no crontab for") {
            Ok(String::new())
        } else if output.status.code() == Some(1) && stderr.trim().is_empty() {
            // Some crontab implementations exit 1 with empty stderr for no crontab.
            // Treat as empty if stdout is empty.
            if output.stdout.is_empty() {
                Ok(String::new())
            } else {
                Err(io::Error::other(format!(
                    "crontab -l failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )))
            }
        } else {
            Err(io::Error::other(format!(
                "crontab -l failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )))
        }
    }
}
pub(crate) fn run_crontab_install(content: &str) -> io::Result<()> {
    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    {
        use std::io::Write;
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("failed to open crontab stdin"))?;
        stdin.write_all(content.as_bytes())?;
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "crontab - failed with status {status}"
        )))
    }
}

/// Install cron watchdog. Works for both privileged and unprivileged users.
/// No privilege check; cron is user-local.
pub fn install_cron(exe: &Path, config: &Path, explicit: bool) -> Result<(), InstallError> {
    let block = cron_block(exe, config, explicit)?;
    // Check crontab availability by trying to list.
    let existing = match run_crontab_list() {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(InstallError::CrontabUnavailable {
                message: format!(
                    "crontab not found: install cron or add manually:\n{block}\n\
                     Reminder: croncheck is the watchdog; no PID file required."
                ),
            });
        }
        Err(e) => {
            // If crontab exists but list failed for other reason, surface it
            // with manual instructions.
            return Err(InstallError::Other(format!(
                "crontab -l failed ({e}); add manually:\n{block}"
            )));
        }
    };
    let merged = merge_crontab(&existing, &block);
    run_crontab_install(&merged).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            InstallError::CrontabUnavailable {
                message: format!("crontab not found; add manually:\n{block}"),
            }
        } else {
            InstallError::Io {
                path: PathBuf::from("crontab -"),
                source: e,
            }
        }
    })?;
    println!("greggd cron watchdog installed");
    println!("entries:\n{block}");
    println!("Verify with: crontab -l");
    Ok(())
}
// ── Cron uninstall (Plan 112) ─────────────────────────────────────────────────

/// Pure helper: stripped crontab after Gregg managed-block removal, or
/// `None` when the crontab has no Gregg block and must be left untouched.
///
/// The caller installs the returned content only when `Some`, so a
/// block-free crontab never triggers a redundant `crontab -` write.
#[must_use]
pub fn cron_uninstall_changed(existing: &str) -> Option<String> {
    let stripped = remove_managed_cron_block(existing);
    if stripped == existing {
        None
    } else {
        Some(stripped)
    }
}

/// Remove only the Gregg managed watchdog block from the current
/// account's crontab, preserving unrelated entries byte-for-byte where
/// [`remove_managed_cron_block`] already guarantees that behavior.
///
/// Returns `true` when a managed block was removed. No crontab or no
/// Gregg marker is a successful no-op (`false`). A missing `crontab`
/// executable is only an error when listing proves there is nothing to
/// conclude from — without the binary there is no positive evidence of
/// Gregg cron integration, so removal is a no-op rather than a blocker
/// for the rest of the uninstall. Scoped to the current account only;
/// never enumerates other users or edits `/var/spool/cron` directly.
pub fn uninstall_cron() -> Result<bool, InstallError> {
    let existing = match run_crontab_list() {
        Ok(content) => content,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(InstallError::Io {
                path: PathBuf::from("crontab -l"),
                source: e,
            });
        }
    };
    let Some(stripped) = cron_uninstall_changed(&existing) else {
        return Ok(false);
    };
    run_crontab_install(&stripped).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            InstallError::CrontabUnavailable {
                message: "crontab not found while removing the Gregg managed block".to_string(),
            }
        } else {
            InstallError::Io {
                path: PathBuf::from("crontab -"),
                source: e,
            }
        }
    })?;
    println!("greggd cron watchdog removed");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_simple() {
        assert_eq!(
            shell_quote(Path::new("/usr/local/bin/greggd")).unwrap(),
            "'/usr/local/bin/greggd'"
        );
    }
    #[test]
    fn shell_quote_with_spaces() {
        assert_eq!(
            shell_quote(Path::new("/tmp/my path/greggd")).unwrap(),
            "'/tmp/my path/greggd'"
        );
    }
    #[test]
    fn shell_quote_with_single_quote() {
        assert_eq!(
            shell_quote(Path::new("/tmp/a'b/greggd")).unwrap(),
            "'/tmp/a'\\''b/greggd'"
        );
    }
    #[test]
    fn shell_quote_rejects_newline() {
        assert!(shell_quote(Path::new("/tmp/a\nb")).is_err());
        assert!(shell_quote(Path::new("/tmp/a\rb")).is_err());
    }
    #[test]
    fn shell_quote_rejects_control() {
        assert!(shell_quote(Path::new("/tmp/a\x01b")).is_err());
    }
    #[test]
    fn cron_block_renders_with_quoted_paths() {
        let exe = Path::new("/usr/local/bin/greggd");
        let cfg = Path::new("/etc/gregg/greggd.toml");
        let block = cron_block_with_config(exe, cfg).unwrap();
        assert!(block.starts_with(CRON_MANAGED_MARKER));
        assert!(block.contains(
            "@reboot '/usr/local/bin/greggd' --config '/etc/gregg/greggd.toml' croncheck"
        ));
        assert!(block.contains(
            "* * * * * '/usr/local/bin/greggd' --config '/etc/gregg/greggd.toml' croncheck"
        ));
    }
    #[test]
    fn cron_block_with_spaces_is_quoted() {
        let exe = Path::new("/tmp/my greggd");
        let cfg = Path::new("/tmp/my config.toml");
        let block = cron_block_with_config(exe, cfg).unwrap();
        assert!(block.contains("'/tmp/my greggd'"));
        assert!(block.contains("'/tmp/my config.toml'"));
    }
    #[test]
    fn cron_block_rejects_unsafe_paths() {
        let exe = Path::new("/tmp/a\nb");
        let cfg = Path::new("/etc/gregg/greggd.toml");
        assert!(cron_block_with_config(exe, cfg).is_err());
    }
    #[test]
    fn remove_managed_block_is_idempotent_and_preserves_unrelated() {
        let existing = "FOO=bar\n# greggd managed watchdog\n@reboot '/a' --config '/b' croncheck\n* * * * * '/a' --config '/b' croncheck\nOTHER=1\n";
        let stripped = remove_managed_cron_block(existing);
        assert_eq!(stripped, "FOO=bar\nOTHER=1\n");
        // Stripping again is idempotent
        assert_eq!(remove_managed_cron_block(&stripped), stripped);
    }
    #[test]
    fn merge_crontab_appends_and_is_idempotent() {
        let exe = Path::new("/usr/local/bin/greggd");
        let cfg = Path::new("/etc/gregg/greggd.toml");
        let block = cron_block_with_config(exe, cfg).unwrap();
        let existing = "FOO=bar\n";
        let merged = merge_crontab(existing, &block);
        assert!(merged.contains("FOO=bar"));
        assert!(merged.contains(CRON_MANAGED_MARKER));
        // Merging again should not duplicate
        let merged2 = merge_crontab(&merged, &block);
        assert_eq!(merged, merged2);
        // Count occurrences of marker
        assert_eq!(merged.matches(CRON_MANAGED_MARKER).count(), 1);
    }
    #[test]
    fn merge_empty_crontab() {
        let exe = Path::new("/usr/local/bin/greggd");
        let cfg = Path::new("/etc/gregg/greggd.toml");
        let block = cron_block_with_config(exe, cfg).unwrap();
        let merged = merge_crontab("", &block);
        assert_eq!(merged, block);
    }
    #[test]
    fn cron_uninstall_reports_change_only_for_managed_block() {
        assert_eq!(cron_uninstall_changed(""), None);
        assert_eq!(cron_uninstall_changed("FOO=bar\n"), None);
        let existing = "FOO=bar\n# greggd managed watchdog\n@reboot '/a' --config '/b' croncheck\n* * * * * '/a' --config '/b' croncheck\nOTHER=1\n";
        assert_eq!(
            cron_uninstall_changed(existing),
            Some("FOO=bar\nOTHER=1\n".to_string())
        );
    }
    #[test]
    fn cron_uninstall_block_only_strips_to_empty() {
        let exe = Path::new("/usr/local/bin/greggd");
        let cfg = Path::new("/etc/gregg/greggd.toml");
        let block = cron_block_with_config(exe, cfg).unwrap();
        assert_eq!(cron_uninstall_changed(&block), Some(String::new()));
    }
}
