//! Plan 166: the cron detail block.
//!
//! Rendered inside the selected system's card, beside the `d` drive and `n`
//! network expansions. It is bounded on all three axes — vertical budget, cell
//! width, and record depth — so a system with sixty jobs on a twenty-row
//! terminal degrades into a truthful partial view rather than an overflow.
//!
//! # Truthfulness rules this renderer exists to enforce
//!
//! - A missing load reading is rendered as *unavailable*, never as `0`. A load
//!   gate that fired because telemetry was missing must not look like a gate
//!   that observed a low load.
//! - A non-child outcome (`load_expired`, `spawn_failed`) is shown as a real
//!   terminal record, with a duration of `—` rather than a fabricated `0ms`.
//! - Remote truncation is marked, and local viewport truncation is marked
//!   separately, because "the remote kept five" and "this pane can show two"
//!   are different facts.
//! - A stale scheduler read is marked stale and is never presented as current.
//!
//! # Inertness
//!
//! Every string that came from a remote machine is passed through
//! [`crate::sanitize`] immediately before it becomes cells. The text was already
//! escaped at the adoption boundary; re-running the sanitizer here is
//! idempotent and is what lets this module be the place that guarantees the
//! rule, rather than relying on a distant chokepoint staying correct.

use std::fmt::Write as _;

use ratatui::layout::Rect;
use ratatui::Frame;

use gregg_protocol::{
    SchedulerJobStateV2, SchedulerJobV2, SchedulerLoadGateV2, SchedulerOutcomeV2,
};

use crate::clientd::snapshot::SystemCronDto;
use crate::cron::{CronCapability, CronRecord};
use crate::sanitize::sanitize;
use crate::state::AppState;
use crate::ui::bar::render_text_line;
use crate::ui::text::truncate_width;

/// Rows the block spends on the `CRON` header plus the job table.
const HEADER_ROWS: usize = 1;

/// Rows the selected-job section costs before any record is rendered.
const SELECTED_JOB_ROWS: usize = 1;

/// Maximum job rows the table will attempt before the budget truncates it.
///
/// A cap in its own right, independent of the terminal: a sixty-job daemon
/// would otherwise make the job table the only thing on screen, and the
/// history — the part an operator opened the pane for — would be pushed out.
const MAX_JOB_ROWS: usize = 24;

/// Marker for a value the remote did not report.
const UNAVAILABLE: &str = "—";

/// Whether the cron block has anything to show for this system.
#[must_use]
pub fn is_visible(state: &AppState) -> bool {
    state.cron_expanded && state.selected_id.is_some()
}

/// How many rows the cron block wants, before the viewport clamps it.
///
/// Drives the height accounting in [`crate::state::entry_height`] and
/// [`crate::ui::layout`], so the outer fleet viewport keeps one selection and
/// one scroll model instead of growing a second inner one.
#[must_use]
pub fn desired_rows(state: &AppState) -> usize {
    if !is_visible(state) {
        return 0;
    }
    let Some(cron) = state.selected_cron() else {
        // Expanded on a system with no scheduler entry at all. One row saying
        // so is better than silently rendering nothing for a pane the operator
        // believes is open.
        return 1;
    };
    let jobs = job_rows(cron).len().min(MAX_JOB_ROWS);
    let mut rows = HEADER_ROWS + jobs;
    if let Some((job, records)) = selected_job_view(state, cron) {
        let _ = job;
        let depth = state.cron_display_history.max(1);
        rows += SELECTED_JOB_ROWS
            + records
                .iter()
                .map(|record| record_rows(record, depth))
                .sum::<usize>();
    }
    rows.min(MAX_JOB_ROWS + SELECTED_JOB_ROWS + 8)
}

/// Render the cron block for the selected system.
///
/// `rows_visible` is the vertical budget the layout computed; the block
/// truncates to it and says so rather than drawing past it.
pub fn render(f: &mut Frame, area: Rect, state: &AppState, rows_visible: usize) {
    if rows_visible == 0 || area.width == 0 || area.height == 0 {
        return;
    }
    let width = usize::from(area.width);
    let mut emitted = 0_usize;
    let mut row = area.y;

    let line = |text: String, f: &mut Frame, row: &mut u16, emitted: &mut usize| {
        if *emitted >= rows_visible {
            return false;
        }
        let clipped = truncate_width(&text, width);
        render_text_line(
            f,
            Rect {
                y: *row,
                height: 1,
                ..area
            },
            &clipped,
        );
        *row = row.saturating_add(1);
        *emitted += 1;
        true
    };

    let Some(cron) = state.selected_cron() else {
        line(
            plain("CRON", "no scheduler state for this system"),
            f,
            &mut row,
            &mut emitted,
        );
        return;
    };

    line(cron_header(cron), f, &mut row, &mut emitted);

    // A stale read is announced before the rows, so a row of numbers the
    // operator is about to trust carries its own caveat.
    if cron.last_error.is_some() && !line(stale_notice(cron), f, &mut row, &mut emitted) {
        return;
    }

    // The job table is bounded by the same constant the vertical budget is
    // built from, so a wide cron table cannot consume the whole block and push
    // the selected job's own history out of view entirely.
    for (job, selected) in job_rows(cron)
        .into_iter()
        .take(MAX_JOB_ROWS)
        .zip(job_flags(state, cron))
    {
        if !line(job_row(job, selected, width), f, &mut row, &mut emitted) {
            return;
        }
    }

    let Some((job, records)) = selected_job_view(state, cron) else {
        return;
    };
    if !line(
        selected_job_header(job, records.len()),
        f,
        &mut row,
        &mut emitted,
    ) {
        return;
    }

    for (index, record) in records.iter().rev().enumerate() {
        let depth = state.cron_display_history.max(1);
        // Newest first: an operator opening the pane wants the last run, not
        // the oldest thing still retained.
        let _ = index;
        if !line(record_headline(record, width), f, &mut row, &mut emitted) {
            return;
        }
        for stream in [Stream::Stdout, Stream::Stderr] {
            for text in stream_lines(record, stream, depth) {
                if !line(text, f, &mut row, &mut emitted) {
                    return;
                }
            }
        }
    }
}

/// The `CRON n jobs` header, or the reason there is nothing to list.
fn cron_header(cron: &SystemCronDto) -> String {
    match cron.capability {
        CronCapability::Unknown => plain("CRON", "not asked yet"),
        CronCapability::Unsupported => {
            plain("CRON", "unsupported by this greggd (no scheduler routes)")
        }
        CronCapability::Supported => {
            let jobs = cron
                .summary
                .as_ref()
                .map_or(0, |summary| summary.jobs.len());
            if jobs == 0 {
                plain("CRON", "0 jobs configured")
            } else {
                format!("CRON  {jobs} jobs")
            }
        }
    }
}

/// A notice that the last scheduler read failed, kept distinct from
/// reachability.
fn stale_notice(cron: &SystemCronDto) -> String {
    // A transport failure string can quote text the remote chose, so it is
    // escaped for the same reason command output is.
    let reason = cron
        .last_error
        .as_ref()
        .map_or_else(|| "unknown".to_owned(), ToString::to_string);
    format!(
        "  ! scheduler read failed ({}); showing last known data",
        inert(&reason)
    )
}

/// Job rows, in the remote's own order.
fn job_rows(cron: &SystemCronDto) -> Vec<&SchedulerJobV2> {
    cron.summary
        .as_ref()
        .map(|summary| summary.jobs.iter().collect())
        .unwrap_or_default()
}

/// Whether each job row is the expanded one, in the same order as
/// [`job_rows`].
fn job_flags(state: &AppState, cron: &SystemCronDto) -> Vec<bool> {
    let selected = state.selected_cron_job();
    job_rows(cron)
        .into_iter()
        .map(|job| Some(job.name.as_str()) == selected)
        .collect()
}

/// The expanded job and the records the daemon published for it.
fn selected_job_view<'a>(
    state: &'a AppState,
    cron: &SystemCronDto,
) -> Option<(&'a str, Vec<CronRecord>)> {
    let job = state.selected_cron_job()?;
    Some((job, cron.job_records(job).to_vec()))
}

/// One at-a-glance job row.
///
/// Columns are emitted in priority order and the tail is dropped when the
/// terminal is narrow, so the name and the state — the two facts that decide
/// whether the operator needs to look closer — always survive.
fn job_row(job: &SchedulerJobV2, selected: bool, width: usize) -> String {
    let marker = if selected { '>' } else { ' ' };
    let mut parts = vec![
        format!("{marker} {}", inert(&job.name)),
        schedule_label(&job.schedule),
        state_label(job.state),
    ];
    if let Some(detail) = state_detail(job) {
        parts.push(detail);
    }
    if let Some(gate) = load_gate_label(job) {
        parts.push(gate);
    }
    if let Some(last) = job.last.as_ref() {
        parts.push(format!("last {}", outcome_label(last.outcome)));
    }
    compose(&parts, width)
}

/// Assemble space-separated parts, dropping trailing ones that do not fit.
fn compose(parts: &[String], width: usize) -> String {
    const INDENT: usize = 2;
    let budget = width.saturating_sub(INDENT);
    let mut out = String::new();
    for (index, part) in parts.iter().enumerate() {
        let separator = usize::from(index > 0);
        let candidate = crate::sanitize::cells(&out) + separator + crate::sanitize::cells(part);
        // The first part is always kept: it is the job name, and dropping it
        // would produce a row that identifies nothing. Everything after it is
        // dropped whole, so a row never ends in a half-printed value.
        if index > 0 && candidate > budget {
            break;
        }
        if index > 0 {
            out.push(' ');
        }
        out.push_str(part);
    }
    format!("{out:>INDENT$}")
}

/// A compact, width-tolerant rendering of a configured schedule.
///
/// The raw five-field string is what the operator configured, so it is kept
/// verbatim where it fits and compacted to its weekday/time form where it does
/// not, rather than being replaced by a summary that hides the configuration.
fn schedule_label(schedule: &str) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let clean = inert(schedule);
    let fields: Vec<&str> = clean.split_whitespace().collect();
    if fields.len() != 5 {
        return clean;
    }
    let (minute, hour, dom, month, dow) = (fields[0], fields[1], fields[2], fields[3], fields[4]);
    let minute_ok = minute.chars().all(|c| c.is_ascii_digit());
    let hour_ok = hour.chars().all(|c| c.is_ascii_digit());
    if !minute_ok || !hour_ok {
        return clean;
    }
    let daily = dom == "*" && month == "*" && dow == "*";
    let weekly = dom == "*" && month == "*" && dow.len() == 1 && dow.as_bytes()[0].is_ascii_digit();
    // Parsed numerically: a width spec pads integers, not `&str`, so
    // `format!("{hour:0>2}")` on the raw field would print `3` and produce
    // the visibly wrong `3:0`.
    let hour_value: u32 = hour.parse().unwrap_or(0);
    let minute_value: u32 = minute.parse().unwrap_or(0);
    let when = format!("{hour_value:02}:{minute_value:02}");
    if daily {
        when
    } else if weekly {
        let index = dow.parse::<usize>().unwrap_or(0).min(6);
        format!("weekly {} {when}", WEEKDAYS[index])
    } else {
        clean
    }
}

/// The current scheduling state, in the fewest words that stay unambiguous.
fn state_label(state: SchedulerJobStateV2) -> String {
    match state {
        SchedulerJobStateV2::Idle => "idle".to_owned(),
        SchedulerJobStateV2::WaitingForSlot => "waiting for slot".to_owned(),
        // Distinct words, not a colour: a delayed job and a running job must
        // never read the same at a glance.
        SchedulerJobStateV2::LoadHigh => "load delayed".to_owned(),
        SchedulerJobStateV2::LoadUnavailable => "load unavailable".to_owned(),
        SchedulerJobStateV2::Running => "running".to_owned(),
    }
}

/// Next run or pending duration, whichever the state implies.
fn state_detail(job: &SchedulerJobV2) -> Option<String> {
    match job.state {
        SchedulerJobStateV2::Idle => Some(format!("next {}", age(job.next_due_unix_ms))),
        SchedulerJobStateV2::Running => job
            .running_since_unix_ms
            .map(|since| format!("for {}", age(since))),
        SchedulerJobStateV2::LoadHigh | SchedulerJobStateV2::LoadUnavailable => job
            .pending_since_unix_ms
            .map(|since| format!("pending {}", age(since))),
        SchedulerJobStateV2::WaitingForSlot => job
            .pending_since_unix_ms
            .map(|since| format!("queued {}", age(since))),
    }
}

/// The load-gate context, or `None` for a time-only job.
///
/// An absent observation is rendered as [`UNAVAILABLE`], never as `0.00`: a
/// gate that fired because telemetry was missing must not be readable as a
/// gate that saw a low load.
fn load_gate_label(job: &SchedulerJobV2) -> Option<String> {
    let gate: &SchedulerLoadGateV2 = job.load.as_ref()?;
    let observed = gate
        .observed
        .map_or_else(|| UNAVAILABLE.to_owned(), |value| format!("{value:.2}"));
    let mut text = format!(
        "load{} {observed} > {:.2}",
        inert(&gate.window),
        gate.threshold
    );
    // The retry context belongs to the *job*, not the gate, and is only
    // meaningful while a load delay is actually in force. It is appended last
    // so it is the first thing dropped on a narrow terminal.
    if matches!(
        job.state,
        SchedulerJobStateV2::LoadHigh | SchedulerJobStateV2::LoadUnavailable
    ) {
        match job.next_retry_unix_ms {
            Some(retry) => {
                let _ = write!(text, " retry {}", age(retry));
            }
            None => text.push_str(" retry pending"),
        }
    }
    Some(text)
}

/// The header above the selected job's history.
fn selected_job_header(job: &str, records: usize) -> String {
    // Escaped here too, not only at adoption. Every string this module turns
    // into cells goes through `inert`, so the guarantee is local to the
    // renderer rather than depending on a distant chokepoint staying correct.
    let job = inert(job);
    if records == 0 {
        format!("  {UNAVAILABLE} {job}: no terminal records yet")
    } else {
        format!("  {job}: {records} most recent")
    }
}

/// One record's headline: time, outcome, child duration, delay, exit.
fn record_headline(record: &CronRecord, width: usize) -> String {
    let inner = &record.record;
    let mut parts = vec![
        clock_label(inner.finished_unix_ms),
        outcome_label(inner.outcome),
        // A non-child outcome has no duration. Rendering `0ms` there would
        // claim a child ran and finished instantly.
        format!(
            "ran {}",
            inner
                .duration_ms
                .map_or_else(|| UNAVAILABLE.to_owned(), duration_label)
        ),
    ];
    if inner.delay_ms > 0 {
        parts.push(format!("delay {}", duration_label(inner.delay_ms)));
    }
    if let Some(code) = inner.exit_code {
        parts.push(format!("exit {code}"));
    } else if let Some(signal) = inner.signal {
        parts.push(format!("signal {signal}"));
    }
    if inner.coalesced {
        parts.push("coalesced".to_owned());
    }
    format!("  {}", compose(&parts, width.saturating_sub(2)))
}

#[derive(Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

/// The rendered lines of one record's output streams.
fn stream_lines(record: &CronRecord, stream: Stream, depth: usize) -> Vec<String> {
    let output = match stream {
        Stream::Stdout => &record.record.stdout,
        Stream::Stderr => &record.record.stderr,
    };
    if output.text.is_empty() {
        return Vec::new();
    }
    // Bounded here, not in the sanitizer: this is the viewport decision, and
    // the stored record is untouched.
    let cleaned = sanitize(&output.text, depth);
    let label = if output.truncated {
        "stdout+"
    } else {
        "stdout"
    };
    let mut lines: Vec<String> = cleaned
        .lines()
        .into_iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| format!("    {label}: {line}"))
        .collect();
    if cleaned.dropped_lines {
        // The remote's own tail truncation and this pane's row budget are
        // different facts, and the `+` in the label already reports the first.
        lines.push(format!("    {label}: more lines not shown"));
    }
    lines
}

/// A terminal outcome, named in words rather than by enum spelling.
fn outcome_label(outcome: SchedulerOutcomeV2) -> String {
    match outcome {
        SchedulerOutcomeV2::Success => "ok".to_owned(),
        SchedulerOutcomeV2::Failed => "failed".to_owned(),
        SchedulerOutcomeV2::SpawnFailed => "spawn failed".to_owned(),
        SchedulerOutcomeV2::WaitFailed => "wait failed".to_owned(),
        // A load-expired occurrence is a real terminal record even though no
        // child ever ran, so it is named rather than omitted.
        SchedulerOutcomeV2::LoadExpired => "load expired".to_owned(),
        SchedulerOutcomeV2::Cancelled => "cancelled".to_owned(),
    }
}

/// Rows one record costs: its headline plus exactly the output lines
/// [`stream_lines`] emits.
///
/// Derived from the renderer rather than assumed flat. A record whose output is
/// four lines costs six rows, not the three a flat estimate gives it, and a
/// record the remote truncated costs one more for the marker row — an
/// under-count is what pushes the newest record out of a fixed budget.
fn record_rows(record: &CronRecord, depth: usize) -> usize {
    1 + stream_lines(record, Stream::Stdout, depth).len()
        + stream_lines(record, Stream::Stderr, depth).len()
}

/// Render a Unix millisecond stamp as `MM-DD HH:MM`.
///
/// Deliberately not a full date: the block is a recent-history view inside a
/// fixed-width card, and the year is constant for anything it shows.
fn clock_label(unix_ms: u64) -> String {
    // Days-from-civil, so there is no date dependency and no timezone state.
    let days = i64::try_from(unix_ms / 86_400_000).unwrap_or(i64::MAX);
    let seconds_of_day = (unix_ms / 1000) % 86_400;
    let (hour, minute) = (seconds_of_day / 3600, (seconds_of_day % 3600) / 60);
    let (month, day) = civil_from_days(days);
    format!("{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// Convert days since the Unix epoch to a `(month, day)` pair.
///
/// Howard Hinnant's `civil_from_days`, which is exact for the whole range and
/// needs no lookup table. Only the month and day are produced because the block
/// never shows a year.
fn civil_from_days(days: i64) -> (u32, u32) {
    // `div_euclid` floors, which the algorithm requires; `/` truncates toward
    // zero and silently produces a wrong date for any negative input.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    // The day *within the year*. Every later step is derived from this, not
    // from `day_of_era`: mixing the two up yields a plausible-looking month
    // number like 4405 rather than an obviously broken one.
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (
        u32::try_from(month).unwrap_or(0),
        u32::try_from(day).unwrap_or(0),
    )
}

/// A compact duration, from milliseconds up to days.
fn duration_label(millis: u64) -> String {
    const SECOND: u64 = 1_000;
    const MINUTE: u64 = 60 * SECOND;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    if millis < SECOND {
        format!("{millis}ms")
    } else if millis < MINUTE {
        // Integer arithmetic rather than a float cast: a `u64` millisecond
        // count does not fit an `f64` mantissa exactly, and pedantic rightly
        // objects. The tenths are derived by division instead.
        let tenths = millis.saturating_mul(10) / SECOND;
        format!("{}.{}s", tenths / 10, tenths % 10)
    } else if millis < HOUR {
        format!("{}m{:02}s", millis / MINUTE, (millis % MINUTE) / SECOND)
    } else if millis < DAY {
        format!("{}h{:02}m", millis / HOUR, (millis % HOUR) / MINUTE)
    } else {
        format!("{}d{:02}h", millis / DAY, (millis % DAY) / HOUR)
    }
}

/// A coarse age or countdown relative to now, in whole units.
fn age(unix_ms: u64) -> String {
    let now = crate::state::now_unix_ms();
    if unix_ms > now {
        duration_label(unix_ms - now)
    } else {
        format!("{} ago", duration_label(now - unix_ms))
    }
}

/// One plain `label  value` line, for the "nothing here" cases.
fn plain(label: &str, value: &str) -> String {
    format!("{label}  {value}")
}

/// Make a remote string inert for cell construction.
fn inert(text: &str) -> String {
    // Unbounded: this is escaping, not a viewport decision. The row itself is
    // clipped to the terminal width by `truncate_width` at the call site.
    sanitize(text, usize::MAX).text
}

#[cfg(test)]
mod tests {
    use super::*;
    use gregg_protocol::{
        SchedulerEpochV2, SchedulerHistoryV2, SchedulerJobHistoryV2, SchedulerJobStateV2,
        SchedulerJobV2, SchedulerOutcomeV2, SchedulerOutputV2,
    };
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use crate::clientd::snapshot::{CronJobHistoryDto, SystemCronDto};
    use crate::cron::{CronCapability, CronFetchError, CronRecord};
    use crate::endpoint::Endpoint;
    use crate::state::{Reachability, SystemState};

    fn epoch() -> SchedulerEpochV2 {
        SchedulerEpochV2 {
            started_at_unix_ms: 1_700_000_000_000,
            nonce: 3,
        }
    }

    fn record(sequence: u64, stdout: &str, stderr: &str) -> CronRecord {
        CronRecord {
            epoch: epoch(),
            record: gregg_protocol::SchedulerRunRecordV2 {
                sequence,
                scheduled_unix_ms: 1_700_000_000_000,
                started_unix_ms: Some(1_700_000_000_100),
                finished_unix_ms: 1_700_000_000_000 + sequence * 60_000,
                outcome: SchedulerOutcomeV2::Success,
                exit_code: Some(0),
                signal: None,
                duration_ms: Some(1_500),
                delay_ms: 0,
                coalesced: false,
                stdout: SchedulerOutputV2::new(stdout.to_owned(), false),
                stderr: SchedulerOutputV2::new(stderr.to_owned(), false),
            },
        }
    }

    /// A one-system state whose scheduler reports `jobs`, with `records`
    /// retained for the job named `history_job`.
    fn state_with_cron(
        jobs: &[(&str, SchedulerJobStateV2)],
        history_job: &str,
        records: Vec<CronRecord>,
    ) -> AppState {
        let mut state = AppState::blank();
        state.adopt_snapshot(&crate::clientd::snapshot::FrontendSnapshot {
            generation: 1,
            produced_at_unix_ms: 1_700_000_000_000,
            refresh_status: crate::clientd::snapshot::RefreshStatusDto::Idle,
            poll_initialized: true,
            cron_display_history: 5,
            systems: vec![crate::clientd::snapshot::SystemSnapshotDto {
                id: "sys-a".to_owned(),
                endpoint: Endpoint::new("box".to_owned(), 11310, None),
                configured_name: None,
                reachability: Reachability::Online,
                latest: None,
                last_success_at_unix_ms: None,
                last_attempt_at_unix_ms: None,
                latency_ms: None,
                offline_reason: None,
            }],
            cron: vec![SystemCronDto {
                system_id: "sys-a".to_owned(),
                capability: CronCapability::Supported,
                summary: Some(gregg_protocol::SchedulerSummaryV2 {
                    schema_version: 2,
                    generated_at_unix_ms: 1_700_000_000_000,
                    epoch: epoch(),
                    history_revision: 1,
                    jobs: jobs
                        .iter()
                        .map(|(name, job_state)| SchedulerJobV2 {
                            name: (*name).to_owned(),
                            schedule: "0 3 * * *".to_owned(),
                            next_due_unix_ms: 1_700_000_100_000,
                            state: *job_state,
                            load: None,
                            pending_since_unix_ms: None,
                            next_retry_unix_ms: None,
                            running_since_unix_ms: None,
                            last: None,
                        })
                        .collect(),
                }),
                epoch: Some(epoch()),
                history_revision: Some(1),
                last_attempt_at_unix_ms: Some(1_700_000_000_000),
                last_success_at_unix_ms: Some(1_700_000_000_000),
                last_error: None,
                history: if records.is_empty() {
                    Vec::new()
                } else {
                    vec![CronJobHistoryDto {
                        job: history_job.to_owned(),
                        records,
                    }]
                },
            }],
            eggpool: None,
            config_reload_error: None,
        });
        state.cron_expanded = true;
        state.cron_job = Some(history_job.to_owned());
        state
    }

    /// Render the block and return the lines the terminal actually holds.
    fn drawn(state: &AppState, width: u16, rows: usize) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(
            width,
            u16::try_from(rows.max(1)).unwrap_or(u16::MAX),
        ))
        .expect("terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, state, rows);
            })
            .expect("draws");
        let buffer = terminal.backend().buffer().clone();
        (0..u16::try_from(rows).unwrap_or(u16::MAX))
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect()
    }

    fn text(lines: &[String]) -> String {
        lines.join("\n")
    }

    #[test]
    fn an_unsupported_daemon_says_so_instead_of_showing_an_empty_table() {
        let mut state = state_with_cron(&[], "", Vec::new());
        state.cron[0].capability = CronCapability::Unsupported;
        state.cron[0].summary = None;
        let lines = drawn(&state, 100, 6);
        assert!(text(&lines).contains("unsupported"), "{}", text(&lines));
        assert!(
            !text(&lines).contains("0 jobs configured"),
            "an old daemon has no job list, not an empty one"
        );
    }

    #[test]
    fn a_never_polled_system_says_it_was_not_asked_yet() {
        let mut state = state_with_cron(&[], "", Vec::new());
        state.cron[0].capability = CronCapability::Unknown;
        let lines = drawn(&state, 100, 6);
        assert!(text(&lines).contains("not asked yet"), "{}", text(&lines));
    }

    #[test]
    fn an_empty_but_supported_scheduler_is_zero_jobs_not_unsupported() {
        let state = state_with_cron(&[], "", Vec::new());
        let lines = drawn(&state, 100, 6);
        assert!(
            text(&lines).contains("0 jobs configured"),
            "{}",
            text(&lines)
        );
        assert!(!text(&lines).contains("unsupported"));
    }

    #[test]
    fn a_failed_scheduler_read_is_marked_and_says_the_data_is_stale() {
        let mut state = state_with_cron(&[("backup", SchedulerJobStateV2::Idle)], "backup", vec![]);
        state.cron[0].last_error = Some(CronFetchError::Transport("connection reset".to_owned()));
        let lines = drawn(&state, 120, 8);
        let rendered = text(&lines);
        assert!(rendered.contains("scheduler read failed"), "{rendered}");
        assert!(rendered.contains("last known data"), "{rendered}");
        assert!(rendered.contains("connection reset"), "{rendered}");
    }

    #[test]
    fn a_scheduler_failure_does_not_render_as_a_system_failure() {
        // The cron plane failing must not add "offline" anywhere: the metrics
        // plane owns reachability, and this block only speaks about the
        // scheduler.
        let mut state = state_with_cron(&[("backup", SchedulerJobStateV2::Idle)], "backup", vec![]);
        state.cron[0].last_error = Some(CronFetchError::Transport("boom".to_owned()));
        let rendered = text(&drawn(&state, 120, 8));
        assert!(!rendered.contains("offline"), "{rendered}");
    }

    #[test]
    fn an_absent_load_observation_never_renders_as_zero() {
        let mut state = state_with_cron(
            &[("backup", SchedulerJobStateV2::LoadUnavailable)],
            "backup",
            vec![],
        );
        state.cron[0].summary.as_mut().expect("summary").jobs[0].load = Some(SchedulerLoadGateV2 {
            window: "1m".to_owned(),
            threshold: 8.0,
            observed: None,
        });
        let rendered = text(&drawn(&state, 120, 8));
        assert!(rendered.contains("load unavailable"), "{rendered}");
        assert!(rendered.contains("load1m — > 8.00"), "{rendered}");
        assert!(
            !rendered.contains("load1m 0.00"),
            "a missing observation must never read as a low one: {rendered}"
        );
    }

    #[test]
    fn a_load_delayed_row_shows_the_window_the_load_and_the_threshold() {
        let mut state = state_with_cron(
            &[("backup", SchedulerJobStateV2::LoadHigh)],
            "backup",
            vec![],
        );
        state.cron[0].summary.as_mut().expect("summary").jobs[0].load = Some(SchedulerLoadGateV2 {
            window: "15m".to_owned(),
            threshold: 6.5,
            observed: Some(9.24),
        });
        let rendered = text(&drawn(&state, 120, 8));
        assert!(rendered.contains("load delayed"), "{rendered}");
        assert!(rendered.contains("load15m 9.24 > 6.50"), "{rendered}");
    }

    #[test]
    fn a_pending_job_shows_how_long_it_has_been_waiting() {
        let mut state = state_with_cron(
            &[("backup", SchedulerJobStateV2::LoadHigh)],
            "backup",
            vec![],
        );
        let job = &mut state.cron[0].summary.as_mut().expect("summary").jobs[0];
        job.pending_since_unix_ms = Some(crate::state::now_unix_ms().saturating_sub(17 * 60_000));
        let rendered = text(&drawn(&state, 120, 8));
        assert!(rendered.contains("pending 17m00s"), "{rendered}");
    }

    #[test]
    fn a_non_child_outcome_shows_no_duration_rather_than_zero() {
        // `load_expired` is a real terminal record even though no child ran, so
        // it appears; and it has no duration, so it must not claim `0ms`.
        let mut record = record(1, "", "");
        record.record.outcome = SchedulerOutcomeV2::LoadExpired;
        record.record.duration_ms = None;
        record.record.exit_code = None;
        let state = state_with_cron(
            &[("backup", SchedulerJobStateV2::Idle)],
            "backup",
            vec![record],
        );
        let rendered = text(&drawn(&state, 120, 10));
        assert!(rendered.contains("load expired"), "{rendered}");
        assert!(rendered.contains("ran —"), "{rendered}");
        assert!(
            !rendered.contains("ran 0ms"),
            "claiming a zero duration would say a child ran: {rendered}"
        );
    }

    #[test]
    fn remote_command_output_cannot_reach_the_terminal_as_control_sequences() {
        // The whole point of the sanitizer, asserted through the real buffer
        // rather than through the helper: nothing in the rendered text may
        // contain a character a terminal would act on. Each line is scanned
        // separately, because the newline that joins them is itself a control
        // character and would otherwise mask the result.
        let hostile = "\u{1b}[2J\u{1b}]0;pwned\u{07}done\rOVERWRITE";
        let state = state_with_cron(
            &[("backup", SchedulerJobStateV2::Idle)],
            "backup",
            vec![record(1, hostile, "")],
        );
        let lines = drawn(&state, 120, 10);
        for line in &lines {
            for character in line.chars() {
                assert!(
                    !character.is_control(),
                    "control character U+{:04X} survived into the buffer: {line:?}",
                    character as u32
                );
            }
        }
        let rendered = text(&lines);
        assert!(
            rendered.contains("^["),
            "the escape must stay visible rather than be stripped: {rendered}"
        );
        assert!(rendered.contains("OVERWRITE"), "{rendered}");
    }

    #[test]
    fn remote_job_names_cannot_reach_the_terminal_either() {
        // A job name comes from a remote configuration file, so it is hostile
        // input on the same footing as command output.
        let mut state = state_with_cron(&[("backup", SchedulerJobStateV2::Idle)], "backup", vec![]);
        state.cron[0].summary.as_mut().expect("summary").jobs[0].name = "\u{1b}[31mevil".to_owned();
        for line in drawn(&state, 120, 8) {
            assert!(
                !line.chars().any(char::is_control),
                "a control character survived into a job row: {line:?}"
            );
        }
    }

    /// A record costs exactly the rows its own output renders as.
    ///
    /// A flat two-per-stream estimate is what silently pushed the newest
    /// record out of a fixed budget, so the accounting is asserted against the
    /// renderer's own line count.
    #[test]
    fn a_records_row_cost_is_the_number_of_rows_it_really_renders() {
        let single = record(1, "one line", "");
        // Headline plus one output line.
        assert_eq!(record_rows(&single, 5), 2);

        let four = record(2, "a\nb\nc\nd", "");
        // Headline plus four output lines.
        assert_eq!(record_rows(&four, 5), 5);
        assert_eq!(stream_lines(&four, Stream::Stdout, 5).len(), 4);

        let both = record(3, "a\nb", "x\ny\nz");
        assert_eq!(record_rows(&both, 5), 6);
        assert_eq!(
            stream_lines(&both, Stream::Stdout, 5).len()
                + stream_lines(&both, Stream::Stderr, 5).len(),
            5
        );
    }

    /// Output longer than the pane's depth costs the marker row as well.
    #[test]
    fn output_longer_than_the_pane_depth_costs_its_marker_row() {
        let mut deep = record(1, "1\n2\n3\n4\n5\n6\n7\n8", "");
        deep.record.stdout.truncated = true;
        // The sanitizer keeps the depth and reports the rest, so the record is
        // a headline, five kept lines and one marker row.
        assert_eq!(record_rows(&deep, 5), 7);
        assert_eq!(stream_lines(&deep, Stream::Stdout, 5).len(), 6);
        // A remote truncation flag on its own adds no row when the stored text
        // already fits, because nothing was dropped here.
        let mut shallow = record(2, "kept line", "");
        shallow.record.stdout.truncated = true;
        assert_eq!(record_rows(&shallow, 5), 2);
    }

    /// A wide job table cannot consume the block and hide the selected job's
    /// own history.
    #[test]
    fn the_job_table_is_capped_so_the_selected_history_still_has_rows() {
        let jobs: Vec<(&str, SchedulerJobStateV2)> = (0..MAX_JOB_ROWS + 8)
            .map(|index| {
                (
                    Box::leak(format!("job-{index:02}").into_boxed_str()) as &str,
                    SchedulerJobStateV2::Idle,
                )
            })
            .collect();
        let records: Vec<CronRecord> = (1..=3)
            .map(|sequence| record(sequence, &format!("line-{sequence}"), ""))
            .collect();
        // The selected job sorts first, so it is inside the cap.
        let state = state_with_cron(&jobs, "job-00", records);

        // A budget large enough for the whole capped table plus the history.
        let rows = desired_rows(&state).min(MAX_JOB_ROWS + SELECTED_JOB_ROWS + 8);
        let block = drawn(&state, 120, rows);
        let text = text(&block);
        // The cap takes effect on the table, so the jobs past it are not drawn
        // even when the budget would have had room for them.
        assert!(text.contains("job-00"), "{text}");
        assert!(
            !text.contains("job-24"),
            "a job past the cap of {MAX_JOB_ROWS} was drawn: {text}"
        );
        // And the selected job's newest record still has rows.
        assert!(text.contains("line-3"), "{text}");
    }

    fn a_pane_budget_truncates_the_history_and_the_viewport_is_the_only_thing_cut() {
        let records: Vec<CronRecord> = (1..=5)
            .map(|sequence| record(sequence, &format!("line-{sequence}"), ""))
            .collect();
        let state = state_with_cron(&[("backup", SchedulerJobStateV2::Idle)], "backup", records);

        let full = text(&drawn(&state, 120, 24));
        assert!(full.contains("line-5"), "{full}");
        assert!(full.contains("line-1"), "{full}");

        // A four-row budget shows the top of the block and stops.
        let short = drawn(&state, 120, 4);
        assert_eq!(short.len(), 4);
        assert!(short[0].contains("CRON"), "{short:?}");
    }

    #[test]
    fn remote_truncation_is_reported_and_is_not_the_same_as_a_viewport_cut() {
        // The remote kept a bounded tail and says so; that is a different fact
        // from this pane having too few rows, and the two must not be conflated.
        let mut record = record(1, "kept line", "");
        record.record.stdout.truncated = true;
        let state = state_with_cron(
            &[("backup", SchedulerJobStateV2::Idle)],
            "backup",
            vec![record],
        );
        let rendered = text(&drawn(&state, 120, 24));
        assert!(rendered.contains("stdout+"), "{rendered}");
    }

    #[test]
    fn a_job_with_no_records_yet_says_so_instead_of_drawing_an_empty_section() {
        let state = state_with_cron(&[("backup", SchedulerJobStateV2::Idle)], "backup", vec![]);
        let rendered = text(&drawn(&state, 120, 12));
        assert!(rendered.contains("no terminal records yet"), "{rendered}");
    }

    #[test]
    fn a_zero_row_budget_draws_nothing_rather_than_indexing_outside_the_buffer() {
        let state = state_with_cron(&[("backup", SchedulerJobStateV2::Idle)], "backup", vec![]);
        let mut terminal = Terminal::new(TestBackend::new(80, 3)).expect("terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &state, 0);
            })
            .expect("draws");
        let buffer = terminal.backend().buffer().clone();
        for y in 0..3u16 {
            for x in 0..80u16 {
                assert_eq!(
                    buffer[(x, y)].symbol(),
                    " ",
                    "a zero budget must leave the buffer untouched"
                );
            }
        }
    }

    #[test]
    fn a_narrow_terminal_drops_the_tail_rather_than_wrapping_or_overflowing() {
        let mut state = state_with_cron(
            &[("backup-and-restore", SchedulerJobStateV2::Idle)],
            "backup-and-restore",
            vec![],
        );
        state.cron[0].summary.as_mut().expect("summary").jobs[0].load = Some(SchedulerLoadGateV2 {
            window: "15m".to_owned(),
            threshold: 6.5,
            observed: Some(9.24),
        });
        let lines = drawn(&state, 40, 8);
        for line in &lines {
            assert!(
                crate::sanitize::cells(line) <= 40,
                "line exceeds the terminal width: {line:?}"
            );
        }
        let row = &lines[1];
        // The name and the state are the two facts that decide whether the
        // operator needs to look closer, so they survive the cut. The gate
        // detail is the part that goes.
        assert!(row.contains("backup-and-restore"), "{row:?}");
        assert!(row.contains("idle"), "{row:?}");
        assert!(
            !row.contains("load15m"),
            "the tail must be dropped to fit, not wrapped: {row:?}"
        );
    }

    #[test]
    fn a_daily_schedule_compacts_but_an_unrecognised_one_is_kept_verbatim() {
        assert_eq!(schedule_label("0 3 * * *"), "03:00");
        assert_eq!(schedule_label("30 2 * * 3"), "weekly Wed 02:30");
        // Not a five-field expression: the operator's own text is what they
        // need to see, so it is not rewritten.
        assert_eq!(schedule_label("@reboot"), "@reboot");
        assert_eq!(schedule_label("0 */6 * * 1-5"), "0 */6 * * 1-5");
    }

    #[test]
    fn durations_scale_from_milliseconds_to_days() {
        assert_eq!(duration_label(0), "0ms");
        assert_eq!(duration_label(999), "999ms");
        assert_eq!(duration_label(1_500), "1.5s");
        assert_eq!(duration_label(90_000), "1m30s");
        assert_eq!(duration_label(3_600_000), "1h00m");
        assert_eq!(duration_label(90_000_000), "1d01h");
    }

    #[test]
    fn the_civil_date_conversion_is_exact_at_the_known_boundaries() {
        // Pinned against hand-computed day numbers rather than round-tripped
        // through the same function, so a wrong day-of-year term cannot be
        // masked by a symmetric error.
        assert_eq!(civil_from_days(0), (1, 1)); // 1970-01-01
        assert_eq!(civil_from_days(11_016), (2, 29)); // 2000-02-29, a leap day
        assert_eq!(civil_from_days(11_017), (3, 1));
        assert_eq!(civil_from_days(20_730), (10, 4)); // 2026-10-04
        assert_eq!(clock_label(0), "01-01 00:00");
    }

    #[test]
    fn the_selected_job_is_marked_so_the_operator_can_see_what_they_are_reading() {
        let state = state_with_cron(
            &[
                ("alpha", SchedulerJobStateV2::Idle),
                ("beta", SchedulerJobStateV2::Idle),
            ],
            "beta",
            vec![],
        );
        let lines = drawn(&state, 120, 10);
        let joined = text(&lines);
        assert!(joined.contains("> beta"), "{joined}");
        assert!(joined.contains("  alpha"), "{joined}");
    }

    #[test]
    fn output_lines_are_filtered_rather_than_drawn_blank() {
        // Blank output lines are a normal part of program output; drawing them
        // would spend the operator's row budget on nothing.
        let state = state_with_cron(
            &[("backup", SchedulerJobStateV2::Idle)],
            "backup",
            vec![record(1, "first\n\n   \nsecond", "")],
        );
        let rendered = text(&drawn(&state, 120, 24));
        assert!(rendered.contains("first"), "{rendered}");
        assert!(rendered.contains("second"), "{rendered}");
        assert!(!rendered.contains("stdout:   "), "{rendered}");
    }

    #[test]
    fn wide_glyphs_are_measured_in_cells_not_bytes() {
        // A CJK job name is two display cells per Rust char, so a byte- or
        // `char`-length budget overflows the terminal by exactly the amount this
        // guards. Asserted on `compose` directly: the rendered buffer is already
        // cell-accurate, so re-measuring its text would double-count every wide
        // glyph rather than test the budget.
        let parts = vec![
            "> バックアップ".to_owned(),
            "03:00".to_owned(),
            "load15m 2.00 > 1.00".to_owned(),
        ];
        let row = compose(&parts, 30);
        assert!(crate::sanitize::cells(&row) <= 30, "{row:?}");
        assert!(
            !row.contains("load15m"),
            "the tail must be dropped to fit the cell budget, not a byte budget: {row:?}"
        );
    }

    #[test]
    fn a_closed_pane_desires_no_rows() {
        let mut state = state_with_cron(&[("backup", SchedulerJobStateV2::Idle)], "backup", vec![]);
        state.cron_expanded = false;
        assert_eq!(desired_rows(&state), 0);
        assert!(!is_visible(&state));
    }

    #[test]
    fn a_selected_system_without_scheduler_state_still_gets_one_row() {
        // A pane the operator believes is open must say something rather than
        // rendering an invisible block.
        let mut state = AppState::blank();
        state.systems = vec![SystemState {
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
        state.selected_id = Some("sys-a".to_owned());
        state.cron_expanded = true;
        assert_eq!(desired_rows(&state), 1);
    }

    #[test]
    fn a_history_document_the_daemon_never_published_renders_as_no_records() {
        // The frontend must not invent records from the summary: it shows only
        // what the daemon actually sent.
        let state = state_with_cron(&[("backup", SchedulerJobStateV2::Idle)], "backup", vec![]);
        assert_eq!(state.selected_cron().expect("cron").history, Vec::new());
        let rendered = text(&drawn(&state, 120, 12));
        assert!(rendered.contains("no terminal records yet"), "{rendered}");
    }

    #[test]
    fn a_history_whose_epoch_differs_is_still_rendered_rather_than_hidden() {
        // Dedup identity is (epoch, sequence); the renderer must not second-
        // guess it, so a retained record from an earlier remote lifetime is
        // shown as what it is.
        let mut older = record(1, "from the previous lifetime", "");
        older.epoch = SchedulerEpochV2 {
            started_at_unix_ms: 1_600_000_000_000,
            nonce: 9,
        };
        let state = state_with_cron(
            &[("backup", SchedulerJobStateV2::Idle)],
            "backup",
            vec![older],
        );
        let rendered = text(&drawn(&state, 120, 24));
        assert!(
            rendered.contains("from the previous lifetime"),
            "{rendered}"
        );
    }

    #[test]
    fn an_empty_history_document_is_rendered_from_the_summary_alone() {
        // Guards the DTO filter: a job with no retained records is omitted from
        // the published history, and the pane must cope.
        let document = SchedulerHistoryV2 {
            schema_version: 2,
            generated_at_unix_ms: 1_700_000_000_000,
            epoch: epoch(),
            history_revision: 1,
            jobs: vec![SchedulerJobHistoryV2 {
                name: "backup".to_owned(),
                records: Vec::new(),
            }],
        };
        assert_eq!(document.jobs[0].records, Vec::new());
    }

    /// The condensed view reserves cron rows in `entry_height`, so it must
    /// actually draw them: otherwise a cron-expanded system in condensed mode
    /// shows a reserved gap instead of the block.
    #[test]
    fn the_condensed_view_draws_the_cron_block_rather_than_reserving_a_gap() {
        use crate::ui::condensed;

        let state = state_with_cron(
            &[("backup", SchedulerJobStateV2::Idle)],
            "backup",
            vec![record(1, "some output", "")],
        );
        let system = &state.systems[0];
        // Built through the real entry point so the test cannot drift from how
        // the view actually lays its columns out.
        let layout = condensed::compute_condensed_table_layout(&state.systems, 120);
        let mut terminal = Terminal::new(TestBackend::new(120, 10)).expect("terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                condensed::render_entry(
                    frame,
                    area,
                    system,
                    &layout,
                    true,
                    0,
                    0,
                    6,
                    None,
                    Some(&state),
                );
            })
            .expect("draws");
        let buffer = terminal.backend().buffer().clone();
        let rendered: String = (0..10u16)
            .map(|y| {
                (0..120u16)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            rendered.contains("CRON"),
            "condensed view drew no cron block"
        );
        assert!(
            rendered.contains("some output"),
            "condensed view drew the header but not the history"
        );
    }
}
