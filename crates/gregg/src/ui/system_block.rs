#![allow(dead_code)]

//! Normal-view system block rendering.
//!
//! The online block is laid out in this fixed order:
//!
//! 1. Header line (priority-aware in `text::header_line`)
//! 2. CPU row
//! 3. MEM row
//! 4. SWP or COMMIT row (platform-determined)
//! 5. DISK aggregate row
//! 6. NET aggregate row when the current fleet has any network telemetry,
//!    optionally followed by per-drive and network detail rows
//!
//! Rows 2 through 5 share one fleet-wide geometry so the opening and
//! closing brackets line up exactly across every online system. The
//! shared [`MetricFleetLayout`] (label width and bar width) is computed
//! once per render in `ui::render` via [`compute_fleet_metric_layout`]
//! and reused by every online system block. The DISK aggregate suffix
//! is rendered as `<used bytes> / <total bytes>` to keep the slash
//! denominator consistent with the percentage calculation; explicit
//! caller-available capacity (`available_bytes`) is preserved by the
//! normalized model and surfaced through the expanded drive detail
//! rows.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::normalized::{aggregate_drives, NormalizedSnapshot};
use crate::state::SystemState;

use super::bar;
use super::text;

/// Indent applied to every metric row in normal view.
const METRIC_ROW_INDENT: &str = "    ";

/// Display-cell width of the prefix portion of a metric row:
/// `indent + label + " ["`. Shared by the fleet layout and the
/// per-system suffix resolver so they cannot disagree about the
/// fixed-width structural prefix.
fn metric_prefix_width(label_width: u16) -> u16 {
    let indent_w = u16::try_from(METRIC_ROW_INDENT.len()).unwrap_or(u16::MAX);
    indent_w.saturating_add(label_width).saturating_add(2) // " ["
}

/// Number of cells reserved between the closing `]` and the suffix
/// text when suffixes are visible. Plan 087: when suffixes are
/// suppressed, the cell that would otherwise be the suffix separator
/// is returned to the bar budget.
const METRIC_SUFFIX_GAP_CELLS: u16 = 2; // "] "

/// Display-cell width of the prefix portion of a metric row when the
/// suffix is suppressed: `indent + label + " ["` plus the closing
/// `]` itself (no gap). Used to size the bar in bar-only mode.
fn metric_compact_prefix_width(label_width: u16) -> u16 {
    metric_prefix_width(label_width).saturating_add(1) // "]"
}

/// Render a normal-view online system block using the precomputed
/// fleet-wide metric geometry.
///
/// `rows` supplies this system's metric rows when the caller has
/// them from a shared per-render memo; identical snapshots produce
/// identical rows, so `None` only triggers a local rebuild fallback.
#[allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::trivially_copy_pass_by_ref
)]
pub(crate) fn render_online(
    f: &mut Frame,
    area: Rect,
    system: &SystemState,
    rows: Option<&MetricRows>,
    fleet_layout: &MetricFleetLayout,
    is_visually_selected: bool,
    drive_rows_visible: usize,
    network_rows_visible: usize,
) {
    if area.width == 0 {
        return;
    }

    let Some(snap) = &system.latest else {
        render_waiting(f, area, system, is_visually_selected);
        return;
    };
    let base_height = rows.map_or_else(
        || base_height_for(snap.network.is_some()),
        MetricRows::base_height,
    );
    if area.height < base_height {
        return;
    }

    let sel_style = if is_visually_selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };

    // Row 0: header line
    let header = text::header_line(system, area.width);
    let header_line = Line::from(Span::styled(header, sel_style));
    f.render_widget(header_line, Rect { height: 1, ..area });

    // Resolve per-system suffix strings against the shared fleet
    // geometry so the opening/closing brackets line up with every other
    // online system in the same render.
    let rebuilt;
    let rows: &MetricRows = if let Some(rows) = rows {
        rows
    } else {
        rebuilt = build_metric_rows(snap, snap.network.is_some());
        &rebuilt
    };
    let suffixes = resolve_system_suffixes(rows, area.width, *fleet_layout);

    for (idx, (row, suffix)) in rows.as_slice().iter().zip(suffixes.iter()).enumerate() {
        render_metric_row(
            f,
            Rect {
                y: area
                    .y
                    .saturating_add(1 + u16::try_from(idx).unwrap_or(u16::MAX)),
                height: 1,
                ..area
            },
            row,
            fleet_layout,
            suffix,
        );
    }

    // Drive-detail visibility is governed by the precomputed viewport entry,
    // whose visibility is based on logical selection.
    render_drive_details(f, area, snap, drive_rows_visible, base_height);
    render_network_details(
        f,
        area,
        snap,
        network_rows_visible,
        base_height.saturating_add(u16::try_from(drive_rows_visible).unwrap_or(u16::MAX)),
    );
}

fn render_waiting(f: &mut Frame, area: Rect, system: &SystemState, is_visually_selected: bool) {
    let sel_style = if is_visually_selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };
    let header = text::header_line(system, area.width);
    f.render_widget(
        Line::from(Span::styled(header, sel_style)),
        Rect { height: 1, ..area },
    );
    f.render_widget(
        Line::from(Span::styled("waiting for data…", sel_style)),
        Rect {
            y: area.y.saturating_add(1),
            height: 1,
            ..area
        },
    );
}

/// One metric row inside the normal-view block.
#[derive(Debug, Clone)]
pub(crate) struct MetricRow {
    /// Label such as `CPU`, `MEM`, `SWP`, `COMMIT`, `DISK`.
    label: &'static str,
    /// Percentage; `None` means the metric is unavailable and the row
    /// should render the shared unavailable marker instead of fabricating
    /// a zero percentage.
    pct: Option<f32>,
    /// Optional detail printed after the percentage when width allows.
    detail: Option<String>,
}

impl MetricRow {
    /// Available percentage plus optional detail, as the line content
    /// will be built. For unavailable rows returns the shared
    /// unavailable marker.
    fn default_suffix(&self) -> String {
        match self.pct {
            None => self.detail.clone().unwrap_or_else(|| "—".to_string()),
            Some(p) => {
                let pct = text::format_pct(p);
                match &self.detail {
                    None => pct,
                    Some(d) if d.is_empty() => pct,
                    Some(d) => format!("{pct} {d}"),
                }
            }
        }
    }

    /// Percentage-only suffix used when detail must be dropped to fit.
    fn percentage_only_suffix(&self) -> String {
        match self.pct {
            None => "—".to_string(),
            Some(p) => text::format_pct(p),
        }
    }

    /// Display-cell width of the metric label (used by the fleet layout
    /// to pick the fleet-wide label column).
    fn label_width(&self) -> u16 {
        u16::try_from(UnicodeWidthStr::width(self.label)).unwrap_or(u16::MAX)
    }
}

/// Fixed-capacity metric rows. NET is present only when the fleet policy
/// requests it; the `len` field keeps the all-legacy layout at four rows.
#[derive(Debug, Clone)]
pub(crate) struct MetricRows {
    rows: [MetricRow; 5],
    len: usize,
}

pub(crate) trait MetricRowSet {
    fn rows(&self) -> &[MetricRow];
}

impl MetricRows {
    fn as_slice(&self) -> &[MetricRow] {
        &self.rows[..self.len]
    }

    fn base_height(&self) -> u16 {
        u16::try_from(self.len + 1).unwrap_or(u16::MAX)
    }
}

/// Shared `len + 1` rule for the local-rebuild fallback (no prebuilt rows):
/// 4 rows (no NET) -> 5, 5 rows (NET) -> 6.
fn base_height_for(has_net: bool) -> u16 {
    if has_net {
        6
    } else {
        5
    }
}

impl MetricRowSet for MetricRows {
    fn rows(&self) -> &[MetricRow] {
        self.as_slice()
    }
}

impl MetricRowSet for [MetricRow; 4] {
    fn rows(&self) -> &[MetricRow] {
        self
    }
}

impl MetricRowSet for [MetricRow; 5] {
    fn rows(&self) -> &[MetricRow] {
        self
    }
}

/// Build metric rows for one snapshot. `include_network` is the fleet-wide
/// mixed-version alignment decision made by the render dispatcher.
pub(crate) fn build_metric_rows(snap: &NormalizedSnapshot, include_network: bool) -> MetricRows {
    let cpu = MetricRow {
        label: "CPU",
        pct: Some(snap.usage_pct),
        detail: Some(format!(
            "{} cores{}",
            snap.logical_cores,
            snap.cpu_frequency_hz
                .map(|hz| format!(" {}", text::format_frequency(hz)))
                .unwrap_or_default()
        )),
    };

    let mem = MetricRow {
        label: "MEM",
        pct: Some(snap.memory.usage_pct),
        detail: Some(format!(
            "{}/{}",
            text::format_bytes(snap.memory.used_bytes),
            text::format_bytes(snap.memory.total_bytes)
        )),
    };

    let third = match (&snap.swap, &snap.commit) {
        (Some(swap_info), _) => {
            let detail = if swap_info.total_bytes == 0 {
                None
            } else {
                Some(format!(
                    "{}/{}",
                    text::format_bytes(swap_info.used_bytes),
                    text::format_bytes(swap_info.total_bytes)
                ))
            };
            MetricRow {
                label: "SWP",
                pct: Some(swap_info.usage_pct),
                detail,
            }
        }
        (None, Some(commit_info)) => MetricRow {
            label: "COMMIT",
            pct: Some(commit_info.usage_pct),
            detail: Some(format!(
                "{}/{}",
                text::format_bytes(commit_info.used_bytes),
                text::format_bytes(commit_info.limit_bytes)
            )),
        },
        (None, None) => MetricRow {
            label: "SWP",
            pct: None,
            detail: None,
        },
    };

    let disk = match snap.drives.as_deref().and_then(aggregate_drives) {
        Some(aggregate) => {
            let used = text::format_bytes(aggregate.used_bytes);
            let total = text::format_bytes(aggregate.total_bytes);
            MetricRow {
                label: "DISK",
                pct: Some(aggregate.usage_pct),
                detail: Some(format!("{used} / {total}")),
            }
        }
        None => MetricRow {
            label: "DISK",
            pct: None,
            detail: None,
        },
    };

    let network = match snap.network.as_ref() {
        Some(network) => MetricRow {
            label: "NET",
            pct: network.aggregate_utilization_pct(),
            detail: Some(format!(
                "{} rx {} tx",
                text::format_rate(network.aggregate_rx_bytes_per_sec),
                text::format_rate(network.aggregate_tx_bytes_per_sec)
            )),
        },
        None => MetricRow {
            label: "NET",
            pct: None,
            detail: None,
        },
    };

    MetricRows {
        rows: [cpu, mem, third, disk, network],
        len: if include_network { 5 } else { 4 },
    }
}

/// Fleet-wide metric geometry shared by every online system block.
///
/// The renderer computes one instance of this struct per render and
/// passes it down so the opening `[` and closing `]` always line up
/// across devices. Per-row suffix strings live with each system because
/// they depend on that system's own metric values.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) struct MetricFleetLayout {
    /// Display-cell width of the longest metric label across every
    /// participating system (so mixed `SWP`/`COMMIT` fleets pick the
    /// wider label).
    pub label_width: u16,
    /// Common bar width shared by all active metric rows and every
    /// online system block in the same render.
    pub bar_width: u16,
    /// Plan 087: when false, normal metric rows render as bar-only —
    /// the text after the closing `]` (percentage, byte counts) is
    /// suppressed fleet-wide. The decision is made once per render
    /// from the longest natural suffix across the entire online fleet.
    pub show_suffix: bool,
}

impl MetricFleetLayout {
    /// Default layout used when the fleet has no online systems with a
    /// usable snapshot yet. The bar width is zero so the renderer
    /// falls back to the no-bracket path.
    pub(crate) fn empty() -> Self {
        let label_width = u16::try_from(METRIC_ROW_INDENT.len()).unwrap_or(0);
        Self {
            label_width,
            bar_width: 0,
            show_suffix: true,
        }
    }
}

/// Compute the shared fleet metric geometry from the rows of every
/// participating online system.
///
/// `rows` is an iterator of references to each system's active metric
/// rows. The resulting layout picks the widest label seen across the
/// fleet (so mixed Linux/Windows fleets keep `COMMIT` wider than `SWP`)
/// and reserves a bar width that is uniform across every system.
///
/// Plan 087: when the longest *natural* suffix across the fleet would
/// occupy more than one quarter of the available terminal width, the
/// entire normal-view suffix region disappears. The fleet then renders
/// pure bar-only rows, the bar grows by the cell that would otherwise
/// be the suffix separator, and the existing Plan 085/086 percentage
/// fallback is not consulted for visible output (no suffix is rendered).
pub(crate) fn compute_fleet_metric_layout<'a, I, T>(rows: I, width: u16) -> MetricFleetLayout
where
    I: IntoIterator<Item = &'a T>,
    T: MetricRowSet + 'a,
{
    let collected: Vec<&'a T> = rows.into_iter().collect();

    let mut label_width: u16 = 0;
    for system_rows in &collected {
        for row in system_rows.rows() {
            label_width = label_width.max(row.label_width());
        }
    }

    // Plan 087: compute the longest natural (full-detail) suffix
    // across every participating system before deciding whether to
    // suppress suffixes at all. The decision uses the natural width
    // so it describes the content the operator is trying to hide, not
    // an already-truncated result of the suffix resolver.
    let mut max_natural_suffix: usize = 0;
    for system_rows in &collected {
        let natural: Vec<String> = system_rows
            .rows()
            .iter()
            .map(MetricRow::default_suffix)
            .collect();
        let w = max_suffix_display(&natural);
        if w > max_natural_suffix {
            max_natural_suffix = w;
        }
    }

    let show_suffix = !should_suppress_suffix(width, max_natural_suffix);

    if !show_suffix {
        // Bar-only mode: the row terminator is the closing `]`. No
        // gap, no suffix text — the bar can claim every remaining cell.
        let prefix_w = metric_compact_prefix_width(label_width);
        let bar_width = width.saturating_sub(prefix_w);
        return MetricFleetLayout {
            label_width,
            bar_width,
            show_suffix: false,
        };
    }

    // Fixed structural prefix/suffix widths:
    //   prefix:  METRIC_ROW_INDENT + label + ' ['
    //   suffix:  '] ' + suffix_text
    let prefix_w = metric_prefix_width(label_width);
    let after_bracket_w: u16 = METRIC_SUFFIX_GAP_CELLS;

    // Total budget available for "suffix bar + suffix text" before the
    // bar width is chosen.
    let total_suffix_budget = usize::from(width.saturating_sub(prefix_w + after_bracket_w));

    // Resolve each system's per-row suffix strings against the full
    // budget, then take the widest suffix seen across the fleet. The
    // shared bar width is what remains after subtracting that widest
    // suffix from the full budget.
    let mut max_suffix: usize = 0;
    for system_rows in &collected {
        let suffixes = resolve_metric_suffixes(*system_rows, total_suffix_budget);
        let width_for_system = max_suffix_display(&suffixes);
        if width_for_system > max_suffix {
            max_suffix = width_for_system;
        }
    }

    let bar_width = width
        .saturating_sub(prefix_w + after_bracket_w)
        .saturating_sub(u16::try_from(max_suffix).unwrap_or(u16::MAX));

    MetricFleetLayout {
        label_width,
        bar_width,
        show_suffix: true,
    }
}

/// Plan 087: decide whether the fleet's normal-view suffix region
/// should be suppressed.
///
/// Suppression is strict and integer-safe:
///
/// ```text
/// hide suffixes when:
///     longest_suffix_display_width * 4 > terminal_width
/// ```
///
/// This is equivalent to `longest > terminal / 4` without the
/// rounding ambiguity introduced by integer division. Widths are in
/// terminal display cells; the same cell width is used to compose
/// and render the suffix.
pub(crate) fn should_suppress_suffix(width: u16, longest_suffix_width: usize) -> bool {
    let width_cells = usize::from(width);
    longest_suffix_width.saturating_mul(4) > width_cells
}

/// Resolve the suffix strings for one system against the fleet's
/// shared geometry. The available suffix budget is computed from the
/// fleet-wide `label_width`, so mixed Linux/Windows fleets (where
/// `COMMIT` widens the label column) do not silently let the Linux
/// row retain detail that overflows the rendered fleet geometry.
///
/// Plan 087: when the fleet layout chose `show_suffix == false`,
/// every row renders empty suffixes and the renderer omits the `] `
/// separator so the bar can claim the cells that would otherwise be
/// the suffix budget.
fn resolve_system_suffixes<T: MetricRowSet>(
    rows: &T,
    width: u16,
    fleet_layout: MetricFleetLayout,
) -> Vec<String> {
    if !fleet_layout.show_suffix {
        return rows.rows().iter().map(|_| String::new()).collect();
    }
    let prefix_w = metric_prefix_width(fleet_layout.label_width);
    let after_bracket_w: u16 = METRIC_SUFFIX_GAP_CELLS;
    let suffix_budget = width
        .saturating_sub(prefix_w + after_bracket_w)
        .saturating_sub(fleet_layout.bar_width);
    resolve_metric_suffixes(rows, usize::from(suffix_budget))
}

fn resolve_metric_suffixes<T: MetricRowSet>(rows: &T, budget: usize) -> Vec<String> {
    if budget == 0 {
        // No room for a suffix at all. The renderer still emits "] "
        // for layout consistency; an empty suffix keeps brackets adjacent.
        return rows.rows().iter().map(|_| String::new()).collect();
    }

    // First pass: full details.
    let mut suffixes: Vec<String> = rows.rows().iter().map(MetricRow::default_suffix).collect();
    if max_suffix_display(&suffixes) <= budget {
        return suffixes;
    }

    // Second pass: drop optional details. Some metrics are unavailable
    // (no percentage) or have no detail field; those already collapsed
    // to the percentage-only form on the first pass.
    suffixes = rows
        .rows()
        .iter()
        .map(MetricRow::percentage_only_suffix)
        .collect();
    if max_suffix_display(&suffixes) <= budget {
        return suffixes;
    }

    // Third pass: truncate every suffix so the longest one fits. The
    // helper guarantees the returned width is `<= budget`.
    suffixes
        .into_iter()
        .map(|s| bar::truncate_to_cells(&s, budget))
        .collect()
}

fn max_suffix_display(suffixes: &[String]) -> usize {
    suffixes
        .iter()
        .map(|s| UnicodeWidthStr::width(s.as_str()))
        .max()
        .unwrap_or(0)
}

/// Render a single metric row using the shared fleet geometry.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn render_metric_row(
    f: &mut Frame,
    area: Rect,
    row: &MetricRow,
    layout: &MetricFleetLayout,
    suffix: &str,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let label_padded = format!(
        "{}{label:<width$}",
        METRIC_ROW_INDENT,
        label = row.label,
        width = usize::from(layout.label_width)
    );
    let bar = make_bar_string(row.pct, layout.bar_width);

    let line = if layout.bar_width == 0 {
        // No bar budget at all — print label and suffix without brackets.
        if suffix.is_empty() {
            label_padded
        } else {
            format!("{label_padded}  {suffix}")
        }
    } else if !layout.show_suffix {
        // Plan 087: bar-only fleet mode suppresses every cell after the
        // closing `]`. The suffix separator that would normally follow
        // is returned to the bar budget, so the rendered shape is
        // exactly `<label prefix> [<bar>]` with no trailing space.
        format!("{label_padded} [{bar}]")
    } else {
        format!("{label_padded} [{bar}] {suffix}")
    };

    bar::render_text_line(f, area, &line);
}

fn make_bar_string(pct: Option<f32>, bar_width: u16) -> String {
    match pct {
        Some(p) => {
            let clamped = p.clamp(0.0, 100.0);
            let width_f = f32::from(bar_width);
            // `clamped` is in [0, 100] and `width_f` fits in u16, so the
            // scaled value is non-negative and bounded by `width_f`. We
            // explicitly clamp before converting to avoid lossy-cast
            // lints (Rust 1.75 lacks `TryFrom<f32>` for integer targets).
            let scaled = (clamped * width_f) / 100.0;
            let clamped_scaled = if scaled.is_finite() && scaled >= 0.0 {
                scaled.min(f32::from(u16::MAX))
            } else {
                0.0
            };
            // The value is pre-clamped to [0, u16::MAX]; Rust 1.75 has no
            // TryFrom<f32> for this conversion.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let filled_u32 = clamped_scaled as u32;
            let filled = usize::try_from(filled_u32.min(u32::from(bar_width))).unwrap_or(0);
            let empty = (usize::from(bar_width)).saturating_sub(filled);
            format!("{}{}", "|".repeat(filled), " ".repeat(empty))
        }
        None => " ".repeat(usize::from(bar_width)),
    }
}

fn render_drive_details(
    f: &mut Frame,
    area: Rect,
    snap: &NormalizedSnapshot,
    drive_rows_visible: usize,
    base_height: u16,
) {
    if drive_rows_visible == 0 {
        return;
    }
    let drives = snap.drives.as_deref().unwrap_or(&[]);
    // Plan 085: compute the table layout from every eligible drive so
    // vertical clipping never shifts horizontal columns.
    let lines = text::render_drive_detail_lines_with_io(drives, snap.disk_io.as_ref(), area.width);
    for (offset, line) in lines.into_iter().take(drive_rows_visible).enumerate() {
        let row = area
            .y
            .saturating_add(base_height + u16::try_from(offset).unwrap_or(u16::MAX));
        if row >= area.y.saturating_add(area.height) {
            break;
        }
        bar::render_text_line(
            f,
            Rect {
                y: row,
                height: 1,
                ..area
            },
            &line,
        );
    }
}

fn render_network_details(
    f: &mut Frame,
    area: Rect,
    snap: &NormalizedSnapshot,
    rows_visible: usize,
    start_offset: u16,
) {
    if rows_visible == 0 {
        return;
    }
    let Some(network) = snap.network.as_ref() else {
        return;
    };
    for (offset, line) in text::render_network_detail_lines(network, area.width)
        .into_iter()
        .take(rows_visible)
        .enumerate()
    {
        let row = area
            .y
            .saturating_add(start_offset.saturating_add(u16::try_from(offset).unwrap_or(u16::MAX)));
        if row >= area.y.saturating_add(area.height) {
            break;
        }
        bar::render_text_line(
            f,
            Rect {
                y: row,
                height: 1,
                ..area
            },
            &line,
        );
    }
}

/// Render a 1-row offline system line.
pub fn render_offline(f: &mut Frame, area: Rect, system: &SystemState, is_visually_selected: bool) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let style = if is_visually_selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };

    let status_text = match system.reachability {
        crate::state::Reachability::Offline => "offline",
        crate::state::Reachability::Pending => "pending",
        crate::state::Reachability::Online => "online",
    };

    // Named systems show `name@host:port`; unnamed systems render only
    // `host:port` so we never duplicate the host as both the synthetic
    // name and the address suffix.
    let prefix = match system.configured_name.as_deref() {
        Some(name) => format!("{name}@{}", system.endpoint.display_address()),
        None => system.endpoint.display_address(),
    };

    // Offline systems with known provenance show the stable failure
    // category (`offline (refused)`). Pending systems have no poll result
    // yet, so they never carry a reason. The suffix is part of the
    // truncated prefix, so width bounding and dot-fill below are unchanged.
    let reason_suffix = match (&system.reachability, &system.offline_reason) {
        (crate::state::Reachability::Offline, Some(reason)) => match reason.detail.as_deref() {
            Some(detail) => format!(" ({}) {detail}", reason.kind),
            None => format!(" ({})", reason.kind),
        },
        _ => String::new(),
    };

    let prefix_with_status = format!("{prefix} {status_text}{reason_suffix} ");
    let total_width = area.width as usize;
    // Bound the prefix to the terminal width so a long name@host:port
    // cannot push the status text into clipped cells.
    let prefix_with_status = bar::truncate_to_cells(&prefix_with_status, total_width);
    let used = UnicodeWidthStr::width(prefix_with_status.as_str());
    let dot_count = total_width.saturating_sub(used);
    let line_text = format!("{prefix_with_status}{}", ".".repeat(dot_count));

    let line = Line::from(Span::styled(line_text, style));
    f.render_widget(line, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(label: &'static str, pct: f32, detail: Option<&str>) -> MetricRow {
        MetricRow {
            label,
            pct: Some(pct),
            detail: detail.map(str::to_string),
        }
    }

    #[test]
    fn label_width_uses_widest_actual_label() {
        let rows = [
            row("CPU", 1.0, None),
            row("MEM", 1.0, None),
            row("COMMIT", 1.0, None),
            row("DISK", 1.0, None),
        ];
        let layout = compute_fleet_metric_layout([&rows], 80);
        assert_eq!(layout.label_width, u16::try_from("COMMIT".len()).unwrap());
    }

    #[test]
    fn make_bar_string_covers_boundaries_directly() {
        // Missing percentage renders an entirely empty bar.
        assert_eq!(make_bar_string(None, 10), "          ");
        assert_eq!(make_bar_string(None, 0), "");

        // Zero width collapses to the empty string regardless of pct.
        assert_eq!(make_bar_string(Some(50.0), 0), "");
        assert_eq!(make_bar_string(Some(100.0), 0), "");

        // Exact bounds fill nothing or everything.
        assert_eq!(make_bar_string(Some(0.0), 8), "        ");
        assert_eq!(make_bar_string(Some(100.0), 8), "||||||||");

        // Out-of-range values clamp into the closed interval.
        assert_eq!(make_bar_string(Some(-5.0), 4), "    ");
        assert_eq!(make_bar_string(Some(150.0), 4), "||||");

        // Non-finite input cannot panic or fabricate a partial bar:
        // infinity clamps to the full bar and NaN falls back to zero
        // filled cells.
        assert_eq!(make_bar_string(Some(f32::NAN), 6), "      ");
        assert_eq!(make_bar_string(Some(f32::INFINITY), 6), "||||||");

        // Fractional percentages truncate toward fewer filled cells.
        assert_eq!(make_bar_string(Some(49.9), 10), "||||      ");
    }

    #[test]
    fn closing_brackets_align_when_details_fit() {
        let rows = [
            row("CPU", 25.2, Some("8 cores")),
            row("MEM", 37.8, Some("5.9 GiB / 15.6 GiB")),
            row("SWP", 0.0, Some("0 B / 4.0 GiB")),
            row("DISK", 60.4, Some("283.8 GiB / 167.1 GiB")),
        ];
        let layout = compute_fleet_metric_layout([&rows], 80);
        let suffixes = resolve_system_suffixes(&rows, 80, layout);
        // The closing `]` must fall in the same terminal column for
        // every row because `bar_width` is shared.
        for (idx, (r, suffix)) in rows.iter().zip(suffixes.iter()).enumerate() {
            let line = build_row_line(r, &layout, suffix);
            assert_eq!(
                line.rfind(']').unwrap(),
                4 /* indent */ + usize::from(layout.label_width) + 2 + usize::from(layout.bar_width),
                "row {:?} closing bracket drifted: {line:?}",
                r.label,
            );
            let _ = idx;
        }
        // All brackets and suffixes end at the same absolute column.
        let closes: Vec<_> = [
            ("CPU", 0usize, "—"),
            ("MEM", 1, "—"),
            ("SWP", 2, "—"),
            ("DISK", 3, "—"),
        ]
        .iter()
        .map(|(label, _idx, suffix)| {
            let r = row(label, 0.0, None);
            let line = build_row_line(&r, &layout, suffix);
            line.rfind(']').unwrap()
        })
        .collect();
        assert!(
            closes.iter().all(|&c| c == closes[0]),
            "closing brackets must align across rows, got {closes:?}"
        );
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn build_row_line(row: &MetricRow, layout: &MetricFleetLayout, suffix: &str) -> String {
        let label_padded = format!(
            "{}{label:<width$}",
            METRIC_ROW_INDENT,
            label = row.label,
            width = usize::from(layout.label_width)
        );
        if layout.bar_width == 0 {
            if suffix.is_empty() {
                label_padded
            } else {
                format!("{label_padded}  {suffix}")
            }
        } else if !layout.show_suffix {
            // Plan 087: bar-only fleet mode suppresses the suffix
            // separator; the row terminates at `]` with no trailing
            // space.
            format!(
                "{label_padded} [{bar}]",
                bar = make_bar_string(row.pct, layout.bar_width)
            )
        } else {
            format!(
                "{label_padded} [{bar}] {suffix}",
                bar = make_bar_string(row.pct, layout.bar_width)
            )
        }
    }

    #[test]
    fn compact_mode_suppresses_suffixes_when_natural_exceeds_one_quarter() {
        // Plan 087: at narrow widths the natural metric suffix already
        // exceeds one quarter of the terminal width, so the entire
        // fleet drops every suffix and the metric rows render as pure
        // bar-only rows.
        let rows = [
            row("CPU", 25.2, Some("8 cores")),
            row("MEM", 37.8, Some("5.9 GiB / 15.6 GiB")),
            row("SWP", 0.0, None),
            row("DISK", 60.4, Some("283.8 GiB / 167.1 GiB")),
        ];
        let layout = compute_fleet_metric_layout([&rows], 30);
        assert!(
            !layout.show_suffix,
            "narrow width must suppress the suffix region: layout={layout:?}"
        );
        let suffixes = resolve_system_suffixes(&rows, 30, layout);
        for suffix in &suffixes {
            assert!(
                suffix.is_empty(),
                "compact mode must not render any suffix text: {suffix:?}"
            );
        }
        let rendered = build_row_line(&rows[0], &layout, &suffixes[0]);
        assert!(
            !rendered.contains('%'),
            "compact mode must omit percentage: {rendered:?}"
        );
        assert!(
            rendered.ends_with(']'),
            "compact row must terminate at `]` with no trailing suffix: {rendered:?}"
        );
        assert!(
            UnicodeWidthStr::width(rendered.as_str()) <= 30,
            "compact row must fit terminal width: {rendered:?}"
        );
    }

    #[test]
    fn should_suppress_suffix_one_quarter_boundary_keeps_suffix() {
        // Plan 087: the strict integer-safe boundary
        // `longest * 4 > width` means a suffix exactly one quarter of
        // the terminal width must keep suffixes visible.
        assert!(!should_suppress_suffix(80, 20));
        assert!(!should_suppress_suffix(40, 10));
        assert!(!should_suppress_suffix(24, 6));
    }

    #[test]
    fn should_suppress_suffix_one_cell_above_quarter_disables_suffix() {
        // Plan 087: one cell above the one-quarter boundary trips
        // suppression.
        assert!(should_suppress_suffix(80, 21));
        assert!(should_suppress_suffix(40, 11));
        assert!(should_suppress_suffix(24, 7));
    }

    #[test]
    fn should_suppress_suffix_helper_compares_in_display_cells() {
        // Plan 087: the helper applies `longest * 4 > width` in
        // integer arithmetic. The caller is responsible for measuring
        // the suffix in terminal display cells, which the production
        // site does via `UnicodeWidthStr::width`. Verify the boundary
        // arithmetic with concrete numeric inputs so a regression in
        // the threshold formula is caught.
        assert!(!should_suppress_suffix(80, 0));
        assert!(!should_suppress_suffix(80, 20));
        assert!(should_suppress_suffix(80, 21));
        assert!(!should_suppress_suffix(40, 10));
        assert!(should_suppress_suffix(40, 11));
        // Integer overflow protection: an enormous suffix must not
        // wrap; comparison remains well-defined for u16 widths.
        assert!(should_suppress_suffix(u16::MAX, usize::MAX));
        assert!(!should_suppress_suffix(u16::MAX, 0));
    }

    #[test]
    fn compact_mode_off_viewport_systems_participate_in_fleet_decision() {
        // Plan 087: an off-viewport system with the longest natural
        // suffix must drive the fleet-wide compact-mode decision the
        // same way an in-viewport system would, because the fleet
        // layout is computed from every online system with a snapshot.
        let tiny = [
            row("CPU", 25.0, Some("4 cores")),
            row("MEM", 30.0, Some("1.0 GiB / 4.0 GiB")),
            row("SWP", 0.0, None),
            row("DISK", 25.0, Some("50.0 GiB / 200.0 GiB")),
        ];
        let wide = [
            row("CPU", 25.0, Some("128 cores")),
            row("MEM", 50.0, Some("8.0 GiB / 16.0 GiB")),
            row("SWP", 0.0, None),
            row("DISK", 90.0, Some("1.2 TiB / 1.4 TiB")),
        ];
        // Build a fleet of three systems: two tiny in-viewport systems
        // and one with the wide natural suffix that would otherwise
        // sit below the viewport. Compact mode must engage regardless
        // of which system "owns" the longest natural suffix.
        let layout = compute_fleet_metric_layout([&tiny, &tiny, &wide], 80);
        assert!(
            !layout.show_suffix,
            "wide-suffix system must trigger fleet-wide compact mode: {layout:?}"
        );
    }

    #[test]
    fn compact_mode_renders_no_trailing_separator_after_bracket() {
        // Plan 087: in compact mode the row must terminate exactly at
        // the closing `]`. There must be no trailing suffix separator
        // (no " " or "  ") between `]` and the line terminator.
        let rows = [
            row("CPU", 25.0, Some("4 cores")),
            row("MEM", 30.0, Some("1.0 GiB / 4.0 GiB")),
            row("SWP", 0.0, None),
            row("DISK", 90.0, Some("1.2 TiB / 1.4 TiB")),
        ];
        let layout = compute_fleet_metric_layout([&rows], 32);
        assert!(!layout.show_suffix);
        let suffixes = resolve_system_suffixes(&rows, 32, layout);
        for (idx, row_) in rows.iter().enumerate() {
            let line = build_row_line(row_, &layout, &suffixes[idx]);
            let close = line.rfind(']').expect("] present");
            let after = &line[close + 1..];
            assert!(
                after.is_empty(),
                "no characters after `]` in compact mode: {line:?}"
            );
        }
    }

    #[test]
    fn compact_mode_returns_to_full_suffixes_when_resized_wider() {
        // Plan 087: the threshold is computed from the terminal width
        // per render, so resizing a terminal from a narrow width that
        // suppresses suffixes back to a wide width must restore
        // suffixes without touching application state.
        let rows = [
            row("CPU", 25.0, Some("8 cores")),
            row("MEM", 50.0, Some("8.0 GiB / 16.0 GiB")),
            row("SWP", 0.0, None),
            row("DISK", 50.0, Some("50.0 GiB / 100.0 GiB")),
        ];
        let narrow = compute_fleet_metric_layout([&rows], 32);
        assert!(!narrow.show_suffix);
        let wide = compute_fleet_metric_layout([&rows], 120);
        assert!(wide.show_suffix);
    }

    #[test]
    fn truncation_respects_budget_for_display_cells() {
        let budget = 12;
        let suffix = bar::truncate_to_cells("283.8 GiB / 167.1 GiB", budget);
        assert!(UnicodeWidthStr::width(suffix.as_str()) <= budget);
    }

    #[test]
    fn truncation_at_zero_returns_empty() {
        assert_eq!(bar::truncate_to_cells("anything", 0), "");
    }

    #[test]
    fn unavailable_row_uses_em_dash_not_fabricated_zero() {
        let rows = [
            MetricRow {
                label: "CPU",
                pct: Some(50.0),
                detail: None,
            },
            MetricRow {
                label: "MEM",
                pct: Some(50.0),
                detail: None,
            },
            MetricRow {
                label: "SWP",
                pct: Some(50.0),
                detail: None,
            },
            MetricRow {
                label: "DISK",
                pct: None,
                detail: None,
            },
        ];
        let layout = compute_fleet_metric_layout([&rows], 80);
        let suffixes = resolve_system_suffixes(&rows, 80, layout);
        let disk = &suffixes[3];
        assert!(
            disk.contains('—'),
            "unavailable disk should use em-dash: {disk:?}"
        );
        assert!(
            !disk.contains("0.0%"),
            "unavailable disk must not fabricate 0.0%: {disk:?}"
        );
    }

    #[test]
    fn fleet_layout_picks_widest_label_across_systems() {
        // Mix a Linux-shaped (SWP) system with a Windows-shaped
        // (COMMIT) system; the fleet label column must match the wider
        // COMMIT label.
        let linux_rows = [
            row("CPU", 25.0, Some("8 cores")),
            row("MEM", 50.0, Some("8.0 GiB / 16.0 GiB")),
            row("SWP", 0.0, Some("0 B / 4.0 GiB")),
            row("DISK", 0.0, None),
        ];
        let windows_rows = [
            row("CPU", 25.0, Some("8 cores")),
            row("MEM", 50.0, Some("8.0 GiB / 16.0 GiB")),
            row("COMMIT", 50.0, Some("4.0 GiB / 8.0 GiB")),
            row("DISK", 0.0, None),
        ];
        let layout = compute_fleet_metric_layout([&linux_rows, &windows_rows], 120);
        assert_eq!(
            layout.label_width,
            u16::try_from("COMMIT".len()).unwrap(),
            "fleet label column must use the wider COMMIT label"
        );
    }

    #[test]
    fn fleet_layout_keeps_brackets_aligned_when_suffixes_differ() {
        // Two systems whose natural suffix widths differ substantially;
        // the shared bar_width chosen by the fleet layout must keep
        // every opening `[` and closing `]` in the same terminal column
        // across both systems even when one needs a wider bar than the
        // other would by itself.
        let small = [
            row("CPU", 25.0, Some("4 cores")),
            row("MEM", 30.0, Some("1.0 GiB / 4.0 GiB")),
            row("SWP", 0.0, None),
            row("DISK", 25.0, Some("100.0 GiB / 400.0 GiB")),
        ];
        let large = [
            row("CPU", 60.0, Some("128 cores")),
            row("MEM", 80.0, Some("120.0 GiB / 150.0 GiB")),
            row("SWP", 0.0, None),
            row("DISK", 90.0, Some("1.2 TiB / 1.4 TiB")),
        ];
        let layout = compute_fleet_metric_layout([&small, &large], 120);

        let suffixes_small = resolve_system_suffixes(&small, 120, layout);
        let suffixes_large = resolve_system_suffixes(&large, 120, layout);

        let line_small = build_row_line(&small[0], &layout, &suffixes_small[0]);
        let line_large = build_row_line(&large[0], &layout, &suffixes_large[0]);
        assert_eq!(
            line_small.rfind(']'),
            line_large.rfind(']'),
            "closing bracket column must agree across systems in a fleet"
        );
        assert_eq!(
            line_small.find('['),
            line_large.find('['),
            "opening bracket column must agree across systems in a fleet"
        );
    }

    fn suffixes_max_width(suffixes: &[String; 4]) -> usize {
        suffixes
            .iter()
            .map(|s| UnicodeWidthStr::width(s.as_str()))
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn mixed_label_fleet_suffix_budget_uses_fleet_label_width() {
        // Plan 086: a Linux system with a long detail must have its
        // suffix budget computed from the fleet-wide `COMMIT` label
        // width, not the local `SWP` label width. Otherwise the
        // rendered line will exceed the terminal width and be clipped
        // by the backend.
        //
        // Plan 087: at width=80 the Linux MEM detail would push the
        // natural suffix past one quarter of the terminal width and
        // trip compact mode, leaving the resolver without a suffix to
        // budget. Verify the Plan 086 invariant at a width where the
        // suffix remains visible (compact mode off).
        let linux = [
            row("CPU", 25.0, Some("128 cores")),
            row("MEM", 80.0, Some("120.0 GiB / 150.0 GiB")),
            row("SWP", 0.0, Some("0 B / 4.0 GiB")),
            row("DISK", 90.0, Some("1.2 TiB / 1.4 TiB")),
        ];
        let windows = [
            row("CPU", 25.0, Some("128 cores")),
            row("MEM", 80.0, Some("120.0 GiB / 150.0 GiB")),
            row("COMMIT", 50.0, Some("4.0 GiB / 8.0 GiB")),
            row("DISK", 90.0, Some("1.2 TiB / 1.4 TiB")),
        ];
        let width = 120u16;
        let layout = compute_fleet_metric_layout([&linux, &windows], width);
        assert_eq!(
            layout.label_width,
            u16::try_from("COMMIT".len()).unwrap(),
            "fleet label column must use the wider COMMIT label"
        );
        assert!(
            layout.show_suffix,
            "compact mode must remain disabled at width 120 with these details"
        );
        let suffixes = resolve_system_suffixes(&linux, width, layout);
        let fleet_suffix_budget = usize::from(
            width
                .saturating_sub(metric_prefix_width(layout.label_width))
                .saturating_sub(2) // "] "
                .saturating_sub(layout.bar_width),
        );
        for suffix in &suffixes {
            let suffix_width = UnicodeWidthStr::width(suffix.as_str());
            assert!(
                suffix_width <= fleet_suffix_budget,
                "suffix '{suffix}' ({suffix_width} cells) exceeds fleet suffix budget {fleet_suffix_budget}",
            );
        }
        // The rendered line must remain width-bounded in terminal cells.
        for (row, suffix) in linux.iter().zip(suffixes.iter()) {
            let line = build_row_line(row, &layout, suffix);
            assert!(
                UnicodeWidthStr::width(line.as_str()) <= usize::from(width),
                "rendered line exceeds terminal width: {line:?}"
            );
        }
    }

    #[test]
    fn compact_mode_engages_for_mixed_label_fleet_at_narrow_width() {
        // Plan 087: in a mixed Linux/Windows fleet, the Linux system
        // is the one most likely to push the natural suffix past the
        // quarter-width boundary. Compact mode must engage fleet-wide
        // (not just on the wide-suffix system) so the bracket columns
        // stay aligned.
        let linux = [
            row("CPU", 25.0, Some("128 cores")),
            row("MEM", 80.0, Some("12000.0 GiB / 15000.0 GiB")),
            row("SWP", 0.0, Some("0 B / 4.0 GiB")),
            row("DISK", 90.0, Some("1.2 TiB / 1.4 TiB")),
        ];
        let windows = [
            row("CPU", 25.0, Some("128 cores")),
            row("MEM", 80.0, Some("120.0 GiB / 150.0 GiB")),
            row("COMMIT", 50.0, Some("4.0 GiB / 8.0 GiB")),
            row("DISK", 90.0, Some("1.2 TiB / 1.4 TiB")),
        ];
        let layout = compute_fleet_metric_layout([&linux, &windows], 80);
        assert!(
            !layout.show_suffix,
            "wide Linux MEM detail must trigger fleet-wide compact mode"
        );
        // Bracket columns must still align across the two systems.
        let linux_suffixes = resolve_system_suffixes(&linux, 80, layout);
        let windows_suffixes = resolve_system_suffixes(&windows, 80, layout);
        let linux_line = build_row_line(&linux[0], &layout, &linux_suffixes[0]);
        let windows_line = build_row_line(&windows[0], &layout, &windows_suffixes[0]);
        assert_eq!(
            linux_line.find('['),
            windows_line.find('['),
            "opening bracket columns must agree across mixed-label compact fleet"
        );
        assert_eq!(
            linux_line.rfind(']'),
            windows_line.rfind(']'),
            "closing bracket columns must agree across mixed-label compact fleet"
        );
    }

    #[test]
    fn disk_suffix_uses_total_not_available_when_they_differ() {
        // Plan 085: The percentage is computed against `used/total`, so the
        // slash denominator must match `total_bytes`, not `available_bytes`,
        // even when explicit caller-available capacity differs from the
        // filesystem reservation semantics.
        use crate::normalized::NormalizedDrive;
        const GIB: u64 = 1024 * 1024 * 1024;
        let mut snap = base_disk_snapshot();
        snap.drives = Some(vec![
            NormalizedDrive {
                name: "/".into(),
                used_bytes: 80 * GIB,
                total_bytes: 100 * GIB,
                available_bytes: Some(10 * GIB),
            },
            NormalizedDrive {
                name: "/home".into(),
                used_bytes: 80 * GIB,
                total_bytes: 100 * GIB,
                available_bytes: Some(10 * GIB),
            },
        ]);

        let rows = build_metric_rows(&snap, false);
        let disk = &rows.rows()[3];
        let pct = disk.pct.expect("disk pct available");
        assert!((pct - 80.0).abs() < 0.01, "percentage = {pct}");
        let detail = disk.detail.as_ref().expect("disk detail present");
        assert!(
            detail.contains("160.0 GiB / 200.0 GiB"),
            "detail must use total as denominator, got {detail:?}"
        );
        assert!(
            !detail.contains("20.0 GiB"),
            "detail must not use available as denominator, got {detail:?}"
        );
    }

    #[test]
    fn disk_suffix_uses_total_when_drive_falls_back_to_total_minus_used() {
        // The compatibility fallback (`total_bytes - used_bytes`) must
        // remain untouched by Plan 085: it is computed downstream only when
        // `available_bytes` is `None`. The aggregate still uses total as the
        // slash denominator.
        use crate::normalized::NormalizedDrive;
        const GIB: u64 = 1024 * 1024 * 1024;
        let mut snap = base_disk_snapshot();
        snap.drives = Some(vec![
            NormalizedDrive {
                name: "/".into(),
                used_bytes: 80 * GIB,
                total_bytes: 100 * GIB,
                available_bytes: None,
            },
            NormalizedDrive {
                name: "/home".into(),
                used_bytes: 80 * GIB,
                total_bytes: 100 * GIB,
                available_bytes: None,
            },
        ]);

        let rows = build_metric_rows(&snap, false);
        let disk = &rows.rows()[3];
        let detail = disk.detail.as_ref().expect("disk detail present");
        assert!(
            detail.contains("160.0 GiB / 200.0 GiB"),
            "fallback availability still uses total denominator, got {detail:?}"
        );
    }

    #[test]
    fn cpu_frequency_and_network_rows_are_optional_and_truthful() {
        let payload = gregg_protocol::test_support::LinuxSnapshotV2Builder::default()
            .cpu_frequency_hz(Some(2_400_000_000))
            .network(Some(gregg_protocol::v2::NetworkPayload {
                aggregate_rx_bytes_per_sec: 10 * 1024 * 1024,
                aggregate_tx_bytes_per_sec: 5 * 1024 * 1024,
                aggregate_rx_capacity_bps: None,
                aggregate_tx_capacity_bps: None,
                interfaces: vec![],
            }))
            .build_payload();
        let snap = NormalizedSnapshot::from_v2_payload(&payload);
        let rows = build_metric_rows(&snap, true);
        assert_eq!(rows.rows().len(), 5);
        assert!(rows.rows()[0].default_suffix().contains("2.40GHz"));
        assert!(rows.rows()[4].pct.is_none());
        assert!(rows.rows()[4].default_suffix().contains("MiB/s"));

        let legacy = NormalizedSnapshot::from_v1(
            &gregg_protocol::test_support::LinuxSnapshotBuilder::default().build(),
        );
        let legacy_rows = build_metric_rows(&legacy, false);
        assert_eq!(legacy_rows.rows().len(), 4);
        assert!(legacy_rows.rows().iter().all(|row| row.label != "NET"));
    }

    fn base_disk_snapshot() -> NormalizedSnapshot {
        let v2_payload =
            gregg_protocol::test_support::LinuxSnapshotV2Builder::default().build_payload();
        let mut snap = NormalizedSnapshot::from_v2_payload(&v2_payload);
        snap.drives = None;
        snap
    }
}
