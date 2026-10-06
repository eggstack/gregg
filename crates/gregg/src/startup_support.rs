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

use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// The maximum a manager call may block.
///
/// A manager that hangs is a manager that has already failed; there is no
/// unbounded wait anywhere in the client's startup path.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// The most a manager call may produce on a single pipe.
///
/// The largest legitimate output is `crontab -l` on a real user's table, which
/// is bounded by the system's crontab spool limit and routinely runs to tens of
/// kilobytes — so this is deliberately far above anything a `systemctl`,
/// `launchctl` or `schtasks` query prints. It exists so a runaway manager cannot
/// exhaust memory; exceeding it is reported, never silently truncated, because a
/// truncated crontab read would be rewritten into a shorter crontab.
const MAX_MANAGER_OUTPUT_BYTES: usize = 1024 * 1024;

/// What a bounded manager call produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerOutput {
    /// Combined stdout and stderr, for diagnostics.
    ///
    /// This is only *lossless* when [`ManagerOutput::lossless`] is true; a lossy
    /// value carries U+FFFD substitutions and must never be written back out.
    pub combined: String,
    /// The exit status code, if the process ran.
    pub code: Option<i32>,
    /// Whether `combined` is the manager's bytes verbatim.
    ///
    /// False when the bytes were not valid UTF-8, or when either pipe exceeded
    /// [`MAX_MANAGER_OUTPUT_BYTES`]. Callers that rewrite a file from this output
    /// must fail closed instead of proceeding on a mangled read.
    pub lossless: bool,
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

    // Drain both pipes *while the child runs*. A pipe holds only a bounded
    // buffer (about 64 KiB on Linux), so a child that writes more than that
    // blocks in `write(2)` and never exits — reading only after `try_wait`
    // reports completion would deadlock every large output (notably
    // `crontab -l` on a real user's table) into the timeout kill below, which is
    // indistinguishable from "the manager produced nothing".
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());

    // Poll rather than block on `wait`: a manager that ignores its arguments
    // would otherwise hang the whole TUI, and there is no portable timeout-wait
    // in std.
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let (out, out_clipped) = settle(stdout);
                let (err, err_clipped) = settle(stderr);
                return Some(ManagerOutput {
                    combined: format!(
                        "{}{}",
                        String::from_utf8_lossy(&out),
                        String::from_utf8_lossy(&err)
                    ),
                    code: status.code(),
                    lossless: is_lossless(&out, &err, out_clipped, err_clipped),
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

/// A detached reader draining one pipe into a bounded buffer.
struct Drain {
    bytes: std::sync::mpsc::Receiver<Vec<u8>>,
}

/// Spawn a reader thread that keeps a pipe drained until it reaches EOF.
///
/// The thread stops reading one byte past [`MAX_MANAGER_OUTPUT_BYTES`] so an
/// oversized body is *detected* without ever being fully buffered.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> Option<Drain> {
    pipe.map(|pipe| {
        let (sender, bytes) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            // Read one byte past the cap: enough to prove the limit was crossed,
            // never enough to buffer an unbounded body.
            let _ = pipe
                .take(u64::try_from(MAX_MANAGER_OUTPUT_BYTES).unwrap_or(u64::MAX) + 1)
                .read_to_end(&mut buffer);
            let _ = sender.send(buffer);
        });
        Drain { bytes }
    })
}

/// Collect one drained pipe, reporting whether the cap discarded bytes.
fn settle(drain: Option<Drain>) -> (Vec<u8>, bool) {
    match drain {
        Some(Drain { bytes }) => match bytes.recv() {
            Ok(buffer) => {
                let clipped = buffer.len() > MAX_MANAGER_OUTPUT_BYTES;
                (buffer, clipped)
            }
            Err(_) => (Vec::new(), true),
        },
        None => (Vec::new(), false),
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

/// Whether a captured body may be written back verbatim.
///
/// A rewrite-from-read caller — `crontab -` replacing a whole table — must fail
/// closed on anything but the manager's exact bytes: invalid UTF-8 would be
/// rewritten with U+FFFD substitutions, and a clipped body would be rewritten
/// shorter than the file it came from.
fn is_lossless(out: &[u8], err: &[u8], out_clipped: bool, err_clipped: bool) -> bool {
    !out_clipped
        && !err_clipped
        && std::str::from_utf8(out).is_ok()
        && std::str::from_utf8(err).is_ok()
}

/// Read a file, treating absence as `None` and every other error as an error.
pub fn read_artifact(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Comfortably past any pipe buffer (Linux's default is 64 KiB). A child
    /// writing this much to an undrained pipe blocks in `write(2)` and never
    /// exits, which is precisely the `crontab -l` hang this module must not have.
    #[cfg(unix)]
    const OVERFILL_BYTES: usize = 512 * 1024;

    /// The regression this guards: a pipe is drained *while the writer runs*.
    ///
    /// Before the fix the reader only ran after the child exited, so a body
    /// larger than the pipe buffer could never be collected at all — the child
    /// blocked forever and the call was killed at the timeout, returning `None`,
    /// which the crontab path then read as "you have no crontab".
    #[cfg(unix)]
    #[test]
    fn drain_consumes_a_body_far_past_the_pipe_buffer_while_it_is_written() {
        use std::io::Write as _;
        use std::os::unix::net::UnixStream;

        let (mut writer, reader) = UnixStream::pair().expect("socket pair");
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let writer_thread = std::thread::spawn(move || {
            let chunk = vec![b'x'; 16 * 1024];
            for _ in 0..OVERFILL_BYTES / chunk.len() {
                writer.write_all(&chunk).expect("write to socket");
            }
            let _ = done_tx.send(());
        });

        let (bytes, clipped) = settle(drain(Some(reader)));

        // Bounded, so a regression fails the test instead of hanging CI.
        done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("writer finished only because the pipe was being drained");
        writer_thread.join().expect("writer thread");

        assert!(!clipped, "a body under the cap must not report clipping");
        assert_eq!(bytes.len(), OVERFILL_BYTES, "every byte must be collected");
        assert_eq!(bytes, vec![b'x'; OVERFILL_BYTES]);
    }

    #[test]
    fn lossless_requires_valid_utf8_and_no_clipping() {
        assert!(is_lossless(b"", b"", false, false), "empty is lossless");
        assert!(is_lossless(b"0 0\n* * * * *\n", b"", false, false));
        assert!(
            !is_lossless(b"\xff\xfe", b"", false, false),
            "invalid UTF-8 must not be reported lossless"
        );
        assert!(
            !is_lossless(b"ok", b"", true, false),
            "a clipped stdout must not be reported lossless"
        );
        assert!(
            !is_lossless(b"ok", b"", false, true),
            "a clipped stderr must not be reported lossless"
        );
    }

    #[test]
    fn settling_an_absent_pipe_is_not_treated_as_clipped() {
        let (bytes, clipped) = settle(None);
        assert!(bytes.is_empty());
        assert!(!clipped);
    }

    #[test]
    fn only_the_allowlist_may_run() {
        assert!(run_bounded("sh", &["-c", "echo hi"], Duration::from_secs(1)).is_none());
        assert!(is_allowed("crontab"));
        assert!(!is_allowed("sudo"));
    }
}
