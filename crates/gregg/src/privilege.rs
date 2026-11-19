//! Effective-privilege detection for user-scoped registration.
//!
//! Client-daemon startup is always user-scoped: a `systemctl --user` unit, a
//! `~/Library/LaunchAgents` plist, a Startup-folder entry, or a managed user
//! crontab. A root/Administrator invocation must register nothing — an artifact
//! written under `/root` or an Administrator profile belongs to a privilege
//! level the client daemon is never meant to occupy, and it would outlive the
//! mistaken invocation.
//!
//! This module answers the one question that gate needs. It is separate from
//! `greggd`'s own check because the client never links the daemon crate.

/// Whether this process runs with elevated privileges.
///
/// Unix: effective UID `0`. Windows: an elevated token. Any other platform is
/// unprivileged.
///
/// The Windows path fails **closed**: a token that cannot be opened or queried
/// counts as privileged. Refusing a legitimate install prints the exact command
/// the operator can run as the right user, while wrongly registering one writes
/// an artifact under the wrong profile and starts a daemon that cannot see the
/// user's config.
#[cfg(unix)]
#[allow(unsafe_code)] // `geteuid` reads the caller's effective UID; no memory access.
#[must_use]
pub fn is_privileged() -> bool {
    // SAFETY: `geteuid` takes no arguments, touches no memory, and cannot fail;
    // it returns the caller's effective UID, and `0` is root on every Unix this
    // crate targets.
    unsafe { libc::geteuid() == 0 }
}

/// Whether this process runs with elevated privileges.
///
/// See the Unix implementation for the fail-closed contract; this one adds one
/// rule of its own: an unreadable token is reported as elevated rather than
/// guessed.
#[cfg(windows)]
#[allow(unsafe_code)] // windows-sys token query on the current process.
#[must_use]
pub fn is_privileged() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = std::ptr::null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that must not be
    // closed and takes no arguments; `OpenProcessToken` writes exactly one
    // `HANDLE` through the pointer, which outlives the call.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return true;
    }
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut returned = 0u32;
    // SAFETY: `token` is an open handle queried for `TOKEN_QUERY` above, the
    // output buffer is exactly `TOKEN_ELEVATION`, and `size` states that, so the
    // call writes at most the buffer's own size.
    let queried = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            std::ptr::from_mut(&mut elevation).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    // SAFETY: `token` was created by the successful `OpenProcessToken` above
    // and is closed exactly once, here.
    unsafe { CloseHandle(token) };
    queried == 0 || elevation.TokenIsElevated != 0
}

/// Whether this process runs with elevated privileges.
///
/// No other platform has an elevation concept this crate registers under, so
/// the answer is always "no".
#[cfg(not(any(unix, windows)))]
#[must_use]
pub fn is_privileged() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The branch truth table belongs to the caller that acts on it
    /// (`privileged_install_refusal`), because only that can be exercised at
    /// both values. Here the probe only has to be callable, and — on Unix —
    /// agree with the primitive it is defined in terms of.
    #[test]
    fn privilege_probe_agrees_with_the_process_identity() {
        let privileged = is_privileged();
        #[cfg(unix)]
        assert_eq!(privileged, effective_uid_is_root());
        // No other platform has an elevation concept to compare against.
        #[cfg(not(unix))]
        let _ = privileged;
    }

    #[cfg(unix)]
    #[allow(unsafe_code)] // Same call as `is_privileged`, stated independently.
    fn effective_uid_is_root() -> bool {
        unsafe { libc::geteuid() == 0 }
    }
}
