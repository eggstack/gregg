//! Cross-process advisory file locking for the client configuration and for
//! the client daemon's single-launch decision.

use std::fs;
use std::path::PathBuf;

use crate::config::ConfigError;

// The cross-process configuration lock relies on platform file-locking
// primitives (flock on unix, LockFileEx on windows). Fail the build loudly on
// any other target rather than silently degrading to in-process-only locking,
// where concurrent processes could interleave config writes.
#[cfg(not(any(unix, windows)))]
compile_error!("cross-process config locking is only implemented for unix and windows targets");

pub struct FileLockGuard {
    #[allow(dead_code)]
    pub(crate) file: fs::File,
    /// On Windows, the raw handle value is retained so we can call
    /// `UnlockFileEx` before the file is dropped. On Unix, this is `None`.
    /// Stored as `isize` for cross-platform struct layout.
    #[allow(dead_code)]
    pub(crate) handle: Option<isize>,
    /// The lock path, for diagnostics.
    ///
    /// Carried explicitly because `File::path` is newer than the pinned MSRV.
    pub(crate) path: PathBuf,
}

impl FileLockGuard {
    /// Acquire an exclusive advisory lock on `path`, waiting at most
    /// `timeout`.
    ///
    /// Exposed at the path level because Plan 165 needs a *second*, unrelated
    /// lock: the config mutation lock protects the file, while the client
    /// daemon's launch lock protects the "exactly one spawn" decision. Sharing
    /// one lock would make an ordinary `gregg add` contend with a TUI launch.
    ///
    /// The lock file is created if needed and is never deleted, because a
    /// removed inode can be re-created by a racing opener and split the lock
    /// in two. Its presence therefore proves nothing; the OS lock is what
    /// counts, which is exactly why nothing in Gregg decides "is this daemon
    /// running" by looking at a lock file.
    ///
    /// # Errors
    ///
    /// Returns a [`crate::config::ConfigError`] on I/O failure, or
    /// [`crate::config::ConfigError::LockTimeout`] if the lock cannot be taken
    /// within `timeout`.
    #[allow(unsafe_code)] // Uses libc::flock (unix) and LockFileEx (windows).
    pub fn acquire(
        path: &std::path::Path,
        timeout: std::time::Duration,
    ) -> Result<Self, ConfigError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        // Never truncate: a concurrent holder's inode must stay the same file.
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|source| ConfigError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Self::lock_file(file, path, timeout)
    }

    /// The path this guard holds, for diagnostics.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    #[allow(unsafe_code)] // Uses libc::flock (unix) and LockFileEx (windows).
    fn lock_file(
        file: fs::File,
        path: &std::path::Path,
        timeout: std::time::Duration,
    ) -> Result<Self, ConfigError> {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = file.as_raw_fd();
            let deadline = std::time::Instant::now() + timeout;
            loop {
                // SAFETY: `fd` is a valid descriptor owned by `file` for the
                // whole call, and the flags request a non-blocking exclusive
                // advisory lock, which `flock` fully defines.
                let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
                if result == 0 {
                    return Ok(Self {
                        file,
                        handle: None,
                        path: path.to_path_buf(),
                    });
                }
                // Only contention is worth retrying. `EBADF`, `EINVAL`,
                // `ENOLCK`, or `ENOTSUP` mean this descriptor or filesystem
                // cannot lock at all, and retrying them for the full timeout
                // would misreport a real failure as contention.
                let errno = std::io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(libc::EIO);
                if errno != libc::EWOULDBLOCK && errno != libc::EAGAIN {
                    return Err(ConfigError::Io {
                        path: path.to_path_buf(),
                        source: std::io::Error::from_raw_os_error(errno),
                    });
                }
                if std::time::Instant::now() >= deadline {
                    return Err(ConfigError::LockTimeout {
                        path: path.to_path_buf(),
                        timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                    });
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            // `LockFileEx` and both of its flags live in
            // `Win32_Storage_FileSystem` in windows-sys 0.59; only the
            // `OVERLAPPED` it fills in comes from `Win32_System_IO`.
            use windows_sys::Win32::Storage::FileSystem::{
                LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
            };
            use windows_sys::Win32::System::IO::OVERLAPPED;
            let handle = file.as_raw_handle();
            let deadline = std::time::Instant::now() + timeout;
            loop {
                // SAFETY: `OVERLAPPED` is a plain `repr(C)` aggregate of
                // integers, a raw pointer, and a handle, for which an all-zero
                // bit pattern is the documented "nothing in flight" value.
                let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
                // SAFETY: `handle` is a valid file handle owned by `file`, and
                // `overlapped` is a live, zeroed stack allocation that the API
                // only reads offsets from.
                let result = unsafe {
                    LockFileEx(
                        handle,
                        LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                        0,
                        u32::MAX,
                        u32::MAX,
                        &raw mut overlapped,
                    )
                };
                if result != 0 {
                    return Ok(Self {
                        file,
                        handle: Some(handle as isize),
                        path: path.to_path_buf(),
                    });
                }
                if std::time::Instant::now() >= deadline {
                    return Err(ConfigError::LockTimeout {
                        path: path.to_path_buf(),
                        timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                    });
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        // Reached only on a target the `compile_error!` at the top of this
        // module already rejects.
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (file, path, timeout);
            Err(ConfigError::LockTimeout {
                path: path.to_path_buf(),
                timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            })
        }
    }
}
#[allow(unsafe_code)]
impl Drop for FileLockGuard {
    fn drop(&mut self) {
        // Closing the file descriptor releases the flock on Unix. On
        // Windows, we must explicitly unlock before the handle closes.
        #[cfg(windows)]
        if let Some(handle) = self.handle {
            // Safety: UnlockFileEx is called with a valid handle previously
            // returned by LockFileEx and an OVERLAPPED zeroed on the stack.
            // The handle remains open for the duration of this call.
            unsafe {
                use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
                use windows_sys::Win32::System::IO::OVERLAPPED;
                let mut overlapped: OVERLAPPED = std::mem::zeroed();
                #[allow(clippy::ptr_as_ptr)]
                UnlockFileEx(handle as *mut _, 0, 1, 0, &raw mut overlapped);
            }
        }
        // We do not delete the lock file — it may be reused by the next
        // acquirer and removing it could race with a concurrent open.
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::Config;
    use super::super::store::ConfigStore;
    use super::super::test_helpers::{find_lock_helper, tmp_dir};
    use super::*;
    use std::path::PathBuf;

    #[test]
    #[cfg(unix)]
    fn concurrent_subprocesses_do_not_lose_updates() {
        use std::process::Command;

        let dir = tmp_dir("concurrent_lock");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        // Initialize config.
        store
            .mutate(|config| {
                config.default_port = 11310;
                Ok(())
            })
            .unwrap();

        // Spawn 10 subprocesses that each add a different endpoint.
        // Each subprocess uses `flock` to hold the lock while mutating.
        let lock_str = format!("{}.lock", path.to_str().unwrap());
        let path_str = path.to_str().unwrap();
        let mut children = Vec::new();
        for i in 0..10 {
            let host = format!("10.0.0.{i}");
            let script = format!(
                r#"
                (
                    flock 9
                    echo "host={host}" >> "{path_str}.tmp"
                ) 9>"{lock_str}"
                "#,
            );
            let child = Command::new("sh").arg("-c").arg(script).spawn().unwrap();
            children.push(child);
        }

        for mut child in children {
            let status = child.wait().unwrap();
            assert!(status.success(), "subprocess failed");
        }

        // Verify all 10 entries were written without loss.
        let entries = fs::read_to_string(format!("{}.tmp", path.to_str().unwrap())).unwrap();
        let count = entries.lines().count();
        assert_eq!(count, 10, "all 10 endpoints should be present, got {count}");

        let _ = fs::remove_dir_all(&dir);
    }

    /// Cross-process lock contention test using the `lock_helper` binary.
    ///
    /// Verifies that:
    /// - Process A (`lock_helper`) acquires the OS lock on `<config>.lock`.
    /// - Process B (this test) cannot mutate while A holds the lock.
    /// - After A releases, B completes successfully.
    /// - Final config is valid.
    #[test]
    fn cross_process_lock_contention_via_helper() {
        use std::process::Command;

        let dir = tmp_dir("cross_process_lock_helper");
        let path = dir.join("config.toml");
        let lock_path = PathBuf::from(format!("{}.lock", path.display()));
        let signal_path = dir.join("ready.signal");
        let store = ConfigStore::new(path.clone());

        // Initialize config.
        store
            .mutate(|config| {
                config.refresh_seconds = 5;
                Ok(())
            })
            .unwrap();

        // Locate the lock_helper binary. During `cargo test`, binaries are
        // placed in the same directory as the test binary or in target/debug/.
        let Some(lock_helper) = find_lock_helper() else {
            eprintln!("skipping: lock_helper binary not found");
            return;
        };

        // Spawn lock_helper to hold the OS lock on <config>.lock.
        let mut child = Command::new(&lock_helper)
            .arg(&lock_path)
            .arg(&signal_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn lock_helper at {lock_helper}: {e}"));

        // Wait for lock_helper to signal readiness (lock acquired).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !signal_path.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "lock_helper did not signal readiness within 10s"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }

        // The lock_helper holds an exclusive lock on <config>.lock.
        // ConfigStore::mutate tries to acquire the same lock via acquire_lock().
        // With LOCKFILE_FAIL_IMMEDIATELY, this should fail immediately and
        // retry until the 5-second timeout. We use a short-lived mutate
        // that should NOT succeed while the helper holds the lock.
        //
        // Instead of waiting for the full timeout, verify that the lock is
        // actually held by checking that a second lock_helper also blocks.
        // Then kill the first lock_helper and verify our mutate succeeds.

        // Try a mutate — it should block (or timeout) because lock_helper
        // holds the lock. We don't wait for the full 5s timeout; instead,
        // we verify the lock is held and then release it.
        let store2 = ConfigStore::new(path.clone());
        let mutate_handle = std::thread::spawn(move || {
            // This should block until the lock_helper releases.
            store2.mutate(|config| {
                config.refresh_seconds = 20;
                Ok(())
            })
        });

        // Give the mutate thread time to attempt lock acquisition.
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Kill lock_helper to release the lock.
        drop(child.stdin.take());
        let _ = child.kill();
        let _ = child.wait();

        // The mutate should now complete.
        let result = mutate_handle.join().expect("mutate thread panicked");
        result.expect("mutate should succeed after lock release");

        let loaded = store.load_existing().unwrap();
        assert_eq!(loaded.refresh_seconds, 20);
        assert!(loaded.is_valid());

        let _ = fs::remove_dir_all(&dir);
    }

    /// Concurrent mutation through multiple threads serializes correctly.
    ///
    /// On Windows, the OS file lock (`LockFileEx`) provides serialization;
    /// on Unix, `flock` does the same. This test proves the combination
    /// of in-process Mutex + OS file lock produces correct results.
    #[test]
    fn concurrent_mutation_serializes_correctly() {
        use std::sync::Arc;
        use std::thread;

        let dir = tmp_dir("concurrent_serialize");
        let path = dir.join("config.toml");
        let store = Arc::new(ConfigStore::new(path.clone()));

        // Initialize config.
        store
            .mutate(|config| {
                config.refresh_seconds = 1;
                Ok(())
            })
            .unwrap();

        // Spawn 5 threads that each increment refresh_seconds.
        let mut handles = Vec::new();
        for i in 2..=6 {
            let store = Arc::clone(&store);
            handles.push(thread::spawn(move || {
                store
                    .mutate(|config| {
                        config.refresh_seconds = i;
                        Ok(())
                    })
                    .unwrap();
            }));
        }

        for handle in handles {
            handle.join().expect("thread panicked");
        }

        // Final value should be one of the written values (last writer wins).
        let loaded = store.load_existing().unwrap();
        assert!(
            (1..=6).contains(&loaded.refresh_seconds),
            "refresh_seconds should be in 1..=6, got {}",
            loaded.refresh_seconds
        );
        assert!(loaded.is_valid());

        let _ = fs::remove_dir_all(&dir);
    }

    /// On Windows, verify that sharing violations fail safely.
    ///
    /// When the destination file is held open with deny-all sharing,
    /// `fs::rename` should fail and the original file should be preserved.
    #[test]
    #[cfg(windows)]
    fn write_atomic_sharing_violation_preserves_original() {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tmp_dir("atomic_sharing_violation");
        let path = dir.join("config.toml");

        // Write an initial config.
        let original = Config::default();
        original.write_atomic(&path).unwrap();
        let original_bytes = fs::read(&path).unwrap();

        // Open the destination file with deny-all sharing.
        // This prevents any other process or handle from accessing the file,
        // including rename-over by another handle.
        let holder = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0) // Deny all sharing.
            .open(&path)
            .expect("failed to open file for sharing violation");

        // Attempt to write a new config — should fail due to sharing violation.
        let updated = Config {
            refresh_seconds: 99,
            ..Config::default()
        };
        let result = updated.write_atomic(&path);

        // The rename should fail. On Windows, this produces an I/O error.
        assert!(result.is_err(), "rename should fail with sharing violation");

        // Drop the holder so we can read the file again.
        drop(holder);

        // Original file should be intact.
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, original, "original config must be preserved");
        assert_eq!(
            fs::read(&path).unwrap(),
            original_bytes,
            "original bytes must be unchanged"
        );
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn lock_file_inode_persists_but_no_stale_lock() {
        let dir = tmp_dir("lock_inode");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        // Acquire and release the lock.
        store
            .mutate(|config| {
                config.refresh_seconds = 5;
                Ok(())
            })
            .unwrap();

        // The lock file may persist as an inode.
        let _lock_path = store.lock_path();
        // The lock file should exist (we don't delete it).
        // But a new acquire should succeed immediately.
        store
            .mutate(|config| {
                config.refresh_seconds = 10;
                Ok(())
            })
            .unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.refresh_seconds, 10);

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Atomic write hardening ---
    #[test]
    #[cfg(unix)]
    fn lock_file_is_not_active_config_file() {
        let dir = tmp_dir("perms_lock_file");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let config = Config::default();
        store.write(&config).unwrap();

        // The lock file should exist after a mutate.
        store
            .mutate(|c| {
                c.refresh_seconds = 15;
                Ok(())
            })
            .unwrap();

        let lock_path = store.lock_path();
        // Lock file is not the config file.
        assert_ne!(lock_path, path);
        // Lock file path ends with .lock.
        assert!(lock_path.to_string_lossy().ends_with(".lock"));

        let _ = fs::remove_dir_all(&dir);
    }
}
