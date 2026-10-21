//! Plan 188 Part B: the bridge between the Gregg client daemon and the GUI.
//!
//! # Ownership
//!
//! The bridge owns the client-daemon attachment: it attaches (starting the
//! daemon if one is not already running), reads the frame stream, and
//! publishes what it learns as plain data. It never touches a Slint
//! component — the UI thread drains [`Bridge::drain`] and applies the
//! result. That split keeps the protocol side independent of an event loop
//! and keeps every Slint mutation on the thread that owns the event loop.
//!
//! # What is not here yet
//!
//! Plan 189 owns the full GUI. This module is the seam it grows into: it
//! already speaks the real local protocol and renders the real reducer's
//! output, so the desktop frontend is built on the same data the TUI shows
//! rather than on a parallel implementation of it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use gregg::clientd::{ensure_running, Attachment, FrontendFrame, FrontendSender, SystemSnapshotDto};
use gregg::config::ConfigStore;
use gregg::normalized::NormalizedSnapshot;
use gregg::sanitize::sanitize_single_line;
use gregg::state::Reachability;
use gregg::ui::text::{format_bytes, format_pct};

/// What a missing reading looks like.
///
/// A missing observation is an em dash, never a zero: `0.0%` would claim the
/// platform reported a measurement of zero rather than reporting none.
const MISSING: &str = "\u{2014}";

/// One monitored system, already formatted for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemSummary {
    /// Stable system id.
    pub id: String,
    /// Display name, falling back to the host.
    pub name: String,
    /// The endpoint being polled, as `host:port`.
    pub endpoint: String,
    /// Reachability in words, including the reason when the system is offline.
    pub status: String,
    /// CPU usage line.
    pub cpu: String,
    /// Memory usage line.
    pub memory: String,
    /// Whether the most recent poll succeeded.
    pub online: bool,
}

/// What the bridge publishes to the UI.
#[derive(Debug, Clone)]
pub enum BridgeMessage {
    /// The handshake succeeded.
    Attached {
        /// The local protocol version the daemon speaks.
        protocol_version: u16,
        /// The daemon's config-specific identity.
        daemon_id: String,
    },
    /// A new fleet document arrived.
    Fleet {
        /// The document's generation.
        generation: u64,
        /// The fleet, in display order.
        systems: Vec<SystemSummary>,
    },
    /// The attachment failed, or the connection was lost.
    Failed {
        /// Why.
        reason: String,
    },
    /// The daemon is going away.
    ShuttingDown,
}

/// The live connection to the client daemon.
pub struct Bridge {
    /// What the bridge has published since the UI last drained it.
    messages: Receiver<BridgeMessage>,
    /// The write half of the connection, once the handshake succeeded.
    ///
    /// `None` until the attachment is up. The daemon owns the configuration
    /// file, so a reload is a request to it and never a local file read.
    sender: Arc<Mutex<Option<FrontendSender>>>,
    /// The generation of the next control request.
    next_generation: AtomicU64,
}

impl Bridge {
    /// Attach to the client daemon for `config_path` on a background thread.
    ///
    /// The attachment is asynchronous — the daemon may have to be started and
    /// waited for — and Slint owns the main thread, so it cannot happen
    /// there.
    #[must_use]
    pub fn spawn(config_path: PathBuf) -> Self {
        let (tx, messages) = std::sync::mpsc::channel();
        let sender: Arc<Mutex<Option<FrontendSender>>> = Arc::new(Mutex::new(None));
        let sender_for_task = Arc::clone(&sender);

        std::thread::Builder::new()
            .name("gregg-gui-bridge".to_owned())
            .spawn(move || {
                let runtime =
                    match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                        Ok(runtime) => runtime,
                        Err(error) => {
                            let _ = tx.send(BridgeMessage::Failed {
                                reason: format!("could not start the bridge runtime: {error}"),
                            });
                            return;
                        }
                    };
                runtime.block_on(pump(config_path, tx, sender_for_task));
            })
            .expect("failed to spawn the client-daemon bridge thread");

        Self {
            messages,
            sender,
            next_generation: AtomicU64::new(0),
        }
    }

    /// Take everything published since the last call.
    pub fn drain(&mut self) -> Vec<BridgeMessage> {
        self.messages.try_iter().collect()
    }

    /// Ask the daemon to re-read its configuration file.
    ///
    /// This is the `Ctrl-R` boundary and the only one: there is no
    /// filesystem watcher, so nothing else can produce a reload. A request
    /// made before the attachment is up is dropped — the daemon did not
    /// exist to receive it, and there is nothing to retry against.
    pub fn request_reload(&self) {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let Ok(slot) = self.sender.lock() else {
            return;
        };
        if let Some(frontend) = slot.as_ref() {
            let _ = frontend.request_reload(generation);
        }
    }
}

/// Attach, then publish every document the daemon sends.
async fn pump(
    config_path: PathBuf,
    tx: Sender<BridgeMessage>,
    sender: Arc<Mutex<Option<FrontendSender>>>,
) {
    let store = ConfigStore::new(config_path);

    // TODO(Plan 189): `ensure_running` launches the *calling* executable as
    // the client daemon. That is right for the `gregg` TUI, which is itself a
    // Gregg client, and wrong for this GUI, which is not: `current_exe()` is
    // `gregg-gui`, so the spawned process would exit immediately and the
    // readiness wait would time out. Plan 189 must call
    // `ensure_running_with_executable(&store, &DaemonExecutable::Path(path))`
    // with an explicit path to a `gregg` binary, so the GUI never spawns
    // itself.
    let attachment: Attachment = match ensure_running(&store).await {
        Ok(attachment) => attachment,
        Err(error) => {
            let _ = tx.send(BridgeMessage::Failed {
                reason: error.to_string(),
            });
            return;
        }
    };

    let Attachment {
        frontend,
        frames,
        hello,
        ..
    } = attachment;

    if let Ok(mut slot) = sender.lock() {
        *slot = Some(frontend);
    }
    let _ = tx.send(BridgeMessage::Attached {
        protocol_version: hello.protocol_version,
        daemon_id: hello.daemon_id,
    });

    let mut frames = frames;
    while let Ok(frame) = frames.next().await {
        match frame {
            FrontendFrame::Snapshot(snapshot) => {
                let _ = tx.send(BridgeMessage::Fleet {
                    generation: snapshot.generation,
                    systems: summarize(&snapshot.systems),
                });
            }
            // The daemon's introduction carries no renderable state — the
            // document that follows carries everything — and a control
            // acknowledgement's outcome is visible only in the next document.
            FrontendFrame::Hello(_) | FrontendFrame::ControlAck { .. } => {}
            FrontendFrame::VersionMismatch(payload) => {
                let _ = tx.send(BridgeMessage::Failed {
                    reason: format!(
                        "the client daemon speaks local protocol {}, but this GUI speaks {}",
                        payload.daemon, payload.frontend
                    ),
                });
                return;
            }
            FrontendFrame::ProtocolError { message } => {
                let _ = tx.send(BridgeMessage::Failed { reason: message });
                return;
            }
            FrontendFrame::ShuttingDown => {
                let _ = tx.send(BridgeMessage::ShuttingDown);
                return;
            }
        }
    }

    let _ = tx.send(BridgeMessage::Failed {
        reason: "the client daemon closed the connection".to_owned(),
    });
}

/// Turn one fleet document into display rows.
fn summarize(systems: &[SystemSnapshotDto]) -> Vec<SystemSummary> {
    systems.iter().map(summarize_system).collect()
}

/// Turn one system into a display row.
fn summarize_system(system: &SystemSnapshotDto) -> SystemSummary {
    let name = system
        .configured_name
        .clone()
        .or_else(|| system.endpoint.name.clone())
        .unwrap_or_else(|| system.endpoint.host.clone());

    let (cpu, memory) = match system.latest.as_ref() {
        Some(latest) => (cpu_line(latest), memory_line(latest)),
        None => (MISSING.to_owned(), MISSING.to_owned()),
    };

    SystemSummary {
        id: system.id.clone(),
        name: one_line(&name),
        endpoint: one_line(&system.endpoint.display_address()),
        status: status_line(system),
        cpu,
        memory,
        online: matches!(system.reachability, Reachability::Online),
    }
}

/// The reachability line.
///
/// An offline system says *why* it is offline, in the same words the TUI
/// uses: the category is always there, and the detail (an HTTP status, say)
/// follows it when there is one.
fn status_line(system: &SystemSnapshotDto) -> String {
    match system.reachability {
        Reachability::Pending => "pending".to_owned(),
        Reachability::Online => "online".to_owned(),
        Reachability::Offline => match system.offline_reason.as_ref() {
            Some(reason) => match reason.detail.as_deref() {
                Some(detail) => format!("offline ({}) {}", reason.kind, one_line(detail)),
                None => format!("offline ({})", reason.kind),
            },
            None => "offline".to_owned(),
        },
    }
}

/// The CPU line.
fn cpu_line(latest: &NormalizedSnapshot) -> String {
    let mut line = format!("CPU {}", format_pct(latest.usage_pct));
    if let Some(iowait) = latest.iowait_pct {
        line.push_str(&format!(" (io wait {})", format_pct(iowait)));
    }
    line
}

/// The memory line.
fn memory_line(latest: &NormalizedSnapshot) -> String {
    format!(
        "MEM {} / {} ({})",
        format_bytes(latest.memory.used_bytes),
        format_bytes(latest.memory.total_bytes),
        format_pct(latest.memory.usage_pct)
    )
}

/// Make a remote-derived string safe to place in a label.
///
/// Control characters are rendered in caret notation rather than dropped, so
/// a hostile or broken name stays visible instead of silently changing what
/// the operator sees.
fn one_line(text: &str) -> String {
    sanitize_single_line(text).text
}

#[cfg(test)]
mod tests {
    use super::*;
    use gregg::endpoint::Endpoint;
    use gregg::poller::{OfflineKind, OfflineReason};

    fn system(reachability: Reachability, latest: Option<NormalizedSnapshot>) -> SystemSnapshotDto {
        SystemSnapshotDto {
            id: "system-1".to_owned(),
            endpoint: Endpoint {
                id: "system-1".to_owned(),
                host: "host-1".to_owned(),
                port: 11310,
                name: None,
            },
            configured_name: None,
            reachability,
            latest,
            last_success_at_unix_ms: None,
            last_attempt_at_unix_ms: None,
            latency_ms: None,
            offline_reason: None,
        }
    }

    #[test]
    fn a_pending_system_has_no_readings() {
        let row = summarize_system(&system(Reachability::Pending, None));
        assert_eq!(row.status, "pending");
        assert_eq!(row.cpu, MISSING);
        assert_eq!(row.memory, MISSING);
        assert!(!row.online);
    }

    #[test]
    fn an_offline_system_states_why() {
        let mut offline = system(Reachability::Offline, None);
        offline.offline_reason = Some(OfflineReason::new(OfflineKind::Refused));
        let row = summarize_system(&offline);
        assert_eq!(row.status, "offline (refused)");
    }

    #[test]
    fn an_offline_system_carries_its_detail() {
        let mut offline = system(Reachability::Offline, None);
        offline.offline_reason = Some(OfflineReason::with_detail(OfflineKind::Http, "HTTP 503"));
        let row = summarize_system(&offline);
        assert_eq!(row.status, "offline (http) HTTP 503");
    }

    #[test]
    fn a_control_character_in_a_name_stays_visible() {
        let mut named = system(Reachability::Pending, None);
        named.configured_name = Some("bad\u{1b}[2Jname".to_owned());
        let row = summarize_system(&named);
        assert!(row.name.contains("^["), "got: {}", row.name);
    }
}
