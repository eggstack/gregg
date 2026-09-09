//! Bounded child-process execution for startup manager probes and commands.

use std::io::{self, Read};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) const MANAGER_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(unix)]
pub(crate) const DIRECT_RESTART_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) fn run_bounded_command(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> io::Result<Output> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
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
            None => thread::sleep(Duration::from_millis(10)),
        }
    };
    Ok(Output {
        status,
        stdout: join_pipe(stdout)?,
        stderr: join_pipe(stderr)?,
    })
}
fn read_pipe<R: Read + Send + 'static>(mut reader: R) -> thread::JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
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
}
