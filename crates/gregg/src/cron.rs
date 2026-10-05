//! Plan 166: remote scheduler observability as the client sees it.
//!
//! `greggd` publishes two additive read-only routes (Plan 162/163): a compact
//! `/v2/scheduler` summary and a larger `/v2/scheduler/history`. This module is
//! the client's model of them: what capability a remote has, the latest
//! summary, why the last read failed, and a **memory-only** history cache that
//! is deeper than the remote ring.
//!
//! # Why the client keeps its own history at all
//!
//! The remote ring retains [`DEFAULT_SCHEDULER_HISTORY_LIMIT`] records per job
//! by default and is the authority. The client daemon, however, is *always
//! running* while TUIs come and go, and it observes successive remote rings.
//! Keeping a deeper local cache is what makes "closing and reopening the TUI
//! does not reset what Gregg has observed" true rather than aspirational.
//!
//! # Why that cache is bounded three ways
//!
//! Per-job depth alone is not a bound. The fleet shape multiplies: endpoints ×
//! jobs × depth. A 64-endpoint fleet of 64-job daemons at depth 50 would be
//! 204,800 records, so the cache is bounded by
//!
//! 1. a per-job depth (`cache_history`, hard maximum),
//! 2. a global record ceiling across *every* system and job, and
//! 3. the remote contract's own per-stream output cap, which makes a single
//!    record's size bounded and therefore makes a record *count* a real memory
//!    bound rather than an approximation.
//!
//! Nothing is written to disk. Plan 166 adds no history database: a restart
//! reseeds the cache from whatever the remote still retains.
//!
//! # Epochs, not sequence numbers
//!
//! Deduplication identity is `(epoch, sequence)`, never `sequence` alone. A
//! restarted `greggd` legitimately reissues sequences from zero, so a
//! sequence-only cache would silently drop a new epoch's records as duplicates
//! — or, worse, keep claiming a stale pending job is still pending. Every
//! locally retained record therefore carries the epoch it was observed under,
//! and a new epoch never rewrites the old one.

use std::collections::BTreeMap;

use gregg_protocol::{SchedulerEpochV2, SchedulerHistoryV2, SchedulerJobV2, SchedulerSummaryV2};
use serde::{Deserialize, Serialize};

/// Number of terminal records the cron view shows for the selected job when
/// the operator has not configured otherwise.
///
/// Matches the remote default so the common case is "exactly what the remote
/// still holds", and is deliberately not a larger number: this is a *display*
/// window, not the cache depth.
pub const DEFAULT_DISPLAY_HISTORY: usize = 5;

/// Hard maximum for the display window.
pub const MAX_DISPLAY_HISTORY: usize = 20;

/// Number of terminal records the client daemon caches per job by default.
///
/// Five times the remote default, because the client is always running and can
/// accumulate rings the remote has already dropped.
pub const DEFAULT_CACHE_HISTORY: usize = 25;

/// Hard maximum for the per-job cache depth.
pub const MAX_CACHE_HISTORY: usize = 50;

/// Hard global ceiling on retained terminal records, across every system, job,
/// and epoch.
///
/// This is the bound that actually protects a large fleet. Per-job depth is
/// only meaningful while the fleet is small, and a global cap is what stops
/// `endpoints × jobs × depth` from becoming memory. 4096 records at the remote
/// contract's worst case (two streams of
/// [`gregg_protocol::MAX_SCHEDULER_OUTPUT_TEXT_BYTES`] plus bounded metadata,
/// ≈ 1.2 KiB stored) is under 5 MiB, which stays proportionate to Gregg's
/// lightweight client. Eviction is oldest-first, so the newest window survives.
pub const MAX_TOTAL_CRON_RECORDS: usize = 4096;

/// Hard maximum on retained job histories (distinct job names) per system.
///
/// A stale job that was removed from a remote's configuration must not
/// accumulate forever across reloads.
pub const MAX_CRON_JOBS_PER_SYSTEM: usize = 64;

/// Whether a remote daemon serves the Plan-162 scheduler routes.
///
/// Distinct from system reachability on purpose: a pre-scheduler `greggd` is a
/// fully healthy daemon whose metrics are current, and marking it offline
/// because an additive route is missing would be a lie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CronCapability {
    /// The scheduler routes have not been asked about yet.
    Unknown,
    /// The remote answered metrics but does not serve the scheduler routes.
    ///
    /// This is the normal, expected state for an old `greggd` — not an error.
    Unsupported,
    /// The remote serves the scheduler routes.
    Supported,
}

/// Why the most recent scheduler read did not produce data.
///
/// Every variant is scoped to the scheduler routes alone. None of them says
/// anything about whether the system is up, and none of them is ever allowed
/// to reach [`crate::state::Reachability`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CronFetchError {
    /// The remote could not be reached, or answered with a non-success status.
    ///
    /// Retained cron data stays usable; the renderer marks it stale.
    Transport(String),
    /// The body exceeded the route's cap and was refused.
    BodyTooLarge,
    /// The body did not parse, or failed the wire's own validation.
    ///
    /// Carrying the validation detail matters: "your greggd produced a document
    /// that breaks its own contract" is a different problem from "the network
    /// flapped", and conflating them hides a real bug.
    Invalid(String),
    /// The history document does not belong to the summary it was fetched with.
    ///
    /// Summary and history are two independent requests, so a remote restart or
    /// a history change between them can pair summary A with history B. That
    /// pair is not history, so it is neither merged nor allowed to advance the
    /// history gate; the retained summary stays and the next cadence retries.
    Incoherent(String),
}

impl std::fmt::Display for CronFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(message) => write!(f, "{message}"),
            Self::BodyTooLarge => write!(f, "response exceeded the scheduler body cap"),
            Self::Invalid(message) => write!(f, "invalid scheduler document: {message}"),
            Self::Incoherent(message) => {
                write!(f, "scheduler history does not match the summary: {message}")
            }
        }
    }
}

/// The operator-visible scheduler state of one system.
///
/// This is exactly what a cron row can draw: the capability the header shows,
/// the summary's job rows, the `(epoch, revision)` the retained records are
/// keyed by, and the stale marker. Two states with equal digests render
/// identically, and *that* — not "a timestamp moved" — is what a frontend
/// publication has to be driven by.
///
/// `generated_at_unix_ms` and the local attempt/success timestamps are
/// deliberately absent. Nothing in a row renders them, and a `greggd` rebuilds
/// its document whenever any live transition happens, so including them would
/// turn an ordinary 30-second cadence into a full document rebuild for every
/// frontend. Everything here is either drawn, or the identity that decides
/// whether the drawn records are the right ones.
///
/// The digest clones the bounded job list rather than hashing it: one clone per
/// cron cadence per endpoint, next to an HTTP request that allocated far more.
#[derive(Debug, Clone, PartialEq)]
pub struct CronRenderedState {
    /// Whether the remote serves the scheduler routes.
    pub capability: CronCapability,
    /// `(epoch, history_revision)` of the retained live state.
    pub identity: Option<(SchedulerEpochV2, u64)>,
    /// The job rows, in the remote's own order; `None` when no summary is held.
    pub jobs: Option<Vec<SchedulerJobV2>>,
    /// The stale marker, including its absence.
    pub last_error: Option<CronFetchError>,
}

/// One retained terminal record, tagged with the epoch it was observed under.
///
/// The epoch is what makes `(epoch, sequence)` a usable deduplication identity
/// and what stops a restarted remote's reused sequence values from colliding
/// with records from its previous lifetime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CronRecord {
    /// The remote scheduler lifetime this record belongs to.
    pub epoch: SchedulerEpochV2,
    /// The record itself, as published by the remote.
    pub record: gregg_protocol::SchedulerRunRecordV2,
}

impl CronRecord {
    /// Deduplication identity of this record.
    #[must_use]
    pub fn identity(&self) -> RecordId {
        RecordId {
            started_at_unix_ms: self.epoch.started_at_unix_ms,
            nonce: self.epoch.nonce,
            sequence: self.record.sequence,
        }
    }
}

/// A record's `(epoch, sequence)` identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RecordId {
    /// Remote scheduler lifetime start.
    pub started_at_unix_ms: u64,
    /// Remote process-lifetime disambiguator.
    pub nonce: u64,
    /// Scheduler-lifetime monotonic record sequence.
    pub sequence: u64,
}

/// Cached history for one job.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CronJobCache {
    /// Retained records, oldest first.
    pub records: Vec<CronRecord>,
}

impl CronJobCache {
    /// Whether a record with this identity is already retained.
    fn contains(&self, id: RecordId) -> bool {
        self.records.iter().any(|record| record.identity() == id)
    }
}

/// Everything the client daemon knows about one system's scheduler.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CronSystemState {
    /// Whether the remote serves the scheduler routes.
    pub capability: CronCapability,
    /// Latest valid summary. `None` until one has been read.
    pub summary: Option<SchedulerSummaryV2>,
    /// Remote lifetime the retained records belong to.
    pub epoch: Option<SchedulerEpochV2>,
    /// `history_revision` of the most recently *applied* history document.
    ///
    /// This is the client's "do I need to refetch?" answer, and it is tracked
    /// separately from the summary's current revision so a failed history fetch
    /// is retried instead of being suppressed forever.
    pub history_revision: Option<u64>,
    /// Unix milliseconds of the last summary attempt.
    pub last_attempt_at_unix_ms: Option<u64>,
    /// Unix milliseconds of the last successful summary read.
    pub last_success_at_unix_ms: Option<u64>,
    /// Most recent scheduler-route failure, if any.
    ///
    /// Its presence means any retained summary is not known to be current.
    pub last_error: Option<CronFetchError>,
    /// Retained per-job history, memory-only.
    pub jobs: BTreeMap<String, CronJobCache>,
}

impl Default for CronSystemState {
    fn default() -> Self {
        Self {
            capability: CronCapability::Unknown,
            summary: None,
            epoch: None,
            history_revision: None,
            last_attempt_at_unix_ms: None,
            last_success_at_unix_ms: None,
            last_error: None,
            jobs: BTreeMap::new(),
        }
    }
}

impl CronSystemState {
    /// A system that has never been asked about.
    #[must_use]
    pub fn unknown() -> Self {
        Self::default()
    }

    /// Record a successful summary read.
    ///
    /// A success is also the latest *attempt*: the read happened, and both
    /// timestamps are what a diagnostic needs to say "we asked recently and it
    /// worked". Neither is operator-visible, so neither forces a publication.
    pub fn apply_summary(&mut self, summary: SchedulerSummaryV2, now_unix_ms: u64) {
        self.capability = CronCapability::Supported;
        self.last_attempt_at_unix_ms = Some(now_unix_ms);
        self.last_success_at_unix_ms = Some(now_unix_ms);
        self.last_error = None;
        self.summary = Some(summary);
    }

    /// The operator-visible scheduler state, for the publication decision.
    #[must_use]
    pub fn rendered_state(&self) -> CronRenderedState {
        CronRenderedState {
            capability: self.capability,
            identity: self
                .summary
                .as_ref()
                .map(|summary| (summary.epoch, summary.history_revision)),
            jobs: self.summary.as_ref().map(|summary| summary.jobs.clone()),
            last_error: self.last_error.clone(),
        }
    }

    /// Record that the remote does not serve the scheduler routes.
    ///
    /// An old `greggd` is not a failure, so this clears the error. It does keep
    /// any previously retained history: the remote may simply have been
    /// replaced, and a record Gregg genuinely observed stays observed.
    pub fn mark_unsupported(&mut self) {
        self.capability = CronCapability::Unsupported;
        self.last_error = None;
        self.summary = None;
    }

    /// Record a scheduler-route failure without touching retained data.
    ///
    /// The plan is explicit that one transient failure must not erase useful
    /// history, so this leaves `summary` and `jobs` alone and only records why
    /// they are not known to be current.
    pub fn mark_failed(&mut self, error: CronFetchError, now_unix_ms: u64) {
        self.last_error = Some(error);
        self.last_attempt_at_unix_ms = Some(now_unix_ms);
    }

    /// Whether a history fetch is needed for this summary.
    ///
    /// Three conditions, and all three are load-bearing:
    ///
    /// - no revision has been applied yet (first support discovery);
    /// - the revision moved (retained history changed);
    /// - the epoch moved, **even if the revision did not**.
    ///
    /// The last one is the subtle one: a restarted `greggd` resets
    /// `history_revision`, and it can reset it to the same small number it used
    /// before. Without the epoch comparison, a daemon restart would look
    /// exactly like "nothing changed" and the new epoch's records would never
    /// be fetched.
    #[must_use]
    pub fn needs_history(&self, summary: &SchedulerSummaryV2) -> bool {
        if self.capability != CronCapability::Supported {
            return true;
        }
        if self.history_revision != Some(summary.history_revision) {
            return true;
        }
        self.epoch != Some(summary.epoch)
    }

    /// Merge a history document into the cache, deduplicating by identity.
    ///
    /// Returns whether anything was actually added, so the caller can publish
    /// only on a real change.
    ///
    /// A record whose `(epoch, sequence)` is already retained is skipped, which
    /// is what makes a repeated response — or a repeated poll that happens to
    /// observe the same ring — a no-op rather than a doubling.
    pub fn apply_history(&mut self, history: &SchedulerHistoryV2, per_job: usize) -> bool {
        let epoch = history.epoch;
        self.epoch = Some(epoch);
        self.history_revision = Some(history.history_revision);

        let mut added = false;
        for job in &history.jobs {
            // A job name arrives from a remote configuration, so it is
            // untrusted input even though it is not printed verbatim.
            if job.name.is_empty() || job.name.len() > gregg_protocol::MAX_SCHEDULER_JOB_NAME_BYTES
            {
                continue;
            }
            let entry = self.jobs.entry(job.name.clone()).or_default();
            for record in &job.records {
                let candidate = CronRecord {
                    epoch,
                    record: record.clone(),
                };
                if entry.contains(candidate.identity()) {
                    continue;
                }
                entry.records.push(candidate);
                added = true;
            }
            // Depth eviction drops from the front: the remote serves oldest
            // first, so the newest records are at the back and survive.
            if entry.records.len() > per_job {
                let excess = entry.records.len() - per_job;
                entry.records.drain(..excess);
            }
        }
        self.evict_stale_jobs();
        added
    }

    /// Drop job histories for names the remote no longer serves.
    ///
    /// Without this, a job removed from a remote's configuration would keep its
    /// records alive for the daemon's whole life. A job that vanished is not
    /// evidence of anything current, and the bound matters more than the
    /// archaeology.
    ///
    /// Only runs when a summary is actually known. "This job is no longer
    /// configured" is a fact that can only come from a summary, and a history
    /// document that arrived before one must not be read as "the remote serves
    /// no jobs at all" — that would silently discard everything.
    fn evict_stale_jobs(&mut self) {
        if self.summary.is_none() {
            return;
        }
        let live: Vec<String> = self
            .summary
            .as_ref()
            .map(|summary| summary.jobs.iter().map(|job| job.name.clone()).collect())
            .unwrap_or_default();
        if self
            .jobs
            .keys()
            .all(|name| live.iter().any(|kept| kept == name))
        {
            return;
        }
        self.jobs
            .retain(|name, _| live.iter().any(|kept| kept == name));
        while self.jobs.len() > MAX_CRON_JOBS_PER_SYSTEM {
            if let Some(key) = self.jobs.keys().next().cloned() {
                self.jobs.remove(&key);
            }
        }
    }

    /// Records retained for one job, oldest first.
    #[must_use]
    pub fn job_records(&self, name: &str) -> &[CronRecord] {
        self.jobs
            .get(name)
            .map_or(&[], |job| job.records.as_slice())
    }

    /// The newest `depth` records for one job, oldest first.
    #[must_use]
    pub fn recent(&self, name: &str, depth: usize) -> Vec<CronRecord> {
        let records = self.job_records(name);
        if depth >= records.len() {
            return records.to_vec();
        }
        records[records.len() - depth..].to_vec()
    }
}

/// The fleet-wide, memory-only cron history cache.
///
/// Owns the *global* record ceiling, which is the only bound that survives a
/// pathological endpoint/job/depth product, and evicts oldest-first across every
/// system so the newest requested window is preserved where possible.
#[derive(Debug, Clone, PartialEq)]
pub struct CronCache {
    /// Per-job depth.
    per_job: usize,
    /// Global record ceiling.
    total_cap: usize,
    /// Current total retained records.
    total: usize,
    /// Per-system scheduler state, keyed by the stable system id.
    systems: BTreeMap<String, CronSystemState>,
}

impl CronCache {
    /// A cache with the given per-job depth and global ceiling, both clamped to
    /// their hard maxima.
    #[must_use]
    pub fn new(per_job: usize, total_cap: usize) -> Self {
        Self {
            per_job: per_job.clamp(1, MAX_CACHE_HISTORY),
            total_cap: total_cap.clamp(1, MAX_TOTAL_CRON_RECORDS),
            total: 0,
            systems: BTreeMap::new(),
        }
    }

    /// The configured per-job depth.
    #[must_use]
    pub fn per_job(&self) -> usize {
        self.per_job
    }

    /// The global record ceiling.
    #[must_use]
    pub fn total_cap(&self) -> usize {
        self.total_cap
    }

    /// Records currently retained across every system and job.
    #[must_use]
    pub fn total_records(&self) -> usize {
        self.total
    }

    /// Number of systems with any scheduler state.
    #[must_use]
    pub fn system_count(&self) -> usize {
        self.systems.len()
    }

    /// Per-system state, if any has been recorded.
    #[must_use]
    pub fn system(&self, system_id: &str) -> Option<&CronSystemState> {
        self.systems.get(system_id)
    }

    /// Mutable per-system state, created on first use.
    pub fn system_mut(&mut self, system_id: &str) -> &mut CronSystemState {
        self.systems.entry(system_id.to_owned()).or_default()
    }

    /// Forget everything known about one system.
    ///
    /// Used when a stable id is deliberately repointed at a different endpoint:
    /// the previous target's capability, summary, error, and retained job
    /// history all described a machine that is no longer configured under this
    /// id. Keeping any of it would let the new target inherit a stale summary,
    /// an error that is not its own, and records Gregg observed somewhere else —
    /// and it would also leave the pane showing the old endpoint's jobs with no
    /// marker that they are not current.
    ///
    /// An equivalent spelling of the *same* normalized endpoint must not call
    /// this: the reconciliation that decides uses the same equivalence rule the
    /// metrics reducer applies to poll results.
    pub fn reset_system(&mut self, system_id: &str) -> bool {
        let removed = self.systems.remove(system_id).is_some();
        if removed {
            // Recount rather than subtracting: this is the same bulk-removal
            // shape as `retain_systems`, and recounting is one pass over a
            // bounded map instead of a second invariant to keep correct.
            self.recount();
        }
        removed
    }

    /// Drop every system that is no longer in the fleet.
    ///
    /// A removed endpoint must not keep its history alive: it will never be
    /// requested again, and the global bound is better spent on live systems.
    pub fn retain_systems(&mut self, live: &[String]) {
        self.systems
            .retain(|id, _| live.iter().any(|kept| kept == id));
        self.recount();
    }

    /// Apply a summary read for one system.
    pub fn apply_summary(
        &mut self,
        system_id: &str,
        summary: SchedulerSummaryV2,
        now_unix_ms: u64,
    ) {
        self.system_mut(system_id)
            .apply_summary(summary, now_unix_ms);
    }

    /// Apply a history document for one system, then enforce the global bound.
    ///
    /// Returns whether anything was added.
    pub fn apply_history(&mut self, system_id: &str, history: &SchedulerHistoryV2) -> bool {
        let per_job = self.per_job;
        let added = self.system_mut(system_id).apply_history(history, per_job);
        // Recount rather than incrementing by the number added. A merge can
        // both append records and drop some (the per-job depth drain and the
        // stale-job prune), so an incremental counter has to be right about
        // every one of those paths or the ceiling silently stops bounding
        // anything. History fetches are revision-driven and therefore rare, so
        // a recount here is not on a hot path.
        self.recount();
        self.enforce_total();
        added
    }

    /// Recount retained records from scratch.
    ///
    /// Used after bulk removal, where incrementally maintaining the counter
    /// would be more code than recomputing it.
    fn recount(&mut self) {
        self.total = self
            .systems
            .values()
            .flat_map(|system| system.jobs.values())
            .map(|job| job.records.len())
            .sum();
    }

    /// Drop oldest-first until the global ceiling is satisfied.
    ///
    /// Deterministic by construction: the victim is always the retained record
    /// with the smallest `finished_unix_ms`, and ties are broken by
    /// `(system id, job name)` so two runs over the same inputs evict the same
    /// record. An "oldest first" policy that depended on hash order would make
    /// the cache's contents unreproducible and therefore untestable.
    fn enforce_total(&mut self) {
        while self.total > self.total_cap {
            let Some(victim) = self.oldest_record() else {
                // The counter and the contents disagree; recounting is the only
                // way back to a truthful state.
                self.recount();
                if self.total <= self.total_cap {
                    return;
                }
                return;
            };
            if let Some(job) = self
                .systems
                .get_mut(&victim.system)
                .and_then(|system| system.jobs.get_mut(&victim.job))
            {
                job.records.remove(0);
            }
            self.total -= 1;
        }
    }

    /// The retained record that eviction would drop next.
    fn oldest_record(&self) -> Option<OldestRecord> {
        let mut best: Option<OldestRecord> = None;
        for (system_id, system) in &self.systems {
            for (job_name, job) in &system.jobs {
                let Some(head) = job.records.first() else {
                    continue;
                };
                let candidate = OldestRecord {
                    finished_unix_ms: head.record.finished_unix_ms,
                    system: system_id.clone(),
                    job: job_name.clone(),
                };
                best = Some(match best {
                    Some(current) if current <= candidate => current,
                    _ => candidate,
                });
            }
        }
        best
    }
}

impl Default for CronCache {
    fn default() -> Self {
        Self::new(DEFAULT_CACHE_HISTORY, MAX_TOTAL_CRON_RECORDS)
    }
}

/// Identity of the next record eviction would drop.
///
/// The derived `Ord` is the eviction policy, so the fields are ordered as the
/// documented policy reads: oldest timestamp first, ties broken by
/// `(system id, job name)`. Leading with the identity would make the minimum
/// the alphabetically-first `(system, job)` pair instead of the oldest record.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OldestRecord {
    finished_unix_ms: u64,
    system: String,
    job: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use gregg_protocol::{
        SchedulerHistoryV2, SchedulerJobHistoryV2, SchedulerJobStateV2, SchedulerJobV2,
        SchedulerOutcomeV2, SchedulerOutputV2, SchedulerRunRecordV2, SchedulerRunSummaryV2,
    };

    fn epoch(start: u64, nonce: u64) -> SchedulerEpochV2 {
        SchedulerEpochV2 {
            started_at_unix_ms: start,
            nonce,
        }
    }

    fn record(sequence: u64, finished: u64) -> SchedulerRunRecordV2 {
        SchedulerRunRecordV2 {
            sequence,
            scheduled_unix_ms: finished.saturating_sub(1_000),
            started_unix_ms: Some(finished.saturating_sub(900)),
            finished_unix_ms: finished,
            outcome: SchedulerOutcomeV2::Success,
            exit_code: Some(0),
            signal: None,
            duration_ms: Some(900),
            delay_ms: 0,
            coalesced: false,
            stdout: SchedulerOutputV2::new(String::new(), false),
            stderr: SchedulerOutputV2::new(String::new(), false),
        }
    }

    fn history_document(
        epoch_value: SchedulerEpochV2,
        revision: u64,
        job: &str,
        records: Vec<SchedulerRunRecordV2>,
    ) -> SchedulerHistoryV2 {
        SchedulerHistoryV2 {
            schema_version: 2,
            generated_at_unix_ms: 1_700_000_000_000,
            epoch: epoch_value,
            history_revision: revision,
            jobs: vec![SchedulerJobHistoryV2 {
                name: job.to_owned(),
                records,
            }],
        }
    }

    fn summary_document(
        epoch_value: SchedulerEpochV2,
        revision: u64,
        jobs: &[&str],
    ) -> SchedulerSummaryV2 {
        SchedulerSummaryV2 {
            schema_version: 2,
            generated_at_unix_ms: 1_700_000_000_000,
            epoch: epoch_value,
            history_revision: revision,
            jobs: jobs
                .iter()
                .map(|name| SchedulerJobV2 {
                    name: (*name).to_owned(),
                    schedule: "0 3 * * *".to_owned(),
                    next_due_unix_ms: 1_700_100_000_000,
                    state: SchedulerJobStateV2::Idle,
                    load: None,
                    pending_since_unix_ms: None,
                    next_retry_unix_ms: None,
                    running_since_unix_ms: None,
                    last: None,
                })
                .collect(),
        }
    }

    fn last_summary(sequence: u64) -> SchedulerRunSummaryV2 {
        SchedulerRunSummaryV2 {
            sequence,
            scheduled_unix_ms: 1_700_000_000_000,
            finished_unix_ms: 1_700_000_001_000,
            outcome: SchedulerOutcomeV2::Success,
            exit_code: Some(0),
            signal: None,
            duration_ms: Some(1_000),
            delay_ms: 0,
            coalesced: false,
        }
    }

    #[test]
    fn a_never_polled_system_knows_nothing_rather_than_nothing_configured() {
        let state = CronSystemState::unknown();
        assert_eq!(state.capability, CronCapability::Unknown);
        assert!(state.summary.is_none());
        assert!(state.jobs.is_empty());
        assert!(state.last_error.is_none());
    }

    #[test]
    fn a_404_is_reported_as_unsupported_not_as_a_failure() {
        let mut state = CronSystemState::unknown();
        state.mark_unsupported();
        assert_eq!(state.capability, CronCapability::Unsupported);
        assert!(
            state.last_error.is_none(),
            "an old greggd is expected, not an error"
        );
    }

    #[test]
    fn a_transient_failure_keeps_the_last_known_data() {
        let mut cache = CronCache::default();
        let summary = summary_document(epoch(1_000, 1), 3, &["backup"]);
        cache.apply_summary("sys", summary, 1_700_000_000_000);
        cache.apply_history(
            "sys",
            &history_document(
                epoch(1_000, 1),
                3,
                "backup",
                vec![record(1, 1_700_000_001_000)],
            ),
        );

        let state = cache.system_mut("sys");
        state.mark_failed(
            CronFetchError::Transport("connection reset".into()),
            1_700_000_005_000,
        );

        assert_eq!(
            state.last_error,
            Some(CronFetchError::Transport("connection reset".into()))
        );
        assert!(state.summary.is_some(), "retained summary must survive");
        assert_eq!(state.job_records("backup").len(), 1, "history must survive");
    }

    #[test]
    fn a_repeated_history_response_does_not_duplicate_the_cache() {
        let mut cache = CronCache::default();
        let doc = history_document(
            epoch(1_000, 1),
            7,
            "backup",
            vec![record(1, 1_700_000_001_000), record(2, 1_700_000_002_000)],
        );
        assert!(cache.apply_history("sys", &doc));
        assert_eq!(cache.system("sys").unwrap().job_records("backup").len(), 2);

        // The same response again — a repeated poll, or a retry after a lost
        // acknowledgement — must be a no-op.
        assert!(!cache.apply_history("sys", &doc));
        assert_eq!(cache.system("sys").unwrap().job_records("backup").len(), 2);
        assert_eq!(cache.total_records(), 2);
    }

    #[test]
    fn history_is_fetched_on_discovery_and_on_revision_change() {
        let state = CronSystemState::unknown();
        let first = summary_document(epoch(1_000, 1), 0, &["backup"]);
        assert!(state.needs_history(&first), "first support discovery");

        let mut cache = CronCache::default();
        cache.apply_history(
            "sys",
            &history_document(epoch(1_000, 1), 0, "backup", vec![]),
        );
        cache.apply_summary("sys", first.clone(), 1);
        assert!(!cache.system("sys").unwrap().needs_history(&first));

        let changed = summary_document(epoch(1_000, 1), 1, &["backup"]);
        assert!(
            cache.system("sys").unwrap().needs_history(&changed),
            "a revision change must trigger exactly one refetch"
        );
    }

    #[test]
    fn a_revision_that_reruns_after_a_restart_still_triggers_a_fetch() {
        // The subtle case: a restarted greggd resets history_revision, and it
        // can reset it to the same small value it used before. A revision-only
        // comparison would conclude "nothing changed" and the new epoch's
        // records would never be fetched.
        let mut cache = CronCache::default();
        cache.apply_history(
            "sys",
            &history_document(
                epoch(1_000, 1),
                4,
                "backup",
                vec![record(9, 1_700_000_009_000)],
            ),
        );
        let before = summary_document(epoch(1_000, 1), 4, &["backup"]);
        cache.apply_summary("sys", before.clone(), 1);
        assert!(!cache.system("sys").unwrap().needs_history(&before));

        let after = summary_document(epoch(2_000, 2), 4, &["backup"]);
        assert!(
            cache.system("sys").unwrap().needs_history(&after),
            "a new epoch with an identical revision must still be fetched"
        );
    }

    #[test]
    fn a_new_epoch_never_rewrites_the_previous_one() {
        let mut cache = CronCache::default();
        cache.apply_summary("sys", summary_document(epoch(1_000, 1), 1, &["backup"]), 1);
        cache.apply_history(
            "sys",
            &history_document(
                epoch(1_000, 1),
                1,
                "backup",
                vec![record(1, 1_700_000_001_000)],
            ),
        );
        // The remote restarts and legitimately reissues sequence 1.
        cache.apply_summary("sys", summary_document(epoch(2_000, 2), 1, &["backup"]), 2);
        cache.apply_history(
            "sys",
            &history_document(
                epoch(2_000, 2),
                1,
                "backup",
                vec![record(1, 1_700_000_005_000)],
            ),
        );

        let retained = cache.system("sys").unwrap().job_records("backup");
        assert_eq!(retained.len(), 2, "both epochs' records are retained");
        assert_eq!(retained[0].epoch, epoch(1_000, 1));
        assert_eq!(retained[1].epoch, epoch(2_000, 2));
        assert_eq!(
            retained[0].record.sequence, retained[1].record.sequence,
            "the restarted remote reissued the same sequence"
        );
        assert_ne!(
            retained[0].identity(),
            retained[1].identity(),
            "which is exactly why a sequence-only identity would have dropped the new record"
        );
    }

    #[test]
    fn a_restart_does_not_fabricate_a_still_pending_job() {
        // "A pending job from the previous epoch is still pending" is exactly
        // the fabrication the plan forbids. State comes only from the summary,
        // so a new epoch's summary is all that is ever rendered.
        let mut cache = CronCache::default();
        let mut running = summary_document(epoch(1_000, 1), 1, &["backup"]);
        running.jobs[0].state = SchedulerJobStateV2::Running;
        cache.apply_summary("sys", running, 1);
        assert_eq!(
            cache.system("sys").unwrap().summary.as_ref().unwrap().jobs[0].state,
            SchedulerJobStateV2::Running
        );

        cache.apply_summary("sys", summary_document(epoch(2_000, 2), 0, &["backup"]), 2);
        assert_eq!(
            cache.system("sys").unwrap().summary.as_ref().unwrap().jobs[0].state,
            SchedulerJobStateV2::Idle,
            "a new epoch's state replaces the old one; nothing is carried over"
        );
    }

    #[test]
    fn per_job_depth_is_enforced_by_dropping_the_oldest() {
        let mut cache = CronCache::new(3, MAX_TOTAL_CRON_RECORDS);
        let records: Vec<SchedulerRunRecordV2> = (1..=6)
            .map(|sequence| record(sequence, 1_700_000_000_000 + sequence * 1_000))
            .collect();
        cache.apply_history(
            "sys",
            &history_document(epoch(1_000, 1), 1, "backup", records),
        );

        let retained = cache.system("sys").unwrap().job_records("backup");
        assert_eq!(retained.len(), 3);
        assert_eq!(
            retained
                .iter()
                .map(|r| r.record.sequence)
                .collect::<Vec<_>>(),
            vec![4, 5, 6],
            "the newest window survives, the oldest is dropped"
        );
    }

    #[test]
    fn the_global_ceiling_survives_a_pathological_fleet() {
        // Two systems, one job, depth 3, but a ceiling of 4: the product of
        // systems x jobs x depth must not be what decides memory.
        let mut cache = CronCache::new(3, 4);
        for system in ["a", "b"] {
            cache.apply_history(
                system,
                &history_document(
                    epoch(1_000, 1),
                    1,
                    "backup",
                    vec![
                        record(1, 1_700_000_001_000),
                        record(2, 1_700_000_002_000),
                        record(3, 1_700_000_003_000),
                    ],
                ),
            );
        }
        assert!(
            cache.total_records() <= 4,
            "{} retained",
            cache.total_records()
        );
    }

    #[test]
    fn global_eviction_prefers_the_oldest_record_fleet_wide() {
        // A ceiling of two across three systems, oldest first. The third insert
        // is the only one over the ceiling, so exactly the globally oldest
        // record goes — not a whole system, and not the newest.
        let mut cache = CronCache::new(10, 2);
        for (index, system) in ["a", "b", "c"].iter().enumerate() {
            cache.apply_history(
                system,
                &history_document(
                    epoch(1_000, 1),
                    1,
                    "backup",
                    vec![record(1, 1_700_000_001_000 + index as u64 * 1_000)],
                ),
            );
        }
        assert_eq!(cache.total_records(), 2);
        assert_eq!(cache.system("a").unwrap().job_records("backup"), &[]);
        assert_eq!(cache.system("b").unwrap().job_records("backup").len(), 1);
        assert_eq!(cache.system("c").unwrap().job_records("backup").len(), 1);
    }

    /// The alphabetically-first system must not decide the victim.
    ///
    /// Timestamps deliberately run *against* the system order here, which is
    /// the case the policy exists for: a system that sorts first but holds the
    /// newest record keeps it, and the oldest record anywhere in the fleet goes
    /// instead.
    #[test]
    fn eviction_follows_the_timestamp_even_when_system_order_disagrees() {
        let mut cache = CronCache::new(10, 2);
        // Newest first by system name: `alpha` is both alphabetically first and
        // the newest.
        for (index, system) in ["alpha", "mike", "zulu"].iter().enumerate() {
            let finished = 1_700_000_009_000 - index as u64 * 1_000;
            cache.apply_history(
                system,
                &history_document(epoch(1_000, 1), 1, "backup", vec![record(1, finished)]),
            );
        }
        assert_eq!(cache.total_records(), 2);
        assert_eq!(
            cache.system("alpha").unwrap().job_records("backup").len(),
            1,
            "the newest record must survive"
        );
        assert_eq!(cache.system("mike").unwrap().job_records("backup").len(), 1);
        assert_eq!(
            cache.system("zulu").unwrap().job_records("backup"),
            &[],
            "the globally oldest record is the victim, not the first system"
        );
    }

    #[test]
    fn eviction_is_deterministic_across_identical_runs() {
        // Hash-order-dependent eviction would make the cache unreproducible.
        fn run() -> Vec<(String, String, u64)> {
            let mut cache = CronCache::new(2, 2);
            for index in 0..4_u64 {
                let system = format!("sys-{index}");
                cache.apply_history(
                    &system,
                    &history_document(
                        epoch(1_000, 1),
                        1,
                        "backup",
                        vec![record(1, 1_700_000_000_000 + index * 1_000)],
                    ),
                );
            }
            cache
                .systems
                .iter()
                .flat_map(|(id, state)| {
                    state
                        .jobs
                        .iter()
                        .flat_map(|(job, cache)| {
                            cache
                                .records
                                .iter()
                                .map(move |r| (id.clone(), job.clone(), r.record.sequence))
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        }
        assert_eq!(run(), run());
    }

    #[test]
    fn a_job_the_remote_no_longer_serves_is_dropped() {
        let mut cache = CronCache::default();
        cache.apply_summary(
            "sys",
            summary_document(epoch(1_000, 1), 1, &["backup", "old"]),
            1,
        );
        cache.apply_history(
            "sys",
            &history_document(
                epoch(1_000, 1),
                1,
                "old",
                vec![record(1, 1_700_000_001_000)],
            ),
        );
        assert_eq!(cache.system("sys").unwrap().job_records("old").len(), 1);

        // The remote's configuration drops the job.
        cache.apply_summary("sys", summary_document(epoch(1_000, 1), 2, &["backup"]), 2);
        cache.apply_history(
            "sys",
            &history_document(epoch(1_000, 1), 2, "backup", vec![]),
        );
        assert!(
            cache.system("sys").unwrap().job_records("old").is_empty(),
            "a removed job must not keep its records alive forever"
        );
    }

    #[test]
    fn a_removed_endpoint_keeps_nothing() {
        let mut cache = CronCache::default();
        cache.apply_history(
            "gone",
            &history_document(epoch(1_000, 1), 1, "backup", vec![record(1, 1)]),
        );
        assert_eq!(cache.total_records(), 1);
        cache.retain_systems(&["kept".to_owned()]);
        assert_eq!(cache.total_records(), 0);
        assert!(cache.system("gone").is_none());
    }

    #[test]
    fn recent_returns_the_newest_window_oldest_first() {
        let mut cache = CronCache::default();
        let records: Vec<SchedulerRunRecordV2> = (1..=5)
            .map(|sequence| record(sequence, 1_700_000_000_000 + sequence * 1_000))
            .collect();
        cache.apply_history(
            "sys",
            &history_document(epoch(1_000, 1), 1, "backup", records),
        );
        let recent = cache.system("sys").unwrap().recent("backup", 2);
        assert_eq!(
            recent.iter().map(|r| r.record.sequence).collect::<Vec<_>>(),
            vec![4, 5]
        );
    }

    #[test]
    fn an_empty_or_oversized_job_name_is_refused() {
        let mut cache = CronCache::default();
        let long = "n".repeat(gregg_protocol::MAX_SCHEDULER_JOB_NAME_BYTES + 1);
        cache.apply_history(
            "sys",
            &history_document(epoch(1_000, 1), 1, "", vec![record(1, 1)]),
        );
        cache.apply_history(
            "sys",
            &history_document(epoch(1_000, 1), 1, &long, vec![record(1, 1)]),
        );
        assert!(cache.system("sys").unwrap().jobs.is_empty());
    }

    #[test]
    fn configured_depths_are_clamped_to_their_hard_maxima() {
        assert_eq!(CronCache::new(0, 0).per_job(), 1);
        assert_eq!(CronCache::new(9_999, 0).per_job(), MAX_CACHE_HISTORY);
        assert_eq!(
            CronCache::new(1, 9_999_999).total_cap(),
            MAX_TOTAL_CRON_RECORDS
        );
    }

    #[test]
    fn the_last_result_survives_in_the_summary() {
        let mut summary = summary_document(epoch(1_000, 1), 1, &["backup"]);
        summary.jobs[0].last = Some(last_summary(42));
        let mut cache = CronCache::default();
        cache.apply_summary("sys", summary, 1);
        let job = &cache.system("sys").unwrap().summary.as_ref().unwrap().jobs[0];
        assert_eq!(job.last.as_ref().map(|l| l.sequence), Some(42));
    }
}
