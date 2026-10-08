//! Bounded child-process execution for startup manager probes and commands.

use std::io::{self, Read, Write};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const MANAGER_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(unix)]
pub(crate) const DIRECT_RESTART_TIMEOUT: Duration = Duration::from_secs(10);
/// Poll interval for bounded child waits. A 10ms sleep keeps manager probes
/// responsive without a timeout thread; wakeups are bounded (~1000 per 10s
/// probe) and probes are infrequent operator commands, so blocking `wait()`
/// with `wait_timeout` is not worth the extra dependency.
pub(crate) const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Upper bound for one captured child pipe (stdout/stderr each). Manager
/// probes (crontab, systemctl) are small; a huge output OOMs discovery, so
/// readers stop after this many bytes.
pub(crate) const MAX_CHILD_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
pub(crate) fn run_bounded_command(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> io::Result<Output> {
    run_bounded_command_with_stdin(program, args, timeout, None)
}

/// As [`run_bounded_command`], additionally feeding `stdin_data` to the
/// child's stdin.
///
/// Used by `crontab -`, whose entire input is a rewritten crontab. A child
/// that rejects the content and exits early makes the write fail with EPIPE;
/// that is not returned on its own, because the child's own exit status is the
/// more truthful error. The write error surfaces only when the child *did*
/// succeed, which is exactly the case where something is genuinely wrong.
pub(crate) fn run_bounded_command_with_stdin(
    program: &str,
    args: &[&str],
    timeout: Duration,
    stdin_data: Option<&[u8]>,
) -> io::Result<Output> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if stdin_data.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    let mut child = command.spawn()?;
    let mut write_error = None;
    if let Some(data) = stdin_data {
        match child.stdin.take() {
            Some(mut stdin) => write_error = stdin.write_all(data).err(),
            None => write_error = Some(io::Error::other("failed to open child stdin")),
        }
    }
    let stdout = child.stdout.take().map(read_pipe);
    let stderr = child.stderr.take().map(read_pipe);
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_pipe(stdout);
                let _ = join_pipe(stderr);
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("{program} timed out after {}s", timeout.as_secs()),
                ));
            }
            None => thread::sleep(CHILD_POLL_INTERVAL),
        }
    };
    let output = Output {
        status,
        stdout: join_pipe(stdout)?,
        stderr: join_pipe(stderr)?,
    };
    if output.status.success() {
        if let Some(error) = write_error {
            return Err(error);
        }
    }
    Ok(output)
}
fn read_pipe<R: Read + Send + 'static>(reader: R) -> thread::JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        // Bounded: a runaway child cannot OOM discovery via a huge crontab
        // or manager dump.
        reader
            .take(u64::try_from(MAX_CHILD_OUTPUT_BYTES + 1).unwrap_or(u64::MAX))
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_CHILD_OUTPUT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "child output exceeds 4 MiB cap",
            ));
        }
        Ok(bytes)
    })
}
fn join_pipe(reader: Option<thread::JoinHandle<io::Result<Vec<u8>>>>) -> io::Result<Vec<u8>> {
    match reader {
        Some(reader) => reader
            .join()
            .map_err(|_| io::Error::other("child output reader panicked"))?,
        None => Ok(Vec::new()),
    }
}
#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;

    #[cfg(unix)]
    #[test]
    fn bounded_manager_command_captures_stderr_and_kills_on_timeout() {
        let output = run_bounded_command(
            "sh",
            &["-c", "printf stdout; printf denied >&2; exit 7"],
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, b"stdout");
        assert_eq!(output.stderr, b"denied");

        let error = run_bounded_command("sh", &["-c", "sleep 1"], Duration::from_millis(40))
            .expect_err("slow manager command must time out");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    /// `crontab -` takes its whole input on stdin, so it needs the bounded
    /// helper to also feed stdin. Unbounded, a hung cron daemon would hang an
    /// operator command forever -- the exact deviation this helper exists to
    /// prevent.
    #[cfg(unix)]
    #[test]
    fn bounded_command_with_stdin_delivers_input_and_still_bounds_the_child() {
        let output = run_bounded_command_with_stdin(
            "sh",
            &["-c", "cat"],
            Duration::from_secs(2),
            Some(b"crontab body\n"),
        )
        .expect("a well-behaved child must succeed");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"crontab body\n");

        // The stdin variant is bounded exactly like the plain one.
        let error = run_bounded_command_with_stdin(
            "sh",
            &["-c", "cat > /dev/null; sleep 1"],
            Duration::from_millis(40),
            Some(b"x"),
        )
        .expect_err("a slow child must time out even when stdin was written");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    /// A child that never reads its stdin makes the write fail with EPIPE once the
    /// pipe buffer fills. Reporting that as the error would bury the child's
    /// own exit status, which is the more truthful diagnosis.
    ///
    /// The payload is larger than a pipe buffer on purpose: a small write is
    /// absorbed by the kernel buffer before the child can exit, so the test
    /// would pass for the wrong reason. Here the write necessarily blocks and
    /// then meets a reader-less pipe.
    #[cfg(unix)]
    #[test]
    fn a_child_that_exits_early_reports_its_status_not_the_broken_pipe() {
        let oversized = vec![b'x'; 512 * 1024];
        let output = run_bounded_command_with_stdin(
            "sh",
            &["-c", "exit 3"],
            Duration::from_secs(10),
            Some(&oversized),
        )
        .expect("the child's own non-zero exit is a normal result here");
        assert_eq!(output.status.code(), Some(3));
    }
}
