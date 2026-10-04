//! Plan 164: the per-config Gregg client daemon and its local IPC boundary.
//!
//! A `gregg` install runs as two cooperating halves against one resolved
//! configuration:
//!
//! ```text
//! gregg daemon run            (once per config, long-lived)
//!   owns: poll scheduler, fleet data, EggPool worker, Ctrl-R reload
//!        |
//!        | local IPC: Unix socket / Windows named pipe,
//!        | versioned handshake, latest-state fan-out
//!        v
//! gregg (TUI)                 (many, short-lived)
//!   owns: terminal, input, selection, viewport, active pane,
//!         expansion, view mode, transient highlight
//! ```
//!
//! The split exists so a TUI exit never stops polling, `Ctrl-R` is the only
//! config-reload boundary, and several TUI windows can share one poll budget
//! instead of each issuing its own generation of requests.
//!
//! The local DTOs live in [`protocol`] and [`snapshot`] and are **not** part of
//! `gregg-protocol`: that crate is the cross-version remote wire contract
//! shared with a whole fleet, while this is a same-binary, same-host channel
//! whose version is verified by handshake rather than by forward compatibility.

pub mod cron;
pub mod daemon;
pub mod frontend;
pub mod identity;
pub mod ipc;
pub mod launch;
pub mod protocol;
pub mod snapshot;
pub mod startup;

pub use daemon::{
    attach, run_daemon, status, stop, AttachError, Attachment, DaemonError, DaemonStatus,
};
pub use frontend::{
    ControlSink, EggpoolIntentRequest, FrameStream, FrontError, FrontendLink, FrontendSender,
};
pub use identity::{
    client_daemon_id, fallback_socket_path, primary_socket_path, socket_candidates,
    ClientDaemonIdentity,
};
pub use ipc::{BoundEndpoint, Connection, TransportError};
pub use launch::{ensure_running, restart, EnsureError};
pub use protocol::{
    classify, encode_frame, parse_length, DaemonRequest, DecodeError, EncodeError,
    FrameDisposition, FrontendFrame, HelloPayload, VersionMismatchPayload, LEN_PREFIX_BYTES,
    MAX_FRAME_BYTES, PROTOCOL_VERSION,
};
pub use snapshot::{FrontendSnapshot, PresentationState, SystemSnapshotDto};
pub use startup::{
    inspect, install, render_instructions, uninstall, Installed, StartupMethod, StartupTarget,
    UninstallStep,
};
