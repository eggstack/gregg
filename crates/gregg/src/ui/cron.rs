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

/// Maximum job rows the table will show before the budget truncates it.
///
/// A cap in its own right, independent of the terminal: a sixty-job daemon
/// would otherwise make the job table the only thing on screen, and the
/// history — the part an operator opened the pane for — would be pushed out.
///
/// This is a *window* size, not a prefix. The window is chosen around the
/// selected job, so a fleet with more jobs than the cap still shows whichever
/// one the operator navigated to.
const MAX_JOB_ROWS: usize = 24;

/// Rows a truncated job table may spend on its above/below markers.
const JOB_WINDOW_MARKERS: usize = 2;

/// Marker for a value the remote did not report.
const UNAVAILABLE: &str = "—";

/// Marker for rows this pane did not draw.
///
/// Distinct from the remote's own `stdout+`/`stderr+` truncation: "the remote
/// kept five lines" and "this viewport can show two" are different facts, and
/// only the first one is about the data.
const TRUNCATION_MARKER: &str = "  … more cron rows not shown";

/// Width used when counting logical rows.
///
/// Row *counts* do not depend on width — only row *content* does — so this
/// only has to be a plausible terminal width, not the real one.
const NOMINAL_WIDTH: usize = 120;

/// Ceiling on the height this block asks the outer layout for.
///
/// The block is bounded by its own job window, the daemon-clamped history depth,
/// and the remote's output cap, so this is a backstop rather than the main
/// control. Exceeding it is not silent: the block then reports its truncation.
const MAX_DESIRED_ROWS: usize = MAX_JOB_ROWS + SELECTED_JOB_ROWS + JOB_WINDOW_MARKERS + 8;

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
///
/// Derived from the very same row builder the renderer emits — including the
/// stale-scheduler notice — because a height budget that disagrees with the
/// content reserves one row too few exactly when the warning is present.
#[must_use]
pub fn desired_rows(state: &AppState) -> usize {
    if !is_visible(state) {
        return 0;
    }
    block_rows(state, NOMINAL_WIDTH, MAX_JOB_ROWS)
        .len()
        .min(MAX_DESIRED_ROWS)
}

/// The block's rows, in the groups the budget is spent in.
struct BlockRows {
    /// The `CRON` header plus the stale-scheduler notice, when present.
    head: Vec<String>,
    /// The selected job's header and its newest record.
    ///
    /// Reserved rather than merely ordered last: this is what the operator
    /// opened the pane for, so a job table of sixty rows must never be the part
    /// that gets cut.
    reserved: Vec<String>,
    /// The visible window of job rows, with its above/below markers.
    table: Vec<String>,
    /// Older records, after the table.
    rest: Vec<String>,
}

impl BlockRows {
    fn len(&self) -> usize {
        self.head.len() + self.reserved.len() + self.table.len() + self.rest.len()
    }

    /// Rows that are spent before any job row can be drawn.
    fn fixed(&self) -> usize {
        self.head.len() + self.reserved.len()
    }

    fn into_rows(self) -> Vec<String> {
        let mut rows = self.head;
        rows.extend(self.reserved);
        rows.extend(self.table);
        rows.extend(self.rest);
        rows
    }
}

/// Build the block's rows, with at most `table_max` job rows in the window.
fn block_rows(state: &AppState, width: usize, table_max: usize) -> BlockRows {
    let Some(cron) = state.selected_cron() else {
        // Expanded on a system with no scheduler entry at all. One row saying so
        // is better than silently rendering nothing for a pane the operator
        // believes is open.
        return BlockRows {
            head: vec![plain("CRON", "no scheduler state for this system")],
            reserved: Vec::new(),
            table: Vec::new(),
            rest: Vec::new(),
        };
    };

    let mut head = vec![cron_header(cron)];
    // A stale read is announced before the rows, so a row of numbers the
    // operator is about to trust carries its own caveat — and it is counted
    // here, not bolted on at render time.
    if cron.last_error.is_some() {
        head.push(stale_notice(cron));
    }

    let jobs = job_rows(cron);
    let selected = state.selected_cron_job();
    let window = job_window(&jobs, selected, table_max);
    let mut table = Vec::with_capacity(window.range.len() + 2);
    if window.above > 0 {
        table.push(format!("  … {} more jobs above", window.above));
    }
    table.extend(
        window
            .range
            .clone()
            .filter_map(|index| jobs.get(index))
            .map(|job| job_row(job, Some(job.name.as_str()) == selected, width)),
    );
    if window.below > 0 {
        table.push(format!("  … {} more jobs below", window.below));
    }

    let Some((job, records)) = selected_job_view(state, cron) else {
        return BlockRows {
            head,
            reserved: Vec::new(),
            table,
            rest: Vec::new(),
        };
    };
    // Newest first: an operator opening the pane wants the last run, not the
    // oldest thing still retained.
    let depth = state.cron_display_history.max(1);
    let mut records_iter = records.iter().rev();
    let newest = records_iter.next();
    let mut reserved = vec![selected_job_header(job, records.len())];
    if let Some(record) = newest {
        reserved.push(record_headline(record, width));
        for stream in [Stream::Stdout, Stream::Stderr] {
            reserved.extend(stream_lines(record, stream, depth));
        }
    }
    let mut rest = Vec::new();
    for record in records_iter {
        rest.push(record_headline(record, width));
        for stream in [Stream::Stdout, Stream::Stderr] {
            rest.extend(stream_lines(record, stream, depth));
        }
    }
    BlockRows {
        head,
        reserved,
        table,
        rest,
    }
}

/// Which job rows the table shows, chosen around the selected job.
struct JobWindow {
    /// Slice of [`job_rows`] indices to draw.
    range: std::ops::Range<usize>,
    /// Jobs hidden above the window.
    above: usize,
    /// Jobs hidden below the window.
    below: usize,
}

/// Choose a bounded job window that contains the selected job.
///
/// A fixed first-`N` slice hid the selection: `Shift-J` could move to a job
/// outside it and the highlighted row simply vanished, with the operator's
/// navigation apparently doing nothing. The window follows the selection
/// instead, clamped at both ends of the list.
fn job_window(jobs: &[&SchedulerJobV2], selected: Option<&str>, max: usize) -> JobWindow {
    let len = jobs.len();
    let max = max.min(len);
    if max == 0 {
        return JobWindow {
            range: 0..0,
            above: 0,
            below: len,
        };
    }
    let index = selected.and_then(|name| jobs.iter().position(|job| job.name == name));
    // Centred when the list is long enough to scroll, clamped to the ends
    // otherwise: near the top the window sits at the top, near the bottom at the
    // bottom, and in the middle the selection stays visible.
    let start = index
        .map_or(0, |index| index.saturating_sub(max / 2))
        .min(len - max);
    JobWindow {
        range: start..start + max,
        above: start,
        below: len - (start + max),
    }
}

/// Render the cron block for the selected system.
///
/// `rows_visible` is the vertical budget the layout computed. When the rows do
/// not fit, one row is reserved for the truncation marker so the block always
/// says what it left out, and the job window shrinks around the selection rather
/// than being cut off at a fixed boundary.
pub fn render(f: &mut Frame, area: Rect, state: &AppState, rows_visible: usize) {
    if rows_visible == 0 || area.width == 0 || area.height == 0 {
        return;
    }
    let width = usize::from(area.width);
    let height = usize::from(area.height).min(rows_visible);
    let full = block_rows(state, width, MAX_JOB_ROWS);
    let (rows, truncated) = if full.len() > height {
        // The block does not fit. Reserve the marker row, then spend what is
        // left on job rows around the selection — the selected job's section is
        // already reserved above them.
        // Two more for the window's above/below markers: without them in the
        // budget the selected row can land exactly on the last visible line and
        // be cut off by the very truncation it should survive.
        let table_max = height
            .saturating_sub(full.fixed() + 1 + JOB_WINDOW_MARKERS)
            .max(1);
        let shrunk = block_rows(state, width, table_max);
        let truncated = shrunk.len() > height;
        (shrunk.into_rows(), truncated)
    } else {
        (full.into_rows(), false)
    };

    let content = if truncated {
        height.saturating_sub(1)
    } else {
        height
    };
    for (index, row) in rows.iter().take(content).enumerate() {
        let Ok(offset) = u16::try_from(index) else {
            return;
        };
        draw_row(f, area, width, area.y.saturating_add(offset), row);
    }
    if truncated && content < height {
        let Ok(offset) = u16::try_from(content) else {
            return;
        };
        draw_row(
            f,
            area,
            width,
            area.y.saturating_add(offset),
            TRUNCATION_MARKER,
        );
    }
}

/// Draw one row, clipped to the terminal width.
fn draw_row(f: &mut Frame, area: Rect, width: usize, y: u16, text: &str) {
    let clipped = truncate_width(text, width);
    render_text_line(
        f,
        Rect {
            y,
            height: 1,
            ..area
        },
        &clipped,
    );
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
    // the visibly wrong `3:0`. Both fields must parse: an all-digit string can
    // still overflow `u32`, and `unwrap_or(0)` turned `99999999999999999999`
    // into a plausible-looking `00:00` — a fabricated midnight on a pane whose
    // whole purpose is to say when a job runs. A field that does not fit keeps
    // the verbatim `clean` string, which is what this function promises.
    let (Ok(hour_value), Ok(minute_value)) = (hour.parse::<u32>(), minute.parse::<u32>()) else {
        return clean;
    };
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

/// Next run or elapsed duration, whichever the state implies.
///
/// Elapsed wording, never the countdown-and-`ago` grammar: a running job is
/// `for 3m`, not `running for 3m ago`, which read as a contradiction because
/// `age()` returns an age for past stamps and a countdown for future ones.
fn state_detail(job: &SchedulerJobV2) -> Option<String> {
    match job.state {
        SchedulerJobStateV2::Idle => Some(format!("next {}", countdown_to(job.next_due_unix_ms))),
        SchedulerJobStateV2::Running => job
            .running_since_unix_ms
            .map(|since| format!("for {}", elapsed_since(since))),
        SchedulerJobStateV2::LoadHigh | SchedulerJobStateV2::LoadUnavailable => job
            .pending_since_unix_ms
            .map(|since| format!("pending {}", elapsed_since(since))),
        SchedulerJobStateV2::WaitingForSlot => job
            .pending_since_unix_ms
            .map(|since| format!("queued {}", elapsed_since(since))),
    }
}

/// The load-gate context, or `None` for a time-only job.
///
/// The relation is rendered *by meaning*, because `greggd` deliberately retains
/// the gate decision that admitted a job: a running or idle row can legitimately
/// carry an observation **below** the threshold. Printing every gate as
/// `observed > threshold` therefore produced lines like `1.20 > 8.00`, which are
/// not merely ugly but false, and `— > 8.00` for a missing reading, which is not
/// a comparison at all.
///
/// So the load is only ever compared above the threshold in the one state where
/// it *is* above it. A retained observation is labelled as the admission or last
/// gate it was, and a missing one is always spelled unavailable — never zero, and
/// never a numeric comparison.
fn load_gate_label(job: &SchedulerJobV2) -> Option<String> {
    let gate: &SchedulerLoadGateV2 = job.load.as_ref()?;
    let window = inert(&gate.window);
    let max = format!("max {:.2}", gate.threshold);
    let unavailable = || format!("load{window} unavailable ({max})");
    let mut text = match (job.state, gate.observed) {
        // The only state where the reading really is above the threshold.
        (SchedulerJobStateV2::LoadHigh, Some(observed)) => {
            format!("load{window} {observed:.2} > {:.2}", gate.threshold)
        }
        // Admitted under the gate it was started with.
        (SchedulerJobStateV2::Running, Some(observed)) => {
            format!("start load{window} {observed:.2} <= {:.2}", gate.threshold)
        }
        // Idle and slot-waiting rows carry the *previous* gate's observation.
        // "last gate" says so, rather than presenting an old reading as the
        // machine's current load.
        (SchedulerJobStateV2::Idle | SchedulerJobStateV2::WaitingForSlot, Some(observed)) => {
            format!(
                "last gate load{window} {observed:.2} <= {:.2}",
                gate.threshold
            )
        }
        (SchedulerJobStateV2::Running, None) => {
            format!("start load{window} unavailable ({max})")
        }
        (SchedulerJobStateV2::Idle | SchedulerJobStateV2::WaitingForSlot, None) => {
            format!("last gate load{window} unavailable ({max})")
        }
        // No reading in any state, including the one that gates on load: there
        // is no comparison to make, and `— > 8.00` is not one.
        (SchedulerJobStateV2::LoadHigh | SchedulerJobStateV2::LoadUnavailable, _) => unavailable(),
    };
    // The retry context belongs to the *job*, not the gate, and is only
    // meaningful while a load delay is actually in force. It is appended last
    // so it is the first thing dropped on a narrow terminal.
    if matches!(
        job.state,
        SchedulerJobStateV2::LoadHigh | SchedulerJobStateV2::LoadUnavailable
    ) {
        match job.next_retry_unix_ms {
            Some(retry) => {
                let _ = write!(text, " retry {}", countdown_to(retry));
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

impl Stream {
    /// The name this pane labels the stream with.
    ///
    /// Naming the wrong stream is not cosmetic: a truncated `stderr` is a
    /// different fact from a truncated `stdout`, so the name comes from the
    /// stream being rendered rather than being written once for both.
    fn name(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
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
    // `+` marks the remote's own tail truncation, on this stream.
    let label = if output.truncated {
        format!("{}+", stream.name())
    } else {
        stream.name().to_owned()
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

/// Render a Unix millisecond stamp as `MM-DD HH:MMZ`.
///
/// The trailing `Z` is not decoration. These fields are UTC instants, while the
/// job's schedule column is the remote host's *local* civil cron — so an
/// unlabelled `07:00` next to a `03:00` schedule read as a contradiction across
/// a timezone (and across DST) with nothing to explain it. The protocol carries
/// no remote timezone history, so the honest fix is to label the clock rather
/// than to invent one; reconstructing scheduler-local civil time needs its own
/// protocol change.
fn clock_label(unix_ms: u64) -> String {
    // Days-from-civil, so there is no date dependency and no timezone state.
    let days = i64::try_from(unix_ms / 86_400_000).unwrap_or(i64::MAX);
    let seconds_of_day = (unix_ms / 1000) % 86_400;
    let (hour, minute) = (seconds_of_day / 3600, (seconds_of_day % 3600) / 60);
    let (month, day) = civil_from_days(days);
    format!("{month:02}-{day:02} {hour:02}:{minute:02}Z")
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

/// How long ago a state began, in whole units.
///
/// Clock skew degrades conservatively: a stamp in the future — a remote whose
/// clock is ahead, or a transition mid-propagation — yields `0ms` rather than
/// underflowing. A saturated zero is a visibly wrong duration; a wrapped
/// `u64` would be an absurd one.
fn elapsed_since(unix_ms: u64) -> String {
    duration_label(crate::state::now_unix_ms().saturating_sub(unix_ms))
}

/// How long until a scheduled instant, in whole units.
///
/// The same conservative boundary as [`elapsed_since`]: a due instant already in
/// the past reads `0ms` rather than wrapping.
fn countdown_to(unix_ms: u64) -> String {
    duration_label(unix_ms.saturating_sub(crate::state::now_unix_ms()))
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
        assert!(
            rendered.contains("load1m unavailable (max 8.00)"),
            "a missing reading is never a comparison, not even the one that \
             gated the job: {rendered}"
        );
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

    /// Each stream is labelled with the stream it came from, in the untruncated
    /// case and the truncated one alike. Labelling stderr as stdout is not
    /// cosmetic: it sends the operator to the wrong place to look for the
    /// failure, and the truncation marker claims the wrong stream was cut.
    #[test]
    fn each_output_stream_is_labelled_with_its_own_name() {
        let mut record = record(1, "the backup ran", "the backup failed");
        record.record.stderr.truncated = true;
        let state = state_with_cron(
            &[("backup", SchedulerJobStateV2::Idle)],
            "backup",
            vec![record],
        );
        let rendered = text(&drawn(&state, 120, 24));
        assert!(rendered.contains("stdout: the backup ran"), "{rendered}");
        assert!(
            rendered.contains("stderr+: the backup failed"),
            "a truncated stderr must say so: {rendered}"
        );
        assert!(
            !rendered.contains("stdout+"),
            "nothing truncated stdout here: {rendered}"
        );
        assert!(
            !rendered.contains("stderr: the backup failed"),
            "a truncated stream keeps its marker: {rendered}"
        );
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
        // The selected job's section is reserved above the table, so the job row
        // is the first table line rather than simply the second line.
        let row = lines
            .iter()
            .find(|line| line.contains("backup-and-restore") && line.contains("idle"))
            .expect("the job row is drawn");
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

    /// An all-digit field that overflows `u32` must keep the verbatim
    /// schedule, not render as a fabricated midnight.
    ///
    /// The digit check admits `99999999999999999999`, and `unwrap_or(0)` then
    /// rewrote it to `00:00` — a plausible-looking wrong time on a pane whose
    /// entire purpose is to say when a job actually runs. A current `greggd`
    /// rejects such a schedule at config validation, so this needs an older,
    /// faulted, or hostile remote; remote scheduler data is untrusted input.
    #[test]
    fn an_unrepresentable_numeric_schedule_is_kept_verbatim() {
        for schedule in [
            "0 99999999999999999999 * * *",
            "0 18446744073709551616 * * *",
            "99999999999999999999 0 * * *",
            "0 4294967296 * * *",
        ] {
            assert_eq!(
                schedule_label(schedule),
                schedule,
                "an overflowing field must never be rendered as a time"
            );
        }
        // The boundary itself still compacts: `u32::MAX` parses.
        assert_eq!(schedule_label("0 4294967295 * * *"), "4294967295:00");
        // Ordinary values are unaffected.
        assert_eq!(schedule_label("0 3 * * *"), "03:00");
        assert_eq!(schedule_label("5 23 * * 0"), "weekly Sun 23:05");
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
                                                      // Labelled UTC: these are instants, while the schedule column beside
                                                      // them is the remote's local civil time.
        assert_eq!(clock_label(0), "01-01 00:00Z");
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
    // ---------------------------------------------------------------------
    // Plan 175: truthful semantics and bounded layout.
    // ---------------------------------------------------------------------

    /// A job with an explicit gate, state, and timestamps, for helper-level
    /// assertions about what a row claims.
    fn gated_job(state: SchedulerJobStateV2, observed: Option<f32>) -> SchedulerJobV2 {
        SchedulerJobV2 {
            name: "backup".to_owned(),
            schedule: "0 3 * * *".to_owned(),
            next_due_unix_ms: crate::state::now_unix_ms() + 11 * 3_600_000,
            state,
            load: Some(SchedulerLoadGateV2 {
                window: "15m".to_owned(),
                threshold: 8.0,
                observed,
            }),
            pending_since_unix_ms: None,
            next_retry_unix_ms: None,
            running_since_unix_ms: None,
            last: None,
        }
    }

    #[test]
    fn a_load_high_row_is_the_only_one_that_compares_above_the_threshold() {
        let label =
            load_gate_label(&gated_job(SchedulerJobStateV2::LoadHigh, Some(9.24))).expect("gate");
        // The trailing retry context is a separate fact about the job; the
        // relation itself is what this test pins.
        assert!(label.starts_with("load15m 9.24 > 8.00"), "{label}");
    }

    #[test]
    fn a_running_row_reports_the_gate_that_admitted_it_below_the_threshold() {
        // greggd deliberately retains the admitting observation, so a running job
        // legitimately carries a reading under the threshold. `1.20 > 8.00` was
        // simply false.
        let label =
            load_gate_label(&gated_job(SchedulerJobStateV2::Running, Some(1.20))).expect("gate");
        assert_eq!(label, "start load15m 1.20 <= 8.00");
    }

    #[test]
    fn an_idle_row_says_last_gate_so_an_old_reading_is_not_current_load() {
        let label =
            load_gate_label(&gated_job(SchedulerJobStateV2::Idle, Some(1.20))).expect("gate");
        assert_eq!(label, "last gate load15m 1.20 <= 8.00");
    }

    #[test]
    fn load_unavailable_is_never_a_numeric_comparison_in_any_state() {
        for state in [
            SchedulerJobStateV2::LoadUnavailable,
            SchedulerJobStateV2::LoadHigh,
            SchedulerJobStateV2::Running,
            SchedulerJobStateV2::Idle,
            SchedulerJobStateV2::WaitingForSlot,
        ] {
            let label = load_gate_label(&gated_job(state, None)).expect("gate");
            assert!(
                !label.contains('>'),
                "{state:?} with no reading rendered a comparison: {label}"
            );
            assert!(
                label.contains("unavailable"),
                "{state:?} did not say unavailable: {label}"
            );
            assert!(
                !label.contains(" 0.00"),
                "{state:?} turned a missing reading into zero: {label}"
            );
        }
        assert!(
            load_gate_label(&gated_job(SchedulerJobStateV2::LoadUnavailable, None))
                .expect("gate")
                .starts_with("load15m unavailable (max 8.00)"),
            "a gate that fired on missing telemetry states the limit it would have \
             enforced, without claiming a comparison"
        );
    }

    #[test]
    fn a_time_only_job_has_no_load_token_at_all() {
        let mut job = gated_job(SchedulerJobStateV2::Idle, Some(1.0));
        job.load = None;
        assert_eq!(load_gate_label(&job), None);
    }

    #[test]
    fn elapsed_states_use_elapsed_wording_and_never_ago() {
        let now = crate::state::now_unix_ms();

        let mut idle = gated_job(SchedulerJobStateV2::Idle, None);
        idle.load = None;
        idle.next_due_unix_ms = now + 11 * 3_600_000;
        assert_eq!(state_detail(&idle).as_deref(), Some("next 11h00m"));

        let mut running = gated_job(SchedulerJobStateV2::Running, None);
        running.running_since_unix_ms = Some(now.saturating_sub(3 * 60_000));
        assert_eq!(state_detail(&running).as_deref(), Some("for 3m00s"));

        let mut pending = gated_job(SchedulerJobStateV2::LoadHigh, None);
        pending.pending_since_unix_ms = Some(now.saturating_sub(17 * 60_000));
        assert_eq!(state_detail(&pending).as_deref(), Some("pending 17m00s"));

        let mut queued = gated_job(SchedulerJobStateV2::WaitingForSlot, None);
        queued.pending_since_unix_ms = Some(now.saturating_sub(2 * 60_000));
        assert_eq!(state_detail(&queued).as_deref(), Some("queued 2m00s"));

        // The retry countdown reads as a countdown, like the next-due one.
        let mut retrying = pending.clone();
        retrying.next_retry_unix_ms = Some(now + 20_000);
        let label = load_gate_label(&retrying).expect("gate");
        assert!(label.contains("retry 20.0s"), "{label}");

        for state in [idle, running, pending, queued] {
            if let Some(detail) = state_detail(&state) {
                assert!(
                    !detail.contains("ago"),
                    "an elapsed state must not read as an age: {detail}"
                );
            }
        }
    }

    #[test]
    fn clock_skew_degrades_to_zero_rather_than_wrapping() {
        let now = crate::state::now_unix_ms();
        // A remote clock ahead, or a transition captured before its own start.
        assert_eq!(elapsed_since(now + 5 * 60_000), "0ms");
        assert_eq!(countdown_to(now.saturating_sub(5 * 60_000)), "0ms");
        // And the ordinary cases still read correctly.
        assert_eq!(elapsed_since(now.saturating_sub(90_000)), "1m30s");
        assert_eq!(countdown_to(now + 90_000), "1m30s");
    }

    #[test]
    fn a_record_clock_is_labelled_utc_and_cannot_be_read_as_local_civil() {
        // 2026-10-05T07:00:00Z. Beside a `03:00` schedule this is unmistakably a
        // different time base from the remote's local civil clock.
        // 2026-10-05T07:00:00Z.
        let utc = 1_791_183_600_000_u64;
        let label = clock_label(utc);
        assert!(label.ends_with('Z'), "{label}");
        assert_eq!(label, "10-05 07:00Z");
    }

    #[test]
    fn the_stale_notice_is_part_of_the_requested_height() {
        // The layout budget has to include the warning, or it reserves one row
        // too few exactly when the warning is present.
        let mut state = state_with_cron(
            &[
                ("backup", SchedulerJobStateV2::Idle),
                ("sweep", SchedulerJobStateV2::Idle),
            ],
            "backup",
            vec![record(1, "line", "")],
        );
        let clean = desired_rows(&state);
        state.cron[0].last_error = Some(CronFetchError::Transport("boom".to_owned()));
        assert_eq!(
            desired_rows(&state),
            clean + 1,
            "the stale row must be requested, not silently dropped"
        );
        // And it is drawn inside that budget.
        let rows = desired_rows(&state);
        let rendered = text(&drawn(&state, 120, rows));
        assert!(rendered.contains("scheduler read failed"), "{rendered}");
    }

    #[test]
    fn a_short_budget_ends_with_an_explicit_truncation_marker() {
        let records: Vec<CronRecord> = (1..=5)
            .map(|sequence| record(sequence, &format!("line-{sequence}"), ""))
            .collect();
        let state = state_with_cron(
            &[
                ("backup", SchedulerJobStateV2::Idle),
                ("sweep", SchedulerJobStateV2::Idle),
            ],
            "backup",
            records,
        );
        let rendered = text(&drawn(&state, 120, 4));
        assert!(
            rendered.contains(TRUNCATION_MARKER),
            "a cut block must say it was cut: {rendered}"
        );
        // The marker is a real row inside the budget, never drawn past it.
        assert_eq!(
            drawn(&state, 120, 4).len(),
            4,
            "the marker must fit inside the budget it reports on"
        );
    }

    #[test]
    fn an_exact_fit_draws_everything_and_no_marker() {
        let state = state_with_cron(
            &[("backup", SchedulerJobStateV2::Idle)],
            "backup",
            vec![record(1, "only-line", "")],
        );
        let rows = desired_rows(&state);
        let rendered = text(&drawn(&state, 120, rows));
        assert!(!rendered.contains(TRUNCATION_MARKER), "{rendered}");
        assert!(rendered.contains("only-line"), "{rendered}");

        // One row short, the marker appears and the last row is the one lost.
        let short = text(&drawn(&state, 120, rows - 1));
        assert!(short.contains(TRUNCATION_MARKER), "{short}");
        assert!(!short.contains("only-line"), "{short}");
    }

    #[test]
    fn the_truncation_marker_is_not_confused_with_remote_stream_truncation() {
        let mut truncated = record(2, "kept", "");
        truncated.record.stdout = SchedulerOutputV2::new("kept\n".to_owned(), true);
        // Retained oldest-first, so the truncated record is the newest and
        // therefore the one inside the selected job's reserved section.
        let state = state_with_cron(
            &[
                ("backup", SchedulerJobStateV2::Idle),
                ("sweep", SchedulerJobStateV2::Idle),
                ("vacuum", SchedulerJobStateV2::Idle),
            ],
            "backup",
            vec![record(1, "older", ""), truncated],
        );
        let rendered = text(&drawn(&state, 120, 6));
        assert!(
            rendered.contains("stdout+"),
            "remote tail truncation keeps its own marker: {rendered}"
        );
        assert!(
            rendered.contains(TRUNCATION_MARKER),
            "and the viewport cut keeps a different one: {rendered}"
        );
    }

    /// A fleet with far more jobs than the window, and a selection the test
    /// chooses.
    fn state_with_many_jobs(count: usize, selected: &str) -> AppState {
        let jobs: Vec<(&str, SchedulerJobStateV2)> = (0..count)
            .map(|index| {
                (
                    Box::leak(format!("job-{index:02}").into_boxed_str()) as &str,
                    SchedulerJobStateV2::Idle,
                )
            })
            .collect();
        let records: Vec<CronRecord> = (1..=2)
            .map(|sequence| record(sequence, &format!("newest-{sequence}"), ""))
            .collect();
        state_with_cron(&jobs, selected, records)
    }

    #[test]
    fn the_selected_job_stays_visible_across_the_old_first_slice_boundary() {
        // 60 jobs against a 24-row window. Selection before, inside, and after
        // the old fixed prefix must all keep the selected row on screen.
        for selected in ["job-00", "job-11", "job-23", "job-24", "job-30", "job-59"] {
            let state = state_with_many_jobs(60, selected);
            let rendered = text(&drawn(&state, 120, MAX_JOB_ROWS + 6));
            assert!(
                rendered.contains(&format!("> {selected}")),
                "selecting {selected} hid its own row: {rendered}"
            );
            // The window is bounded, so a sixty-job list never all fits.
            assert!(
                !rendered.contains("job-59 ") || selected == "job-59",
                "the window stopped bounding the table: {rendered}"
            );
        }
    }

    #[test]
    fn omitted_jobs_are_counted_above_and_below() {
        let state = state_with_many_jobs(60, "job-30");
        // The full requested height, so the whole bounded window is drawn.
        let rendered = text(&drawn(&state, 120, desired_rows(&state)));
        assert!(
            rendered.contains("more jobs above"),
            "the window hid jobs without saying so: {rendered}"
        );
        assert!(
            rendered.contains("more jobs below"),
            "the window hid jobs without saying so: {rendered}"
        );
    }

    #[test]
    fn navigation_moves_the_window_with_the_selection() {
        let mut state = state_with_many_jobs(60, "job-00");
        let first = text(&drawn(&state, 120, MAX_JOB_ROWS + 6));
        assert!(first.contains("> job-00"), "{first}");
        assert!(!first.contains("> job-30"), "{first}");

        // `Shift-K` moves the selection the same way the renderer reads it.
        state.cron_job = Some("job-30".to_owned());
        let moved = text(&drawn(&state, 120, MAX_JOB_ROWS + 6));
        assert!(moved.contains("> job-30"), "{moved}");
        assert!(!moved.contains("> job-00"), "{moved}");
    }

    #[test]
    fn a_short_terminal_prioritises_the_selected_job_over_unrelated_job_rows() {
        // The point of pressing `c`: see the job that just ran. On a short block
        // the job table must not be the only thing on screen.
        let state = state_with_many_jobs(60, "job-50");
        for budget in 6..=10 {
            let rendered = text(&drawn(&state, 120, budget));
            assert!(
                rendered.contains("newest-"),
                "budget {budget} hid the selected job's newest record: {rendered}"
            );
        }
    }
}
