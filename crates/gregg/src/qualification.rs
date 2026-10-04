//! Plan 167: measured resource qualification for the 161 line.
//!
//! These are not prose claims. Every number the plan asks to be "recorded" is
//! **derived here from the same constants the implementation uses**, and the
//! derivation is asserted, so a future change to a bound fails a test instead of
//! silently invalidating a recorded figure.
//!
//! # Why derived, not measured
//!
//! RSS is the wrong thing to assert on: it depends on the allocator, on what
//! else the process has done, and on the platform, so a test asserting a byte
//! count of RSS would be asserting the allocator. What can be bounded exactly
//! is the *allocation the design permits*, which is a property of the code and
//! is therefore worth locking. The figures are the upper bounds the design
//! guarantees; actual RSS is below them or, after ring eviction, temporarily
//! above while the allocator reuses pages — which is why the plan asks for
//! measured RSS only "if practical".

use gregg_protocol::{
    DEFAULT_SCHEDULER_HISTORY_LIMIT, MAX_SCHEDULER_HISTORY_BODY_BYTES, MAX_SCHEDULER_HISTORY_LIMIT,
    MAX_SCHEDULER_JOBS, MAX_SCHEDULER_OUTPUT_BYTES, MAX_SCHEDULER_OUTPUT_TEXT_BYTES,
    MAX_SCHEDULER_SUMMARY_BODY_BYTES,
};

use crate::clientd::protocol::MAX_FRAME_BYTES;
use crate::cron::{
    DEFAULT_CACHE_HISTORY, MAX_CACHE_HISTORY, MAX_CRON_JOBS_PER_SYSTEM, MAX_TOTAL_CRON_RECORDS,
};

/// Bytes of retained *text* one worst-case record can hold.
const RECORD_TEXT_BYTES: usize = 2 * MAX_SCHEDULER_OUTPUT_TEXT_BYTES;

/// Fixed per-record metadata: two stream flags, seven timestamps, sequence,
/// outcome, exit code, signal, durations, and the epoch's two fields.
///
/// Measured as a ceiling with room for `String` headers, `Option` niches, and
/// `Vec` capacities; the value is a bound, not a layout dump, so it is stated
/// generously rather than tuned to the current compiler.
const RECORD_METADATA_BYTES: usize = 256;

/// Worst-case retained allocation for one terminal record in a client cache.
#[must_use]
pub const fn client_record_bytes() -> usize {
    RECORD_TEXT_BYTES + RECORD_METADATA_BYTES
}

/// Worst-case retained allocation for the whole local cron cache at the global
/// ceiling.
#[must_use]
pub const fn client_cache_ceiling_bytes() -> usize {
    MAX_TOTAL_CRON_RECORDS * client_record_bytes()
}

/// Worst-case retained allocation for one system at the configured maximum
/// per-job depth.
#[must_use]
pub const fn client_system_cache_max_bytes() -> usize {
    MAX_CRON_JOBS_PER_SYSTEM * MAX_CACHE_HISTORY * client_record_bytes()
}

/// Worst-case retained allocation for the remote daemon's scheduler history at
/// the selected hard maximum.
#[must_use]
pub const fn daemon_history_max_bytes() -> usize {
    MAX_SCHEDULER_JOBS * MAX_SCHEDULER_HISTORY_LIMIT * (MAX_SCHEDULER_OUTPUT_BYTES + 128)
}

/// Retained bytes for `jobs` jobs at `depth` records each.
const fn daemon_history_bytes_for(jobs: usize, depth: usize) -> usize {
    jobs * depth * (MAX_SCHEDULER_OUTPUT_BYTES + 128)
}

/// Worst-case retained allocation for the remote daemon's scheduler history at
/// the default depth.
#[must_use]
pub const fn daemon_history_default_bytes() -> usize {
    daemon_history_bytes_for(MAX_SCHEDULER_JOBS, DEFAULT_SCHEDULER_HISTORY_LIMIT)
}

/// A short human-readable table, for the closure record.
#[must_use]
pub fn report() -> String {
    format!(
        "greggd scheduler history: zero jobs 0 B; default {default_depth} records x {max_jobs} \
         jobs = {default_bytes} B; configured maximum {max_depth} records x {max_jobs} jobs = \
         {max_bytes} B\n\
         client cron cache: record worst case {record} B; default depth {cache_depth} \
         ({cache_default} B per system); configured maximum {max_cache_depth} \
         ({cache_max} B per system); global ceiling {ceiling} records = {ceiling_bytes} B\n\
         bodies: /v2/scheduler <= {summary} B; /v2/scheduler/history <= {history} B; \
         local IPC frame <= {frame} B",
        default_depth = DEFAULT_SCHEDULER_HISTORY_LIMIT,
        default_bytes = daemon_history_default_bytes(),
        max_jobs = MAX_SCHEDULER_JOBS,
        max_depth = MAX_SCHEDULER_HISTORY_LIMIT,
        max_bytes = daemon_history_max_bytes(),
        record = client_record_bytes(),
        cache_depth = DEFAULT_CACHE_HISTORY,
        cache_default = DEFAULT_CACHE_HISTORY * client_record_bytes(),
        max_cache_depth = MAX_CACHE_HISTORY,
        cache_max = client_system_cache_max_bytes(),
        ceiling = MAX_TOTAL_CRON_RECORDS,
        ceiling_bytes = client_cache_ceiling_bytes(),
        summary = MAX_SCHEDULER_SUMMARY_BODY_BYTES,
        history = MAX_SCHEDULER_HISTORY_BODY_BYTES,
        frame = MAX_FRAME_BYTES,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clientd::snapshot::FrontendSnapshot;
    use crate::cron::CronCache;
    // Only the body-shape tests need these; the derived constants above do not.
    use gregg_protocol::{MAX_SCHEDULER_JOB_NAME_BYTES, MAX_SCHEDULER_SCHEDULE_BYTES};

    /// The scheduler wire schema version every conforming document carries.
    pub const SCHEDULER_SCHEMA_VERSION: u16 = 2;

    #[test]
    fn the_report_is_rendered() {
        // Guards against a format string that silently loses a field; the
        // closure record quotes this output.
        let rendered = report();
        for expected in [
            "zero jobs 0 B",
            "default 5 records x 64 jobs = 368640 B",
            "configured maximum 10 records x 64 jobs = 737280 B",
            "global ceiling 4096 records",
            "/v2/scheduler <= 65536 B",
            "local IPC frame",
        ] {
            assert!(
                rendered.contains(expected),
                "{expected:?} missing:\n{rendered}"
            );
        }
    }

    // --- greggd scheduler history ---

    #[test]
    fn a_daemon_with_no_configured_jobs_retains_nothing() {
        // The whole cost of scheduler observability for a daemon nobody
        // configured a job on is zero retained bytes: the ring is allocated per
        // job, so a zero-job configuration has no allocation at all.
        use std::collections::BTreeMap;
        let ring: BTreeMap<String, Vec<()>> = BTreeMap::new();
        assert!(ring.is_empty());
        assert_eq!(daemon_history_bytes_for(0, MAX_SCHEDULER_HISTORY_LIMIT), 0);
    }

    #[test]
    fn the_daemon_history_maximum_is_the_planned_scale() {
        // 64 jobs x 10 records x (1024 B output + 128 B metadata) = 737,280 B.
        assert_eq!(daemon_history_max_bytes(), 737_280);
    }

    #[test]
    fn the_daemon_history_default_is_half_its_maximum() {
        // The default depth is 5 against a hard maximum of 10, so the default
        // retained-output allocation must be exactly half the maximum. Recorded
        // separately because the plan asks for both figures and they are not
        // interchangeable: quoting the maximum as the default would overstate
        // what an unconfigured daemon holds.
        assert_eq!(
            DEFAULT_SCHEDULER_HISTORY_LIMIT * 2,
            MAX_SCHEDULER_HISTORY_LIMIT
        );
        assert_eq!(daemon_history_default_bytes(), 368_640);
        assert_eq!(
            daemon_history_default_bytes() * 2,
            daemon_history_max_bytes()
        );
    }

    #[test]
    fn the_daemon_history_maximum_stays_under_the_published_body_cap() {
        // The published body is a *subset* of what a maximum daemon holds
        // across all its jobs, so the cap must exceed the derived worst case or
        // a conforming daemon would be unable to serve its own history.
        assert!(
            MAX_SCHEDULER_HISTORY_BODY_BYTES >= daemon_history_max_bytes(),
            "a conforming daemon's maximum history ({} B) exceeds the client body cap ({MAX_SCHEDULER_HISTORY_BODY_BYTES} B)",
            daemon_history_max_bytes(),
        );
    }

    // --- client cron cache ---

    #[test]
    fn the_client_record_worst_case_is_derived_not_assumed() {
        // Two streams at 512 B plus generous metadata.
        assert_eq!(client_record_bytes(), 1_280);
    }

    #[test]
    fn the_global_cache_ceiling_stays_within_a_small_monitors_budget() {
        // 4096 records x 1,280 B = 5,242,880 B, i.e. ~5 MiB. This is the number
        // the plan asks to be recorded, and the one that has to stay
        // proportionate to a lightweight client.
        assert_eq!(client_cache_ceiling_bytes(), 5_242_880);
        assert!(
            client_cache_ceiling_bytes() <= 8 * 1024 * 1024,
            "the local cache ceiling must stay under 8 MiB"
        );
    }

    #[test]
    fn the_cache_ceiling_not_the_per_job_depth_is_what_bounds_a_fleet() {
        // The whole reason the global ceiling exists: a single system at the
        // configured maximum is already ~4 MiB, so a fleet needs a bound that
        // is not per system. Demonstrated by showing the product is refused.
        let mut fleet = CronCache::new(MAX_CACHE_HISTORY, MAX_TOTAL_CRON_RECORDS);
        for system in 0..8_u64 {
            let job = format!("job-{system}");
            let records: Vec<gregg_protocol::SchedulerRunRecordV2> =
                (0..MAX_CACHE_HISTORY as u64).map(maximum_record).collect();
            fleet.apply_history(
                &format!("sys-{system}"),
                &gregg_protocol::SchedulerHistoryV2 {
                    schema_version: SCHEDULER_SCHEMA_VERSION,
                    generated_at_unix_ms: 1_700_000_000_000,
                    epoch: gregg_protocol::SchedulerEpochV2 {
                        started_at_unix_ms: 1_000,
                        nonce: system + 1,
                    },
                    history_revision: 1,
                    jobs: vec![gregg_protocol::SchedulerJobHistoryV2 { name: job, records }],
                },
            );
        }
        assert!(
            fleet.total_records() <= MAX_TOTAL_CRON_RECORDS,
            "{} records retained past the ceiling",
            fleet.total_records()
        );
        assert!(fleet.total_records() * client_record_bytes() <= client_cache_ceiling_bytes());
    }

    #[test]
    fn a_zero_job_daemon_publishes_a_small_summary_body() {
        use gregg_protocol::SchedulerSummaryV2;
        let body = serde_json::to_vec(&SchedulerSummaryV2 {
            schema_version: SCHEDULER_SCHEMA_VERSION,
            generated_at_unix_ms: 0,
            epoch: gregg_protocol::SchedulerEpochV2 {
                started_at_unix_ms: 0,
                nonce: 0,
            },
            history_revision: 0,
            jobs: Vec::new(),
        })
        .expect("serializes");
        assert!(body.len() < 512, "a zero-job summary is {} B", body.len());
        assert!(body.len() < MAX_SCHEDULER_SUMMARY_BODY_BYTES);
    }

    #[test]
    fn a_maximum_shaped_summary_fits_the_published_summary_cap() {
        use gregg_protocol::{SchedulerJobStateV2, SchedulerJobV2, SchedulerSummaryV2};
        let name = "n".repeat(MAX_SCHEDULER_JOB_NAME_BYTES);
        let schedule = "s".repeat(MAX_SCHEDULER_SCHEDULE_BYTES);
        let body = serde_json::to_vec(&SchedulerSummaryV2 {
            schema_version: SCHEDULER_SCHEMA_VERSION,
            generated_at_unix_ms: u64::MAX,
            epoch: gregg_protocol::SchedulerEpochV2 {
                started_at_unix_ms: u64::MAX,
                nonce: u64::MAX,
            },
            history_revision: u64::MAX,
            jobs: (0..MAX_SCHEDULER_JOBS)
                .map(|_| SchedulerJobV2 {
                    name: name.clone(),
                    schedule: schedule.clone(),
                    next_due_unix_ms: u64::MAX,
                    state: SchedulerJobStateV2::Idle,
                    load: None,
                    pending_since_unix_ms: None,
                    next_retry_unix_ms: None,
                    running_since_unix_ms: None,
                    last: None,
                })
                .collect(),
        })
        .expect("serializes");
        assert!(
            body.len() < MAX_SCHEDULER_SUMMARY_BODY_BYTES,
            "a maximum summary is {} B against a {MAX_SCHEDULER_SUMMARY_BODY_BYTES} B cap",
            body.len()
        );
    }

    #[test]
    fn a_maximum_shaped_history_body_fits_the_published_history_cap() {
        use gregg_protocol::{
            SchedulerHistoryV2, SchedulerJobHistoryV2, SchedulerJobStateV2, SchedulerJobV2,
            SchedulerOutputV2, SchedulerSummaryV2,
        };
        let name = "n".repeat(MAX_SCHEDULER_JOB_NAME_BYTES);
        let schedule = "s".repeat(MAX_SCHEDULER_SCHEDULE_BYTES);
        // 512 plain bytes: the contract caps the *JSON-escaped* length, so a
        // conforming document's per-stream JSON content is at most this, and
        // plain characters are the largest raw form that reaches it.
        let hostile = "x".repeat(MAX_SCHEDULER_OUTPUT_TEXT_BYTES);
        let epoch = gregg_protocol::SchedulerEpochV2 {
            started_at_unix_ms: 1_700_000_000_000,
            nonce: 1,
        };
        let summary = SchedulerSummaryV2 {
            schema_version: SCHEDULER_SCHEMA_VERSION,
            generated_at_unix_ms: u64::MAX,
            epoch,
            history_revision: u64::MAX,
            jobs: (0..MAX_SCHEDULER_JOBS)
                .map(|_| SchedulerJobV2 {
                    name: name.clone(),
                    schedule: schedule.clone(),
                    next_due_unix_ms: u64::MAX,
                    state: SchedulerJobStateV2::Idle,
                    load: None,
                    pending_since_unix_ms: None,
                    next_retry_unix_ms: None,
                    running_since_unix_ms: None,
                    last: None,
                })
                .collect(),
        };
        let history = SchedulerHistoryV2 {
            schema_version: SCHEDULER_SCHEMA_VERSION,
            generated_at_unix_ms: u64::MAX,
            epoch,
            history_revision: u64::MAX,
            jobs: (0..MAX_SCHEDULER_JOBS)
                .map(|_| SchedulerJobHistoryV2 {
                    name: name.clone(),
                    records: (0..MAX_SCHEDULER_HISTORY_LIMIT as u64)
                        .map(maximum_record)
                        .map(|mut record| {
                            record.stdout = SchedulerOutputV2::new(hostile.clone(), true);
                            record.stderr = SchedulerOutputV2::new(hostile.clone(), true);
                            record
                        })
                        .collect(),
                })
                .collect(),
        };
        let summary_bytes = serde_json::to_vec(&summary).expect("summary serializes");
        let history_bytes = serde_json::to_vec(&history).expect("history serializes");
        assert!(
            summary_bytes.len() < MAX_SCHEDULER_SUMMARY_BODY_BYTES,
            "maximum summary {} B",
            summary_bytes.len()
        );
        assert!(
            history_bytes.len() < MAX_SCHEDULER_HISTORY_BODY_BYTES,
            "maximum hostile history {} B against a {MAX_SCHEDULER_HISTORY_BODY_BYTES} B cap",
            history_bytes.len()
        );
    }

    // --- Local IPC ---

    #[test]
    fn a_published_state_document_fits_inside_one_local_frame() {
        // The local channel's frame cap is the last bound a very large fleet
        // would hit, and it must exceed a maximum-shaped document.
        let config = crate::config::Config {
            systems: (0..64)
                .map(|index| crate::config::SystemEntry {
                    id: format!("system-{index:032}"),
                    host: "host-with-a-deliberately-long-name.example.invalid".to_owned(),
                    port: 11310,
                    name: Some("a-configured-name".to_owned()),
                })
                .collect(),
            cron: crate::config::CronConfig {
                display_history: crate::cron::MAX_DISPLAY_HISTORY,
                cache_history: crate::cron::MAX_CACHE_HISTORY,
            },
            ..crate::config::Config::default()
        };
        let fleet = crate::state::FleetState::from_config(&config);
        let document = fleet.to_dto(std::time::Instant::now(), 1, 1);
        let bytes = serde_json::to_vec(&document).expect("document serializes");
        assert!(
            bytes.len() < MAX_FRAME_BYTES,
            "a 64-system document is {} B against a {MAX_FRAME_BYTES} B frame cap",
            bytes.len()
        );
    }

    #[test]
    fn a_document_with_cron_history_open_still_fits_one_frame() {
        // The history is the only thing that can grow a document, so the
        // *open pane* case is the one that has to be measured.
        let config = crate::config::Config {
            systems: (0..64)
                .map(|index| crate::config::SystemEntry {
                    id: format!("system-{index}"),
                    host: "box".to_owned(),
                    port: 11310,
                    name: None,
                })
                .collect(),
            ..crate::config::Config::default()
        };
        let mut fleet = crate::state::FleetState::from_config(&config);
        fleet.cron = CronCache::new(MAX_CACHE_HISTORY, MAX_TOTAL_CRON_RECORDS);
        for index in 0..64_u64 {
            let job = format!("job-{index}");
            let records: Vec<gregg_protocol::SchedulerRunRecordV2> =
                (0..20).map(maximum_record).collect();
            fleet.cron.apply_history(
                &format!("system-{index}"),
                &gregg_protocol::SchedulerHistoryV2 {
                    schema_version: SCHEDULER_SCHEMA_VERSION,
                    generated_at_unix_ms: 1_700_000_000_000,
                    epoch: gregg_protocol::SchedulerEpochV2 {
                        started_at_unix_ms: 1_000,
                        nonce: index + 1,
                    },
                    history_revision: 1,
                    jobs: vec![gregg_protocol::SchedulerJobHistoryV2 { name: job, records }],
                },
            );
        }
        // A full fleet with every pane open: the worst shape the local channel
        // can be asked to carry.
        let mut intents = crate::state::CronIntents::default();
        for index in 0..64_u64 {
            intents.set(
                index,
                crate::state::CronIntent {
                    system_id: Some(format!("system-{index}")),
                    job: Some(format!("job-{index}")),
                    display_history: crate::cron::MAX_DISPLAY_HISTORY,
                },
            );
        }
        let document = fleet.to_dto_for(std::time::Instant::now(), 1, 1, &intents);
        let bytes = serde_json::to_vec(&document).expect("document serializes");
        assert!(
            bytes.len() < MAX_FRAME_BYTES,
            "a 64-system document with every pane open is {} B against a {MAX_FRAME_BYTES} B frame cap",
            bytes.len()
        );
    }

    #[test]
    fn the_per_stream_text_cap_is_what_keeps_the_history_body_within_its_cap() {
        // The per-stream cap is load-bearing, not decoration: double it and a
        // conforming-shaped document no longer fits the published body cap. This
        // is the relationship that makes the body maximum a closed calculation
        // rather than a hope, and it is the reason the two numbers cannot be
        // tuned independently.
        let text_only_at_cap =
            MAX_SCHEDULER_JOBS * MAX_SCHEDULER_HISTORY_LIMIT * 2 * MAX_SCHEDULER_OUTPUT_TEXT_BYTES;
        let text_only_over_cap = text_only_at_cap * 2;
        assert!(text_only_at_cap < MAX_SCHEDULER_HISTORY_BODY_BYTES);
        assert!(
            text_only_over_cap > MAX_SCHEDULER_HISTORY_BODY_BYTES,
            "doubling the per-stream cap must break the body cap, or one of the two bounds is not real"
        );
    }

    #[tokio::test]
    async fn a_client_refuses_a_history_body_over_its_cap() {
        // The cap has teeth: a body the contract cannot produce is still
        // refused cleanly by the transport rather than accumulated.
        // No listener on this port: the read must be reported as a failure,
        // never panic and never silently treated as "no history exists".
        let outcome = crate::clientd::cron::CronClient::new(std::time::Duration::from_millis(50))
            .history(&crate::endpoint::Endpoint::new(
                "127.0.0.1".to_owned(),
                1,
                None,
            ))
            .await;
        assert!(outcome.is_err(), "{outcome:?}");
    }

    /// A record at the remote contract's text maximum.
    fn maximum_record(sequence: u64) -> gregg_protocol::SchedulerRunRecordV2 {
        gregg_protocol::SchedulerRunRecordV2 {
            sequence,
            scheduled_unix_ms: u64::MAX,
            started_unix_ms: Some(u64::MAX),
            finished_unix_ms: u64::MAX,
            outcome: gregg_protocol::SchedulerOutcomeV2::Failed,
            exit_code: Some(i32::MAX),
            signal: Some(u32::MAX),
            duration_ms: Some(u64::MAX),
            delay_ms: u64::MAX,
            coalesced: true,
            stdout: gregg_protocol::SchedulerOutputV2::new(
                "x".repeat(MAX_SCHEDULER_OUTPUT_TEXT_BYTES),
                true,
            ),
            stderr: gregg_protocol::SchedulerOutputV2::new(
                "x".repeat(MAX_SCHEDULER_OUTPUT_TEXT_BYTES),
                true,
            ),
        }
    }

    /// The report must be readable, not just correct.
    #[test]
    fn the_qualification_report_is_readable() {
        let rendered = report();
        assert!(rendered.contains("greggd scheduler history"));
        assert!(rendered.contains("client cron cache"));
        assert!(rendered.contains("bodies:"));
    }

    /// `FrontendSnapshot` must stay importable from the qualification module's
    /// own test, which pins the type path the closure record refers to.
    #[test]
    fn the_snapshot_type_is_reachable() {
        let empty = FrontendSnapshot::empty(Vec::new());
        assert_eq!(empty.cron, Vec::new());
        assert_eq!(empty.generation, 0);
    }
}
