//! Windows named-pipe control endpoint for explicit scheduled-job rehash.

#[path = "control/windows_pipe.rs"]
mod windows_pipe;

use std::path::Path;
use std::time::Duration;

use crate::local_control::{
    decode_response, encode_response, RehashRequest, RehashResponse, RehashTransportError,
    MAX_REHASH_RESPONSE_BYTES, REHASH_COMMAND,
};
use tracing::warn;

const SERVER_APPLY_TIMEOUT: Duration = Duration::from_millis(750);

/// Bind the protected config-specific named pipe.
pub(crate) fn bind_listener(path: &Path) -> std::io::Result<windows_pipe::Listener> {
    windows_pipe::Listener::bind(path)
}

/// Serve bounded local rehash requests. The pipe is intentionally not a STOP
/// path; Windows shutdown remains owned by the Service Control Manager.
pub(crate) fn spawn_control_task(
    mut listener: windows_pipe::Listener,
    rehash_tx: tokio::sync::mpsc::Sender<RehashRequest>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let mut connection = match listener.accept().await {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(error) => {
                    warn!(%error, "Windows rehash control listener failed");
                    return;
                }
            };
            let request = match connection.read_request().await {
                Ok(request) => request,
                Err(error) => {
                    warn!(%error, "Windows rehash control request rejected");
                    continue;
                }
            };
            if request.as_slice() != REHASH_COMMAND {
                warn!("malformed Windows rehash control request rejected");
                continue;
            }

            let (reply, receiver) = tokio::sync::oneshot::channel();
            let response = match rehash_tx.try_send(RehashRequest { reply }) {
                Ok(()) => match tokio::time::timeout(SERVER_APPLY_TIMEOUT, receiver).await {
                    Ok(Ok(response)) => response,
                    Ok(Err(_)) | Err(_) => RehashResponse::Uncertain,
                },
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => RehashResponse::Busy,
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => RehashResponse::Rejected(
                    crate::local_control::RehashRejection::ControlUnavailable,
                ),
            };
            let frame = encode_response(&response);
            if let Err(error) = connection.write_response(&frame).await {
                warn!(%error, "Windows rehash control response failed");
            }
        }
    })
}

/// Send one local request and distinguish absence from post-delivery uncertainty.
pub fn send_rehash(config_path: &Path) -> Result<RehashResponse, RehashTransportError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(RehashTransportError::Io)?;
    let frame = runtime
        .block_on(windows_pipe::send(config_path, REHASH_COMMAND))
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                RehashTransportError::NotRunning
            }
            std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::BrokenPipe => RehashTransportError::Uncertain {
                detail: format!("request may have been delivered: {error}"),
            },
            _ => RehashTransportError::Io(error),
        })?;
    if frame.len() > MAX_REHASH_RESPONSE_BYTES {
        return Err(RehashTransportError::BadResponse);
    }
    match decode_response(&frame)? {
        RehashResponse::Uncertain => Err(RehashTransportError::Uncertain {
            detail: "daemon accepted the request but did not confirm scheduler application".into(),
        }),
        response => Ok(response),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_control::{RehashRequest, RehashResponse};

    #[tokio::test]
    async fn control_acknowledgment_waits_for_scheduler_reply() {
        let path = std::env::temp_dir().join(format!(
            "greggd-rehash-control-test-{}.missing.toml",
            std::process::id()
        ));
        let listener = bind_listener(&path).expect("bind protected pipe");
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let _server = spawn_control_task(listener, tx);
        let server_reply = tokio::spawn(async move {
            let request: RehashRequest = rx.recv().await.expect("scheduler receives request");
            request
                .reply
                .send(RehashResponse::Applied)
                .expect("reply delivered");
        });
        let frame = windows_pipe::send(&path, REHASH_COMMAND)
            .await
            .expect("local pipe request succeeds");
        assert_eq!(decode_response(&frame).unwrap(), RehashResponse::Applied);
        server_reply.await.unwrap();
    }
}
