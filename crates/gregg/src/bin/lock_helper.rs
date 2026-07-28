//! Test helper for cross-process lock contention tests.
//!
//! Usage: `lock_helper <lock-file> <signal-file>`
//!
//! 1. Opens the lock file at `<lock-file>` and acquires an exclusive OS-level lock.
//! 2. Creates `<signal-file>` to signal readiness to the parent process.
//! 3. Waits for stdin to close (parent sends EOF or kills the process).
//! 4. Releases the lock by closing the file handle.
//!
//! This helper is used by `#[cfg(windows)]` and `#[cfg(unix)]` tests in
//! `config.rs` to prove cross-process lock contention behavior.

#![allow(unsafe_code)] // Required for libc::flock and windows-sys LockFileEx/UnlockFileEx.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: lock_helper <lock-file> <signal-file>");
        std::process::exit(1);
    }

    let lock_path = PathBuf::from(&args[1]);
    let signal_path = PathBuf::from(&args[2]);

    // Open (or create) the lock file without truncating.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap_or_else(|e| {
            eprintln!("failed to open lock file {}: {e}", lock_path.display());
            std::process::exit(1);
        });

    // Acquire the exclusive OS-level lock.
    acquire_lock(&file, &lock_path);

    // Signal readiness.
    fs::write(&signal_path, b"ready").unwrap_or_else(|e| {
        eprintln!("failed to write signal file {}: {e}", signal_path.display());
        std::process::exit(1);
    });

    // Wait for stdin to close (parent sends EOF).
    let mut buf = [0u8; 1];
    let _ = io::stdin().read(&mut buf);

    // File handle drops here, releasing the lock.
    drop(file);
}

/// Acquire an exclusive lock on the file with bounded retry.
///
/// On Windows, uses `LockFileEx` with `LOCKFILE_EXCLUSIVE_LOCK`.
/// On Unix, uses `flock` with `LOCK_EX`.
fn acquire_lock(file: &File, lock_path: &std::path::Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);

    loop {
        if try_lock(file) {
            return;
        }
        if std::time::Instant::now() >= deadline {
            eprintln!(
                "lock_helper: timed out acquiring lock on {}",
                lock_path.display()
            );
            std::process::exit(1);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[cfg(windows)]
fn try_lock(file: &File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    let handle = file.as_raw_handle();
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    let result = unsafe {
        LockFileEx(
            handle as *mut _,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    result != 0
}

#[cfg(unix)]
fn try_lock(file: &File) -> bool {
    use std::os::unix::io::AsRawFd;

    let fd = file.as_raw_fd();
    let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    result == 0
}

#[cfg(not(any(unix, windows)))]
fn try_lock(_file: &File) -> bool {
    // On unsupported platforms, always succeed (no OS-level locking).
    true
}
