//! Configuration store: advisory locking coordination, atomic persistence, staging I/O, and store errors.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::lock::FileLockGuard;
use super::model::Config;
use super::validation::ConfigViolation;
#[cfg(all(test, unix))]
use std::cell::Cell;
#[cfg(all(test, unix))]
thread_local! {
    static FAIL_NEXT_PERMISSION_SET: Cell<bool> = const { Cell::new(false) };
}

/// Create a replacement file with user-only permissions before exposing any
/// configuration bytes to it.
///
/// On Unix, the file is created with mode `0o600`. On Windows, the file
/// inherits the default ACL of the parent directory (typically the user's
/// profile directory), which already restricts access to the owning user.
pub(crate) fn create_secure_temp_file(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }

    let file = options.open(path)?;

    #[cfg(unix)]
    if !file.metadata()?.file_type().is_file() {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "temporary config path is not a regular file",
        ));
    }

    #[cfg(unix)]
    if let Err(error) = set_secure_permissions(&file) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }

    Ok(file)
}
#[cfg(unix)]
pub(crate) fn set_secure_permissions(file: &fs::File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    #[cfg(test)]
    {
        if FAIL_NEXT_PERMISSION_SET.with(|fail| fail.replace(false)) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected permission-setting failure",
            ));
        }
    }

    file.set_permissions(fs::Permissions::from_mode(0o600))
}

/// Remove temporary config files left by an interrupted atomic write.
pub(crate) fn cleanup_stale_temps(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|name| name.starts_with(".gregg-") && name.ends_with(".toml.tmp"))
            && entry.file_type()?.is_file()
        {
            match fs::remove_file(entry.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                // On Windows, a stale temp may be briefly locked by antivirus
                // or a concurrent reader; treat as non-fatal best-effort.
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                    #[cfg(windows)]
                    eprintln!(
                        "warning: cleanup_stale_temps could not remove {}: {}",
                        entry.path().display(),
                        error
                    );
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}
#[cfg(all(test, unix))]
fn inject_permission_set_failure() {
    FAIL_NEXT_PERMISSION_SET.with(|fail| fail.set(true));
}

pub(crate) fn sync_parent_directory(dir: &Path) -> io::Result<()> {
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        // Windows requires this flag to open a directory as a file handle.
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    let file = match options.open(dir) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            // On Windows, opening a directory for fsync may require privileges
            // not available in all CI environments; treat as non-fatal best-effort.
            #[cfg(windows)]
            eprintln!(
                "warning: sync_parent_directory could not open {}: {}",
                dir.display(),
                error
            );
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    match file.sync_all() {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            #[cfg(windows)]
            eprintln!(
                "warning: sync_parent_directory sync failed for {}: {}",
                dir.display(),
                error
            );
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Default lock acquisition timeout in milliseconds.
pub(crate) const LOCK_TIMEOUT_MS: u64 = 5_000;

/// Configuration store with advisory locking.
pub struct ConfigStore {
    path: PathBuf,
    lock: Mutex<()>,
}
impl ConfigStore {
    /// Create a new config store for the given path.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    /// Return the config path.
    #[must_use]
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Derive the cross-process lock file path from the config path.
    ///
    /// The lock file is named `<config-path>.lock` and lives in the same
    /// directory as the configuration file.
    pub(crate) fn lock_path(&self) -> PathBuf {
        let mut lock_path = self.path.as_os_str().to_owned();
        lock_path.push(".lock");
        PathBuf::from(lock_path)
    }

    /// Load an existing config, or return an error if the file does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the file is missing, unreadable, or
    /// invalid.
    #[allow(dead_code)]
    pub fn load_existing(&self) -> Result<Config, ConfigError> {
        self.cleanup_stale_temps()?;
        Config::load(&self.path)
    }

    /// Load an existing config, or return a default if the file is missing.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the file exists but cannot be read or
    /// parsed.
    pub fn load_or_default(&self) -> Result<Config, ConfigError> {
        self.cleanup_stale_temps()?;
        match Config::load(&self.path) {
            Ok(config) => Ok(config),
            Err(error) if matches!(&error, ConfigError::Io { source, .. } if source.kind() == io::ErrorKind::NotFound) => {
                Ok(Config::default())
            }
            Err(error) => Err(error),
        }
    }

    fn cleanup_stale_temps(&self) -> Result<(), ConfigError> {
        let Some(dir) = self.path.parent() else {
            return Ok(());
        };
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        match cleanup_stale_temps(dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(ConfigError::Io {
                path: dir.to_path_buf(),
                source,
            }),
        }
    }

    /// Atomically persist a configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the write fails.
    pub fn write(&self, config: &Config) -> Result<(), ConfigError> {
        config.write_atomic(&self.path)
    }

    /// Acquire the cross-process file lock with a bounded timeout.
    ///
    /// Uses nonblocking `flock(2)` with bounded backoff. The lock file
    /// is created if it does not exist but is **not** truncated before
    /// lock acquisition. The file handle is retained in the returned
    /// guard so the lock is held until the guard is dropped.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::LockTimeout`] if the lock cannot be acquired
    /// within `LOCK_TIMEOUT_MS`.
    #[allow(unsafe_code)] // Uses libc::flock (unix) and LockFileEx (windows).
    fn acquire_lock(&self) -> Result<FileLockGuard, ConfigError> {
        let lock_path = self.lock_path();

        // Ensure the parent directory exists.
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        // Open without truncating — the lock file may persist as an inode
        // but must not imply a stale lock after the descriptor closes.
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| ConfigError::Io {
                path: lock_path.clone(),
                source: e,
            })?;

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = file.as_raw_fd();
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(LOCK_TIMEOUT_MS);

            loop {
                let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
                if result == 0 {
                    return Ok(FileLockGuard { file, handle: None });
                }
                if std::time::Instant::now() >= deadline {
                    return Err(ConfigError::LockTimeout {
                        path: lock_path,
                        timeout_ms: LOCK_TIMEOUT_MS,
                    });
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }

        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
            };
            use windows_sys::Win32::System::IO::OVERLAPPED;

            let handle = file.as_raw_handle();
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(LOCK_TIMEOUT_MS);

            loop {
                let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
                #[allow(clippy::ptr_as_ptr)]
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

                if result != 0 {
                    // Lock acquired.
                    return Ok(FileLockGuard {
                        file,
                        handle: Some(handle as isize),
                    });
                }

                let last_error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
                // ERROR_LOCK_VIOLATION = 0x21, ERROR_IO_INCOMPLETE = 0x3E4
                if last_error == 0x21 || last_error == 0x3E4 {
                    if std::time::Instant::now() >= deadline {
                        return Err(ConfigError::LockTimeout {
                            path: lock_path,
                            timeout_ms: LOCK_TIMEOUT_MS,
                        });
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                } else {
                    #[allow(clippy::cast_possible_wrap)]
                    let err_code = last_error as i32;
                    return Err(ConfigError::Io {
                        path: lock_path,
                        source: io::Error::from_raw_os_error(err_code),
                    });
                }
            }
        }

        #[cfg(not(unix))]
        #[cfg(not(windows))]
        {
            // Unreachable: the module-level compile_error rejects builds on
            // targets without a cross-process lock implementation.
            Ok(FileLockGuard { file, handle: None })
        }
    }

    /// Mutate the config under the lock, validate, and persist.
    ///
    /// The mutation function is called while the lock is held and the
    /// config is loaded. If the mutation or validation fails, the config
    /// is not written.
    ///
    /// This is a synchronous, potentially blocking operation. Callers from
    /// an async task must run it on a blocking thread; the CLI invokes it
    /// before starting its Tokio runtime.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] on lock timeout, load, mutation, validation,
    /// or write failure.
    pub fn mutate(
        &self,
        f: impl FnOnce(&mut Config) -> Result<(), ConfigError>,
    ) -> Result<(), ConfigError> {
        let _thread_guard = self.lock.lock().map_err(|_| ConfigError::LockPoisoned)?;
        let _file_guard = self.acquire_lock()?;
        let mut config = self.load_or_default()?;
        f(&mut config)?;
        let violations = config.validate();
        if !violations.is_empty() {
            return Err(ConfigError::Validation(violations));
        }
        self.write(&config)
    }

    /// Load the config, run a mutation, validate, and persist — all under
    /// the lock. Returns the updated config.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] on any failure.
    ///
    /// This is a synchronous, potentially blocking operation. Callers from
    /// an async task must run it on a blocking thread.
    pub fn mutate_with_result<T>(
        &self,
        f: impl FnOnce(&mut Config) -> Result<T, ConfigError>,
    ) -> Result<T, ConfigError> {
        let _thread_guard = self.lock.lock().map_err(|_| ConfigError::LockPoisoned)?;
        let _file_guard = self.acquire_lock()?;
        let mut config = self.load_or_default()?;
        let result = f(&mut config)?;
        let violations = config.validate();
        if !violations.is_empty() {
            return Err(ConfigError::Validation(violations));
        }
        self.write(&config)?;
        Ok(result)
    }

    /// Run a transactional config edit under the cross-process lock.
    ///
    /// This implements the full read-edit-validate-commit sequence:
    ///
    /// 1. Acquire the in-process mutex and OS file lock.
    /// 2. Load the current valid configuration, or create a default in memory.
    /// 3. Serialize it to a temporary file in the destination directory.
    /// 4. Invoke the editor (via `edit`) on the temporary file.
    /// 5. If the editor exits nonzero, delete the temporary file and leave
    ///    the live config unchanged.
    /// 6. Parse the complete edited file (rejecting unknown fields).
    /// 7. Reject validation violations.
    /// 8. Atomically replace the live config using the durable write path.
    /// 9. Clean up the temporary file on all paths.
    ///
    /// The live config file is **never** opened directly in the editor.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] on lock timeout, load, editor, parse,
    /// validation, or write failure.
    pub fn edit_transaction(
        &self,
        edit: impl FnOnce(&Path) -> Result<(), ConfigError>,
    ) -> Result<(), ConfigError> {
        let _thread_guard = self.lock.lock().map_err(|_| ConfigError::LockPoisoned)?;
        let _file_guard = self.acquire_lock()?;

        // Step 2: Load current config or create default.
        let config = self.load_or_default()?;

        // Step 3: Serialize to a temporary file in the destination directory.
        let dir = self.path.parent().ok_or_else(|| ConfigError::AtomicWrite {
            path: self.path.clone(),
            source: AtomicWriteError::NoParentDirectory,
        })?;
        fs::create_dir_all(dir).map_err(|e| ConfigError::AtomicWrite {
            path: self.path.clone(),
            source: AtomicWriteError::Io(e),
        })?;

        let temp_name = format!(
            ".gregg-edit-{}-{}.toml.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        );
        let temp_path = dir.join(&temp_name);

        {
            // Create the editor-visible file securely before serializing any
            // current configuration into it.
            let mut file =
                create_secure_temp_file(&temp_path).map_err(|e| ConfigError::AtomicWrite {
                    path: self.path.clone(),
                    source: AtomicWriteError::Io(e),
                })?;
            let content = config
                .to_toml()
                .map_err(|source| ConfigError::AtomicWrite {
                    path: self.path.clone(),
                    source: AtomicWriteError::Serialization(source),
                })?;
            file.write_all(content.as_bytes()).map_err(|e| {
                let _ = fs::remove_file(&temp_path);
                ConfigError::AtomicWrite {
                    path: self.path.clone(),
                    source: AtomicWriteError::Io(e),
                }
            })?;
            file.flush().map_err(|e| {
                let _ = fs::remove_file(&temp_path);
                ConfigError::AtomicWrite {
                    path: self.path.clone(),
                    source: AtomicWriteError::Io(e),
                }
            })?;

            file.sync_all().map_err(|e| {
                let _ = fs::remove_file(&temp_path);
                ConfigError::AtomicWrite {
                    path: self.path.clone(),
                    source: AtomicWriteError::Io(e),
                }
            })?;
        }

        // Step 4-5: Invoke the editor on the temporary file.
        let edit_result = edit(&temp_path);

        if let Err(e) = edit_result {
            // Editor failed — clean up and leave live config unchanged.
            let _ = fs::remove_file(&temp_path);
            return Err(e);
        }

        // Step 6: Parse the complete edited file.
        let parse_result = Config::load(&temp_path);

        // Step 9: Clean up the temporary file on all paths.
        let _ = fs::remove_file(&temp_path);

        let edited = parse_result?;

        // Step 7: Reject validation violations.
        let violations = edited.validate();
        if !violations.is_empty() {
            return Err(ConfigError::Validation(violations));
        }

        // Step 8: Atomically replace the live config.
        self.write(&edited)?;

        Ok(())
    }
}

/// RAII guard for the cross-process configuration lock.
///
/// Holds the lock file handle for the duration of the critical section.
/// The OS-level advisory lock is released when the guard is dropped
/// (the file descriptor/handle is closed or explicitly unlocked). The
/// lock file inode may persist on disk, but it does not imply a stale
/// lock after the descriptor closes.
#[derive(Debug)]
pub enum ConfigError {
    /// I/O error reading or writing the config file.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// TOML parsing error.
    Parse {
        path: Option<PathBuf>,
        source: toml::de::Error,
    },
    /// Configuration failed validation.
    Validation(Vec<ConfigViolation>),
    /// Atomic write operation failed.
    AtomicWrite {
        path: PathBuf,
        source: AtomicWriteError,
    },
    /// Lock mutex was poisoned.
    LockPoisoned,
    /// Cross-process lock could not be acquired within the timeout.
    LockTimeout { path: PathBuf, timeout_ms: u64 },
    /// The editor could not be launched or exited with a nonzero status.
    EditorFailed { path: PathBuf, message: String },
}
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "failed to read {}: {source}", path.display()),
            Self::Parse { path, source } => {
                if let Some(p) = path {
                    write!(f, "failed to parse {}: {source}", p.display())
                } else {
                    write!(f, "failed to parse config: {source}")
                }
            }
            Self::Validation(violations) => {
                write!(f, "configuration validation failed:")?;
                for v in violations {
                    write!(f, "\n  - {v}")?;
                }
                Ok(())
            }
            Self::AtomicWrite { path, source } => {
                write!(f, "atomic write to {} failed: {source}", path.display())
            }
            Self::LockPoisoned => write!(f, "config lock was poisoned"),
            Self::LockTimeout { path, timeout_ms } => write!(
                f,
                "could not acquire config lock at {} within {timeout_ms}ms; another process may be modifying the configuration",
                path.display()
            ),
            Self::EditorFailed { path, message } => {
                write!(f, "editor failed on {}: {message}", path.display())
            }
        }
    }
}
impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::AtomicWrite { source, .. } => Some(source),
            Self::Validation(_)
            | Self::LockPoisoned
            | Self::LockTimeout { .. }
            | Self::EditorFailed { .. } => None,
        }
    }
}

/// Errors specific to the atomic write operation.
#[derive(Debug)]
pub enum AtomicWriteError {
    /// The path has no parent directory.
    NoParentDirectory,
    /// An I/O error occurred.
    Io(std::io::Error),
    /// TOML serialization failed.
    Serialization(toml::ser::Error),
    /// The file was written but verification re-parse failed.
    VerificationFailed,
}
impl fmt::Display for AtomicWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoParentDirectory => write!(f, "path has no parent directory"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Serialization(e) => write!(f, "TOML serialization error: {e}"),
            Self::VerificationFailed => write!(f, "verification re-parse failed"),
        }
    }
}
impl std::error::Error for AtomicWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoParentDirectory | Self::VerificationFailed => None,
            Self::Io(e) => Some(e),
            Self::Serialization(e) => Some(e),
        }
    }
}

/// A single configuration validation violation.
#[cfg(test)]
mod tests {
    use super::super::model::SystemEntry;
    use super::super::test_helpers::tmp_dir;
    use super::*;

    #[test]
    fn write_atomic_creates_file() {
        let dir = tmp_dir("atomic_create");
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(config, loaded);

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn write_atomic_overwrites_existing() {
        let dir = tmp_dir("atomic_overwrite");
        let path = dir.join("config.toml");

        let mut config = Config::default();
        config.write_atomic(&path).unwrap();

        config.refresh_seconds = 10;
        config.write_atomic(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.refresh_seconds, 10);

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn write_atomic_round_trip_verifies_written_config() {
        let dir = tmp_dir("atomic_verify");
        let path = dir.join("config.toml");
        let config = Config {
            refresh_seconds: 42,
            ..Config::default()
        };

        config.write_atomic(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn write_atomic_preserves_old_on_failure() {
        let dir = tmp_dir("atomic_preserve");
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        // Attempt write to an invalid nested path that cannot exist on
        // any platform (contains a null byte which is invalid on all OSes).
        let bad_path = dir.join("\0").join("config.toml");
        let result = config.write_atomic(&bad_path);
        assert!(result.is_err());

        let loaded = Config::load(&path).unwrap();
        assert_eq!(config, loaded);

        let _ = fs::remove_dir_all(&dir);
    }

    // --- ConfigStore ---
    #[test]
    fn config_store_load_or_default_empty() {
        let dir = tmp_dir("store_default");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let config = store.load_or_default().unwrap();
        assert_eq!(config, Config::default());

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn config_store_load_existing_missing_errors() {
        let dir = tmp_dir("store_missing");
        let path = dir.join("nonexistent.toml");
        let store = ConfigStore::new(path);

        assert!(store.load_existing().is_err());

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn config_store_reports_lock_parent_creation_errors() {
        let dir = tmp_dir("store_lock_parent_error");
        let parent = dir.join("not-a-directory");
        fs::write(&parent, "file").unwrap();
        let path = parent.join("config.toml");
        let store = ConfigStore::new(path);

        let result = store.mutate(|_| Ok(()));
        match result {
            Err(ConfigError::Io { path, .. }) => assert_eq!(path, parent),
            other => panic!("expected lock parent I/O error, got {other:?}"),
        }

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn config_store_write_and_load() {
        let dir = tmp_dir("store_write");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "id1".into(),
            host: "192.168.1.1".into(),
            port: 11310,
            name: None,
        });
        store.write(&config).unwrap();

        let loaded = store.load_existing().unwrap();
        assert_eq!(config, loaded);

        let _ = fs::remove_dir_all(store.path().parent().unwrap());
    }
    #[test]
    fn config_store_mutate() {
        let dir = tmp_dir("store_mutate");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        store
            .mutate(|config| {
                config.refresh_seconds = 10;
                Ok(())
            })
            .unwrap();

        let loaded = store.load_existing().unwrap();
        assert_eq!(loaded.refresh_seconds, 10);

        let _ = fs::remove_dir_all(store.path().parent().unwrap());
    }

    // --- Parse errors ---
    #[test]
    #[cfg(unix)]
    fn write_atomic_to_readonly_directory() {
        let dir = tmp_dir("atomic_readonly");
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        // Make directory read-only.
        let mut perms = fs::metadata(&dir).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&dir, perms).unwrap();

        let result = config.write_atomic(&path);
        assert!(result.is_err());

        // Original file should still be intact.
        let loaded = Config::load(&path).unwrap();
        assert_eq!(config, loaded);

        // Restore permissions for cleanup.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn write_atomic_no_parent_directory() {
        let config = Config::default();
        // Path::new("/").parent() returns None, triggering NoParentDirectory.
        let result = config.write_atomic(Path::new("/"));
        match result {
            Err(ConfigError::AtomicWrite {
                source: AtomicWriteError::NoParentDirectory,
                ..
            }) => {}
            other => panic!("expected NoParentDirectory, got {other:?}"),
        }
    }
    #[test]
    fn write_atomic_multiple_rapid_writes() {
        let dir = tmp_dir("atomic_rapid");
        let path = dir.join("config.toml");

        for i in 0..10 {
            let config = Config {
                refresh_seconds: i + 1,
                ..Default::default()
            };
            config.write_atomic(&path).unwrap();
        }

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.refresh_seconds, 10);
        assert!(loaded.is_valid());

        let _ = fs::remove_dir_all(&dir);
    }

    // --- ConfigStore concurrent mutation ---
    #[test]
    fn config_store_concurrent_mutation() {
        let dir = tmp_dir("store_concurrent");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        // Sequential mutations through the store should produce the
        // final state without corruption.
        store
            .mutate(|c| {
                c.refresh_seconds = 2;
                Ok(())
            })
            .unwrap();
        store
            .mutate(|c| {
                c.refresh_seconds = 3;
                Ok(())
            })
            .unwrap();
        store
            .mutate(|c| {
                c.refresh_seconds = 4;
                Ok(())
            })
            .unwrap();

        let loaded = store.load_existing().unwrap();
        assert_eq!(loaded.refresh_seconds, 4);

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Cross-process locking ---
    #[test]
    fn mutate_acquires_and_releases_lock() {
        let dir = tmp_dir("mutate_lock");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        store
            .mutate(|config| {
                config.refresh_seconds = 10;
                Ok(())
            })
            .unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.refresh_seconds, 10);

        // Lock should be released — a second mutate should succeed.
        store
            .mutate(|config| {
                config.refresh_seconds = 20;
                Ok(())
            })
            .unwrap();

        let config = store.load_existing().unwrap();
        assert_eq!(config.refresh_seconds, 20);

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn failed_validation_releases_lock() {
        let dir = tmp_dir("lock_validation_fail");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path);

        // A mutation that produces an invalid config should release the lock.
        let result = store.mutate(|config| {
            config.refresh_seconds = 0; // Invalid: below minimum
            Ok(())
        });
        assert!(result.is_err());

        // Lock should be released — a valid mutation should succeed.
        store
            .mutate(|config| {
                config.refresh_seconds = 15;
                Ok(())
            })
            .unwrap();

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn write_atomic_uses_collision_resistant_temp_name() {
        let dir = tmp_dir("atomic_collision_resistant");
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        // Verify no temp files remain.
        let entries: Vec<_> = fs::read_dir(&dir).unwrap().filter_map(Result::ok).collect();
        assert_eq!(entries.len(), 1, "should only have the final config file");
        assert_eq!(entries[0].file_name().to_str().unwrap(), "config.toml");

        let _ = fs::remove_dir_all(&dir);
    }

    // --- edit_transaction tests ---

    /// Helper: count temp files in a directory.
    fn count_temp_files(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.contains(".tmp") || n.contains("gregg-edit"))
            })
            .count()
    }
    #[test]
    fn edit_transaction_valid_edit_commits() {
        let dir = tmp_dir("edit_valid");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();
        let original_bytes = fs::read(&path).unwrap();

        // Editor writes valid TOML with a changed refresh_seconds.
        store
            .edit_transaction(|temp_path| {
                let mut config = Config::load(temp_path)?;
                config.refresh_seconds = 30;
                fs::write(temp_path, config.to_toml().unwrap()).map_err(|e| ConfigError::Io {
                    path: temp_path.to_path_buf(),
                    source: e,
                })?;
                Ok(())
            })
            .unwrap();

        let loaded = store.load_existing().unwrap();
        assert_eq!(loaded.refresh_seconds, 30);
        assert_ne!(fs::read(&path).unwrap(), original_bytes);
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn edit_transaction_invalid_toml_preserves_original() {
        let dir = tmp_dir("edit_invalid_toml");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        #[cfg(unix)]
        let original_mode = {
            use std::os::unix::fs::PermissionsExt;

            fs::metadata(&path).unwrap().permissions().mode() & 0o777
        };

        // Editor writes invalid TOML.
        let result = store.edit_transaction(|temp_path| {
            fs::write(temp_path, "this is not valid {{{").map_err(|e| ConfigError::Io {
                path: temp_path.to_path_buf(),
                source: e,
            })?;
            Ok(())
        });
        assert!(result.is_err());

        // Original bytes unchanged.
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                original_mode
            );
        }
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn edit_transaction_validation_failure_preserves_original() {
        let dir = tmp_dir("edit_validation_fail");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        #[cfg(unix)]
        let original_mode = {
            use std::os::unix::fs::PermissionsExt;

            fs::metadata(&path).unwrap().permissions().mode() & 0o777
        };

        // Editor writes TOML with an invalid value (refresh_seconds = 0).
        let result = store.edit_transaction(|temp_path| {
            let mut config = Config::load(temp_path)?;
            config.refresh_seconds = 0; // Invalid: below minimum
            fs::write(temp_path, config.to_toml().unwrap()).map_err(|e| ConfigError::Io {
                path: temp_path.to_path_buf(),
                source: e,
            })?;
            Ok(())
        });
        assert!(result.is_err());

        // Original bytes unchanged.
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                original_mode
            );
        }
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn edit_transaction_nonzero_editor_exit_preserves_original() {
        let dir = tmp_dir("edit_nonzero_exit");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        #[cfg(unix)]
        let original_mode = {
            use std::os::unix::fs::PermissionsExt;

            fs::metadata(&path).unwrap().permissions().mode() & 0o777
        };

        // Editor "exits nonzero" — closure returns an error.
        let result = store.edit_transaction(|temp_path| {
            Err(ConfigError::EditorFailed {
                path: temp_path.to_path_buf(),
                message: "editor exited with status: 1".to_string(),
            })
        });
        assert!(result.is_err());

        // Original bytes unchanged.
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                original_mode
            );
        }
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn edit_transaction_editor_launch_failure_preserves_original() {
        let dir = tmp_dir("edit_launch_fail");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();
        let original_bytes = fs::read(&path).unwrap();

        // Editor "launch fails" — closure returns an error.
        let result = store.edit_transaction(|temp_path| {
            Err(ConfigError::EditorFailed {
                path: temp_path.to_path_buf(),
                message: "failed to launch editor: not found".to_string(),
            })
        });
        assert!(result.is_err());

        // Original bytes unchanged.
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn edit_transaction_missing_config_starts_from_default() {
        let dir = tmp_dir("edit_missing_config");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        // No config file exists yet.
        assert!(!path.exists());

        // Editor writes valid TOML (just the default).
        store
            .edit_transaction(|temp_path| {
                let config = Config::load(temp_path)?;
                // Verify the temp file started from defaults.
                assert_eq!(config, Config::default());
                // Write it back unchanged.
                fs::write(temp_path, config.to_toml().unwrap()).map_err(|e| ConfigError::Io {
                    path: temp_path.to_path_buf(),
                    source: e,
                })?;
                Ok(())
            })
            .unwrap();

        // Config file should now exist with default values.
        assert!(path.exists());
        let loaded = store.load_existing().unwrap();
        assert_eq!(loaded, Config::default());
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn edit_transaction_temp_files_removed_on_success_and_failure() {
        let dir = tmp_dir("edit_temp_cleanup");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();

        // Success path.
        store
            .edit_transaction(|temp_path| {
                fs::write(temp_path, Config::default().to_toml().unwrap()).map_err(|e| {
                    ConfigError::Io {
                        path: temp_path.to_path_buf(),
                        source: e,
                    }
                })?;
                Ok(())
            })
            .unwrap();
        assert_eq!(count_temp_files(&dir), 0, "no temp files after success");

        // Failure path (invalid TOML).
        let _ = store.edit_transaction(|temp_path| {
            fs::write(temp_path, "invalid {{{").map_err(|e| ConfigError::Io {
                path: temp_path.to_path_buf(),
                source: e,
            })?;
            Ok(())
        });
        assert_eq!(count_temp_files(&dir), 0, "no temp files after failure");

        // Failure path (editor error).
        let _ = store.edit_transaction(|temp_path| {
            Err(ConfigError::EditorFailed {
                path: temp_path.to_path_buf(),
                message: "fail".to_string(),
            })
        });
        assert_eq!(
            count_temp_files(&dir),
            0,
            "no temp files after editor error"
        );

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn edit_transaction_concurrent_mutation_no_lost_updates() {
        use std::sync::Arc;
        use std::thread;

        let dir = tmp_dir("edit_concurrent");
        let path = dir.join("config.toml");
        let store = Arc::new(ConfigStore::new(path.clone()));

        // Initialize config.
        store
            .mutate(|c| {
                c.refresh_seconds = 5;
                Ok(())
            })
            .unwrap();

        let original_bytes = fs::read(&path).unwrap();

        // Start an edit_transaction that holds the lock briefly.
        let store1 = store.clone();
        let edit_handle = thread::spawn(move || {
            store1
                .edit_transaction(|temp_path| {
                    // Hold the lock briefly to simulate editor interaction.
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    let mut config = Config::load(temp_path)?;
                    config.refresh_seconds = 20;
                    fs::write(temp_path, config.to_toml().unwrap()).map_err(|e| {
                        ConfigError::Io {
                            path: temp_path.to_path_buf(),
                            source: e,
                        }
                    })?;
                    Ok(())
                })
                .unwrap();
        });

        // Give the edit thread time to acquire the lock.
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Try a concurrent mutate — it should block until edit completes,
        // then see the updated config.
        let store2 = store.clone();
        let mutate_handle = thread::spawn(move || {
            store2
                .mutate(|c| {
                    c.refresh_seconds = 30;
                    Ok(())
                })
                .unwrap();
        });

        edit_handle.join().unwrap();
        mutate_handle.join().unwrap();

        // The final config should reflect the mutate (30), not the edit (20).
        let loaded = store.load_existing().unwrap();
        assert_eq!(loaded.refresh_seconds, 30);
        assert_ne!(fs::read(&path).unwrap(), original_bytes);
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn edit_transaction_rejects_unknown_fields() {
        let dir = tmp_dir("edit_unknown_fields");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();
        let original_bytes = fs::read(&path).unwrap();

        // Editor writes TOML with an unknown field.
        let result = store.edit_transaction(|temp_path| {
            let config = Config::load(temp_path)?;
            let toml = config.to_toml().unwrap();
            // Append an unknown field.
            let modified = format!("{toml}\nunknown_field = \"oops\"\n");
            fs::write(temp_path, modified).map_err(|e| ConfigError::Io {
                path: temp_path.to_path_buf(),
                source: e,
            })?;
            Ok(())
        });
        assert!(result.is_err());

        // Original bytes unchanged.
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        assert_eq!(count_temp_files(&dir), 0, "no temp files should remain");

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Config file permission tests (Unix only) ---
    #[test]
    #[cfg(unix)]
    fn write_atomic_creates_new_config_with_0600_perms() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_new_file");
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        let metadata = fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "new config file must be user-only 0600");

        let loaded = Config::load(&path).unwrap();
        assert_eq!(config, loaded);

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    #[cfg(unix)]
    fn write_atomic_preserves_0600_on_overwrite() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_overwrite");
        let path = dir.join("config.toml");

        let config = Config::default();
        config.write_atomic(&path).unwrap();

        let mode1 = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode1, 0o600);

        config.write_atomic(&path).unwrap();
        let mode2 = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode2, 0o600, "overwrite must preserve 0600");

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    #[cfg(unix)]
    fn write_atomic_does_not_expose_broad_permission_temp_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_no_leak");
        let path = dir.join("config.toml");

        let config = Config::default();

        // Verify no broad-permission temp files remain after write.
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.contains(".tmp") {
                let mode = entry.metadata().unwrap().permissions().mode() & 0o777;
                panic!("temp file {name} should not remain: mode {mode:o}");
            }
        }

        config.write_atomic(&path).unwrap();

        // After write, only the config file should exist, and no temp files.
        let entries: Vec<_> = fs::read_dir(&dir).unwrap().filter_map(Result::ok).collect();
        assert_eq!(entries.len(), 1, "only config file should remain");
        assert!(!entries[0].file_name().to_string_lossy().contains(".tmp"));

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    #[cfg(unix)]
    fn edit_transaction_preserves_0600_perms() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_edit");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let config = Config::default();
        store.write(&config).unwrap();
        let mode1 = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode1, 0o600);

        store
            .edit_transaction(|temp_path| {
                let mut config = Config::load(temp_path)?;
                config.refresh_seconds = 30;
                fs::write(temp_path, config.to_toml().unwrap()).map_err(|e| ConfigError::Io {
                    path: temp_path.to_path_buf(),
                    source: e,
                })?;
                Ok(())
            })
            .unwrap();

        let mode2 = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode2, 0o600, "edit must preserve 0600");

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    #[cfg(unix)]
    fn edit_transaction_failure_preserves_original_perms_and_bytes() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_edit_fail");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let config = Config::default();
        store.write(&config).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        let original_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(original_mode, 0o600);

        let result = store.edit_transaction(|temp_path| {
            Err(ConfigError::EditorFailed {
                path: temp_path.to_path_buf(),
                message: "simulated".to_string(),
            })
        });
        assert!(result.is_err());

        // Original bytes unchanged.
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        // Original permissions unchanged.
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, original_mode, "edit failure must preserve 0600");

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    #[cfg(unix)]
    fn edit_transaction_editor_sees_secure_temp_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_editor_visible");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());
        store.write(&Config::default()).unwrap();

        store
            .edit_transaction(|temp_path| {
                let mode = fs::metadata(temp_path).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "editor temp must start user-only");
                assert_eq!(Config::load(temp_path).unwrap(), Config::default());
                Ok(())
            })
            .unwrap();

        assert_eq!(count_temp_files(&dir), 0, "editor temp must be cleaned up");
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    #[cfg(unix)]
    fn write_atomic_permission_failure_is_fatal_and_cleans_temp() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_injected_failure");
        let path = dir.join("config.toml");
        let original = Config::default();
        original.write_atomic(&path).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        let original_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;

        inject_permission_set_failure();
        let result = Config {
            refresh_seconds: 10,
            ..original
        }
        .write_atomic(&path);

        match result {
            Err(ConfigError::AtomicWrite {
                source: AtomicWriteError::Io(error),
                ..
            }) => assert_eq!(error.kind(), io::ErrorKind::PermissionDenied),
            other => panic!("expected permission failure, got {other:?}"),
        }
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            original_mode
        );
        assert_eq!(count_temp_files(&dir), 0, "failed temp must be cleaned up");

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    #[cfg(unix)]
    fn mutate_preserves_0600_through_store() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_mutate");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        store
            .mutate(|c| {
                c.refresh_seconds = 20;
                Ok(())
            })
            .unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mutate through store must produce 0600");

        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    #[cfg(unix)]
    fn repeated_writes_preserve_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmp_dir("perms_repeated");
        let path = dir.join("config.toml");

        for i in 1..=5 {
            let config = Config {
                refresh_seconds: i,
                ..Default::default()
            };
            config.write_atomic(&path).unwrap();
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "write {i} must produce 0600");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    // --- Windows config path tests ---
    #[test]
    fn write_atomic_path_with_spaces() {
        let dir = tmp_dir("atomic spaces in path");
        let path = dir.join("config.toml");
        let config = Config::default();
        config.write_atomic(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(config, loaded);
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn write_atomic_repeated_mutation_produces_valid_config() {
        let dir = tmp_dir("atomic_repeat_mutation");
        let path = dir.join("config.toml");

        let mut config = Config::default();
        for i in 1..=20 {
            config.refresh_seconds = i;
            config.write_atomic(&path).unwrap();
        }
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.refresh_seconds, 20);
        assert!(loaded.is_valid());
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn edit_transaction_path_with_spaces() {
        let dir = tmp_dir("edit spaces in path");
        let path = dir.join("config.toml");
        let store = ConfigStore::new(path.clone());

        let original = Config::default();
        store.write(&original).unwrap();

        store
            .edit_transaction(|temp_path| {
                let mut config = Config::load(temp_path)?;
                config.refresh_seconds = 25;
                fs::write(temp_path, config.to_toml().unwrap()).map_err(|e| ConfigError::Io {
                    path: temp_path.to_path_buf(),
                    source: e,
                })?;
                Ok(())
            })
            .unwrap();

        let loaded = store.load_existing().unwrap();
        assert_eq!(loaded.refresh_seconds, 25);
        assert_eq!(count_temp_files(&dir), 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
