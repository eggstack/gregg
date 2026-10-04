//! Plan 164: the serializable mirror of client state handed to frontends.
//!
//! The daemon owns fleet data. A frontend needs a *complete, self-contained*
//! copy it can render from, so this module defines wire-shaped mirrors of the
//! reducer's types. They are deliberately not the reducer's own types: those
//! hold `std::time::Instant` values, which mean nothing across a process
//! boundary.
//!
//! # How time is carried
//!
//! Every timestamp here is Unix epoch **milliseconds**, taken from the daemon's
//! wall clock, and the frontend converts to a relative age against its own
//! clock on the same machine. This preserves the existing "updated 2s ago"
//! semantics exactly. A monotonic `Instant` cannot be transported at all: its
//! epoch is process-local, so a deserialized one would either panic or be
//! silently meaningless.
//!
//! # Truthfulness
//!
//! Absent facts stay absent. An optional snapshot is `null`, not a
//! zero-filled record, so "never polled" and "polled and all zeros" remain
//! distinguishable — the same rule the metrics path already follows.

use serde::{Deserialize, Serialize};

use crate::cron::{CronCapability, CronFetchError, CronRecord, CronSystemState};
use crate::eggpool::EggpoolPeriod;
use crate::endpoint::Endpoint;
use crate::normalized::NormalizedSnapshot;
use crate::poller::OfflineReason;
use crate::state::{EggpoolWorkerState, Pane, Reachability, RefreshStatus, SystemViewMode};

/// Reachability of one system, as published to frontends.
///
/// This is the reducer's own enum rather than a parallel copy, so a frontend can
/// never disagree with the daemon about what "offline" means.
pub type ReachabilityDto = Reachability;

/// One monitored system's fleet data, as published to frontends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemSnapshotDto {
    /// Stable identifier. Selection and viewport are keyed on this.
    pub id: String,
    /// The endpoint being polled, carried whole rather than as a display
    /// string: a frontend lays out `host:port` from the real fields, so it
    /// cannot invent a spelling the daemon does not use.
    pub endpoint: Endpoint,
    /// Configured display name, if any.
    pub configured_name: Option<String>,
    /// Current reachability.
    pub reachability: ReachabilityDto,
    /// Most recent successful metrics snapshot, normalized across wire
    /// versions. `null` until the first success.
    pub latest: Option<NormalizedSnapshot>,
    /// Unix milliseconds of the most recent successful poll.
    pub last_success_at_unix_ms: Option<u64>,
    /// Unix milliseconds of the most recent attempt, successful or not.
    pub last_attempt_at_unix_ms: Option<u64>,
    /// Round-trip latency of the most recent successful poll.
    pub latency_ms: Option<u64>,
    /// Why the most recent poll failed, when it did.
    ///
    /// Carried as structured provenance, not as a display string, so the
    /// renderer chooses its own wording and can never invent a reason for a
    /// pending system.
    pub offline_reason: Option<OfflineReason>,
}

impl SystemSnapshotDto {
    /// A placeholder system used by protocol tests and by the daemon before its
    /// first poll.
    #[must_use]
    pub fn placeholder(index: usize, reachability: Reachability) -> Self {
        Self {
            id: format!("system-{index}"),
            endpoint: Endpoint {
                id: format!("system-{index}"),
                host: format!("host-{index}"),
                port: 11310,
                name: None,
            },
            configured_name: None,
            reachability,
            latest: None,
            last_success_at_unix_ms: None,
            last_attempt_at_unix_ms: None,
            latency_ms: None,
            offline_reason: None,
        }
    }
}

/// Poll progress, as published to frontends.
pub type RefreshStatusDto = RefreshStatus;

/// Local `EggPool` worker availability, as published to frontends.
///
/// This describes local machinery only. `EggPool`'s own service health is a
/// separate, independently fetched fact and is never inferred from this.
pub type EggpoolWorkerStateDto = EggpoolWorkerState;

/// The `EggPool` pane's state, as published to frontends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EggpoolSnapshotDto {
    /// The configured source the pane displays, carried whole so a frontend
    /// never has to re-parse a display string back into an address.
    pub endpoint: crate::config::EggpoolEntry,
    /// Currently selected rolling window.
    pub period: EggpoolPeriod,
    /// Latest desired request identity.
    pub request_generation: u64,
    /// Local worker availability.
    pub worker_state: EggpoolWorkerStateDto,
    /// Last successful summary for the selected period, if any.
    pub summary: Option<crate::eggpool::EggpoolSummary>,
    /// Unix milliseconds of the last successful request.
    pub last_success_at_unix_ms: Option<u64>,
    /// Unix milliseconds of the last attempt.
    pub last_attempt_at_unix_ms: Option<u64>,
    /// Most recent non-cancelled summary failure, as a structured
    /// classification rather than display text.
    pub last_error: Option<crate::eggpool::EggpoolFetchOutcome>,
    /// Latest valid service-health snapshot, independent of the summary period.
    pub health: Option<crate::eggpool::EggpoolHealthSnapshot>,
    /// Unix milliseconds of the last successful health read.
    pub last_health_success_at_unix_ms: Option<u64>,
    /// Unix milliseconds of the last health read attempt.
    pub last_health_attempt_at_unix_ms: Option<u64>,
    /// Most recent health failure, meaning any retained snapshot is not current.
    pub last_health_error: Option<crate::eggpool::EggpoolHealthFetchOutcome>,
}

/// One job's bounded history, as published to frontends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CronJobHistoryDto {
    /// Job name, matching a job in the published summary.
    pub job: String,
    /// Retained records, oldest first.
    pub records: Vec<CronRecord>,
}

/// One system's scheduler observability, as published to frontends.
///
/// Published as a vector parallel to `systems` rather than as a field on
/// [`SystemSnapshotDto`], so the scheduler plane stays visibly separate from the
/// normalized metrics payload. They are fetched on different cadences, sized
/// differently, and fail independently, and folding them together would make
/// "the metrics are fine but the cron route failed" hard to express.
///
/// `history` carries records **only for jobs a frontend currently has open**.
/// The summary needs no request to display, and the history body is the largest
/// document in the system, so retransmitting it on every five-second metrics
/// publication would put the cron plane on the hot path to draw five job rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemCronDto {
    /// Stable system id this belongs to.
    pub system_id: String,
    /// Whether the remote serves the scheduler routes.
    pub capability: CronCapability,
    /// Latest valid summary. `None` until one has been read.
    pub summary: Option<gregg_protocol::SchedulerSummaryV2>,
    /// Remote lifetime the retained records belong to.
    pub epoch: Option<gregg_protocol::SchedulerEpochV2>,
    /// `history_revision` of the most recently applied history document.
    pub history_revision: Option<u64>,
    /// Unix milliseconds of the last scheduler attempt.
    pub last_attempt_at_unix_ms: Option<u64>,
    /// Unix milliseconds of the last successful scheduler read.
    pub last_success_at_unix_ms: Option<u64>,
    /// Most recent scheduler-route failure, meaning retained data is not
    /// known to be current. Distinct from system reachability.
    pub last_error: Option<CronFetchError>,
    /// Bounded history for the jobs a frontend has open; empty otherwise.
    pub history: Vec<CronJobHistoryDto>,
}

impl SystemCronDto {
    /// Project one system's scheduler state for publication.
    ///
    /// `requested` is the `(job, depth)` set the reduced frontend intents asked
    /// for. Passing an empty set is normal and produces a document with a
    /// summary and no records, which is everything the at-a-glance rows need.
    #[must_use]
    pub fn from_state(
        system_id: &str,
        state: &CronSystemState,
        requested: &[(String, usize)],
    ) -> Self {
        let history = requested
            .iter()
            .map(|(job, depth)| CronJobHistoryDto {
                job: job.clone(),
                records: state.recent(job, *depth),
            })
            .filter(|entry| !entry.records.is_empty())
            .collect();
        Self {
            system_id: system_id.to_owned(),
            capability: state.capability,
            summary: state.summary.clone(),
            epoch: state.epoch,
            history_revision: state.history_revision,
            last_attempt_at_unix_ms: state.last_attempt_at_unix_ms,
            last_success_at_unix_ms: state.last_success_at_unix_ms,
            last_error: state.last_error.clone(),
            history,
        }
    }

    /// Records retained for one job in this document.
    #[must_use]
    pub fn job_records(&self, job: &str) -> &[CronRecord] {
        self.history
            .iter()
            .find(|entry| entry.job == job)
            .map_or(&[], |entry| entry.records.as_slice())
    }
}

/// Complete, self-contained client state for one frontend to render from.
///
/// A frontend can always draw from this alone; there is no incremental patch
/// protocol and no cross-snapshot bookkeeping to get wrong.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrontendSnapshot {
    /// Monotonic generation, incremented on every publication.
    ///
    /// A frontend compares generations to detect a *newer* document and drops
    /// anything older, which is what makes skipping safe.
    pub generation: u64,
    /// Unix milliseconds at which the daemon produced this document.
    pub produced_at_unix_ms: u64,
    /// Poll progress.
    pub refresh_status: RefreshStatusDto,
    /// Whether the daemon has accepted at least one poll generation.
    ///
    /// This is what a frontend needs in order to place its first selection.
    /// The daemon publishes a document the moment it binds, before any poll has
    /// completed, and every system in it is `pending` in configured order. A
    /// frontend that treated *that* document as "the fleet is established"
    /// would pin its selection to the first configured system and never move
    /// it when the first batch reveals that system is offline. This flag is
    /// therefore about the fleet's history, not about the frontend's.
    pub poll_initialized: bool,
    /// Fleet data, in display order.
    pub systems: Vec<SystemSnapshotDto>,
    /// Scheduler observability, in the same order as `systems`.
    ///
    /// One entry per system, always present so a frontend never has to infer
    /// "no entry" from a missing key. `history` inside each entry is populated
    /// only for jobs a frontend currently has open.
    pub cron: Vec<SystemCronDto>,
    /// `EggPool` pane state, when the config has an `EggPool` entry.
    pub eggpool: Option<EggpoolSnapshotDto>,
    /// Diagnostic from the most recent rejected config reload.
    pub config_reload_error: Option<String>,
}

impl FrontendSnapshot {
    /// A snapshot with no data yet, used before the first publication.
    #[must_use]
    pub fn empty(systems: Vec<SystemSnapshotDto>) -> Self {
        Self {
            generation: 0,
            produced_at_unix_ms: 0,
            refresh_status: RefreshStatusDto::Idle,
            poll_initialized: false,
            cron: systems
                .iter()
                .map(|system| SystemCronDto {
                    system_id: system.id.clone(),
                    capability: CronCapability::Unknown,
                    summary: None,
                    epoch: None,
                    history_revision: None,
                    last_attempt_at_unix_ms: None,
                    last_success_at_unix_ms: None,
                    last_error: None,
                    history: Vec::new(),
                })
                .collect(),
            systems,
            eggpool: None,
            config_reload_error: None,
        }
    }

    /// Look up one system by stable ID.
    #[must_use]
    pub fn system(&self, id: &str) -> Option<&SystemSnapshotDto> {
        self.systems.iter().find(|system| system.id == id)
    }

    /// Look up one system's scheduler state by stable ID.
    #[must_use]
    pub fn cron_for(&self, id: &str) -> Option<&SystemCronDto> {
        self.cron.iter().find(|entry| entry.system_id == id)
    }

    /// Whether a new frontend that has no state yet should paint this.
    ///
    /// An empty document is still valid state, so a first frontend must accept
    /// it; after that, only a strictly newer generation is worth applying.
    #[must_use]
    pub fn is_newer_than(&self, last_applied: u64) -> bool {
        last_applied == 0 || self.generation > last_applied
    }
}

/// Presentation-only state a frontend owns and the daemon must not touch.
///
/// Kept here so the split is explicit and reviewable: selection, viewport,
/// active pane, expansion, view mode, and the transient highlight are all
/// *per frontend*, because two TUI windows on the same daemon have genuinely
/// different selections.
///
/// The bool count is inherent to the model rather than a design smell: three
/// independent expansions (drives, network, cron) plus one transient highlight
/// is exactly the state the plan requires, and collapsing them into an enum
/// would make drive and network mutually exclusive, which they are not.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationState {
    /// Currently selected system, by stable ID.
    pub selected_id: Option<String>,
    /// First visible system in the viewport, by stable ID.
    pub viewport_top_id: Option<String>,
    /// Currently active top-level pane.
    pub active_pane: Pane,
    /// Current Systems presentation mode.
    pub system_view_mode: SystemViewMode,
    /// Whether the selected system's drives are expanded.
    pub drives_expanded: bool,
    /// Whether the selected system's network details are expanded.
    pub network_expanded: bool,
    /// Whether the selected system's cron details are expanded.
    ///
    /// Independent of drive and network expansion: all three can be open at
    /// once, and a system with many jobs stays inside a bounded vertical
    /// budget rather than growing without limit.
    pub cron_expanded: bool,
    /// Which cron job's history is expanded, by name.
    ///
    /// Exactly one job's multiline history is shown at a time, because a
    /// cron-expanded system may have dozens of jobs. Held by name rather than
    /// by index so moving between systems can repair the selection against a
    /// different job list.
    pub cron_job: Option<String>,
    /// Whether the logical selection is currently highlighted.
    pub selection_highlight_active: bool,
}

impl Default for PresentationState {
    fn default() -> Self {
        Self {
            selected_id: None,
            viewport_top_id: None,
            active_pane: Pane::Systems,
            system_view_mode: SystemViewMode::Normal,
            drives_expanded: false,
            network_expanded: false,
            cron_expanded: false,
            cron_job: None,
            selection_highlight_active: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clientd::protocol::{encode_frame, FrontendFrame};

    /// Serializing and deserializing a value must return exactly the same
    /// value. A snapshot that loses a field across the process boundary would
    /// silently degrade what the TUI can render.
    fn assert_json_roundtrip<T>(value: &T)
    where
        T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).expect("serializes");
        let back: T = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(*value, back, "round trip changed the value: {json}");
    }

    #[test]
    fn a_snapshot_round_trips_through_json() {
        let mut systems = vec![SystemSnapshotDto::placeholder(0, Reachability::Online)];
        systems[0].last_success_at_unix_ms = Some(1_700_000_000_000);
        systems[0].latency_ms = Some(42);
        systems[0].offline_reason = Some(OfflineReason::with_detail(
            crate::poller::OfflineKind::Http,
            "HTTP 503",
        ));
        let snapshot = FrontendSnapshot::empty(systems);
        assert_json_roundtrip(&snapshot);
    }

    #[test]
    fn a_never_polled_system_stays_distinguishable_from_a_zeroed_one() {
        let pending = SystemSnapshotDto::placeholder(0, Reachability::Pending);
        assert_eq!(pending.reachability, Reachability::Pending);
        assert!(pending.latest.is_none());
        assert!(pending.last_success_at_unix_ms.is_none());
        assert!(pending.offline_reason.is_none());
    }

    #[test]
    fn an_offline_reason_keeps_its_structured_payload() {
        let system = SystemSnapshotDto {
            offline_reason: Some(OfflineReason::with_detail(
                crate::poller::OfflineKind::Http,
                "HTTP 503",
            )),
            ..SystemSnapshotDto::placeholder(0, Reachability::Offline)
        };
        let json = serde_json::to_string(&system).expect("serializes");
        assert!(json.contains("HTTP 503"), "{json}");
        let back: SystemSnapshotDto = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back.offline_reason, system.offline_reason);
    }

    #[test]
    fn generations_order_snapshots_and_a_first_snapshot_always_applies() {
        let mut snapshot = FrontendSnapshot::empty(Vec::new());
        assert!(
            snapshot.is_newer_than(0),
            "a first frontend must accept an empty document"
        );
        snapshot.generation = 5;
        assert!(snapshot.is_newer_than(4));
        assert!(
            !snapshot.is_newer_than(5),
            "a replayed snapshot is not newer"
        );
        assert!(
            !snapshot.is_newer_than(6),
            "a skipped snapshot is not newer"
        );
    }

    #[test]
    fn a_snapshot_is_addressable_by_stable_id() {
        let systems = vec![
            SystemSnapshotDto::placeholder(0, Reachability::Pending),
            SystemSnapshotDto::placeholder(1, Reachability::Pending),
        ];
        let snapshot = FrontendSnapshot::empty(systems);
        assert!(snapshot.system("system-1").is_some());
        assert!(snapshot.system("nope").is_none());
    }

    #[test]
    fn presentation_state_starts_unhighlighted() {
        // Plan 087: the transient highlight must never be on at startup.
        assert!(!PresentationState::default().selection_highlight_active);
        assert_eq!(PresentationState::default().active_pane, Pane::Systems);
    }

    #[test]
    fn the_frame_cap_has_large_headroom_for_a_full_fleet() {
        // A 64-system snapshot with placeholders must stay far below the cap, so
        // the cap is not the thing that breaks first as fleets grow.
        let systems: Vec<SystemSnapshotDto> = (0..64)
            .map(|index| SystemSnapshotDto::placeholder(index, Reachability::Online))
            .collect();
        let encoded = encode_frame(&FrontendFrame::Snapshot(Box::new(FrontendSnapshot::empty(
            systems,
        ))))
        .expect("encodes");
        assert!(encoded.len() < 512 * 1024, "{} bytes", encoded.len());
    }
}
