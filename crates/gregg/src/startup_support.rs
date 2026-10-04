//! Plan 165: the only place Gregg shells out to a startup manager.
//!
//! Every manager invocation in the client goes through here, which keeps three
//! properties checkable by reading one file: the command is looked up on a
//! fixed allowlist (never on `PATH` from a variable), the call is bounded, and
//! nothing is ever run through a shell — arguments are passed as a vector, so a
//! config path containing a space, a quote, or a `;` is a path, not a command.
//!
//! There is no `sudo` here, and there is no way to add one: the allowlist does
//! not contain it.

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// The maximum a manager call may block.
///
/// A manager that hangs is a manager that has already failed; there is no
/// unbounded wait anywhere in the client's startup path.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// What a bounded manager call produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerOutput {
    /// Combined stdout and stderr, for diagnostics.
    pub combined: String,
    /// The exit status code, if the process ran.
    pub code: Option<i32>,
}

impl ManagerOutput {
    /// Whether the manager reported success.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.code == Some(0)
    }
}

/// Run one manager command, bounded, with no shell.
///
/// Only the binaries on this allowlist may be run. That is a deliberate
/// narrowing rather than a convenience: the caller supplies no binary name from
/// configuration or environment, so there is no path by which a crafted config
/// value chooses what gets executed.
pub fn run_bounded(program: &str, args: &[&str], timeout: Duration) -> Option<ManagerOutput> {
    if !ALLOWED.contains(&program) {
        return None;
    }
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    // Poll rather than block on `wait_with_output`: a manager that ignores its
    // arguments would otherwise hang the whole TUI, and there is no portable
    // timeout-wait in std.
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = std::io::Read::read_to_string(&mut out, &mut stdout);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut err, &mut stderr);
                }
                return Some(ManagerOutput {
                    combined: format!("{stdout}{stderr}"),
                    code: status.code(),
                });
            }
            Ok(None) => {}
            Err(_) => return None,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// The complete set of external programs the client daemon's startup path may
/// execute.
///
/// `systemctl`, `launchctl`, `crontab`, and `schtasks` are the user-scoped
/// startup managers, and `id -u` supplies the launchd user-domain number.
/// `sudo` is absent, and nothing here can install a system service:
/// `systemctl` is only ever called with `--user`.
const ALLOWED: &[&str] = &["systemctl", "launchctl", "crontab", "schtasks", "id"];

/// Whether a program is on the startup allowlist.
#[must_use]
pub fn is_allowed(program: &str) -> bool {
    ALLOWED.contains(&program)
}

/// The bounded call with this module's default timeout.
pub fn run(program: &str, args: &[&str]) -> Option<ManagerOutput> {
    run_bounded(program, args, DEFAULT_TIMEOUT)
}

/// Read a file, treating absence as `None` and every other error as an error.
pub fn read_artifact(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
