//! Plan 164: config-specific identity for the Gregg client daemon.
//!
//! The client daemon is **per configuration file**, not per machine. Two
//! operators (or one operator with two configs) on the same host each get their
//! own daemon, their own poll cadence, and their own `Ctrl-R` reload boundary.
//! Everything addressable is therefore derived from the resolved config path
//! alone — never from host, port, the current PID, the clock, or a random
//! source — so `gregg daemon` and the TUI always agree on where to look.
//!
//! This is the same identity discipline `greggd` already uses for its control
//! socket: a 64-bit FNV-1a digest of a *normalized* config path, rendered as 16
//! lowercase hex characters. `DefaultHasher` is deliberately not used, because
//! its algorithm is not a compatibility contract across Rust releases.

use std::path::{Path, PathBuf};

/// Unix `sockaddr_un.sun_path` is 108 bytes including the NUL terminator, so a
/// socket path must leave room for the whole name. Anything longer is reported
/// as unbindable and the caller falls back to the system temp directory.
///
/// Windows has no equivalent constraint: the name is a pipe name, not a socket
/// path, and is bounded only by the fixed digest length below.
#[cfg(unix)]
const UNIX_PATH_MAX: usize = 100;

/// Normalized identity for exactly one config path.
///
/// The digest and the parent directory are computed together so they can never
/// disagree: re-normalizing per candidate would let a file created or deleted
/// between calls flip between the canonical and the lexical `current_dir()`
/// branch, and a differing working directory for `daemon run` versus the TUI
/// would otherwise produce two `<id>` values for one spelling of the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientDaemonIdentity {
    /// 16 lowercase hex characters.
    id: String,
    normalized: PathBuf,
}

impl ClientDaemonIdentity {
    /// Build the identity for a config path.
    #[must_use]
    pub fn for_path(config_path: &Path) -> Self {
        const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

        let normalized = normalize(config_path);
        let mut hash = FNV_OFFSET_BASIS;
        for byte in normalized.as_os_str().as_encoded_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        Self {
            id: format!("{hash:016x}"),
            normalized,
        }
    }

    /// The 16-hex-character identity digest.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The normalized config path this identity was derived from.
    #[must_use]
    pub fn normalized_config_path(&self) -> &Path {
        &self.normalized
    }

    /// Socket file name, shared by the primary and fallback locations so the
    /// two always agree on identity.
    #[cfg(unix)]
    fn socket_file_name(&self) -> String {
        format!("gregg-client-{}.sock", self.id)
    }

    /// Preferred location: beside the config file, so an operator can see which
    /// config a socket belongs to.
    #[cfg(unix)]
    pub(crate) fn primary(&self) -> Option<PathBuf> {
        self.join(self.normalized.parent())
    }

    /// Deterministic fallback under the system temp directory, used when the
    /// config's directory is not writable by the current principal (a shared
    /// or read-only config location).
    #[cfg(unix)]
    pub(crate) fn fallback(&self) -> Option<PathBuf> {
        self.join(Some(std::env::temp_dir().as_path()))
    }

    #[cfg(unix)]
    fn join(&self, directory: Option<&Path>) -> Option<PathBuf> {
        let path = directory?.join(self.socket_file_name());
        if path.as_os_str().len() > UNIX_PATH_MAX {
            return None;
        }
        Some(path)
    }

    /// Every location the daemon may have bound, primary first.
    ///
    /// The client always probes both, because it cannot know whether the daemon
    /// found its primary location writable.
    #[cfg(unix)]
    pub(crate) fn candidates(&self) -> Vec<PathBuf> {
        let mut out = Vec::with_capacity(2);
        if let Some(primary) = self.primary() {
            out.push(primary);
        }
        if let Some(fallback) = self.fallback() {
            if !out.contains(&fallback) {
                out.push(fallback);
            }
        }
        out
    }

    /// The single name a Windows client daemon binds.
    ///
    /// A named pipe is not a filesystem entry: `CreateNamedPipeW` requires the
    /// `\\.\pipe\` prefix and rejects anything else, and the whole `\\.\pipe\`
    /// namespace is flat. So there is no "beside the config" location to prefer
    /// and no unwritable-directory fallback to reach for — the identity digest
    /// alone names the pipe, and the config path stays the only input.
    #[cfg(windows)]
    pub(crate) fn candidates(&self) -> Vec<PathBuf> {
        vec![PathBuf::from(format!(r"\\.\pipe\gregg-client-{}", self.id))]
    }
}

/// Normalize a config path for identity.
///
/// An existing path is filesystem-canonicalized so relative, absolute, and
/// symlink spellings of one file converge. A missing path is made absolute and
/// normalized lexically without requiring it to exist, which preserves the
/// implicit-default-config behavior: `gregg` with no `--config` must reach the
/// same daemon as `gregg daemon run` with no `--config`, even before the config
/// file is created.
fn normalize(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let _ = normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

/// 16-hex-character identity digest for a config path.
#[must_use]
pub fn client_daemon_id(config_path: &Path) -> String {
    ClientDaemonIdentity::for_path(config_path).id
}

/// Preferred endpoint path for a config, beside the config file.
///
/// On Windows a named pipe has no filesystem location to prefer, so this is
/// simply the one name the daemon binds.
#[must_use]
pub fn primary_socket_path(config_path: &Path) -> Option<PathBuf> {
    ClientDaemonIdentity::for_path(config_path)
        .candidates()
        .into_iter()
        .next()
}

/// Deterministic fallback endpoint path under the system temp directory.
///
/// Windows named pipes have no directory to fall back to — the `\\.\pipe\`
/// namespace is flat and never unwritable — so this is always `None` there.
#[must_use]
pub fn fallback_socket_path(config_path: &Path) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        ClientDaemonIdentity::for_path(config_path).fallback()
    }
    #[cfg(not(unix))]
    {
        let _ = config_path;
        None
    }
}

/// Every socket path a client daemon for this config may have bound.
#[must_use]
pub fn socket_candidates(config_path: &Path) -> Vec<PathBuf> {
    ClientDaemonIdentity::for_path(config_path).candidates()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_sixteen_lowercase_hex_characters() {
        let identity = ClientDaemonIdentity::for_path(Path::new("/tmp/gregg.toml"));
        assert_eq!(identity.id().len(), 16);
        assert!(identity
            .id()
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
    }

    #[test]
    fn the_same_spelling_always_yields_the_same_identity() {
        let a = client_daemon_id(Path::new("/tmp/gregg-a.toml"));
        let b = client_daemon_id(Path::new("/tmp/gregg-a.toml"));
        assert_eq!(a, b);
    }

    #[test]
    fn different_configs_get_different_identities() {
        let a = client_daemon_id(Path::new("/tmp/gregg-a.toml"));
        let b = client_daemon_id(Path::new("/tmp/gregg-b.toml"));
        assert_ne!(a, b);
    }

    #[test]
    fn a_relative_missing_path_normalizes_to_an_absolute_one() {
        // The config may not exist yet, so identity cannot depend on it. A
        // relative spelling must still resolve against the current directory so
        // two shells in different directories never disagree about one config.
        let relative = ClientDaemonIdentity::for_path(Path::new("gregg-identity-probe/gregg.toml"));
        let normalized = relative.normalized_config_path();
        assert!(normalized.is_absolute(), "{normalized:?} is not absolute");
        assert!(
            normalized.ends_with("gregg-identity-probe/gregg.toml"),
            "{normalized:?}"
        );
        // Repeating the same spelling is stable, which is what a TUI and a
        // separately launched daemon rely on.
        assert_eq!(
            relative.id(),
            ClientDaemonIdentity::for_path(Path::new("gregg-identity-probe/gregg.toml")).id()
        );
    }

    #[test]
    fn identity_does_not_depend_on_config_contents_or_the_clock() {
        // Only the path is hashed, so a host/port edit inside the TOML cannot
        // orphan an already-running daemon.
        let before = client_daemon_id(Path::new("/tmp/gregg-stable.toml"));
        let after = client_daemon_id(Path::new("/tmp/gregg-stable.toml"));
        assert_eq!(before, after);
    }

    #[cfg(unix)]
    #[test]
    fn primary_sits_beside_the_config_and_shares_the_digest() {
        let config = Path::new("/tmp/gregg-adjacent/gregg.toml");
        let identity = ClientDaemonIdentity::for_path(config);
        let primary = identity.primary().expect("primary path");
        assert_eq!(primary.parent(), Some(Path::new("/tmp/gregg-adjacent")));
        let name = primary
            .file_name()
            .and_then(|name| name.to_str())
            .expect("utf-8 file name");
        assert!(name.contains(identity.id()), "{name} must contain the id");
    }

    #[cfg(unix)]
    #[test]
    fn the_fallback_lives_under_the_temp_directory() {
        let identity = ClientDaemonIdentity::for_path(Path::new("/tmp/gregg-adjacent/gregg.toml"));
        let fallback = identity.fallback().expect("fallback path");
        assert_eq!(fallback.parent(), Some(std::env::temp_dir().as_path()));
    }

    #[cfg(unix)]
    #[test]
    fn a_very_long_path_reports_no_unix_socket_location() {
        // A path that cannot fit in `sun_path` must be reported as unbindable
        // rather than truncated into a colliding name.
        let long = PathBuf::from(format!("/tmp/{}/gregg.toml", "d".repeat(200)));
        let identity = ClientDaemonIdentity::for_path(&long);
        assert_eq!(identity.primary(), None);
        // The fallback is short, so one usable candidate always remains.
        assert!(identity.candidates().len() <= 1);
        assert_ne!(identity.candidates().len(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn candidates_start_with_the_primary_and_are_deduplicated() {
        let identity = ClientDaemonIdentity::for_path(Path::new("/tmp/gregg-adjacent/gregg.toml"));
        let candidates = identity.candidates();
        assert_eq!(candidates.first(), identity.primary().as_ref());
        let mut sorted = candidates.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), candidates.len());
    }

    #[cfg(windows)]
    #[test]
    fn the_windows_candidate_is_one_pipe_name_in_the_flat_namespace() {
        let identity = ClientDaemonIdentity::for_path(Path::new(r"C:\Users\op\gregg.toml"));
        let candidates = identity.candidates();
        // `CreateNamedPipeW` rejects any name without the `\\.\pipe\` prefix,
        // and there is no directory to prefer or fall back to, so there is
        // exactly one name and it is derived from the digest alone.
        assert_eq!(candidates.len(), 1);
        let name = candidates[0].to_str().expect("utf-8 pipe name");
        assert!(
            name.starts_with(r"\\.\pipe\gregg-client-"),
            "not a pipe name: {name}"
        );
        assert!(name.ends_with(identity.id()), "{name} must end in the id");
    }

    #[cfg(windows)]
    #[test]
    fn a_very_long_config_path_still_yields_one_usable_pipe_name() {
        // The `sun_path` limit is a Unix constraint. A pipe name is a fixed
        // digest, so a long config path must not cost this platform its
        // endpoint.
        let long = PathBuf::from(format!(r"C:\Users\{}\gregg.toml", "d".repeat(200)));
        let identity = ClientDaemonIdentity::for_path(&long);
        assert_eq!(identity.candidates().len(), 1);
        assert!(fallback_socket_path(&long).is_none());
    }
}
