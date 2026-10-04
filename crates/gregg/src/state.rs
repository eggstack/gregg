//! Application state model for the polling engine and TUI.
//!
//! Plan 164 splits this module along the process boundary, and the split is a
//! *type* boundary, not a naming convention:
//!
//! - [`FleetState`] is everything derived from the network. It is owned by the
//!   client daemon, mutated only by poll batches, `EggPool` results, and config
//!   reloads, and serialized to frontends by [`FleetState::to_dto`].
//! - [`AppState`] is the frontend's render model. Its fleet fields are a
//!   *published copy* written by exactly one function,
//!   [`AppState::adopt_snapshot`], and its presentation fields are written only
//!   by [`AppState::apply_action_changed`].
//!
//! There is deliberately no other writer. A TUI that could still apply a
//! [`PollBatch`] or mint an `EggPool` request generation would own remote
//! polling state, and two such TUIs would each poll the fleet — which is the
//! duplication this architecture exists to remove.

use std::ops::Range;
use std::time::{Duration, Instant};

use crate::action::Action;
use crate::clientd::snapshot::{
    CronJobHistoryDto, EggpoolSnapshotDto, FrontendSnapshot, SystemCronDto, SystemSnapshotDto,
};
use crate::config::Config;
use crate::cron::CronCapability;
use crate::eggpool::{
    EggpoolDesiredState, EggpoolFetchOutcome, EggpoolHealthFetchOutcome, EggpoolHealthSnapshot,
    EggpoolPeriod, EggpoolResult, EggpoolSummary,
};
use crate::endpoint::Endpoint;
use crate::normalized::NormalizedSnapshot;
use crate::poller::{OfflineReason, PollBatch, PollOutcome};
use serde::{Deserialize, Serialize};

/// A stable system identifier (UUID v4 string).
pub type SystemId = String;

/// Reachability state for a single system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reachability {
    /// No poll result received yet.
    Pending,
    /// The most recent poll succeeded.
    Online,
    /// The most recent poll failed.
    Offline,
}

/// Whether the poll scheduler is currently idle or running a generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefreshStatus {
    /// No poll in progress.
    Idle,
    /// A poll generation is in flight.
    Polling {
        /// The generation number of the in-flight poll.
        generation: u64,
    },
}

/// The TUI presentation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemViewMode {
    /// The detailed, one-block-per-system view.
    Normal,
    /// The one-row-per-system fleet view.
    Condensed,
}

/// The two fixed top-level panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    /// The configured system fleet.
    Systems,
    /// The optional `EggPool` summary.
    Eggpool,
}

/// Whether Gregg's local `EggPool` worker is idle, working, or gone.
///
/// Plan 151: this describes only local machinery. `EggPool`'s own proxy
/// and provider service health is a separate fact (Plan 152) and is never
/// inferred from these variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EggpoolWorkerState {
    /// No request is currently in flight.
    Idle,
    /// Gregg published a current desired request and awaits its result.
    Refreshing,
    /// Gregg's local worker or control path is gone; nothing can dispatch.
    WorkerUnavailable,
}

/// Reducer-owned transient state for the optional `EggPool` pane.
#[derive(Debug, Clone)]
pub struct EggpoolState {
    /// The configured source displayed by the pane.
    pub endpoint: crate::config::EggpoolEntry,
    /// Currently selected rolling window.
    pub period: EggpoolPeriod,
    /// Latest desired request identity.
    pub request_generation: u64,
    /// Current local worker state.
    pub worker_state: EggpoolWorkerState,
    /// Last successful summary for the selected period.
    pub summary: Option<EggpoolSummary>,
    /// Completion time of the last successful request.
    pub last_success_at: Option<Instant>,
    /// Completion time of the last request attempt.
    pub last_attempt_at: Option<Instant>,
    /// Most recent non-cancelled summary failure.
    pub last_error: Option<EggpoolFetchOutcome>,
    /// Latest valid `EggPool` service-health snapshot, independent of the
    /// selected summary period.
    pub health: Option<EggpoolHealthSnapshot>,
    /// Completion time of the last successful health read.
    pub last_health_success_at: Option<Instant>,
    /// Completion time of the last health read attempt.
    pub last_health_attempt_at: Option<Instant>,
    /// Most recent health failure. Its presence means any retained
    /// snapshot is not current.
    pub last_health_error: Option<EggpoolHealthFetchOutcome>,
}

/// Per-system mutable state.
#[derive(Debug, Clone)]
pub struct SystemState {
    /// Stable unique identifier matching the config entry.
    pub id: SystemId,
    /// The endpoint used for polling.
    pub endpoint: Endpoint,
    /// Configured display name, if any.
    pub configured_name: Option<String>,
    /// Current reachability.
    pub reachability: Reachability,
    /// Most recent successful snapshot (normalized from v1 or v2).
    pub latest: Option<NormalizedSnapshot>,
    /// When the most recent successful poll completed.
    pub last_success_at: Option<Instant>,
    /// When the most recent poll attempt completed (success or failure).
    pub last_attempt_at: Option<Instant>,
    /// Round-trip latency of the most recent successful poll.
    pub latency: Option<Duration>,
    /// Normalized provenance of the most recent failed poll, if any.
    /// Stored from the accepted poll result and cleared by the next
    /// accepted success; the renderer consumes only this, never transport errors.
    pub offline_reason: Option<OfflineReason>,
}

/// The display order: online systems first (in configured order), then
/// offline/pending systems (in configured order).
///
/// A free function so the daemon's [`FleetState`] and the frontend's
/// [`AppState`] cannot disagree about what "display order" means.
#[must_use]
pub fn display_order_of(systems: &[SystemState]) -> Vec<usize> {
    // One allocation: online indices are appended first, then a second
    // pass appends the offline/pending indices, preserving configured
    // order within each group.
    let mut order = Vec::with_capacity(systems.len());
    for (index, system) in systems.iter().enumerate() {
        if matches!(system.reachability, Reachability::Online) {
            order.push(index);
        }
    }
    for (index, system) in systems.iter().enumerate() {
        if matches!(
            system.reachability,
            Reachability::Offline | Reachability::Pending
        ) {
            order.push(index);
        }
    }
    order
}

/// Everything in the client that is derived from the network.
///
/// This is the client daemon's own state. It is never serialized to a
/// frontend directly: [`FleetState::to_dto`] projects it into the local IPC
/// DTO so that process-local `Instant` values never have to cross the
/// boundary.
#[derive(Debug)]
pub struct FleetState {
    /// Ordered list of all monitored systems.
    pub systems: Vec<SystemState>,
    /// Last generation whose results were applied.
    pub last_applied_generation: u64,
    /// Current refresh status.
    pub refresh_status: RefreshStatus,
    /// Diagnostic from the most recent rejected config reload.
    pub config_reload_error: Option<String>,
    /// How many cron records a pane should display, from the configuration.
    ///
    /// Held here because the daemon owns the configuration and a frontend
    /// never opens the file.
    pub cron_display_history: usize,
    /// Optional `EggPool` pane state.
    pub eggpool: Option<EggpoolState>,
    /// Plan 166: remote scheduler observability, including the memory-only
    /// history cache.
    ///
    /// Owned here rather than in the polling task because it is fleet data the
    /// document projects, and because a single owner means a single place where
    /// the dedup epoch, the applied revision, and the bounds live. The task
    /// that performs the fetches hands finished observations over; it never
    /// touches the cache directly.
    pub cron: crate::cron::CronCache,
}

impl FleetState {
    /// Create fleet state from a configuration. Every system starts
    /// [`Reachability::Pending`].
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        Self {
            systems: config.systems.iter().map(system_from_entry).collect(),
            last_applied_generation: 0,
            refresh_status: RefreshStatus::Idle,
            config_reload_error: None,
            cron_display_history: config.cron.display_history(),
            eggpool: config.eggpool.clone().map(|endpoint| EggpoolState {
                endpoint,
                period: EggpoolPeriod::Hour,
                request_generation: 0,
                worker_state: EggpoolWorkerState::Idle,
                summary: None,
                last_success_at: None,
                last_attempt_at: None,
                last_error: None,
                health: None,
                last_health_success_at: None,
                last_health_attempt_at: None,
                last_health_error: None,
            }),
            cron: crate::cron::CronCache::new(
                config.cron.cache_history(),
                crate::cron::MAX_TOTAL_CRON_RECORDS,
            ),
        }
    }

    /// Return the display order of the current fleet.
    #[must_use]
    pub fn display_order(&self) -> Vec<usize> {
        display_order_of(&self.systems)
    }

    /// Reconcile the configured system endpoint list while retaining safe
    /// state for unchanged stable IDs.
    ///
    /// Plan 140: retained entries are moved out of the old-ID map instead
    /// of deep-cloned, avoiding `NormalizedSnapshot` copies during reload.
    ///
    /// Selection and viewport are *not* touched here: they belong to a
    /// frontend, and a frontend repairs them against the reconciled fleet in
    /// [`AppState::adopt_snapshot`].
    pub fn reconcile_systems(&mut self, config: &Config) {
        let old_systems = std::mem::take(&mut self.systems);
        let mut old_by_id = old_systems
            .into_iter()
            .map(|system| (system.id.clone(), system))
            .collect::<std::collections::HashMap<_, _>>();

        self.systems = config
            .systems
            .iter()
            .map(|entry| {
                let Some(mut old) = old_by_id.remove(&entry.id) else {
                    return system_from_entry(entry);
                };

                let endpoint = entry.to_endpoint();
                if equivalent_endpoint_host(&old.endpoint.host, &endpoint.host)
                    && old.endpoint.port == endpoint.port
                {
                    old.endpoint = endpoint;
                    old.configured_name.clone_from(&entry.name);
                    old
                } else {
                    system_from_entry(entry)
                }
            })
            .collect();
    }

    /// Record a rejected config reload for publication to frontends.
    pub fn set_config_reload_error(&mut self, error: String) {
        self.config_reload_error = Some(error);
    }

    /// Clear the rejected config reload diagnostic after a success.
    pub fn clear_config_reload_error(&mut self) {
        self.config_reload_error = None;
    }

    /// Apply a borrowed poll batch, reporting whether any *fleet* field
    /// changed.
    ///
    /// Selection and viewport are deliberately excluded: they are per
    /// frontend, and the daemon has no business choosing them. Returns `false`
    /// for a rejected generation and for an accepted batch in which every
    /// result was ignored or left fleet data identical.
    pub fn apply_batch(&mut self, batch: &PollBatch) {
        let _ = self.apply_batch_changed(batch);
    }

    /// Borrowed batch application reporting fleet-visible change.
    pub fn apply_batch_changed(&mut self, batch: &PollBatch) -> bool {
        if !self.accept_batch_generation(batch.generation) {
            return false;
        }

        let mut visible_changed = false;
        let mut id_map: Option<std::collections::HashMap<String, usize>> = None;

        for (result_index, result) in batch.results.iter().enumerate() {
            let mut missed = false;
            let Some(system_index) = self.resolve_result_index_with_map(
                result_index,
                &result.system_id,
                id_map.as_ref(),
            ) else {
                continue;
            };
            if id_map.is_none()
                && self
                    .systems
                    .get(result_index)
                    .is_none_or(|system| system.id != result.system_id)
            {
                missed = true;
            }
            if missed {
                id_map = Some(self.build_result_index_map());
            }
            // Compat path only: build normalized first, then compare by
            // reference so `latest` is never cloned per result.
            let system = &mut self.systems[system_index];
            // A stable ID may be retained while its configured target
            // changes. Results from the superseded target are stale even
            // when their scheduler generation is otherwise current.
            if !equivalent_endpoint_host(&system.endpoint.host, &result.endpoint.host)
                || system.endpoint.port != result.endpoint.port
            {
                continue;
            }
            match &result.outcome {
                PollOutcome::Online(snapshot) => {
                    let normalized = NormalizedSnapshot::from_v1(snapshot);
                    if system.reachability != Reachability::Online
                        || system.latest.as_ref() != Some(&normalized)
                        || system.offline_reason.is_some()
                    {
                        visible_changed = true;
                    }
                    system.reachability = Reachability::Online;
                    system.latest = Some(normalized);
                    system.last_success_at = Some(batch.completed_at);
                    system.last_attempt_at = Some(batch.completed_at);
                    system.latency = Some(result.latency);
                    system.offline_reason = None;
                }
                PollOutcome::OnlineV2(snapshot) => {
                    let normalized = NormalizedSnapshot::from_v2_payload(snapshot);
                    if system.reachability != Reachability::Online
                        || system.latest.as_ref() != Some(&normalized)
                        || system.offline_reason.is_some()
                    {
                        visible_changed = true;
                    }
                    system.reachability = Reachability::Online;
                    system.latest = Some(normalized);
                    system.last_success_at = Some(batch.completed_at);
                    system.last_attempt_at = Some(batch.completed_at);
                    system.latency = Some(result.latency);
                    system.offline_reason = None;
                }
                _ => {
                    let reason = result.outcome.offline_reason();
                    if system.reachability != Reachability::Offline
                        || system.offline_reason != reason
                    {
                        visible_changed = true;
                    }
                    system.reachability = Reachability::Offline;
                    system.last_attempt_at = Some(batch.completed_at);
                    system.offline_reason = reason;
                }
            }
        }

        self.last_applied_generation = batch.generation;
        visible_changed
    }

    /// Apply an owned poll batch, moving successful payload data into the
    /// normalized state instead of cloning strings and collections.
    pub fn apply_batch_owned(&mut self, batch: PollBatch) {
        let _ = self.apply_batch_owned_changed(batch);
    }

    /// Owned batch application reporting fleet-visible change.
    pub fn apply_batch_owned_changed(&mut self, batch: PollBatch) -> bool {
        if !self.accept_batch_generation(batch.generation) {
            return false;
        }

        let mut visible_changed = false;
        let mut id_map: Option<std::collections::HashMap<String, usize>> = None;
        let PollBatch {
            generation,
            completed_at,
            results,
            ..
        } = batch;
        for (result_index, result) in results.into_iter().enumerate() {
            if id_map.is_none()
                && self
                    .systems
                    .get(result_index)
                    .is_none_or(|system| system.id != result.system_id)
            {
                id_map = Some(self.build_result_index_map());
            }
            let Some(system_index) = self.resolve_result_index_with_map(
                result_index,
                &result.system_id,
                id_map.as_ref(),
            ) else {
                continue;
            };
            let (before_reachability, before_reason) = {
                let system = &self.systems[system_index];
                (system.reachability, system.offline_reason.clone())
            };
            let system = &mut self.systems[system_index];
            if !equivalent_endpoint_host(&system.endpoint.host, &result.endpoint.host)
                || system.endpoint.port != result.endpoint.port
            {
                continue;
            }
            match result.outcome {
                PollOutcome::Online(snapshot) => {
                    let normalized = NormalizedSnapshot::from_v1_owned(*snapshot);
                    let latest_same = system.latest.as_ref() == Some(&normalized);
                    if before_reachability != Reachability::Online
                        || !latest_same
                        || before_reason.is_some()
                    {
                        visible_changed = true;
                    }
                    system.reachability = Reachability::Online;
                    system.latest = Some(normalized);
                    system.last_success_at = Some(completed_at);
                    system.last_attempt_at = Some(completed_at);
                    system.latency = Some(result.latency);
                    system.offline_reason = None;
                }
                PollOutcome::OnlineV2(snapshot) => {
                    let normalized = NormalizedSnapshot::from_v2_payload_owned(*snapshot);
                    let latest_same = system.latest.as_ref() == Some(&normalized);
                    if before_reachability != Reachability::Online
                        || !latest_same
                        || before_reason.is_some()
                    {
                        visible_changed = true;
                    }
                    system.reachability = Reachability::Online;
                    system.latest = Some(normalized);
                    system.last_success_at = Some(completed_at);
                    system.last_attempt_at = Some(completed_at);
                    system.latency = Some(result.latency);
                    system.offline_reason = None;
                }
                outcome => {
                    let reason = outcome.offline_reason();
                    if before_reachability != Reachability::Offline || before_reason != reason {
                        visible_changed = true;
                    }
                    system.reachability = Reachability::Offline;
                    system.last_attempt_at = Some(completed_at);
                    system.offline_reason = reason;
                }
            }
        }

        self.last_applied_generation = generation;
        visible_changed
    }

    fn accept_batch_generation(&self, generation: u64) -> bool {
        let wrapped_generation = self.last_applied_generation == u64::MAX && generation == 1;
        if generation <= self.last_applied_generation && !wrapped_generation {
            debug_assert!(generation <= self.last_applied_generation);
            return false;
        }
        true
    }

    /// Positional match with stable-ID fallback, using a one-per-batch
    /// index after the first miss so a reordered batch stays O(n) instead
    /// of O(n²). The map owns its keys so it can live across the mutable
    /// per-result updates; built only on the first miss (rare), keeping
    /// the common ordered path allocation-free.
    fn resolve_result_index_with_map(
        &self,
        result_index: usize,
        system_id: &str,
        map: Option<&std::collections::HashMap<String, usize>>,
    ) -> Option<usize> {
        if self
            .systems
            .get(result_index)
            .is_some_and(|system| system.id == system_id)
        {
            return Some(result_index);
        }
        if let Some(map) = map {
            return map.get(system_id).copied();
        }
        self.systems
            .iter()
            .position(|system| system.id == system_id)
    }

    /// Build the one-per-batch stable-ID index after the first miss.
    fn build_result_index_map(&self) -> std::collections::HashMap<String, usize> {
        let mut map = std::collections::HashMap::with_capacity(self.systems.len());
        for (index, system) in self.systems.iter().enumerate() {
            map.insert(system.id.clone(), index);
        }
        map
    }

    /// Apply one `EggPool` result if it belongs to the current request and period.
    pub fn apply_eggpool_result(&mut self, result: &EggpoolResult) {
        let _ = self.apply_eggpool_result_changed(result);
    }

    /// `EggPool` result application reporting fleet-visible change.
    ///
    /// The summary and health planes are applied independently, so a
    /// partial result is the normal case: a failed health refresh never
    /// erases a successful summary, and a failed summary never erases valid
    /// service health. Returns `false` for stale generations/periods and for
    /// cancelled outcomes that leave visible state untouched (already idle).
    pub fn apply_eggpool_result_changed(&mut self, result: &EggpoolResult) -> bool {
        let Some(eggpool) = self.eggpool.as_mut() else {
            return false;
        };
        if result.generation != eggpool.request_generation || result.period != eggpool.period {
            return false;
        }
        if matches!(result.summary, EggpoolFetchOutcome::Cancelled) {
            // A cancelled worker leaves `Refreshing` forever unless
            // resolved. Return to `Idle` without touching `last_attempt_at`,
            // `last_error`, or any health state.
            if eggpool.worker_state != EggpoolWorkerState::Idle {
                eggpool.worker_state = EggpoolWorkerState::Idle;
                return true;
            }
            return false;
        }
        let mut changed = false;
        if eggpool.worker_state != EggpoolWorkerState::Idle {
            eggpool.worker_state = EggpoolWorkerState::Idle;
            changed = true;
        }
        // Summary plane: the selected period only.
        if eggpool.last_attempt_at != Some(result.completed_at) {
            changed = true;
        }
        eggpool.last_attempt_at = Some(result.completed_at);
        match &result.summary {
            EggpoolFetchOutcome::Online(summary) => {
                if eggpool.summary.as_ref() != Some(summary) {
                    changed = true;
                }
                eggpool.summary = Some(summary.clone());
                eggpool.last_success_at = Some(result.completed_at);
                if eggpool.last_error.is_some() {
                    changed = true;
                }
                eggpool.last_error = None;
            }
            error => {
                if eggpool.last_error.as_ref() != Some(error) {
                    changed = true;
                }
                eggpool.last_error = Some(error.clone());
            }
        }
        // Health plane: current service health, with no period.
        if eggpool.last_health_attempt_at != Some(result.completed_at) {
            changed = true;
        }
        eggpool.last_health_attempt_at = Some(result.completed_at);
        match &result.health {
            EggpoolHealthFetchOutcome::Online(snapshot) => {
                if eggpool.health.as_ref() != Some(snapshot) {
                    changed = true;
                }
                eggpool.health = Some(snapshot.clone());
                eggpool.last_health_success_at = Some(result.completed_at);
                if eggpool.last_health_error.is_some() {
                    changed = true;
                }
                eggpool.last_health_error = None;
            }
            error => {
                if eggpool.last_health_error.as_ref() != Some(error) {
                    changed = true;
                }
                // A previous snapshot stays visible, but the renderer marks
                // it as no longer current rather than claiming freshness.
                eggpool.last_health_error = Some(error.clone());
            }
        }
        changed
    }

    /// Adopt a reconfigured `EggPool` endpoint, preserving observed state.
    ///
    /// A reload that changes only the `EggPool` address must not discard the
    /// summary already on screen; the next fetch will replace it. Losing the
    /// entry entirely does discard state, because the pane no longer has a
    /// source to describe.
    pub fn adopt_eggpool_endpoint(&mut self, entry: crate::config::EggpoolEntry) {
        match self.eggpool.as_mut() {
            Some(state) => state.endpoint = entry,
            None => {
                self.eggpool = Some(EggpoolState {
                    endpoint: entry,
                    period: EggpoolPeriod::Hour,
                    request_generation: 0,
                    worker_state: EggpoolWorkerState::Idle,
                    summary: None,
                    last_success_at: None,
                    last_attempt_at: None,
                    last_error: None,
                    health: None,
                    last_health_success_at: None,
                    last_health_attempt_at: None,
                    last_health_error: None,
                });
            }
        }
    }

    /// Drop the `EggPool` entry when a reload removes it.
    pub fn clear_eggpool(&mut self) {
        self.eggpool = None;
    }

    /// Switch the `EggPool` summary period, invalidating the old window.
    ///
    /// A period applies to the summary plane only; service health has no
    /// period and stays visible, so switching windows never blanks the health
    /// block.
    pub fn set_eggpool_period(&mut self, period: EggpoolPeriod) -> bool {
        let Some(state) = self.eggpool.as_mut() else {
            return false;
        };
        if state.period == period {
            return false;
        }
        state.period = period;
        state.summary = None;
        state.last_error = None;
        true
    }

    /// Mark an `EggPool` activation or manual refresh as a new request.
    pub fn begin_eggpool_request(&mut self) -> Option<(EggpoolPeriod, u64)> {
        let eggpool = self.eggpool.as_mut()?;
        eggpool.request_generation = eggpool.request_generation.saturating_add(1);
        eggpool.worker_state = EggpoolWorkerState::Refreshing;
        Some((eggpool.period, eggpool.request_generation))
    }

    /// Mark the local worker as unavailable without exposing a channel error.
    pub fn mark_eggpool_worker_unavailable(&mut self) {
        if let Some(eggpool) = self.eggpool.as_mut() {
            eggpool.worker_state = EggpoolWorkerState::WorkerUnavailable;
        }
    }

    /// Return the current `EggPool` request identity without changing state.
    #[must_use]
    pub fn eggpool_request(&self) -> Option<(EggpoolPeriod, u64)> {
        self.eggpool
            .as_ref()
            .map(|eggpool| (eggpool.period, eggpool.request_generation))
    }

    /// The converged `EggPool` worker state the daemon should hold.
    ///
    /// `active` follows whether any attached frontend has the pane open, and
    /// `period` is the shortest window any of them asked for. Both are
    /// order-independent reductions over the subscriber set, so two frontends
    /// cannot race the worker into two different activations.
    #[must_use]
    pub fn eggpool_desired_state(&self, intents: &EggpoolIntents) -> Option<EggpoolDesiredState> {
        let eggpool = self.eggpool.as_ref()?;
        Some(EggpoolDesiredState {
            active: intents.any_active(),
            period: intents.converged_period(eggpool.period),
            generation: eggpool.request_generation,
        })
    }

    /// Project the fleet into the local IPC document at wall-clock `now`.
    ///
    /// `Instant` values are converted to Unix milliseconds here and only
    /// here, so no process-local clock ever reaches the wire.
    #[must_use]
    pub fn to_dto(&self, now: Instant, now_unix_ms: u64, generation: u64) -> FrontendSnapshot {
        self.to_dto_for(now, now_unix_ms, generation, &CronIntents::default())
    }

    /// Project the fleet, publishing cron history only for the jobs the
    /// reduced frontend intents asked for.
    ///
    /// The summary needs no request to display, so it always rides along. The
    /// history records are the largest thing in the document, so they are
    /// included per open frontend request and omitted otherwise.
    #[must_use]
    pub fn to_dto_for(
        &self,
        now: Instant,
        now_unix_ms: u64,
        generation: u64,
        intents: &CronIntents,
    ) -> FrontendSnapshot {
        FrontendSnapshot {
            generation,
            produced_at_unix_ms: now_unix_ms,
            refresh_status: self.refresh_status.clone(),
            poll_initialized: self.last_applied_generation != 0,
            systems: self
                .systems
                .iter()
                .map(|system| system_to_dto(system, now, now_unix_ms))
                .collect(),
            eggpool: self.eggpool.as_ref().map(|eggpool| EggpoolSnapshotDto {
                endpoint: eggpool.endpoint.clone(),
                period: eggpool.period,
                request_generation: eggpool.request_generation,
                worker_state: eggpool.worker_state,
                summary: eggpool.summary.clone(),
                last_success_at_unix_ms: elapsed_to_unix_ms(
                    eggpool.last_success_at,
                    now,
                    now_unix_ms,
                ),
                last_attempt_at_unix_ms: elapsed_to_unix_ms(
                    eggpool.last_attempt_at,
                    now,
                    now_unix_ms,
                ),
                last_error: eggpool.last_error.clone(),
                health: eggpool.health.clone(),
                last_health_success_at_unix_ms: elapsed_to_unix_ms(
                    eggpool.last_health_success_at,
                    now,
                    now_unix_ms,
                ),
                last_health_attempt_at_unix_ms: elapsed_to_unix_ms(
                    eggpool.last_health_attempt_at,
                    now,
                    now_unix_ms,
                ),
                last_health_error: eggpool.last_health_error.clone(),
            }),
            cron_display_history: self.cron_display_history,
            cron: self
                .systems
                .iter()
                .map(|system| {
                    let state = self.cron.system(&system.id);
                    SystemCronDto {
                        system_id: system.id.clone(),
                        capability: state.map_or(CronCapability::Unknown, |s| s.capability),
                        summary: state.and_then(|s| s.summary.clone()),
                        epoch: state.and_then(|s| s.epoch),
                        history_revision: state.and_then(|s| s.history_revision),
                        last_attempt_at_unix_ms: state.and_then(|s| s.last_attempt_at_unix_ms),
                        last_success_at_unix_ms: state.and_then(|s| s.last_success_at_unix_ms),
                        last_error: state.and_then(|s| s.last_error.clone()),
                        history: state
                            .map(|s| {
                                intents
                                    .requests_for(&system.id)
                                    .into_iter()
                                    .map(|(job, depth)| CronJobHistoryDto {
                                        job: job.clone(),
                                        records: s.recent(&job, depth),
                                    })
                                    .filter(|entry| !entry.records.is_empty())
                                    .collect()
                            })
                            .unwrap_or_default(),
                    }
                })
                .collect(),
            config_reload_error: self.config_reload_error.clone(),
        }
    }
}

/// One frontend's current `EggPool` intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EggpoolIntent {
    /// Whether this frontend has the `EggPool` pane open.
    pub active: bool,
    /// The rolling window this frontend is displaying.
    pub period: EggpoolPeriod,
}

/// Make a remote string inert without bounding it.
///
/// Applied at the single point where published fleet data enters a frontend,
/// so every renderer path — normal, condensed, diagnostics, cron — is covered
/// by one decision rather than by remembering to sanitize at each cell site.
///
/// The line bound is deliberately infinite here. This step is about *escaping*;
/// truncating to a viewport is a render-time concern, and a stored record must
/// not be shortened by a display decision. The cron renderer applies its own
/// viewport bound later, and running the escaped text through the sanitizer a
/// second time is idempotent, so the two steps compose.
fn clean(text: &str) -> String {
    crate::sanitize::sanitize(text, usize::MAX).text
}

/// Strip terminal controls from every remote string in a metrics snapshot.
///
/// Hostnames, drive names, mount points, filesystem names, and interface names
/// all come from the machine being watched. A host whose hostname is
/// `ESC [ 2 J evil` would otherwise clear the operator's screen, and one whose
/// interface is named with an OSC hyperlink would plant a clickable URL in the
/// middle of a monitoring display. Numeric telemetry is untouched.
fn clean_snapshot(snapshot: &mut NormalizedSnapshot) {
    let identity = &mut snapshot.system;
    identity.name = clean(&identity.name);
    identity.hostname = clean(&identity.hostname);
    identity.os_name = clean(&identity.os_name);
    identity.os_version = clean(&identity.os_version);
    identity.kernel_name = clean(&identity.kernel_name);
    identity.kernel_release = clean(&identity.kernel_release);
    identity.architecture = clean(&identity.architecture);

    if let Some(drives) = snapshot.drives.as_mut() {
        for drive in drives {
            drive.name = clean(&drive.name);
        }
    }
    if let Some(disk_io) = snapshot.disk_io.as_mut() {
        for device in &mut disk_io.devices {
            device.id = clean(&device.id);
            device.name = clean(&device.name);
            device.drive_name = device.drive_name.as_deref().map(clean);
        }
    }
    if let Some(network) = snapshot.network.as_mut() {
        for interface in &mut network.interfaces {
            interface.id = clean(&interface.id);
            interface.name = clean(&interface.name);
        }
    }
}

/// Strip terminal controls from one published scheduler document.
///
/// Job names and schedule strings come from a remote configuration, and record
/// output is by definition arbitrary program output, so all of it is treated as
/// hostile. The record text keeps its line structure; only the viewport bound
/// shortens it, and that happens in the renderer.
fn clean_cron(entry: &mut SystemCronDto) {
    if let Some(summary) = entry.summary.as_mut() {
        for job in &mut summary.jobs {
            job.name = clean(&job.name);
            job.schedule = clean(&job.schedule);
        }
    }
    for job in &mut entry.history {
        job.job = clean(&job.job);
        for record in &mut job.records {
            record.record.stdout.text = clean(&record.record.stdout.text);
            record.record.stderr.text = clean(&record.record.stderr.text);
        }
    }
}

/// Whether a published scheduler document differs in a render-visible way.
///
/// Timestamps are excluded, exactly as they are for systems: a row that already
/// says the same thing at a slightly different age does not need a frame, and
/// treating it as changed would redraw on every poll.
fn cron_visibly_differ(before: &[SystemCronDto], after: &[SystemCronDto]) -> bool {
    if before.len() != after.len() {
        return true;
    }
    before.iter().zip(after.iter()).any(|(a, b)| {
        a.system_id != b.system_id
            || a.capability != b.capability
            || a.summary != b.summary
            || a.epoch != b.epoch
            || a.history_revision != b.history_revision
            || a.last_error != b.last_error
            || a.history != b.history
    })
}

/// Reduce the `EggPool` worker state across every attached frontend.
///
/// Two rules, both order-independent, so two TUI windows can never race the
/// worker into two different activations:
///
/// - `active` is true when **any** frontend has the pane open. Losing the last
///   pane always converges the worker to inactive, and gaining one always
///   activates it.
/// - `period` is the **shortest** window any active frontend asked for, so the
///   pane never shows a coarser window than the operator is looking at.
///
/// Entries are keyed by the stable subscriber id the accept loop assigns, so
/// a frontend that refreshes its intent replaces its own entry instead of
/// leaving a stale one that would keep the worker activated forever.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EggpoolIntents {
    entries: std::collections::HashMap<u64, EggpoolIntent>,
}

impl EggpoolIntents {
    /// Record a subscriber's current intent, returning the previous one.
    pub fn set(&mut self, id: u64, active: bool, period: EggpoolPeriod) -> Option<EggpoolIntent> {
        self.entries.insert(id, EggpoolIntent { active, period })
    }

    /// Drop a disconnected frontend.
    pub fn remove(&mut self, id: u64) -> Option<EggpoolIntent> {
        self.entries.remove(&id)
    }

    /// Whether any frontend currently has the `EggPool` pane open.
    #[must_use]
    pub fn any_active(&self) -> bool {
        self.entries.values().any(|intent| intent.active)
    }

    /// The period the daemon should actually fetch.
    #[must_use]
    pub fn converged_period(&self, fallback: EggpoolPeriod) -> EggpoolPeriod {
        self.entries
            .values()
            .filter(|intent| intent.active)
            .map(|intent| intent.period)
            .min()
            .unwrap_or(fallback)
    }

    /// Number of attached frontends.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no frontend is attached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One frontend's cron-detail intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronIntent {
    /// The system whose cron detail this frontend has open.
    pub system_id: Option<String>,
    /// Which job's history this frontend is displaying.
    pub job: Option<String>,
    /// How many records this frontend wants published.
    pub display_history: usize,
}

/// Reduce cron-detail intents across every attached frontend.
///
/// Unlike [`EggpoolIntents`], this one does **not** shape any remote request.
/// The scheduler summary is polled on the daemon's own cadence and the history
/// body on revision change whether or not any TUI is attached, so opening a cron
/// pane can never add a remote request. What the intent governs is only which
/// records are *published*.
///
/// Reduction is a union with a per-pair maximum depth, which is order
/// independent: two windows on the same job at different depths must not make
/// the publication depend on which one spoke last, and a window asking for
/// fewer records must never be silently served the smaller set because a
/// neighbour wanted less.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CronIntents {
    entries: std::collections::HashMap<u64, CronIntent>,
}

/// The `(job, depth)` pairs the published document must carry for one system.
pub type CronRequests = std::collections::BTreeMap<String, usize>;

impl CronIntents {
    /// Record a subscriber's current intent, returning the previous one.
    pub fn set(&mut self, id: u64, intent: CronIntent) -> Option<CronIntent> {
        self.entries.insert(id, intent)
    }

    /// Drop a disconnected frontend.
    ///
    /// Necessary, not tidy: a departed window that left its intent behind would
    /// keep the daemon publishing records nobody is reading for the rest of the
    /// process's life.
    pub fn remove(&mut self, id: u64) -> Option<CronIntent> {
        self.entries.remove(&id)
    }

    /// Whether any frontend currently has a cron detail open.
    #[must_use]
    pub fn any_active(&self) -> bool {
        self.entries
            .values()
            .any(|intent| intent.system_id.is_some() && intent.job.is_some())
    }

    /// The reduced `(system, job) -> depth` set to publish.
    #[must_use]
    pub fn requests(&self) -> std::collections::BTreeMap<String, CronRequests> {
        let mut reduced: std::collections::BTreeMap<String, CronRequests> =
            std::collections::BTreeMap::new();
        for intent in self.entries.values() {
            let (Some(system_id), Some(job)) = (&intent.system_id, &intent.job) else {
                continue;
            };
            if job.is_empty() {
                continue;
            }
            let depth = intent
                .display_history
                .clamp(1, crate::cron::MAX_DISPLAY_HISTORY);
            let per_system = reduced.entry(system_id.clone()).or_default();
            let slot = per_system.entry(job.clone()).or_insert(0);
            *slot = (*slot).max(depth);
        }
        reduced
    }

    /// The reduced requests for one system, or an empty set.
    #[must_use]
    pub fn requests_for(&self, system_id: &str) -> Vec<(String, usize)> {
        self.requests()
            .get(system_id)
            .map(|per_system| {
                per_system
                    .iter()
                    .map(|(job, depth)| (job.clone(), *depth))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Number of attached frontends.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no frontend has published an intent yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod cron_intent_tests {
    use super::*;

    fn closed() -> CronIntent {
        CronIntent {
            system_id: None,
            job: None,
            display_history: 0,
        }
    }

    fn open(system: &str, job: &str, depth: usize) -> CronIntent {
        CronIntent {
            system_id: Some(system.to_owned()),
            job: Some(job.to_owned()),
            display_history: depth,
        }
    }

    #[test]
    fn a_closed_pane_asks_for_nothing() {
        let mut intents = CronIntents::default();
        intents.set(1, closed());
        assert!(!intents.any_active());
        assert!(intents.requests().is_empty());
    }

    #[test]
    fn an_open_pane_asks_for_its_own_job_only() {
        let mut intents = CronIntents::default();
        intents.set(1, open("sys-a", "backup", 5));
        assert!(intents.any_active());
        assert_eq!(
            intents.requests_for("sys-a"),
            vec![("backup".to_owned(), 5)]
        );
        assert_eq!(intents.requests_for("sys-b"), Vec::new());
    }

    #[test]
    fn two_frontends_on_the_same_job_reduce_to_the_deeper_window() {
        // Order independence: which window spoke last must not decide what the
        // other one is served.
        for order in [[1_u64, 2], [2, 1]] {
            let mut intents = CronIntents::default();
            for subscriber in order {
                let depth = if subscriber == 1 { 3 } else { 12 };
                intents.set(subscriber, open("sys-a", "backup", depth));
            }
            assert_eq!(
                intents.requests_for("sys-a"),
                vec![("backup".to_owned(), 12)],
                "a window asking for fewer records must not shrink the publication"
            );
        }
    }

    #[test]
    fn two_frontends_on_different_systems_both_get_their_own_records() {
        let mut intents = CronIntents::default();
        intents.set(1, open("sys-a", "backup", 5));
        intents.set(2, open("sys-b", "rotate", 7));
        assert_eq!(
            intents.requests_for("sys-a"),
            vec![("backup".to_owned(), 5)]
        );
        assert_eq!(
            intents.requests_for("sys-b"),
            vec![("rotate".to_owned(), 7)]
        );
    }

    #[test]
    fn two_frontends_on_the_same_system_both_get_their_own_job() {
        let mut intents = CronIntents::default();
        intents.set(1, open("sys-a", "backup", 5));
        intents.set(2, open("sys-a", "rotate", 5));
        let requests = intents.requests_for("sys-a");
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert!(requests.contains(&("backup".to_owned(), 5)));
        assert!(requests.contains(&("rotate".to_owned(), 5)));
    }

    #[test]
    fn a_disconnected_frontend_stops_asking() {
        // Not tidiness: a departed window that left its intent behind would keep
        // the daemon transmitting records nobody is reading, for the rest of the
        // process's life.
        let mut intents = CronIntents::default();
        intents.set(1, open("sys-a", "backup", 5));
        assert!(intents.any_active());
        intents.remove(1);
        assert!(!intents.any_active());
        assert!(intents.requests().is_empty());
    }

    #[test]
    fn the_last_frontend_to_close_leaves_nothing_requested() {
        let mut intents = CronIntents::default();
        intents.set(1, open("sys-a", "backup", 5));
        intents.set(2, open("sys-b", "rotate", 5));
        intents.remove(1);
        assert_eq!(intents.requests_for("sys-a"), Vec::new());
        assert_eq!(intents.requests_for("sys-b").len(), 1);
        intents.remove(2);
        assert!(intents.requests().is_empty());
    }

    #[test]
    fn a_frontend_cannot_widen_the_publication_past_the_hard_maximum() {
        let mut intents = CronIntents::default();
        intents.set(1, open("sys-a", "backup", usize::MAX));
        assert_eq!(
            intents.requests_for("sys-a"),
            vec![("backup".to_owned(), crate::cron::MAX_DISPLAY_HISTORY)],
            "the daemon clamps what a frontend asks for"
        );
    }

    #[test]
    fn a_zero_depth_is_clamped_rather_than_producing_an_empty_slice() {
        let mut intents = CronIntents::default();
        intents.set(1, open("sys-a", "backup", 0));
        assert_eq!(
            intents.requests_for("sys-a"),
            vec![("backup".to_owned(), 1)]
        );
    }

    #[test]
    fn an_empty_job_name_is_ignored() {
        let mut intents = CronIntents::default();
        intents.set(1, open("sys-a", "", 5));
        assert!(intents.requests().is_empty());
    }

    // --- Document projection ---

    /// A fleet with one system whose cron cache holds `count` records for
    /// `job`, so the projection can be exercised end to end.
    fn fleet_with_cron(job: &str, count: u64) -> FleetState {
        use gregg_protocol::{
            SchedulerEpochV2, SchedulerHistoryV2, SchedulerJobHistoryV2, SchedulerRunRecordV2,
        };
        let config = Config {
            systems: vec![crate::config::SystemEntry {
                id: "sys-a".to_owned(),
                host: "box".to_owned(),
                port: 11310,
                name: None,
            }],
            ..Config::default()
        };
        let mut fleet = FleetState::from_config(&config);
        let epoch = SchedulerEpochV2 {
            started_at_unix_ms: 1_000,
            nonce: 1,
        };
        let records: Vec<SchedulerRunRecordV2> = (0..count)
            .map(|sequence| SchedulerRunRecordV2 {
                sequence,
                scheduled_unix_ms: 1_700_000_000_000 + sequence,
                started_unix_ms: Some(1_700_000_000_100 + sequence),
                finished_unix_ms: 1_700_000_001_000 + sequence,
                outcome: gregg_protocol::SchedulerOutcomeV2::Success,
                exit_code: Some(0),
                signal: None,
                duration_ms: Some(900),
                delay_ms: 0,
                coalesced: false,
                stdout: gregg_protocol::SchedulerOutputV2::new(String::new(), false),
                stderr: gregg_protocol::SchedulerOutputV2::new(String::new(), false),
            })
            .collect();
        // A summary too: the job list is what `c` and `Shift-J`/`Shift-K` read,
        // and a history document on its own names no jobs.
        fleet.cron.apply_summary(
            "sys-a",
            gregg_protocol::SchedulerSummaryV2 {
                schema_version: 2,
                generated_at_unix_ms: 1_700_000_000_000,
                epoch,
                history_revision: 1,
                jobs: vec![gregg_protocol::SchedulerJobV2 {
                    name: job.to_owned(),
                    schedule: "0 3 * * *".to_owned(),
                    next_due_unix_ms: 1_700_000_100_000,
                    state: gregg_protocol::SchedulerJobStateV2::Idle,
                    load: None,
                    pending_since_unix_ms: None,
                    next_retry_unix_ms: None,
                    running_since_unix_ms: None,
                    last: None,
                }],
            },
            1_700_000_000_000,
        );
        fleet.cron.apply_history(
            "sys-a",
            &SchedulerHistoryV2 {
                schema_version: 2,
                generated_at_unix_ms: 1_700_000_000_000,
                epoch,
                history_revision: 1,
                jobs: vec![SchedulerJobHistoryV2 {
                    name: job.to_owned(),
                    records,
                }],
            },
        );
        fleet
    }

    #[test]
    fn a_document_with_nobody_asking_carries_no_history() {
        // This is the whole point of the two-tier model: with no cron pane open
        // anywhere, the largest document in the system is not retransmitted on
        // every five-second metrics publication.
        let fleet = fleet_with_cron("backup", 20);
        let document = fleet.to_dto(std::time::Instant::now(), 1, 1);
        let cron = document.cron_for("sys-a").expect("an entry per system");
        assert_eq!(
            cron.history,
            Vec::new(),
            "history must not be published unsolicited"
        );
    }

    #[test]
    fn an_open_pane_publishes_only_its_own_job_at_its_own_depth() {
        let fleet = fleet_with_cron("backup", 20);
        let mut intents = CronIntents::default();
        intents.set(1, open("sys-a", "backup", 5));
        let document = fleet.to_dto_for(std::time::Instant::now(), 1, 1, &intents);

        let cron = document.cron_for("sys-a").expect("an entry per system");
        assert_eq!(cron.history.len(), 1, "only the requested job");
        let records = &cron.history[0].records;
        assert_eq!(records.len(), 5, "the requested depth");
        assert_eq!(
            records.last().map(|r| r.record.sequence),
            Some(19),
            "the newest window, not the oldest"
        );
    }

    #[test]
    fn a_job_with_no_records_is_omitted_rather_than_published_empty() {
        let fleet = fleet_with_cron("backup", 3);
        let mut intents = CronIntents::default();
        intents.set(1, open("sys-a", "never-ran", 5));
        let document = fleet.to_dto_for(std::time::Instant::now(), 1, 1, &intents);
        let cron = document.cron_for("sys-a").expect("an entry per system");
        assert_eq!(cron.history, Vec::new());
    }

    // --- Actions ---

    #[test]
    fn opening_the_cron_pane_lands_on_a_defined_job_deterministically() {
        let fleet = fleet_with_cron("backup", 3);
        let mut intents = CronIntents::default();
        intents.set(1, open("sys-a", "backup", 5));
        let document = fleet.to_dto_for(std::time::Instant::now(), 1, 1, &intents);
        let mut app = AppState::from_snapshot(&document);
        app.cron_job = None;

        app.apply_action(crate::action::Action::ToggleCron);
        assert!(app.cron_expanded);
        assert_eq!(
            app.cron_job.as_deref(),
            Some("backup"),
            "a pane that opened with no job named would render an empty history block"
        );

        // Opening twice lands on the same job: the default is not order-of-keys
        // dependent.
        app.cron_job = None;
        app.apply_action(crate::action::Action::ToggleCron);
        app.apply_action(crate::action::Action::ToggleCron);
        assert_eq!(app.cron_job.as_deref(), Some("backup"));
    }

    #[test]
    fn a_cron_pane_on_a_system_with_no_scheduler_asks_for_nothing() {
        let mut app = AppState::blank();
        app.systems = vec![SystemState {
            id: "sys-a".to_owned(),
            endpoint: Endpoint::new("box".to_owned(), 11310, None),
            configured_name: None,
            reachability: Reachability::Online,
            latest: None,
            last_success_at: None,
            last_attempt_at: None,
            latency: None,
            offline_reason: None,
        }];
        app.selected_id = Some("sys-a".to_owned());
        app.cron_expanded = true;
        // The pane is open but the daemon has said nothing yet, so there is no
        // job to ask for. Inventing one would be a request the daemon cannot
        // answer, and the block would show a header for a job that does not
        // exist.
        assert_eq!(app.selected_cron_job(), None);
        assert_eq!(app.cron_job_names(), Vec::<&str>::new());
    }

    #[test]
    fn the_expansions_are_independent_of_one_another() {
        // `n` is deliberately excluded here: unlike `d` and `c` it is gated on
        // the system actually having network telemetry, so on a system with no
        // snapshot it is a no-op. That gate is covered by its own tests.
        let mut app = AppState::blank();
        app.systems = vec![SystemState {
            id: "sys-a".to_owned(),
            endpoint: Endpoint::new("box".to_owned(), 11310, None),
            configured_name: None,
            reachability: Reachability::Online,
            latest: None,
            last_success_at: None,
            last_attempt_at: None,
            latency: None,
            offline_reason: None,
        }];
        app.selected_id = Some("sys-a".to_owned());
        app.apply_action(crate::action::Action::ToggleDrives);
        app.apply_action(crate::action::Action::ToggleCron);
        assert!(
            app.drives_expanded && app.cron_expanded,
            "opening cron must not close drives"
        );

        // Closing one must not close the other.
        app.apply_action(crate::action::Action::ToggleCron);
        assert!(!app.cron_expanded);
        assert!(app.drives_expanded, "closing cron must not close drives");
        app.apply_action(crate::action::Action::ToggleDrives);
        assert!(!app.drives_expanded);
        app.apply_action(crate::action::Action::ToggleCron);
        assert!(app.cron_expanded, "reopening cron must not reopen drives");
    }

    #[test]
    fn a_closing_cron_pane_forgets_its_job() {
        // Retained state for a closed pane would be re-expanded into a job the
        // operator had moved away from.
        let fleet = fleet_with_cron("backup", 3);
        let mut intents = CronIntents::default();
        intents.set(1, open("sys-a", "backup", 5));
        let document = fleet.to_dto_for(std::time::Instant::now(), 1, 1, &intents);
        let mut app = AppState::from_snapshot(&document);
        app.cron_expanded = true;
        assert_eq!(app.cron_job.as_deref(), Some("backup"));
        app.apply_action(crate::action::Action::ToggleCron);
        assert!(!app.cron_expanded);
        app.apply_action(crate::action::Action::ToggleCron);
        assert_eq!(app.cron_job.as_deref(), Some("backup"));
    }

    #[test]
    fn moving_the_cron_sub_selection_is_clamped_and_never_wraps() {
        // A `K` on the first job must be obviously a no-op, not a silent jump
        // to the last one.
        let fleet = fleet_with_cron("backup", 3);
        let mut intents = CronIntents::default();
        intents.set(1, open("sys-a", "backup", 5));
        let document = fleet.to_dto_for(std::time::Instant::now(), 1, 1, &intents);
        let mut app = AppState::from_snapshot(&document);
        app.cron_expanded = true;
        app.cron_job = Some("backup".to_owned());

        app.apply_action(crate::action::Action::CronJobPrevious);
        assert_eq!(
            app.cron_job.as_deref(),
            Some("backup"),
            "clamped, not wrapped"
        );
        // Only one job exists, so next is also clamped.
        app.apply_action(crate::action::Action::CronJobNext);
        assert_eq!(app.cron_job.as_deref(), Some("backup"));
    }

    #[test]
    fn moving_between_systems_repairs_a_stale_cron_job_name() {
        // The name is retained so an unchanged selection survives a system
        // switch, but a name the new system does not have must not be rendered.
        let mut config = Config {
            systems: vec![
                crate::config::SystemEntry {
                    id: "a".to_owned(),
                    host: "a".to_owned(),
                    port: 11310,
                    name: None,
                },
                crate::config::SystemEntry {
                    id: "b".to_owned(),
                    host: "b".to_owned(),
                    port: 11310,
                    name: None,
                },
            ],
            ..Config::default()
        };
        let mut first = crate::clientd::snapshot::FrontendSnapshot::empty(Vec::new());
        first.systems = (0..2)
            .map(|index| crate::clientd::snapshot::SystemSnapshotDto {
                id: if index == 0 { "a" } else { "b" }.to_owned(),
                endpoint: Endpoint::new("x".to_owned(), 11310, None),
                configured_name: None,
                reachability: Reachability::Pending,
                latest: None,
                last_success_at_unix_ms: None,
                last_attempt_at_unix_ms: None,
                latency_ms: None,
                offline_reason: None,
            })
            .collect();
        first.cron = (0..2)
            .map(|index| SystemCronDto {
                system_id: if index == 0 { "a" } else { "b" }.to_owned(),
                capability: crate::cron::CronCapability::Supported,
                summary: Some(gregg_protocol::SchedulerSummaryV2 {
                    schema_version: 2,
                    generated_at_unix_ms: 1_700_000_000_000,
                    epoch: gregg_protocol::SchedulerEpochV2 {
                        started_at_unix_ms: 1,
                        nonce: 1,
                    },
                    history_revision: 0,
                    jobs: vec![gregg_protocol::SchedulerJobV2 {
                        name: if index == 0 { "only-on-a" } else { "only-on-b" }.to_owned(),
                        schedule: "0 3 * * *".to_owned(),
                        next_due_unix_ms: 0,
                        state: gregg_protocol::SchedulerJobStateV2::Idle,
                        load: None,
                        pending_since_unix_ms: None,
                        next_retry_unix_ms: None,
                        running_since_unix_ms: None,
                        last: None,
                    }],
                }),
                epoch: None,
                history_revision: None,
                last_attempt_at_unix_ms: None,
                last_success_at_unix_ms: None,
                last_error: None,
                history: Vec::new(),
            })
            .collect();
        let mut app = AppState::from_snapshot(&first);
        app.cron_expanded = true;
        app.cron_job = Some("only-on-a".to_owned());
        assert_eq!(app.selected_id.as_deref(), Some("a"));
        assert_eq!(app.selected_cron_job(), Some("only-on-a"));

        config.systems.truncate(1);
        let _ = config;
        app.apply_action(crate::action::Action::MoveDown);
        assert_eq!(app.selected_id.as_deref(), Some("b"));
        assert_eq!(
            app.selected_cron_job(),
            Some("only-on-b"),
            "the retained name belongs to a different system and must be repaired"
        );
    }

    #[test]
    fn a_stale_scheduler_read_is_a_render_visible_change() {
        // If this were false, a failing cron read would never repaint and the
        // operator would keep looking at numbers they think are current.
        let fleet = fleet_with_cron("backup", 3);
        let mut document = fleet.to_dto(std::time::Instant::now(), 1, 1);
        let mut app = AppState::from_snapshot(&document);
        document.generation = 2;
        document.cron[0].last_error =
            Some(crate::cron::CronFetchError::Transport("boom".to_owned()));
        assert!(app.adopt_snapshot(&document));
    }

    #[test]
    fn an_unchanged_scheduler_read_is_not_a_render_visible_change() {
        // Timestamps move on every publication; treating that as a change would
        // redraw on every poll.
        let fleet = fleet_with_cron("backup", 3);
        let document = fleet.to_dto(std::time::Instant::now(), 1, 1);
        let mut app = AppState::from_snapshot(&document);
        let mut later = document.clone();
        later.generation = 2;
        later.produced_at_unix_ms = document.produced_at_unix_ms + 5_000;
        assert!(
            !app.adopt_snapshot(&later),
            "an identical scheduler read must not force a frame"
        );
    }

    #[test]
    fn the_configured_display_depth_travels_with_the_document() {
        let mut config = Config {
            cron: crate::config::CronConfig {
                display_history: 7,
                cache_history: 25,
            },
            systems: vec![crate::config::SystemEntry {
                id: "sys-a".to_owned(),
                host: "box".to_owned(),
                port: 11310,
                name: None,
            }],
            ..Config::default()
        };
        let fleet = FleetState::from_config(&config);
        let document = fleet.to_dto(std::time::Instant::now(), 1, 1);
        assert_eq!(document.cron_display_history, 7);
        let app = AppState::from_snapshot(&document);
        assert_eq!(
            app.cron_display_history, 7,
            "the TUI must not read the config file to learn its own depth"
        );
        config.cron.display_history = 5;
    }

    #[test]
    fn every_system_gets_a_cron_entry_even_before_it_is_polled() {
        // A frontend must never have to infer "no entry" from a missing key, or
        // "not asked yet" and "asked and got nothing" become indistinguishable.
        let config = Config {
            systems: vec![
                crate::config::SystemEntry {
                    id: "a".to_owned(),
                    host: "a".to_owned(),
                    port: 11310,
                    name: None,
                },
                crate::config::SystemEntry {
                    id: "b".to_owned(),
                    host: "b".to_owned(),
                    port: 11310,
                    name: None,
                },
            ],
            ..Config::default()
        };
        let fleet = FleetState::from_config(&config);
        let document = fleet.to_dto(std::time::Instant::now(), 1, 1);
        assert_eq!(document.cron.len(), 2);
        for entry in &document.cron {
            assert_eq!(entry.capability, crate::cron::CronCapability::Unknown);
            assert!(entry.summary.is_none());
            assert!(entry.last_error.is_none());
        }
    }
}

/// Current wall-clock reading in Unix milliseconds.
///
/// Every local IPC timestamp goes through this one function so the daemon and
/// its frontends cannot disagree about which clock produced a stamp.
#[must_use]
pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Convert a process-local `Instant` into Unix milliseconds relative to the
/// daemon's own wall clock.
///
/// `None` stays `None`: a never-polled system must stay distinguishable from a
/// polled one, and a zero timestamp would read as "1970".
#[must_use]
fn elapsed_to_unix_ms(value: Option<Instant>, now: Instant, now_unix_ms: u64) -> Option<u64> {
    let value = value?;
    // `saturating_sub` keeps a clock that stepped backwards between the event
    // and publication from wrapping into the far future.
    let elapsed_ms = now.saturating_duration_since(value).as_millis();
    Some(
        u64::try_from(elapsed_ms)
            .unwrap_or(u64::MAX)
            .saturating_add(now_unix_ms),
    )
}

/// Rebuild a local `Instant` from Unix milliseconds published by the daemon.
///
/// The daemon and the TUI run on the same machine, so the difference between
/// the document's wall-clock stamp and the frontend's own reading of the clock
/// is the age the operator should see. A stamp in the future (clock stepped,
/// or a stale document) is treated as *now* rather than as negative age.
#[must_use]
fn instant_from_unix_ms(value: Option<u64>, now: Instant, now_unix_ms: u64) -> Option<Instant> {
    let value = value?;
    let age_ms = now_unix_ms.saturating_sub(value);
    Some(
        now.checked_sub(Duration::from_millis(age_ms))
            .unwrap_or(now),
    )
}

/// Whether two system lists differ in anything a rendered row shows.
///
/// A row shows the endpoint, the configured name, reachability, the normalized
/// snapshot, and the offline reason. It also shows *that* a system exists, so
/// a length change is a visible change. Timestamps and latency are excluded on
/// purpose: they only feed the "updated Ns ago" age, and treating a new age as
/// a new row would force a frame on every poll.
#[must_use]
fn systems_visibly_differ(old: &[SystemState], new: &[SystemSnapshotDto]) -> bool {
    old.len() != new.len()
        || old.iter().zip(new).any(|(old, new)| {
            old.id != new.id
                || old.endpoint != new.endpoint
                || old.configured_name != new.configured_name
                || old.reachability != new.reachability
                || old.latest != new.latest
                || old.offline_reason != new.offline_reason
        })
}

/// Whether the `EggPool` pane would render differently.
///
/// The endpoint, window, worker availability, both data planes, and both error
/// classifications count. The last-attempt ages do not, for the same reason
/// system timestamps do not: an age that advanced is not a changed row.
#[must_use]
fn eggpool_visibly_differ(old: Option<&EggpoolState>, new: Option<&EggpoolSnapshotDto>) -> bool {
    match (old, new) {
        (None, None) => false,
        (Some(_), None) | (None, Some(_)) => true,
        (Some(old), Some(new)) => {
            old.endpoint != new.endpoint
                || old.period != new.period
                || old.worker_state != new.worker_state
                || old.summary != new.summary
                || old.health != new.health
                || old.last_error != new.last_error
                || old.last_health_error != new.last_health_error
        }
    }
}

fn system_to_dto(system: &SystemState, now: Instant, now_unix_ms: u64) -> SystemSnapshotDto {
    SystemSnapshotDto {
        id: system.id.clone(),
        endpoint: system.endpoint.clone(),
        configured_name: system.configured_name.clone(),
        reachability: system.reachability,
        latest: system.latest.clone(),
        last_success_at_unix_ms: elapsed_to_unix_ms(system.last_success_at, now, now_unix_ms),
        last_attempt_at_unix_ms: elapsed_to_unix_ms(system.last_attempt_at, now, now_unix_ms),
        latency_ms: system
            .latency
            .map(|latency| u64::try_from(latency.as_millis()).unwrap_or(u64::MAX)),
        offline_reason: system.offline_reason.clone(),
    }
}

/// The top-level application state.
///
/// Plan 164 groups this as the frontend's *render model*: published fleet data
/// on one side, per-frontend presentation on the other. The grouping is
/// documented rather than encoded as nested structs because the renderer reads
/// both together on every frame; what matters for ownership is that only
/// [`Self::adopt_snapshot`] writes the fleet group and only
/// [`Self::apply_action_changed`] writes the presentation group.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
pub struct AppState {
    // ===== published fleet data =====
    //
    // Written by exactly one function, `adopt_snapshot`, from a document the
    // client daemon published. Nothing in a TUI may mutate these directly.
    /// Ordered list of all monitored systems, in configured order.
    pub systems: Vec<SystemState>,
    /// Current refresh status as last published by the daemon.
    pub refresh_status: RefreshStatus,
    /// Diagnostic from the most recent rejected config reload.
    pub config_reload_error: Option<String>,
    /// Optional `EggPool` pane state.
    pub eggpool: Option<EggpoolState>,
    /// Plan 166: scheduler observability for every system, in the document's
    /// order.
    ///
    /// Published fleet data, so it is written only by [`Self::adopt_snapshot`]
    /// and lives in its own vector rather than inside [`SystemState`]: the two
    /// planes are fetched on different cadences, sized differently, and fail
    /// independently, and merging them would make "metrics are fine but the
    /// cron route failed" hard to express.
    pub cron: Vec<SystemCronDto>,
    /// How many cron records the configured pane should display.
    pub cron_display_history: usize,
    /// Local IPC generation of the last document this frontend applied.
    ///
    /// Used to drop superseded documents. This is *not* the poll generation:
    /// it counts publications, so a slow frontend can skip straight to the
    /// newest state without losing an ordered poll result.
    pub last_snapshot_generation: u64,

    // ===== presentation state =====
    //
    // Written only by `apply_action_changed` and `adopt_snapshot`'s
    // selection repair. These are per frontend: two TUI windows on one
    // daemon genuinely have different selections.
    /// Currently selected system, by stable ID.
    pub selected_id: Option<SystemId>,
    /// The first visible system in the viewport, by stable ID.
    pub viewport_top_id: Option<SystemId>,
    /// Terminal dimensions (width, height), if known.
    pub terminal_size: Option<(u16, u16)>,
    /// Currently active top-level pane.
    pub active_pane: Pane,
    /// Current Systems presentation mode.
    pub system_view_mode: SystemViewMode,
    /// Whether the selected online system's drives are expanded.
    pub drives_expanded: bool,
    /// Whether the selected online system's network details are expanded.
    /// This is independent of drive expansion so both telemetry families
    /// can be inspected at once.
    pub network_expanded: bool,
    /// Whether the selected system's cron details are expanded.
    ///
    /// Independent of drive and network expansion: opening cron details must
    /// not silently close the other two.
    pub cron_expanded: bool,
    /// Which cron job's history is expanded, by name.
    ///
    /// Only one job's multiline history is shown at a time, because a
    /// cron-expanded system may have dozens of jobs. Held by name so moving
    /// between systems can repair it against a different job list, and so a
    /// reordering of the remote's job list does not silently change which job
    /// is being read.
    pub cron_job: Option<String>,
    /// Plan 087: whether the logical selection is currently being
    /// visually highlighted with `Modifier::REVERSED`. Independent of
    /// `selected_id`; cleared by the event-loop timer (about ten
    /// seconds of inactivity) and on pane changes away from Systems.
    /// Logical selection itself remains available for keyboard actions
    /// (`d` and friends) when this flag is `false`.
    pub selection_highlight_active: bool,
    /// A period the operator asked for that the daemon has not confirmed yet.
    ///
    /// The `EggPool` period is daemon-owned, because only the daemon knows
    /// which period it actually fetched. `j`/`k` on the pane records a
    /// *request* here and the event loop forwards it over IPC; the field is
    /// cleared as soon as a document carrying the daemon's converged period
    /// arrives. Keeping the request separate from `eggpool.period` is what
    /// stops the pane from claiming a window whose numbers were fetched for
    /// a different one.
    pub eggpool_period_request: Option<EggpoolPeriod>,

    /// Test-only shadow of the daemon's [`FleetState`].
    ///
    /// Renderer and reducer tests need to put data on the screen, and before
    /// Plan 164 they did it by calling the reducer that lived in the same
    /// struct. That reducer is now the daemon's, so these tests drive the
    /// *real* path instead: apply a batch to a fleet, project it into a
    /// document, and adopt that document. A test that constructs
    /// `SystemState` values by hand would pass while the actual
    /// daemon-to-frontend pipeline was broken, which is precisely the code
    /// this field exists to keep honest.
    ///
    /// The field does not exist in a production build, so no production path
    /// can reach a fleet mutation through a frontend.
    #[cfg(test)]
    test_fleet: FleetState,
    /// Test-only local IPC generation counter for [`Self::republish`].
    #[cfg(test)]
    test_generation: u64,
    /// Whether any published document has carried polled reachability.
    ///
    /// Frontend-local, and deliberately *not* derived from
    /// `last_snapshot_generation`: a frontend that attaches after the daemon
    /// has been polling still needs the one-time placement, and a frontend
    /// that was served the pre-poll document must still get it when the first
    /// batch arrives.
    saw_reachability: bool,
}

impl AppState {
    /// Build a frontend render model from the first published document.
    #[must_use]
    pub fn from_snapshot(snapshot: &FrontendSnapshot) -> Self {
        let mut state = Self::blank();
        state.adopt_snapshot_inner(snapshot, Instant::now(), now_unix_ms());
        state
    }

    /// Build a render model from a configuration, for tests and tooling.
    ///
    /// This routes through the *same* publication path a real document takes:
    /// a [`FleetState`] is projected into a DTO and then adopted. That is
    /// deliberate — a test helper that built `SystemState` values by hand
    /// could drift from what the daemon actually publishes, and would then
    /// pass while the real pipeline was broken.
    ///
    /// No production frontend calls this. A real TUI receives its first state
    /// from the daemon over local IPC and never reads the configuration file,
    /// because reading it would make the TUI a second owner of fleet data.
    #[cfg(test)]
    #[must_use]
    pub fn synthetic(config: &Config) -> Self {
        let mut state = Self::blank();
        state.test_fleet = FleetState::from_config(config);
        state.republish();
        state
    }

    /// Project the shadow fleet and adopt the resulting document.
    #[cfg(test)]
    fn republish(&mut self) -> bool {
        self.test_generation = self.test_generation.saturating_add(1);
        let snapshot = self.test_fleet.to_dto(
            std::time::Instant::now(),
            now_unix_ms(),
            self.test_generation,
        );
        // The generation is *not* reset: only the very first document may snap
        // selection to display-order position zero. Zeroing it here would make
        // every publish look like a fresh TUI and re-introduce the
        // reset-on-every-batch behaviour the reducer had already fixed.
        self.adopt_snapshot(&snapshot)
    }

    /// A render model with no data at all, used before the first document.
    #[must_use]
    pub fn blank() -> Self {
        Self {
            systems: Vec::new(),
            refresh_status: RefreshStatus::Idle,
            config_reload_error: None,
            eggpool: None,
            cron: Vec::new(),
            cron_display_history: crate::cron::DEFAULT_DISPLAY_HISTORY,
            last_snapshot_generation: 0,
            selected_id: None,
            viewport_top_id: None,
            terminal_size: None,
            active_pane: Pane::Systems,
            system_view_mode: SystemViewMode::Normal,
            drives_expanded: false,
            network_expanded: false,
            cron_expanded: false,
            cron_job: None,
            selection_highlight_active: false,
            eggpool_period_request: None,
            #[cfg(test)]
            test_fleet: FleetState {
                cron: crate::cron::CronCache::default(),
                cron_display_history: crate::cron::DEFAULT_DISPLAY_HISTORY,
                systems: Vec::new(),
                last_applied_generation: 0,
                refresh_status: RefreshStatus::Idle,
                config_reload_error: None,
                eggpool: None,
            },
            #[cfg(test)]
            test_generation: 0,
            saw_reachability: false,
        }
    }

    /// The only writer of fleet data in a frontend.
    ///
    /// Returns `true` when anything a renderer can see changed, so the event
    /// loop's draw gate stays as cheap as it was when the TUI owned the
    /// reducer itself. A document that is not newer than the last applied one
    /// is dropped without touching anything, which is what lets a slow
    /// frontend skip revisions.
    pub fn adopt_snapshot(&mut self, snapshot: &FrontendSnapshot) -> bool {
        if !snapshot.is_newer_than(self.last_snapshot_generation) {
            return false;
        }
        let now = Instant::now();
        let now_unix_ms = now_unix_ms();
        self.adopt_snapshot_inner(snapshot, now, now_unix_ms)
    }

    #[allow(clippy::too_many_lines)]
    fn adopt_snapshot_inner(
        &mut self,
        snapshot: &FrontendSnapshot,
        now: Instant,
        now_unix_ms: u64,
    ) -> bool {
        let first_document = self.last_snapshot_generation == 0;
        // The one-time placement is tied to the *fleet* reaching polled
        // reachability, not to this frontend hearing anything at all.
        let first_reachability = !self.saw_reachability && snapshot.poll_initialized;
        self.saw_reachability |= snapshot.poll_initialized;
        let selected_before = self.selected_id.clone();
        let viewport_before = self.viewport_top_id.clone();
        let pane_before = self.active_pane;
        let refresh_before = self.refresh_status.clone();
        let reload_error_before = self.config_reload_error.clone();

        self.refresh_status = snapshot.refresh_status.clone();
        self.config_reload_error
            .clone_from(&snapshot.config_reload_error);
        // Compare before overwriting: the incoming document is borrowed from
        // the caller, so this needs no clone of the state it replaces.
        let eggpool_changed =
            eggpool_visibly_differ(self.eggpool.as_ref(), snapshot.eggpool.as_ref());
        self.eggpool = snapshot.eggpool.as_ref().map(|dto| EggpoolState {
            endpoint: dto.endpoint.clone(),
            period: dto.period,
            request_generation: dto.request_generation,
            worker_state: dto.worker_state,
            summary: dto.summary.clone(),
            last_success_at: instant_from_unix_ms(dto.last_success_at_unix_ms, now, now_unix_ms),
            last_attempt_at: instant_from_unix_ms(dto.last_attempt_at_unix_ms, now, now_unix_ms),
            last_error: dto.last_error.clone(),
            health: dto.health.clone(),
            last_health_success_at: instant_from_unix_ms(
                dto.last_health_success_at_unix_ms,
                now,
                now_unix_ms,
            ),
            last_health_attempt_at: instant_from_unix_ms(
                dto.last_health_attempt_at_unix_ms,
                now,
                now_unix_ms,
            ),
            last_health_error: dto.last_health_error.clone(),
        });
        let new_systems: Vec<SystemState> = snapshot
            .systems
            .iter()
            .map(|dto| {
                let mut endpoint = dto.endpoint.clone();
                endpoint.host = clean(&endpoint.host);
                endpoint.name = endpoint.name.as_deref().map(clean);
                let mut latest = dto.latest.clone();
                if let Some(latest) = latest.as_mut() {
                    clean_snapshot(latest);
                }
                SystemState {
                    id: dto.id.clone(),
                    endpoint,
                    configured_name: dto.configured_name.as_deref().map(clean),
                    reachability: dto.reachability,
                    latest,
                    last_success_at: instant_from_unix_ms(
                        dto.last_success_at_unix_ms,
                        now,
                        now_unix_ms,
                    ),
                    last_attempt_at: instant_from_unix_ms(
                        dto.last_attempt_at_unix_ms,
                        now,
                        now_unix_ms,
                    ),
                    latency: dto.latency_ms.map(Duration::from_millis),
                    offline_reason: dto.offline_reason.clone(),
                }
            })
            .collect();
        self.eggpool_period_request = None;
        self.last_snapshot_generation = snapshot.generation;

        // Cron arrives through the same document, sanitized at the same single
        // boundary as the metrics it sits beside.
        let mut new_cron = snapshot.cron.clone();
        for entry in &mut new_cron {
            clean_cron(entry);
        }
        let cron_changed = cron_visibly_differ(&self.cron, &new_cron);
        self.cron = new_cron;
        self.cron_display_history = snapshot.cron_display_history;

        // Whether a redraw is warranted is a question about what the operator
        // can *see*, so it is decided here, before the old values are dropped.
        // Timestamps and latency alone never count: a row that already says
        // the same thing at a slightly different age does not need a frame, and
        // treating it as changed would redraw on every single poll.
        let systems_changed = systems_visibly_differ(&self.systems, &snapshot.systems);
        self.systems = new_systems;
        let visible_changed = self.refresh_status != refresh_before
            || self.config_reload_error != reload_error_before
            || eggpool_changed
            || systems_changed
            || cron_changed
            || self.cron_display_history != snapshot.cron_display_history;

        // An `EggPool`-only config has nothing to select in Systems, so the
        // pane starts where the content is.
        if first_document {
            self.active_pane = if self.systems.is_empty() && self.eggpool.is_some() {
                Pane::Eggpool
            } else {
                Pane::Systems
            };
            self.selected_id = self.systems.first().map(|system| system.id.clone());
            self.viewport_top_id = self.selected_id.clone();
        } else {
            // A reload may add or remove systems, so repair the logical IDs
            // against the new list before anything reads them.
            self.selected_id = self
                .selected_id
                .take()
                .filter(|id| self.systems.iter().any(|system| &system.id == id))
                .or_else(|| self.systems.first().map(|system| system.id.clone()));
            self.viewport_top_id = self
                .viewport_top_id
                .take()
                .filter(|id| self.systems.iter().any(|system| &system.id == id))
                .or_else(|| self.selected_id.clone());
            if self.systems.is_empty() {
                self.selected_id = None;
                self.viewport_top_id = None;
            }
        }
        // A selection change, a reload, or a remote that dropped a job can all
        // leave the retained cron job name naming something the selected system
        // no longer has.
        self.repair_cron_selection();

        // The first document that carries polled reachability establishes the
        // reachability-sorted display order. Pinning selection and viewport top
        // to that order stops an offline first-configured system from dragging
        // the viewport below the online entries that came back first. Later
        // documents preserve ordinary selection and scroll semantics.
        if first_reachability {
            if let Some(&first_index) = self.display_order().first() {
                if let Some(first_system) = self.systems.get(first_index) {
                    let id = first_system.id.clone();
                    self.selected_id = Some(id.clone());
                    self.viewport_top_id = Some(id);
                }
            }
        }
        ensure_selected_visible(self);

        visible_changed
            || selected_before != self.selected_id
            || viewport_before != self.viewport_top_id
            || pane_before != self.active_pane
    }

    /// The published scheduler state for one system.
    #[must_use]
    pub fn cron_for(&self, system_id: &str) -> Option<&SystemCronDto> {
        self.cron.iter().find(|entry| entry.system_id == system_id)
    }

    /// The published scheduler state for the selected system.
    #[must_use]
    pub fn selected_cron(&self) -> Option<&SystemCronDto> {
        self.selected_id.as_deref().and_then(|id| self.cron_for(id))
    }

    /// Job names the selected system's scheduler currently reports, in the
    /// remote's own order.
    ///
    /// Empty for a system with no scheduler, an old daemon, or no summary yet.
    #[must_use]
    pub fn cron_job_names(&self) -> Vec<&str> {
        self.selected_cron()
            .and_then(|cron| cron.summary.as_ref())
            .map(|summary| summary.jobs.iter().map(|job| job.name.as_str()).collect())
            .unwrap_or_default()
    }

    /// The job whose history the cron block expands, defaulting to the first.
    ///
    /// A `None` job list yields `None` rather than an invented name, so an
    /// unsupported daemon cannot show a fabricated job header.
    #[must_use]
    pub fn selected_cron_job(&self) -> Option<&str> {
        let names = self.cron_job_names();
        if names.is_empty() {
            return None;
        }
        match self.cron_job.as_deref() {
            Some(current) if names.contains(&current) => Some(current),
            // The retained name is gone (a system switch, or the remote dropped
            // the job). Falling back to the first job keeps the block truthful
            // rather than showing a header for a job that no longer exists.
            _ => names.first().copied(),
        }
    }

    /// Move the cron sub-selection by `delta`, clamped to the job list.
    ///
    /// Clamped rather than wrapping so repeated `K` on the first job is
    /// obviously a no-op instead of silently landing on the last one.
    fn step_cron_job(&mut self, delta: isize) {
        let names = self.cron_job_names();
        if names.is_empty() {
            self.cron_job = None;
            return;
        }
        let current = self
            .selected_cron_job()
            .and_then(|name| names.iter().position(|candidate| *candidate == name))
            .unwrap_or(0);
        let last = names.len() - 1;
        let next = isize::try_from(current)
            .unwrap_or(0)
            .saturating_add(delta)
            .clamp(0, isize::try_from(last).unwrap_or(0));
        self.cron_job = names
            .get(usize::try_from(next).unwrap_or(0))
            .map(|name| (*name).to_owned());
    }

    /// Repair the cron sub-selection against the selected system's job list.
    ///
    /// Called after every selection change and after every document, so moving
    /// between systems, or a remote that dropped a job, cannot leave the block
    /// naming a job that does not exist. The retained name is *adopted*, not
    /// merely validated, so `cron_job` always agrees with what the pane shows
    /// and there is no second, subtly different answer to "which job is this".
    pub fn repair_cron_selection(&mut self) {
        self.cron_job = self.selected_cron_job().map(str::to_owned);
    }

    /// Record the `EggPool` period the operator asked for.
    ///
    /// Returns the request when it differs from what is already pending, so
    /// the event loop forwards a changed request exactly once.
    pub fn request_eggpool_period(&mut self, longer: bool) -> Option<EggpoolPeriod> {
        let current = self.eggpool.as_ref()?.period;
        let next = if longer {
            current.longer()
        } else {
            current.shorter()
        };
        if next == current || self.eggpool_period_request == Some(next) {
            return None;
        }
        self.eggpool_period_request = Some(next);
        Some(next)
    }

    /// Return the display order: online systems first (in configured
    /// order), then offline/pending systems (in configured order).
    #[must_use]
    pub fn display_order(&self) -> Vec<usize> {
        display_order_of(&self.systems)
    }

    /// Apply a user action.
    pub fn apply_action(&mut self, action: Action) {
        let _ = self.apply_action_changed(action);
    }

    /// Plan 143: action application reporting render-visible change.
    ///
    /// Returns `false` for boundary navigation with unchanged selection and
    /// highlight, clearing an already-clear highlight, `RefreshNow`/`Quit`
    /// (handled by the scheduler/event loop), and other logical no-ops.
    /// `Resize` always reports changed. No full-`AppState` equality scan is
    /// performed; only render-visible fields are compared.
    pub fn apply_action_changed(&mut self, action: Action) -> bool {
        if matches!(action, Action::Resize { .. }) {
            self.apply_action_inner(action);
            return true;
        }
        let before_selected = self.selected_id.clone();
        let before_viewport = self.viewport_top_id.clone();
        let before_pane = self.active_pane;
        let before_view = self.system_view_mode;
        let before_drives = self.drives_expanded;
        let before_network = self.network_expanded;
        let before_cron = (self.cron_expanded, self.cron_job.clone());
        let before_highlight = self.selection_highlight_active;
        let before_terminal = self.terminal_size;
        let before_eggpool_period = self.eggpool.as_ref().map(|eggpool| eggpool.period);
        self.apply_action_inner(action);
        before_selected != self.selected_id
            || before_viewport != self.viewport_top_id
            || before_pane != self.active_pane
            || before_view != self.system_view_mode
            || before_drives != self.drives_expanded
            || before_network != self.network_expanded
            || before_cron != (self.cron_expanded, self.cron_job.clone())
            || before_highlight != self.selection_highlight_active
            || before_terminal != self.terminal_size
            || before_eggpool_period != self.eggpool.as_ref().map(|eggpool| eggpool.period)
    }

    #[allow(clippy::match_same_arms, clippy::too_many_lines)]
    fn apply_action_inner(&mut self, action: Action) {
        match action {
            Action::MoveDown => {
                if self.active_pane == Pane::Eggpool {
                    // Plan 164: the period is daemon-owned, so this records a
                    // request instead of mutating local state. The event loop
                    // forwards it and the pane updates when the daemon's
                    // converged period comes back.
                    self.request_eggpool_period(true);
                } else {
                    let order = self.display_order();
                    self.move_selection(&order, 1);
                    self.selection_highlight_active = true;
                    ensure_selected_visible_with_order(self, &order);
                    return;
                }
            }
            Action::MoveUp => {
                if self.active_pane == Pane::Eggpool {
                    self.request_eggpool_period(false);
                } else {
                    let order = self.display_order();
                    self.move_selection(&order, -1_isize);
                    self.selection_highlight_active = true;
                    ensure_selected_visible_with_order(self, &order);
                    return;
                }
            }
            Action::PageDown if self.active_pane == Pane::Systems => {
                let order = self.display_order();
                let page = self.page_size(&order);
                self.move_selection(&order, page);
                self.selection_highlight_active = true;
                ensure_selected_visible_with_order(self, &order);
                return;
            }
            Action::PageUp if self.active_pane == Pane::Systems => {
                let order = self.display_order();
                let page = self.page_size(&order);
                self.move_selection(&order, -page);
                self.selection_highlight_active = true;
                ensure_selected_visible_with_order(self, &order);
                return;
            }
            Action::SelectFirst if self.active_pane == Pane::Systems => {
                let order = self.display_order();
                self.selected_id = order
                    .first()
                    .and_then(|&i| self.systems.get(i).map(|s| &s.id))
                    .cloned();
                self.selection_highlight_active = true;
                ensure_selected_visible_with_order(self, &order);
                return;
            }
            Action::SelectLast if self.active_pane == Pane::Systems => {
                let order = self.display_order();
                self.selected_id = order
                    .last()
                    .and_then(|&i| self.systems.get(i).map(|s| &s.id))
                    .cloned();
                self.selection_highlight_active = true;
                ensure_selected_visible_with_order(self, &order);
                return;
            }
            Action::PreviousPane => self.cycle_pane(false),
            Action::NextPane => self.cycle_pane(true),
            Action::ClearSelectionHighlight => {
                self.selection_highlight_active = false;
            }
            Action::ToggleSystemView if self.active_pane == Pane::Systems => {
                self.system_view_mode = match self.system_view_mode {
                    SystemViewMode::Normal => SystemViewMode::Condensed,
                    SystemViewMode::Condensed => SystemViewMode::Normal,
                };
            }
            // These arms change nothing that affects selection visibility,
            // so skip the viewport fix-up below.
            Action::PageDown
            | Action::PageUp
            | Action::SelectFirst
            | Action::SelectLast
            | Action::RefreshNow
            | Action::Quit => return,
            // Note: `ToggleSystemView` on the Systems pane deliberately
            // falls through because view mode changes entry heights.
            Action::ToggleSystemView
            | Action::ToggleDrives
            | Action::ToggleNetwork
            | Action::ToggleCron
            | Action::CronJobNext
            | Action::CronJobPrevious
                if self.active_pane == Pane::Eggpool =>
            {
                return
            }
            Action::ToggleCron => {
                self.cron_expanded = !self.cron_expanded;
                // Opening the pane always lands on a defined job, so the
                // history block never renders with "selected job: (none)".
                if self.cron_expanded {
                    self.repair_cron_selection();
                }
            }
            Action::CronJobNext => self.step_cron_job(1),
            Action::CronJobPrevious => self.step_cron_job(-1_isize),
            Action::ToggleSystemView => return,
            Action::ToggleDrives => {
                // Unlike `ToggleNetwork`, drive expansion is intentionally
                // unguarded: with no drive details the toggle still flips
                // state but yields zero detail rows, and existing tests lock
                // this behavior (`network_expansion_is_independent...`,
                // `view_controls_wrap...`). Gating on
                // `valid_drive_detail_count > 0` would break those tests,
                // so the availability guard stays network-only.
                self.drives_expanded = !self.drives_expanded;
            }
            Action::ToggleNetwork => {
                let has_network = self
                    .selected_id
                    .as_ref()
                    .and_then(|selected| self.systems.iter().find(|system| &system.id == selected))
                    .filter(|system| system.reachability == Reachability::Online)
                    .and_then(|system| system.latest.as_ref())
                    .and_then(|snapshot| snapshot.network.as_ref())
                    .is_some();
                if has_network {
                    self.network_expanded = !self.network_expanded;
                }
            }
            Action::Resize { width, height } => {
                self.terminal_size = Some((width, height));
            }
        }
        ensure_selected_visible(self);
    }

    fn cycle_pane(&mut self, next: bool) {
        let before = self.active_pane;
        match (
            self.active_pane,
            self.systems.is_empty(),
            self.eggpool.is_some(),
            next,
        ) {
            (Pane::Systems, false, true, _) => self.active_pane = Pane::Eggpool,
            (Pane::Eggpool, _, true, _) if !self.systems.is_empty() => {
                self.active_pane = Pane::Systems;
            }
            _ => {}
        }
        // Plan 087: leaving the Systems pane must drop the visual
        // selection highlight immediately so a stale reversed row
        // does not reappear when the operator comes back. Re-entering
        // Systems does not activate the highlight on its own.
        if before == Pane::Systems && self.active_pane != Pane::Systems {
            self.selection_highlight_active = false;
        }
    }

    /// Move selection by a relative offset in display order.
    fn move_selection(&mut self, order: &[usize], offset: isize) {
        if order.is_empty() {
            self.selected_id = None;
            return;
        }

        let current_pos = self
            .selected_id
            .as_ref()
            .and_then(|sel| order.iter().position(|&i| &self.systems[i].id == sel))
            .unwrap_or(0);

        let len = order.len();
        let magnitude = offset.unsigned_abs();
        let new_pos = if offset >= 0 {
            current_pos.saturating_add(magnitude)
        } else {
            current_pos.saturating_sub(magnitude)
        }
        .min(len - 1);

        self.selected_id = order
            .get(new_pos)
            .and_then(|&i| self.systems.get(i))
            .map(|s| s.id.clone());
    }

    /// Compute the page size (number of systems to skip) based on
    /// terminal height and the current viewport.
    ///
    /// Returns 0 when the top entry exceeds the usable viewport (nothing
    /// fits, so page movement is a no-op); otherwise returns at least one
    /// when entries exist.
    fn page_size(&self, order: &[usize]) -> isize {
        let height = self
            .terminal_size
            .map_or(24, |(_, h)| h)
            .saturating_sub(view_header_height(self.system_view_mode));

        let top_pos = self
            .viewport_top_id
            .as_ref()
            .and_then(|top| order.iter().position(|&i| &self.systems[i].id == top))
            .unwrap_or(0);

        let mut rows = 0_u16;
        let mut count = 0_isize;
        for &idx in order.iter().skip(top_pos) {
            let h = entry_height(self, idx);
            if rows + h > height {
                if count == 0 {
                    return 0;
                }
                break;
            }
            rows += h;
            count += 1;
        }

        count.max(1)
    }
}

#[cfg(test)]
impl AppState {
    /// Apply a poll batch through the real daemon path and adopt the result.
    pub fn apply_batch(&mut self, batch: &PollBatch) {
        let _ = self.apply_batch_changed(batch);
    }

    /// Plan 143: batch application reporting render-visible change.
    pub fn apply_batch_changed(&mut self, batch: &PollBatch) -> bool {
        let was_initialized = self.test_fleet.last_applied_generation != 0;
        let visible = self.test_fleet.apply_batch_changed(batch);
        // Mirrors the daemon: the one-time "never polled" -> "polled"
        // transition is published even when it changed nothing visible, so a
        // frontend's one-time selection placement cannot be deferred onto an
        // unrelated later change.
        if visible || was_initialized != (self.test_fleet.last_applied_generation != 0) {
            self.republish()
        } else {
            false
        }
    }

    /// Apply an owned poll batch through the real daemon path.
    pub fn apply_batch_owned(&mut self, batch: PollBatch) {
        let _ = self.apply_batch_owned_changed(batch);
    }

    /// Owned batch application reporting render-visible change.
    pub fn apply_batch_owned_changed(&mut self, batch: PollBatch) -> bool {
        if self.test_fleet.apply_batch_owned_changed(batch) {
            self.republish()
        } else {
            false
        }
    }

    /// The poll generation the daemon's fleet has accepted.
    #[must_use]
    pub fn last_applied_generation(&self) -> u64 {
        self.test_fleet.last_applied_generation
    }

    /// Force the shadow fleet's accepted poll generation.
    ///
    /// Only reachable from tests, and only to reach the single
    /// `u64::MAX`-to-`1` wrap the reducer has to tolerate.
    pub fn set_last_applied_generation(&mut self, generation: u64) {
        self.test_fleet.last_applied_generation = generation;
    }

    /// Reconcile the configured systems and adopt the result.
    pub fn reconcile_systems(&mut self, config: &Config) {
        self.test_fleet.reconcile_systems(config);
        self.republish();
    }

    /// Record a rejected config reload and adopt the result.
    pub fn set_config_reload_error(&mut self, error: String) {
        self.test_fleet.set_config_reload_error(error);
        self.republish();
    }

    /// Clear the rejected config reload diagnostic and adopt the result.
    pub fn clear_config_reload_error(&mut self) {
        self.test_fleet.clear_config_reload_error();
        self.republish();
    }

    /// Apply an `EggPool` result through the real daemon path.
    pub fn apply_eggpool_result(&mut self, result: &EggpoolResult) {
        let _ = self.apply_eggpool_result_changed(result);
    }

    /// `EggPool` result application reporting render-visible change.
    pub fn apply_eggpool_result_changed(&mut self, result: &EggpoolResult) -> bool {
        if self.test_fleet.apply_eggpool_result_changed(result) {
            self.republish()
        } else {
            false
        }
    }

    /// Perform the daemon half of an `EggPool` intent request.
    ///
    /// A frontend's `j`/`k` on the pane only records a request. The window
    /// actually changes when the daemon's converged document comes back, and
    /// that document is what a test must see — asserting that `j`/`k` mutates
    /// the local period would lock in the ownership this plan removed.
    pub fn daemon_apply_eggpool_request(&mut self) {
        if let Some(period) = self.eggpool_period_request {
            if self.test_fleet.set_eggpool_period(period) {
                self.test_fleet.begin_eggpool_request();
            }
        }
        self.republish();
    }

    /// Mark an `EggPool` activation or manual refresh as a new request.
    pub fn begin_eggpool_request(&mut self) -> Option<(EggpoolPeriod, u64)> {
        let minted = self.test_fleet.begin_eggpool_request();
        if minted.is_some() {
            self.republish();
        }
        minted
    }

    /// Mark the local worker as unavailable and adopt the result.
    pub fn mark_eggpool_worker_unavailable(&mut self) {
        self.test_fleet.mark_eggpool_worker_unavailable();
        self.republish();
    }

    /// Return the current `EggPool` request identity.
    #[must_use]
    pub fn eggpool_request(&self) -> Option<(EggpoolPeriod, u64)> {
        self.test_fleet.eggpool_request()
    }

    /// The desired worker state a single frontend implies.
    ///
    /// With one attached frontend the daemon's reduction over all subscriber
    /// intents *is* this value, so a test can assert the same thing the
    /// daemon would publish without standing up a connection.
    #[must_use]
    pub fn eggpool_desired_state(&self) -> Option<EggpoolDesiredState> {
        self.test_fleet
            .eggpool_desired_state(&single_frontend_intents(self))
    }
}

/// The intent set a lone frontend implies: its own pane and period.
#[cfg(test)]
fn single_frontend_intents(state: &AppState) -> EggpoolIntents {
    let mut intents = EggpoolIntents::default();
    if let Some(eggpool) = state.eggpool.as_ref() {
        intents.set(1, state.active_pane == Pane::Eggpool, eggpool.period);
    }
    intents
}

fn system_from_entry(entry: &crate::config::SystemEntry) -> SystemState {
    SystemState {
        id: entry.id.clone(),
        endpoint: entry.to_endpoint(),
        configured_name: entry.name.clone(),
        reachability: Reachability::Pending,
        latest: None,
        last_success_at: None,
        last_attempt_at: None,
        latency: None,
        offline_reason: None,
    }
}

fn equivalent_endpoint_host(left: &str, right: &str) -> bool {
    match (
        crate::endpoint::normalize_host(left),
        crate::endpoint::normalize_host(right),
    ) {
        (Ok(left), Ok(right)) => left.eq_ignore_ascii_case(&right),
        _ => left.eq_ignore_ascii_case(right),
    }
}

/// Return the full row height for a system entry in the current view.
#[must_use]
pub fn entry_height(state: &AppState, system_index: usize) -> u16 {
    let Some(system) = state.systems.get(system_index) else {
        return 1;
    };
    match (state.system_view_mode, system.reachability) {
        (SystemViewMode::Condensed, _) => {
            if (state.drives_expanded || state.network_expanded || state.cron_expanded)
                && state.selected_id.as_deref() == Some(system.id.as_str())
                && system.reachability == Reachability::Online
            {
                1_u16
                    .saturating_add(if state.drives_expanded {
                        valid_drive_detail_count(system)
                    } else {
                        0
                    })
                    .saturating_add(if state.network_expanded {
                        valid_network_detail_count(system)
                    } else {
                        0
                    })
                    .saturating_add(cron_detail_count(state, system_index))
            } else {
                1
            }
        }
        (SystemViewMode::Normal, Reachability::Pending | Reachability::Offline) => 1,
        (SystemViewMode::Normal, Reachability::Online) => {
            let details =
                if (state.drives_expanded || state.network_expanded || state.cron_expanded)
                    && state.selected_id.as_deref() == Some(system.id.as_str())
                {
                    let drives = if state.drives_expanded {
                        // Same helper as the condensed view: a legal
                        // `drives = None` with `disk_io = Some(..)` still renders
                        // the table heading plus the aggregate I/O total, so
                        // gating on `drives.is_some()` here made the two views
                        // disagree.
                        valid_drive_detail_count(system)
                    } else {
                        0
                    };
                    let network = if state.network_expanded {
                        valid_network_detail_count(system)
                    } else {
                        0
                    };
                    // Cron is additive like the other two: opening it never closes
                    // drive or network details, and vice versa.
                    drives
                        .saturating_add(network)
                        .saturating_add(cron_detail_count(state, system_index))
                } else {
                    0
                };
            normal_base_height_for(system).saturating_add(details)
        }
    }
}

/// Compute which systems in display order are visible given a top
/// index, the system states, and available height.
///
/// Online entries take four metric rows plus a header, or six rows when that
/// system's snapshot exposes network telemetry. Selected-system drive and
/// network detail rows follow; offline and pending entries take one row. A first entry is retained
/// even when its full dynamic height is taller than the viewport so the caller
/// can clip only detail rows while preserving its complete base block.
#[must_use]
pub fn visible_range(
    display_order: &[usize],
    state: &AppState,
    top_index: usize,
    height: u16,
) -> Range<usize> {
    if height == 0 {
        return 0..0;
    }

    let mut rows_used = 0_u16;
    let mut count = 0_usize;

    for &idx in display_order.iter().skip(top_index) {
        if idx >= state.systems.len() {
            break;
        }
        let h = entry_height(state, idx);

        if count == 0 && height < minimum_render_height(state, idx) {
            return top_index..top_index;
        }

        if rows_used + h > height && count > 0 {
            break;
        }
        rows_used += h;
        count += 1;
    }

    top_index..(top_index + count)
}

fn minimum_render_height(state: &AppState, system_index: usize) -> u16 {
    match state
        .systems
        .get(system_index)
        .map(|system| (state.system_view_mode, system.reachability))
    {
        Some((SystemViewMode::Normal, Reachability::Online)) => state
            .systems
            .get(system_index)
            .map_or(1, normal_base_height_for),
        Some(_) => 1,
        None => 0,
    }
}

/// Adjust `viewport_top_id` so the selected system is visible.
pub fn ensure_selected_visible(state: &mut AppState) {
    let order = state.display_order();
    ensure_selected_visible_with_order(state, &order);
}

/// [`ensure_selected_visible`] with a precomputed display order, so
/// callers already holding one avoid rebuilding it.
///
/// The below-viewport walk calls O(n) `visible_range` per step (O(n²)
/// worst-case far below the viewport). With n≈100 this is negligible
/// and keeps minimal-scroll obvious; a single-pass walk would save
/// nothing measurable.
fn ensure_selected_visible_with_order(state: &mut AppState, order: &[usize]) {
    if order.is_empty() {
        return;
    }

    let (_, height) = state.terminal_size.unwrap_or((80, 24));

    let selected_pos = state
        .selected_id
        .as_ref()
        .and_then(|sel| order.iter().position(|&i| &state.systems[i].id == sel));

    let top_pos = state
        .viewport_top_id
        .as_ref()
        .and_then(|top| order.iter().position(|&i| &state.systems[i].id == top))
        .unwrap_or(0);

    let Some(selected_pos) = selected_pos else {
        return;
    };

    // The renderer uses the complete frame as its viewport.
    let usable_height = height.saturating_sub(view_header_height(state.system_view_mode));

    // Find which systems fit from top_pos downward.
    let visible = visible_range(order, state, top_pos, usable_height);

    if visible.contains(&selected_pos) {
        // Already visible, nothing to do.
        return;
    }

    // If selected is above viewport, scroll up.
    if selected_pos < top_pos {
        state.viewport_top_id = Some(state.systems[order[selected_pos]].id.clone());
        return;
    }

    // If selected is below viewport, move the top only as far as necessary.
    if selected_pos >= top_pos {
        let mut candidate = selected_pos;
        while candidate > top_pos {
            let previous = candidate - 1;
            let range = visible_range(order, state, previous, usable_height);
            if range.contains(&selected_pos) {
                candidate = previous;
            } else {
                break;
            }
        }
        state.viewport_top_id = Some(state.systems[order[candidate]].id.clone());
    }
}

/// Rows reserved above the entries by a view.
#[must_use]
pub const fn view_header_height(system_view_mode: SystemViewMode) -> u16 {
    match system_view_mode {
        SystemViewMode::Normal => 0,
        SystemViewMode::Condensed => 2,
    }
}

fn valid_drive_count(system: &SystemState) -> u16 {
    system
        .latest
        .as_ref()
        .and_then(|snapshot| snapshot.drives.as_deref())
        .map_or(0, valid_drive_count_from_slice)
}

/// Number of rendered drive-detail lines for one selected system.
#[must_use]
pub fn entry_detail_drive_count(state: &AppState, system_index: usize) -> usize {
    if !state.drives_expanded {
        return 0;
    }
    state
        .systems
        .get(system_index)
        .map_or(0, |system| usize::from(valid_drive_detail_count(system)))
}

/// Number of rendered network-detail lines for one selected system.
#[must_use]
pub fn entry_detail_network_count(state: &AppState, system_index: usize) -> usize {
    if !state.network_expanded {
        return 0;
    }
    state
        .systems
        .get(system_index)
        .map_or(0, |system| usize::from(valid_network_detail_count(system)))
}

/// Number of rendered cron rows for the selected system.
///
/// Zero when the cron block is closed, so the height accounting only grows for
/// a pane the operator actually opened. Bounded by the caller's remaining
/// budget in [`crate::ui::layout`], exactly like the drive and network counts.
#[must_use]
pub fn entry_detail_cron_count(state: &AppState, system_index: usize) -> usize {
    if !state.cron_expanded
        || state.selected_id.as_deref().is_none_or(|selected| {
            state
                .systems
                .get(system_index)
                .is_none_or(|system| system.id != selected)
        })
    {
        return 0;
    }
    crate::ui::cron::desired_rows(state)
}

/// Rows the cron block contributes, as a `u16` for the height arithmetic.
fn cron_detail_count(state: &AppState, system_index: usize) -> u16 {
    u16::try_from(entry_detail_cron_count(state, system_index)).unwrap_or(u16::MAX)
}

fn valid_drive_count_from_slice(drives: &[crate::normalized::NormalizedDrive]) -> u16 {
    drives
        .iter()
        .filter(|drive| drive.total_bytes > 0 && drive.used_bytes <= drive.total_bytes)
        .count()
        .try_into()
        .unwrap_or(u16::MAX)
}

fn valid_drive_detail_count(system: &SystemState) -> u16 {
    let rows = valid_drive_count(system);
    if system
        .latest
        .as_ref()
        .and_then(|snapshot| snapshot.disk_io.as_ref())
        .is_some()
    {
        rows.saturating_add(2) // table heading + aggregate I/O total
    } else {
        rows
    }
}

/// Number of rows in one online normal-view block before optional details.
#[must_use]
pub fn normal_base_height_for(system: &SystemState) -> u16 {
    if system
        .latest
        .as_ref()
        .is_some_and(|snapshot| snapshot.network.is_some())
    {
        6
    } else {
        5
    }
}

fn valid_network_detail_count(system: &SystemState) -> u16 {
    system.latest.as_ref().map_or(0, |snapshot| {
        snapshot.network.as_ref().map_or(0, |network| {
            1_u16.saturating_add(network.interfaces.len().try_into().unwrap_or(u16::MAX))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EggpoolEntry, EggpoolScheme, SystemEntry};
    use gregg_protocol::test_support::LinuxSnapshotBuilder;
    use gregg_protocol::StatusSnapshot;

    fn test_config_with_ids(ids: &[&str]) -> Config {
        let mut config = Config::default();
        for (i, id) in ids.iter().enumerate() {
            config.systems.push(SystemEntry {
                id: (*id).to_string(),
                host: format!("host{i}.local"),
                port: 11310 + u16::try_from(i).unwrap(),
                name: Some(format!("System {i}")),
            });
        }
        config
    }

    fn eggpool_config(with_system: bool) -> Config {
        let mut config = if with_system {
            test_config_with_ids(&["system"])
        } else {
            Config::default()
        };
        config.eggpool = Some(EggpoolEntry {
            id: "eggpool-id".into(),
            host: "pool.local".into(),
            port: 11300,
            scheme: EggpoolScheme::Http,
            name: Some("Main EggPool".into()),
            api_key_env: None,
        });
        config
    }

    fn make_snapshot() -> StatusSnapshot {
        LinuxSnapshotBuilder::default().build()
    }

    fn batch_for_indices(state: &AppState, indices: impl IntoIterator<Item = usize>) -> PollBatch {
        let now = Instant::now();
        PollBatch {
            generation: 1,
            started_at: now,
            completed_at: now,
            results: indices
                .into_iter()
                .map(|index| {
                    let system = &state.systems[index];
                    crate::poller::PollResult {
                        system_id: system.id.clone(),
                        endpoint: system.endpoint.clone(),
                        outcome: PollOutcome::Online(Box::new(make_snapshot())),
                        latency: Duration::from_millis(1),
                    }
                })
                .collect(),
        }
    }

    #[test]
    fn owned_ordered_and_reordered_batches_use_safe_fast_path() {
        let ids: Vec<String> = (0..500).map(|index| format!("system-{index}")).collect();
        let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();

        let config = test_config_with_ids(&id_refs);
        let mut ordered = AppState::synthetic(&config);
        ordered.apply_batch_owned(batch_for_indices(&ordered, 0..ordered.systems.len()));
        assert!(ordered
            .systems
            .iter()
            .all(|system| system.reachability == Reachability::Online));

        let mut reordered = AppState::synthetic(&config);
        reordered.apply_batch_owned(batch_for_indices(
            &reordered,
            (0..reordered.systems.len()).rev(),
        ));
        assert!(reordered
            .systems
            .iter()
            .all(|system| system.reachability == Reachability::Online));
    }

    #[test]
    fn from_config_creates_correct_initial_state() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let state = AppState::synthetic(&config);

        assert_eq!(state.systems.len(), 3);
        assert_eq!(state.selected_id.as_deref(), Some("a"));
        assert_eq!(state.viewport_top_id.as_deref(), Some("a"));
        assert_eq!(state.last_applied_generation(), 0);
        assert_eq!(state.refresh_status, RefreshStatus::Idle);
        assert!(state.terminal_size.is_none());

        for system in &state.systems {
            assert_eq!(system.reachability, Reachability::Pending);
            assert!(system.latest.is_none());
        }
    }

    #[test]
    fn from_config_preserves_configured_endpoint_host_exactly() {
        let mut config = Config::default();
        config.systems.push(SystemEntry {
            id: "exact".into(),
            host: "192.168.183.143".into(),
            port: 11310,
            name: None,
        });

        let state = AppState::synthetic(&config);
        assert_eq!(state.systems[0].endpoint.host, "192.168.183.143");
    }

    #[test]
    fn reconcile_systems_replaces_targets_preserves_unchanged_state_and_repairs_ids() {
        let old_config = Config {
            systems: vec![
                SystemEntry {
                    id: "changed".into(),
                    host: "192.168.182.143".into(),
                    port: 11310,
                    name: Some("Old".into()),
                },
                SystemEntry {
                    id: "same".into(),
                    host: "same.local".into(),
                    port: 11311,
                    name: Some("Same".into()),
                },
                SystemEntry {
                    id: "removed".into(),
                    host: "removed.local".into(),
                    port: 11312,
                    name: None,
                },
            ],
            ..Config::default()
        };
        let mut state = AppState::synthetic(&old_config);
        state.selected_id = Some("removed".into());

        let first_batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: state
                .systems
                .iter()
                .take(2)
                .map(|system| crate::poller::PollResult {
                    system_id: system.id.clone(),
                    endpoint: system.endpoint.clone(),
                    outcome: PollOutcome::Online(Box::new(make_snapshot())),
                    latency: Duration::from_millis(25),
                })
                .collect(),
        };
        state.apply_batch(&first_batch);
        state.systems[0].offline_reason = Some(crate::poller::OfflineReason::new(
            crate::poller::OfflineKind::Timeout,
        ));

        let retained_snapshot = state.systems[1].latest.clone();
        let retained_success = state.systems[1].last_success_at;
        let new_config = Config {
            systems: vec![
                SystemEntry {
                    id: "changed".into(),
                    host: "192.168.183.143".into(),
                    port: 11310,
                    name: Some("New".into()),
                },
                SystemEntry {
                    id: "same".into(),
                    host: "same.local".into(),
                    port: 11311,
                    name: Some("Renamed".into()),
                },
                SystemEntry {
                    id: "added".into(),
                    host: "added.local".into(),
                    port: 11313,
                    name: None,
                },
            ],
            ..old_config.clone()
        };

        state.reconcile_systems(&new_config);

        assert_eq!(state.systems.len(), 3);
        assert_eq!(state.systems[0].endpoint.host, "192.168.183.143");
        assert_eq!(state.systems[0].configured_name.as_deref(), Some("New"));
        assert_eq!(state.systems[0].reachability, Reachability::Pending);
        assert!(state.systems[0].latest.is_none());
        assert!(state.systems[0].last_success_at.is_none());
        assert!(state.systems[0].last_attempt_at.is_none());
        assert!(state.systems[0].latency.is_none());
        assert!(state.systems[0].offline_reason.is_none());

        assert_eq!(state.systems[1].configured_name.as_deref(), Some("Renamed"));
        assert_eq!(state.systems[1].reachability, Reachability::Online);
        assert_eq!(state.systems[1].latest, retained_snapshot);
        // Timestamps cross the process boundary as Unix milliseconds, so a
        // round trip is accurate to the millisecond and no finer. The retained
        // age is preserved; only the sub-millisecond part is transport loss,
        // which is why the comparison is a tolerance and not equality.
        let retained_age = retained_success
            .map(|then| then.elapsed())
            .expect("the retained system had a success timestamp");
        let observed_age = state.systems[1]
            .last_success_at
            .map(|then| then.elapsed())
            .expect("the retained system kept its success timestamp");
        assert!(
            retained_age.abs_diff(observed_age) <= Duration::from_millis(1),
            "retained success age moved from {retained_age:?} to {observed_age:?}",
        );
        assert_eq!(state.selected_id.as_deref(), Some("changed"));
        assert_eq!(state.viewport_top_id.as_deref(), Some("changed"));
        assert_eq!(state.systems[2].id, "added");
        assert_eq!(state.systems[2].reachability, Reachability::Pending);
    }

    #[test]
    fn reconcile_systems_preserves_state_when_dns_host_case_changes() {
        let config = Config {
            systems: vec![SystemEntry {
                id: "same".into(),
                host: "Server.Local".into(),
                port: 11310,
                name: None,
            }],
            ..Config::default()
        };
        let mut state = AppState::synthetic(&config);
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "same".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(10),
            }],
        });

        state.reconcile_systems(&Config {
            systems: vec![SystemEntry {
                id: "same".into(),
                host: "server.local".into(),
                port: 11310,
                name: None,
            }],
            ..Config::default()
        });

        assert_eq!(state.systems[0].reachability, Reachability::Online);
        assert!(state.systems[0].latest.is_some());
    }

    #[test]
    fn reconcile_systems_preserves_state_when_ipv6_spellings_change() {
        let old_config = Config {
            systems: vec![SystemEntry {
                id: "same".into(),
                host: "fd00::1".into(),
                port: 11310,
                name: None,
            }],
            ..Config::default()
        };
        let mut state = AppState::synthetic(&old_config);
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "same".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(10),
            }],
        });

        state.reconcile_systems(&Config {
            systems: vec![SystemEntry {
                id: "same".into(),
                host: "fd00:0000:0000:0000:0000:0000:0000:0001".into(),
                port: 11310,
                name: None,
            }],
            ..Config::default()
        });

        assert_eq!(state.systems[0].reachability, Reachability::Online);
        assert!(state.systems[0].latest.is_some());
    }

    #[test]
    fn apply_batch_accepts_case_only_endpoint_changes() {
        let config = Config {
            systems: vec![SystemEntry {
                id: "same".into(),
                host: "server.local".into(),
                port: 11310,
                name: None,
            }],
            ..Config::default()
        };
        let mut state = AppState::synthetic(&config);
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "same".into(),
                endpoint: Endpoint::new("SERVER.LOCAL".into(), 11310, None),
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(10),
            }],
        });

        assert_eq!(state.systems[0].reachability, Reachability::Online);
        assert!(state.systems[0].latest.is_some());
    }

    #[test]
    fn apply_batch_rejects_result_from_superseded_endpoint() {
        let mut config = test_config_with_ids(&["a"]);
        config.systems[0].host = "new.local".into();
        let mut state = AppState::synthetic(&config);
        let old_endpoint = Endpoint::new("old.local".into(), 11310, None);
        state.systems[0].endpoint = old_endpoint.clone();
        state.reconcile_systems(&config);

        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: old_endpoint,
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(1),
            }],
        });

        assert_eq!(state.systems[0].endpoint.host, "new.local");
        assert_eq!(state.systems[0].reachability, Reachability::Pending);
        assert!(state.systems[0].latest.is_none());
    }

    #[test]
    fn apply_batch_accepts_the_single_generation_wrap_after_max() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);
        state.set_last_applied_generation(u64::MAX);
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: Vec::new(),
        });
        assert_eq!(state.last_applied_generation(), 1);
    }

    #[test]
    fn apply_batch_rejects_a_skipped_generation_wrap() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);
        state.set_last_applied_generation(u64::MAX - 1);
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: Vec::new(),
        });
        assert_eq!(state.last_applied_generation(), u64::MAX - 1);
    }

    #[test]
    fn from_config_empty_systems() {
        let config = Config::default();
        let state = AppState::synthetic(&config);

        assert!(state.systems.is_empty());
        assert!(state.selected_id.is_none());
        assert!(state.viewport_top_id.is_none());
    }

    #[test]
    fn pane_initialization_and_cycling_follow_configured_sources() {
        let systems = AppState::synthetic(&test_config_with_ids(&["a"]));
        assert_eq!(systems.active_pane, Pane::Systems);
        let eggpool = AppState::synthetic(&eggpool_config(false));
        assert_eq!(eggpool.active_pane, Pane::Eggpool);
        assert!(eggpool.eggpool.is_some());

        let mut both = AppState::synthetic(&eggpool_config(true));
        both.apply_action(Action::NextPane);
        assert_eq!(both.active_pane, Pane::Eggpool);
        both.apply_action(Action::PreviousPane);
        assert_eq!(both.active_pane, Pane::Systems);
    }

    #[test]
    fn eggpool_period_movement_is_bounded_and_invalidates_old_summary() {
        let mut state = AppState::synthetic(&eggpool_config(false));
        assert_eq!(state.eggpool.as_ref().unwrap().period, EggpoolPeriod::Hour);

        // The shortest window cannot move, so there is nothing to ask for.
        state.apply_action(Action::MoveUp);
        assert_eq!(state.eggpool_period_request, None);
        state.daemon_apply_eggpool_request();
        assert_eq!(state.eggpool.as_ref().unwrap().period, EggpoolPeriod::Hour);

        // A longer window is a *request*. The displayed window only changes
        // when the daemon's converged document arrives, so a TUI can never
        // claim a period whose numbers were fetched for a different one.
        state.apply_action(Action::MoveDown);
        assert_eq!(state.eggpool_period_request, Some(EggpoolPeriod::Day));
        assert_eq!(state.eggpool.as_ref().unwrap().period, EggpoolPeriod::Hour);
        state.daemon_apply_eggpool_request();
        assert_eq!(state.eggpool.as_ref().unwrap().period, EggpoolPeriod::Day);
        // The confirmed request is cleared, so an unrelated adopt does not
        // replay it.
        assert_eq!(state.eggpool_period_request, None);

        for _ in 0..3 {
            state.apply_action(Action::MoveDown);
            state.daemon_apply_eggpool_request();
        }
        let eggpool = state.eggpool.as_ref().unwrap();
        assert_eq!(eggpool.period, EggpoolPeriod::Month);
        assert_eq!(eggpool.request_generation, 3);
        assert!(eggpool.summary.is_none());
    }

    #[test]
    fn eggpool_results_reject_stale_or_mismatched_requests_and_retain_same_period_failures() {
        let mut state = AppState::synthetic(&eggpool_config(false));
        // Move to the day window and let the daemon converge on it, so the
        // mismatched-period rejection below is about a *stale* period rather
        // than about a request that was never answered.
        state.apply_action(Action::MoveDown);
        state.daemon_apply_eggpool_request();
        let generation = state.eggpool_request().unwrap().1;
        assert_eq!(state.eggpool.as_ref().unwrap().period, EggpoolPeriod::Day);
        let now = Instant::now();
        let summary = EggpoolSummary {
            accounted_tokens: 42,
            cache_read_ratio: Some(0.5),
            output_tokens_per_second: 2.0,
            avg_ttft_ms: Some(12.0),
            period: EggpoolPeriod::Day,
        };
        let result = |generation, period, summary| EggpoolResult {
            generation,
            period,
            started_at: now,
            completed_at: now,
            summary,
            health: EggpoolHealthFetchOutcome::Unsupported,
        };
        state.apply_eggpool_result(&result(
            0,
            EggpoolPeriod::Day,
            EggpoolFetchOutcome::Online(summary.clone()),
        ));
        assert!(state.eggpool.as_ref().unwrap().summary.is_none());
        state.apply_eggpool_result(&result(
            generation,
            EggpoolPeriod::Hour,
            EggpoolFetchOutcome::Online(summary.clone()),
        ));
        assert!(state.eggpool.as_ref().unwrap().summary.is_none());
        state.apply_eggpool_result(&result(
            generation,
            EggpoolPeriod::Day,
            EggpoolFetchOutcome::Online(summary),
        ));
        assert!(state.eggpool.as_ref().unwrap().summary.is_some());
        state.apply_eggpool_result(&result(
            generation,
            EggpoolPeriod::Day,
            EggpoolFetchOutcome::Timeout,
        ));
        assert!(state.eggpool.as_ref().unwrap().summary.is_some());
        assert!(matches!(
            state.eggpool.as_ref().unwrap().last_error,
            Some(EggpoolFetchOutcome::Timeout)
        ));
        state.apply_eggpool_result(&result(
            generation,
            EggpoolPeriod::Day,
            EggpoolFetchOutcome::Online(EggpoolSummary {
                accounted_tokens: 43,
                cache_read_ratio: None,
                output_tokens_per_second: 3.0,
                avg_ttft_ms: None,
                period: EggpoolPeriod::Day,
            }),
        ));
        assert_eq!(
            state
                .eggpool
                .as_ref()
                .unwrap()
                .summary
                .as_ref()
                .unwrap()
                .accounted_tokens,
            43
        );
    }

    fn health_snapshot(proxy: crate::eggpool::EggpoolProxyHealth) -> EggpoolHealthSnapshot {
        EggpoolHealthSnapshot {
            schema_version: 1,
            proxy,
            available: true,
            reason_code: None,
            uptime_seconds: Some(10.0),
            model_count: Some(2),
            routable_accounts: Some(1),
            enabled_accounts: Some(1),
            providers: vec![crate::eggpool::EggpoolProviderRow {
                id: "openai".into(),
                status: crate::eggpool::EggpoolProviderHealth::Ready,
                observation: Some(crate::eggpool::EggpoolProviderObservation::Verified),
            }],
        }
    }

    #[test]
    fn eggpool_summary_and_health_planes_are_applied_independently() {
        let mut state = AppState::synthetic(&eggpool_config(false));
        let generation = state.begin_eggpool_request().unwrap().1;
        let now = Instant::now();
        let result = |summary, health| EggpoolResult {
            generation,
            period: EggpoolPeriod::Hour,
            started_at: now,
            completed_at: now,
            summary,
            health,
        };

        // A good health snapshot is retained even when the summary failed.
        state.apply_eggpool_result(&result(
            EggpoolFetchOutcome::Timeout,
            EggpoolHealthFetchOutcome::Online(health_snapshot(
                crate::eggpool::EggpoolProxyHealth::Degraded,
            )),
        ));
        let eggpool = state.eggpool.as_ref().unwrap();
        assert!(eggpool.summary.is_none());
        assert!(matches!(
            eggpool.last_error,
            Some(EggpoolFetchOutcome::Timeout)
        ));
        assert_eq!(
            eggpool.health.as_ref().unwrap().proxy,
            crate::eggpool::EggpoolProxyHealth::Degraded
        );
        assert!(eggpool.last_health_error.is_none());
        assert!(eggpool.last_health_success_at.is_some());

        // A failed health refresh keeps the previous snapshot visible and
        // records that it is no longer current.
        state.apply_eggpool_result(&result(
            EggpoolFetchOutcome::Online(EggpoolSummary {
                accounted_tokens: 7,
                cache_read_ratio: None,
                output_tokens_per_second: 1.0,
                avg_ttft_ms: None,
                period: EggpoolPeriod::Hour,
            }),
            EggpoolHealthFetchOutcome::ConnectionRefused,
        ));
        let eggpool = state.eggpool.as_ref().unwrap();
        assert_eq!(eggpool.summary.as_ref().unwrap().accounted_tokens, 7);
        assert!(eggpool.last_error.is_none());
        assert_eq!(
            eggpool.health.as_ref().unwrap().proxy,
            crate::eggpool::EggpoolProxyHealth::Degraded
        );
        assert!(matches!(
            eggpool.last_health_error,
            Some(EggpoolHealthFetchOutcome::ConnectionRefused)
        ));
        assert!(eggpool.last_health_attempt_at.is_some());

        // A successful health refresh replaces it and clears the error.
        state.apply_eggpool_result(&result(
            EggpoolFetchOutcome::Online(EggpoolSummary {
                accounted_tokens: 8,
                cache_read_ratio: None,
                output_tokens_per_second: 1.0,
                avg_ttft_ms: None,
                period: EggpoolPeriod::Hour,
            }),
            EggpoolHealthFetchOutcome::Online(health_snapshot(
                crate::eggpool::EggpoolProxyHealth::Ready,
            )),
        ));
        let eggpool = state.eggpool.as_ref().unwrap();
        assert_eq!(
            eggpool.health.as_ref().unwrap().proxy,
            crate::eggpool::EggpoolProxyHealth::Ready
        );
        assert!(eggpool.last_health_error.is_none());
    }

    #[test]
    fn eggpool_period_change_keeps_health_and_rejects_another_periods_summary() {
        let mut state = AppState::synthetic(&eggpool_config(false));
        let now = Instant::now();
        let generation = state.eggpool.as_ref().unwrap().request_generation;
        let result = |period, summary| EggpoolResult {
            generation,
            period,
            started_at: now,
            completed_at: now,
            summary,
            health: EggpoolHealthFetchOutcome::Online(health_snapshot(
                crate::eggpool::EggpoolProxyHealth::Ready,
            )),
        };
        state.apply_eggpool_result(&result(
            EggpoolPeriod::Hour,
            EggpoolFetchOutcome::Online(EggpoolSummary {
                accounted_tokens: 1,
                cache_read_ratio: None,
                output_tokens_per_second: 1.0,
                avg_ttft_ms: None,
                period: EggpoolPeriod::Hour,
            }),
        ));
        assert!(state.eggpool.as_ref().unwrap().health.is_some());

        // A period move is a summary-plane change only.
        state.apply_action(crate::action::Action::MoveDown);
        state.daemon_apply_eggpool_request();
        assert_eq!(state.eggpool.as_ref().unwrap().period, EggpoolPeriod::Day);
        assert!(
            state.eggpool.as_ref().unwrap().health.is_some(),
            "service health has no period"
        );
        let other_period = result(
            EggpoolPeriod::Hour,
            EggpoolFetchOutcome::Online(EggpoolSummary {
                accounted_tokens: 99,
                cache_read_ratio: None,
                output_tokens_per_second: 1.0,
                avg_ttft_ms: None,
                period: EggpoolPeriod::Hour,
            }),
        );
        assert!(!state.apply_eggpool_result_changed(&other_period));
        assert!(state.eggpool.as_ref().unwrap().summary.is_none());
    }

    #[test]
    fn apply_batch_online_result() {
        let config = test_config_with_ids(&["a", "b"]);
        let mut state = AppState::synthetic(&config);
        let snap = make_snapshot();

        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(snap.clone())),
                latency: Duration::from_millis(50),
            }],
        };

        state.apply_batch(&batch);

        assert_eq!(state.systems[0].reachability, Reachability::Online);
        assert!(state.systems[0].latest.is_some());
        assert!(state.systems[0].last_success_at.is_some());
        assert!(state.systems[0].latency.is_some());
        assert!(state.systems[0].offline_reason.is_none());
        assert_eq!(state.last_applied_generation(), 1);
        // System b is still pending.
        assert_eq!(state.systems[1].reachability, Reachability::Pending);
    }

    #[test]
    fn apply_batch_offline_result() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);

        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::ConnectionRefused,
                latency: Duration::from_millis(10),
            }],
        };

        state.apply_batch(&batch);

        assert_eq!(state.systems[0].reachability, Reachability::Offline);
        assert!(state.systems[0].latest.is_none());
        assert!(state.systems[0].last_attempt_at.is_some());
        assert_eq!(
            state.systems[0]
                .offline_reason
                .as_ref()
                .map(|reason| reason.kind),
            Some(crate::poller::OfflineKind::Refused)
        );
    }

    #[test]
    fn apply_batch_rejects_old_generation() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);

        let batch = PollBatch {
            generation: 2,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(50),
            }],
        };

        state.apply_batch(&batch);
        assert_eq!(state.last_applied_generation(), 2);

        // Older batch should be rejected.
        let old_batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::ConnectionRefused,
                latency: Duration::from_millis(10),
            }],
        };

        state.apply_batch(&old_batch);
        // Generation should not have changed back.
        assert_eq!(state.last_applied_generation(), 2);
        // Reachability should still be Online.
        assert_eq!(state.systems[0].reachability, Reachability::Online);
        // A stale failure must not plant provenance over newer online state.
        assert!(state.systems[0].offline_reason.is_none());
    }

    #[test]
    fn apply_batch_success_clears_offline_reason() {
        use crate::poller::{OfflineKind, OfflineReason};
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);
        state.systems[0].reachability = Reachability::Offline;
        state.systems[0].offline_reason = Some(OfflineReason::new(OfflineKind::Timeout));

        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(10),
            }],
        };
        state.apply_batch(&batch);

        // Recovery in the same accepted generation clears stale provenance.
        assert_eq!(state.systems[0].reachability, Reachability::Online);
        assert!(state.systems[0].offline_reason.is_none());
    }

    #[test]
    fn apply_batch_newer_failure_replaces_reason() {
        use crate::poller::OfflineKind;
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);

        for (generation, outcome, kind) in [
            (1, PollOutcome::Timeout, OfflineKind::Timeout),
            (2, PollOutcome::DnsFailure, OfflineKind::Dns),
            (3, PollOutcome::HttpStatus(503), OfflineKind::Http),
        ] {
            let batch = PollBatch {
                generation,
                started_at: Instant::now(),
                completed_at: Instant::now(),
                results: vec![crate::poller::PollResult {
                    system_id: "a".into(),
                    endpoint: state.systems[0].endpoint.clone(),
                    outcome,
                    latency: Duration::from_millis(10),
                }],
            };
            state.apply_batch(&batch);
            assert_eq!(state.systems[0].reachability, Reachability::Offline);
            assert_eq!(
                state.systems[0].offline_reason.as_ref().map(|r| r.kind),
                Some(kind)
            );
        }
        assert_eq!(
            state.systems[0]
                .offline_reason
                .as_ref()
                .and_then(|r| r.detail.clone()),
            Some("HTTP 503".to_string())
        );
    }

    #[test]
    fn apply_batch_cancelled_no_state_change() {
        use crate::poller::OfflineKind;
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);

        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::Cancelled,
                latency: Duration::from_millis(50),
            }],
        };

        state.apply_batch(&batch);

        // A cancelled poll (scheduler panic) records an attempt like any
        // other offline outcome instead of leaving a stale timestamp.
        assert_eq!(state.systems[0].reachability, Reachability::Offline);
        assert!(state.systems[0].last_attempt_at.is_some());
        assert_eq!(
            state.systems[0].offline_reason.as_ref().map(|r| r.kind),
            Some(OfflineKind::Cancelled)
        );
    }

    #[test]
    fn display_order_online_first() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::synthetic(&config);

        // Make b online.
        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "b".into(),
                endpoint: state.systems[1].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(50),
            }],
        };
        state.apply_batch(&batch);

        let order = state.display_order();
        // b is online, should be first. a and c are pending, should follow.
        assert_eq!(order.len(), 3);
        assert_eq!(state.systems[order[0]].id, "b");
        // a and c should maintain configured order.
        let remaining: Vec<&str> = order[1..]
            .iter()
            .map(|&i| state.systems[i].id.as_str())
            .collect();
        assert_eq!(remaining, vec!["a", "c"]);
    }

    #[test]
    fn display_order_preserves_configured_order() {
        let config = test_config_with_ids(&["a", "b", "c", "d"]);
        let mut state = AppState::synthetic(&config);

        // Make c and a online.
        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![
                crate::poller::PollResult {
                    system_id: "c".into(),
                    endpoint: state.systems[2].endpoint.clone(),
                    outcome: PollOutcome::Online(Box::new(make_snapshot())),
                    latency: Duration::from_millis(50),
                },
                crate::poller::PollResult {
                    system_id: "a".into(),
                    endpoint: state.systems[0].endpoint.clone(),
                    outcome: PollOutcome::Online(Box::new(make_snapshot())),
                    latency: Duration::from_millis(50),
                },
            ],
        };
        state.apply_batch(&batch);

        let order = state.display_order();
        // Online: a (index 0), c (index 2) in configured order.
        assert_eq!(state.systems[order[0]].id, "a");
        assert_eq!(state.systems[order[1]].id, "c");
        // Offline: b, d in configured order.
        assert_eq!(state.systems[order[2]].id, "b");
        assert_eq!(state.systems[order[3]].id, "d");
    }

    #[test]
    fn select_next_moves_forward() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::synthetic(&config);

        assert_eq!(state.selected_id.as_deref(), Some("a"));

        state.apply_action(Action::MoveDown);
        assert_eq!(state.selected_id.as_deref(), Some("b"));

        state.apply_action(Action::MoveDown);
        assert_eq!(state.selected_id.as_deref(), Some("c"));

        // Should clamp at the end.
        state.apply_action(Action::MoveDown);
        assert_eq!(state.selected_id.as_deref(), Some("c"));
    }

    #[test]
    fn select_previous_moves_backward() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::synthetic(&config);

        state.apply_action(Action::MoveDown);
        state.apply_action(Action::MoveDown);
        assert_eq!(state.selected_id.as_deref(), Some("c"));

        state.apply_action(Action::MoveUp);
        assert_eq!(state.selected_id.as_deref(), Some("b"));

        state.apply_action(Action::MoveUp);
        assert_eq!(state.selected_id.as_deref(), Some("a"));

        // Should clamp at the beginning.
        state.apply_action(Action::MoveUp);
        assert_eq!(state.selected_id.as_deref(), Some("a"));
    }

    #[test]
    fn select_first_and_last() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::synthetic(&config);

        state.apply_action(Action::SelectLast);
        assert_eq!(state.selected_id.as_deref(), Some("c"));

        state.apply_action(Action::SelectFirst);
        assert_eq!(state.selected_id.as_deref(), Some("a"));
    }

    #[test]
    fn page_down_and_up() {
        let config = test_config_with_ids(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        let mut state = AppState::synthetic(&config);
        state.terminal_size = Some((80, 20));

        state.apply_action(Action::PageDown);
        // Page size should be > 1, so selection should move.
        let after_page_down = state.selected_id.clone();
        assert_ne!(after_page_down.as_deref(), Some("a"));

        state.apply_action(Action::PageUp);
        // Should move back toward the beginning.
        let after_page_up = state.selected_id.clone();
        assert_eq!(after_page_up.as_deref(), Some("a"));
    }

    #[test]
    fn page_movement_is_noop_when_no_entry_fits() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::synthetic(&config);
        state.terminal_size = Some((80, 0));

        state.apply_action(Action::PageDown);
        assert_eq!(state.selected_id.as_deref(), Some("a"));

        state.apply_action(Action::PageUp);
        assert_eq!(state.selected_id.as_deref(), Some("a"));
    }

    #[test]
    fn move_selection_tolerates_isize_min_offset() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::synthetic(&config);
        let order = state.display_order();

        // Negating `isize::MIN` would overflow; the helper must clamp
        // rather than panic.
        state.move_selection(&order, isize::MIN);
        assert_eq!(state.selected_id.as_deref(), Some("a"));

        state.move_selection(&order, isize::MAX);
        assert_eq!(state.selected_id.as_deref(), Some("c"));
    }

    #[test]
    fn selection_preserved_across_reorder() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::synthetic(&config);

        // Move past the launch-initialization phase so later batches
        // observe ordinary selection/scroll semantics, matching the
        // production flow (per Phase 083 the *first* accepted batch
        // pins selection and viewport to display-order position zero).
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![],
        });
        assert_eq!(state.last_applied_generation(), 1);

        // Select b.
        state.apply_action(Action::MoveDown);
        assert_eq!(state.selected_id.as_deref(), Some("b"));

        // Make a online (changes display order but b is still selected).
        let batch = PollBatch {
            generation: 2,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: state.systems[0].endpoint.clone(),
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(50),
            }],
        };
        state.apply_batch(&batch);

        assert_eq!(state.selected_id.as_deref(), Some("b"));
    }

    #[test]
    fn first_batch_snaps_selection_and_viewport_to_display_order_top() {
        // Phase 083: a fresh launch must place selection and viewport
        // at display-order position zero after the first accepted
        // batch — an offline first-configured system must not pull the
        // viewport below later online systems.
        let config = test_config_with_ids(&["offline0", "online2", "offline1"]);
        let mut state = AppState::synthetic(&config);
        // Operator already scrolled before the first poll arrives.
        state.apply_action(Action::SelectLast);
        assert_eq!(state.selected_id.as_deref(), Some("offline1"));

        let endpoints: Vec<_> = state
            .systems
            .iter()
            .map(|system| system.endpoint.clone())
            .collect();
        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: endpoints
                .iter()
                .enumerate()
                .map(|(idx, endpoint)| {
                    let outcome = if idx == 1 {
                        PollOutcome::Online(Box::new(make_snapshot()))
                    } else {
                        PollOutcome::ConnectionRefused
                    };
                    crate::poller::PollResult {
                        system_id: state.systems[idx].id.clone(),
                        endpoint: endpoint.clone(),
                        outcome,
                        latency: Duration::from_millis(50),
                    }
                })
                .collect(),
        };
        state.apply_batch(&batch);

        let order = state.display_order();
        let first_id = &state.systems[order[0]].id;
        assert_eq!(state.selected_id.as_deref(), Some(first_id.as_str()));
        assert_eq!(state.viewport_top_id.as_deref(), Some(first_id.as_str()));
        // online2 is the only online system, must be at the top.
        assert_eq!(first_id, "online2");
    }

    #[test]
    fn subsequent_batches_do_not_reset_selection_to_top() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::synthetic(&config);
        // First batch initializes the session.
        state.apply_batch(&PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![],
        });
        // Pick something offline on purpose to verify reachability does
        // not drive the second-batch reset.
        state.apply_action(Action::SelectLast);
        let chosen = state.selected_id.clone();
        assert_eq!(chosen.as_deref(), Some("c"));

        let endpoint = state.systems[0].endpoint.clone();
        state.apply_batch(&PollBatch {
            generation: 2,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint,
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(50),
            }],
        });

        // The existing offline selection survives the second batch.
        assert_eq!(state.selected_id, chosen);
    }

    #[test]
    fn entry_height_online_is_five() {
        let mut state = AppState {
            systems: vec![SystemState {
                id: "test".into(),
                endpoint: Endpoint::new("host".into(), 11310, None),
                configured_name: None,
                reachability: Reachability::Online,
                latest: None,
                last_success_at: None,
                last_attempt_at: None,
                latency: None,
                offline_reason: None,
            }],
            selected_id: Some("test".into()),
            viewport_top_id: Some("test".into()),
            last_snapshot_generation: 0,
            saw_reachability: false,
            refresh_status: RefreshStatus::Idle,
            config_reload_error: None,
            cron: Vec::new(),
            cron_display_history: crate::cron::DEFAULT_DISPLAY_HISTORY,
            terminal_size: None,
            active_pane: Pane::Systems,
            system_view_mode: SystemViewMode::Normal,
            drives_expanded: false,
            network_expanded: false,
            cron_expanded: false,
            cron_job: None,
            selection_highlight_active: false,
            eggpool: None,
            eggpool_period_request: None,
            test_fleet: FleetState {
                cron: crate::cron::CronCache::default(),
                cron_display_history: crate::cron::DEFAULT_DISPLAY_HISTORY,
                systems: Vec::new(),
                last_applied_generation: 0,
                refresh_status: RefreshStatus::Idle,
                config_reload_error: None,
                eggpool: None,
            },
            test_generation: 0,
        };
        assert_eq!(entry_height(&state, 0), 5);

        state.systems[0].latest = Some(NormalizedSnapshot::from_v2_payload(
            &gregg_protocol::test_support::LinuxSnapshotV2Builder::default()
                .network(Some(gregg_protocol::v2::NetworkPayload {
                    aggregate_rx_bytes_per_sec: 0,
                    aggregate_tx_bytes_per_sec: 0,
                    aggregate_rx_capacity_bps: None,
                    aggregate_tx_capacity_bps: None,
                    interfaces: vec![],
                }))
                .build_payload(),
        ));
        assert_eq!(entry_height(&state, 0), 6);

        state.systems[0].reachability = Reachability::Pending;
        assert_eq!(entry_height(&state, 0), 1);

        state.systems[0].reachability = Reachability::Offline;
        assert_eq!(entry_height(&state, 0), 1);
    }

    #[test]
    fn visible_range_handles_mixed_heights() {
        let config = test_config_with_ids(&["a", "b", "c", "d", "e"]);
        let state = AppState::synthetic(&config);
        let order = state.display_order();
        let range = visible_range(&order, &state, 0, 20);
        // Should include some entries.
        assert!(!range.is_empty());
    }

    #[test]
    fn visible_range_small_terminal() {
        let config = test_config_with_ids(&["a", "b", "c"]);
        let mut state = AppState::synthetic(&config);
        state.systems[0].reachability = Reachability::Online;
        let order = state.display_order();
        let range = visible_range(&order, &state, 0, 3);
        // Terminal too small for even one online entry.
        assert!(range.is_empty());
    }

    #[test]
    fn visible_range_online_boundary_is_five_rows() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);
        state.systems[0].reachability = Reachability::Online;
        let order = state.display_order();

        assert!(visible_range(&order, &state, 0, 4).is_empty());
        assert_eq!(visible_range(&order, &state, 0, 5), 0..1);
    }

    #[test]
    fn visible_range_first_offline_entry_does_not_reserve_online_height() {
        let config = test_config_with_ids(&["offline", "online"]);
        let mut state = AppState::synthetic(&config);
        state.systems[1].reachability = Reachability::Online;
        let order = vec![0, 1];

        assert_eq!(visible_range(&order, &state, 0, 1), 0..1);
    }

    #[test]
    fn visible_range_expanded_online_entry_clips_only_drive_rows() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);
        state.systems[0].reachability = Reachability::Online;
        state.systems[0].latest = Some(NormalizedSnapshot::from_v1(&make_snapshot()));
        state.systems[0].latest.as_mut().unwrap().drives = Some(
            (0..3)
                .map(|index| crate::normalized::NormalizedDrive {
                    name: format!("drive{index}"),
                    used_bytes: 1,
                    total_bytes: 2,
                    available_bytes: None,
                })
                .collect(),
        );
        state.selected_id = Some("a".into());
        state.drives_expanded = true;
        let order = state.display_order();

        assert_eq!(visible_range(&order, &state, 0, 5), 0..1);
        assert_eq!(visible_range(&order, &state, 0, 6), 0..1);
        assert_eq!(entry_height(&state, 0), 8);

        let viewport = crate::ui::layout::compute_viewport(
            &state,
            ratatui::layout::Rect::new(0, 0, 80, 5),
            &order,
        );
        assert_eq!(viewport[0].drive_rows_visible, 0);
        let viewport = crate::ui::layout::compute_viewport(
            &state,
            ratatui::layout::Rect::new(0, 0, 80, 6),
            &order,
        );
        assert_eq!(viewport[0].drive_rows_visible, 1);
    }

    #[test]
    fn mixed_network_availability_uses_non_overlapping_per_system_heights() {
        let config = test_config_with_ids(&["network-a", "legacy", "network-b", "offline"]);
        let mut state = AppState::synthetic(&config);
        for index in [0, 1, 2] {
            state.systems[index].reachability = Reachability::Online;
        }
        state.systems[0].latest = Some(NormalizedSnapshot::from_v2_payload(
            &gregg_protocol::test_support::LinuxSnapshotV2Builder::default()
                .network(Some(gregg_protocol::v2::NetworkPayload {
                    aggregate_rx_bytes_per_sec: 0,
                    aggregate_tx_bytes_per_sec: 0,
                    aggregate_rx_capacity_bps: None,
                    aggregate_tx_capacity_bps: None,
                    interfaces: vec![],
                }))
                .build_payload(),
        ));
        state.systems[1].latest = Some(NormalizedSnapshot::from_v1(&make_snapshot()));
        state.systems[2].latest = state.systems[0].latest.clone();

        let order = state.display_order();
        assert_eq!(
            order
                .iter()
                .map(|&index| entry_height(&state, index))
                .collect::<Vec<_>>(),
            vec![6, 5, 6, 1]
        );
        let viewport = crate::ui::layout::compute_viewport(
            &state,
            ratatui::layout::Rect::new(0, 0, 120, 18),
            &order,
        );
        assert_eq!(
            viewport
                .iter()
                .map(|entry| (entry.rect.y, entry.rect.height))
                .collect::<Vec<_>>(),
            vec![(0, 6), (6, 5), (11, 6), (17, 1)]
        );
    }

    #[test]
    fn expansion_offsets_follow_selected_system_base_height() {
        let config = test_config_with_ids(&["legacy", "network"]);
        let mut state = AppState::synthetic(&config);
        for system in &mut state.systems {
            system.reachability = Reachability::Online;
        }
        state.systems[0].latest = Some(NormalizedSnapshot::from_v1(&make_snapshot()));
        state.systems[1].latest = Some(NormalizedSnapshot::from_v2_payload(
            &gregg_protocol::test_support::LinuxSnapshotV2Builder::default()
                .network(Some(gregg_protocol::v2::NetworkPayload {
                    aggregate_rx_bytes_per_sec: 1,
                    aggregate_tx_bytes_per_sec: 2,
                    aggregate_rx_capacity_bps: None,
                    aggregate_tx_capacity_bps: None,
                    interfaces: vec![],
                }))
                .build_payload(),
        ));
        state.systems[0].latest.as_mut().unwrap().drives =
            Some(vec![crate::normalized::NormalizedDrive {
                name: "/".into(),
                used_bytes: 1,
                total_bytes: 2,
                available_bytes: None,
            }]);
        state.systems[1].latest.as_mut().unwrap().drives = state.systems[0]
            .latest
            .as_ref()
            .and_then(|snapshot| snapshot.drives.clone());

        state.selected_id = Some("legacy".into());
        state.drives_expanded = true;
        let order = state.display_order();
        let legacy_viewport = crate::ui::layout::compute_viewport(
            &state,
            ratatui::layout::Rect::new(0, 0, 120, 6),
            &order,
        );
        assert_eq!(legacy_viewport[0].drive_rows_visible, 1);
        assert_eq!(legacy_viewport[0].network_rows_visible, 0);

        state.selected_id = Some("network".into());
        state.viewport_top_id = Some("network".into());
        state.network_expanded = true;
        let network_viewport = crate::ui::layout::compute_viewport(
            &state,
            ratatui::layout::Rect::new(0, 0, 120, 8),
            &order,
        );
        assert_eq!(network_viewport[0].drive_rows_visible, 1);
        assert_eq!(network_viewport[0].network_rows_visible, 1);
    }

    #[test]
    fn ensure_selected_visible_adjusts_viewport() {
        let config = test_config_with_ids(&["a", "b", "c", "d", "e"]);
        let mut state = AppState::synthetic(&config);
        state.terminal_size = Some((80, 6)); // Very small: 4 usable rows

        // Select the last system.
        state.apply_action(Action::SelectLast);
        assert_eq!(state.selected_id.as_deref(), Some("e"));

        // Ensure selected is visible.
        ensure_selected_visible(&mut state);

        // The viewport should have been adjusted so e is visible.
        let order = state.display_order();
        let top_pos = state
            .viewport_top_id
            .as_ref()
            .and_then(|top| order.iter().position(|&i| &state.systems[i].id == top));
        let selected_pos = order
            .iter()
            .position(|&i| &state.systems[i].id == state.selected_id.as_ref().unwrap());
        assert!(top_pos.is_some());
        assert!(selected_pos.is_some());
        assert!(selected_pos.unwrap() >= top_pos.unwrap());
    }

    #[test]
    fn selection_stays_visible_across_dynamic_online_entries() {
        let config = test_config_with_ids(&["a", "b", "c", "d"]);
        let mut state = AppState::synthetic(&config);
        state.terminal_size = Some((80, 10));
        for system in &mut state.systems {
            system.reachability = Reachability::Online;
            system.latest = Some(NormalizedSnapshot::from_v1(&make_snapshot()));
        }

        state.apply_action(Action::SelectLast);
        let order = state.display_order();
        let top = order
            .iter()
            .position(|&index| state.systems[index].id == state.viewport_top_id.clone().unwrap())
            .unwrap();
        let selected = order
            .iter()
            .position(|&index| state.systems[index].id == state.selected_id.clone().unwrap())
            .unwrap();
        assert_eq!(top, 2);
        assert!(visible_range(&order, &state, top, 10).contains(&selected));

        state.apply_action(Action::MoveUp);
        assert_eq!(state.viewport_top_id.as_deref(), Some("c"));
    }

    #[test]
    fn expansion_changes_only_selected_entry_height() {
        let config = test_config_with_ids(&["a", "b"]);
        let mut state = AppState::synthetic(&config);
        for system in &mut state.systems {
            system.reachability = Reachability::Online;
            system.latest = Some(NormalizedSnapshot::from_v1(&make_snapshot()));
        }
        state.systems[0].latest.as_mut().unwrap().drives =
            Some(vec![crate::normalized::NormalizedDrive {
                name: "/".into(),
                used_bytes: 1,
                total_bytes: 2,
                available_bytes: None,
            }]);
        assert_eq!(entry_height(&state, 0), 5);
        assert_eq!(entry_height(&state, 1), 5);
        state.apply_action(Action::ToggleDrives);
        assert_eq!(entry_height(&state, 0), 6);
        assert_eq!(entry_height(&state, 1), 5);
    }

    #[test]
    fn normal_and_condensed_drive_detail_heights_agree() {
        // A legal v2 payload can carry `drives: None` together with
        // `disk_io: Some(..)`. The condensed view renders the table heading
        // plus the aggregate I/O total for that, so the normal view must
        // reserve the same two rows.
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);
        for system in &mut state.systems {
            system.reachability = Reachability::Online;
            system.latest = Some(NormalizedSnapshot::from_v1(&make_snapshot()));
        }
        state.systems[0].latest.as_mut().unwrap().drives = None;
        state.systems[0].latest.as_mut().unwrap().disk_io =
            Some(crate::normalized::NormalizedDiskIo {
                aggregate_read_bytes_per_sec: 1,
                aggregate_write_bytes_per_sec: 2,
                devices: Vec::new(),
            });

        let normal_base = entry_height(&state, 0);
        state.apply_action(Action::ToggleDrives);
        let normal_expanded = entry_height(&state, 0);

        state.system_view_mode = SystemViewMode::Condensed;
        let condensed_expanded = entry_height(&state, 0);

        assert_eq!(
            normal_expanded - normal_base,
            condensed_expanded - 1,
            "both views must reserve the drive heading and I/O total rows"
        );
        assert_eq!(normal_expanded - normal_base, 2);
    }

    #[test]
    fn resize_updates_terminal_size() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);

        state.apply_action(Action::Resize {
            width: 120,
            height: 40,
        });

        assert_eq!(state.terminal_size, Some((120, 40)));
    }

    #[test]
    fn empty_config_no_selection() {
        let config = Config::default();
        let mut state = AppState::synthetic(&config);

        state.apply_action(Action::MoveDown);
        assert!(state.selected_id.is_none());

        state.apply_action(Action::MoveUp);
        assert!(state.selected_id.is_none());

        state.apply_action(Action::SelectFirst);
        assert!(state.selected_id.is_none());

        state.apply_action(Action::SelectLast);
        assert!(state.selected_id.is_none());
    }

    #[test]
    fn multiple_systems_online_offline_mixed_display_order() {
        let config = test_config_with_ids(&["a", "b", "c", "d", "e"]);
        let mut state = AppState::synthetic(&config);

        // Make a, c, e online.
        let batch = PollBatch {
            generation: 1,
            started_at: Instant::now(),
            completed_at: Instant::now(),
            results: vec![
                crate::poller::PollResult {
                    system_id: "a".into(),
                    endpoint: state.systems[0].endpoint.clone(),
                    outcome: PollOutcome::Online(Box::new(make_snapshot())),
                    latency: Duration::from_millis(50),
                },
                crate::poller::PollResult {
                    system_id: "c".into(),
                    endpoint: state.systems[2].endpoint.clone(),
                    outcome: PollOutcome::Online(Box::new(make_snapshot())),
                    latency: Duration::from_millis(50),
                },
                crate::poller::PollResult {
                    system_id: "e".into(),
                    endpoint: state.systems[4].endpoint.clone(),
                    outcome: PollOutcome::Online(Box::new(make_snapshot())),
                    latency: Duration::from_millis(50),
                },
            ],
        };
        state.apply_batch(&batch);

        let order = state.display_order();
        assert_eq!(order.len(), 5);
        // Online first: a, c, e (in configured order).
        assert_eq!(state.systems[order[0]].id, "a");
        assert_eq!(state.systems[order[1]].id, "c");
        assert_eq!(state.systems[order[2]].id, "e");
        // Offline: b, d.
        assert_eq!(state.systems[order[3]].id, "b");
        assert_eq!(state.systems[order[4]].id, "d");
    }

    #[test]
    fn view_controls_wrap_and_preserve_selection_and_expansion() {
        let config = test_config_with_ids(&["a", "b"]);
        let mut state = AppState::synthetic(&config);
        state.terminal_size = Some((80, 8));
        state.systems[0].reachability = Reachability::Online;
        state.systems[0].latest = Some(NormalizedSnapshot::from_v1(&make_snapshot()));
        state.selected_id = Some("a".into());

        state.apply_action(Action::ToggleDrives);
        state.apply_action(Action::ToggleSystemView);
        assert_eq!(state.system_view_mode, SystemViewMode::Condensed);
        assert!(state.drives_expanded);
        assert_eq!(state.selected_id.as_deref(), Some("a"));
        state.apply_action(Action::ToggleSystemView);
        assert_eq!(state.system_view_mode, SystemViewMode::Normal);
        assert!(state.drives_expanded);
    }

    #[test]
    fn network_expansion_is_independent_and_legacy_is_a_noop() {
        let config = test_config_with_ids(&["live"]);
        let mut state = AppState::synthetic(&config);
        state.systems[0].reachability = Reachability::Online;
        state.systems[0].latest = Some(NormalizedSnapshot::from_v2_payload(
            &gregg_protocol::test_support::LinuxSnapshotV2Builder::default()
                .network(Some(gregg_protocol::v2::NetworkPayload {
                    aggregate_rx_bytes_per_sec: 10,
                    aggregate_tx_bytes_per_sec: 20,
                    aggregate_rx_capacity_bps: None,
                    aggregate_tx_capacity_bps: None,
                    interfaces: vec![],
                }))
                .build_payload(),
        ));
        state.apply_action(Action::ToggleDrives);
        state.apply_action(Action::ToggleNetwork);
        assert!(state.drives_expanded);
        assert!(state.network_expanded);
        assert_eq!(entry_height(&state, 0), 7);

        let mut legacy = AppState::synthetic(&config);
        legacy.systems[0].reachability = Reachability::Online;
        legacy.systems[0].latest = Some(NormalizedSnapshot::from_v1(&make_snapshot()));
        legacy.apply_action(Action::ToggleNetwork);
        assert!(!legacy.network_expanded);
        assert_eq!(entry_height(&legacy, 0), 5);
    }

    #[test]
    fn condensed_expansion_counts_only_valid_drive_rows() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);
        state.system_view_mode = SystemViewMode::Condensed;
        state.drives_expanded = true;
        state.systems[0].reachability = Reachability::Online;
        let mut snapshot = NormalizedSnapshot::from_v1(&make_snapshot());
        snapshot.drives = Some(vec![
            crate::normalized::NormalizedDrive {
                name: "/".into(),
                used_bytes: 1,
                total_bytes: 2,
                available_bytes: None,
            },
            crate::normalized::NormalizedDrive {
                name: "/bad".into(),
                used_bytes: 3,
                total_bytes: 2,
                available_bytes: None,
            },
        ]);
        state.systems[0].latest = Some(snapshot);
        assert_eq!(entry_height(&state, 0), 2);
    }

    // ===== Plan 143: changed-result reducer seams =====

    fn plan143_online_batch(state: &AppState, generation: u64) -> PollBatch {
        let now = Instant::now();
        PollBatch {
            generation,
            started_at: now,
            completed_at: now,
            results: state
                .systems
                .iter()
                .map(|system| crate::poller::PollResult {
                    system_id: system.id.clone(),
                    endpoint: system.endpoint.clone(),
                    outcome: PollOutcome::Online(Box::new(make_snapshot())),
                    latency: Duration::from_millis(1),
                })
                .collect(),
        }
    }

    #[test]
    fn plan143_stale_generation_reports_unchanged_without_draw() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);
        let first = plan143_online_batch(&state, 1);
        assert!(state.apply_batch_changed(&first));
        let mut draws = 0;
        if state.apply_batch_changed(&first) {
            draws += 1;
        }
        assert_eq!(draws, 0, "rejected stale batch must not force a frame");
        // Owned path mirrors the borrowed compatibility path.
        let stale_owned = plan143_online_batch(&state, 1);
        assert!(!state.apply_batch_owned_changed(stale_owned));
    }

    #[test]
    fn plan143_stale_endpoint_target_ignored_without_draw() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);
        let first = plan143_online_batch(&state, 1);
        assert!(state.apply_batch_changed(&first));
        let now = Instant::now();
        let stale_target = PollBatch {
            generation: 2,
            started_at: now,
            completed_at: now,
            results: vec![crate::poller::PollResult {
                system_id: "a".into(),
                endpoint: Endpoint::new("old.local".into(), 11310, None),
                outcome: PollOutcome::Online(Box::new(make_snapshot())),
                latency: Duration::from_millis(1),
            }],
        };
        // Host/port guard ignores the superseded target; nothing else
        // changed, so no frame.
        assert!(!state.apply_batch_changed(&stale_target));
    }

    #[test]
    fn plan143_boundary_navigation_reports_unchanged() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);
        state.selection_highlight_active = true;
        // Single system: moving down cannot change selection; highlight is
        // already active, so no visible change.
        assert!(!state.apply_action_changed(Action::MoveDown));
        assert!(!state.apply_action_changed(Action::MoveUp));
        // Clearing an already-clear highlight is a no-op.
        state.selection_highlight_active = false;
        assert!(!state.apply_action_changed(Action::ClearSelectionHighlight));
        // Clearing an active highlight is visible.
        state.selection_highlight_active = true;
        assert!(state.apply_action_changed(Action::ClearSelectionHighlight));
        assert!(!state.selection_highlight_active);
    }

    #[test]
    fn plan143_real_selection_change_redraws_and_arms_highlight() {
        let config = test_config_with_ids(&["a", "b"]);
        let mut state = AppState::synthetic(&config);
        state.selection_highlight_active = false;
        assert!(state.apply_action_changed(Action::MoveDown));
        assert!(state.selection_highlight_active);
        assert_eq!(state.selected_id.as_deref(), Some("b"));
    }

    #[test]
    fn plan143_resize_always_reports_changed() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);
        state.terminal_size = Some((80, 24));
        assert!(state.apply_action_changed(Action::Resize {
            width: 80,
            height: 24
        }));
    }

    #[test]
    fn plan143_repeated_identical_online_batch_reports_unchanged() {
        let config = test_config_with_ids(&["a"]);
        let mut state = AppState::synthetic(&config);
        let first = plan143_online_batch(&state, 1);
        assert!(state.apply_batch_changed(&first));
        // Same snapshot values again (latency/timestamps differ but are not
        // rendered): no visible change, no frame.
        let second = plan143_online_batch(&state, 2);
        assert!(!state.apply_batch_changed(&second));
    }
}
