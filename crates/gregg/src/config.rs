//! Client configuration, validation, file I/O, atomic persistence, and
//! advisory locking.
//!
//! Configuration is stored as canonical TOML and validated before every
//! load and before every mutation. Atomic writes ensure a partially written
//! file can never corrupt the client state.
//!
//! Ownership split (Plan 105, behavior-preserving):
//!
//! - [`model`] — entries, limits, defaults, load/validate/write primitives;
//! - [`store`] — store coordination, atomic persistence, staging I/O, errors;
//! - [`validation`] — violation kinds and field checks;
//! - [`lock`] — cross-process advisory file locking.
//!
//! The re-exports below preserve the historical `crate::config::X` paths
//! so no call site changes with the move.

pub mod lock;
pub mod model;
pub mod store;
pub mod validation;

pub use lock::FileLockGuard;
pub use model::{
    Config, EggpoolEntry, EggpoolScheme, SystemEntry, DEFAULT_EGGPOOL_PORT,
    MAX_CONCURRENT_REQUESTS, MAX_EGGPOOL_NAME_LEN, MAX_ENV_NAME_LEN, MAX_PORT, MAX_REFRESH_SECONDS,
    MAX_REQUEST_TIMEOUT_MS, MIN_PORT, MIN_REFRESH_SECONDS, MIN_REQUEST_TIMEOUT_MS,
    SUPPORTED_CONFIG_VERSION,
};
pub use store::{AtomicWriteError, ConfigError, ConfigStore};
pub use validation::ConfigViolation;

#[cfg(test)]
pub(crate) mod test_helpers {
    //! Shared test helpers for the config modules (single implementation so
    //! the split does not duplicate fixture scaffolding).
    use std::path::PathBuf;

    /// Isolated temp dir for one test.
    pub(crate) fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gregg_test_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Locate the `lock_helper` binary for cross-process tests.
    ///
    /// During `cargo test`, the binary is in `target/debug/` or
    /// `target/debug/deps/`. This function searches upward from the test
    /// binary's directory to handle varying CI layouts.
    ///
    /// Returns `None` if the binary is not found (e.g., on CI runners
    /// where `[[bin]]` targets may not be compiled for all platforms).
    pub(crate) fn find_lock_helper() -> Option<String> {
        let exe_dir = std::env::current_exe()
            .expect("current_exe should succeed")
            .parent()
            .expect("exe should have a parent")
            .to_path_buf();

        let binary_name = if cfg!(windows) {
            "lock_helper.exe"
        } else {
            "lock_helper"
        };

        // Search upward from the test binary's directory (up to 5 levels).
        let mut search_dir = exe_dir.clone();
        for _ in 0..5 {
            let candidate = search_dir.join(binary_name);
            if candidate.exists() {
                return Some(candidate.to_string_lossy().into_owned());
            }
            if !search_dir.pop() {
                break;
            }
        }

        None
    }
}
