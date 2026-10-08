//! Plan 162: validation for the scheduler-observability wire documents.
//!
//! These checks reject impossible or out-of-bound wire documents while leaving
//! *absence* alone: a pre-feature daemon answering `404` is a compatibility
//! outcome handled by the client's transport, never a protocol error here.

use std::fmt;

use thiserror::Error;

use crate::scheduler::{
    SchedulerEpochV2, SchedulerHistoryV2, SchedulerJobHistoryV2, SchedulerJobStateV2,
    SchedulerJobV2, SchedulerLoadGateV2, SchedulerOutcomeV2, SchedulerOutputV2,
    SchedulerRunRecordV2, SchedulerRunSummaryV2, SchedulerSummaryV2,
    MAX_SCHEDULER_HISTORY_BODY_BYTES, MAX_SCHEDULER_HISTORY_LIMIT, MAX_SCHEDULER_JOBS,
    MAX_SCHEDULER_JOB_NAME_BYTES, MAX_SCHEDULER_OUTPUT_TEXT_BYTES, MAX_SCHEDULER_SCHEDULE_BYTES,
};
use crate::v2::SCHEMA_VERSION_V2;

/// A single protocol-invariant violation for a scheduler document.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{kind}")]
pub struct ValidationViolationScheduler {
    /// Field-level violation kind.
    pub kind: ViolationKindScheduler,
    /// JSON path to the offending field, in dotted lowercase form.
    pub field: String,
}

impl ValidationViolationScheduler {
    fn new(kind: ViolationKindScheduler, field: impl Into<String>) -> Self {
        Self {
            kind,
            field: field.into(),
        }
    }
}

/// The kind of a single scheduler protocol-invariant violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViolationKindScheduler {
    /// `schema_version` did not match the v2 route family.
    UnsupportedSchemaVersion {
        /// Version found in the document.
        found: u16,
    },
    /// The job list exceeded [`MAX_SCHEDULER_JOBS`].
    TooManyJobs {
        /// Count found in the document.
        found: usize,
        /// Documented maximum.
        max: usize,
    },
    /// The record list exceeded [`MAX_SCHEDULER_HISTORY_LIMIT`].
    TooManyRecords {
        /// Count found for one job.
        found: usize,
        /// Documented maximum.
        max: usize,
    },
    /// A text field exceeded its byte bound.
    FieldTooLong {
        /// Byte length found.
        len: usize,
        /// Documented maximum.
        max: usize,
    },
    /// A job row carried an empty name.
    ///
    /// A job is addressed by its name in `/v2/scheduler/history` and in the
    /// client's per-system keying, so an empty one cannot identify anything.
    /// This is its own kind rather than a `FieldTooLong` because an empty
    /// string is within the length bound, not over it.
    EmptyJobName,
    /// A load gate carried a non-finite threshold.
    NonFiniteLoad,
    /// A load decision contradicts the row it was published on: a
    /// load-deferred row published no gate at all, or a `load_unavailable`
    /// row published an observed reading. The client must not be able to
    /// render a fabricated reading.
    InconsistentLoadDecision,
    /// `exit_code` or `signal` was present for an outcome that never ran a
    /// child.
    StatusWithoutChild {
        /// Outcome that cannot carry child status.
        outcome: SchedulerOutcomeV2,
    },
    /// `duration_ms` was present for an outcome that never ran a child.
    DurationWithoutChild {
        /// Outcome that cannot have a duration.
        outcome: SchedulerOutcomeV2,
    },
    /// A record's sequence was not strictly greater than its predecessor's.
    NonMonotonicSequence {
        /// Sequence found.
        found: u64,
        /// Sequence of the preceding record.
        previous: u64,
    },
    /// Two job entries in the same document shared a name.
    DuplicateJobName {
        /// Repeated name.
        name: String,
    },
    /// A timestamp was not a plausible post-epoch instant.
    TimestampOutOfRange {
        /// Value found.
        found: u64,
    },
    /// A load window was not one of the three configured windows.
    UnknownLoadWindow {
        /// Window found.
        found: String,
    },
}

impl fmt::Display for ViolationKindScheduler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchemaVersion { found } => write!(
                f,
                "unsupported schema_version {found} (expected {SCHEMA_VERSION_V2})"
            ),
            Self::TooManyJobs { found, max } => {
                write!(f, "scheduler document has {found} jobs (maximum {max})")
            }
            Self::TooManyRecords { found, max } => {
                write!(f, "job history has {found} records (maximum {max})")
            }
            Self::FieldTooLong { len, max } => {
                write!(
                    f,
                    "field exceeds maximum length of {max} bytes (found {len})"
                )
            }
            Self::EmptyJobName => f.write_str("job name must not be empty"),
            Self::NonFiniteLoad => f.write_str("load value must be finite"),
            Self::InconsistentLoadDecision => {
                f.write_str("load decision fields contradict the published job state")
            }
            Self::StatusWithoutChild { outcome } => write!(
                f,
                "child exit status is not meaningful for outcome {outcome:?}"
            ),
            Self::DurationWithoutChild { outcome } => write!(
                f,
                "child duration is not meaningful for outcome {outcome:?}"
            ),
            Self::NonMonotonicSequence { found, previous } => write!(
                f,
                "record sequence {found} does not exceed the preceding sequence {previous}"
            ),
            Self::DuplicateJobName { name } => write!(f, "duplicate job name {name:?}"),
            Self::TimestampOutOfRange { found } => {
                write!(f, "timestamp {found} ms is not a plausible wall clock")
            }
            Self::UnknownLoadWindow { found } => {
                write!(f, "unknown load window {found:?} (expected 1m, 5m, or 15m)")
            }
        }
    }
}

/// Upper bound for any scheduler timestamp: 2100-01-01T00:00:00Z in
/// milliseconds.
///
/// An *absolute* ceiling, not a duration. A document that claims an instant
/// beyond 2100 is a buggy or hostile producer; everything below it is
/// accepted without opinion, so ordinary clock skew and future-proofing stay
/// valid.
const MAX_TIMESTAMP_UNIX_MS: u64 = 4_102_444_800_000;

fn check_text(
    violations: &mut Vec<ValidationViolationScheduler>,
    field: &str,
    value: &str,
    max: usize,
) {
    if value.len() > max {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::FieldTooLong {
                len: value.len(),
                max,
            },
            field,
        ));
    }
}

fn check_timestamp(violations: &mut Vec<ValidationViolationScheduler>, field: &str, value: u64) {
    if value > MAX_TIMESTAMP_UNIX_MS {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::TimestampOutOfRange { found: value },
            field,
        ));
    }
}

fn check_epoch(
    violations: &mut Vec<ValidationViolationScheduler>,
    field: &str,
    epoch: &SchedulerEpochV2,
) {
    check_timestamp(
        violations,
        &format!("{field}.started_at_unix_ms"),
        epoch.started_at_unix_ms,
    );
}

/// A non-child outcome cannot carry child status, and a load gate is only
/// meaningful for a time-only-versus-load-gated distinction.
fn child_capable(outcome: SchedulerOutcomeV2) -> bool {
    matches!(
        outcome,
        SchedulerOutcomeV2::Success
            | SchedulerOutcomeV2::Failed
            | SchedulerOutcomeV2::WaitFailed
            | SchedulerOutcomeV2::Cancelled
    )
}

fn check_optional_child_fields(
    violations: &mut Vec<ValidationViolationScheduler>,
    prefix: &str,
    outcome: SchedulerOutcomeV2,
    exit_code: Option<i32>,
    signal: Option<u32>,
    duration_ms: Option<u64>,
) {
    if !child_capable(outcome) && (exit_code.is_some() || signal.is_some()) {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::StatusWithoutChild { outcome },
            format!("{prefix}.exit_code"),
        ));
    }
    if !child_capable(outcome) && duration_ms.is_some() {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::DurationWithoutChild { outcome },
            format!("{prefix}.duration_ms"),
        ));
    }
}

fn check_output(
    violations: &mut Vec<ValidationViolationScheduler>,
    field: &str,
    output: &SchedulerOutputV2,
) {
    // The published text is bounded by *escaped* length, which is the form
    // that actually reaches the wire, so that is what is checked.
    let escaped = text_escaped_len(&output.text);
    if escaped > MAX_SCHEDULER_OUTPUT_TEXT_BYTES {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::FieldTooLong {
                len: escaped,
                max: MAX_SCHEDULER_OUTPUT_TEXT_BYTES,
            },
            format!("{field}.text"),
        ));
    }
}

/// JSON-escaped length of an entire string, excluding its surrounding quotes.
fn text_escaped_len(text: &str) -> usize {
    text.chars()
        .map(crate::scheduler::json_escaped_len)
        .sum::<usize>()
}

fn check_duplicate_names<'a>(
    violations: &mut Vec<ValidationViolationScheduler>,
    names: impl Iterator<Item = &'a str>,
) {
    let mut seen: Vec<&str> = Vec::new();
    for name in names {
        if seen.contains(&name) {
            violations.push(ValidationViolationScheduler::new(
                ViolationKindScheduler::DuplicateJobName {
                    name: name.to_owned(),
                },
                "jobs",
            ));
        } else {
            seen.push(name);
        }
    }
}

fn check_load_gate(
    violations: &mut Vec<ValidationViolationScheduler>,
    field: &str,
    gate: &SchedulerLoadGateV2,
) {
    if !matches!(gate.window.as_str(), "1m" | "5m" | "15m") {
        // A long window fails the same way a wrong one does: it is not one of
        // the three configured windows. Reporting the value keeps the
        // diagnosis precise without a second violation kind.
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::UnknownLoadWindow {
                found: gate.window.clone(),
            },
            format!("{field}.window"),
        ));
    }
    if !gate.threshold.is_finite() {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::NonFiniteLoad,
            format!("{field}.threshold"),
        ));
    }
    if gate.observed.is_some_and(|value| !value.is_finite()) {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::NonFiniteLoad,
            format!("{field}.observed"),
        ));
    }
}

fn check_run_summary(
    violations: &mut Vec<ValidationViolationScheduler>,
    field: &str,
    summary: &SchedulerRunSummaryV2,
) {
    check_timestamp(
        violations,
        &format!("{field}.scheduled_unix_ms"),
        summary.scheduled_unix_ms,
    );
    check_timestamp(
        violations,
        &format!("{field}.finished_unix_ms"),
        summary.finished_unix_ms,
    );
    check_optional_child_fields(
        violations,
        field,
        summary.outcome,
        summary.exit_code,
        summary.signal,
        summary.duration_ms,
    );
}

fn check_job(
    violations: &mut Vec<ValidationViolationScheduler>,
    index: usize,
    job: &SchedulerJobV2,
) {
    let field = format!("jobs.{index}");
    check_text(
        violations,
        &format!("{field}.name"),
        &job.name,
        MAX_SCHEDULER_JOB_NAME_BYTES,
    );
    if job.name.is_empty() {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::EmptyJobName,
            format!("{field}.name"),
        ));
    }
    check_text(
        violations,
        &format!("{field}.schedule"),
        &job.schedule,
        MAX_SCHEDULER_SCHEDULE_BYTES,
    );
    check_timestamp(
        violations,
        &format!("{field}.next_due_unix_ms"),
        job.next_due_unix_ms,
    );
    if let Some(pending) = job.pending_since_unix_ms {
        check_timestamp(
            violations,
            &format!("{field}.pending_since_unix_ms"),
            pending,
        );
    }
    if let Some(retry) = job.next_retry_unix_ms {
        check_timestamp(violations, &format!("{field}.next_retry_unix_ms"), retry);
    }
    if let Some(running) = job.running_since_unix_ms {
        check_timestamp(
            violations,
            &format!("{field}.running_since_unix_ms"),
            running,
        );
    }

    if let Some(gate) = &job.load {
        check_load_gate(violations, &format!("{field}.load"), gate);
    }

    // Cross-field truthfulness. An idle or running job is not waiting on a
    // load decision, and a load-deferred job cannot be running.
    let deferred = matches!(
        job.state,
        SchedulerJobStateV2::LoadHigh | SchedulerJobStateV2::LoadUnavailable
    );
    if deferred {
        if job.running_since_unix_ms.is_some() {
            violations.push(ValidationViolationScheduler::new(
                ViolationKindScheduler::InconsistentLoadDecision,
                format!("{field}.running_since_unix_ms"),
            ));
        }
        if job.load.is_none() {
            violations.push(ValidationViolationScheduler::new(
                ViolationKindScheduler::InconsistentLoadDecision,
                format!("{field}.load"),
            ));
        }
    }
    if job.state == SchedulerJobStateV2::LoadUnavailable
        && job
            .load
            .as_ref()
            .is_some_and(|gate| gate.observed.is_some())
    {
        // An observed reading contradicts "load telemetry unavailable".
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::InconsistentLoadDecision,
            format!("{field}.load.observed"),
        ));
    }
    if !deferred && job.next_retry_unix_ms.is_some() {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::InconsistentLoadDecision,
            format!("{field}.next_retry_unix_ms"),
        ));
    }
    // A gate is deliberately retained beyond the deferral that produced it: a
    // running row names the gate it was admitted under and an idle or
    // slot-waiting row names the *last* gate, so an old reading is never
    // presented as the machine's current load. Those states therefore accept a
    // retained `observed` reading — `LoadUnavailable` is the only state in
    // which one contradicts the row.

    if let Some(last) = &job.last {
        check_run_summary(violations, &format!("{field}.last"), last);
    }
}

fn check_record(
    violations: &mut Vec<ValidationViolationScheduler>,
    field: &str,
    record: &SchedulerRunRecordV2,
) {
    check_timestamp(
        violations,
        &format!("{field}.scheduled_unix_ms"),
        record.scheduled_unix_ms,
    );
    if let Some(started) = record.started_unix_ms {
        check_timestamp(violations, &format!("{field}.started_unix_ms"), started);
    }
    check_timestamp(
        violations,
        &format!("{field}.finished_unix_ms"),
        record.finished_unix_ms,
    );
    check_optional_child_fields(
        violations,
        field,
        record.outcome,
        record.exit_code,
        record.signal,
        record.duration_ms,
    );
    if record.started_unix_ms.is_some() && !child_capable(record.outcome) {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::StatusWithoutChild {
                outcome: record.outcome,
            },
            format!("{field}.started_unix_ms"),
        ));
    }
    check_output(violations, &format!("{field}.stdout"), &record.stdout);
    check_output(violations, &format!("{field}.stderr"), &record.stderr);
}

fn check_job_history(
    violations: &mut Vec<ValidationViolationScheduler>,
    index: usize,
    history: &SchedulerJobHistoryV2,
) {
    let field = format!("jobs.{index}");
    check_text(
        violations,
        &format!("{field}.name"),
        &history.name,
        MAX_SCHEDULER_JOB_NAME_BYTES,
    );
    // History is keyed by the same job name as the summary, so it carries the
    // same non-empty requirement. The client tolerates an empty name by
    // skipping the row, which would silently drop records the daemon published.
    if history.name.is_empty() {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::EmptyJobName,
            format!("{field}.name"),
        ));
    }
    if history.records.len() > MAX_SCHEDULER_HISTORY_LIMIT {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::TooManyRecords {
                found: history.records.len(),
                max: MAX_SCHEDULER_HISTORY_LIMIT,
            },
            format!("{field}.records"),
        ));
    }
    // Sequence is the deduplication identity inside one epoch, so a document
    // that repeats or rewinds a sequence would break client suppression.
    let mut previous: Option<u64> = None;
    for (position, record) in history.records.iter().enumerate() {
        check_record(violations, &format!("{field}.records.{position}"), record);
        if let Some(previous) = previous {
            if record.sequence <= previous {
                violations.push(ValidationViolationScheduler::new(
                    ViolationKindScheduler::NonMonotonicSequence {
                        found: record.sequence,
                        previous,
                    },
                    format!("{field}.records.{position}.sequence"),
                ));
            }
        }
        previous = Some(record.sequence);
    }
}

/// Validate a `/v2/scheduler` summary document.
///
/// # Errors
///
/// Returns every violation found.
pub fn validate_summary(
    summary: &SchedulerSummaryV2,
) -> Result<(), Vec<ValidationViolationScheduler>> {
    let mut violations = Vec::new();

    if summary.schema_version != SCHEMA_VERSION_V2 {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::UnsupportedSchemaVersion {
                found: summary.schema_version,
            },
            "schema_version",
        ));
    }
    check_timestamp(
        &mut violations,
        "generated_at_unix_ms",
        summary.generated_at_unix_ms,
    );
    check_epoch(&mut violations, "epoch", &summary.epoch);

    if summary.jobs.len() > MAX_SCHEDULER_JOBS {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::TooManyJobs {
                found: summary.jobs.len(),
                max: MAX_SCHEDULER_JOBS,
            },
            "jobs",
        ));
    }
    for (index, job) in summary.jobs.iter().enumerate() {
        check_job(&mut violations, index, job);
    }
    check_duplicate_names(
        &mut violations,
        summary.jobs.iter().map(|job| job.name.as_str()),
    );

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

/// Validate a `/v2/scheduler/history` document.
///
/// # Errors
///
/// Returns every violation found.
pub fn validate_history(
    history: &SchedulerHistoryV2,
) -> Result<(), Vec<ValidationViolationScheduler>> {
    let mut violations = Vec::new();

    if history.schema_version != SCHEMA_VERSION_V2 {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::UnsupportedSchemaVersion {
                found: history.schema_version,
            },
            "schema_version",
        ));
    }
    check_timestamp(
        &mut violations,
        "generated_at_unix_ms",
        history.generated_at_unix_ms,
    );
    check_epoch(&mut violations, "epoch", &history.epoch);

    if history.jobs.len() > MAX_SCHEDULER_JOBS {
        violations.push(ValidationViolationScheduler::new(
            ViolationKindScheduler::TooManyJobs {
                found: history.jobs.len(),
                max: MAX_SCHEDULER_JOBS,
            },
            "jobs",
        ));
    }
    for (index, job) in history.jobs.iter().enumerate() {
        check_job_history(&mut violations, index, job);
    }
    check_duplicate_names(
        &mut violations,
        history.jobs.iter().map(|job| job.name.as_str()),
    );

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

/// Reject a history document that would exceed the wire body budget.
///
/// The server bounds this by construction, so this exists for the client
/// boundary and for tests that prove the frozen constant really is a maximum.
///
/// # Errors
///
/// Returns `true` when the document's serialized size exceeds
/// [`MAX_SCHEDULER_HISTORY_BODY_BYTES`].
pub fn history_body_exceeds_budget(history: &SchedulerHistoryV2) -> bool {
    serde_json::to_vec(history).map_or(true, |bytes| bytes.len() > MAX_SCHEDULER_HISTORY_BODY_BYTES)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::*;
    use crate::scheduler::{SchedulerEpochV2, SchedulerOutputV2};

    const START: u64 = 1_700_000_000_000;

    fn epoch() -> SchedulerEpochV2 {
        SchedulerEpochV2 {
            started_at_unix_ms: START,
            nonce: 7,
        }
    }

    fn empty_output() -> SchedulerOutputV2 {
        SchedulerOutputV2::new(String::new(), false)
    }

    fn run(outcome: SchedulerOutcomeV2, sequence: u64) -> SchedulerRunRecordV2 {
        SchedulerRunRecordV2 {
            sequence,
            scheduled_unix_ms: START,
            started_unix_ms: Some(START),
            finished_unix_ms: START + 1_000,
            outcome,
            exit_code: Some(0),
            signal: None,
            duration_ms: Some(1_000),
            delay_ms: 0,
            coalesced: false,
            stdout: empty_output(),
            stderr: empty_output(),
        }
    }

    fn job(name: &str) -> SchedulerJobV2 {
        SchedulerJobV2 {
            name: name.to_owned(),
            schedule: "0 3 * * *".to_owned(),
            next_due_unix_ms: START + 3_600_000,
            state: SchedulerJobStateV2::Idle,
            load: None,
            pending_since_unix_ms: None,
            next_retry_unix_ms: None,
            running_since_unix_ms: None,
            last: None,
        }
    }

    fn summary(jobs: Vec<SchedulerJobV2>) -> SchedulerSummaryV2 {
        SchedulerSummaryV2 {
            schema_version: SCHEMA_VERSION_V2,
            generated_at_unix_ms: START,
            epoch: epoch(),
            history_revision: 3,
            jobs,
        }
    }

    fn history(name: &str, records: Vec<SchedulerRunRecordV2>) -> SchedulerHistoryV2 {
        SchedulerHistoryV2 {
            schema_version: SCHEMA_VERSION_V2,
            generated_at_unix_ms: START,
            epoch: epoch(),
            history_revision: 3,
            jobs: vec![SchedulerJobHistoryV2 {
                name: name.to_owned(),
                records,
            }],
        }
    }

    #[test]
    fn empty_documents_are_valid() {
        assert!(summary(Vec::new()).validate().is_ok());
        assert!(history("a", Vec::new()).validate().is_ok());
    }

    #[test]
    fn wrong_schema_version_is_rejected() {
        let mut document = summary(Vec::new());
        document.schema_version = 1;
        let violations = document.validate().expect_err("v1 rejected");
        assert!(matches!(
            violations[0].kind,
            ViolationKindScheduler::UnsupportedSchemaVersion { found: 1 }
        ));
    }

    #[test]
    fn job_count_above_the_bound_is_rejected() {
        let jobs: Vec<SchedulerJobV2> = (0..=MAX_SCHEDULER_JOBS)
            .map(|index| job(&format!("job-{index}")))
            .collect();
        let violations = summary(jobs).validate().expect_err("over limit rejected");
        assert!(violations
            .iter()
            .any(|v| matches!(v.kind, ViolationKindScheduler::TooManyJobs { .. })));
    }

    #[test]
    fn record_depth_above_the_hard_maximum_is_rejected() {
        let records: Vec<SchedulerRunRecordV2> = (0..=MAX_SCHEDULER_HISTORY_LIMIT)
            .map(|index| run(SchedulerOutcomeV2::Success, index as u64 + 1))
            .collect();
        let violations = history("a", records)
            .validate()
            .expect_err("over limit rejected");
        assert!(violations
            .iter()
            .any(|v| matches!(v.kind, ViolationKindScheduler::TooManyRecords { .. })));
    }

    #[test]
    fn exact_history_depth_is_accepted() {
        let records: Vec<SchedulerRunRecordV2> = (0..MAX_SCHEDULER_HISTORY_LIMIT)
            .map(|index| run(SchedulerOutcomeV2::Success, index as u64 + 1))
            .collect();
        assert!(history("a", records).validate().is_ok());
    }

    #[test]
    fn non_monotonic_sequence_is_rejected() {
        let records = vec![
            run(SchedulerOutcomeV2::Success, 5),
            run(SchedulerOutcomeV2::Success, 5),
        ];
        let violations = history("a", records)
            .validate()
            .expect_err("duplicate sequence rejected");
        assert!(violations
            .iter()
            .any(|v| matches!(v.kind, ViolationKindScheduler::NonMonotonicSequence { .. })));
    }

    #[test]
    fn non_child_outcomes_may_not_carry_child_status() {
        let mut record = run(SchedulerOutcomeV2::LoadExpired, 1);
        record.started_unix_ms = None;
        record.duration_ms = None;
        record.exit_code = Some(3);
        let violations = history("a", vec![record])
            .validate()
            .expect_err("load-expired with exit code rejected");
        assert!(violations.iter().any(|v| matches!(
            v.kind,
            ViolationKindScheduler::StatusWithoutChild {
                outcome: SchedulerOutcomeV2::LoadExpired
            }
        )));
    }

    #[test]
    fn honest_non_child_outcomes_validate() {
        let mut record = run(SchedulerOutcomeV2::LoadExpired, 1);
        record.started_unix_ms = None;
        record.duration_ms = None;
        record.exit_code = None;
        assert!(history("a", vec![record]).validate().is_ok());

        let mut spawn = run(SchedulerOutcomeV2::SpawnFailed, 1);
        spawn.started_unix_ms = None;
        spawn.duration_ms = None;
        spawn.exit_code = None;
        assert!(history("a", vec![spawn]).validate().is_ok());
    }

    #[test]
    fn signal_death_is_accepted_without_an_exit_code() {
        let mut record = run(SchedulerOutcomeV2::Failed, 1);
        record.exit_code = None;
        record.signal = Some(9);
        assert!(history("a", vec![record]).validate().is_ok());
    }

    #[test]
    fn oversized_output_text_is_rejected() {
        let mut record = run(SchedulerOutcomeV2::Success, 1);
        record.stderr =
            SchedulerOutputV2::new("e".repeat(MAX_SCHEDULER_OUTPUT_TEXT_BYTES + 1), true);
        let violations = history("a", vec![record])
            .validate()
            .expect_err("oversized output rejected");
        assert!(violations.iter().any(|v| v.field.ends_with("stderr.text")));
    }

    #[test]
    fn control_output_is_measured_in_escaped_bytes() {
        let mut record = run(SchedulerOutcomeV2::Success, 1);
        // 128 raw control bytes escape to 768 bytes, over the 512-byte budget.
        record.stdout = SchedulerOutputV2::new("\u{1}".repeat(128), false);
        let violations = history("a", vec![record])
            .validate()
            .expect_err("escaped budget enforced");
        assert!(violations.iter().any(|v| v.field.ends_with("stdout.text")));
    }

    #[test]
    fn load_unavailable_may_not_carry_an_observed_reading() {
        let mut entry = job("gated");
        entry.state = SchedulerJobStateV2::LoadUnavailable;
        entry.load = Some(SchedulerLoadGateV2 {
            window: "15m".to_owned(),
            threshold: 8.0,
            observed: Some(1.0),
        });
        entry.next_retry_unix_ms = Some(START + 60_000);
        let violations = summary(vec![entry])
            .validate()
            .expect_err("unavailable with reading rejected");
        assert!(violations
            .iter()
            .any(|v| matches!(v.kind, ViolationKindScheduler::InconsistentLoadDecision)));
    }

    #[test]
    fn load_high_with_observed_reading_is_accepted() {
        let mut entry = job("gated");
        entry.state = SchedulerJobStateV2::LoadHigh;
        entry.pending_since_unix_ms = Some(START);
        entry.load = Some(SchedulerLoadGateV2 {
            window: "15m".to_owned(),
            threshold: 8.0,
            observed: Some(9.24),
        });
        entry.next_retry_unix_ms = Some(START + 60_000);
        assert!(summary(vec![entry]).validate().is_ok());
    }

    #[test]
    fn a_retained_gate_is_accepted_outside_the_deferring_states() {
        // greggd keeps the gate that admitted a job, so a completed or
        // slot-waiting row carries the reading it ran under. Rejecting it
        // would make a healthy daemon publish a document the client refuses.
        for state in [
            SchedulerJobStateV2::Idle,
            SchedulerJobStateV2::WaitingForSlot,
            SchedulerJobStateV2::Running,
        ] {
            let mut entry = job("gated");
            entry.state = state;
            if state == SchedulerJobStateV2::Running {
                entry.running_since_unix_ms = Some(START);
            }
            entry.load = Some(SchedulerLoadGateV2 {
                window: "15m".to_owned(),
                threshold: 8.0,
                observed: Some(1.2),
            });
            assert!(
                summary(vec![entry]).validate().is_ok(),
                "a {state:?} row may carry the gate that admitted it"
            );
        }
    }

    #[test]
    fn unknown_load_window_is_rejected() {
        let mut entry = job("gated");
        entry.state = SchedulerJobStateV2::LoadHigh;
        entry.load = Some(SchedulerLoadGateV2 {
            window: "30m".to_owned(),
            threshold: 8.0,
            observed: None,
        });
        let violations = summary(vec![entry])
            .validate()
            .expect_err("unknown window rejected");
        assert!(violations
            .iter()
            .any(|v| matches!(v.kind, ViolationKindScheduler::UnknownLoadWindow { .. })));
    }

    #[test]
    fn duplicate_job_names_are_rejected() {
        let violations = summary(vec![job("same"), job("same")])
            .validate()
            .expect_err("duplicate name rejected");
        assert!(violations
            .iter()
            .any(|v| matches!(v.kind, ViolationKindScheduler::DuplicateJobName { .. })));
    }

    /// An empty name is within the length bound, so it cannot be caught by
    /// the byte cap. It must still be rejected on both routes: the client keys
    /// cron rows by job name and tolerates an empty one by skipping the row,
    /// which would silently drop the records the daemon published.
    #[test]
    fn empty_job_names_are_rejected_on_both_routes() {
        let violations = summary(vec![job("")])
            .validate()
            .expect_err("empty summary job name rejected");
        assert!(
            violations
                .iter()
                .any(|v| matches!(v.kind, ViolationKindScheduler::EmptyJobName)
                    && v.field == "jobs.0.name"),
            "expected EmptyJobName at jobs.0.name: {violations:?}"
        );

        let violations = history("", vec![run(SchedulerOutcomeV2::Success, 1)])
            .validate()
            .expect_err("empty history job name rejected");
        assert!(
            violations
                .iter()
                .any(|v| matches!(v.kind, ViolationKindScheduler::EmptyJobName)
                    && v.field == "jobs.0.name"),
            "expected EmptyJobName at jobs.0.name: {violations:?}"
        );

        // The violation must read as what it is, not as an over-long field.
        assert_eq!(
            ViolationKindScheduler::EmptyJobName.to_string(),
            "job name must not be empty"
        );

        // A one-byte name stays valid: this is a non-empty rule, not a
        // minimum-length rule.
        assert!(summary(vec![job("a")]).validate().is_ok());
    }

    #[test]
    fn representative_maximum_history_stays_inside_the_body_budget() {
        // The frozen constants must really be a maximum, not a hope: fill the
        // pathological shape (64 jobs at the hard depth, every output stream
        // filled to the escaped budget) and prove it still serializes under the
        // published client cap.
        let text = "\u{1}".repeat(MAX_SCHEDULER_OUTPUT_TEXT_BYTES / 6);
        assert_eq!(text_escaped_len(&text), 510);
        assert!(text_escaped_len(&text) <= MAX_SCHEDULER_OUTPUT_TEXT_BYTES);
        let output = SchedulerOutputV2::new(text, true);
        let jobs: Vec<SchedulerJobHistoryV2> = (0..MAX_SCHEDULER_JOBS)
            .map(|index| {
                let records: Vec<SchedulerRunRecordV2> = (0..MAX_SCHEDULER_HISTORY_LIMIT)
                    .map(|position| {
                        let mut record =
                            run(SchedulerOutcomeV2::Success, (index * 100 + position) as u64);
                        record.stdout = output.clone();
                        record.stderr = output.clone();
                        record
                    })
                    .collect();
                SchedulerJobHistoryV2 {
                    name: format!("job-{index:02}"),
                    records,
                }
            })
            .collect();
        let document = SchedulerHistoryV2 {
            schema_version: SCHEMA_VERSION_V2,
            generated_at_unix_ms: START,
            epoch: epoch(),
            history_revision: 1,
            jobs,
        };
        assert!(document.validate().is_ok());
        assert!(
            !history_body_exceeds_budget(&document),
            "frozen maximum must stay under the published client cap"
        );
        let measured = serde_json::to_vec(&document).unwrap().len();
        // Plan 162 recorded this exact figure (832,022 bytes for the
        // pathological shape) as the derived history maximum. Pinning it means
        // widening any of the frozen constants, or changing serialization,
        // fails here instead of silently invalidating the published cap.
        assert_eq!(measured, 832_022, "derived history maximum changed");
        assert!(!history_body_exceeds_budget(&document));
    }
}
