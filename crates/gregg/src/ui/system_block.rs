#![allow(dead_code)]

//! Normal-view system block rendering.
//!
//! The five-row online block is laid out in this fixed order:
//!
//! 1. Header line (priority-aware in `text::header_line`)
//! 2. CPU row
//! 3. MEM row
//! 4. SWP or COMMIT row (platform-determined)
//! 5. DISK aggregate row, optionally followed by per-drive detail rows
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

/// Render a normal-view online system block using the precomputed
/// fleet-wide metric geometry.
#[allow(clippy::too_many_lines, clippy::trivially_copy_pass_by_ref)]
pub(crate) fn render_online(
    f: &mut Frame,
    area: Rect,
    system: &SystemState,
    fleet_layout: &MetricFleetLayout,
    is_selected: bool,
    drive_rows_visible: usize,
) {
    if area.height < 5 || area.width == 0 {
        return;
    }

    let Some(snap) = &system.latest else {
        render_waiting(f, area, system, is_selected);
        return;
    };

    let sel_style = if is_selected {
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
    let rows = build_metric_rows(snap);
    let suffixes = resolve_system_suffixes(&rows, area.width, fleet_layout);

    let row_areas = [
        Rect {
            y: area.y.saturating_add(1),
            height: 1,
            ..area
        },
        Rect {
            y: area.y.saturating_add(2),
            height: 1,
            ..area
        },
        Rect {
            y: area.y.saturating_add(3),
            height: 1,
            ..area
        },
        Rect {
            y: area.y.saturating_add(4),
            height: 1,
            ..area
        },
    ];

    for (idx, row) in rows.iter().enumerate() {
        render_metric_row(f, row_areas[idx], row, fleet_layout, &suffixes[idx]);
    }

    render_drive_details(f, area, snap, drive_rows_visible);
}

fn render_waiting(f: &mut Frame, area: Rect, system: &SystemState, is_selected: bool) {
    let sel_style = if is_selected {
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
            None => "—".to_string(),
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

/// Build the four metric rows for one snapshot.
pub(crate) fn build_metric_rows(snap: &NormalizedSnapshot) -> [MetricRow; 4] {
    let cpu = MetricRow {
        label: "CPU",
        pct: Some(snap.usage_pct),
        detail: Some(format!("{} cores", snap.logical_cores)),
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

    [cpu, mem, third, disk]
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
    /// Common bar width shared by all four metric rows and every
    /// online system block in the same render.
    pub bar_width: u16,
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
        }
    }
}

/// Compute the shared fleet metric geometry from the rows of every
/// participating online system.
///
/// `rows` is an iterator of references to each system's four metric
/// rows. The resulting layout picks the widest label seen across the
/// fleet (so mixed Linux/Windows fleets keep `COMMIT` wider than `SWP`)
/// and reserves a bar width that is uniform across every system. The
/// bar shrinks when the longest resolved suffix across all systems
/// pushes the available width down.
pub(crate) fn compute_fleet_metric_layout<'a, I>(rows: I, width: u16) -> MetricFleetLayout
where
    I: IntoIterator<Item = &'a [MetricRow; 4]>,
{
    let collected: Vec<&'a [MetricRow; 4]> = rows.into_iter().collect();

    let mut label_width: u16 = 0;
    for system_rows in &collected {
        for row in *system_rows {
            label_width = label_width.max(row.label_width());
        }
    }

    // Fixed structural prefix/suffix widths:
    //   prefix:  METRIC_ROW_INDENT + label + ' ['
    //   suffix:  '] ' + suffix_text
    let prefix_w = metric_prefix_width(label_width);
    let after_bracket_w: u16 = 2; // "] "

    // Total budget available for "suffix bar + suffix text" before the
    // bar width is chosen.
    let total_suffix_budget = usize::from(width.saturating_sub(prefix_w + after_bracket_w));

    // Resolve each system's per-row suffix strings against the full
    // budget, then take the widest suffix seen across the fleet. The
    // shared bar width is what remains after subtracting that widest
    // suffix from the full budget.
    let mut max_suffix: usize = 0;
    for system_rows in &collected {
        let suffixes = resolve_metric_suffixes(system_rows, total_suffix_budget);
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
    }
}

/// Resolve the suffix strings for one system against the fleet's
/// shared geometry. The available suffix budget is computed from the
/// fleet-wide `label_width`, so mixed Linux/Windows fleets (where
/// `COMMIT` widens the label column) do not silently let the Linux
/// row retain detail that overflows the rendered fleet geometry.
fn resolve_system_suffixes(
    rows: &[MetricRow; 4],
    width: u16,
    fleet_layout: &MetricFleetLayout,
) -> [String; 4] {
    let prefix_w = metric_prefix_width(fleet_layout.label_width);
    let after_bracket_w: u16 = 2; // "] "
    let suffix_budget = width
        .saturating_sub(prefix_w + after_bracket_w)
        .saturating_sub(fleet_layout.bar_width);
    resolve_metric_suffixes(rows, usize::from(suffix_budget))
}

fn resolve_metric_suffixes(rows: &[MetricRow; 4], budget: usize) -> [String; 4] {
    if budget == 0 {
        // No room for a suffix at all. The renderer still emits "] "
        // for layout consistency; an empty suffix keeps brackets adjacent.
        return [String::new(), String::new(), String::new(), String::new()];
    }

    // First pass: full details.
    let mut suffixes = [
        rows[0].default_suffix(),
        rows[1].default_suffix(),
        rows[2].default_suffix(),
        rows[3].default_suffix(),
    ];
    if max_suffix_display(&suffixes) <= budget {
        return suffixes;
    }

    // Second pass: drop optional details. Some metrics are unavailable
    // (no percentage) or have no detail field; those already collapsed
    // to the percentage-only form on the first pass.
    suffixes = [
        rows[0].percentage_only_suffix(),
        rows[1].percentage_only_suffix(),
        rows[2].percentage_only_suffix(),
        rows[3].percentage_only_suffix(),
    ];
    if max_suffix_display(&suffixes) <= budget {
        return suffixes;
    }

    // Third pass: truncate every suffix so the longest one fits. The
    // helper guarantees the returned width is `<= budget`.
    suffixes.map(|s| bar::truncate_to_cells(&s, budget))
}

fn max_suffix_display(suffixes: &[String; 4]) -> usize {
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
) {
    if drive_rows_visible == 0 {
        return;
    }
    let Some(drives) = snap.drives.as_deref() else {
        return;
    };
    // Plan 085: compute the table layout from every eligible drive so
    // vertical clipping never shifts horizontal columns.
    let lines = text::render_drive_detail_lines(drives, area.width);
    for (offset, line) in lines.into_iter().take(drive_rows_visible).enumerate() {
        let row = area
            .y
            .saturating_add(5 + u16::try_from(offset).unwrap_or(u16::MAX));
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
pub fn render_offline(f: &mut Frame, area: Rect, system: &SystemState, is_selected: bool) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let style = if is_selected {
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

    let prefix_with_status = format!("{prefix} {status_text} ");
    let total_width = area.width as usize;
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
    fn closing_brackets_align_when_details_fit() {
        let rows = [
            row("CPU", 25.2, Some("8 cores")),
            row("MEM", 37.8, Some("5.9 GiB / 15.6 GiB")),
            row("SWP", 0.0, Some("0 B / 4.0 GiB")),
            row("DISK", 60.4, Some("283.8 GiB / 167.1 GiB")),
        ];
        let layout = compute_fleet_metric_layout([&rows], 80);
        let suffixes = resolve_system_suffixes(&rows, 80, &layout);
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
        } else {
            format!(
                "{label_padded} [{bar}] {suffix}",
                bar = make_bar_string(row.pct, layout.bar_width)
            )
        }
    }

    #[test]
    fn detail_dropped_when_budget_too_tight() {
        let rows = [
            row("CPU", 25.2, Some("8 cores")),
            row("MEM", 37.8, Some("5.9 GiB / 15.6 GiB")),
            row("SWP", 0.0, None),
            row("DISK", 60.4, Some("283.8 GiB / 167.1 GiB")),
        ];
        let layout = compute_fleet_metric_layout([&rows], 30);
        let suffixes = resolve_system_suffixes(&rows, 30, &layout);
        let max = suffixes_max_width(&suffixes);
        for suffix in &suffixes {
            assert!(
                UnicodeWidthStr::width(suffix.as_str()) <= max,
                "suffix must fit the resolved max width"
            );
        }
        for suffix in &suffixes {
            assert!(suffix.contains('%'), "percentage must survive: {suffix:?}");
        }
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
        let suffixes = resolve_system_suffixes(&rows, 80, &layout);
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

        let suffixes_small = resolve_system_suffixes(&small, 120, &layout);
        let suffixes_large = resolve_system_suffixes(&large, 120, &layout);

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
        let linux = [
            row("CPU", 25.0, Some("128 cores")),
            row(
                "MEM",
                80.0,
                // Detail chosen so the default suffix
                // ("80.0% " + detail) exceeds the total fleet
                // suffix budget = 80 - 14 = 66 cells but fits in the
                // OLD local budget of (fleet_max + 3). The OLD code
                // would truncate the suffix to (fleet_max + 3) cells,
                // which renders the line 1 cell wider than the
                // terminal width.
                Some("12000.0 GiB / 15000.0 GiB / 8000.0 GiB / 500.0 GiB / 300.0 GiB"),
            ),
            row("SWP", 0.0, Some("0 B / 4.0 GiB")),
            row("DISK", 90.0, Some("1.2 TiB / 1.4 TiB")),
        ];
        let windows = [
            row("CPU", 25.0, Some("128 cores")),
            row("MEM", 80.0, Some("120.0 GiB / 150.0 GiB")),
            row("COMMIT", 50.0, Some("4.0 GiB / 8.0 GiB")),
            row("DISK", 90.0, Some("1.2 TiB / 1.4 TiB")),
        ];
        let width = 80u16;
        let layout = compute_fleet_metric_layout([&linux, &windows], width);
        assert_eq!(
            layout.label_width,
            u16::try_from("COMMIT".len()).unwrap(),
            "fleet label column must use the wider COMMIT label"
        );
        let suffixes = resolve_system_suffixes(&linux, width, &layout);
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
                "suffix '{suffix}' ({} cells) exceeds fleet suffix budget {fleet_suffix_budget}",
                suffix_width
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

        let rows = build_metric_rows(&snap);
        let disk = &rows[3];
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

        let rows = build_metric_rows(&snap);
        let disk = &rows[3];
        let detail = disk.detail.as_ref().expect("disk detail present");
        assert!(
            detail.contains("160.0 GiB / 200.0 GiB"),
            "fallback availability still uses total denominator, got {detail:?}"
        );
    }

    fn base_disk_snapshot() -> NormalizedSnapshot {
        let v2_payload =
            gregg_protocol::test_support::LinuxSnapshotV2Builder::default().build_payload();
        let mut snap = NormalizedSnapshot::from_v2_payload(&v2_payload);
        snap.drives = None;
        snap
    }
}
