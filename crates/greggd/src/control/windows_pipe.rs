//! Bounded Windows named-pipe transport for daemon control.
//!
//! Server and client derive the same pipe name from a normalized config path.
//! Existing paths are canonicalized; missing paths are lexically normalized
//! after becoming absolute. The path is hashed and never appears in a request.

#![cfg(windows)]

use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer};
use tokio::time::timeout;

/// Maximum request bytes, including newline.
pub const MAX_REQUEST_BYTES: usize = 32;
/// Maximum response bytes, including newline.
pub const MAX_RESPONSE_BYTES: usize = 160;
/// Maximum duration of one accept, read, or write.
pub const CONTROL_IO_TIMEOUT: Duration = Duration::from_secs(1);

const PIPE_PREFIX: &str = r"\\.\pipe\greggd-control-";
const PIPE_SDDL: &str = "D:P(A;;GA;;;BA)(A;;GA;;;LS)";
const REJECT_REMOTE_CLIENTS: bool = true;
const MAX_PIPE_INSTANCES: usize = 254;

/// Derive the deterministic named-pipe path for a config file.
/// Existing paths are canonicalized; missing paths use absolute lexical
/// normalization. Bind and send use this same rule.
pub fn pipe_name(config_path: &Path) -> io::Result<String> {
    let absolute = if config_path.is_absolute() {
        config_path.to_path_buf()
    } else {
        std::env::current_dir()?.join(config_path)
    };
    let normalized = match std::fs::canonicalize(&absolute) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => lexical_normalize(&absolute),
        Err(error) => return Err(error),
    };
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for unit in std::os::windows::ffi::OsStrExt::encode_wide(normalized.as_os_str()) {
        for byte in unit.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    Ok(format!("{PIPE_PREFIX}{hash:016x}"))
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.file_name().is_some() {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

/// Listener for one normalized config identity.
#[derive(Debug)]
pub struct Listener {
    name: String,
    pending: Option<NamedPipeServer>,
    first_instance: bool,
}

impl Listener {
    /// Create the first pipe instance with the protected Administrators and
    /// `LocalService` DACL and remote-client rejection enabled.
    pub fn bind(config_path: &Path) -> io::Result<Self> {
        let name = pipe_name(config_path)?;
        let pending = win32::create_server(&name, true)?;
        Ok(Self {
            name,
            pending: Some(pending),
            first_instance: false,
        })
    }

    /// Accept one client, bounded to one second. Tokio's underlying pipe
    /// connect future is cancellation-safe, so a timeout preserves the instance.
    pub async fn accept(&mut self) -> io::Result<Connection> {
        if self.pending.is_none() {
            self.pending = Some(win32::create_server(&self.name, self.first_instance)?);
            self.first_instance = false;
        }
        let result = {
            let pending = self.pending.as_ref().expect("pipe instance exists");
            timeout(CONTROL_IO_TIMEOUT, pending.connect()).await
        };
        match result {
            Err(_) => Err(timed_out("named-pipe accept timed out")),
            Ok(Err(error)) => Err(error),
            Ok(Ok(())) => Ok(Connection {
                pipe: self.pending.take(),
            }),
        }
    }
}

/// One accepted connection, carrying one bounded line in each direction.
#[derive(Debug)]
pub struct Connection {
    pipe: Option<NamedPipeServer>,
}

impl Connection {
    /// Read a request line, including its newline. Errors close this connection.
    pub async fn read_request(&mut self) -> io::Result<Vec<u8>> {
        let result = match self.pipe.as_mut() {
            Some(pipe) => match timeout(
                CONTROL_IO_TIMEOUT,
                read_line_bounded(pipe, MAX_REQUEST_BYTES),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err(timed_out("named-pipe request timed out")),
            },
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "pipe connection is closed",
            )),
        };
        if result.is_err() {
            self.pipe.take();
        }
        result
    }

    /// Write one newline-terminated response within the byte and time bounds.
    pub async fn write_response(&mut self, response: &[u8]) -> io::Result<()> {
        validate_line(response, MAX_RESPONSE_BYTES)?;
        let result = match self.pipe.as_mut() {
            Some(pipe) => match timeout(CONTROL_IO_TIMEOUT, write_line(pipe, response)).await {
                Ok(result) => result,
                Err(_) => Err(timed_out("named-pipe response timed out")),
            },
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "pipe connection is closed",
            )),
        };
        if result.is_err() {
            self.pipe.take();
        }
        result
    }
}

/// Send one bounded request line and read one bounded response line.
pub async fn send(config_path: &Path, request: &[u8]) -> io::Result<Vec<u8>> {
    validate_line(request, MAX_REQUEST_BYTES)?;
    let name = pipe_name(config_path)?;
    let mut pipe = ClientOptions::new().read(true).write(true).open(&name)?;
    timeout(CONTROL_IO_TIMEOUT, write_line(&mut pipe, request))
        .await
        .map_err(|_| timed_out("named-pipe request write timed out"))??;
    timeout(
        CONTROL_IO_TIMEOUT,
        read_line_bounded(&mut pipe, MAX_RESPONSE_BYTES),
    )
    .await
    .map_err(|_| timed_out("named-pipe response read timed out"))?
}

async fn read_line_bounded<R: AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> io::Result<Vec<u8>> {
    let mut line = Vec::with_capacity(limit.min(64));
    loop {
        let mut byte = [0_u8; 1];
        if reader.read(&mut byte).await? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "peer closed before newline",
            ));
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            return Ok(line);
        }
        if line.len() >= limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame exceeds {limit} bytes"),
            ));
        }
    }
}

async fn write_line<W: AsyncWrite + Unpin>(writer: &mut W, line: &[u8]) -> io::Result<()> {
    writer.write_all(line).await?;
    writer.flush().await
}

fn validate_line(line: &[u8], limit: usize) -> io::Result<()> {
    if line.is_empty()
        || line.len() > limit
        || line.last() != Some(&b'\n')
        || line[..line.len() - 1].contains(&b'\n')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("expected one newline-terminated line of at most {limit} bytes"),
        ));
    }
    Ok(())
}

fn timed_out(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, message)
}

/// Sole unsafe boundary for SDDL conversion and raw Windows security attributes.
#[allow(unsafe_code)]
mod win32 {
    use super::{io, NamedPipeServer, MAX_PIPE_INSTANCES, PIPE_SDDL, REJECT_REMOTE_CLIENTS};
    use std::ffi::{c_void, OsStr};
    use std::ptr;
    use tokio::net::windows::named_pipe::{PipeMode, ServerOptions};
    #[cfg(test)]
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE};
    use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
    #[cfg(test)]
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSidToSidW,
        GetSecurityInfo, SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    #[cfg(test)]
    use windows_sys::Win32::Security::{
        CreateRestrictedToken, ImpersonateLoggedOnUser, RevertToSelf, DACL_SECURITY_INFORMATION,
        PSID, SID_AND_ATTRIBUTES, TOKEN_DUPLICATE, TOKEN_QUERY,
    };
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    #[cfg(test)]
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    pub(super) fn create_server(name: &str, first_instance: bool) -> io::Result<NamedPipeServer> {
        let sddl: Vec<u16> = PIPE_SDDL.encode_utf16().chain(std::iter::once(0)).collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: sddl is NUL-terminated and lives through the call; descriptor
        // is a live pointer-sized out-parameter and the optional size is null.
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                ptr::null_mut(),
            )
        };
        if converted == 0 {
            return Err(io::Error::last_os_error());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(u32::MAX),
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let mut options = ServerOptions::new();
        options
            .pipe_mode(PipeMode::Byte)
            .max_instances(MAX_PIPE_INSTANCES)
            .first_pipe_instance(first_instance)
            .reject_remote_clients(REJECT_REMOTE_CLIENTS);
        // SAFETY: attributes and its descriptor remain alive through this
        // synchronous call; Windows consumes the attributes before returning.
        let created = unsafe {
            options.create_with_security_attributes_raw(
                OsStr::new(name),
                ptr::from_ref(&attributes).cast_mut().cast::<c_void>(),
            )
        };
        // SAFETY: descriptor was allocated by the conversion and is freed once.
        unsafe {
            LocalFree(descriptor as HLOCAL);
        }
        created
    }

    /// Impersonation guard that always restores the process token on drop.
    #[cfg(test)]
    pub(super) struct RestrictedImpersonation;

    #[cfg(test)]
    impl Drop for RestrictedImpersonation {
        fn drop(&mut self) {
            // SAFETY: every guard is constructed only after a successful
            // ImpersonateLoggedOnUser on this thread, so this is the paired
            // revert and it takes no pointer arguments.
            unsafe {
                RevertToSelf();
            }
        }
    }

    /// Impersonate a restricted form of the current process token in which the
    /// Administrators group is deny-only, so the protected pipe DACL cannot
    /// grant it access. Native ACL regression coverage only.
    #[cfg(test)]
    pub(super) fn impersonate_restricted_administrator() -> io::Result<RestrictedImpersonation> {
        let mut process_token = ptr::null_mut();
        // SAFETY: GetCurrentProcess returns a valid pseudo handle, and
        // process_token is a live out-parameter. The handle is closed once.
        let opened = unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_DUPLICATE | TOKEN_QUERY,
                &raw mut process_token,
            )
        };
        if opened == 0 {
            return Err(io::Error::last_os_error());
        }

        let administrators_sid: Vec<u16> = "S-1-5-32-544"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut administrators: PSID = ptr::null_mut();
        // SAFETY: the SID text is NUL-terminated and outlives the call;
        // administrators is a live out-parameter released with LocalFree below.
        let converted =
            unsafe { ConvertStringSidToSidW(administrators_sid.as_ptr(), &raw mut administrators) };
        if converted == 0 {
            let error = io::Error::last_os_error();
            // SAFETY: process_token came from a successful OpenProcessToken.
            unsafe {
                CloseHandle(process_token);
            }
            return Err(error);
        }

        let mut deny_administrators = [SID_AND_ATTRIBUTES {
            Sid: administrators,
            Attributes: 0,
        }];
        let mut restricted = ptr::null_mut();
        // SAFETY: process_token is a live token handle opened with
        // TOKEN_DUPLICATE, the SID array is valid for the call, and restricted
        // is a live out-parameter closed below.
        let created = unsafe {
            CreateRestrictedToken(
                process_token,
                0,
                1,
                deny_administrators.as_mut_ptr(),
                0,
                ptr::null(),
                0,
                ptr::null(),
                &raw mut restricted,
            )
        };
        // SAFETY: both allocations came from the calls above and are released
        // exactly once here, whatever those calls returned.
        unsafe {
            LocalFree(administrators as HLOCAL);
            CloseHandle(process_token);
        }
        if created == 0 {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: restricted is a live impersonation token handle.
        let impersonated = unsafe { ImpersonateLoggedOnUser(restricted) };
        // SAFETY: impersonation copied the token onto this thread, so the
        // handle is no longer needed and is closed exactly once.
        unsafe {
            CloseHandle(restricted);
        }
        if impersonated == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(RestrictedImpersonation)
    }

    /// Read the DACL actually attached to the created pipe object and render it
    /// back to SDDL, so callers inspect the effective policy on the object
    /// instead of the constant it was built from.
    #[cfg(test)]
    pub(super) fn effective_dacl_sddl(handle: HANDLE) -> io::Result<String> {
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: handle is a live pipe handle owned by the caller; the
        // security-descriptor out-parameter is a live local, the unused owner,
        // group, and SACL out-parameters are null, and the descriptor is
        // released with LocalFree below.
        let status = unsafe {
            GetSecurityInfo(
                handle,
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &raw mut descriptor,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(
                i32::try_from(status).unwrap_or(i32::MAX),
            ));
        }

        let mut text: *mut u16 = ptr::null_mut();
        let mut length = 0_u32;
        // SAFETY: descriptor came from GetSecurityInfo and owns the DACL; text
        // and length are live out-parameters released with LocalFree below.
        let converted = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &raw mut text,
                &raw mut length,
            )
        };
        let rendered = if converted == 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: a successful conversion returns a NUL-terminated string of
            // length characters that stays valid until the LocalFree below.
            let units = unsafe { std::slice::from_raw_parts(text, length as usize) };
            Ok(String::from_utf16_lossy(units))
        };
        // SAFETY: both pointers were allocated by the calls above and each is
        // released exactly once.
        unsafe {
            if !text.is_null() {
                LocalFree(text as HLOCAL);
            }
            LocalFree(descriptor as HLOCAL);
        }
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::{
        pipe_name, validate_line, Listener, CONTROL_IO_TIMEOUT, MAX_REQUEST_BYTES,
        MAX_RESPONSE_BYTES, PIPE_PREFIX, PIPE_SDDL,
    };
    use std::io;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::net::windows::named_pipe::ClientOptions;

    fn test_config(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "greggd-pipe-{name}-{}.missing.toml",
            std::process::id()
        ))
    }

    #[test]
    fn config_identity_is_deterministic_after_lexical_normalization() {
        let config = test_config("identity");
        let with_parent = config
            .parent()
            .unwrap()
            .join("unused")
            .join("..")
            .join(config.file_name().unwrap());
        assert_eq!(
            pipe_name(&config).unwrap(),
            pipe_name(&with_parent).unwrap()
        );
        assert!(pipe_name(&config).unwrap().starts_with(PIPE_PREFIX));
    }

    #[test]
    fn pipe_policy_is_protected_for_administrators_and_local_service() {
        assert_eq!(PIPE_SDDL, "D:P(A;;GA;;;BA)(A;;GA;;;LS)");
        assert!(PIPE_SDDL.starts_with("D:P"));
        assert!(PIPE_SDDL.contains(";;;BA)"));
        assert!(PIPE_SDDL.contains(";;;LS)"));
    }

    #[tokio::test]
    async fn created_pipe_object_carries_the_protected_dacl() {
        use std::os::windows::io::AsRawHandle;

        // The pipe handle is created through tokio, so this must run inside a
        // runtime even though the test itself never awaits.
        let config = test_config("acl-descriptor");
        let listener = Listener::bind(&config).expect("binds protected pipe");
        let handle = listener
            .pending
            .as_ref()
            .expect("first instance exists")
            .as_raw_handle();
        let effective = super::win32::effective_dacl_sddl(handle).expect("reads effective DACL");
        assert_eq!(
            effective, PIPE_SDDL,
            "the pipe object must carry the protected owner-only DACL"
        );
    }

    #[tokio::test]
    async fn a_deny_only_administrators_token_cannot_connect() {
        let config = test_config("acl-denied");
        let _listener = Listener::bind(&config).expect("binds protected pipe");
        let name = pipe_name(&config).expect("derives pipe name");
        let guard = super::win32::impersonate_restricted_administrator()
            .expect("restricts the current token");
        let opened = ClientOptions::new().read(true).write(true).open(&name);
        drop(guard);
        let error = opened.expect_err("a deny-only Administrators token must be refused");
        assert_eq!(
            error
                .raw_os_error()
                .and_then(|code| u32::try_from(code).ok()),
            Some(windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED),
            "expected an access-denied connect failure, got {error}"
        );
    }

    #[test]
    fn malformed_and_oversized_lines_are_rejected() {
        assert!(validate_line(b"STOP\n", MAX_REQUEST_BYTES).is_ok());
        assert!(validate_line(b"STOP", MAX_REQUEST_BYTES).is_err());
        assert!(validate_line(b"STOP\nEXTRA\n", MAX_REQUEST_BYTES).is_err());
        assert!(validate_line(&[b'x'; MAX_REQUEST_BYTES + 1], MAX_REQUEST_BYTES).is_err());
        assert!(validate_line(b"OK\n", MAX_RESPONSE_BYTES).is_ok());
        let mut maximum = [b'x'; MAX_RESPONSE_BYTES];
        *maximum.last_mut().unwrap() = b'\n';
        assert!(validate_line(&maximum, MAX_RESPONSE_BYTES).is_ok());
        assert!(validate_line(&[b'x'; MAX_RESPONSE_BYTES + 1], MAX_RESPONSE_BYTES).is_err());
    }

    #[tokio::test]
    async fn named_pipe_client_and_server_exchange_one_line() {
        let config = test_config("exchange");
        let mut listener = Listener::bind(&config).expect("binds protected pipe");
        let server = tokio::spawn(async move {
            let mut connection = listener.accept().await.expect("accepts client");
            assert_eq!(
                connection.read_request().await.expect("reads request"),
                b"STOP\n"
            );
            connection
                .write_response(b"OK\n")
                .await
                .expect("writes response");
        });
        let response = super::send(&config, b"STOP\n")
            .await
            .expect("sends request");
        assert_eq!(response, b"OK\n");
        server.await.expect("server task succeeds");
    }

    #[tokio::test]
    async fn oversized_peer_frame_is_rejected() {
        let config = test_config("oversized");
        let mut listener = Listener::bind(&config).expect("binds protected pipe");
        let name = pipe_name(&config).expect("derives pipe name");
        let server = tokio::spawn(async move {
            let mut connection = listener.accept().await.expect("accepts client");
            connection
                .read_request()
                .await
                .expect_err("oversized request must fail")
                .kind()
        });
        let mut client = ClientOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .expect("connects to server");
        client
            .write_all(&[b'x'; MAX_REQUEST_BYTES + 1])
            .await
            .expect("sends test payload");
        assert_eq!(
            server.await.expect("server task succeeds"),
            io::ErrorKind::InvalidData
        );
    }

    #[tokio::test]
    async fn accept_timeout_is_bounded_and_retryable() {
        let config = test_config("accept-timeout");
        let mut listener = Listener::bind(&config).expect("binds protected pipe");
        let started = tokio::time::Instant::now();
        let error = listener.accept().await.expect_err("no client is waiting");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() <= CONTROL_IO_TIMEOUT + Duration::from_millis(250));
        let accepting = tokio::spawn(async move { listener.accept().await });
        let name = pipe_name(&config).expect("derives pipe name");
        let client = ClientOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .expect("connects after canceled accept");
        assert!(accepting.await.expect("accept task succeeds").is_ok());
        drop(client);
    }
}
