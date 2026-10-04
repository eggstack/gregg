//! Plan 162: scheduler-observability wire contract for schema version 2.
//!
//! These types are the *frozen* remote contract (Plan 162 questions 1-7) served
//! on the additive read-only routes:
//!
//! - `GET/HEAD /v2/scheduler` — [`SchedulerSummaryV2`]
//! - `GET/HEAD /v2/scheduler/history` — [`SchedulerHistoryV2`]
//!
//! Scheduler documents are deliberately kept out of `StatusPayloadV2`:
//! metrics are polled at a high cadence while history carries bounded command
//! output, so merging them would make every ordinary metrics poll materially
//! larger. A client polls the summary cheaply and fetches history only when the
//! summary's [`SchedulerSummaryV2::history_revision`] changes.
//!
//! # Frozen resource constants
//!
//! Every number below was fixed by Plan 162 and must not be widened by later
//! implementation judgment. The derivation and the measured footprint rule live
//! in `plans/162-scheduler-observability-contract-and-resource-qualification.md`.
//!
//! | Constant | Value | Role |
//! | --- | --- | --- |
//! | [`MAX_SCHEDULER_JOBS`] | 64 | mirrors the daemon's `MAX_JOBS` |
//! | [`DEFAULT_SCHEDULER_HISTORY_LIMIT`] | 5 | per-job retained terminal records |
//! | [`MAX_SCHEDULER_HISTORY_LIMIT`] | 10 | hard per-job maximum |
//! | [`MAX_SCHEDULER_OUTPUT_BYTES`] | 1024 | raw captured tail, per stream |
//! | [`MAX_SCHEDULER_OUTPUT_TEXT_BYTES`] | 512 | published text, per stream, **JSON-escaped** |
//! | [`MAX_SCHEDULER_HISTORY_BODY_BYTES`] | 1 MiB | client body cap |
//!
//! # Why the text cap is escaped-byte based
//!
//! `serde_json` renders a C0 control byte as six bytes (`\u00XX`), so a naive
//! "512 bytes of text" cap could still emit 3 KiB of JSON for a single stream.
//! [`json_escaped_len`] and [`truncate_to_escaped_budget`] make the published
//! length an exact function of the character sequence, so the maximum history
//! body is bounded *by construction* rather than discovered after
//! serialization. See [`MAX_SCHEDULER_HISTORY_BODY_BYTES`].
//!
//! # Truthfulness rules
//!
//! - Absent values are `None`, never a fabricated zero. `exit_code` is `None`
//!   for a signal death, `duration_ms` is `None` for an outcome that never ran a
//!   child, and `observed_load` is `None` when load telemetry was unavailable.
//! - A load-expired occurrence and a spawn failure are terminal records even
//!   though no child ever existed.
//! - Command `argv`, `working_dir`, environment, and service identity are
//!   never published. The operator-facing name and schedule are sufficient to
//!   identify a job, and the existing LAN listener is unauthenticated.

use serde::{Deserialize, Serialize};

/// Maximum number of jobs in one scheduler document.
///
/// Mirrors the daemon's own `MAX_JOBS` configuration bound so a document that
/// validates can always be produced by a conforming daemon.
pub const MAX_SCHEDULER_JOBS: usize = 64;

/// Default number of retained terminal records per job.
pub const DEFAULT_SCHEDULER_HISTORY_LIMIT: usize = 5;

/// Hard maximum number of retained terminal records per job.
///
/// Chosen (Plan 162 question 4) so the pathological worst case — 64 jobs at
/// this depth — retains at most `64 * 10 * 2 * 512` = 655,360 bytes of
/// published text plus bounded per-record metadata, which stays proportionate
/// to Gregg's lightweight daemon target.
pub const MAX_SCHEDULER_HISTORY_LIMIT: usize = 10;

/// Raw captured output tail retained per stream, in bytes.
///
/// Selected over 256 and 4096 (Plan 162 question 5): 256 truncates ordinary
/// multi-line diagnostics too aggressively, while 4096 doubles the worst-case
/// retained bytes for no diagnostic benefit once the published text is capped
/// at [`MAX_SCHEDULER_OUTPUT_TEXT_BYTES`].
pub const MAX_SCHEDULER_OUTPUT_BYTES: usize = 1024;

/// Maximum published output text per stream, measured in **JSON-escaped**
/// bytes.
///
/// Bounding the escaped form is what makes the history body maximum a closed
/// calculation. The raw capture cap is larger so that a lossy UTF-8 conversion
/// has room to keep useful tail text after replacing invalid sequences.
pub const MAX_SCHEDULER_OUTPUT_TEXT_BYTES: usize = 512;

/// Maximum UTF-8 byte length of a scheduler job name.
pub const MAX_SCHEDULER_JOB_NAME_BYTES: usize = 128;

/// Maximum UTF-8 byte length of a configured schedule string.
pub const MAX_SCHEDULER_SCHEDULE_BYTES: usize = 128;

/// Maximum response body Gregg's remote history client accepts.
///
/// Derived maximum (`MAX_SCHEDULER_JOBS * MAX_SCHEDULER_HISTORY_LIMIT`
/// records, each carrying two streams at [`MAX_SCHEDULER_OUTPUT_TEXT_BYTES`]
/// plus bounded metadata) is ≈ 812 KiB; 1 MiB leaves bounded margin for JSON
/// envelope and key overhead without permitting an unbounded body.
pub const MAX_SCHEDULER_HISTORY_BODY_BYTES: usize = 1024 * 1024;

/// Maximum response body Gregg's remote scheduler-summary client accepts.
///
/// A summary carries no command output: at most 64 jobs of a few bounded
/// scalar fields. 64 KiB is orders of magnitude above that shape and is the
/// same order as the existing v2 status payload cap.
pub const MAX_SCHEDULER_SUMMARY_BODY_BYTES: usize = 64 * 1024;

/// Hard deserialization bound for the per-job record list.
///
/// Twice [`MAX_SCHEDULER_HISTORY_LIMIT`] so a slightly over-limit document
/// still deserializes and receives detailed [`SchedulerHistoryV2::validate`]
/// diagnostics, while a hostile list is rejected during serde with a bounded
/// allocation.
const MAX_DESERIALIZE_RECORDS: usize = MAX_SCHEDULER_HISTORY_LIMIT * 2;

/// Deserialize a `Vec<T>` with a streaming length cap.
///
/// The visitor errors as soon as more than `cap` elements arrive, so the
/// allocation stays bounded even for hostile input. Serialization is unchanged.
fn deserialize_capped_vec<'de, D, T>(deserializer: D, cap: usize) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct CappedVecVisitor<T>(usize, std::marker::PhantomData<T>);

    impl<'de, T> serde::de::Visitor<'de> for CappedVecVisitor<T>
    where
        T: Deserialize<'de>,
    {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "a list of at most {} entries", self.0)
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let cap = self.0;
            let mut items: Vec<T> = Vec::new();
            while let Some(item) = seq.next_element::<T>()? {
                if items.len() == cap {
                    return Err(serde::de::Error::custom(format!(
                        "list exceeds the {cap}-entry deserialization cap"
                    )));
                }
                items.push(item);
            }
            Ok(items)
        }
    }

    deserializer.deserialize_seq(CappedVecVisitor(cap, std::marker::PhantomData))
}

fn deserialize_capped_jobs<'de, D>(deserializer: D) -> Result<Vec<SchedulerJobV2>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_capped_vec(deserializer, MAX_SCHEDULER_JOBS * 2)
}

fn deserialize_capped_job_histories<'de, D>(
    deserializer: D,
) -> Result<Vec<SchedulerJobHistoryV2>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_capped_vec(deserializer, MAX_SCHEDULER_JOBS * 2)
}

fn deserialize_capped_records<'de, D>(
    deserializer: D,
) -> Result<Vec<SchedulerRunRecordV2>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_capped_vec(deserializer, MAX_DESERIALIZE_RECORDS)
}

/// Restart discriminator for one greggd scheduler lifetime.
///
/// This is a **deduplication** aid, not an authentication token. A polling
/// client pairs `epoch` with [`SchedulerRunRecordV2::sequence`] to suppress
/// duplicate records across polls and to detect a remote daemon restart, at
/// which point sequence values legitimately restart from zero.
///
/// `nonce` exists because a wall-clock start time alone can collide: two
/// restarts inside the same millisecond would otherwise be indistinguishable.
/// It is an FNV-1a mix of the process id, the start time, and a
/// process-lifetime counter — cheap, dependency-free, and adequate for
/// distinguishing two concurrently or rapidly restarted daemons. The recorded
/// collision assumption is that a peer can be indistinguishable only when a new
/// process starts in the same millisecond as the previous one *and* reuses the
/// same pid *and* the same counter value, which is not reachable in practice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SchedulerEpochV2 {
    /// Unix epoch in milliseconds when this scheduler lifetime began.
    pub started_at_unix_ms: u64,
    /// Process-lifetime disambiguator for same-millisecond restarts.
    pub nonce: u64,
}

/// Current scheduling state of one configured job.
///
/// An explicit enum, never a free-form string assembled by the frontend: the
/// client must not be able to render a waiting-for-slot job as load-delayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerJobStateV2 {
    /// Not due and not pending.
    Idle,
    /// Due and pending, waiting for the single global child slot.
    WaitingForSlot,
    /// Due and pending, delayed because observed load exceeds the threshold.
    LoadHigh,
    /// Due and pending, delayed because required load telemetry is unavailable.
    ///
    /// Distinct from [`Self::LoadHigh`]: a missing load reading is not a low
    /// one, and the frontend must not render this as `0`.
    LoadUnavailable,
    /// A child for this job is currently executing.
    Running,
}

/// Terminal outcome of one scheduled occurrence.
///
/// Non-child outcomes are first-class: a recent-runs display that silently
/// omitted failed-to-start or expired occurrences would not be truthful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerOutcomeV2 {
    /// Child ran and exited `0`.
    Success,
    /// Child ran and exited nonzero or was terminated abnormally.
    Failed,
    /// The child could not be created (missing executable, permission, ...).
    SpawnFailed,
    /// The child was created but its exit status could not be observed.
    WaitFailed,
    /// The occurrence expired waiting for load; no child ever ran.
    LoadExpired,
    /// The daemon shut down and terminated the active child.
    ///
    /// Only published when the record survives long enough to be served. A
    /// daemon shutdown clears memory-only history, so a remote client may
    /// never observe this outcome for the final occurrence of a lifetime.
    Cancelled,
}

/// Load-gate context for a load-gated job.
///
/// `observed_load` is `None` whenever the load decision was made without a
/// usable reading, which is what distinguishes
/// [`SchedulerJobStateV2::LoadUnavailable`] from a genuinely low load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SchedulerLoadGateV2 {
    /// Configured load window: `1m`, `5m`, or `15m`.
    pub window: String,
    /// Configured inclusive threshold.
    pub threshold: f32,
    /// Observed load for `window`, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<f32>,
}

/// Compact description of a job's most recent terminal occurrence.
///
/// Carried in the summary so the at-a-glance cron view needs no history fetch;
/// the full record (including output tails) is in the history document.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SchedulerRunSummaryV2 {
    /// Scheduler-lifetime monotonic record sequence.
    pub sequence: u64,
    /// Civil occurrence this run was scheduled for.
    pub scheduled_unix_ms: u64,
    /// When the occurrence reached a terminal state.
    pub finished_unix_ms: u64,
    /// Terminal outcome.
    pub outcome: SchedulerOutcomeV2,
    /// Child exit code; `None` for a signal death or a non-child outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Terminating Unix signal; `None` off Unix or for a normal exit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<u32>,
    /// Child wall duration; `None` when no child ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// How long the occurrence waited before starting.
    pub delay_ms: u64,
    /// Whether later civil occurrences were folded into this one.
    pub coalesced: bool,
}

/// Bounded captured output for one stream.
///
/// `text` never exceeds [`MAX_SCHEDULER_OUTPUT_TEXT_BYTES`] **JSON-escaped**
/// bytes. `truncated` is independent per stream so a noisy stdout cannot hide
/// that stderr was also cut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SchedulerOutputV2 {
    /// Lossy, JSON-safe tail of the stream.
    pub text: String,
    /// Whether the raw or the published bound truncated this stream.
    pub truncated: bool,
}

impl SchedulerOutputV2 {
    /// Build an output record from already-bounded text.
    #[must_use]
    pub fn new(text: String, truncated: bool) -> Self {
        Self { text, truncated }
    }
}

/// One job's current state as published by the summary document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SchedulerJobV2 {
    /// Stable operator-facing job name.
    pub name: String,
    /// Configured five-field schedule string (or one of the existing aliases).
    pub schedule: String,
    /// Next civil occurrence.
    pub next_due_unix_ms: u64,
    /// Current scheduling state.
    pub state: SchedulerJobStateV2,
    /// Load-gate context; `None` for a time-only job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load: Option<SchedulerLoadGateV2>,
    /// When the current pending occurrence was first observed; `None` unless
    /// pending or running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_since_unix_ms: Option<u64>,
    /// Next load-gate retry; present only while load-delayed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_retry_unix_ms: Option<u64>,
    /// Active start time; present only while running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_since_unix_ms: Option<u64>,
    /// Most recent terminal outcome summary, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<SchedulerRunSummaryV2>,
}

/// Response body of `GET/HEAD /v2/scheduler`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SchedulerSummaryV2 {
    /// Schema major version; always `2`.
    pub schema_version: u16,
    /// Unix epoch in milliseconds when the document was produced.
    pub generated_at_unix_ms: u64,
    /// Current scheduler lifetime.
    pub epoch: SchedulerEpochV2,
    /// Counter that changes whenever retained terminal history changes.
    ///
    /// This is the client's only trigger to refetch the history document. It
    /// does **not** change for live-state transitions alone, so a job that
    /// moves to load-delayed does not force a history download.
    pub history_revision: u64,
    /// One entry per configured job; empty when the daemon has no jobs.
    #[serde(default, deserialize_with = "deserialize_capped_jobs")]
    pub jobs: Vec<SchedulerJobV2>,
}

impl SchedulerSummaryV2 {
    /// Validate that every field satisfies the scheduler wire invariants.
    ///
    /// Returns `Ok(())` or a list of structured violations.
    ///
    /// # Errors
    ///
    /// Returns every violation found, so a caller can report a complete
    /// diagnosis rather than only the first problem.
    pub fn validate(&self) -> Result<(), Vec<crate::ValidationViolationScheduler>> {
        crate::validate_scheduler::validate_summary(self)
    }
}

/// One job's bounded terminal history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SchedulerJobHistoryV2 {
    /// Job name; matches a [`SchedulerJobV2::name`] in the summary document.
    pub name: String,
    /// Retained terminal records, oldest first.
    #[serde(default, deserialize_with = "deserialize_capped_records")]
    pub records: Vec<SchedulerRunRecordV2>,
}

/// One terminal record of one scheduled occurrence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SchedulerRunRecordV2 {
    /// Scheduler-lifetime monotonic record sequence.
    ///
    /// Deduplication identity is `(epoch, sequence)`. A record is immutable once
    /// published, so a client that has seen `(epoch, sequence)` never needs to
    /// re-apply it.
    pub sequence: u64,
    /// Civil occurrence this run was scheduled for.
    pub scheduled_unix_ms: u64,
    /// When the child started; `None` for outcomes that never ran a child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_unix_ms: Option<u64>,
    /// When the occurrence reached a terminal state.
    pub finished_unix_ms: u64,
    /// Terminal outcome.
    pub outcome: SchedulerOutcomeV2,
    /// Child exit code; `None` for a signal death or a non-child outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Terminating Unix signal; `None` off Unix or for a normal exit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<u32>,
    /// Child wall duration; `None` when no child ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// How long the occurrence waited before starting.
    pub delay_ms: u64,
    /// Whether later civil occurrences were folded into this one.
    pub coalesced: bool,
    /// Bounded stdout tail.
    pub stdout: SchedulerOutputV2,
    /// Bounded stderr tail.
    pub stderr: SchedulerOutputV2,
}

/// Response body of `GET/HEAD /v2/scheduler/history`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SchedulerHistoryV2 {
    /// Schema major version; always `2`.
    pub schema_version: u16,
    /// Unix epoch in milliseconds when the document was produced.
    pub generated_at_unix_ms: u64,
    /// Current scheduler lifetime; records from other epochs are never served.
    pub epoch: SchedulerEpochV2,
    /// Matches [`SchedulerSummaryV2::history_revision`] for the same publication.
    pub history_revision: u64,
    /// One entry per configured job, each carrying its retained records.
    #[serde(default, deserialize_with = "deserialize_capped_job_histories")]
    pub jobs: Vec<SchedulerJobHistoryV2>,
}

impl SchedulerHistoryV2 {
    /// Validate that every field satisfies the scheduler wire invariants.
    ///
    /// # Errors
    ///
    /// Returns every violation found.
    pub fn validate(&self) -> Result<(), Vec<crate::ValidationViolationScheduler>> {
        crate::validate_scheduler::validate_history(self)
    }
}

/// Number of bytes `serde_json` emits for one `char` inside a JSON string.
///
/// Only the two-character escapes are short forms; every other C0 control
/// expands to the six-byte `\u00XX` form. Non-ASCII characters are emitted as
/// UTF-8 and pass through unchanged, matching `serde_json`'s default (no
/// `escape_non_ascii`) behavior, which is what Gregg's serializers use.
#[must_use]
pub fn json_escaped_len(character: char) -> usize {
    match character {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        c if (c as u32) < 0x20 => 6,
        _ => character.len_utf8(),
    }
}

/// Truncate `text` so its JSON-escaped length fits `budget` escaped bytes.
///
/// Returns the retained prefix and whether anything was dropped. Truncation
/// happens on a character boundary, so the result is always valid UTF-8 and
/// never splits a multi-byte sequence or a surrogate pair.
///
/// The first character is always retained even when it alone exceeds the
/// budget: returning an empty string would falsely claim "no output" for a
/// stream that did produce output. The reserved 6-byte allowance for one
/// maximal escape keeps the returned prefix's escaped length within `budget`
/// for every budget of at least 6.
///
/// ```
/// use gregg_protocol::truncate_to_escaped_budget;
///
/// // Each C0 control byte costs six escaped bytes.
/// let (text, truncated) = truncate_to_escaped_budget("\u{1b}\u{1b}\u{1b}", 12);
/// assert_eq!(truncated, true);
/// assert_eq!(text, "\u{1b}\u{1b}");
/// ```
#[must_use]
pub fn truncate_to_escaped_budget(text: &str, budget: usize) -> (&str, bool) {
    let mut used = 0usize;
    let mut end = 0usize;
    for (offset, character) in text.char_indices() {
        let cost = json_escaped_len(character);
        if used + cost > budget {
            // Always keep at least one character so a non-empty stream is
            // never reported as empty.
            if end == 0 {
                return (&text[..character.len_utf8()], true);
            }
            return (&text[..end], true);
        }
        used += cost;
        end = offset + character.len_utf8();
    }
    (text, false)
}

/// Convert arbitrary captured bytes to bounded, JSON-safe published text.
///
/// The raw byte bound is applied first (by the caller's tail buffer), then
/// invalid UTF-8 is replaced lossily, and finally the escaped-length budget is
/// enforced. `truncated` is `true` if either stage dropped bytes.
#[must_use]
pub fn output_text_from_bytes(raw_tail: &[u8], already_raw_truncated: bool) -> (String, bool) {
    let lossy = String::from_utf8_lossy(raw_tail);
    let (text, text_truncated) =
        truncate_to_escaped_budget(&lossy, MAX_SCHEDULER_OUTPUT_TEXT_BYTES);
    (text.to_owned(), already_raw_truncated || text_truncated)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::*;

    #[test]
    fn escaped_len_matches_serde_json_for_control_characters() {
        // The whole point of the escaped budget is that it agrees with the
        // real serializer, so assert both directions.
        for character in [
            '\u{0}', '\u{1}', '\u{1b}', '\u{7f}', '\n', '\t', '"', '\\', 'a',
        ] {
            // Two surrounding quotes.
            let buffer = serde_json::to_string(&character.to_string()).unwrap();
            assert_eq!(
                buffer.len() - 2,
                json_escaped_len(character),
                "escaped length mismatch for {character:?}"
            );
        }
    }

    #[test]
    fn non_ascii_passes_through_unescaped() {
        let text = "naïve → ok";
        assert_eq!(json_escaped_len('é'), 'é'.len_utf8());
        let (kept, truncated) = truncate_to_escaped_budget(text, MAX_SCHEDULER_OUTPUT_TEXT_BYTES);
        assert_eq!(kept, text);
        assert!(!truncated);
    }

    #[test]
    fn budget_counts_escaped_bytes_not_characters() {
        let (text, truncated) = truncate_to_escaped_budget("\u{1}\u{2}\u{3}", 12);
        assert!(truncated);
        assert_eq!(text, "\u{1}\u{2}");
        let (whole, truncated) = truncate_to_escaped_budget("\u{1}\u{2}", 12);
        assert!(!truncated);
        assert_eq!(whole, "\u{1}\u{2}");
    }

    #[test]
    fn truncation_never_splits_a_multibyte_character() {
        // 'a' costs one escaped byte, so a 2-byte budget cannot also hold the
        // 2-byte 'é'; the cut must land before it, not inside it.
        let (text, truncated) = truncate_to_escaped_budget("aé", 2);
        assert!(truncated);
        assert_eq!(text, "a");
        // With room for both, the whole string survives.
        let (whole, truncated) = truncate_to_escaped_budget("aé", 3);
        assert!(!truncated);
        assert_eq!(whole, "aé");
    }

    #[test]
    fn first_character_is_always_retained() {
        let (text, truncated) = truncate_to_escaped_budget("abc", 0);
        assert!(truncated);
        assert_eq!(text, "a");
    }

    #[test]
    fn lossy_conversion_bounds_invalid_utf8_and_marks_truncation() {
        let raw = [0xff_u8, b'o', b'k', 0xfe];
        let (text, truncated) = output_text_from_bytes(&raw, false);
        assert!(!truncated);
        assert!(text.contains('\u{fffd}'));
        assert!(text.contains("ok"));
    }

    #[test]
    fn output_text_marks_truncation_from_either_stage() {
        let raw = vec![b'x'; MAX_SCHEDULER_OUTPUT_BYTES];
        let (text, truncated) = output_text_from_bytes(&raw, true);
        assert!(truncated);
        assert_ne!(text, "");
        let escaped = serde_json::to_string(&text).unwrap();
        assert!(escaped.len() - 2 <= MAX_SCHEDULER_OUTPUT_TEXT_BYTES);
    }
}
