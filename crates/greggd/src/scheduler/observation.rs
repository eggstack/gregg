//! Plan 163: bounded, memory-only scheduler observability.
//!
//! This module owns everything the HTTP server needs to answer "what is this
//! daemon's maintenance scheduler doing?" without ever touching the execution
//! engine:
//!
//! ```text
//! execution Engine
//!   +-- authoritative per-job scheduling state
//!   +-- active child
//!   +-- bounded per-job terminal history
//!         |
//!         +-- publish compact read-only SchedulerPublication
//! ```
//!
//! Three properties matter more than anything else here:
//!
//! 1. **Memory-only.** Nothing in this module performs filesystem I/O. A
//!    daemon restart clears history and starts a new
//!    [`SchedulerEpochV2`]; there is no replay.
//! 2. **Bounded by construction.** Every buffer has a frozen capacity from
//!    `gregg-protocol` (see Plan 162). No child can make memory grow with the
//!    number of bytes it writes.
//! 3. **Never blocks execution.** Output is drained concurrently with the
//!    child, and publication is a cheap `Arc` swap. The HTTP handlers only
//!    ever read an already-serialized [`Bytes`] body.
//!
//! Publication happens on *externally visible state changes only*: the observer
//! compares the derived state against the last published state and skips the
//! swap when they match, so the Plan-160 one-minute civil-clock reconciliation
//! wake does not produce spurious publications.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use gregg_protocol::{
    output_text_from_bytes, SchedulerEpochV2, SchedulerHistoryV2, SchedulerJobHistoryV2,
    SchedulerJobStateV2, SchedulerJobV2, SchedulerLoadGateV2, SchedulerOutcomeV2,
    SchedulerOutputV2, SchedulerRunRecordV2, SchedulerRunSummaryV2, SchedulerSummaryV2,
    MAX_SCHEDULER_OUTPUT_BYTES,
};
use tokio::io::AsyncReadExt;
use tokio::sync::RwLock;

/// Per-stream read size for the output drain.
///
/// One chunk is read, folded into the fixed-capacity tail, and reused. The
/// child's pipe never accumulates more than this one scratch buffer on the
/// daemon side, so a noisy child cannot grow daemon memory.
///
/// Crate-visible so the scheduler's post-exit settle tests can size their
/// scripted readers against the real read size.
pub(crate) const DRAIN_CHUNK: usize = 4096;

/// Process-lifetime counter mixed into the epoch nonce.
///
/// Two scheduler starts in the same process (a restart test, or a config
/// reload) must not share a nonce even when they share a millisecond.
static EPOCH_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Build a restart discriminator for one scheduler lifetime.
///
/// FNV-1a over the process id, the start time, and a process-lifetime counter.
/// This mirrors the FNV-1a identity greggd already uses for its control socket,
/// so it adds no dependency and no new hashing convention. It is a
/// deduplication aid, not an authentication token.
fn make_epoch(started_at_unix_ms: u64) -> SchedulerEpochV2 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    let mut mix = |value: u64| {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
    };
    mix(u64::from(std::process::id()));
    mix(started_at_unix_ms);
    mix(EPOCH_COUNTER.fetch_add(1, Ordering::Relaxed));
    SchedulerEpochV2 {
        started_at_unix_ms,
        nonce: hash,
    }
}

/// Build an epoch for an explicit start time. Test seam.
#[cfg(test)]
fn make_epoch_at(started_at_unix_ms: u64) -> SchedulerEpochV2 {
    make_epoch(started_at_unix_ms)
}

/// Fixed-capacity tail of one child output stream.
///
/// Retains the **last** `capacity` bytes, which is the diagnostically useful
/// end of a failure. `total_bytes` counts everything the child ever wrote on
/// this stream so a test can prove retention is independent of it.
#[derive(Clone)]
pub(crate) struct OutputTail {
    buffer: Vec<u8>,
    capacity: usize,
    truncated: bool,
    total_bytes: u64,
    /// Reusable read scratch, owned here rather than inside [`drain_step`]'s
    /// future.
    ///
    /// A per-read `[u8; DRAIN_CHUNK]` declared inside the future is *inlined*
    /// into it, and the scheduler keeps up to four of these futures alive at
    /// once (two concurrent drains plus two settle branches), so the scratch
    /// alone made every enclosing future tens of kilobytes and failed clippy's
    /// `large_futures`. Parking it in the tail keeps the future's size
    /// independent of the read size, costs one allocation per stream instead of
    /// one per read, and changes nothing about what is captured.
    ///
    /// `Option` only so a fold can move the box out of `&mut self` (see
    /// [`OutputTail::push_scratch`]); it is always `Some` outside that call.
    scratch: Option<Box<[u8; DRAIN_CHUNK]>>,
}

impl std::fmt::Debug for OutputTail {
    /// Deliberately omits the scratch's contents: it is zero-filled read
    /// scratch of no diagnostic value, and 4 KiB of `0` would drown the
    /// retained tail. The field is still named so the shape stays checkable.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputTail")
            .field("buffer", &self.buffer)
            .field("capacity", &self.capacity)
            .field("truncated", &self.truncated)
            .field("total_bytes", &self.total_bytes)
            .field("scratch", &format_args!("<{DRAIN_CHUNK} bytes>"))
            .finish()
    }
}

impl OutputTail {
    /// An empty tail with the frozen per-stream capacity.
    pub(crate) fn new() -> Self {
        Self {
            // Allocated eagerly at the exact bound: a child that writes
            // megabytes never grows this.
            buffer: Vec::with_capacity(MAX_SCHEDULER_OUTPUT_BYTES),
            capacity: MAX_SCHEDULER_OUTPUT_BYTES,
            truncated: false,
            total_bytes: 0,
            scratch: Some(Box::new([0u8; DRAIN_CHUNK])),
        }
    }

    /// The reusable read scratch for the next chunk on this stream.
    pub(crate) fn scratch(&mut self) -> &mut [u8] {
        self.scratch
            .as_deref_mut()
            .expect("the drain scratch is taken only for the length of one fold")
            .as_mut_slice()
    }

    /// Fold the first `count` bytes of this tail's read scratch into the tail.
    ///
    /// The scratch box is moved out for the fold rather than read through a
    /// borrow: `push` needs `&mut self`, which a live borrow of the scratch
    /// would forbid. Moving the box is a pointer copy, so it allocates nothing.
    fn push_scratch(&mut self, count: usize) {
        let scratch = self
            .scratch
            .take()
            .expect("the drain scratch is taken only for the length of one fold");
        self.push(&scratch[..count]);
        self.scratch = Some(scratch);
    }

    /// Fold one read chunk into the tail.
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len() as u64);
        if bytes.len() >= self.capacity {
            // The chunk alone exceeds the tail: keep only its final bytes.
            self.buffer.clear();
            self.buffer
                .extend_from_slice(&bytes[bytes.len() - self.capacity..]);
            self.truncated = true;
            return;
        }
        let overflow = (self.buffer.len() + bytes.len()).saturating_sub(self.capacity);
        if overflow > 0 {
            self.buffer.drain(..overflow);
            self.truncated = true;
        }
        self.buffer.extend_from_slice(bytes);
    }

    /// Total bytes observed on this stream, regardless of how many were
    /// retained. Proves retention is independent of the child's output volume.
    #[cfg(test)]
    pub(crate) fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Convert the retained tail into the frozen wire representation.
    ///
    /// The raw bound is applied first, then lossy UTF-8, then the escaped
    /// length budget. `truncated` is true if any stage dropped bytes.
    pub(crate) fn into_wire(self) -> SchedulerOutputV2 {
        let (text, truncated) = output_text_from_bytes(&self.buffer, self.truncated);
        SchedulerOutputV2::new(text, truncated)
    }
}

impl Default for OutputTail {
    fn default() -> Self {
        Self::new()
    }
}

/// Read at most one chunk from one child stream into a fixed-capacity tail.
///
/// Returns `true` when at least one byte was folded into the tail, so a caller
/// spending a bounded budget can tell "output is still arriving" from "this
/// stream is finished". `done` records that the stream can produce no further
/// bytes — EOF, a read error, or a stream that was never piped.
///
/// `done` lives beside the tail, not inside this future, because the future is
/// rebuilt on every scheduler wake: a rebuilt drain must never wait for a pipe
/// EOF it has already observed.
///
/// A cancelled read consumes no bytes, so losing this future at a deadline
/// loses nothing already captured. The tail is **borrowed**, not created here:
/// an accumulator owned by the future would be dropped with it and every byte
/// read before the last wake would vanish — while the terminal record still
/// reported `truncated: false`. The returned future borrows the stream rather
/// than owning it, so nothing can outlive the scheduler or delay shutdown.
/// Nothing is spawned.
///
/// The read scratch lives in the tail for the same "nothing is owned here"
/// reason, so this future's size does not scale with `DRAIN_CHUNK`.
pub(crate) async fn drain_step<R>(
    reader: Option<&mut R>,
    tail: &mut OutputTail,
    done: &mut bool,
) -> bool
where
    R: AsyncReadExt + Unpin,
{
    if *done {
        return false;
    }
    let Some(reader) = reader else {
        *done = true;
        return false;
    };
    // Bound the borrow of the scratch to this statement: the read future is a
    // temporary, and the arms below need `tail` mutably again.
    let read = reader.read(tail.scratch()).await;
    match read {
        Ok(0) => {
            *done = true;
            false
        }
        Ok(count) => {
            tail.push_scratch(count);
            true
        }
        Err(error) => {
            tracing::debug!(%error, "scheduled child output stream ended with a read error");
            *done = true;
            false
        }
    }
}

/// Concurrently drain one child stream into a fixed-capacity tail to EOF.
///
/// Runs alongside the other stream and the child wait in a single
/// `tokio::join!`, so neither stream can block the other and neither can fill
/// the child's pipe while the direct child is still running. A read error ends
/// the drain and keeps whatever was already retained: losing the tail is better
/// than failing the scheduler, and the terminal record still reports the
/// child's own exit status.
pub(crate) async fn drain_into<R>(
    mut reader: Option<&mut R>,
    tail: &mut OutputTail,
    done: &mut bool,
) where
    R: AsyncReadExt + Unpin,
{
    while drain_step(reader.as_deref_mut(), tail, done).await {}
}

/// One immutable scheduler publication, shared with the HTTP server.
///
/// Both bodies are serialized at publish time so a request handler only clones
/// an `Arc` and streams known-length bytes. It never serializes, never awaits
/// scheduler mutation, and never holds a scheduler lock.
#[derive(Debug)]
pub(crate) struct SchedulerPublication {
    /// Serialized `/v2/scheduler` body.
    pub(crate) summary_bytes: Bytes,
    /// Serialized `/v2/scheduler/history` body.
    pub(crate) history_bytes: Bytes,
}

/// Handle the HTTP server holds to read the latest publication.
///
/// Cheap to clone; the inner lock is held only for an `Arc` clone.
#[derive(Debug, Clone)]
pub(crate) struct SchedulerPublisher {
    inner: Arc<RwLock<Arc<SchedulerPublication>>>,
}

impl Default for SchedulerPublisher {
    /// A default publisher already serves a valid empty scheduler document, so
    /// a server that never started a scheduler task still answers `200`.
    fn default() -> Self {
        Self::empty()
    }
}

impl SchedulerPublisher {
    /// Create a publisher already serving a valid empty scheduler document.
    ///
    /// A daemon with no configured jobs must answer `200` with an empty
    /// document rather than `404`, so the routes work before (and without) any
    /// scheduler task existing.
    pub(crate) fn empty() -> Self {
        // A zero epoch is only ever served for a daemon that has no scheduler
        // task at all, so there is nothing to deduplicate against.
        let publication = Self::try_publish(0, make_epoch(0), 0, Vec::new(), Vec::new())
            .expect("an empty scheduler document always serializes");
        Self {
            inner: Arc::new(RwLock::new(publication)),
        }
    }

    /// Serialize one coherent summary/history pair.
    ///
    /// Returns `None` if serialization ever fails. A failure must not be
    /// reported as a fabricated empty scheduler, so the caller keeps its
    /// previous publication instead of publishing a placeholder.
    fn try_publish(
        generated_at_unix_ms: u64,
        epoch: SchedulerEpochV2,
        history_revision: u64,
        jobs: Vec<SchedulerJobV2>,
        histories: Vec<SchedulerJobHistoryV2>,
    ) -> Option<Arc<SchedulerPublication>> {
        let summary = SchedulerSummaryV2 {
            schema_version: 2,
            generated_at_unix_ms,
            epoch,
            history_revision,
            jobs,
        };
        let history = SchedulerHistoryV2 {
            schema_version: 2,
            generated_at_unix_ms,
            epoch,
            history_revision,
            jobs: histories,
        };
        // Both documents are built from this daemon's own configuration and
        // bounded histories, so they cannot fail to serialize in practice.
        let (Ok(summary_bytes), Ok(history_bytes)) =
            (serde_json::to_vec(&summary), serde_json::to_vec(&history))
        else {
            return None;
        };
        Some(Arc::new(SchedulerPublication {
            summary_bytes: Bytes::from(summary_bytes),
            history_bytes: Bytes::from(history_bytes),
        }))
    }

    /// Read the latest publication.
    ///
    /// The read guard is released before the `Arc` is returned, so a handler
    /// never holds a lock across an await.
    pub(crate) async fn current(&self) -> Arc<SchedulerPublication> {
        self.inner.read().await.clone()
    }

    /// Install a new publication.
    ///
    /// Replaces the value behind the shared cell, so every existing clone —
    /// including the one the HTTP server holds — observes it without being
    /// re-wired.
    async fn store(&self, publication: Arc<SchedulerPublication>) {
        *self.inner.write().await = publication;
    }
}

/// Bounded per-job terminal history.
#[derive(Debug)]
struct JobHistory {
    name: String,
    records: VecDeque<SchedulerRunRecordV2>,
}

impl JobHistory {
    fn new(name: &str, limit: usize) -> Self {
        Self {
            name: name.to_owned(),
            // Never allocate more than the frozen bound, even for a configured
            // limit above what a job can ever reach.
            records: VecDeque::with_capacity(
                limit.min(gregg_protocol::MAX_SCHEDULER_HISTORY_LIMIT),
            ),
        }
    }
}

/// Terminal outcome of one finished occurrence, as the execution layer reports
/// it. The observer turns this into the frozen wire record.
pub(crate) struct TerminalRecord {
    /// Civil occurrence this run was scheduled for.
    pub(crate) scheduled_unix_ms: u64,
    /// When the child started; `None` for outcomes that never ran a child.
    pub(crate) started_unix_ms: Option<u64>,
    /// When the occurrence reached its terminal state.
    pub(crate) finished_unix_ms: u64,
    /// How long the occurrence waited before starting.
    pub(crate) delay_ms: u64,
    /// Whether later civil occurrences were folded into this one.
    pub(crate) coalesced: bool,
    /// Child exit code; `None` for a signal death or a non-child outcome.
    pub(crate) exit_code: Option<i32>,
    /// Terminating Unix signal.
    pub(crate) signal: Option<u32>,
    /// Child wall duration; `None` when no child ran.
    pub(crate) duration_ms: Option<u64>,
    /// Bounded stdout tail.
    pub(crate) stdout: OutputTail,
    /// Bounded stderr tail.
    pub(crate) stderr: OutputTail,
}

impl TerminalRecord {
    /// An occurrence that never created a child (spawn failure, load expiry).
    pub(crate) fn without_child(
        scheduled_unix_ms: u64,
        finished_unix_ms: u64,
        delay_ms: u64,
        coalesced: bool,
        outcome: SchedulerOutcomeV2,
    ) -> Self {
        debug_assert!(
            matches!(
                outcome,
                SchedulerOutcomeV2::SpawnFailed | SchedulerOutcomeV2::LoadExpired
            ),
            "only non-child outcomes may omit a start time"
        );
        Self {
            scheduled_unix_ms,
            started_unix_ms: None,
            finished_unix_ms,
            delay_ms,
            coalesced,
            exit_code: None,
            signal: None,
            duration_ms: None,
            stdout: OutputTail::new(),
            stderr: OutputTail::new(),
        }
    }
}

/// Owns the daemon-lifetime epoch, the bounded history, and publication.
///
/// Deliberately separate from the metrics [`ServerState`] publication: history
/// changes only at scheduler state transitions, while metrics are republished
/// every sample. Sharing one lock would couple a low-frequency document to a
/// high-frequency write path for no benefit.
#[derive(Debug)]
pub(crate) struct SchedulerObserver {
    epoch: SchedulerEpochV2,
    limit: usize,
    histories: Vec<JobHistory>,
    next_sequence: u64,
    history_revision: u64,
    publisher: SchedulerPublisher,
    /// Comparable state of the last publication, so an unchanged scheduler
    /// wake performs no swap and no serialization.
    last_published: Option<PublishedComparable>,
}

/// The part of a publication that decides whether state actually changed.
///
/// `generated_at_unix_ms` is deliberately excluded: including it would make
/// every comparison differ and defeat the whole point.
#[derive(Debug, PartialEq)]
struct PublishedComparable {
    epoch: SchedulerEpochV2,
    history_revision: u64,
    jobs: Vec<SchedulerJobV2>,
}

impl SchedulerObserver {
    /// Create an observer for one scheduler lifetime.
    ///
    /// `limit` is the validated per-job depth; `0` keeps live state and
    /// retains no records.
    pub(crate) fn new(
        job_names: &[String],
        limit: usize,
        started_at_unix_ms: u64,
        publisher: SchedulerPublisher,
    ) -> Self {
        Self {
            epoch: make_epoch(started_at_unix_ms),
            limit,
            histories: job_names
                .iter()
                .map(|name| JobHistory::new(name, limit))
                .collect(),
            next_sequence: 1,
            history_revision: 0,
            publisher,
            last_published: None,
        }
    }

    /// The shared publication cell, for deterministic document proofs.
    #[cfg(test)]
    pub(crate) fn publication_handle(&self) -> SchedulerPublisher {
        self.publisher.clone()
    }

    /// Whether child output should be piped at all.
    pub(crate) fn capture_enabled(&self) -> bool {
        self.limit > 0
    }

    /// Record a terminal occurrence and publish the new history revision.
    ///
    /// Returns the assigned sequence. Sequence is monotonic within the
    /// scheduler lifetime, which is what makes `(epoch, sequence)` a usable
    /// client deduplication identity.
    pub(crate) fn record_terminal(
        &mut self,
        index: usize,
        record: TerminalRecord,
        outcome: SchedulerOutcomeV2,
    ) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        if self.limit == 0 {
            // History disabled: still advance the revision so a client can
            // observe that a run completed even though nothing is retained.
            self.history_revision = self.history_revision.saturating_add(1);
            return sequence;
        }
        let Some(history) = self.histories.get_mut(index) else {
            return sequence;
        };
        let wire = SchedulerRunRecordV2 {
            sequence,
            scheduled_unix_ms: record.scheduled_unix_ms,
            started_unix_ms: record.started_unix_ms,
            finished_unix_ms: record.finished_unix_ms,
            outcome,
            exit_code: record.exit_code,
            signal: record.signal,
            duration_ms: record.duration_ms,
            delay_ms: record.delay_ms,
            coalesced: record.coalesced,
            stdout: record.stdout.into_wire(),
            stderr: record.stderr.into_wire(),
        };
        history.records.push_back(wire);
        while history.records.len() > self.limit {
            history.records.pop_front();
        }
        self.history_revision = self.history_revision.saturating_add(1);
        sequence
    }

    /// The most recent terminal record summary for one job, for the summary
    /// document's at-a-glance row.
    pub(crate) fn last_summary(&self, index: usize) -> Option<SchedulerRunSummaryV2> {
        let record = self.histories.get(index)?.records.back()?;
        Some(SchedulerRunSummaryV2 {
            sequence: record.sequence,
            scheduled_unix_ms: record.scheduled_unix_ms,
            finished_unix_ms: record.finished_unix_ms,
            outcome: record.outcome,
            exit_code: record.exit_code,
            signal: record.signal,
            duration_ms: record.duration_ms,
            delay_ms: record.delay_ms,
            coalesced: record.coalesced,
        })
    }

    /// Publish current live state, but only when it differs from the last
    /// publication.
    ///
    /// Returns `true` when a new publication was installed.
    pub(crate) async fn publish(
        &mut self,
        jobs: Vec<SchedulerJobV2>,
        generated_at_unix_ms: u64,
    ) -> bool {
        // Compared field by field against the last publication so the common
        // "nothing changed" wake costs no allocation and no clone.
        if self.last_published.as_ref().is_some_and(|previous| {
            previous.epoch == self.epoch
                && previous.history_revision == self.history_revision
                && previous.jobs == jobs
        }) {
            return false;
        }
        let histories = if self.limit == 0 {
            // A disabled history still reports the configured job names with
            // empty record lists, so the document shape is stable.
            self.histories
                .iter()
                .map(|history| SchedulerJobHistoryV2 {
                    name: history.name.clone(),
                    records: Vec::new(),
                })
                .collect()
        } else {
            self.histories
                .iter()
                .map(|history| SchedulerJobHistoryV2 {
                    name: history.name.clone(),
                    records: history.records.iter().cloned().collect(),
                })
                .collect()
        };
        // The comparison key is only built on the path that actually publishes,
        // where one clone is noise next to serializing two documents. The
        // unchanged-wake path above returns before reaching it.
        let comparable = PublishedComparable {
            epoch: self.epoch,
            history_revision: self.history_revision,
            jobs: jobs.clone(),
        };
        // A serialization failure keeps the previous publication rather than
        // degrading into a fabricated empty scheduler document.
        let Some(next) = SchedulerPublisher::try_publish(
            generated_at_unix_ms,
            self.epoch,
            self.history_revision,
            jobs,
            histories,
        ) else {
            return false;
        };
        self.publisher.store(next).await;
        self.last_published = Some(comparable);
        true
    }
}

/// Derive the current wire state of one job from scheduler-owned facts.
///
/// `pending_since_unix_ms` and `next_retry_unix_ms` come from the engine's own
/// pending occurrence, never from client polling timestamps, so pending age and
/// reason are authoritative.
pub(crate) fn job_state(
    running: bool,
    pending: bool,
    gate: Option<&SchedulerLoadGateV2>,
) -> SchedulerJobStateV2 {
    if running {
        return SchedulerJobStateV2::Running;
    }
    if !pending {
        return SchedulerJobStateV2::Idle;
    }
    match gate {
        // No load decision yet, or one that currently allows the job: the
        // occurrence is waiting for the single global child slot. It must not
        // be rendered as load-delayed.
        None => SchedulerJobStateV2::WaitingForSlot,
        Some(gate) if gate.observed.is_none() => SchedulerJobStateV2::LoadUnavailable,
        Some(_) => SchedulerJobStateV2::LoadHigh,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::*;
    use gregg_protocol::{MAX_SCHEDULER_HISTORY_LIMIT, MAX_SCHEDULER_OUTPUT_TEXT_BYTES};

    fn names(count: usize) -> Vec<String> {
        (0..count).map(|index| format!("job-{index}")).collect()
    }

    fn idle_job(name: &str) -> SchedulerJobV2 {
        SchedulerJobV2 {
            name: name.to_owned(),
            schedule: "0 3 * * *".to_owned(),
            next_due_unix_ms: 1_700_000_000_000,
            state: SchedulerJobStateV2::Idle,
            load: None,
            pending_since_unix_ms: None,
            next_retry_unix_ms: None,
            running_since_unix_ms: None,
            last: None,
        }
    }

    fn success_record() -> TerminalRecord {
        TerminalRecord {
            scheduled_unix_ms: 1_700_000_000_000,
            started_unix_ms: Some(1_700_000_001_000),
            finished_unix_ms: 1_700_000_002_000,
            delay_ms: 1_000,
            coalesced: false,
            exit_code: Some(0),
            signal: None,
            duration_ms: Some(1_000),
            stdout: OutputTail::new(),
            stderr: OutputTail::new(),
        }
    }

    #[test]
    fn epoch_nonce_varies_between_lifetimes() {
        let first = make_epoch_at(1_700_000_000_000);
        let second = make_epoch_at(1_700_000_000_000);
        assert_ne!(
            first.nonce, second.nonce,
            "two lifetimes in one process must not share a nonce"
        );
        assert_eq!(first.started_at_unix_ms, second.started_at_unix_ms);
    }

    #[test]
    fn output_tail_keeps_the_final_bytes() {
        let mut tail = OutputTail::new();
        tail.push(b"abcdefghij");
        let wire = tail.into_wire();
        assert_eq!(wire.text, "abcdefghij");
        assert!(!wire.truncated);
    }

    #[test]
    fn output_tail_stays_at_the_frozen_capacity_under_a_large_stream() {
        let mut tail = OutputTail::new();
        // 1 MiB written through realistic 4 KiB reads.
        for _ in 0..256 {
            tail.push(&[b'x'; DRAIN_CHUNK]);
        }
        assert_eq!(tail.total_bytes(), 256 * DRAIN_CHUNK as u64);
        // The backing allocation never exceeds the frozen capacity, however
        // much the child wrote.
        assert!(tail.buffer.capacity() <= MAX_SCHEDULER_OUTPUT_BYTES);
        let wire = tail.into_wire();
        assert!(
            wire.truncated,
            "a stream past the cap must be marked truncated"
        );
        // Retention is independent of how much was written.
        assert!(
            wire.text.len() <= MAX_SCHEDULER_OUTPUT_TEXT_BYTES + 4,
            "retained {} bytes",
            wire.text.len()
        );
    }

    #[test]
    fn a_single_chunk_larger_than_the_cap_keeps_only_its_tail() {
        let mut tail = OutputTail::new();
        let big = vec![b'y'; MAX_SCHEDULER_OUTPUT_BYTES * 3];
        tail.push(&big);
        // The raw tail keeps the final `MAX_SCHEDULER_OUTPUT_BYTES`, and the
        // published text is then capped separately at the escaped budget.
        assert_eq!(tail.buffer.len(), MAX_SCHEDULER_OUTPUT_BYTES);
        let wire = tail.into_wire();
        assert!(wire.truncated);
        assert_eq!(wire.text.len(), MAX_SCHEDULER_OUTPUT_TEXT_BYTES);
    }

    #[test]
    fn stdout_and_stderr_truncate_independently() {
        let mut stdout = OutputTail::new();
        let mut stderr = OutputTail::new();
        stdout.push(&vec![b'o'; MAX_SCHEDULER_OUTPUT_BYTES * 2]);
        stderr.push(b"short diagnostic");
        assert!(stdout.into_wire().truncated);
        assert!(!stderr.into_wire().truncated);
    }

    #[tokio::test]
    async fn drain_reads_a_stream_to_eof() {
        let data: &'static [u8] = b"line one\nline two\n";
        let mut cursor = std::io::Cursor::new(data);
        let mut tail = OutputTail::new();
        let mut done = false;
        drain_into(Some(&mut cursor), &mut tail, &mut done).await;
        assert!(done, "a stream read to EOF must be marked finished");
        assert_eq!(tail.into_wire().text, "line one\nline two\n");
    }

    #[tokio::test]
    async fn drain_handles_a_missing_stream() {
        let mut tail = OutputTail::new();
        let mut done = false;
        drain_into(None::<&mut std::io::Cursor<Vec<u8>>>, &mut tail, &mut done).await;
        assert!(
            done,
            "a stream that was never piped has nothing to wait for"
        );
        assert_eq!(tail.into_wire().text, "");
    }

    /// A finished stream must stay finished across a rebuilt drain: the
    /// post-exit settle phase re-enters this state, and a stream that already
    /// reached EOF has to report that instead of waiting again.
    #[tokio::test]
    async fn a_drain_step_on_a_finished_stream_makes_no_progress() {
        let data: &'static [u8] = b"one\ntwo\n";
        let mut cursor = std::io::Cursor::new(data);
        let mut tail = OutputTail::new();
        let mut done = false;
        assert!(drain_step(Some(&mut cursor), &mut tail, &mut done).await);
        assert!(!done, "one chunk is not EOF");
        assert!(
            !drain_step(Some(&mut cursor), &mut tail, &mut done).await,
            "the end of the stream is not progress"
        );
        assert!(done);
        assert!(
            !drain_step(Some(&mut cursor), &mut tail, &mut done).await,
            "a finished stream cannot make progress again"
        );
        assert!(done);
        assert_eq!(tail.into_wire().text, "one\ntwo\n");
    }

    /// A drain cancelled by a scheduler wake and then re-entered must keep what
    /// it already read. This is the mechanism that stops a child running longer
    /// than one civil-clock recheck from publishing an empty tail.
    #[tokio::test]
    async fn a_cancelled_drain_keeps_what_it_already_folded_in() {
        let data: &'static [u8] = b"first line\nsecond line\n";
        let mut cursor = std::io::Cursor::new(data);
        let mut tail = OutputTail::new();
        let mut done = false;
        // First wake: one read reaches the scheduler, then a deadline wake
        // cancels the drain. Only bytes already folded into `tail` survive it.
        let mut chunk = [0u8; 11];
        let count = cursor.read(&mut chunk).await.expect("a cursor never fails");
        tail.push(&chunk[..count]);
        assert_eq!(tail.total_bytes(), 11);

        // Second wake: the rebuilt drain continues into the *same* tail, so the
        // record carries the whole run rather than only the post-wake bytes.
        drain_into(Some(&mut cursor), &mut tail, &mut done).await;
        let wire = tail.into_wire();
        assert_eq!(wire.text, "first line\nsecond line\n");
        assert!(
            !wire.truncated,
            "a complete short run must not claim output was dropped"
        );
    }

    #[test]
    fn history_ring_evicts_the_oldest_at_the_qualified_depth() {
        let mut observer = SchedulerObserver::new(
            &names(1),
            MAX_SCHEDULER_HISTORY_LIMIT,
            1_700_000_000_000,
            SchedulerPublisher::empty(),
        );
        for _ in 0..(MAX_SCHEDULER_HISTORY_LIMIT + 3) {
            observer.record_terminal(0, success_record(), SchedulerOutcomeV2::Success);
        }
        let history = &observer.histories[0].records;
        assert_eq!(history.len(), MAX_SCHEDULER_HISTORY_LIMIT);
        // Oldest evicted, newest retained, sequences still contiguous.
        assert_eq!(history.front().unwrap().sequence, 4);
        assert_eq!(
            history.back().unwrap().sequence,
            MAX_SCHEDULER_HISTORY_LIMIT as u64 + 3
        );
    }

    #[test]
    fn history_revision_advances_only_with_retained_history() {
        let mut observer =
            SchedulerObserver::new(&names(1), 5, 1_700_000_000_000, SchedulerPublisher::empty());
        assert_eq!(observer.history_revision, 0);
        observer.record_terminal(0, success_record(), SchedulerOutcomeV2::Success);
        assert_eq!(observer.history_revision, 1);
        observer.record_terminal(0, success_record(), SchedulerOutcomeV2::Failed);
        assert_eq!(observer.history_revision, 2);
    }

    #[test]
    fn disabled_history_retains_nothing_but_still_advances_the_revision() {
        let mut observer =
            SchedulerObserver::new(&names(1), 0, 1_700_000_000_000, SchedulerPublisher::empty());
        assert!(!observer.capture_enabled());
        observer.record_terminal(0, success_record(), SchedulerOutcomeV2::Success);
        assert!(observer.histories[0].records.is_empty());
        assert_eq!(observer.history_revision, 1);
    }

    #[test]
    fn non_child_outcomes_omit_start_time_and_duration() {
        let mut observer =
            SchedulerObserver::new(&names(1), 5, 1_700_000_000_000, SchedulerPublisher::empty());
        observer.record_terminal(
            0,
            TerminalRecord::without_child(
                1_700_000_000_000,
                1_700_000_060_000,
                60_000,
                false,
                SchedulerOutcomeV2::LoadExpired,
            ),
            SchedulerOutcomeV2::LoadExpired,
        );
        let record = &observer.histories[0].records[0];
        assert_eq!(record.outcome, SchedulerOutcomeV2::LoadExpired);
        assert_eq!(record.started_unix_ms, None);
        assert_eq!(record.duration_ms, None);
        assert_eq!(record.exit_code, None);
    }

    #[tokio::test]
    async fn unchanged_state_publishes_nothing() {
        let mut observer =
            SchedulerObserver::new(&names(2), 5, 1_700_000_000_000, SchedulerPublisher::empty());
        let jobs = vec![idle_job("job-0"), idle_job("job-1")];
        assert!(observer.publish(jobs.clone(), 1_000).await);
        assert!(
            !observer.publish(jobs.clone(), 2_000).await,
            "a different timestamp alone must not republish"
        );
        assert!(
            !observer.publish(jobs.clone(), 3_000).await,
            "an unchanged reconciliation wake must not republish"
        );
    }

    #[tokio::test]
    async fn a_live_state_change_publishes() {
        let mut observer =
            SchedulerObserver::new(&names(1), 5, 1_700_000_000_000, SchedulerPublisher::empty());
        assert!(observer.publish(vec![idle_job("job-0")], 1_000).await);
        let mut delayed = idle_job("job-0");
        delayed.state = SchedulerJobStateV2::LoadHigh;
        delayed.pending_since_unix_ms = Some(900);
        delayed.load = Some(SchedulerLoadGateV2 {
            window: "15m".to_owned(),
            threshold: 8.0,
            observed: Some(9.5),
        });
        delayed.next_retry_unix_ms = Some(1_100);
        assert!(observer.publish(vec![delayed], 2_000).await);
    }

    #[tokio::test]
    async fn a_terminal_record_publishes_a_new_revision() {
        let mut observer =
            SchedulerObserver::new(&names(1), 5, 1_700_000_000_000, SchedulerPublisher::empty());
        assert!(observer.publish(vec![idle_job("job-0")], 1_000).await);
        observer.record_terminal(0, success_record(), SchedulerOutcomeV2::Success);
        let mut ran = idle_job("job-0");
        ran.last = observer.last_summary(0);
        assert!(observer.publish(vec![ran], 2_000).await);
    }

    #[tokio::test]
    async fn published_documents_are_valid_and_bounded() {
        let mut observer =
            SchedulerObserver::new(&names(2), 5, 1_700_000_000_000, SchedulerPublisher::empty());
        observer.record_terminal(0, success_record(), SchedulerOutcomeV2::Success);
        observer
            .publish(
                vec![idle_job("job-0"), idle_job("job-1")],
                1_700_000_000_000,
            )
            .await;
        let publication = observer.publisher.current().await;
        let summary: SchedulerSummaryV2 =
            serde_json::from_slice(&publication.summary_bytes).expect("valid summary");
        let history: SchedulerHistoryV2 =
            serde_json::from_slice(&publication.history_bytes).expect("valid history");
        summary.validate().expect("summary validates");
        history.validate().expect("history validates");
        assert_eq!(summary.jobs.len(), 2);
        assert_eq!(history.jobs[0].records.len(), 1);
        assert_eq!(summary.history_revision, 1);
        assert!(!gregg_protocol::history_body_exceeds_budget(&history));
    }

    #[tokio::test]
    async fn empty_publisher_serves_a_valid_empty_document() {
        let publication = SchedulerPublisher::empty().current().await;
        let summary: SchedulerSummaryV2 =
            serde_json::from_slice(&publication.summary_bytes).expect("valid empty summary");
        assert_eq!(summary.jobs.len(), 0);
        assert!(summary.validate().is_ok());
    }

    #[test]
    fn state_derivation_separates_slot_wait_from_load_delay() {
        let high = SchedulerLoadGateV2 {
            window: "15m".to_owned(),
            threshold: 8.0,
            observed: Some(9.5),
        };
        let missing = SchedulerLoadGateV2 {
            window: "15m".to_owned(),
            threshold: 8.0,
            observed: None,
        };
        assert_eq!(job_state(false, false, None), SchedulerJobStateV2::Idle);
        assert_eq!(
            job_state(false, true, None),
            SchedulerJobStateV2::WaitingForSlot
        );
        assert_eq!(
            job_state(false, true, Some(&high)),
            SchedulerJobStateV2::LoadHigh
        );
        assert_eq!(
            job_state(false, true, Some(&missing)),
            SchedulerJobStateV2::LoadUnavailable
        );
        assert_eq!(
            job_state(true, true, Some(&high)),
            SchedulerJobStateV2::Running
        );
    }
}
