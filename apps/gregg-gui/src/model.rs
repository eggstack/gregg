//! The GUI's presentation model.
//!
//! This module is a **pure projection**: one daemon document in, one
//! [`GuiViewModel`] out. Nothing here touches a socket, a file, a clock, or a
//! Slint component, which is what makes the whole of the GUI's truthfulness
//! testable without a desktop session.
//!
//! # The rules this module exists to keep
//!
//! - **Absent is absent.** A metric the daemon did not report is `—`, never
//!   `0.0%` and never `0 B`. A platform that does not support a metric has no
//!   row at all, which is a different statement from "the value is missing".
//! - **Unpolled is not offline.** A system that has never been polled is
//!   `pending`, and a pending system carries no failure reason: there is
//!   nothing to have failed yet.
//! - **Selection is keyed by stable id.** A host that disappears or moves must
//!   not leave the window showing a different host's numbers, and a host that
//!   comes back must not silently steal the selection.
//! - **The daemon's aggregates are authoritative.** Disk and network totals
//!   come from the shared `gregg::normalized` helpers, never from a second
//!   summation of the per-device rows.
//!
//! # What this module does not do
//!
//! It does not own the connection, the intents, or the window. It is the
//! projection only, so the same model can be rendered into a window, into a
//! tray menu, or into a test assertion.

use gregg::clientd::snapshot::SystemSnapshotDto;
use gregg::clientd::snapshot::{EggpoolSnapshotDto, FrontendSnapshot, SystemCronDto};
use gregg::cron::CronCapability;
use gregg::eggpool::EggpoolPeriod;
use gregg::normalized::{aggregate_drives, NormalizedSnapshot};
use gregg::state::Reachability;
use gregg::ui::text::{
    format_bytes, format_capacity, format_frequency, format_load, format_pct, format_rate,
};

/// How a missing reading looks.
///
/// An em dash, never a number: a missing load reading is not a load of zero,
/// and `0.0%` would claim the platform reported a measurement of zero rather
/// than reporting none.
pub const MISSING: &str = "\u{2014}";

/// The longest a single displayed name may be.
///
/// A native menu is not a scrollable viewport, so an unbounded name would make
/// the menu unusable. The full name still appears in the window.
pub const MAX_NAME_CHARS: usize = 40;

/// The longest a single displayed endpoint may be.
pub const MAX_ENDPOINT_CHARS: usize = 40;

/// The most hosts the tray summary will name.
///
/// The summary is one line, and it is the tray's hover text as well as the
/// pulldown's header: a line that named every host in a large fleet would be
/// unreadable in both. The full list is in the pulldown.
pub const MAX_TRAY_HOSTS: usize = 5;

/// How the GUI is connected to the client daemon.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ConnectionState {
    /// The attachment has not completed yet.
    #[default]
    Connecting,
    /// The handshake succeeded.
    Connected {
        /// The local protocol version the daemon speaks.
        protocol_version: u16,
        /// The daemon's config-specific identity.
        daemon_id: String,
    },
    /// The connection is gone, or never came up.
    Disconnected {
        /// Why, in the operator's words.
        reason: String,
    },
    /// The daemon speaks a different local protocol version.
    Incompatible {
        /// Why, in the operator's words.
        reason: String,
    },
}

/// How many hosts are in each state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FleetCounts {
    /// Hosts that answered.
    pub online: usize,
    /// Hosts that have not been polled yet.
    pub pending: usize,
    /// Hosts whose last poll failed.
    pub offline: usize,
    /// Every host in the fleet.
    pub total: usize,
}

/// One host's state, in the daemon's own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostState {
    /// The last poll succeeded.
    Online,
    /// The host has never been polled.
    Pending,
    /// The last poll failed.
    Offline {
        /// The failure category, e.g. `refused` or `http`.
        kind: String,
        /// The detail, when the daemon reported one.
        detail: Option<String>,
    },
}

/// One measured row.
///
/// It is not `Eq`: a row carries a measured share, and a float is not
/// something Rust can say is equal to itself.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricRow {
    /// A stable key for the row, e.g. `cpu` or `memory`.
    pub key: &'static str,
    /// The short label, e.g. `CPU` or `MEM`.
    pub label: String,
    /// The share of the row that is used, in `0..=100`, when the platform
    /// reported a usable one. It is what the row's bar is filled to, and it
    /// is `None` when the platform reported no share at all.
    pub pct: Option<f32>,
    /// The share as text, e.g. `39.4%`. Empty when there is no share.
    pub pct_text: String,
    /// The value that is not a share, condensed for the bar's row, e.g.
    /// `6.1 / 15.5 GiB`. Empty when the row is nothing but a share.
    pub value: String,
    /// The row as the host list's one-line summary reads it, e.g.
    /// `6.1 GiB / 15.5 GiB (39.4%)`. The summary is the long form, because
    /// the host list has no room for a bar and no column to put a share in.
    pub summary: String,
}

/// One drive's row in the selected host's detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveRow {
    /// The mount name.
    pub mount: String,
    /// `used / total`.
    pub usage: String,
    /// Remaining space, when the platform reported it.
    pub remaining: Option<String>,
    /// Read/write throughput, when the platform reported it.
    pub throughput: Option<String>,
}

/// One network interface's row in the selected host's detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceRow {
    /// The interface name.
    pub name: String,
    /// `Rx/s` and `Tx/s`.
    pub throughput: String,
    /// Link capacity, when the platform reported it.
    pub capacity: Option<String>,
    /// Whether this is a loopback interface.
    ///
    /// Loopback is shown in the *interface detail* only. It is never part of
    /// an aggregate, because a loopback's traffic is the host talking to
    /// itself.
    pub loopback: bool,
}

/// One host, ready to render.
///
/// It is not `Eq`, because a host carries measured rows and a measured share
/// is a float.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemCard {
    /// The stable system id. Selection is keyed on this.
    pub id: String,
    /// The display name, truncated for a native menu.
    pub name: String,
    /// The endpoint, as `host:port`.
    pub endpoint: String,
    /// The endpoint in full, untruncated.
    ///
    /// [`Self::endpoint`] is truncated because a *native menu* has to stay
    /// readable. The panel's host-management forms are not a native menu,
    /// and an endpoint that is truncated in the form is an endpoint that
    /// cannot be written back: the panel's forms use this one.
    pub address: String,
    /// The configured name, when the host has one.
    ///
    /// The display name falls back to the endpoint's name and then to the
    /// host, so it cannot say whether a name is *configured*. A rename form
    /// has to: an empty field means "clear the configured name", and that
    /// is only the same as an empty field if the host had no name to begin
    /// with.
    pub configured_name: Option<String>,
    /// The host's state.
    pub state: HostState,
    /// The state in words, e.g. `offline (http) HTTP 503`.
    pub status: String,
    /// A one-line summary for a compact list.
    pub headline: String,
    /// The measured rows.
    pub metrics: Vec<MetricRow>,
    /// The selected host's drive detail. Empty for other hosts.
    pub drives: Vec<DriveRow>,
    /// The selected host's interface detail. Empty for other hosts.
    pub interfaces: Vec<InterfaceRow>,
    /// How long ago the last successful poll was, when there was one.
    pub age: Option<String>,
}

impl SystemCard {
    /// Whether this host answered its last poll.
    #[must_use]
    pub fn is_online(&self) -> bool {
        matches!(self.state, HostState::Online)
    }
}

/// The cron plane for one host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronView {
    /// Whether the remote serves the scheduler routes at all.
    pub supported: bool,
    /// The configured jobs and their last terminal outcome.
    pub jobs: Vec<CronJobRow>,
    /// The records retained for the selected job, if one is selected.
    pub records: Vec<String>,
    /// Whether the retained data may be out of date.
    ///
    /// Distinct from the host being unreachable: the metrics plane can be fine
    /// while the scheduler route failed.
    pub stale: bool,
}

/// One cron job's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronJobRow {
    /// The job's name.
    pub name: String,
    /// The job's current scheduling state, in the shared words.
    pub state: String,
    /// Its last terminal outcome, in the shared words.
    pub outcome: String,
    /// The load-gate context, when the job has one.
    pub load_gate: Option<String>,
    /// Whether the job is currently running.
    pub running: bool,
    /// Whether the job is waiting on a load gate.
    pub deferred: bool,
}

/// The `EggPool` plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EggpoolView {
    /// The rolling window the daemon is actually serving.
    pub period: EggpoolPeriod,
    /// The four summary values, already formatted.
    pub values: Vec<(String, String)>,
    /// The service health, in words.
    pub health: String,
    /// Whether the service reported itself ready.
    pub ready: bool,
    /// The most recent failure, when there is one.
    pub error: Option<String>,
}

/// Everything the window and the tray render.
///
/// It is not `Eq`, because a host carries measured rows and a measured share
/// is a float.
#[derive(Debug, Clone, PartialEq)]
pub struct GuiViewModel {
    /// The connection state.
    pub connection: ConnectionState,
    /// The fleet counts.
    pub counts: FleetCounts,
    /// Every host, in the daemon's display order.
    pub systems: Vec<SystemCard>,
    /// The selected host's id.
    pub selected_id: Option<String>,
    /// The selected host's card, when a host is selected.
    pub selected: Option<SystemCard>,
    /// The cron plane of the selected host.
    pub cron: Option<CronView>,
    /// The `EggPool` plane.
    pub eggpool: Option<EggpoolView>,
    /// A diagnostic from a rejected configuration reload.
    pub config_reload_error: Option<String>,
    /// The one-line fleet summary the tray shows.
    pub tray_summary: String,
}

/// The GUI's own selection state.
///
/// Separate from the view model because it *survives* a projection: a host
/// that is temporarily absent must not lose the selection, and a host that
/// comes back must get it again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// The selected host, by stable id.
    pub selected_id: Option<String>,
    /// The open cron job, by name, and the host it belongs to.
    pub cron_job: Option<(String, String)>,
}

/// Project one daemon document into the GUI's view model.
///
/// `now_unix_ms` is injected rather than read, so the ages a test asserts are
/// the ages the test chose.
#[must_use]
pub fn project(
    snapshot: &FrontendSnapshot,
    selection: &Selection,
    connection: &ConnectionState,
    now_unix_ms: u64,
) -> GuiViewModel {
    let counts = FleetCounts::from_systems(&snapshot.systems);
    let systems: Vec<SystemCard> = snapshot
        .systems
        .iter()
        .map(|system| card(system, now_unix_ms))
        .collect();

    // The selection is repaired against *this* document, so a host that
    // disappeared cannot leave the window showing a different host's numbers.
    let selected_id = repair_selection(selection.selected_id.as_deref(), &systems);
    let selected = selected_id
        .as_deref()
        .and_then(|id| systems.iter().find(|card| card.id == id))
        .cloned();

    let cron = selected_id
        .as_deref()
        .and_then(|id| snapshot.cron_for(id))
        .map(|entry| cron_view(entry, selection, now_unix_ms));

    let eggpool = snapshot.eggpool.as_ref().map(eggpool_view);

    let tray_summary = tray_summary(&counts, &systems);

    GuiViewModel {
        connection: connection.clone(),
        counts,
        systems,
        selected_id,
        selected,
        cron,
        eggpool,
        config_reload_error: snapshot.config_reload_error.clone(),
        tray_summary,
    }
}

/// Keep the selection on the same host, or move it deliberately.
///
/// A host that merely *reordered* keeps the selection, because the selection
/// is keyed on the id. A host that *disappeared* releases the selection rather
/// than sliding it onto whichever host took its row, because "the window is
/// showing a different machine's numbers" is a far worse failure than "the
/// window is showing nothing selected".
fn repair_selection(selected_id: Option<&str>, systems: &[SystemCard]) -> Option<String> {
    match selected_id {
        Some(id) if systems.iter().any(|card| card.id == id) => Some(id.to_owned()),
        Some(_) => None,
        None => systems.first().map(|card| card.id.clone()),
    }
}

impl FleetCounts {
    /// Count the fleet by state.
    ///
    /// A pending host is counted as pending, never as offline: "not polled
    /// yet" and "the last poll failed" are different facts.
    #[must_use]
    pub fn from_systems(systems: &[SystemSnapshotDto]) -> Self {
        let mut counts = Self {
            total: systems.len(),
            ..Self::default()
        };
        for system in systems {
            match system.reachability {
                Reachability::Online => counts.online += 1,
                Reachability::Pending => counts.pending += 1,
                Reachability::Offline => counts.offline += 1,
            }
        }
        counts
    }
}

/// Build one host's card.
fn card(system: &SystemSnapshotDto, now_unix_ms: u64) -> SystemCard {
    let name = display_name(system);
    let state = host_state(system);
    let metrics = system.latest.as_ref().map(metric_rows).unwrap_or_default();
    let headline = headline(&metrics);
    let age = system
        .last_success_at_unix_ms
        .map(|then| age_text(now_unix_ms.saturating_sub(then)));

    SystemCard {
        id: system.id.clone(),
        name: truncate(&name, MAX_NAME_CHARS),
        endpoint: truncate(&system.endpoint.display_address(), MAX_ENDPOINT_CHARS),
        address: system.endpoint.display_address(),
        configured_name: system.configured_name.clone(),
        status: status_text(&state),
        state,
        headline,
        metrics,
        drives: Vec::new(),
        interfaces: Vec::new(),
        age,
    }
}

/// The host's display name: the configured name, else the endpoint's name,
/// else the host.
fn display_name(system: &SystemSnapshotDto) -> String {
    system
        .configured_name
        .clone()
        .or_else(|| system.endpoint.name.clone())
        .unwrap_or_else(|| system.endpoint.host.clone())
}

/// The host's state, with the failure reason when there is one.
fn host_state(system: &SystemSnapshotDto) -> HostState {
    match system.reachability {
        Reachability::Online => HostState::Online,
        Reachability::Pending => HostState::Pending,
        Reachability::Offline => match &system.offline_reason {
            Some(reason) => HostState::Offline {
                kind: reason.kind.to_string(),
                detail: reason.detail.clone(),
            },
            None => HostState::Offline {
                kind: "unknown".to_owned(),
                detail: None,
            },
        },
    }
}

/// The state in words.
///
/// The category is always there when the host is offline, and the detail
/// follows it when the daemon reported one. A pending host says `pending` and
/// nothing else: there is no reason to report for a host that has not been
/// polled.
fn status_text(state: &HostState) -> String {
    match state {
        HostState::Online => "online".to_owned(),
        HostState::Pending => "pending".to_owned(),
        HostState::Offline { kind, detail } => match detail {
            Some(detail) => format!("offline ({kind}) {detail}"),
            None => format!("offline ({kind})"),
        },
    }
}

/// The share of a row that is used, when the platform reported a usable one.
///
/// A non-finite reading is not a share: the bar is left empty rather than
/// filled with a number nobody measured. An out-of-range reading is clamped,
/// because a bar that runs past its own track is not a measurement either.
fn usable_pct(pct: f32) -> Option<f32> {
    if pct.is_finite() {
        Some(pct.clamp(0.0, 100.0))
    } else {
        None
    }
}

/// Two formatted measurements in one cell, with the unit written once when
/// both carry the same one: `6.1 / 15.5 GiB`, `4.2 / 16.1 KiB/s`.
///
/// When the two land in **different** units — a small `used` against a large
/// `total` — each keeps its own, because dropping either unit would make
/// the number mean something other than what was measured.
fn format_pair(first: &str, second: &str) -> String {
    match (first.rsplit_once(' '), second.rsplit_once(' ')) {
        (Some((first_value, first_unit)), Some((second_value, second_unit)))
            if first_unit == second_unit =>
        {
            format!("{first_value} / {second_value} {first_unit}")
        }
        _ => format!("{first} / {second}"),
    }
}

/// The measured rows for one host.
///
/// The rows that are always present are CPU, memory and disk. The optional
/// families — swap or commit, load, frequency, I/O wait, network — appear
/// only when the platform actually reported them, so a missing row means
/// "this platform does not report it" rather than "the value is zero".
fn metric_rows(latest: &NormalizedSnapshot) -> Vec<MetricRow> {
    let mut rows = vec![
        MetricRow {
            key: "cpu",
            label: "CPU".to_owned(),
            pct: usable_pct(latest.usage_pct),
            pct_text: format_pct(latest.usage_pct),
            value: String::new(),
            summary: format_pct(latest.usage_pct),
        },
        MetricRow {
            key: "memory",
            label: "MEM".to_owned(),
            pct: usable_pct(latest.memory.usage_pct),
            pct_text: format_pct(latest.memory.usage_pct),
            value: format_pair(
                &format_bytes(latest.memory.used_bytes),
                &format_bytes(latest.memory.total_bytes),
            ),
            summary: format!(
                "{} / {} ({})",
                format_bytes(latest.memory.used_bytes),
                format_bytes(latest.memory.total_bytes),
                format_pct(latest.memory.usage_pct)
            ),
        },
    ];

    // Swap and Windows commit are alternatives: a platform reports one.
    if let Some(swap) = latest.swap.as_ref() {
        rows.push(MetricRow {
            key: "swap",
            label: "SWP".to_owned(),
            pct: usable_pct(swap.usage_pct),
            pct_text: format_pct(swap.usage_pct),
            value: format_pair(
                &format_bytes(swap.used_bytes),
                &format_bytes(swap.total_bytes),
            ),
            summary: format!(
                "{} / {} ({})",
                format_bytes(swap.used_bytes),
                format_bytes(swap.total_bytes),
                format_pct(swap.usage_pct)
            ),
        });
    } else if let Some(commit) = latest.commit.as_ref() {
        rows.push(MetricRow {
            key: "commit",
            label: "COMMIT".to_owned(),
            pct: usable_pct(commit.usage_pct),
            pct_text: format_pct(commit.usage_pct),
            value: format_pair(
                &format_bytes(commit.used_bytes),
                &format_bytes(commit.limit_bytes),
            ),
            summary: format!(
                "{} / {} ({})",
                format_bytes(commit.used_bytes),
                format_bytes(commit.limit_bytes),
                format_pct(commit.usage_pct)
            ),
        });
    }

    // The disk aggregate comes from the shared helper, so the GUI and the TUI
    // cannot disagree about the denominator.
    if let Some(drives) = latest.drives.as_ref() {
        if let Some(aggregate) = aggregate_drives(drives) {
            rows.push(MetricRow {
                key: "disk",
                label: "DISK".to_owned(),
                pct: usable_pct(aggregate.usage_pct),
                pct_text: format_pct(aggregate.usage_pct),
                value: format_pair(
                    &format_bytes(aggregate.used_bytes),
                    &format_bytes(aggregate.total_bytes),
                ),
                summary: format!(
                    "{} / {} ({})",
                    format_bytes(aggregate.used_bytes),
                    format_bytes(aggregate.total_bytes),
                    format_pct(aggregate.usage_pct)
                ),
            });
        }
    }

    if let Some(network) = latest.network.as_ref() {
        let rx = format_rate(network.aggregate_rx_bytes_per_sec);
        let tx = format_rate(network.aggregate_tx_bytes_per_sec);
        // The bar's row is the condensed one: the two rates share a unit, and
        // the link's capacity is the denominator the share was taken against.
        let mut value = format_pair(&rx, &tx);
        let mut summary = format!("{rx} / {tx}");
        // The link capacity is optional even when the throughput is not.
        if let Some(capacity) = network
            .aggregate_rx_capacity_bps
            .or(network.aggregate_tx_capacity_bps)
        {
            let capacity = format_capacity(Some(capacity));
            value = format!("{value} ({capacity})");
            summary = format!("{summary} ({capacity})");
        }
        // The network's share of its link is the same measure the TUI draws
        // its own bar from, so the two cannot disagree.
        let pct = network.aggregate_utilization_pct().and_then(usable_pct);
        rows.push(MetricRow {
            key: "network",
            label: "NET".to_owned(),
            pct,
            pct_text: pct.map_or_else(String::new, format_pct),
            value,
            summary,
        });
    }

    if let Some(load) = latest.load.as_ref() {
        rows.push(MetricRow {
            key: "load",
            label: "LOAD".to_owned(),
            pct: None,
            pct_text: String::new(),
            value: format_load(load),
            summary: format_load(load),
        });
    }

    if let Some(frequency) = latest.cpu_frequency_hz {
        rows.push(MetricRow {
            key: "frequency",
            label: "FREQ".to_owned(),
            pct: None,
            pct_text: String::new(),
            value: format_frequency(frequency),
            summary: format_frequency(frequency),
        });
    }

    // I/O wait is only shown when the platform supports it *and* reported it.
    // A platform that does not support it has no row at all, which is a
    // different statement from "the value is missing".
    if latest.cpu_iowait_supported {
        if let Some(iowait) = latest.iowait_pct {
            rows.push(MetricRow {
                key: "iowait",
                label: "IO".to_owned(),
                // I/O wait is a fraction of *time*, not a used share of a
                // *capacity*, so it gets no bar: the bar column means "used
                // share" everywhere else in this pane, and a bar that means
                // something else would misread. The TUI's own metric rows do
                // not carry an IO row either — it is a header token there.
                pct: None,
                pct_text: String::new(),
                value: format_pct(iowait),
                summary: format_pct(iowait),
            });
        }
    }

    rows
}

/// The one-line summary of a host's metrics.
fn headline(metrics: &[MetricRow]) -> String {
    metrics
        .iter()
        .filter(|row| row.key == "cpu" || row.key == "memory")
        .map(|row| format!("{} {}", row.label, row.summary))
        .collect::<Vec<_>>()
        .join("  ")
}

/// The detail rows for the selected host.
#[must_use]
pub fn detail_rows(
    snapshot: &FrontendSnapshot,
    system_id: &str,
) -> (Vec<DriveRow>, Vec<InterfaceRow>) {
    let Some(system) = snapshot.system(system_id) else {
        return (Vec::new(), Vec::new());
    };
    let Some(latest) = system.latest.as_ref() else {
        return (Vec::new(), Vec::new());
    };

    let drives: Vec<DriveRow> = latest
        .drives
        .as_ref()
        .map(|drives| {
            drives
                .iter()
                .map(|drive| DriveRow {
                    mount: truncate(&drive.name, MAX_NAME_CHARS),
                    usage: format!(
                        "{} / {}",
                        format_bytes(drive.used_bytes),
                        format_bytes(drive.total_bytes)
                    ),
                    remaining: drive.available_bytes.map(format_bytes),
                    throughput: None,
                })
                .collect()
        })
        .unwrap_or_default();

    // The per-device throughput belongs to the disk-IO plane, which is
    // reported separately from the capacity plane.
    let throughput: std::collections::HashMap<&str, String> = latest
        .disk_io
        .as_ref()
        .map(|io| {
            io.devices
                .iter()
                .filter_map(|device| {
                    device.drive_name.as_ref().map(|name| {
                        (
                            name.as_str(),
                            format!(
                                "{} R / {} W",
                                format_rate(device.read_bytes_per_sec),
                                format_rate(device.write_bytes_per_sec)
                            ),
                        )
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let drives = drives
        .into_iter()
        .map(|mut drive| {
            drive.throughput = throughput.get(drive.mount.as_str()).cloned();
            drive
        })
        .collect();

    // Loopback appears in the interface detail only. It is never part of an
    // aggregate, because a loopback's traffic is the host talking to itself.
    let interfaces = latest
        .network
        .as_ref()
        .map(|network| {
            network
                .interfaces
                .iter()
                .map(|interface| InterfaceRow {
                    name: truncate(&interface.name, MAX_NAME_CHARS),
                    throughput: format!(
                        "{} Rx / {} Tx",
                        format_rate(interface.rx_bytes_per_sec),
                        format_rate(interface.tx_bytes_per_sec)
                    ),
                    capacity: interface
                        .rx_capacity_bps
                        .or(interface.tx_capacity_bps)
                        .map(|capacity| format_capacity(Some(capacity))),
                    loopback: interface.is_loopback,
                })
                .collect()
        })
        .unwrap_or_default();

    (drives, interfaces)
}

/// The cron plane of one host.
fn cron_view(entry: &SystemCronDto, selection: &Selection, now_unix_ms: u64) -> CronView {
    let supported = !matches!(entry.capability, CronCapability::Unsupported);
    let jobs = entry
        .summary
        .as_ref()
        .map(|summary| {
            summary
                .jobs
                .iter()
                .map(|job| CronJobRow {
                    name: truncate(&job.name, MAX_NAME_CHARS),
                    // The state word and the outcome word are the *shared*
                    // ones, so the GUI cannot drift from the TUI about what
                    // "load delayed" or "spawn failed" mean.
                    state: gregg::cron::state_label(job.state),
                    outcome: job
                        .last
                        .as_ref()
                        .map(|last| gregg::cron::outcome_label(last.outcome))
                        .unwrap_or_else(|| "no result yet".to_owned()),
                    load_gate: gregg::cron::load_gate_label(job, now_unix_ms),
                    running: gregg::cron::job_is_running(job),
                    deferred: gregg::cron::job_is_load_deferred(job),
                })
                .collect()
        })
        .unwrap_or_default();

    // The records are only the ones the daemon published for the job this
    // frontend has open, so a closed view carries no history.
    let open_job = selection
        .cron_job
        .as_ref()
        .filter(|(system_id, _)| system_id == &entry.system_id)
        .map(|(_, job)| job.as_str());
    let records = open_job
        .map(|job| {
            entry
                .job_records(job)
                .iter()
                .flat_map(record_lines)
                .collect()
        })
        .unwrap_or_default();

    // The retained data may be out of date when the last scheduler read
    // failed, which is a different fact from the host being unreachable.
    let stale = entry.last_error.is_some();

    CronView {
        supported,
        jobs,
        records,
        stale,
    }
}

/// One record's lines: a headline, then the bounded output.
///
/// Remote output is hostile display text. It is escaped rather than stripped,
/// so a control sequence stays visible as `^[` instead of silently changing
/// what the operator sees, and it is never interpreted as styling.
fn record_lines(record: &gregg::cron::CronRecord) -> Vec<String> {
    let run = &record.record;
    let mut lines = vec![format!(
        "{}  {}",
        gregg::cron::outcome_label(run.outcome),
        gregg::cron::duration_label(run.duration_ms.unwrap_or(0))
    )];
    for (label, output) in [("out", &run.stdout), ("err", &run.stderr)] {
        let text = gregg::sanitize::sanitize_single_line(&output.text).text;
        if !text.is_empty() {
            lines.push(format!("{label} {text}"));
        }
        if output.truncated {
            // Remote truncation is reported separately from any local
            // viewport truncation, because they mean different things.
            lines.push(format!("{label} truncated by the remote"));
        }
    }
    lines
}

/// The `EggPool` plane.
fn eggpool_view(snapshot: &EggpoolSnapshotDto) -> EggpoolView {
    // Exactly the four summary values the DTO exposes, and no more.
    let values = snapshot
        .summary
        .as_ref()
        .map(|summary| {
            vec![
                ("tokens".to_owned(), format!("{}", summary.accounted_tokens)),
                (
                    "cache read".to_owned(),
                    summary
                        .cache_read_ratio
                        .map(|ratio| format!("{:.1}%", ratio * 100.0))
                        .unwrap_or_else(|| MISSING.to_owned()),
                ),
                (
                    "output".to_owned(),
                    format!("{:.1} tok/s", summary.output_tokens_per_second),
                ),
                (
                    "ttft".to_owned(),
                    summary
                        .avg_ttft_ms
                        .map(|ms| format!("{ms:.0} ms"))
                        .unwrap_or_else(|| MISSING.to_owned()),
                ),
            ]
        })
        .unwrap_or_default();
    let health = snapshot
        .health
        .as_ref()
        .map(|health| health.proxy.as_str().to_owned())
        .unwrap_or_else(|| "unknown".to_owned());
    let ready = snapshot
        .health
        .as_ref()
        .is_some_and(|health| health.proxy == gregg::eggpool::EggpoolProxyHealth::Ready);
    let error = snapshot
        .last_error
        .as_ref()
        .map(gregg::eggpool::outcome_text)
        .filter(|text| !text.is_empty())
        .or_else(|| {
            snapshot
                .last_health_error
                .as_ref()
                .map(|error| gregg::eggpool::health_error_text(error).to_owned())
        });

    EggpoolView {
        period: snapshot.period,
        values,
        health,
        ready,
        error,
    }
}

/// The one-line fleet summary the tray shows.
fn tray_summary(counts: &FleetCounts, systems: &[SystemCard]) -> String {
    let mut summary = format!(
        "{} online, {} pending, {} offline",
        counts.online, counts.pending, counts.offline
    );
    if counts.total == 0 {
        summary.push_str(" — no systems configured");
        return summary;
    }
    let names: Vec<&str> = systems
        .iter()
        .take(MAX_TRAY_HOSTS)
        .map(|card| card.name.as_str())
        .collect();
    summary.push_str(" — ");
    summary.push_str(&names.join(", "));
    if systems.len() > MAX_TRAY_HOSTS {
        summary.push_str(&format!(" (+{} more)", systems.len() - MAX_TRAY_HOSTS));
    }
    summary
}

/// How long ago a poll succeeded, in words.
fn age_text(elapsed_ms: u64) -> String {
    let seconds = elapsed_ms / 1000;
    if seconds < 60 {
        return format!("{seconds}s ago");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    format!("{}h ago", minutes / 60)
}

/// Truncate a string to a character budget, with an ellipsis when it did not
/// fit.
///
/// Character-based rather than byte-based, so a multi-byte name is not cut in
/// the middle of a character.
#[must_use]
pub fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut truncated: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    truncated.push('\u{2026}');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use gregg::clientd::snapshot::{CronJobHistoryDto, SystemCronDto, SystemSnapshotDto};
    use gregg::config::{EggpoolEntry, EggpoolScheme};
    use gregg::cron::{CronFetchError, CronRecord};
    use gregg::eggpool::{EggpoolHealthSnapshot, EggpoolProxyHealth, EggpoolSummary};
    use gregg::endpoint::Endpoint;
    use gregg::normalized::{
        CommitMetrics, NormalizedDrive, NormalizedNetwork, NormalizedNetworkInterface, SwapMetrics,
    };
    use gregg::poller::{OfflineKind, OfflineReason};
    use gregg::state::EggpoolWorkerState;
    use gregg_protocol::MemoryMetrics;
    use gregg_protocol::{
        SchedulerEpochV2, SchedulerJobStateV2, SchedulerJobV2, SchedulerOutcomeV2,
        SchedulerOutputV2, SchedulerRunRecordV2, SchedulerSummaryV2,
    };

    fn endpoint(index: usize) -> Endpoint {
        Endpoint {
            id: format!("system-{index}"),
            host: format!("host-{index}"),
            port: 11310,
            name: None,
        }
    }

    fn online(index: usize) -> SystemSnapshotDto {
        SystemSnapshotDto {
            id: format!("system-{index}"),
            endpoint: endpoint(index),
            configured_name: None,
            reachability: Reachability::Online,
            latest: None,
            last_success_at_unix_ms: Some(1_000),
            last_attempt_at_unix_ms: Some(1_000),
            latency_ms: Some(5),
            offline_reason: None,
        }
    }

    fn with_snapshot(index: usize, latest: NormalizedSnapshot) -> SystemSnapshotDto {
        let mut system = online(index);
        system.latest = Some(latest);
        system
    }

    fn normalized() -> NormalizedSnapshot {
        NormalizedSnapshot {
            wire_version: 2,
            observed_at_unix_ms: 1_000,
            sample_interval_ms: 5_000,
            cpu_iowait_supported: true,
            load_supported: true,
            swap_supported: true,
            commit_supported: false,
            system: gregg_protocol::SystemIdentity {
                name: "host-0".to_owned(),
                hostname: "host-0".to_owned(),
                os_name: "Linux".to_owned(),
                os_version: "6".to_owned(),
                kernel_name: "Linux".to_owned(),
                kernel_release: "6.1".to_owned(),
                architecture: "x86_64".to_owned(),
            },
            logical_cores: 8,
            usage_pct: 12.5,
            iowait_pct: Some(1.5),
            cpu_frequency_hz: Some(3_000_000_000),
            load: Some(gregg_protocol::LoadAverage {
                one: 0.5,
                five: 0.4,
                fifteen: 0.3,
            }),
            memory: MemoryMetrics {
                used_bytes: 4_000_000_000,
                total_bytes: 16_000_000_000,
                usage_pct: 25.0,
            },
            swap: Some(SwapMetrics {
                used_bytes: 0,
                total_bytes: 0,
                usage_pct: 0.0,
            }),
            commit: None,
            drives: Some(vec![NormalizedDrive {
                name: "/".to_owned(),
                used_bytes: 50_000_000_000,
                total_bytes: 100_000_000_000,
                available_bytes: Some(50_000_000_000),
            }]),
            disk_io: None,
            network: None,
        }
    }

    fn snapshot_of(systems: Vec<SystemSnapshotDto>) -> FrontendSnapshot {
        FrontendSnapshot::empty(systems)
    }

    /// The connected state most of these projections assume.
    fn connected() -> ConnectionState {
        ConnectionState::Connected {
            protocol_version: 1,
            daemon_id: "abc".to_owned(),
        }
    }

    /// A scheduler summary carrying the given jobs.
    fn cron_summary(jobs: Vec<SchedulerJobV2>) -> SchedulerSummaryV2 {
        SchedulerSummaryV2 {
            schema_version: 2,
            generated_at_unix_ms: 1_000,
            epoch: SchedulerEpochV2 {
                started_at_unix_ms: 1_000,
                nonce: 7,
            },
            history_revision: 1,
            jobs,
        }
    }

    /// One scheduled job, in the state the test chooses.
    fn cron_job(name: &str, state: SchedulerJobStateV2) -> SchedulerJobV2 {
        SchedulerJobV2 {
            name: name.to_owned(),
            schedule: "0 * * * *".to_owned(),
            next_due_unix_ms: 60_000,
            state,
            load: None,
            pending_since_unix_ms: None,
            next_retry_unix_ms: None,
            running_since_unix_ms: None,
            last: None,
        }
    }

    /// One retained terminal record.
    fn cron_record(
        sequence: u64,
        outcome: SchedulerOutcomeV2,
        stdout: SchedulerOutputV2,
        stderr: SchedulerOutputV2,
    ) -> CronRecord {
        CronRecord {
            epoch: SchedulerEpochV2 {
                started_at_unix_ms: 1_000,
                nonce: 7,
            },
            record: SchedulerRunRecordV2 {
                sequence,
                scheduled_unix_ms: 1_000,
                started_unix_ms: Some(1_000),
                finished_unix_ms: 2_000,
                outcome,
                exit_code: Some(0),
                signal: None,
                duration_ms: Some(250),
                delay_ms: 0,
                coalesced: false,
                stdout,
                stderr,
            },
        }
    }

    /// One stream's bounded output.
    fn output(text: &str) -> SchedulerOutputV2 {
        SchedulerOutputV2 {
            text: text.to_owned(),
            truncated: false,
        }
    }

    /// One system's scheduler state.
    fn cron_entry(
        system_id: &str,
        capability: CronCapability,
        summary: Option<SchedulerSummaryV2>,
        history: Vec<CronJobHistoryDto>,
        last_error: Option<CronFetchError>,
    ) -> SystemCronDto {
        SystemCronDto {
            system_id: system_id.to_owned(),
            capability,
            summary,
            epoch: Some(SchedulerEpochV2 {
                started_at_unix_ms: 1_000,
                nonce: 7,
            }),
            history_revision: Some(1),
            last_attempt_at_unix_ms: Some(1_000),
            last_success_at_unix_ms: Some(1_000),
            last_error,
            history,
        }
    }

    /// A one-host document carrying the given scheduler state.
    fn with_cron(entry: SystemCronDto) -> FrontendSnapshot {
        let mut snapshot = snapshot_of(vec![with_snapshot(0, normalized())]);
        snapshot.cron = vec![entry];
        snapshot
    }

    /// An `EggPool` snapshot the test can adjust.
    fn eggpool_snapshot(
        health: Option<EggpoolHealthSnapshot>,
        summary: Option<EggpoolSummary>,
    ) -> EggpoolSnapshotDto {
        EggpoolSnapshotDto {
            endpoint: EggpoolEntry {
                id: "eggpool".to_owned(),
                host: "127.0.0.1".to_owned(),
                port: 8080,
                scheme: EggpoolScheme::Http,
                name: None,
                api_key_env: None,
            },
            period: EggpoolPeriod::Day,
            request_generation: 7,
            worker_state: EggpoolWorkerState::Idle,
            summary,
            last_success_at_unix_ms: Some(1_000),
            last_attempt_at_unix_ms: Some(1_000),
            last_error: None,
            health,
            last_health_success_at_unix_ms: Some(1_000),
            last_health_attempt_at_unix_ms: Some(1_000),
            last_health_error: None,
        }
    }

    /// A service-health snapshot reporting the given proxy state.
    fn eggpool_health(proxy: EggpoolProxyHealth) -> EggpoolHealthSnapshot {
        EggpoolHealthSnapshot {
            schema_version: 1,
            proxy,
            available: proxy == EggpoolProxyHealth::Ready,
            reason_code: None,
            uptime_seconds: None,
            model_count: None,
            routable_accounts: None,
            enabled_accounts: None,
            providers: Vec::new(),
        }
    }

    /// A summary for the given period.
    fn eggpool_summary(period: EggpoolPeriod) -> EggpoolSummary {
        EggpoolSummary {
            accounted_tokens: 1_234,
            cache_read_ratio: Some(0.25),
            output_tokens_per_second: 12.5,
            avg_ttft_ms: Some(120.0),
            period,
        }
    }

    /// A one-host document carrying the given `EggPool` state.
    fn with_eggpool(eggpool: EggpoolSnapshotDto) -> FrontendSnapshot {
        let mut snapshot = snapshot_of(vec![with_snapshot(0, normalized())]);
        snapshot.eggpool = Some(eggpool);
        snapshot
    }

    #[test]
    fn a_pending_host_is_pending_and_never_offline() {
        let mut system = online(0);
        system.reachability = Reachability::Pending;
        system.latest = None;
        system.offline_reason = None;
        let model = project(
            &snapshot_of(vec![system]),
            &Selection::default(),
            &ConnectionState::Connected {
                protocol_version: 1,
                daemon_id: "abc".to_owned(),
            },
            1_000,
        );
        assert_eq!(model.counts.pending, 1);
        assert_eq!(model.counts.offline, 0);
        assert_eq!(model.counts.online, 0);
        assert_eq!(model.systems[0].status, "pending");
    }

    #[test]
    fn an_offline_host_states_why_and_keeps_the_detail() {
        let mut system = online(0);
        system.reachability = Reachability::Offline;
        system.latest = None;
        system.offline_reason = Some(OfflineReason::with_detail(OfflineKind::Http, "HTTP 503"));
        let model = project(
            &snapshot_of(vec![system]),
            &Selection::default(),
            &ConnectionState::Connected {
                protocol_version: 1,
                daemon_id: "abc".to_owned(),
            },
            1_000,
        );
        assert_eq!(model.counts.offline, 1);
        assert_eq!(model.systems[0].status, "offline (http) HTTP 503");
    }

    #[test]
    fn a_host_with_no_reading_shows_no_metric_rows_rather_than_zeroes() {
        let system = online(0);
        let model = project(
            &snapshot_of(vec![system]),
            &Selection::default(),
            &ConnectionState::Connected {
                protocol_version: 1,
                daemon_id: "abc".to_owned(),
            },
            1_000,
        );
        assert!(
            model.systems[0].metrics.is_empty(),
            "an unpolled host must have no metrics at all"
        );
        assert!(!model.systems[0].headline.contains("0.0%"));
    }

    /// The bar column means "used share of a capacity", so only the rows
    /// that carry such a share draw a bar.
    ///
    /// The TUI's metric rows are the five that do: CPU, MEM, SWP-or-COMMIT,
    /// DISK and NET. LOAD, FREQ and IO are extra readings the TUI's condensed
    /// table carries, and none of them is a used share — I/O wait in
    /// particular is a fraction of *time*, and a bar that meant something
    /// different from every other bar in the pane would misread.
    #[test]
    fn only_a_used_share_draws_a_bar() {
        let model = project(
            &snapshot_of(vec![with_snapshot(0, normalized())]),
            &Selection::default(),
            &ConnectionState::Connected {
                protocol_version: 1,
                daemon_id: "abc".to_owned(),
            },
            1_000,
        );
        let rows = &model.systems[0].metrics;
        for row in rows {
            match row.key {
                "cpu" | "memory" | "swap" | "commit" | "disk" | "network" => {
                    assert!(
                        row.pct.is_some(),
                        "{} is a used share and must draw a bar",
                        row.key
                    );
                }
                "load" | "frequency" | "iowait" => {
                    assert!(
                        row.pct.is_none(),
                        "{} is not a used share and must not draw a bar",
                        row.key
                    );
                }
                other => panic!("an unknown metric row: {other}"),
            }
        }
        // The row that has no share still carries its reading, in the value
        // the pane shows beside the bar.
        let iowait = rows
            .iter()
            .find(|row| row.key == "iowait")
            .expect("an IO row");
        assert_eq!(iowait.value, "1.5%");
        assert!(iowait.pct_text.is_empty());
    }

    #[test]
    fn an_unsupported_family_has_no_row_and_a_missing_value_is_a_dash() {
        let mut latest = normalized();
        // A platform that does not support I/O wait reports no value and
        // declares the family unsupported.
        latest.cpu_iowait_supported = false;
        latest.iowait_pct = None;
        let model = project(
            &snapshot_of(vec![with_snapshot(0, latest)]),
            &Selection::default(),
            &ConnectionState::Connected {
                protocol_version: 1,
                daemon_id: "abc".to_owned(),
            },
            1_000,
        );
        let keys: Vec<&str> = model.systems[0].metrics.iter().map(|row| row.key).collect();
        assert!(!keys.contains(&"iowait"), "{keys:?}");
        assert!(keys.contains(&"cpu"));
        assert!(keys.contains(&"memory"));
        assert!(keys.contains(&"disk"));
    }

    #[test]
    fn swap_and_commit_are_alternatives_and_commit_is_labelled_commit() {
        let mut latest = normalized();
        latest.swap = None;
        latest.commit = Some(CommitMetrics {
            used_bytes: 1_000,
            limit_bytes: 4_000,
            usage_pct: 25.0,
        });
        let model = project(
            &snapshot_of(vec![with_snapshot(0, latest)]),
            &Selection::default(),
            &ConnectionState::Connected {
                protocol_version: 1,
                daemon_id: "abc".to_owned(),
            },
            1_000,
        );
        let labels: Vec<&str> = model.systems[0]
            .metrics
            .iter()
            .map(|row| row.label.as_str())
            .collect();
        assert!(labels.contains(&"COMMIT"), "{labels:?}");
        assert!(!labels.contains(&"SWP"), "{labels:?}");
    }

    #[test]
    fn the_disk_aggregate_uses_the_shared_helper() {
        let latest = normalized();
        let model = project(
            &snapshot_of(vec![with_snapshot(0, latest)]),
            &Selection::default(),
            &ConnectionState::Connected {
                protocol_version: 1,
                daemon_id: "abc".to_owned(),
            },
            1_000,
        );
        let disk = model.systems[0]
            .metrics
            .iter()
            .find(|row| row.key == "disk")
            .expect("a disk row");
        // Half of the total is 50%, and the denominator is the *total*, not
        // the available space.
        assert!(disk.pct_text.contains("50.0%"), "{}", disk.pct_text);
        assert!(
            disk.value.contains("93.1 GiB"),
            "the denominator must be the total: {}",
            disk.value
        );
        // The bar's row is the condensed one, so the unit is written once.
        assert!(disk.value.contains(" / "), "{}", disk.value);
        assert_eq!(disk.value.matches("GiB").count(), 1, "{}", disk.value);
    }

    #[test]
    fn the_network_aggregate_ignores_loopback() {
        let mut latest = normalized();
        latest.network = Some(NormalizedNetwork {
            aggregate_rx_bytes_per_sec: 1_000,
            aggregate_tx_bytes_per_sec: 2_000,
            aggregate_rx_capacity_bps: Some(1_000_000_000),
            aggregate_tx_capacity_bps: Some(1_000_000_000),
            interfaces: vec![
                NormalizedNetworkInterface {
                    id: "eth0".to_owned(),
                    name: "eth0".to_owned(),
                    rx_bytes_per_sec: 1_000,
                    tx_bytes_per_sec: 2_000,
                    rx_capacity_bps: Some(1_000_000_000),
                    tx_capacity_bps: Some(1_000_000_000),
                    is_loopback: false,
                    aggregate_member: true,
                },
                NormalizedNetworkInterface {
                    id: "lo".to_owned(),
                    name: "lo".to_owned(),
                    rx_bytes_per_sec: 9_000,
                    tx_bytes_per_sec: 9_000,
                    rx_capacity_bps: None,
                    tx_capacity_bps: None,
                    is_loopback: true,
                    aggregate_member: false,
                },
            ],
        });
        let model = project(
            &snapshot_of(vec![with_snapshot(0, latest.clone())]),
            &Selection::default(),
            &ConnectionState::Connected {
                protocol_version: 1,
                daemon_id: "abc".to_owned(),
            },
            1_000,
        );
        let net = model.systems[0]
            .metrics
            .iter()
            .find(|row| row.key == "network")
            .expect("a network row");
        assert!(net.value.contains("1000 B/s"), "{}", net.value);
        assert!(!net.value.contains("9000 B/s"), "{}", net.value);
    }

    #[test]
    fn the_selection_follows_the_host_and_not_the_row() {
        let first = online(0);
        let mut second = online(1);
        second.configured_name = Some("second".to_owned());
        let selection = Selection {
            selected_id: Some("system-1".to_owned()),
            cron_job: None,
        };

        // The order changes; the selection must not.
        let model = project(
            &snapshot_of(vec![second.clone(), first.clone()]),
            &selection,
            &ConnectionState::Connected {
                protocol_version: 1,
                daemon_id: "abc".to_owned(),
            },
            1_000,
        );
        assert_eq!(model.selected_id.as_deref(), Some("system-1"));
        assert_eq!(
            model.selected.as_ref().map(|card| card.id.as_str()),
            Some("system-1")
        );
    }

    #[test]
    fn a_disappearing_host_releases_the_selection_instead_of_sliding_it() {
        let selection = Selection {
            selected_id: Some("system-1".to_owned()),
            cron_job: None,
        };
        let model = project(
            &snapshot_of(vec![online(0)]),
            &selection,
            &ConnectionState::Connected {
                protocol_version: 1,
                daemon_id: "abc".to_owned(),
            },
            1_000,
        );
        assert_eq!(
            model.selected_id, None,
            "the window must not show a different host's numbers"
        );
        assert!(model.selected.is_none());
    }

    #[test]
    fn a_returning_host_takes_the_selection_back() {
        let selection = Selection {
            selected_id: Some("system-1".to_owned()),
            cron_job: None,
        };
        let model = project(
            &snapshot_of(vec![online(0), online(1)]),
            &selection,
            &ConnectionState::Connected {
                protocol_version: 1,
                daemon_id: "abc".to_owned(),
            },
            1_000,
        );
        assert_eq!(model.selected_id.as_deref(), Some("system-1"));
    }

    #[test]
    fn a_first_projection_selects_the_first_host() {
        let model = project(
            &snapshot_of(vec![online(0), online(1)]),
            &Selection::default(),
            &ConnectionState::Connected {
                protocol_version: 1,
                daemon_id: "abc".to_owned(),
            },
            1_000,
        );
        assert_eq!(model.selected_id.as_deref(), Some("system-0"));
    }

    #[test]
    fn the_tray_summary_counts_every_state_and_bounds_the_names() {
        let systems: Vec<SystemSnapshotDto> = (0..8)
            .map(|index| {
                let mut system = online(index);
                system.configured_name = Some(format!("host-{index}"));
                system
            })
            .collect();
        let model = project(
            &snapshot_of(systems),
            &Selection::default(),
            &ConnectionState::Connected {
                protocol_version: 1,
                daemon_id: "abc".to_owned(),
            },
            1_000,
        );
        assert!(
            model.tray_summary.contains("8 online"),
            "{}",
            model.tray_summary
        );
        assert!(
            model.tray_summary.contains("(+3 more)"),
            "{}",
            model.tray_summary
        );
    }

    #[test]
    fn an_empty_fleet_says_so_rather_than_showing_zeroes() {
        let model = project(
            &snapshot_of(Vec::new()),
            &Selection::default(),
            &ConnectionState::Connected {
                protocol_version: 1,
                daemon_id: "abc".to_owned(),
            },
            1_000,
        );
        assert!(model.tray_summary.contains("no systems configured"));
    }

    #[test]
    fn a_long_name_is_truncated_on_a_character_boundary() {
        let name = "a".repeat(60);
        let truncated = truncate(&name, 10);
        assert_eq!(truncated.chars().count(), 10);
        assert!(truncated.ends_with('\u{2026}'));
    }

    #[test]
    fn a_unicode_name_is_not_cut_in_the_middle_of_a_character() {
        let name = "héllo wörld ".repeat(4);
        let truncated = truncate(&name, 8);
        assert!(truncated.chars().count() <= 8);
        assert!(name.starts_with(&truncated[..truncated.len() - 3]));
    }

    #[test]
    fn a_detail_projection_uses_the_daemons_own_rows() {
        let mut latest = normalized();
        latest.network = Some(NormalizedNetwork {
            aggregate_rx_bytes_per_sec: 0,
            aggregate_tx_bytes_per_sec: 0,
            aggregate_rx_capacity_bps: None,
            aggregate_tx_capacity_bps: None,
            interfaces: vec![NormalizedNetworkInterface {
                id: "lo".to_owned(),
                name: "lo".to_owned(),
                rx_bytes_per_sec: 0,
                tx_bytes_per_sec: 0,
                rx_capacity_bps: None,
                tx_capacity_bps: None,
                is_loopback: true,
                aggregate_member: false,
            }],
        });
        let snapshot = snapshot_of(vec![with_snapshot(0, latest)]);
        let (drives, interfaces) = detail_rows(&snapshot, "system-0");
        assert_eq!(drives.len(), 1);
        assert_eq!(drives[0].mount, "/");
        assert_eq!(drives[0].remaining.as_deref(), Some("46.6 GiB"));
        assert_eq!(interfaces.len(), 1);
        assert!(interfaces[0].loopback);
    }

    #[test]
    fn a_detail_projection_for_an_unknown_host_is_empty_rather_than_stale() {
        let snapshot = snapshot_of(vec![with_snapshot(0, normalized())]);
        let (drives, interfaces) = detail_rows(&snapshot, "system-9");
        assert!(drives.is_empty());
        assert!(interfaces.is_empty());
    }

    // ---------------------------------------------------------------------
    // The cron plane.
    // ---------------------------------------------------------------------

    #[test]
    fn an_unsupported_scheduler_is_not_a_host_outage() {
        let snapshot = with_cron(cron_entry(
            "system-0",
            CronCapability::Unsupported,
            None,
            Vec::new(),
            None,
        ));
        let model = project(&snapshot, &Selection::default(), &connected(), 1_000);
        let cron = model.cron.expect("a cron plane");
        assert!(
            !cron.supported,
            "a remote without the routes is not supported"
        );
        // The host's own reachability wording is untouched: an old daemon is a
        // healthy daemon whose metrics are current.
        assert_eq!(model.systems[0].status, "online");
        assert_eq!(model.counts.online, 1);
        assert_eq!(model.counts.offline, 0);
        assert_eq!(model.counts.pending, 0);
    }

    #[test]
    fn a_stale_cron_summary_is_not_a_host_outage() {
        let snapshot = with_cron(cron_entry(
            "system-0",
            CronCapability::Supported,
            Some(cron_summary(vec![cron_job(
                "backup",
                SchedulerJobStateV2::Idle,
            )])),
            Vec::new(),
            Some(CronFetchError::Transport(
                "the scheduler route answered 503".to_owned(),
            )),
        ));
        let model = project(&snapshot, &Selection::default(), &connected(), 1_000);
        let cron = model.cron.expect("a cron plane");
        assert!(cron.stale, "a failed scheduler read is stale data");
        // The retained summary is still shown, and the host is still online:
        // the scheduler plane fails independently of the metrics plane.
        assert_eq!(cron.jobs.len(), 1);
        assert_eq!(model.systems[0].status, "online");
        assert_eq!(model.counts.offline, 0);
    }

    #[test]
    fn only_the_open_job_records_are_shown() {
        let snapshot = with_cron(cron_entry(
            "system-0",
            CronCapability::Supported,
            Some(cron_summary(vec![
                cron_job("backup", SchedulerJobStateV2::Idle),
                cron_job("cleanup", SchedulerJobStateV2::Idle),
            ])),
            vec![
                CronJobHistoryDto {
                    job: "backup".to_owned(),
                    records: vec![cron_record(
                        1,
                        SchedulerOutcomeV2::Success,
                        output("the backup ran"),
                        output(""),
                    )],
                },
                CronJobHistoryDto {
                    job: "cleanup".to_owned(),
                    records: vec![cron_record(
                        2,
                        SchedulerOutcomeV2::Failed,
                        output("the cleanup ran"),
                        output(""),
                    )],
                },
            ],
            None,
        ));
        let selection = Selection {
            selected_id: Some("system-0".to_owned()),
            cron_job: Some(("system-0".to_owned(), "backup".to_owned())),
        };
        let model = project(&snapshot, &selection, &connected(), 1_000);
        let cron = model.cron.expect("a cron plane");
        assert!(
            cron.records
                .iter()
                .any(|line| line.contains("the backup ran")),
            "{:?}",
            cron.records
        );
        assert!(
            !cron
                .records
                .iter()
                .any(|line| line.contains("the cleanup ran")),
            "a job nobody opened must not show its records: {:?}",
            cron.records
        );
    }

    #[test]
    fn a_changed_job_name_shows_that_job_records() {
        let snapshot = with_cron(cron_entry(
            "system-0",
            CronCapability::Supported,
            Some(cron_summary(vec![
                cron_job("backup", SchedulerJobStateV2::Idle),
                cron_job("cleanup", SchedulerJobStateV2::Idle),
            ])),
            vec![
                CronJobHistoryDto {
                    job: "backup".to_owned(),
                    records: vec![cron_record(
                        1,
                        SchedulerOutcomeV2::Success,
                        output("the backup ran"),
                        output(""),
                    )],
                },
                CronJobHistoryDto {
                    job: "cleanup".to_owned(),
                    records: vec![cron_record(
                        2,
                        SchedulerOutcomeV2::Failed,
                        output("the cleanup ran"),
                        output(""),
                    )],
                },
            ],
            None,
        ));
        let selection = Selection {
            selected_id: Some("system-0".to_owned()),
            cron_job: Some(("system-0".to_owned(), "cleanup".to_owned())),
        };
        let model = project(&snapshot, &selection, &connected(), 1_000);
        let cron = model.cron.expect("a cron plane");
        assert!(
            cron.records
                .iter()
                .any(|line| line.contains("the cleanup ran")),
            "{:?}",
            cron.records
        );
        assert!(
            !cron
                .records
                .iter()
                .any(|line| line.contains("the backup ran")),
            "the previously open job must stop showing its records: {:?}",
            cron.records
        );
    }

    #[test]
    fn a_cron_job_of_another_host_shows_no_records() {
        let snapshot = with_cron(cron_entry(
            "system-0",
            CronCapability::Supported,
            Some(cron_summary(vec![cron_job(
                "backup",
                SchedulerJobStateV2::Idle,
            )])),
            vec![CronJobHistoryDto {
                job: "backup".to_owned(),
                records: vec![cron_record(
                    1,
                    SchedulerOutcomeV2::Success,
                    output("the backup ran"),
                    output(""),
                )],
            }],
            None,
        ));
        let selection = Selection {
            selected_id: Some("system-0".to_owned()),
            // The open job belongs to a *different* host.
            cron_job: Some(("system-9".to_owned(), "backup".to_owned())),
        };
        let model = project(&snapshot, &selection, &connected(), 1_000);
        let cron = model.cron.expect("a cron plane");
        assert!(
            cron.records.is_empty(),
            "another host's job must not borrow this host's records: {:?}",
            cron.records
        );
    }

    #[test]
    fn a_closed_cron_view_shows_no_records() {
        let snapshot = with_cron(cron_entry(
            "system-0",
            CronCapability::Supported,
            Some(cron_summary(vec![cron_job(
                "backup",
                SchedulerJobStateV2::Idle,
            )])),
            vec![CronJobHistoryDto {
                job: "backup".to_owned(),
                records: vec![cron_record(
                    1,
                    SchedulerOutcomeV2::Success,
                    output("the backup ran"),
                    output(""),
                )],
            }],
            None,
        ));
        let selection = Selection {
            selected_id: Some("system-0".to_owned()),
            cron_job: None,
        };
        let model = project(&snapshot, &selection, &connected(), 1_000);
        let cron = model.cron.expect("a cron plane");
        assert!(
            cron.records.is_empty(),
            "a closed view must not carry records: {:?}",
            cron.records
        );
    }

    #[test]
    fn a_cron_record_line_uses_the_shared_words() {
        let record = cron_record(1, SchedulerOutcomeV2::SpawnFailed, output(""), output(""));
        let lines = record_lines(&record);
        assert_eq!(
            lines.first().map(String::as_str),
            Some(
                format!(
                    "{}  {}",
                    gregg::cron::outcome_label(SchedulerOutcomeV2::SpawnFailed),
                    gregg::cron::duration_label(250)
                )
                .as_str()
            ),
            "{lines:?}"
        );
    }

    #[test]
    fn remote_truncation_is_reported_apart_from_the_output() {
        let record = cron_record(
            1,
            SchedulerOutcomeV2::Success,
            SchedulerOutputV2 {
                text: "the beginning of the output".to_owned(),
                truncated: true,
            },
            output(""),
        );
        let lines = record_lines(&record);
        // The text is still shown: the remote's bound is a fact about the
        // output, not a replacement for it.
        assert!(
            lines
                .iter()
                .any(|line| line == "out the beginning of the output"),
            "{lines:?}"
        );
        // And the truncation is reported on a line of its own.
        assert!(
            lines
                .iter()
                .any(|line| line == "out truncated by the remote"),
            "{lines:?}"
        );
        // A remote truncation is not a local viewport truncation, so the local
        // marker is never invented here.
        assert!(
            !lines
                .iter()
                .any(|line| line.contains("more cron rows not shown")),
            "{lines:?}"
        );
    }

    #[test]
    fn remote_output_is_escaped_rather_than_stripped() {
        let hostile = "before\u{1b}[2Jafter";
        let record = cron_record(1, SchedulerOutcomeV2::Success, output(hostile), output(""));
        let lines = record_lines(&record);
        let expected = gregg::sanitize::sanitize_single_line(hostile).text;
        assert_eq!(expected, "before^[[2Jafter");
        assert!(
            lines.iter().any(|line| line == &format!("out {expected}")),
            "{lines:?}"
        );
        // The escape sequence is visible, not executed and not removed.
        assert!(
            !lines.iter().any(|line| line.contains('\u{1b}')),
            "{lines:?}"
        );
    }

    // ---------------------------------------------------------------------
    // The `EggPool` plane.
    // ---------------------------------------------------------------------

    #[test]
    fn a_missing_health_snapshot_is_not_ready() {
        let snapshot = with_eggpool(eggpool_snapshot(
            None,
            Some(eggpool_summary(EggpoolPeriod::Day)),
        ));
        let model = project(&snapshot, &Selection::default(), &connected(), 1_000);
        let eggpool = model.eggpool.expect("an eggpool plane");
        assert!(!eggpool.ready, "an unknown health is not a ready health");
        assert_eq!(eggpool.health, "unknown");
    }

    #[test]
    fn only_a_ready_proxy_is_ready() {
        for (proxy, ready) in [
            (EggpoolProxyHealth::Ready, true),
            (EggpoolProxyHealth::Degraded, false),
            (EggpoolProxyHealth::Unready, false),
        ] {
            let snapshot = with_eggpool(eggpool_snapshot(
                Some(eggpool_health(proxy)),
                Some(eggpool_summary(EggpoolPeriod::Day)),
            ));
            let model = project(&snapshot, &Selection::default(), &connected(), 1_000);
            let eggpool = model.eggpool.expect("an eggpool plane");
            assert_eq!(
                eggpool.ready, ready,
                "{proxy:?} must not be reported as ready={ready}"
            );
            assert_eq!(eggpool.health, proxy.as_str());
        }
    }

    #[test]
    fn the_eggpool_plane_shows_exactly_the_four_summary_values() {
        let snapshot = with_eggpool(eggpool_snapshot(
            Some(eggpool_health(EggpoolProxyHealth::Ready)),
            Some(eggpool_summary(EggpoolPeriod::Day)),
        ));
        let model = project(&snapshot, &Selection::default(), &connected(), 1_000);
        let eggpool = model.eggpool.expect("an eggpool plane");
        let keys: Vec<&str> = eggpool.values.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, vec!["tokens", "cache read", "output", "ttft"]);
    }

    #[test]
    fn a_missing_summary_value_is_a_dash_and_not_a_zero() {
        let mut summary = eggpool_summary(EggpoolPeriod::Day);
        summary.cache_read_ratio = None;
        summary.avg_ttft_ms = None;
        let snapshot = with_eggpool(eggpool_snapshot(
            Some(eggpool_health(EggpoolProxyHealth::Ready)),
            Some(summary),
        ));
        let model = project(&snapshot, &Selection::default(), &connected(), 1_000);
        let eggpool = model.eggpool.expect("an eggpool plane");
        let cache = eggpool
            .values
            .iter()
            .find(|(key, _)| key == "cache read")
            .expect("a cache read value");
        assert_eq!(cache.1, MISSING);
        let ttft = eggpool
            .values
            .iter()
            .find(|(key, _)| key == "ttft")
            .expect("a ttft value");
        assert_eq!(ttft.1, MISSING);
    }

    #[test]
    fn the_eggpool_period_is_the_daemon_confirmed_period() {
        let mut snapshot = with_eggpool(eggpool_snapshot(
            Some(eggpool_health(EggpoolProxyHealth::Ready)),
            Some(eggpool_summary(EggpoolPeriod::Week)),
        ));
        snapshot.eggpool.as_mut().expect("an eggpool").period = EggpoolPeriod::Week;
        let model = project(&snapshot, &Selection::default(), &connected(), 1_000);
        let eggpool = model.eggpool.expect("an eggpool plane");
        assert_eq!(eggpool.period, EggpoolPeriod::Week);

        // The view follows the document: it never keeps a period of its own,
        // so a period the daemon never fetched cannot be shown.
        snapshot.eggpool.as_mut().expect("an eggpool").period = EggpoolPeriod::Month;
        let model = project(&snapshot, &Selection::default(), &connected(), 1_000);
        let eggpool = model.eggpool.expect("an eggpool plane");
        assert_eq!(eggpool.period, EggpoolPeriod::Month);
    }

    // ---------------------------------------------------------------------
    // Selection repair.
    // ---------------------------------------------------------------------

    #[test]
    fn a_repointed_host_keeps_its_selection_and_shows_its_current_data() {
        let selection = Selection {
            selected_id: Some("system-0".to_owned()),
            cron_job: None,
        };

        // The same stable id, pointed at another endpoint, with its own
        // numbers.
        let mut latest = normalized();
        latest.drives = Some(vec![NormalizedDrive {
            name: "/".to_owned(),
            used_bytes: 10_000_000_000,
            total_bytes: 100_000_000_000,
            available_bytes: Some(90_000_000_000),
        }]);
        let mut repointed = with_snapshot(0, latest);
        repointed.endpoint = Endpoint {
            id: "system-0".to_owned(),
            host: "host-9".to_owned(),
            port: 11310,
            name: None,
        };

        let snapshot = snapshot_of(vec![repointed]);
        let model = project(&snapshot, &selection, &connected(), 1_000);
        assert_eq!(
            model.selected_id.as_deref(),
            Some("system-0"),
            "a repointed host keeps the selection: it is the same host"
        );
        let card = model.selected.expect("the selected host");
        assert_eq!(card.endpoint, "host-9:11310");

        // The numbers are the *current* endpoint's, never the previous one's.
        let disk = card
            .metrics
            .iter()
            .find(|row| row.key == "disk")
            .expect("a disk row");
        assert!(disk.pct_text.contains("10.0%"), "{}", disk.pct_text);
        assert!(
            !disk.summary.contains("50.0%"),
            "the previous endpoint's numbers must not survive: {}",
            disk.summary
        );

        // And so do the detail rows.
        let (drives, _) = detail_rows(&snapshot, "system-0");
        assert_eq!(drives.len(), 1);
        assert!(drives[0].usage.contains("9.3 GiB"), "{}", drives[0].usage);
        assert!(
            !drives[0].usage.contains("46.6 GiB"),
            "the previous endpoint's drive must not survive: {}",
            drives[0].usage
        );
    }

    #[test]
    fn a_reordered_fleet_keeps_the_selection_on_the_same_host() {
        let first = with_snapshot(0, normalized());
        let mut second_latest = normalized();
        second_latest.system.name = "host-1".to_owned();
        let second = with_snapshot(1, second_latest);
        let selection = Selection {
            selected_id: Some("system-1".to_owned()),
            cron_job: None,
        };

        // The order changes; the selection must not.
        let model = project(
            &snapshot_of(vec![second.clone(), first.clone()]),
            &selection,
            &connected(),
            1_000,
        );
        assert_eq!(model.selected_id.as_deref(), Some("system-1"));
        let card = model.selected.expect("the selected host");
        assert_eq!(card.id, "system-1");
        assert_eq!(card.name, "host-1");
        assert!(
            !card.endpoint.starts_with("host-0"),
            "the selected card must be the host's own: {}",
            card.endpoint
        );
    }

    /// A pair of measurements shares its unit when both carry the same one.
    #[test]
    fn a_pair_writes_its_unit_once() {
        assert_eq!(format_pair("6.1 GiB", "15.5 GiB"), "6.1 / 15.5 GiB");
        assert_eq!(format_pair("4.2 KiB/s", "16.1 KiB/s"), "4.2 / 16.1 KiB/s");
    }

    /// Two measurements in different units keep both, because dropping either
    /// unit would make the number mean something other than what was measured.
    #[test]
    fn a_pair_in_two_units_keeps_both() {
        assert_eq!(format_pair("1.0 MiB", "2.0 GiB"), "1.0 MiB / 2.0 GiB");
        assert_eq!(format_pair("0 B", "2.0 GiB"), "0 B / 2.0 GiB");
    }

    /// A share is only a share when the platform reported a finite one.
    #[test]
    fn a_non_finite_share_is_no_share() {
        assert_eq!(usable_pct(f32::NAN), None);
        assert_eq!(usable_pct(f32::INFINITY), None);
        assert_eq!(usable_pct(f32::NEG_INFINITY), None);
        assert_eq!(usable_pct(0.0), Some(0.0));
        assert_eq!(usable_pct(42.5), Some(42.5));
    }

    /// An out-of-range reading is clamped: a bar that runs past its own track
    /// is not a measurement either.
    #[test]
    fn an_out_of_range_share_is_clamped() {
        assert_eq!(usable_pct(-1.0), Some(0.0));
        assert_eq!(usable_pct(150.0), Some(100.0));
    }
}
